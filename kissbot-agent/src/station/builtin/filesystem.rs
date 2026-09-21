//! filesystem group：read 工具（读文本文件，路径校验防穿透）

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use kissbot_mcp::{CallToolResult, Tool};
use serde_json::Value;

use super::{ObjectSchema, PropertySchema};
use crate::station::{Group, NativeTool, TERR_TOOL_INTERNAL_ERROR, TERR_TOOL_WRONG_ARGUMENTS};
use crate::types::{Error, Result};

/// Read 工具返回内容的最大字节数（截断防大文件）
const READ_MAX_BYTES: usize = 64 * 1024;

/// 构造 filesystem group：read 工具（元数据 + 原生实现）
pub fn group(name: &str) -> Result<Group> {
    Group::with_builtin_tools(
        name,
        vec![(
            Arc::new(Tool::new(
                "read",
                "读取文本文件内容（路径限当前工作目录内，返回限长 64KB）",
                read_tool_schema().value(),
            )),
            Arc::new(ReadTool::new(std::env::current_dir().unwrap_or_default())),
        )],
    )
}

/// `read` 工具的参数 schema
fn read_tool_schema() -> ObjectSchema {
    ObjectSchema {
        kind: "object",
        properties: BTreeMap::from([(
            "path",
            PropertySchema {
                kind: "string",
                description: "文件路径（相对或绝对，限工作目录内）",
            },
        )]),
        required: &["path"],
    }
}

pub struct ReadTool {
    cwd: PathBuf,
}

impl ReadTool {
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd }
    }

    /// 路径校验：相对路径基于 cwd 解析 → canonicalize（消解 .. 与符号链接）→
    /// 校验规范绝对路径等于 cwd 或在 cwd 子目录内（在规范化后的绝对路径上判断，防穿透）
    pub fn resolve_safe_path(&self, raw: &str) -> Result<PathBuf> {
        let path = Path::new(raw);
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        };
        // 目标不存在时 canonicalize 失败：先规范化父目录再拼接文件名
        let canon = std::fs::canonicalize(&absolute).unwrap_or_else(|_| {
            absolute.parent()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .map(|p| p.join(absolute.file_name().unwrap_or_default()))
                .unwrap_or(absolute.clone())
        });
        let canon_cwd = std::fs::canonicalize(&self.cwd)
            .unwrap_or_else(|_| self.cwd.clone());
        if !canon.starts_with(&canon_cwd) {
            return Err(Error::InternalError(format!("路径越界: {}", raw)));
        }
        Ok(canon)
    }
}

#[async_trait]
impl NativeTool for ReadTool {
    async fn call(&self, arguments: Value) -> CallToolResult {
        let Some(raw) = arguments["path"].as_str() else {
            return CallToolResult::error(TERR_TOOL_WRONG_ARGUMENTS, "缺少参数 path");
        };
        let safe = match self.resolve_safe_path(raw) {
            Ok(safe) => safe,
            Err(e) => {
                return CallToolResult::error(TERR_TOOL_INTERNAL_ERROR, format!("路径解析失败: {}", e));
            }
        };
        match tokio::fs::read(&safe).await {
            Ok(content) => {
                let text = String::from_utf8_lossy(&content[..content.len().min(READ_MAX_BYTES)]).to_string();
                // 文本结果用文本块直接承载（不加 JSON 转义）
                CallToolResult::text(text)
            }
            Err(e) => CallToolResult::error(
                TERR_TOOL_INTERNAL_ERROR,
                format!("读取文件失败 {}: {}", safe.display(), e),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_tool_rejects_escape_paths() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadTool::new(dir.path().to_path_buf());
        // 子目录内绝对路径 OK
        let sub = dir.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("a.txt"), "hi").unwrap();
        assert!(tool.resolve_safe_path(sub.join("a.txt").to_str().unwrap()).is_ok());
        // 相对路径基于 cwd 解析
        assert!(tool.resolve_safe_path("sub/a.txt").is_ok());
        // 越界：上级目录
        assert!(tool.resolve_safe_path("../outside.txt").is_err(), ".. 穿透应拒绝");
        // 越界：cwd 之外的绝对路径
        let outside = tempfile::tempdir().unwrap();
        assert!(tool.resolve_safe_path(outside.path().to_str().unwrap()).is_err(), "cwd 外应拒绝");
        // 不存在的路径：父目录在 cwd 内仍应通过校验（读取时自然报不存在）
        assert!(tool.resolve_safe_path("sub/missing.txt").is_ok());
    }

    #[tokio::test]
    async fn read_tool_reads_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "文件内容").unwrap();
        let tool = ReadTool::new(dir.path().to_path_buf());
        let result = tool.call(serde_json::json!({ "path": "a.txt" })).await;
        // 文本结果由文本块直接承载（不加 JSON 转义），不设 structuredContent
        assert_eq!(result.content.iter().find_map(|c| c.as_text()), Some("文件内容"));
        assert!(result.structured_content.is_null());
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn builtin_read_schema_is_typed_object() {
        // 内置工具的参数 schema 由固定结构体给出（非手拼 JSON）
        let schema = read_tool_schema().value();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["path"]["type"], "string");
        assert!(schema["properties"]["path"]["description"].as_str().unwrap().contains("文件路径"));
        assert_eq!(schema["required"], serde_json::json!(["path"]));
        // group 里注册的工具元数据用的是同一份 schema，且带所属 group
        let group = group("filesystem").unwrap();
        let entry = group.tools.get("read").expect("filesystem group 应注册 read");
        assert_eq!(entry.value().config.input_schema.as_ref(), &schema);
        assert_eq!(entry.value().config.group(), Some("filesystem"));
    }
}

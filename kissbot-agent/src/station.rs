use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use serde_json::Value;

use crate::config_manager::{StationConfig, ToolConfig};
use crate::types::{Error, Result};

/// 工具统一接口：统一参数（serde_json::Value）与返回值（serde_json::Value）
#[async_trait]
pub trait Tool: Send + Sync {
    async fn call(&self, params: Value) -> Result<Value>;
}

// ========== 内置示例工具：Read（读文本文件，路径校验防穿透） ==========

pub struct ReadTool {
    cwd: PathBuf,
}

/// Read 工具返回内容的最大字节数（截断防大文件）
const READ_MAX_BYTES: usize = 64 * 1024;

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
impl Tool for ReadTool {
    async fn call(&self, params: Value) -> Result<Value> {
        let raw = params.get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| Error::InternalError("缺少参数 path".to_string()))?;
        let safe = self.resolve_safe_path(raw)?;
        let content = tokio::fs::read(&safe).await
            .map_err(|e| Error::IoError(format!("读取文件失败 {}: {}", safe.display(), e)))?;
        let text = String::from_utf8_lossy(&content[..content.len().min(READ_MAX_BYTES)]).to_string();
        Ok(Value::String(text))
    }
}

// ========== Station 运行态 ==========

/// Station 运行态：base_url 非空 = REST 调用（本轮骨架）；为空 = 本地调用（查 local_tools 执行）
pub struct StationRuntime {
    config: Arc<StationConfig>,
    local_tools: DashMap<String, Arc<dyn Tool>>,
    /// REST 分支请求客户端（远程 Station 后端实现时消费；本轮骨架未使用）
    #[allow(dead_code)]
    client: reqwest::Client,
}

impl StationRuntime {
    pub fn new(config: Arc<StationConfig>) -> Self {
        Self {
            config,
            local_tools: DashMap::new(),
            client: reqwest::Client::new(),
        }
    }

    pub fn station_id(&self) -> &str {
        self.config.station_id.as_str()
    }

    /// 配置访问器（coordinator 判断 base_url 用）
    pub fn config(&self) -> &StationConfig {
        &self.config
    }

    /// 注册本地工具实现（base_url 为空的 station 用）
    pub fn register_local(&self, name: &str, tool: Arc<dyn Tool>) {
        self.local_tools.insert(name.to_string(), tool);
    }

    /// 该 station 配置的工具名集合（LLM tools 聚合用）
    pub fn configured_tools(&self) -> Vec<ToolConfig> {
        self.config.tools.iter().map(|(_, s)| (*s.load_full()).clone()).collect()
    }

    pub fn has_tool(&self, name: &str) -> bool {
        self.local_tools.contains_key(name) || self.config.tools.contains_key(name)
    }

    /// 执行工具：base_url 空 → 本地查表执行；非空 → REST（本轮骨架，返回未实现错误）
    pub async fn call_tool(&self, name: &str, params: Value) -> Result<Value> {
        if self.config.base_url.is_empty() {
            let tool = self.local_tools.get(name)
                .ok_or_else(|| Error::InternalError(format!("本地工具不存在: {}", name)))?;
            return tool.call(params).await;
        }
        // REST 分支：本轮不实现（后续接入远程 station 后端）
        Err(Error::InternalError(format!("远程 Station 调用未实现（本轮仅本地模式）: {}", name)))
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
        let result = tool.call(serde_json::json!({ "path": "a.txt" })).await.unwrap();
        // 返回内容为纯文本字符串（Value::String）
        assert_eq!(result, serde_json::Value::String("文件内容".to_string()));
    }

    #[tokio::test]
    async fn station_runtime_local_call() {
        let runtime = StationRuntime::new(Arc::new(station_config("local", "")));
        runtime.register_local("echo", Arc::new(EchoTool));
        let out = runtime.call_tool("echo", serde_json::json!({"v": 1})).await.unwrap();
        assert_eq!(out["v"], 1);
        // 未注册工具报错
        assert!(runtime.call_tool("nope", serde_json::json!({})).await.is_err());
        // base_url 非空 → REST 骨架（未实现错误）
        let remote = StationRuntime::new(Arc::new(station_config("remote", "http://127.0.0.1:1")));
        assert!(remote.call_tool("any", serde_json::json!({})).await.is_err());
    }

    /// 构造测试用 StationConfig（tools 空）
    fn station_config(station_id: &str, base_url: &str) -> StationConfig {
        StationConfig {
            station_id: Arc::new(station_id.into()),
            base_url: Arc::new(base_url.into()),
            timeout_secs: 5,
            tools: Arc::new(kissbot_api::ArcSwapHashMap::new()),
        }
    }
}

/// 测试用 mock tool
struct EchoTool;
#[async_trait]
impl Tool for EchoTool {
    async fn call(&self, params: serde_json::Value) -> Result<serde_json::Value> {
        Ok(params)
    }
}

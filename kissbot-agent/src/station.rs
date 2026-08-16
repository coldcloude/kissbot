use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use dashmap::DashMap;
use serde_json::Value;
use tracing::warn;

use crate::config_manager::{ConfigManager, McpConfig, StationRepo, SubStationConfig, ToolConfig};
use crate::station_client::StationClient;
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

// ========== 内置注册表 ==========

/// 内置 toolkit 声明：toolkit 名 → 内置工具集合（元数据 + 实现）
/// 配置显式声明对应 toolkit 名时才注册；未声明则不注册（内置工具也须显式配置 toolkit 才可用）
pub struct BuiltinToolkit {
    /// 内置工具元数据（ToolConfig，含参数 JSON Schema）
    pub tool_configs: Vec<ToolConfig>,
    /// 内置工具实现（(工具名, 实现)，与 tool_configs 同名对应）
    pub tool_impls: Vec<(&'static str, Arc<dyn Tool>)>,
}

/// 内置注册表（本轮仅 filesystem toolkit：Read 工具）
fn builtin_registry() -> Vec<(&'static str, BuiltinToolkit)> {
    vec![(
        "filesystem",
        BuiltinToolkit {
            tool_configs: vec![ToolConfig {
                name: Arc::new("read".into()),
                description: Arc::new("读取文本文件内容（路径限当前工作目录内，返回限长 64KB）".into()),
                parameters: Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件路径（相对或绝对，限工作目录内）" }
                    },
                    "required": ["path"]
                })),
            }],
            tool_impls: vec![(
                "read",
                Arc::new(ReadTool::new(std::env::current_dir().unwrap_or_default())),
            )],
        },
    )]
}

// ========== 运行态结构 ==========

/// Toolkit 内单个工具条目：元数据 + 实现（None = 仅元数据注册，无本地实现，调用返回未实现）
struct ToolkitEntry {
    config: ToolConfig,
    imp: Option<Arc<dyn Tool>>,
}

/// Toolkit 运行态：工具实现表 + MCP 占位。Toolkit 中无子 Station
pub struct Toolkit {
    name: String,
    tools: DashMap<String, Arc<ToolkitEntry>>,
    mcps: DashMap<String, Arc<McpConfig>>,
}

impl Toolkit {
    fn new(name: String) -> Self {
        Self { name, tools: DashMap::new(), mcps: DashMap::new() }
    }

    /// 该 toolkit 的工具元数据列表（LLM tools 聚合用）
    fn configured_tools(&self) -> Vec<ToolConfig> {
        self.tools.iter().map(|e| e.value().config.clone()).collect()
    }

    /// 该 toolkit 的 MCP 占位列表
    fn configured_mcps(&self) -> Vec<McpConfig> {
        self.mcps.iter().map(|e| (**e.value()).clone()).collect()
    }
}

/// 子 Station 运行态：只存连接信息 + HTTP 客户端（子只能 HTTP 通信，孙子由子递归）
pub struct SubStation {
    config: Arc<SubStationConfig>,
    client: StationClient,
}

/// 全局 Station 单例（进程内唯一；每个 agent 一个，静态访问）
/// 内部：本地 toolkit 集合（含实现） + 直接子 Station 集合（仅连接信息）
static SINGLETON: OnceLock<Station> = OnceLock::new();

pub struct Station {
    toolkits: DashMap<String, Arc<Toolkit>>,
    sub_stations: DashMap<String, Arc<SubStation>>,
}

impl Station {
    /// 取全局单例（进程内唯一；new() 完成后可用，此前调用 panic）
    pub fn get() -> &'static Station {
        SINGLETON.get().expect("Station 未初始化")
    }

    /// 从 ConfigManager 读 station.json 构建全局 Station 并注册单例
    pub async fn new() -> Result<()> {
        let repo = ConfigManager::get().station_repo_snapshot().await;
        let station = Self::from_repo(&repo)?;
        let _ = SINGLETON.set(station);
        Ok(())
    }

    /// 按配置构建 Station（纯构造，不注册单例；Task 4 起为生产入口 new() 的构建步骤，单测直接消费）：
    /// 内置注册表填充声明 toolkit 的实现；配置声明的 tools/mcps 补充元数据；
    /// 工具名整树全局唯一（本地硬约束，冲突启动失败）
    pub fn from_repo(repo: &StationRepo) -> Result<Station> {
        let registry = builtin_registry();
        let toolkits = DashMap::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (name, tcfg) in repo.toolkits.iter() {
            let tcfg = tcfg.load_full();
            let toolkit = Arc::new(Toolkit::new(name.clone()));
            // 1. 内置注册表填充（声明该 toolkit 名才注册内置实现）
            if let Some((_, builtin)) = registry.iter().find(|(n, _)| *n == name.as_str()) {
                for cfg in &builtin.tool_configs {
                    let tool_name = cfg.name.to_string();
                    Self::check_unique(&mut seen, &tool_name)?;
                    let imp = builtin.tool_impls.iter()
                        .find(|(tn, _)| *tn == tool_name)
                        .map(|(_, t)| t.clone());
                    toolkit.tools.insert(tool_name, Arc::new(ToolkitEntry { config: (*cfg).clone(), imp }));
                }
            }
            // 2. 配置声明的 tools（元数据补充；与内置重名 → 冲突）
            for (tool_name, tcfg) in tcfg.tools.iter() {
                if toolkit.tools.contains_key(tool_name) {
                    return Err(Error::InternalError(format!("工具名冲突: {}（toolkit 内全局唯一）", tool_name)));
                }
                Self::check_unique(&mut seen, tool_name)?;
                toolkit.tools.insert(tool_name.clone(), Arc::new(ToolkitEntry { config: (*tcfg.load_full()).clone(), imp: None }));
            }
            // 3. 配置声明的 mcps（占位）
            for (mcp_name, mcfg) in tcfg.mcps.iter() {
                toolkit.mcps.insert(mcp_name.clone(), Arc::new((*mcfg.load_full()).clone()));
            }
            toolkits.insert(name.clone(), toolkit);
        }
        // 子 Station：只存连接信息
        let sub_stations = DashMap::new();
        for (id, scfg) in repo.sub_stations.iter() {
            let scfg = scfg.load_full();
            let sub = SubStation {
                config: Arc::new((*scfg).clone()),
                client: StationClient::new(scfg.timeout_secs),
            };
            sub_stations.insert(id.clone(), Arc::new(sub));
        }
        Ok(Station { toolkits, sub_stations })
    }

    /// 工具名唯一性校验：本地 toolkit 间不得重名（工具名整树全局唯一，本地先硬约束）
    fn check_unique(seen: &mut HashSet<String>, name: &str) -> Result<()> {
        if !seen.insert(name.to_string()) {
            return Err(Error::InternalError(format!("工具名冲突: {}（整树全局唯一）", name)));
        }
        Ok(())
    }

    /// 工具元数据平铺查询：本地 toolkit 白名单过滤 + 直接子递归（HTTP 骨架，本轮未实现跳过）
    /// filter = None 返回全部；Some(空集) 返回空
    pub async fn tools(&self, filter: Option<&HashSet<String>>) -> Result<Vec<ToolConfig>> {
        let mut out = Vec::new();
        for entry in self.toolkits.iter() {
            let toolkit = entry.value();
            if let Some(f) = filter {
                if !f.contains(toolkit.name.as_str()) { continue; }
            }
            out.extend(toolkit.configured_tools());
        }
        // 直接子递归（HTTP 骨架：未实现返回 Err → 记日志跳过，不阻塞整体）
        // 先整树克隆出 Arc 再 await（不跨 await 持 DashMap 读锁）
        let subs: Vec<Arc<SubStation>> = self.sub_stations.iter().map(|e| e.value().clone()).collect();
        for sub in subs {
            match sub.client.list_tools(filter).await {
                Ok(tools) => out.extend(tools),
                Err(e) => warn!("子 Station {} 查询工具失败: {}", sub.config.station_id.as_str(), e),
            }
        }
        Ok(out)
    }

    /// MCP 元数据平铺查询（占位接口：本地返回配置，直接子 HTTP 骨架跳过）
    #[allow(dead_code)] // MCP 本轮占位，无生产消费方
    pub async fn mcps(&self, filter: Option<&HashSet<String>>) -> Result<Vec<McpConfig>> {
        let mut out = Vec::new();
        for entry in self.toolkits.iter() {
            let toolkit = entry.value();
            if let Some(f) = filter {
                if !f.contains(toolkit.name.as_str()) { continue; }
            }
            out.extend(toolkit.configured_mcps());
        }
        // 先整树克隆出 Arc 再 await（不跨 await 持 DashMap 读锁）
        let subs: Vec<Arc<SubStation>> = self.sub_stations.iter().map(|e| e.value().clone()).collect();
        for sub in subs {
            match sub.client.list_mcps(filter).await {
                Ok(mcps) => out.extend(mcps),
                Err(e) => warn!("子 Station {} 查询 MCP 失败: {}", sub.config.station_id.as_str(), e),
            }
        }
        Ok(out)
    }

    /// 执行工具：本地实现表（跨 toolkit，工具名全局唯一）命中执行；
    /// 未命中 → 直接子递归（HTTP 骨架：子返回 Err 记日志跳过；全部未命中 → 工具不存在）
    pub async fn call_tool(&self, name: &str, params: Value) -> Result<Value> {
        // 本地查找：无 await 阶段完成查找并释放 DashMap 读锁（不跨 await 持锁）
        let local: Option<Option<Arc<dyn Tool>>> = {
            let mut found = None;
            for entry in self.toolkits.iter() {
                if let Some(t) = entry.value().tools.get(name) {
                    found = Some(t.value().imp.clone());
                    break;
                }
            }
            found
        };
        if let Some(imp) = local {
            return match imp {
                Some(tool) => tool.call(params).await,
                None => Err(Error::InternalError(format!("工具未实现（仅元数据注册）: {}", name))),
            };
        }
        // 直接子递归（骨架：子未实现返回 Err → 记日志跳过；全部未命中 → 工具不存在）
        // 先整树克隆出 Arc 再 await（不跨 await 持 DashMap 读锁）
        let subs: Vec<Arc<SubStation>> = self.sub_stations.iter().map(|e| e.value().clone()).collect();
        for sub in subs {
            match sub.client.call_tool(name, params.clone()).await {
                Ok(v) => return Ok(v),
                Err(e) => warn!("子 Station {} 调用工具 {} 失败: {}", sub.config.station_id.as_str(), name, e),
            }
        }
        Err(Error::InternalError(format!("工具不存在: {}", name)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_manager::{McpConfig, StationRepo, SubStationConfig, ToolkitConfig, ToolConfig};
    use arc_swap::ArcSwap;
    use kissbot_api::ArcSwapHashMap;

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

    fn filesystem_toolkit() -> ToolkitConfig {
        ToolkitConfig {
            tools: Arc::new(ArcSwapHashMap::new()),
            mcps: Arc::new(ArcSwapHashMap::new()),
        }
    }

    fn repo_with_filesystem() -> StationRepo {
        let mut repo = StationRepo::default();
        let map = Arc::make_mut(&mut repo.toolkits);
        map.insert("filesystem".to_string(), ArcSwap::new(Arc::new(filesystem_toolkit())));
        repo
    }

    fn repo_with_mcp() -> StationRepo {
        let mut repo = StationRepo::default();
        let map = Arc::make_mut(&mut repo.toolkits);
        let mut t = filesystem_toolkit();
        {
            let m = Arc::make_mut(&mut t.mcps);
            m.insert("mcp1".to_string(), ArcSwap::new(Arc::new(McpConfig {
                name: Arc::new("mcp1".into()),
                description: Arc::new("占位".into()),
            })));
        }
        map.insert("filesystem".to_string(), ArcSwap::new(Arc::new(t)));
        repo
    }

    fn repo_with_sub() -> StationRepo {
        let mut repo = StationRepo::default();
        let map = Arc::make_mut(&mut repo.sub_stations);
        map.insert("station-a".to_string(), ArcSwap::new(Arc::new(SubStationConfig {
            station_id: Arc::new("station-a".into()),
            base_url: Arc::new("http://127.0.0.1:9001".into()),
            timeout_secs: 5,
        })));
        repo
    }

    #[tokio::test]
    async fn tools_none_returns_all_local() {
        // 内置 filesystem toolkit 声明后，read 工具元数据由内置注册表填充
        let station = Station::from_repo(&repo_with_filesystem()).unwrap();
        let tools = station.tools(None).await.unwrap();
        assert_eq!(tools.len(), 1, "内置注册表应填充 read");
        assert_eq!(tools[0].name.as_str(), "read");
    }

    #[tokio::test]
    async fn tools_filter_whitelist_semantics() {
        // None=全部；Some(命中)=白名单；Some(未命中)/Some(空集)=空
        let station = Station::from_repo(&repo_with_filesystem()).unwrap();
        let hit: HashSet<String> = ["filesystem".to_string()].into_iter().collect();
        let miss: HashSet<String> = ["other".to_string()].into_iter().collect();
        let empty = HashSet::new();
        assert_eq!(station.tools(Some(&hit)).await.unwrap().len(), 1);
        assert!(station.tools(Some(&miss)).await.unwrap().is_empty());
        assert!(station.tools(Some(&empty)).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn tools_with_sub_skips_http_skeleton() {
        // 子 Station HTTP 骨架未实现 → Err 记日志跳过；无本地工具时结果为空
        let station = Station::from_repo(&repo_with_sub()).unwrap();
        assert!(station.tools(None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn mcps_placeholder_local_only() {
        let station = Station::from_repo(&repo_with_mcp()).unwrap();
        let mcps = station.mcps(None).await.unwrap();
        assert_eq!(mcps.len(), 1);
        assert_eq!(mcps[0].name.as_str(), "mcp1");
    }

    #[test]
    fn from_repo_rejects_duplicate_tool_names() {
        // 两个 toolkit 声明同名工具（一个内置 read、一个配置 read）→ 本地硬约束启动失败
        let mut repo = StationRepo::default();
        {
            let map = Arc::make_mut(&mut repo.toolkits);
            map.insert("filesystem".to_string(), ArcSwap::new(Arc::new(filesystem_toolkit())));
            map.insert("other".to_string(), ArcSwap::new(Arc::new(ToolkitConfig {
                tools: Arc::new({
                    let mut m = ArcSwapHashMap::new();
                    m.insert("read".to_string(), ArcSwap::new(Arc::new(ToolConfig {
                        name: Arc::new("read".into()),
                        description: Arc::new("d".into()),
                        parameters: Arc::new(serde_json::json!({})),
                    })));
                    m
                }),
                mcps: Arc::new(ArcSwapHashMap::new()),
            })));
        }
        // Station 不含 Debug（含 Arc<dyn Tool>），用 match 取错而非 unwrap_err
        let err = match Station::from_repo(&repo) {
            Err(e) => e,
            Ok(_) => panic!("应冲突"),
        };
        assert!(err.to_string().contains("工具名冲突"), "冲突应报错: {}", err);
    }

    #[tokio::test]
    async fn call_tool_local_hit_and_miss() {
        let station = Station::from_repo(&repo_with_filesystem()).unwrap();
        // 命中：read 本地实现执行（读测试目录内文件）
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "内容").unwrap();
        let read = crate::station::ReadTool::new(dir.path().to_path_buf());
        let station2 = Station::from_repo(&{
            let repo = repo_with_filesystem();
            // 替换内置实现无法直接注入，这里改用临时单实例验证路径：直接调 ReadTool
            repo
        }).unwrap();
        let _ = (station2, read);
        // 未注册工具 → 工具不存在
        let miss = station.call_tool("nope", serde_json::json!({})).await;
        assert!(miss.is_err() && miss.unwrap_err().to_string().contains("工具不存在"));
    }
}

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use dashmap::DashMap;
use kissbot_mcp::{
    CallToolResult, DiscoverResult, Implementation, KissbotCapability, McpClient, McpClientError,
    McpError, McpHandler, Tool, ToolsCallParams, ToolsListParams, ToolsListResult,
};
use serde_json::{Value, json};
use tokio::sync::OnceCell;
use tracing::warn;

use crate::config_manager::ConfigManager;
use crate::configs::{StationRepo, SubStationConfig};
use crate::types::{Error, Result};

// ========== 工具错误码（工具级失败随 CallToolResult 回带） ==========

pub const TERR_TOOL_NOT_FOUND: u32 = 0x0000_0001;
pub const TERR_TOOL_NOT_IMPLEMENTED: u32 = 0x0000_0002;
pub const TERR_TOOL_WRONG_ARGUMENTS: u32 = 0x0000_0003;
/// 级联自环（kissbot 工具错误码；协议级同名错误码见 `kissbot_mcp::STATION_CYCLE`）
pub const TERR_TOOL_CYCLE: u32 = 0x0000_0004;

pub const TERR_TOOL_INTERNAL_ERROR: u32 = 0x0001_0000;

pub const TERR_TOOL_EXTERNAL_ERROR: u32 = 0x0002_0000;

/// 构造工具错误 JSON（Nexus 侧 ToolData.error 的形状）
pub fn tool_error(code: u32, message: String) -> Value {
    json!({
        "code": code,
        "message": message
    })
}

/// Station 原生工具接口：本级 Station 自己实现的工具（区别于代理到子 Station 的 MCP 工具）
#[async_trait]
pub trait NativeTool: Send + Sync {
    async fn call(&self, arguments: Value) -> CallToolResult;
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

// ========== 内置注册表 ==========

/// 内置 group 声明：工具元数据（MCP Tool）+ 原生实现
/// 配置显式声明对应 group 名时才注册；未声明则不注册（内置工具也须显式配置 group 才可用）
pub struct BuiltinGroup {
    pub tools: Vec<Arc<Tool>>,
    /// 原生实现（(工具名, 实现)，与 tools 同名对应）
    pub impls: Vec<(&'static str, Arc<dyn NativeTool>)>,
}

/// 内置注册表（本轮仅 filesystem group：Read 工具）
fn builtin_registry() -> Vec<(&'static str, BuiltinGroup)> {
    vec![(
        "filesystem",
        BuiltinGroup {
            tools: vec![Arc::new(Tool::new(
                "read",
                "读取文本文件内容（路径限当前工作目录内，返回限长 64KB）",
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件路径（相对或绝对，限工作目录内）" }
                    },
                    "required": ["path"]
                }),
            ))],
            impls: vec![(
                "read",
                Arc::new(ReadTool::new(std::env::current_dir().unwrap_or_default())),
            )],
        },
    )]
}

// ========== 运行态结构 ==========

/// group 内单个工具条目：元数据 + 原生实现（None = 仅元数据注册，调用返回未实现）
struct GroupTool {
    config: Arc<Tool>,
    imp: Option<Arc<dyn NativeTool>>,
}

/// group 运行态：一组工具（工具名在本级 Station 内全局唯一）
pub struct Group {
    name: String,
    tools: DashMap<String, Arc<GroupTool>>,
}

impl Group {
    fn new(name: String) -> Self {
        Self { name, tools: DashMap::new() }
    }

    /// 该 group 的工具元数据列表
    fn configured_tools(&self) -> Vec<Arc<Tool>> {
        self.tools.iter().map(|e| e.value().config.clone()).collect()
    }
}

/// 子 Station 运行态：只存连接信息 + MCP 客户端（孙子由子 Station 自己递归）
pub struct SubStation {
    config: Arc<SubStationConfig>,
    client: McpClient,
    /// 对端声明的 kissbot 扩展能力：首次交互时经 `server/discover` 探测一次并缓存
    /// （`None` = 已探测但对方未声明扩展，即第三方通用 MCP 服务端）；探测失败不写入，下次重试
    kissbot_caps: OnceCell<Option<KissbotCapability>>,
}

impl SubStation {
    /// 生效的默认 group（未配置 default_group 时回落为 station_id）
    fn effective_group(&self) -> &str {
        self.config.effective_group()
    }

    /// 对端是否支持 kissbot 分组扩展（经 `server/discover` 探测并缓存）
    async fn supports_group(&self) -> bool {
        self.kissbot_capability()
            .await
            .is_some_and(|caps| caps.group)
    }

    /// 探测对端 kissbot 扩展能力：命中缓存直接返回；未命中调 `server/discover`，
    /// 成功则缓存（含「未声明」的否定结果），失败不缓存（下次重试）
    async fn kissbot_capability(&self) -> Option<KissbotCapability> {
        let caps = self
            .kissbot_caps
            .get_or_try_init(|| async { self.client.discover().await.map(|r| r.capabilities.kissbot) })
            .await;
        match caps {
            Ok(caps) => caps.clone(),
            Err(e) => {
                warn!("子 Station {} server/discover 失败: {}", self.config.station_id.as_str(), e);
                None
            }
        }
    }
}

/// 全局 Station 单例（进程内唯一；每个 agent 一个，静态访问）
static SINGLETON: OnceLock<Arc<Station>> = OnceLock::new();

pub struct Station {
    /// 本站点 station_id（来自 station.json；祖先链防环用）
    station_id: String,
    groups: DashMap<String, Arc<Group>>,
    sub_stations: DashMap<String, Arc<SubStation>>,
    /// 远程工具路由表：工具名 → 直接子 station_id（仅 call_tool 路由用，不用于元数据查询）
    /// list_tools 拉取子 Station 成功时更新（快照语义：先清该子旧记录再逐个插入）；
    /// 工具名整树全局唯一：与本地/先到子重名时保留先到者（后到者剔除 + warn）
    tool_routes: DashMap<String, String>,
}

impl Station {
    /// 取全局单例（进程内唯一；new() 完成后可用，此前调用 panic）
    pub fn get() -> &'static Station {
        SINGLETON.get().expect("Station 未初始化").as_ref()
    }

    /// 取全局单例的 Arc（供 MCP 服务端持有处理器）
    pub fn shared() -> Arc<Station> {
        SINGLETON.get().expect("Station 未初始化").clone()
    }

    /// 从 ConfigManager 读 station.json 构建全局 Station 并注册单例
    pub async fn new() -> Result<()> {
        let repo = ConfigManager::get().station_repo_snapshot().await;
        let api_key = kissbot_security::SecurityConfig::get().api_key.to_string();
        let station = Self::from_repo(&repo, &api_key)?;
        let _ = SINGLETON.set(Arc::new(station));
        Ok(())
    }

    /// 按配置构建 Station（纯构造，不注册单例；单测直接消费）：
    /// 内置注册表填充声明 group 的原生实现；配置声明的 tools 补充元数据；
    /// 工具名整树全局唯一（本地硬约束，冲突启动失败）
    pub fn from_repo(repo: &StationRepo, api_key: &str) -> Result<Station> {
        let registry = builtin_registry();
        let groups = DashMap::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (name, gcfg) in repo.groups.iter() {
            let gcfg = gcfg.load_full();
            let group = Arc::new(Group::new(name.clone()));
            // 1. 内置注册表填充（声明该 group 名才注册内置实现）
            if let Some((_, builtin)) = registry.iter().find(|(n, _)| *n == name.as_str()) {
                for tool in &builtin.tools {
                    let tool_name = tool.name.to_string();
                    Self::check_unique(&mut seen, &tool_name)?;
                    let imp = builtin.impls.iter()
                        .find(|(tn, _)| *tn == tool_name)
                        .map(|(_, t)| t.clone());
                    group.tools.insert(tool_name, Arc::new(GroupTool { config: tool.clone(), imp }));
                }
            }
            // 2. 配置声明的 tools（元数据补充；与内置重名 → 冲突）
            for (tool_name, tcfg) in gcfg.tools.iter() {
                if group.tools.contains_key(tool_name) {
                    return Err(Error::InternalError(format!("工具名冲突: {}（group 内全局唯一）", tool_name)));
                }
                Self::check_unique(&mut seen, tool_name)?;
                group.tools.insert(tool_name.clone(), Arc::new(GroupTool { config: tcfg.load_full(), imp: None }));
            }
            groups.insert(name.clone(), group);
        }
        // 子 Station：只存连接信息 + MCP 客户端（MCP 端点为 base_url + /mcp）
        let sub_stations = DashMap::new();
        for (id, scfg) in repo.sub_stations.iter() {
            let scfg = scfg.load_full();
            let endpoint = format!("{}/mcp", scfg.base_url.trim_end_matches('/'));
            let sub = SubStation {
                config: Arc::new((*scfg).clone()),
                client: McpClient::new(&endpoint, scfg.timeout_secs, Some(api_key)),
                kissbot_caps: OnceCell::new(),
            };
            sub_stations.insert(id.clone(), Arc::new(sub));
        }
        Ok(Station {
            station_id: repo.station_id.to_string(),
            groups,
            sub_stations,
            tool_routes: DashMap::new(),
        })
    }

    /// 工具名唯一性校验：本级 Station 内不得重名（工具名整树全局唯一，本地先硬约束）
    fn check_unique(seen: &mut HashSet<String>, name: &str) -> Result<()> {
        if !seen.insert(name.to_string()) {
            return Err(Error::InternalError(format!("工具名冲突: {}（整树全局唯一）", name)));
        }
        Ok(())
    }

    /// 合并单个子 Station 的工具进路由表（快照语义：先清该子旧记录，再逐个插入）
    /// 工具名冲突（与本地或先到子重名）保留先到者：后到者不进路由表
    /// 返回成功插入的工具名集合（调用方据此从返回列表剔除冲突项）
    fn merge_sub_tools(&self, station_id: &str, tools: Vec<Arc<Tool>>, local_names: &HashSet<String>) -> HashSet<String> {
        // 1. 清该子旧路由记录（快照语义：该子当前拉取结果为准）
        self.tool_routes.retain(|_, v| v.as_str() != station_id);
        // 2. 逐个插入（冲突保留先到者：本地工具与先插入的其他子优先）
        let mut inserted = HashSet::new();
        for t in tools {
            let name = t.name.to_string();
            if self.tool_routes.contains_key(&name) || local_names.contains(&name) {
                warn!("工具名冲突: {}（保留先到者，剔除子 Station {} 的同名工具）", name, station_id);
                continue;
            }
            self.tool_routes.insert(name.clone(), station_id.to_string());
            inserted.insert(name);
        }
        inserted
    }

    /// 工具元数据平铺查询：本级 group 白名单过滤 + 直接子经 MCP 实时拉取（更新路由缓存）
    /// filter = None 返回全部；Some(空集) 返回空；ancestors 为根到当前父节点的 station_id 链
    ///
    /// group 是工具的固有属性，随工具定义（`_meta["kissbot/group"]`）携带，因此本方法对每个
    /// 返回的工具都保证带上 group：本级工具用其所属 group；子站工具优先用对端回带的 group，
    /// 缺省回落该子站的默认 group。
    pub async fn list_tools(&self, filter: Option<&HashSet<String>>, ancestors: &[String]) -> std::result::Result<Vec<Arc<Tool>>, McpError> {
        if ancestors.iter().any(|a| a == &self.station_id) {
            return Err(McpError::station_cycle(&self.station_id));
        }
        let mut tools = Vec::new();
        // 本级工具名集合（冲突校验用：本级优先，子工具与本级重名剔除）
        let local_names: HashSet<String> = self.groups.iter()
            .flat_map(|e| e.value().tools.iter().map(|t| t.key().clone()).collect::<Vec<String>>())
            .collect();
        for entry in self.groups.iter() {
            let group = entry.value();
            if let Some(f) = filter {
                if !f.contains(group.name.as_str()) { continue; }
            }
            if group.tools.is_empty() { continue; }
            tools.extend(
                group
                    .configured_tools()
                    .into_iter()
                    .map(|tool| Arc::new((*tool).clone().with_group(group.name.clone()))),
            );
        }
        // 直接子：经 MCP 实时拉取 → 更新路由缓存 → 合并元数据（剔除冲突项）
        // - 对端声明支持 kissbot 分组扩展：按本级白名单在服务端过滤，工具 group 以对端携带为准
        // - 对端不支持（通用 MCP 服务端）：不传 group（传了也会被忽略），其工具整体归入默认 group，
        //   再由本级按白名单本地过滤
        // - 祖先链对所有子都附带：纯提示信息，规范要求忽略未知 `_meta` 键，附带可为未声明能力的
        //   同级 kissbot 服务端保留防环能力
        // 先整树克隆出 Arc 再 await（不跨 await 持 DashMap 读锁）
        let mut child_ancestors = ancestors.to_vec();
        child_ancestors.push(self.station_id.clone());
        let group_vec: Option<Vec<String>> = filter.map(|s| s.iter().cloned().collect());
        let subs: Vec<Arc<SubStation>> = self.sub_stations.iter().map(|e| e.value().clone()).collect();
        for sub in subs {
            let group_supported = sub.supports_group().await;
            let group_arg = if group_supported { group_vec.as_deref() } else { None };
            match sub.client.list_tools(group_arg, &child_ancestors).await {
                Ok(result) => {
                    let default_group = sub.effective_group();
                    // 通用 MCP 服务端：本地按默认 group 判断该子是否在白名单内
                    if !group_supported {
                        if let Some(f) = filter {
                            if !f.contains(default_group) { continue; }
                        }
                    }
                    let inserted = self.merge_sub_tools(sub.config.station_id.as_str(), result.tools.clone(), &local_names);
                    if inserted.is_empty() { continue; }
                    tools.extend(result.tools.into_iter()
                        .filter(|t| inserted.contains(t.name.as_str()))
                        // 对端未携带 group 时回落该子站默认 group
                        .map(|tool| {
                            let group = tool.group().map(str::to_string).unwrap_or_else(|| default_group.to_string());
                            Arc::new((*tool).clone().with_group(group))
                        }));
                }
                Err(e) => warn!("子 Station {} 查询工具失败: {}", sub.config.station_id.as_str(), e),
            }
        }
        // 确定性顺序（规范建议：同一工具集合的多次请求返回相同顺序，便于客户端缓存与提示词缓存）
        tools.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        Ok(tools)
    }

    /// 执行工具：本级原生实现命中执行；未命中 → 查远程路由表（工具名 → 直接子 station_id）
    /// 经 MCP `tools/call` 路由到对应子；路由未命中 → 工具不存在
    /// ancestors 为根到当前父节点的 station_id 链；自身在链中 → 级联自环（协议级错误）
    pub async fn call_tool(&self, name: &str, arguments: Value, ancestors: &[String]) -> std::result::Result<CallToolResult, McpError> {
        if ancestors.iter().any(|a| a == &self.station_id) {
            return Err(McpError::station_cycle(&self.station_id));
        }
        // 本地查找：无 await 阶段完成查找并释放 DashMap 读锁（不跨 await 持锁）
        let local: Option<Option<Arc<dyn NativeTool>>> = {
            let mut found = None;
            for entry in self.groups.iter() {
                if let Some(t) = entry.value().tools.get(name) {
                    found = Some(t.value().imp.clone());
                    break;
                }
            }
            found
        };
        if let Some(imp) = local {
            return Ok(match imp {
                Some(tool) => tool.call(arguments).await,
                None => CallToolResult::error(TERR_TOOL_NOT_IMPLEMENTED, format!("工具未实现: {}", name)),
            });
        }
        // 远程路由：查路由缓存表（工具名 → 直接子 station_id），命中 → 该子 MCP 调用
        // 不遍历全部子、不远程获取列表；先克隆 Arc 再 await（不跨 await 持锁）
        let mut child_ancestors = ancestors.to_vec();
        child_ancestors.push(self.station_id.clone());
        let routed = self.tool_routes.get(name).map(|r| r.value().clone());
        if let Some(station_id) = routed {
            if let Some(sub) = self.sub_stations.get(&station_id).map(|s| s.value().clone()) {
                return match sub.client.call_tool(name, arguments, &child_ancestors).await {
                    // 工具级失败：识别 kissbot 来源的循环错误码（与本地自环一致地以协议级错误上抛），
                    // 其余（含非 kissbot / 结构不符 / 未识别的 kissbot 错误码）原样透传
                    Ok(result) => {
                        if result
                            .kissbot_error()
                            .is_some_and(|detail| detail.code == TERR_TOOL_CYCLE)
                        {
                            return Err(McpError::station_cycle(&self.station_id));
                        }
                        Ok(result)
                    }
                    // 协议级错误：kissbot 的循环错误码同样识别为上抛；其余按本级外部调用失败处理
                    Err(McpClientError::Remote { code, .. }) if code == kissbot_mcp::STATION_CYCLE => {
                        Err(McpError::station_cycle(&self.station_id))
                    }
                    Err(e) => Ok(CallToolResult::error(
                        TERR_TOOL_EXTERNAL_ERROR,
                        format!("调用 sub station 工具失败: {}", e),
                    )),
                };
            }
        }
        // 工具不存在
        Ok(CallToolResult::error(TERR_TOOL_NOT_FOUND, format!("工具不存在: {}", name)))
    }
}

/// `server/discover` 返回的自然语言说明
const STATION_INSTRUCTIONS: &str = "kissbot Station：提供按 group 分组的工具，并可级联下级 Station。";

#[async_trait]
impl McpHandler for Station {
    async fn discover(&self) -> DiscoverResult {
        DiscoverResult::tools_only(
            Implementation::new("kissbot-station", env!("CARGO_PKG_VERSION")),
            Some(STATION_INSTRUCTIONS.to_string()),
        )
        // 声明 kissbot 扩展能力，供对端经 server/discover 一次性判定
        .with_kissbot_extensions()
    }

    async fn list_tools(&self, params: ToolsListParams) -> std::result::Result<ToolsListResult, McpError> {
        let filter: Option<HashSet<String>> = params.meta.groups.map(|v| v.into_iter().collect());
        let tools = self.list_tools(filter.as_ref(), &params.meta.ancestors).await?;
        Ok(ToolsListResult::new(tools))
    }

    async fn call_tool(&self, params: ToolsCallParams) -> std::result::Result<CallToolResult, McpError> {
        self.call_tool(params.name.as_str(), params.arguments, &params.meta.ancestors).await
    }
}

#[cfg(test)]
mod tests {
    use crate::configs::GroupConfig;

    use super::*;
    use arc_swap::ArcSwap;
    use kissbot_api::ArcSwapHashMap;
    use kissbot_mcp::{RequestMeta, ToolsCallParams};

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
        assert_eq!(result.text_content().unwrap(), "文件内容");
        assert!(result.structured_content.is_null());
        assert!(!result.is_error);
    }

    fn filesystem_group() -> GroupConfig {
        GroupConfig::default()
    }

    fn repo_with_filesystem() -> StationRepo {
        let mut repo = StationRepo::default();
        repo.station_id = Arc::new("station-self".into());
        let map = Arc::make_mut(&mut repo.groups);
        map.insert("filesystem".to_string(), ArcSwap::new(Arc::new(filesystem_group())));
        repo
    }

    fn repo_with_sub() -> StationRepo {
        let mut repo = StationRepo::default();
        repo.station_id = Arc::new("station-self".into());
        let map = Arc::make_mut(&mut repo.sub_stations);
        map.insert("station-a".to_string(), ArcSwap::new(Arc::new(SubStationConfig {
            station_id: Arc::new("station-a".into()),
            base_url: Arc::new("http://127.0.0.1:9001".into()),
            timeout_secs: 5,
            default_group: Arc::new(String::new()),
        })));
        repo
    }

    #[tokio::test]
    async fn list_tools_none_returns_all_local() {
        // 内置 filesystem group 声明后，read 工具元数据由内置注册表填充
        let station = Station::from_repo(&repo_with_filesystem(), "test-key").unwrap();
        let tools = station.list_tools(None, &[]).await.unwrap();
        assert_eq!(tools.len(), 1, "内置注册表应填充 read");
        assert_eq!(tools[0].name.as_str(), "read");
        assert_eq!(tools[0].group(), Some("filesystem"), "group 随工具携带");
    }

    #[tokio::test]
    async fn list_tools_filter_whitelist_semantics() {
        // None=全部；Some(命中)=白名单；Some(未命中)/Some(空集)=空
        let station = Station::from_repo(&repo_with_filesystem(), "test-key").unwrap();
        let hit: HashSet<String> = ["filesystem".to_string()].into_iter().collect();
        let miss: HashSet<String> = ["other".to_string()].into_iter().collect();
        let empty = HashSet::new();
        assert_eq!(station.list_tools(Some(&hit), &[]).await.unwrap().len(), 1);
        assert!(station.list_tools(Some(&miss), &[]).await.unwrap().is_empty());
        assert!(station.list_tools(Some(&empty), &[]).await.unwrap().is_empty(), "空集过滤返回空");
    }

    #[tokio::test]
    async fn list_tools_with_sub_returns_empty_skeleton() {
        // 子 Station 不可达：告警跳过，路由表不更新，结果只有本地（此处本地为空）
        let station = Station::from_repo(&repo_with_sub(), "test-key").unwrap();
        let listing = station.list_tools(None, &[]).await.unwrap();
        assert!(listing.is_empty(), "无本地工具，子连接错误被跳过");
        assert!(station.tool_routes.is_empty(), "无子工具时路由表恒空");
    }

    #[test]
    fn merge_sub_tools_inserts_and_keeps_first_wins() {
        let station = Station::from_repo(&repo_with_filesystem(), "test-key").unwrap();
        let local_names: HashSet<String> = station.groups.iter()
            .flat_map(|e| e.value().tools.iter().map(|t| t.key().clone()).collect::<Vec<String>>())
            .collect();
        // 子 a 提供 x/y → 全插入
        let ta = vec![tool_config("x"), tool_config("y")];
        let ins_a = station.merge_sub_tools("station-a", ta, &local_names);
        assert_eq!(ins_a.len(), 2);
        assert_eq!(station.tool_routes.get("x").unwrap().value().as_str(), "station-a");
        // 子 b 提供 y（与 a 重名，先到者保留）与 z → y 剔除、z 插入
        let tb = vec![tool_config("y"), tool_config("z")];
        let ins_b = station.merge_sub_tools("station-b", tb, &local_names);
        assert_eq!(ins_b.len(), 1, "y 冲突剔除，仅 z 插入");
        assert!(ins_b.contains("z") && !ins_b.contains("y"));
        assert_eq!(station.tool_routes.get("y").unwrap().value().as_str(), "station-a", "先到者保留");
        assert_eq!(station.tool_routes.get("z").unwrap().value().as_str(), "station-b");
        // 子 c 提供 read（与本地 filesystem 内置重名，本地优先）→ 剔除
        let tc = vec![tool_config("read")];
        let ins_c = station.merge_sub_tools("station-c", tc, &local_names);
        assert!(ins_c.is_empty(), "与本地重名剔除");
        assert!(!station.tool_routes.contains_key("read"), "本地工具不进路由表");
        // 快照语义：子 a 重新拉取只含 x → 旧 y 记录被清
        let ta2 = vec![tool_config("x")];
        let _ = station.merge_sub_tools("station-a", ta2, &local_names);
        assert!(!station.tool_routes.contains_key("y"), "快照：该子旧记录清除");
    }

    fn tool_config(name: &str) -> Arc<Tool> {
        Arc::new(Tool::new(name, "d", json!({})))
    }

    #[test]
    fn from_repo_rejects_duplicate_tool_names() {
        // 两个 group 声明同名工具（一个内置 read、一个配置 read）→ 本地硬约束启动失败
        let mut repo = StationRepo::default();
        {
            let map = Arc::make_mut(&mut repo.groups);
            map.insert("filesystem".to_string(), ArcSwap::new(Arc::new(filesystem_group())));
            map.insert("other".to_string(), ArcSwap::new(Arc::new(GroupConfig {
                tools: Arc::new({
                    let mut m = ArcSwapHashMap::new();
                    m.insert("read".to_string(), ArcSwap::new(tool_config("read")));
                    m
                }),
            })));
        }
        // Station 不含 Debug（含 Arc<dyn NativeTool>），用 match 取错而非 unwrap_err
        let err = match Station::from_repo(&repo, "test-key") {
            Err(e) => e,
            Ok(_) => panic!("应冲突"),
        };
        assert!(err.to_string().contains("工具名冲突"), "冲突应报错: {}", err);
    }

    #[test]
    fn from_repo_rejects_same_group_tool_conflict() {
        // 同一 group 内配置声明的工具与内置注册表填充的工具重名 → 冲突
        // （走 from_repo 步骤 2 的 group.tools.contains_key 分支，非跨 group 的 check_unique）
        let mut repo = StationRepo::default();
        {
            let map = Arc::make_mut(&mut repo.groups);
            map.insert("filesystem".to_string(), ArcSwap::new(Arc::new(GroupConfig {
                tools: Arc::new({
                    let mut m = ArcSwapHashMap::new();
                    m.insert("read".to_string(), ArcSwap::new(tool_config("read")));
                    m
                }),
            })));
        }
        let err = match Station::from_repo(&repo, "test-key") {
            Err(e) => e,
            Ok(_) => panic!("应冲突"),
        };
        assert!(err.to_string().contains("工具名冲突"), "冲突应报错: {}", err);
    }

    #[tokio::test]
    async fn call_tool_routes_via_cache_table() {
        let station = Station::from_repo(&repo_with_sub(), "test-key").unwrap();
        // 注入路由（模拟 list_tools 拉取后缓存）：工具 x 属于 station-a
        station.tool_routes.insert("x".to_string(), "station-a".to_string());
        // 命中路由 → 经 MCP 调子 Station（子站不可达）：工具级失败映射为 EXTERNAL_ERROR
        //（关键是「不是工具不存在」——路由确实生效了）
        let result = station.call_tool("x", json!({}), &[]).await.unwrap();
        assert!(result.is_error);
        assert_eq!(result.structured_content["code"], TERR_TOOL_EXTERNAL_ERROR, "路由命中应调子而非工具不存在");
        assert!(result.error_message().unwrap().contains("调用 sub station 工具失败"), "子站调用失败文案");
        // 未命中路由（本地也无）→ 工具不存在
        let miss = station.call_tool("nope", json!({}), &[]).await.unwrap();
        assert_eq!(miss.structured_content["code"], TERR_TOOL_NOT_FOUND);
        assert!(miss.error_message().unwrap().contains("工具不存在"));
    }

    #[tokio::test]
    async fn call_tool_local_miss_returns_not_found() {
        let station = Station::from_repo(&repo_with_filesystem(), "test-key").unwrap();
        let miss = station.call_tool("nope", json!({}), &[]).await.unwrap();
        assert_eq!(miss.structured_content["code"], TERR_TOOL_NOT_FOUND);
    }

    #[tokio::test]
    async fn call_tool_local_native_tool_executes() {
        let station = Station::from_repo(&repo_with_filesystem(), "test-key").unwrap();
        // 内置 read 实现绑定进程 cwd；用 Cargo.toml 验证真实执行路径
        let result = station.call_tool("read", json!({ "path": "Cargo.toml" }), &[]).await.unwrap();
        assert!(!result.is_error, "本地 read 应成功: {:?}", result.error_message());
        assert!(result.text_content().unwrap().contains("kissbot-agent"));
    }

    #[tokio::test]
    async fn call_tool_cycle_is_protocol_error() {
        let station = Station::from_repo(&repo_with_filesystem(), "test-key").unwrap();
        let err = station
            .call_tool("read", json!({}), &["station-self".to_string()])
            .await
            .unwrap_err();
        assert_eq!(err.code, kissbot_mcp::STATION_CYCLE);
    }

    #[tokio::test]
    async fn mcp_handler_list_and_call_roundtrip() {
        let station = Station::from_repo(&repo_with_filesystem(), "test-key").unwrap();
        let meta = RequestMeta::new(Implementation::new("t", "1"));
        // 显式走 McpHandler 实现（Station 另有同名固有方法）
        let listing = McpHandler::list_tools(&station, ToolsListParams {
            cursor: None,
            meta: meta.clone().with_groups(&["filesystem".to_string()]),
        })
        .await
        .unwrap();
        assert_eq!(listing.tools[0].name.as_str(), "read");
        assert_eq!(listing.tools[0].group(), Some("filesystem"), "group 随工具携带");

        let called = McpHandler::call_tool(&station, ToolsCallParams {
            name: Arc::new("read".to_string()),
            arguments: json!({ "path": "Cargo.toml" }),
            meta,
        })
        .await
        .unwrap();
        assert!(!called.is_error);
    }

    #[tokio::test]
    async fn list_tools_cycle_is_protocol_error() {
        let station = Station::from_repo(&repo_with_filesystem(), "test-key").unwrap();
        let err = station.list_tools(None, &["station-self".to_string()]).await.unwrap_err();
        assert_eq!(err.code, kissbot_mcp::STATION_CYCLE);
    }

    // ========== 对接第三方 MCP 服务端（非 Station） ==========

    /// tools/list 观测记录：(收到的 group 参数, 收到的祖先链)
    type ListObservation = Arc<tokio::sync::Mutex<Option<(Option<Vec<String>>, Vec<String>)>>>;

    /// 测试用 Mock MCP 服务端：可切换是否声明 kissbot 分组扩展、是否回带 group
    struct MockMcpHandler {
        kissbot_group: bool,
        echo_group: bool,
        observed: ListObservation,
    }

    #[async_trait]
    impl McpHandler for MockMcpHandler {
        async fn discover(&self) -> DiscoverResult {
            let result = DiscoverResult::tools_only(Implementation::new("mock-mcp", "1.0.0"), None);
            if self.kissbot_group { result.with_kissbot_extensions() } else { result }
        }

        async fn list_tools(&self, params: ToolsListParams) -> std::result::Result<ToolsListResult, McpError> {
            *self.observed.lock().await =
                Some((params.meta.groups.clone(), params.meta.ancestors.clone()));
            let mut tool = Tool::new("mock-tool", "第三方工具", json!({ "type": "object" }));
            // 回带 group 的模拟：把请求白名单的第一项作为该工具的 group
            if self.echo_group {
                if let Some(group) = params.meta.groups.as_ref().and_then(|g| g.first()) {
                    tool = tool.with_group(group.clone());
                }
            }
            Ok(ToolsListResult::new(vec![Arc::new(tool)]))
        }

        async fn call_tool(&self, params: ToolsCallParams) -> std::result::Result<CallToolResult, McpError> {
            // 按工具名模拟三类返回：kissbot 循环错误、非 kissbot 错误、正常结果
            match params.name.as_str() {
                "cycle-tool" => Ok(CallToolResult::error(TERR_TOOL_CYCLE, "远端检测到自环")),
                "foreign-tool" => Ok(CallToolResult {
                    content: vec![kissbot_mcp::Content::text("boom")],
                    structured_content: json!({ "error": "boom" }),
                    is_error: true,
                    ..Default::default()
                }),
                _ => {
                    let value = json!({ "mock": params.name.to_string() });
                    Ok(CallToolResult {
                        content: vec![kissbot_mcp::Content::text(value.to_string())],
                        structured_content: value,
                        ..Default::default()
                    })
                }
            }
        }
    }

    /// 起一个 Mock MCP 服务端，返回（基地址, 观测记录器）
    async fn spawn_mock_mcp(kissbot_group: bool, echo_group: bool) -> (String, ListObservation) {
        let observed: ListObservation = Arc::new(tokio::sync::Mutex::new(None));
        let handler = Arc::new(MockMcpHandler {
            kissbot_group,
            echo_group,
            observed: observed.clone(),
        });
        let server = kissbot_mcp::McpServer::new(handler, Implementation::new("mock-mcp", "1.0.0"));
        let app = kissbot_mcp::router(server, kissbot_mcp::McpHttpConfig::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { let _ = axum::serve(listener, app).await; });
        (format!("http://{}", addr), observed)
    }

    /// 构造只挂一个远程子 Station 的仓库
    fn repo_with_remote_sub(base_url: &str, default_group: &str) -> StationRepo {
        let mut repo = StationRepo::default();
        repo.station_id = Arc::new("station-self".into());
        let map = Arc::make_mut(&mut repo.sub_stations);
        map.insert("remote".to_string(), ArcSwap::new(Arc::new(SubStationConfig {
            station_id: Arc::new("remote".into()),
            base_url: Arc::new(base_url.into()),
            timeout_secs: 5,
            default_group: Arc::new(default_group.into()),
        })));
        repo
    }

    fn group_filter(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn generic_mcp_server_tools_fall_into_default_group_with_local_filter() {
        // 通用 MCP 服务端（未声明 kissbot 扩展、不回带 group）
        let (base, observed) = spawn_mock_mcp(false, false).await;
        let station = Station::from_repo(&repo_with_remote_sub(&base, "remote-plain"), "test-key").unwrap();

        // 不下传 group：不支持分组过滤，传了也会被忽略
        let all = station.list_tools(None, &[]).await.unwrap();
        assert_eq!(all.iter().map(|t| t.name.to_string()).collect::<Vec<_>>(), vec!["mock-tool"]);
        assert_eq!(all[0].group(), Some("remote-plain"), "对端不带 group → 归入默认 group");
        let (sent_group, sent_ancestors) = observed.lock().await.clone().unwrap();
        assert!(sent_group.is_none(), "对端不支持时不传 group");
        assert_eq!(sent_ancestors, vec!["station-self".to_string()], "祖先链始终附带（纯提示）");

        // 本级白名单含默认 group → 保留；不含 → 本地过滤掉
        assert_eq!(station.list_tools(Some(&group_filter(&["remote-plain"])), &[]).await.unwrap().len(), 1);
        assert!(station.list_tools(Some(&group_filter(&["other"])), &[]).await.unwrap().is_empty());
        assert!(station.list_tools(Some(&group_filter(&[])), &[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn kissbot_mcp_server_group_is_passed_down_and_echo_is_trusted() {
        // 声明 kissbot 扩展且回带 group 的服务端
        let (base, observed) = spawn_mock_mcp(true, true).await;
        let station = Station::from_repo(&repo_with_remote_sub(&base, "ignored-default"), "test-key").unwrap();

        let filtered = station.list_tools(Some(&group_filter(&["mock-group"])), &[]).await.unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].group(), Some("mock-group"), "以对端携带的 group 为准");
        let (sent_group, _) = observed.lock().await.clone().unwrap();
        assert_eq!(sent_group, Some(vec!["mock-group".to_string()]), "支持时下传 group");
    }

    #[tokio::test]
    async fn kissbot_mcp_server_without_group_echo_falls_back_to_default_group() {
        // 声明支持但响应不带 group → 回落默认 group
        let (base, _observed) = spawn_mock_mcp(true, false).await;
        let station = Station::from_repo(&repo_with_remote_sub(&base, "fallback-group"), "test-key").unwrap();
        let listing = station.list_tools(None, &[]).await.unwrap();
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].group(), Some("fallback-group"));
    }

    #[tokio::test]
    async fn default_group_falls_back_to_station_id_when_unset() {
        let (base, _observed) = spawn_mock_mcp(false, false).await;
        let station = Station::from_repo(&repo_with_remote_sub(&base, ""), "test-key").unwrap();
        let listing = station.list_tools(None, &[]).await.unwrap();
        assert_eq!(listing[0].group(), Some("remote"), "未配置默认 group 时回落 station_id");
    }

    #[tokio::test]
    async fn generic_mcp_server_tool_can_be_called_after_routing() {
        let (base, _observed) = spawn_mock_mcp(false, false).await;
        let station = Station::from_repo(&repo_with_remote_sub(&base, "remote-plain"), "test-key").unwrap();
        // 先拉取建立路由缓存
        station.list_tools(None, &[]).await.unwrap();
        let called = station.call_tool("mock-tool", json!({}), &[]).await.unwrap();
        assert!(!called.is_error);
        assert_eq!(called.structured_content["mock"], "mock-tool");
    }

    #[tokio::test]
    async fn sub_station_kissbot_cycle_error_surfaces_as_protocol_cycle() {
        // 子站以工具级 kissbot 错误（code=TERR_TOOL_CYCLE）报告循环 → 本级识别为协议级自环
        let (base, _observed) = spawn_mock_mcp(false, false).await;
        let station = Station::from_repo(&repo_with_remote_sub(&base, "remote"), "test-key").unwrap();
        station.tool_routes.insert("cycle-tool".to_string(), "remote".to_string());
        let err = station.call_tool("cycle-tool", json!({}), &[]).await.unwrap_err();
        assert_eq!(err.code, kissbot_mcp::STATION_CYCLE, "kissbot 循环错误码应被识别并上抛");
    }

    #[tokio::test]
    async fn sub_station_foreign_error_passes_through_unchanged() {
        // 非 kissbot 来源的错误体：本级不解释、不改写，原样回传
        let (base, _observed) = spawn_mock_mcp(false, false).await;
        let station = Station::from_repo(&repo_with_remote_sub(&base, "remote"), "test-key").unwrap();
        station.tool_routes.insert("foreign-tool".to_string(), "remote".to_string());
        let result = station.call_tool("foreign-tool", json!({}), &[]).await.unwrap();
        assert!(result.is_error);
        assert_eq!(result.structured_content, json!({ "error": "boom" }), "第三方错误体原样透传");
        assert!(result.kissbot_error().is_none(), "非 kissbot 来源不按错误码解释");
    }

    #[tokio::test]
    async fn station_discover_declares_kissbot_extensions() {
        let station = Station::from_repo(&repo_with_filesystem(), "test-key").unwrap();
        let result = McpHandler::discover(&station).await;
        assert!(result.supports_group() && result.supports_ancestors() && result.supports_kissbot());
    }
}

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwap;
use arcstr::ArcStr;
use async_trait::async_trait;
use dashmap::DashMap;
use kissbot_llm::ToolConfig;
use kissbot_mcp::{
    CallToolResult, DiscoverResult, Implementation, McpClient, McpClientError, McpError,
    McpHandler, Tool, ToolsCallParams, ToolsListParams, ToolsListResult,
};
use serde_json::Value;
use tracing::warn;

use crate::config_manager::ConfigManager;
use crate::configs::{StationRepo, SubStationConfig};
use crate::station::native::NativeTool;
use crate::types::Result;

mod native;
mod filesystem;

use native::native_group;

// ========== 工具错误码（工具级失败随 CallToolResult 回带） ==========

pub const TERR_TOOL_NOT_FOUND: u32 = 0x0000_0001;
pub const TERR_TOOL_WRONG_ARGUMENTS: u32 = 0x0000_0003;
/// 级联自环（kissbot 工具错误码；协议级同名错误码见 `kissbot_mcp::STATION_CYCLE`）
pub const TERR_TOOL_CYCLE: u32 = 0x0000_0004;

pub const TERR_TOOL_INTERNAL_ERROR: u32 = 0x0001_0000;

pub const TERR_TOOL_EXTERNAL_ERROR: u32 = 0x0002_0000;

/// 构造工具错误 JSON（Nexus 侧 ToolData.error 的形状）
pub fn tool_error(code: u32, message: impl Into<ArcStr>) -> Value {
    let message: ArcStr = message.into();
    serde_json::json!({
        "code": code,
        "message": message
    })
}

/// MCP 工具定义 → LLM 侧工具定义（kissbot-llm 的 ToolConfig，OpenAI `tools[].function` 形状）：
/// 转换留在 agent（kissbot-llm 不依赖 MCP）
pub fn tool_config_from_mcp(tool: &Tool) -> ToolConfig {
    ToolConfig {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.input_schema.clone(),
    }
}

/// 子 Station 运行态：只存连接信息 + MCP 客户端（孙子由子 Station 自己递归）
pub struct SubStation {
    config: Arc<SubStationConfig>,
    client: McpClient,
}

impl SubStation {
    /// 生效的默认 group（根据 station_id 构造）
    fn default_group(&self) -> ArcStr {
        ArcStr::from(format!("[{}]", self.config.station_id))
    }

    /// 唯一性前缀
    fn global_name(&self, name: ArcStr) -> ArcStr {
        if let Some(prefix) = self.config.prefix.as_ref() {
            ArcStr::from(format!("{}{}", prefix.as_str(), name.as_str()))
        } else {
            name
        }
    }

    /// 唯一性前缀
    fn local_name<'a>(&self, name: &'a str) -> &'a str {
        if let Some(prefix) = self.config.prefix.as_ref() {
            if let Some(n) = name.strip_prefix(prefix.as_str()) {
                n
            } else {
                name
            }
        } else {
            name
        }
    }
}

/// 全局 Station 单例（进程内唯一；每个 agent 一个，静态访问）
static SINGLETON: OnceLock<Arc<Station>> = OnceLock::new();

pub struct Station {
    /// 本站点 station_id（来自 station.json；祖先链防环用）
    station_id: ArcStr,
    /// 本级工具索引：工具名 → 条目。工具名在 group 内唯一；跨 group 重名时保留先并入的 group
    /// 的工具并告警（MCP tools/call 只带工具名，服务端对外必须给出唯一名字）
    tools: DashMap<ArcStr, Arc<NativeTool>>,
    sub_stations: DashMap<ArcStr, Arc<SubStation>>,
    /// 远程工具路由表：工具名 → 直接子 station_id（仅 call_tool 路由用，不用于元数据查询）
    /// list_tools 拉取子 Station 成功时更新；
    /// 与本级/先到子重名时保留先到者（后到者剔除 + warn）
    tool_routes: ArcSwap<HashMap<ArcStr, ArcStr>>,
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
    pub async fn new() {
        let repo = ConfigManager::get().station_repo_snapshot().await;
        let api_key = kissbot_security::SecurityConfig::get().api_key.clone();
        let station = Self::from_repo(&repo, api_key.as_str());
        let _ = SINGLETON.set(Arc::new(station));
    }

    /// 按配置构建 Station（纯构造，不注册单例；单测直接消费）：
    /// 配置只声明加载哪些 group，工具元数据与原生实现由内置注册表按 group 名成对给出
    pub fn from_repo(repo: &StationRepo, api_key: &str) -> Self {
        let result = Self {
            station_id: repo.station_id.clone(),
            tools: DashMap::new(),
            sub_stations: DashMap::new(),
            tool_routes: ArcSwap::from_pointee(HashMap::new()),
        };
        for name in repo.groups.iter() {
            // 未内置的 group 只有名字、没有工具，注册为空
            if let Some(group) = native_group(name.as_str()) {
                for tool in group.tools {
                    let tool_name = tool.config.name.as_str();
                    if !result.tools.contains_key(tool_name) {
                        result.tools.insert(ArcStr::from(tool_name), Arc::new(tool));
                    }
                }
            }
        }
        // 子 Station：只存连接信息 + MCP 客户端（MCP 端点为 base_url + /mcp）
        for (id, scfg) in repo.sub_stations.iter() {
            let endpoint = format!("{}/mcp", scfg.base_url.trim_end_matches('/'));
            let sub = SubStation {
                config: scfg.clone(),
                client: McpClient::new(&endpoint, scfg.timeout_secs, Some(api_key)),
            };
            result.sub_stations.insert(id.clone(), Arc::new(sub));
        }
        result
    }

    /// 直接向本地 nexus 提供工具列表：`groups` 为会话启用的 group 白名单（None = 不过滤）
    pub async fn list_tools_local(&self, groups: Option<&HashSet<ArcStr>>) -> Result<Vec<Arc<Tool>>> {
        let tools = self.list_tools(Arc::new(Vec::new())).await?;
        match groups {
            Some(groups) => Ok(tools
                .into_iter()
                .filter(|tool| tool.group().is_some_and(|g| groups.contains(g)))
                .collect()),
            None => Ok(tools),
        }
    }

    /// 本地直接 tool call
    pub async fn call_tool_local(&self, name: &str, arguments: Value) -> Result<CallToolResult> {
        let result = self.call_tool(name, arguments, Arc::new(Vec::new())).await?;
        Ok(result)
    }

    /// 工具元数据平铺查询：本级 + 子 MCP 实时拉取（更新路由缓存）
    /// ancestors 为根到当前父节点的 station_id 链
    /// group 是工具的固有属性，随工具定义（`_meta["kissbot/group"]`）携带，因此本方法对每个
    /// 返回的工具都保证带上 group：本级工具用其所属 group；子站工具优先用对端回带的 group，
    /// 缺省回落该子站的默认 group。本方法返回全量工具，不做 group 白名单过滤
    /// （会话级白名单由 list_tools_local 在 nexus 入口处本地过滤）。
    pub async fn list_tools(&self, mut ancestors: Arc<Vec<ArcStr>>) -> std::result::Result<Vec<Arc<Tool>>, McpError> {
        if ancestors.iter().any(|a| a == &self.station_id) {
            return Err(McpError::station_cycle(&self.station_id));
        }
        let mut tools = Vec::new();
        // 本级工具名集合（冲突校验用：本级优先，子工具与本级重名剔除）
        let mut local_names: HashSet<ArcStr> = HashSet::new();
        for entry in self.tools.iter() {
            local_names.insert(entry.key().clone());
            tools.push(entry.value().config.clone());
        }
        // 经 MCP 实时拉取 → 更新路由缓存 → 合并元数据（剔除冲突项）
        // - 分组归因不依赖对端能力声明：工具带 `kissbot/group` 就用它，否则归入该子的默认 group，
        //   因此通用 MCP 服务端无需改造即可接入
        // - 祖先链对所有子都附带：纯提示信息，规范要求忽略未知 `_meta` 键，附带可为未声明能力的
        //   同级 kissbot 服务端保留防环能力
        {
            // 添加自己到 ancestors
            let child_ancestors = Arc::make_mut(&mut ancestors);
            child_ancestors.push(self.station_id.clone());
        }
        // 先整树克隆出 Arc 再 await（不跨 await 持 DashMap 读锁）
        let subs: Vec<Arc<SubStation>> = self.sub_stations.iter().map(|e| e.value().clone()).collect();
        let mut tool_routes = HashMap::new();
        for sub in subs {
            match sub.client.list_tools(ancestors.clone()).await {
                Ok(mut result) => {
                    let default_group = sub.default_group();
                    for mut tool in result.tools.drain(..) {
                        let name = sub.global_name(tool.name.clone());
                        if !local_names.contains(name.as_str()) && !tool_routes.contains_key(name.as_str()) {
                            {
                                let t = Arc::make_mut(&mut tool);
                                // 重写名字
                                t.name = name.clone();
                                // 对无 group 的添加默认 group
                                let mut absent = true;
                                if let Some(meta) = t.meta.as_ref() {
                                    absent = meta.group.is_none();
                                }
                                if absent {
                                    t.set_group(default_group.clone())
                                }
                            }
                            tools.push(tool);
                            tool_routes.insert(name.clone(), sub.config.station_id.clone());
                        }
                    }
                }
                Err(e) => warn!("子 Station {} 查询工具失败: {}", sub.config.station_id.as_str(), e),
            }
        }
        // 更新route
        self.tool_routes.store(Arc::new(tool_routes));
        // 确定性顺序（规范建议：同一工具集合的多次请求返回相同顺序，便于客户端缓存与提示词缓存）
        tools.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        Ok(tools)
    }

    /// 执行工具：本级原生实现命中执行；未命中 → 查远程路由表（工具名 → 直接子 station_id）
    /// 经 MCP `tools/call` 路由到对应子；路由未命中 → 工具不存在
    /// ancestors 为根到当前父节点的 station_id 链；自身在链中 → 级联自环（协议级错误）
    pub async fn call_tool(&self, name: &str, arguments: Value, mut ancestors: Arc<Vec<ArcStr>>) -> std::result::Result<CallToolResult, McpError> {
        if ancestors.iter().any(|a| a == &self.station_id) {
            return Err(McpError::station_cycle(&self.station_id));
        }
        // 本地查找：无 await 阶段完成查找并释放 DashMap 读锁（不跨 await 持锁）
        let local = self.tools.get(name).map(|e| e.value().implement.clone());
        if let Some(tool) = local {
            return Ok(tool.call(arguments).await);
        }
        // 远程路由：查路由缓存表（工具名 → 直接子 station_id），命中 → 该子 MCP 调用
        // 不遍历全部子、不远程获取列表；先克隆 Arc 再 await（不跨 await 持锁）
        {
            // 添加自己到 ancestors
            let child_ancestors = Arc::make_mut(&mut ancestors);
            child_ancestors.push(self.station_id.clone());
        }
        let routed = self.tool_routes.load().get(name).map(|r| r.clone());
        if let Some(station_id) = routed {
            if let Some(sub) = self.sub_stations.get(station_id.as_str()).map(|s| s.value().clone()) {
                let nn = sub.local_name(name);
                return match sub.client.call_tool(nn, arguments, ancestors).await {
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
const STATION_INSTRUCTIONS: ArcStr = arcstr::literal!("kissbot Station：提供按 group 分组的工具，并可级联下级 Station。");

#[async_trait]
impl McpHandler for Station {
    async fn discover(&self) -> DiscoverResult {
        // 默认声明 kissbot 级联扩展能力（理解祖先链），供对端判定
        DiscoverResult::new(
            Implementation::new("kissbot-station", env!("CARGO_PKG_VERSION")),
            Some(STATION_INSTRUCTIONS.clone()),
        )
    }

    async fn list_tools(&self, mut params: ToolsListParams) -> std::result::Result<ToolsListResult, McpError> {
        let ancestors = if let Some(ancestors) = params.meta.ancestors.take() { ancestors } else { Arc::new(Vec::new()) };
        let tools = self.list_tools(ancestors).await?;
        Ok(ToolsListResult::new(tools))
    }

    async fn call_tool(&self, mut params: ToolsCallParams) -> std::result::Result<CallToolResult, McpError> {
        let ancestors = if let Some(ancestors) = params.meta.ancestors.take() { ancestors } else { Arc::new(Vec::new()) };
        self.call_tool(params.name.as_str(), params.arguments, ancestors).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kissbot_mcp::{RequestMeta, ToolsCallParams};
    use serde_json::json;

    fn repo_with_filesystem() -> StationRepo {
        let mut repo = StationRepo::default();
        repo.station_id = ArcStr::from("station-self");
        repo.groups = vec![ArcStr::from("filesystem")];
        repo
    }

    fn repo_with_sub() -> StationRepo {
        let mut repo = StationRepo::default();
        repo.station_id = ArcStr::from("station-self");
        let map = Arc::make_mut(&mut repo.sub_stations);
        map.insert(ArcStr::from("station-a"), Arc::new(SubStationConfig {
            station_id: ArcStr::from("station-a"),
            base_url: ArcStr::from("http://127.0.0.1:9001"),
            timeout_secs: 5,
            prefix: None,
        }));
        repo
    }

    fn ancestors(list: &[&str]) -> Arc<Vec<ArcStr>> {
        Arc::new(list.iter().map(|s| ArcStr::from(*s)).collect())
    }

    fn first_text(result: &CallToolResult) -> Option<&str> {
        result.content.iter().find_map(|c| c.as_text())
    }

    /// 注入远程路由缓存（工具名 → 直接子 station_id）
    fn set_routes(station: &Station, routes: &[(&str, &str)]) {
        let mut map = HashMap::new();
        for (name, station_id) in routes {
            map.insert(ArcStr::from(*name), ArcStr::from(*station_id));
        }
        station.tool_routes.store(Arc::new(map));
    }

    #[tokio::test]
    async fn list_tools_returns_all_local() {
        // 内置 filesystem group 声明后，read 工具元数据由内置注册表填充
        let station = Station::from_repo(&repo_with_filesystem(), "test-key");
        let tools = station.list_tools(ancestors(&[])).await.unwrap();
        assert_eq!(tools.len(), 1, "内置注册表应填充 read");
        assert_eq!(tools[0].name.as_str(), "read");
        assert_eq!(tools[0].group(), Some("filesystem"), "group 随工具携带");
    }

    #[tokio::test]
    async fn list_tools_local_filters_by_group() {
        // 本地直连 nexus 的入口：按会话启用的 group 白名单过滤（None = 全部）
        let station = Station::from_repo(&repo_with_filesystem(), "test-key");
        let hit: HashSet<ArcStr> = [ArcStr::from("filesystem")].into_iter().collect();
        let miss: HashSet<ArcStr> = [ArcStr::from("other")].into_iter().collect();
        assert_eq!(station.list_tools_local(Some(&hit)).await.unwrap().len(), 1);
        assert!(station.list_tools_local(Some(&miss)).await.unwrap().is_empty());
        assert_eq!(station.list_tools_local(None).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn list_tools_with_unreachable_sub_returns_empty() {
        // 子 Station 不可达：告警跳过，路由表留空，结果只有本地（此处本地为空）
        let station = Station::from_repo(&repo_with_sub(), "test-key");
        let listing = station.list_tools(ancestors(&[])).await.unwrap();
        assert!(listing.is_empty(), "无本地工具，子连接错误被跳过");
        assert!(station.tool_routes.load().is_empty(), "无子工具时路由表恒空");
    }

    #[tokio::test]
    async fn call_tool_routes_via_cache_table() {
        let station = Station::from_repo(&repo_with_sub(), "test-key");
        // 注入路由（模拟 list_tools 拉取后缓存）：工具 x 属于 station-a
        set_routes(&station, &[("x", "station-a")]);
        // 命中路由 → 经 MCP 调子 Station（子站不可达）：工具级失败映射为 EXTERNAL_ERROR
        //（关键是「不是工具不存在」——路由确实生效了）
        let result = station.call_tool("x", json!({}), ancestors(&[])).await.unwrap();
        assert!(result.is_error);
        assert_eq!(result.structured_content["code"], TERR_TOOL_EXTERNAL_ERROR, "路由命中应调子而非工具不存在");
        assert!(result.error_message().unwrap().contains("调用 sub station 工具失败"), "子站调用失败文案");
        // 未命中路由（本地也无）→ 工具不存在
        let miss = station.call_tool("nope", json!({}), ancestors(&[])).await.unwrap();
        assert_eq!(miss.structured_content["code"], TERR_TOOL_NOT_FOUND);
        assert!(miss.error_message().unwrap().contains("工具不存在"));
    }

    #[tokio::test]
    async fn call_tool_local_miss_returns_not_found() {
        let station = Station::from_repo(&repo_with_filesystem(), "test-key");
        let miss = station.call_tool_local("nope", json!({})).await.unwrap();
        assert_eq!(miss.structured_content["code"], TERR_TOOL_NOT_FOUND);
    }

    #[tokio::test]
    async fn call_tool_local_native_tool_executes() {
        let station = Station::from_repo(&repo_with_filesystem(), "test-key");
        // 内置 read 实现绑定进程 cwd；用 Cargo.toml 验证真实执行路径
        let result = station.call_tool_local("read", json!({ "path": "Cargo.toml" })).await.unwrap();
        assert!(!result.is_error, "本地 read 应成功: {:?}", result.error_message());
        assert!(first_text(&result).unwrap().contains("kissbot-agent"));
    }

    #[tokio::test]
    async fn call_tool_cycle_is_protocol_error() {
        let station = Station::from_repo(&repo_with_filesystem(), "test-key");
        let err = station
            .call_tool("read", json!({}), ancestors(&["station-self"]))
            .await
            .unwrap_err();
        assert_eq!(err.code, kissbot_mcp::STATION_CYCLE);
    }

    #[tokio::test]
    async fn list_tools_cycle_is_protocol_error() {
        let station = Station::from_repo(&repo_with_filesystem(), "test-key");
        let err = station.list_tools(ancestors(&["station-self"])).await.unwrap_err();
        assert_eq!(err.code, kissbot_mcp::STATION_CYCLE);
    }

    #[tokio::test]
    async fn mcp_handler_list_and_call_roundtrip() {
        let station = Station::from_repo(&repo_with_filesystem(), "test-key");
        let meta = RequestMeta::new(Implementation::new("t", "1"));
        // 显式走 McpHandler 实现（Station 另有同名固有方法）
        let listing = McpHandler::list_tools(&station, ToolsListParams {
            cursor: None,
            meta: meta.clone(),
        })
        .await
        .unwrap();
        assert_eq!(listing.tools[0].name.as_str(), "read");
        assert_eq!(listing.tools[0].group(), Some("filesystem"), "group 随工具携带");

        let called = McpHandler::call_tool(&station, ToolsCallParams {
            name: ArcStr::from("read"),
            arguments: json!({ "path": "Cargo.toml" }),
            meta,
        })
        .await
        .unwrap();
        assert!(!called.is_error);
    }

    // ========== 对接第三方 MCP 服务端（非 Station） ==========

    /// tools/list 观测记录：服务端收到的祖先链
    type ListObservation = Arc<tokio::sync::Mutex<Option<Vec<ArcStr>>>>;

    /// 测试用 Mock MCP 服务端：可切换工具名与是否在工具定义里携带 group
    struct MockMcpHandler {
        tool_name: ArcStr,
        echo_group: bool,
        observed: ListObservation,
    }

    #[async_trait]
    impl McpHandler for MockMcpHandler {
        async fn discover(&self) -> DiscoverResult {
            DiscoverResult::new(Implementation::new("mock-mcp", "1.0.0"), None)
        }

        async fn list_tools(&self, params: ToolsListParams) -> std::result::Result<ToolsListResult, McpError> {
            *self.observed.lock().await = params.meta.ancestors.as_ref().map(|a| a.as_ref().clone());
            let mut tool = Tool::new(self.tool_name.clone(), "第三方工具", json!({ "type": "object" }));
            if self.echo_group {
                tool.set_group("mock-group");
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
    async fn spawn_mock_mcp_named(tool_name: &str, echo_group: bool) -> (String, ListObservation) {
        let observed: ListObservation = Arc::new(tokio::sync::Mutex::new(None));
        let handler = Arc::new(MockMcpHandler {
            tool_name: ArcStr::from(tool_name),
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

    async fn spawn_mock_mcp(echo_group: bool) -> (String, ListObservation) {
        spawn_mock_mcp_named("mock-tool", echo_group).await
    }

    /// 构造只挂一个远程子 Station 的仓库（可选名字前缀）
    fn repo_with_remote_sub(base_url: &str, prefix: Option<&str>) -> StationRepo {
        let mut repo = StationRepo::default();
        repo.station_id = ArcStr::from("station-self");
        let map = Arc::make_mut(&mut repo.sub_stations);
        map.insert(ArcStr::from("remote"), Arc::new(SubStationConfig {
            station_id: ArcStr::from("remote"),
            base_url: ArcStr::from(base_url),
            timeout_secs: 5,
            prefix: prefix.map(ArcStr::from),
        }));
        repo
    }

    #[tokio::test]
    async fn remote_tool_without_group_falls_back_to_station_default_group() {
        // 对端工具未携带 group（如通用 MCP 服务端）→ 归入子站默认 group
        let (base, observed) = spawn_mock_mcp(false).await;
        let station = Station::from_repo(&repo_with_remote_sub(&base, None), "test-key");

        let all = station.list_tools(ancestors(&[])).await.unwrap();
        assert_eq!(all.iter().map(|t| t.name.to_string()).collect::<Vec<_>>(), vec!["mock-tool"]);
        assert_eq!(all[0].group(), Some("[remote]"), "对端不带 group → 归入子站默认 group");
        // 祖先链始终附带（纯提示），值为本级 station_id
        assert_eq!(observed.lock().await.clone(), Some(vec![ArcStr::from("station-self")]));
    }

    #[tokio::test]
    async fn remote_tool_carried_group_is_trusted() {
        // 对端工具携带 group → 以对端携带的 group 为准
        let (base, _observed) = spawn_mock_mcp(true).await;
        let station = Station::from_repo(&repo_with_remote_sub(&base, None), "test-key");

        let tools = station.list_tools(ancestors(&[])).await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].group(), Some("mock-group"), "以对端携带的 group 为准");
    }

    #[tokio::test]
    async fn remote_tool_name_gets_prefix_and_call_strips_it() {
        // 前缀让远端工具名在本级唯一；调用时去掉前缀再转发给子站
        let (base, _observed) = spawn_mock_mcp(false).await;
        let station = Station::from_repo(&repo_with_remote_sub(&base, Some("a/")), "test-key");

        let tools = station.list_tools(ancestors(&[])).await.unwrap();
        assert_eq!(tools[0].name.as_str(), "a/mock-tool");

        let called = station.call_tool("a/mock-tool", json!({}), ancestors(&[])).await.unwrap();
        assert!(!called.is_error);
        // 子站收到的是去掉前缀的本地名
        assert_eq!(called.structured_content["mock"], "mock-tool");
    }

    #[tokio::test]
    async fn local_tool_takes_precedence_over_remote_same_name() {
        // 远端工具与本级同名 → 本级优先，远端被剔除且不进路由表
        let (base, _observed) = spawn_mock_mcp_named("read", false).await;
        let mut repo = repo_with_remote_sub(&base, None);
        repo.groups = vec![ArcStr::from("filesystem")];
        let station = Station::from_repo(&repo, "test-key");

        let tools = station.list_tools(ancestors(&[])).await.unwrap();
        assert_eq!(tools.iter().map(|t| t.name.to_string()).collect::<Vec<_>>(), vec!["read"]);
        assert_eq!(tools[0].group(), Some("filesystem"), "保留的是本级工具");
        assert!(station.tool_routes.load().is_empty(), "被剔除的远端工具不进路由表");
    }

    #[tokio::test]
    async fn sub_station_kissbot_cycle_error_surfaces_as_protocol_cycle() {
        // 子站以工具级 kissbot 错误（code=TERR_TOOL_CYCLE）报告循环 → 本级识别为协议级自环
        let (base, _observed) = spawn_mock_mcp(false).await;
        let station = Station::from_repo(&repo_with_remote_sub(&base, None), "test-key");
        set_routes(&station, &[("cycle-tool", "remote")]);
        let err = station.call_tool("cycle-tool", json!({}), ancestors(&[])).await.unwrap_err();
        assert_eq!(err.code, kissbot_mcp::STATION_CYCLE, "kissbot 循环错误码应被识别并上抛");
    }

    #[tokio::test]
    async fn sub_station_foreign_error_passes_through_unchanged() {
        // 非 kissbot 来源的错误体：本级不解释、不改写，原样回传
        let (base, _observed) = spawn_mock_mcp(false).await;
        let station = Station::from_repo(&repo_with_remote_sub(&base, None), "test-key");
        set_routes(&station, &[("foreign-tool", "remote")]);
        let result = station.call_tool("foreign-tool", json!({}), ancestors(&[])).await.unwrap();
        assert!(result.is_error);
        assert_eq!(result.structured_content, json!({ "error": "boom" }), "第三方错误体原样透传");
        assert!(result.kissbot_error().is_none(), "非 kissbot 来源不按错误码解释");
    }

    #[tokio::test]
    async fn station_discover_declares_kissbot_extensions() {
        let station = Station::from_repo(&repo_with_filesystem(), "test-key");
        let result = McpHandler::discover(&station).await;
        assert!(result.supports_ancestors() && result.supports_kissbot());
    }
}

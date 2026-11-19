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
use crate::types::{Error, Result};

mod native;
mod filesystem;

use native::native_group;

pub const MCP_ERROR_STATION_CYCLE: i64 = -50001;

/// MCP 工具定义 → LLM 侧工具定义（kissbot-llm 的 ToolConfig，OpenAI `tools[].function` 形状）：
/// 转换留在 agent（kissbot-llm 不依赖 MCP）
pub fn tool_config_from_mcp(tool: &Tool) -> ToolConfig {
    ToolConfig {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.input_schema.clone(),
    }
}

/// 全局 Station 单例（进程内唯一；每个 agent 一个，静态访问）
static SINGLETON: OnceLock<Arc<Station>> = OnceLock::new();

pub struct Station {
    /// 本站点 station_id（来自 station.json；祖先链防环用）
    station_id: ArcStr,
    /// 本级工具索引：工具名 → 条目。工具名在 group 内唯一；跨 group 重名时按 group 名排序
    /// 保留先到者并告警（MCP tools/call 只带工具名，服务端对外必须给出唯一名字）
    tools: DashMap<ArcStr, Arc<NativeTool>>,
    /// 子 Station 运行态：key = station.json 的 sub_stations key，value 只有 MCP 客户端。
    /// 连接参数（base_url / timeout / prefix）不在此缓存，每次用到时按 id 从 ConfigManager 现读；
    /// 是否装配由调用方判断（from_repo 只装配启用的子站、管理 API 只在启用时调用），
    /// 增删由 add_sub_station / remove_sub_station 逐项维护
    sub_stations: DashMap<ArcStr, Arc<McpClient>>,
    /// 远程工具路由表：工具名 → 直接子 station_id（仅 call_tool 路由用，不用于元数据查询）
    /// list_tools 拉取子 Station 成功时整体重建；子站增删时整表清空（由下一次 list_tools 刷新）；
    /// 与本级/先到子重名时保留先到者（后到者剔除 + warn）
    tool_routes: ArcSwap<HashMap<ArcStr, ArcStr>>,
    /// 构造子 Station 运行态用的 api key（子站 MCP 端点认证）
    api_key: ArcStr,
    server_info: Arc<Implementation>,
}

const SERVER_INFO_NAME: ArcStr = arcstr::literal!("kissbot-station");
const SERVER_INFO_VERSION: ArcStr = arcstr::literal!("0.1");

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
    /// 逐项注册配置声明的 group 与子 Station（与运行期增删同一套入口，不做整份装配）；
    /// 是否装配子站由这里按配置的 enabled 判断
    pub fn from_repo(repo: &StationRepo, api_key: &str) -> Self {
        let result = Self {
            station_id: repo.station_id.clone(),
            tools: DashMap::new(),
            sub_stations: DashMap::new(),
            tool_routes: ArcSwap::from_pointee(HashMap::new()),
            api_key: ArcStr::from(api_key),
            server_info: Arc::new(Implementation::new(SERVER_INFO_NAME, SERVER_INFO_VERSION)),
        };
        for group in repo.groups.iter() {
            result.add_group(group.as_str());
        }
        for (station_id, config) in repo.sub_stations.iter() {
            // 停用的子站不装配运行态（add_sub_station 自身不判断 enabled）
            if !config.enabled {
                continue;
            }
            result.add_sub_station(station_id, config);
        }
        result
    }

    /// 增加一个本级 group：只注册该 group 自己声明的工具（已存在的同名工具保留先到者并告警）。
    /// 未内置的 group 只有名字、没有工具，注册为空；重复增加为空操作
    pub fn add_group(&self, group: &str) {
        if let Some(native) = native_group(group) {
            for tool in native.tools {
                let tool_name = ArcStr::from(tool.config.name.as_str());
                if self.tools.contains_key(&tool_name) {
                    warn!("工具名重名，保留先到者: {}（group: {}）", tool_name.as_str(), group);
                    continue;
                }
                self.tools.insert(tool_name, Arc::new(tool));
            }
        }
    }

    /// 删除一个本级 group：只移除该 group 注册进来的工具，其它 group 的工具不受影响。
    /// 远端工具名单不受本级 group 影响，路由表无需变动（本级工具在 call_tool 中优先）
    pub fn remove_group(&self, group: &str) {
        self.tools.retain(|_, tool| tool.config.group() != Some(group));
    }

    /// 建立（或按当前配置重建）一个子 Station 运行态：只动该子站的客户端与路由。
    /// 是否该装配由调用方决定（本方法不判断 enabled）；连接参数不保存，列表/调用时按 id 从 ConfigManager 现读
    pub fn add_sub_station(&self, station_id: &ArcStr, config: &SubStationConfig) {
        let endpoint = format!("{}/mcp", config.base_url.trim_end_matches('/'));
        let client = McpClient::new(&endpoint, config.timeout_secs, Some(self.api_key.as_str()));
        self.sub_stations.insert(station_id.clone(), Arc::new(client));
        // 子站名单变化后整张路由表失效，由下一次 list_tools 重建
        self.tool_routes.store(Arc::new(HashMap::new()));
    }

    /// 删除一个子 Station 运行态：只移除该子站的客户端，
    /// 其工具不再参与列表与路由（路由命中但运行态已无该子站时按工具不存在处理）
    pub fn remove_sub_station(&self, station_id: &ArcStr) {
        self.sub_stations.remove(station_id);
        // 子站名单变化后整张路由表失效，由下一次 list_tools 重建
        self.tool_routes.store(Arc::new(HashMap::new()));
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
    pub async fn call_tool_local(&self, name: ArcStr, arguments: Value) -> (Value, Result<CallToolResult>) {
        let (arguments_recover, result) = self.call_tool(name, arguments, Arc::new(Vec::new())).await;
        match result {
            Ok(r) => (arguments_recover, Ok(r)),
            Err(e) => (arguments_recover, Err(Error::from(e))),
        }
    }

    /// 工具元数据平铺查询：本级 + 子 MCP 实时拉取（更新路由缓存）
    /// ancestors 为根到当前父节点的 station_id 链
    /// group 是工具的固有属性，随工具定义（`_meta["kissbot/group"]`）携带，因此本方法对每个
    /// 返回的工具都保证带上 group：本级工具用其所属 group；子站工具优先用对端回带的 group，
    /// 缺省回落该子站的默认 group。本方法返回全量工具，不做 group 白名单过滤
    /// （会话级白名单由 list_tools_local 在 nexus 入口处本地过滤）。
    pub async fn list_tools(&self, mut ancestors: Arc<Vec<ArcStr>>) -> std::result::Result<Vec<Arc<Tool>>, McpError> {
        if ancestors.iter().any(|a| a == &self.station_id) {
            return Err(McpError {
                code: MCP_ERROR_STATION_CYCLE,
                message: ArcStr::from(format!("station cycle detected: {}", self.station_id)),
                data: Value::Null,
            });
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
        // 子站连接参数按 id 现读配置（运行态只存 MCP 客户端）；配置里没有该子站则跳过
        let repo = ConfigManager::get().station_repo_snapshot().await;
        // 先整树克隆出 Arc 再 await（不跨 await 持 DashMap 读锁）
        let subs: Vec<(ArcStr, Arc<McpClient>)> = self.sub_stations.iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();
        let mut tool_routes = HashMap::new();
        for (station_id, client) in subs {
            let Some(config) = repo.sub_stations.get(&station_id).cloned() else { continue };
            match client.list_tools(ancestors.clone()).await {
                Ok(result) => {
                    let default_group = config.default_group();
                    for mut tool in result.tools.into_iter() {
                        let name = config.global_name(tool.name.clone());
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
                            tool_routes.insert(name.clone(), station_id.clone());
                        }
                    }
                }
                Err(e) => warn!("子 Station {} 查询工具失败: {}", station_id.as_str(), e),
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
    pub async fn call_tool(&self, name: ArcStr, arguments: Value, mut ancestors: Arc<Vec<ArcStr>>) -> (Value, std::result::Result<CallToolResult, McpError>) {
        if ancestors.iter().any(|a| a == &self.station_id) {
            // 级联自环按照自定义code协议错误返回
            return (arguments, Err(McpError {
                code: MCP_ERROR_STATION_CYCLE,
                message: ArcStr::from(format!("station cycle detected: {}", self.station_id)),
                data: Value::Null,
            }));
        }
        // 本地查找：无 await 阶段完成查找并释放 DashMap 读锁（不跨 await 持锁）
        let local = self.tools.get(&name).map(|e| e.value().implement.clone());
        if let Some(tool) = local {
            let (arguments_recover, result) = tool.call(arguments).await;
            return (arguments_recover, Ok(result));
        }
        // 远程路由：查路由缓存表（工具名 → 直接子 station_id），命中 → 该子 MCP 调用
        // 不遍历全部子、不远程获取列表；先克隆 Arc 再 await（不跨 await 持锁）
        {
            // 添加自己到 ancestors
            let child_ancestors = Arc::make_mut(&mut ancestors);
            child_ancestors.push(self.station_id.clone());
        }
        let routed = self.tool_routes.load().get(&name).map(|r| r.clone());
        if let Some(station_id) = routed {
            // 连接参数按 id 现读配置；运行态或配置已无该子站 → 视为工具不存在
            let repo = ConfigManager::get().station_repo_snapshot().await;
            let config = repo.sub_stations.get(&station_id).cloned();
            if let (Some(client), Some(config)) = (self.sub_stations.get(&station_id).map(|s| s.value().clone()), config) {
                let nn = config.local_name(name);
                let (arguments_recover, result) = client.call_tool(nn, arguments, ancestors).await;
                return match result {
                    Ok(r) => {
                        // 返回参数和结果
                        (arguments_recover, Ok(r))
                    }
                    Err(McpClientError::Remote(e)) => {
                        // 协议错误透传
                        (arguments_recover, Err(e))
                    },
                    Err(e) => {
                        // 其他错误按照内部错误上报
                        (arguments_recover, Err(McpError::internal(ArcStr::from(e.to_string()))))
                    },
                };
            }
        }
        // 工具不存在
        (arguments, Err(McpError::invalid_params(ArcStr::from(format!("工具不存在: {}", name)))))
    }
}

/// `server/discover` 返回的自然语言说明
const STATION_INSTRUCTIONS: ArcStr = arcstr::literal!("kissbot Station：提供按 group 分组的工具，并可级联下级 Station。");

#[async_trait]
impl McpHandler for Station {
    fn server_info(&self) -> Arc<Implementation> {
        self.server_info.clone()
    }

    async fn discover(&self) -> DiscoverResult {
        // 默认声明 kissbot 级联扩展能力（理解祖先链），供对端判定
        DiscoverResult::new(Some(STATION_INSTRUCTIONS.clone()))
    }

    async fn list_tools(&self, mut params: ToolsListParams) -> std::result::Result<ToolsListResult, McpError> {
        let ancestors = if let Some(ancestors) = params.meta.ancestors.take() { ancestors } else { Arc::new(Vec::new()) };
        let tools = self.list_tools(ancestors).await?;
        Ok(ToolsListResult::new(tools))
    }

    async fn call_tool(&self, mut params: ToolsCallParams) -> std::result::Result<CallToolResult, McpError> {
        let ancestors = if let Some(ancestors) = params.meta.ancestors.take() { ancestors } else { Arc::new(Vec::new()) };
        self.call_tool(params.name, params.arguments, ancestors).await.1
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::test_support as ts;
    use kissbot_api::ContentBlock;
    use kissbot_mcp::{RequestMeta, ToolsCallParams};
    use serde_json::json;

    /// 只声明 filesystem group 的仓库（无子站）
    fn repo_with_filesystem() -> StationRepo {
        let mut repo = StationRepo::default();
        repo.station_id = ArcStr::from(ts::STATION_ID);
        repo.groups = Arc::new(BTreeSet::from([ArcStr::from("filesystem")]));
        repo
    }

    fn ancestors(list: &[&str]) -> Arc<Vec<ArcStr>> {
        Arc::new(list.iter().map(|s| ArcStr::from(*s)).collect())
    }

    fn first_text(result: &CallToolResult) -> Option<&str> {
        result.content.iter().find_map(|c| match c {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
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
        ts::init().await;
        let station = Station::from_repo(&repo_with_filesystem(), "test-key");
        let tools = station.list_tools(ancestors(&[])).await.unwrap();
        assert_eq!(tools.len(), 1, "内置注册表应填充 read");
        assert_eq!(tools[0].name.as_str(), "read");
        assert_eq!(tools[0].group(), Some("filesystem"), "group 随工具携带");
    }

    #[tokio::test]
    async fn list_tools_local_filters_by_group() {
        // 本地直连 nexus 的入口：按会话启用的 group 白名单过滤（None = 全部）
        ts::init().await;
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
        let fx = ts::init().await;
        let station = Station::from_repo(&fx.repo_with(&[ts::SUB_UNREACHABLE]), "test-key");
        let listing = station.list_tools(ancestors(&[])).await.unwrap();
        assert!(listing.is_empty(), "无本地工具，子连接错误被跳过");
        assert!(station.tool_routes.load().is_empty(), "无子工具时路由表恒空");
    }

    #[tokio::test]
    async fn call_tool_local_native_tool_executes() {
        ts::init().await;
        let station = Station::from_repo(&repo_with_filesystem(), "test-key");
        // 内置 read 实现绑定进程 cwd；用 Cargo.toml 验证真实执行路径
        let result = station.call_tool_local(ArcStr::from("read"), json!({ "path": "Cargo.toml" })).await.1.unwrap();
        assert!(!result.is_error, "本地 read 应成功: {:?}", first_text(&result).unwrap());
        assert!(first_text(&result).unwrap().contains("kissbot-agent"));
    }

    #[tokio::test]
    async fn mcp_handler_list_and_call_roundtrip() {
        ts::init().await;
        let station = Station::from_repo(&repo_with_filesystem(), "test-key");
        let meta = RequestMeta::new(Implementation::new(ArcStr::from("t"), ArcStr::from("1")));
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

    #[tokio::test]
    async fn remote_tool_without_group_falls_back_to_station_default_group() {
        // 对端工具未携带 group（如通用 MCP 服务端）→ 归入子站默认 group
        let fx = ts::init().await;
        let station = Station::from_repo(&fx.repo_with(&[ts::SUB_PLAIN]), "test-key");

        let all = station.list_tools(ancestors(&[])).await.unwrap();
        assert_eq!(all.iter().map(|t| t.name.to_string()).collect::<Vec<_>>(), vec!["mock-tool"]);
        assert_eq!(all[0].group(), Some("[plain]"), "对端不带 group → 归入子站默认 group");
        // 祖先链始终附带（纯提示），值为本级 station_id
        assert_eq!(*fx.plain_observed().lock().await, Some(vec![ArcStr::from(ts::STATION_ID)]));
    }

    #[tokio::test]
    async fn remote_tool_carried_group_is_trusted() {
        // 对端工具携带 group → 以对端携带的 group 为准
        let fx = ts::init().await;
        let station = Station::from_repo(&fx.repo_with(&[ts::SUB_GROUPED]), "test-key");

        let tools = station.list_tools(ancestors(&[])).await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].group(), Some("mock-group"), "以对端携带的 group 为准");
    }

    #[tokio::test]
    async fn remote_tool_name_gets_prefix_and_call_strips_it() {
        // 前缀让远端工具名在本级唯一；调用时去掉前缀再转发给子站
        let fx = ts::init().await;
        let station = Station::from_repo(&fx.repo_with(&[ts::SUB_PREFIXED]), "test-key");

        let tools = station.list_tools(ancestors(&[])).await.unwrap();
        assert_eq!(tools[0].name.as_str(), "a/mock-tool");

        let called = station.call_tool(ArcStr::from("a/mock-tool"), json!({}), ancestors(&[])).await.1.unwrap();
        assert!(!called.is_error);
        // 子站收到的是去掉前缀的本地名
        assert_eq!(called.structured_content["mock"], "mock-tool");
    }

    #[tokio::test]
    async fn local_tool_takes_precedence_over_remote_same_name() {
        // 远端工具与本级同名 → 本级优先，远端被剔除且不进路由表
        let fx = ts::init().await;
        let mut repo = fx.repo_with(&[ts::SUB_READ]);
        repo.groups = Arc::new(BTreeSet::from([ArcStr::from("filesystem")]));
        let station = Station::from_repo(&repo, "test-key");

        let tools = station.list_tools(ancestors(&[])).await.unwrap();
        assert_eq!(tools.iter().map(|t| t.name.to_string()).collect::<Vec<_>>(), vec!["read"]);
        assert_eq!(tools[0].group(), Some("filesystem"), "保留的是本级工具");
        assert!(station.tool_routes.load().is_empty(), "被剔除的远端工具不进路由表");
    }

    #[tokio::test]
    async fn sub_station_foreign_error_passes_through_unchanged() {
        // 非 kissbot 来源的错误体：本级不解释、不改写，原样回传
        let fx = ts::init().await;
        let station = Station::from_repo(&fx.repo_with(&[ts::SUB_PLAIN]), "test-key");
        set_routes(&station, &[("foreign-tool", ts::SUB_PLAIN)]);
        let result = station.call_tool(ArcStr::from("foreign-tool"), json!({}), ancestors(&[])).await.1.unwrap();
        assert!(result.is_error);
        assert_eq!(result.structured_content, json!({ "error": "boom" }), "第三方错误体原样透传");
    }

    #[tokio::test]
    async fn station_discover_declares_kissbot_extensions() {
        let station = Station::from_repo(&repo_with_filesystem(), "test-key");
        let result = McpHandler::discover(&station).await;
        assert!(result.supports_ancestors() && result.supports_kissbot());
    }

    // ========== 运行期变更（逐项增删工具 / 子站） ==========

    #[tokio::test]
    async fn add_and_remove_group_only_touches_that_group() {
        // 运行期增删 group：只增删该 group 的工具，其它 group 与子站运行态不受影响
        ts::init().await;
        let station = Station::from_repo(&StationRepo::default(), "test-key");
        assert!(station.list_tools(ancestors(&[])).await.unwrap().is_empty(), "无 group 时无本级工具");

        // 新增：注册该 group 的工具；重复新增幂等
        station.add_group("filesystem");
        station.add_group("filesystem");
        let tools = station.list_tools(ancestors(&[])).await.unwrap();
        assert_eq!(tools.iter().map(|t| t.name.to_string()).collect::<Vec<_>>(), vec!["read"], "新增 group 后工具可用");

        // 新增一个没有内置实现的 group：只有名字、没有工具，不影响已有工具
        let before = station.tools.get("read").unwrap().value().config.clone();
        station.add_group("unknown-group");
        let after = station.tools.get("read").unwrap().value().config.clone();
        assert!(Arc::ptr_eq(&before, &after), "新增其它 group 不影响已有工具");

        // 删除未内置的 group：不动 filesystem 的工具
        station.remove_group("unknown-group");
        assert!(station.tools.contains_key("read"), "删除其它 group 不影响已有工具");

        // 删除 filesystem：只移除该 group 的工具
        station.remove_group("filesystem");
        assert!(station.list_tools(ancestors(&[])).await.unwrap().is_empty(), "删除 group 后其工具失效");
    }

    #[tokio::test]
    async fn add_and_remove_sub_station_only_touches_that_station() {
        let fx = ts::init().await;
        let station = Station::from_repo(&repo_with_filesystem(), "test-key");
        let read_before = station.tools.get("read").unwrap().value().config.clone();
        let config = fx.sub_config(ts::SUB_TOGGLE);

        // 新增子站：远端工具可见、可路由；本级工具不受影响
        station.add_sub_station(&ArcStr::from(ts::SUB_TOGGLE), &config);
        let tools = station.list_tools(ancestors(&[])).await.unwrap();
        assert_eq!(tools.iter().map(|t| t.name.to_string()).collect::<Vec<_>>(), vec!["mock-tool", "read"]);
        let called = station.call_tool(ArcStr::from("mock-tool"), json!({}), ancestors(&[])).await.1.unwrap();
        assert_eq!(called.structured_content["mock"], "mock-tool");
        let read_after = station.tools.get("read").expect("本级工具应保留").value().config.clone();
        assert!(Arc::ptr_eq(&read_before, &read_after), "子站变更不重建本级工具");

        // 删除子站：只拆该子站运行态与路由（即使路由表残留旧值也不转发）
        station.remove_sub_station(&ArcStr::from(ts::SUB_TOGGLE));
        set_routes(&station, &[("mock-tool", ts::SUB_TOGGLE)]);
        let tools = station.list_tools(ancestors(&[])).await.unwrap();
        assert_eq!(tools.iter().map(|t| t.name.to_string()).collect::<Vec<_>>(), vec!["read"], "删除子站后其工具消失");
        let (_, err) = station.call_tool(ArcStr::from("mock-tool"), json!({}), ancestors(&[])).await;
        assert!(err.is_err(), "删除子站后不路由（视为工具不存在）");
    }

    #[tokio::test]
    async fn sub_station_change_clears_tool_routes() {
        // 子站增删后整张路由表失效（由下一次 list_tools 重建），避免残留指向旧名单的条目
        let station = Station::from_repo(&repo_with_filesystem(), "test-key");
        set_routes(&station, &[("a-tool", "sub-a"), ("b-tool", "sub-b")]);
        station.remove_sub_station(&ArcStr::from("sub-a"));
        assert!(station.tool_routes.load().is_empty(), "子站删除后路由表清空");

        set_routes(&station, &[("a-tool", "sub-a"), ("b-tool", "sub-b")]);
        let fx = ts::init().await;
        station.add_sub_station(&ArcStr::from(ts::SUB_TOGGLE), &fx.sub_config(ts::SUB_TOGGLE));
        assert!(station.tool_routes.load().is_empty(), "子站新增后路由表清空");
    }

    #[tokio::test]
    async fn from_repo_skips_disabled_sub_station() {
        // 是否装配由调用方判断：from_repo 只装配配置里启用的子站（add_sub_station 自身不判断 enabled）
        let fx = ts::init().await;
        let station = Station::from_repo(&fx.repo_with(&[ts::SUB_DISABLED]), "test-key");
        assert!(station.sub_stations.is_empty(), "停用子站不装配运行态");

        // 调用方显式装配时（管理 API 启用路径）直接生效，与配置里的 enabled 无关
        let repo = fx.repo_with(&[ts::SUB_TOGGLE]);
        let station = Station::from_repo(&repo, "test-key");
        assert!(station.sub_stations.contains_key(ts::SUB_TOGGLE), "启用子站正常装配");
    }
}

//! 测试用进程级装配：统一注册 ConfigManager / Nexus / Station 单例，并持有共享 mock MCP 服务端。
//! 三者都是进程级单例（只能注册一次），且 Station 运行期按 id 现读 ConfigManager 的子站配置，
//! 因此各测试模块共用这里的装配，避免各自初始化导致 data_dir / station.json 不一致。

use std::sync::Arc;

use arcstr::ArcStr;
use async_trait::async_trait;
use kissbot_api::ContentBlock;
use kissbot_mcp::{
    CallToolResult, DiscoverResult, Implementation, McpError, McpHandler, Tool, ToolsCallParams,
    ToolsListParams, ToolsListResult,
};
use serde_json::json;
use tokio::sync::OnceCell;

use crate::config_manager::ConfigManager;
use crate::configs::{StationRepo, SubStationConfig};
use crate::nexus::Nexus;
use crate::station::Station;

/// 本级 station_id（测试里的 Station 都用它，便于断言祖先链）
pub const STATION_ID: &str = "station-self";
/// 共享子站 id（与装配出的 station.json 子站一一对应）
pub const SUB_PLAIN: &str = "plain"; // 普通 mock 服务端（工具 mock-tool，无 group）
pub const SUB_GROUPED: &str = "grouped"; // 工具定义回带 group 的 mock 服务端
pub const SUB_PREFIXED: &str = "prefixed"; // 指向普通服务端，前缀 a/
pub const SUB_TOGGLE: &str = "toggle"; // 独立服务端，用于启停测试
pub const SUB_READ: &str = "read-named"; // 工具名为 read 的服务端（本级同名工具优先）
pub const SUB_UNREACHABLE: &str = "unreachable"; // 无服务端监听
pub const SUB_DISABLED: &str = "disabled"; // 配置停用
pub const SUB_CHANNEL: &str = "station-a"; // 管理 API 测试用（默认停用）
/// 前缀子站使用的前缀
pub const PREFIXED_PREFIX: &str = "a/";

/// tools/list 观测记录：服务端收到的祖先链
pub type ListObservation = Arc<tokio::sync::Mutex<Option<Vec<ArcStr>>>>;

/// 测试用 mock MCP 服务端：工具名与是否携带 group 可配，记录收到的祖先链
struct MockHandler {
    tool_name: ArcStr,
    echo_group: bool,
    observed: ListObservation,
}

#[async_trait]
impl McpHandler for MockHandler {
    fn server_info(&self) -> Arc<Implementation> {
        Arc::new(Implementation::new(ArcStr::from("mock-mcp"), ArcStr::from("1.0.0")))
    }

    async fn discover(&self) -> DiscoverResult {
        DiscoverResult::new(None)
    }

    async fn list_tools(&self, params: ToolsListParams) -> std::result::Result<ToolsListResult, McpError> {
        *self.observed.lock().await = params.meta.ancestors.as_ref().map(|a| a.as_ref().clone());
        let mut tool = Tool::new(self.tool_name.clone(), ArcStr::from("第三方工具"), json!({ "type": "object" }));
        if self.echo_group {
            tool.set_group(ArcStr::from("mock-group"));
        }
        Ok(ToolsListResult::new(vec![Arc::new(tool)]))
    }

    async fn call_tool(&self, params: ToolsCallParams) -> std::result::Result<CallToolResult, McpError> {
        // 按工具名模拟两类返回：第三方错误体（foreign-tool）、正常结果
        match params.name.as_str() {
            "foreign-tool" => Ok(CallToolResult {
                content: vec![ContentBlock::text(ArcStr::from("boom"))],
                structured_content: json!({ "error": "boom" }),
                is_error: true,
                ..Default::default()
            }),
            _ => {
                let value = json!({ "mock": params.name.to_string() });
                Ok(CallToolResult {
                    content: vec![ContentBlock::text(ArcStr::from(value.to_string()))],
                    structured_content: value,
                    ..Default::default()
                })
            }
        }
    }
}

/// 在独立线程的 runtime 上启动 mock MCP 服务端：线程与 runtime 常驻，
/// 服务端可被不同测试（不同 tokio 测试 runtime）复用
fn spawn_mock(tool_name: &'static str, echo_group: bool) -> (String, ListObservation) {
    let observed: ListObservation = Arc::new(tokio::sync::Mutex::new(None));
    let handler_observed = observed.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let handler = Arc::new(MockHandler {
                tool_name: ArcStr::from(tool_name),
                echo_group,
                observed: handler_observed,
            });
            let app = kissbot_mcp::router(kissbot_mcp::McpServer::new(handler), kissbot_mcp::McpHttpConfig::default());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(format!("http://{}", listener.local_addr().unwrap())).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
    (rx.recv().unwrap(), observed)
}

/// 进程级测试装配（首次使用时初始化）
pub struct Fixture {
    dir: tempfile::TempDir,
    repo: StationRepo,
    plain_observed: ListObservation,
}

static INIT: OnceCell<Fixture> = OnceCell::const_new();

/// 初始化并返回进程级装配（幂等）：写 config.json / station.json → 注册三个单例
pub async fn init() -> &'static Fixture {
    INIT.get_or_init(|| async {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        // mock MCP 服务端（独立线程常驻；端口随机）
        let (plain, plain_observed) = spawn_mock("mock-tool", false);
        let (grouped, _) = spawn_mock("mock-tool", true);
        let (toggle, _) = spawn_mock("mock-tool", false);
        let (read_named, _) = spawn_mock("read", false);

        // station.json：本级不声明 group；子站覆盖测试需要的各种连接形态
        let station_json = json!({
            "station_id": STATION_ID,
            "groups": [],
            "sub_stations": {
                SUB_PLAIN: { "station_id": SUB_PLAIN, "base_url": plain, "timeout_secs": 5 },
                SUB_GROUPED: { "station_id": SUB_GROUPED, "base_url": grouped, "timeout_secs": 5 },
                SUB_PREFIXED: { "station_id": SUB_PREFIXED, "base_url": plain, "timeout_secs": 5, "prefix": PREFIXED_PREFIX },
                SUB_TOGGLE: { "station_id": SUB_TOGGLE, "base_url": toggle, "timeout_secs": 5 },
                SUB_READ: { "station_id": SUB_READ, "base_url": read_named, "timeout_secs": 5 },
                SUB_UNREACHABLE: { "station_id": SUB_UNREACHABLE, "base_url": "http://127.0.0.1:1", "timeout_secs": 1 },
                SUB_DISABLED: { "station_id": SUB_DISABLED, "base_url": plain, "timeout_secs": 5, "enabled": false },
                SUB_CHANNEL: { "station_id": SUB_CHANNEL, "base_url": "http://127.0.0.1:9001", "timeout_secs": 5, "enabled": false }
            }
        });
        std::fs::write(data_dir.join("station.json"), station_json.to_string()).unwrap();

        let cfg_path = dir.path().join("config.json");
        let cfg_json = format!(
            r#"{{"api":{{"memory_store_url":"","memory_ego_url":""}},"security":{{"api_key":"user-key-456","admin_api_key":"admin-key-123"}},"agent":{{"data_dir":"{}","mgmt_host":"127.0.0.1","mgmt_port":9090,"ws_reconnect_interval_secs":5}}}}"#,
            data_dir.to_str().unwrap()
        );
        std::fs::write(&cfg_path, cfg_json).unwrap();
        // 2024 edition：设置环境变量需要 unsafe
        unsafe { std::env::set_var("KISSBOT_CONFIG", cfg_path.to_str().unwrap()) };

        ConfigManager::new().await.unwrap();
        let _ = Nexus::new().await;
        Station::new().await;

        let repo = ConfigManager::get().station_repo_snapshot().await;
        Fixture { dir, repo, plain_observed }
    }).await
}

impl Fixture {
    /// 读回落盘的 nexus.json
    pub fn saved_nexus_json(&self) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(self.dir.path().join("data").join("nexus.json")).unwrap()).unwrap()
    }

    /// 读回落盘的 station.json
    pub fn saved_station_json(&self) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(self.dir.path().join("data").join("station.json")).unwrap()).unwrap()
    }

    /// 只保留指定子站的配置副本（决定 Station::from_repo 建立哪些运行态；cfg 仍从 ConfigManager 现读）
    pub fn repo_with(&self, subs: &[&str]) -> StationRepo {
        let mut repo = StationRepo { station_id: ArcStr::from(STATION_ID), ..StationRepo::default() };
        let map = Arc::make_mut(&mut repo.sub_stations);
        for id in subs {
            let config = self.repo.sub_stations.get(*id).expect("预置子站应存在").clone();
            map.insert(ArcStr::from(*id), config);
        }
        repo
    }

    /// 预置子站配置
    pub fn sub_config(&self, id: &str) -> Arc<SubStationConfig> {
        self.repo.sub_stations.get(id).expect("预置子站应存在").clone()
    }

    /// 普通 mock 服务端的祖先链观测记录（断言 tools/list 附带的 ancestors）
    pub fn plain_observed(&self) -> ListObservation {
        self.plain_observed.clone()
    }
}

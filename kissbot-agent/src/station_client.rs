use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

use kissbot_api::ApiResponse;

use crate::config_manager::{McpConfig, ToolConfig};
use crate::types::{Error, Result, StationCallToolRequest, StationListMcpsRequest, StationListToolsRequest};

/// 子 Station HTTP 通信客户端（子 Station 只能通过 HTTP 通信，不可本地调用）
pub struct StationClient {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl StationClient {
    pub fn new(base_url: &str, default_timeout_secs: u64, api_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(default_timeout_secs))
                .build()
                .unwrap_or_default(),
        }
    }

    /// 查询子 Station 平铺后的工具元数据（可选 toolkit 白名单过滤；None = 全部）
    pub async fn list_tools(&self, filter: Option<&HashSet<String>>, ancestors: &[String]) -> Result<Vec<ToolConfig>> {
        let req = StationListToolsRequest {
            filter: filter.map(|set| set.iter().cloned().collect()),
            ancestors: ancestors.to_vec(),
        };
        let resp = self.post::<_, Vec<ToolConfig>>("/station/tools", &req).await?;
        Ok(resp)
    }

    /// 查询子 Station 平铺后的 MCP 元数据（可选 toolkit 白名单过滤）
    pub async fn list_mcps(&self, filter: Option<&HashSet<String>>, ancestors: &[String]) -> Result<Vec<McpConfig>> {
        let req = StationListMcpsRequest {
            filter: filter.map(|set| set.iter().cloned().collect()),
            ancestors: ancestors.to_vec(),
        };
        let resp = self.post::<_, Vec<McpConfig>>("/station/mcps", &req).await?;
        Ok(resp)
    }

    /// 调用子 Station 上的工具
    pub async fn call_tool(&self, name: &str, params: Value, ancestors: &[String]) -> Result<Value> {
        let req = StationCallToolRequest {
            tool_name: name.to_string(),
            parameters: params,
            ancestors: ancestors.to_vec(),
        };
        let resp = self.post::<_, Value>("/station/call-tool", &req).await?;
        Ok(resp)
    }

    /// 发送 POST JSON，统一解析 ApiResponse<T>；非 2xx 或 success=false 均映射为错误
    async fn post<T: Serialize, R: DeserializeOwned>(&self, path: &str, body: &T) -> Result<R> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.client
            .post(&url)
            .header("X-Api-Key", &self.api_key)
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if status.is_success() {
            let api: ApiResponse<R> = serde_json::from_slice(&bytes)?;
            if api.success {
                api.data.ok_or_else(|| Error::StationConnectionError("station 响应缺少 data".to_string()))
            } else {
                // HTTP 200 + success=false 表示远端工具调用失败（业务错误），交由上层按工具失败处理
                Err(Error::InternalError(api.error.unwrap_or_else(|| "station 返回错误".to_string())))
            }
        } else {
            // 非 2xx：尝试从 ApiResponse 提取 error 文案；无法解析则用状态码
            let message = serde_json::from_slice::<ApiResponse<Value>>(&bytes)
                .ok()
                .and_then(|r| r.error)
                .unwrap_or_else(|| format!("station HTTP {}: {}", status.as_u16(), url));
            Err(Error::StationConnectionError(message))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;
    use axum::{Json, Router};
    use kissbot_api::ApiResponse;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::net::TcpListener as TokioTcpListener;

    #[tokio::test]
    async fn client_parses_list_tools_success() {
        let (base, _guard) = spawn_test_server().await;
        let client = StationClient::new(&base, 5, "test-key");
        let filter: HashSet<String> = ["filesystem".to_string()].into_iter().collect();
        let tools = client.list_tools(Some(&filter), &["station-a".to_string()]).await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name.as_str(), "read");
    }

    #[tokio::test]
    async fn client_parses_call_tool_failure_as_error() {
        let (base, _guard) = spawn_test_server().await;
        let client = StationClient::new(&base, 5, "test-key");
        let err = client.call_tool("fail", json!({}), &[]).await.unwrap_err();
        assert!(err.to_string().contains("boom"), "应透传服务端错误: {}", err);
    }

    // 返回 (base_url, guard)；guard 停止服务器
    async fn spawn_test_server() -> (String, Arc<tokio::sync::OnceCell<()>>) {
        // 用 TcpListener 绑定 127.0.0.1:0 获取空闲端口
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let app = Router::new()
            .route("/station/tools", post(handle_tools))
            .route("/station/mcps", post(handle_mcps))
            .route("/station/call-tool", post(handle_call));
        let tcp = TokioTcpListener::bind(addr).await.unwrap();
        let guard = Arc::new(tokio::sync::OnceCell::new());
        let g = guard.clone();
        tokio::spawn(async move {
            let _ = axum::serve(tcp, app).await;
            let _ = g.set(());
        });
        (format!("http://{}", addr), guard)
    }

    async fn handle_tools() -> Json<ApiResponse<Vec<ToolConfig>>> {
        Json(ApiResponse::success(vec![ToolConfig {
            name: Arc::new("read".into()),
            description: Arc::new("读取文件".into()),
            parameters: Arc::new(json!({})),
        }]))
    }

    async fn handle_mcps() -> Json<ApiResponse<Vec<McpConfig>>> {
        Json(ApiResponse::success(vec![]))
    }

    async fn handle_call(Json(req): Json<StationCallToolRequest>) -> Json<ApiResponse<Value>> {
        if req.tool_name == "fail" {
            Json(ApiResponse::error("boom".to_string()))
        } else {
            Json(ApiResponse::success(json!({"ok": true})))
        }
    }
}

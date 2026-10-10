//! 摘要记忆（memory-struct-abstract）REST 客户端，与 memory_ego_client 同模式：
//! 共享 reqwest client + 组件地址（api.memory_struct_abstract_url）+ api_key（security.api_key）。
//! 接口约定见 [memory-struct-abstract 技术规格](../../docs/spec/memory-struct-abstract.md)。

use arcstr::ArcStr;
use kissbot_api::{
    ApiResponse, CaseNode, CaseRecordsResponse, CaseRecordsTailRequest, CaseSummaryRequest,
    CaseSummaryResponse, LocateMessage, LocateRequest, LocateResponse, RoleEvent,
};
use kissbot_security::HEADER_API_KEY;
use serde::{Serialize, de::DeserializeOwned};

use crate::types::{Error, Result};

/// 摘要记忆 REST 客户端：请求走 `X-Api-Key`；`memory_struct_abstract_url` 为空时
/// 所有调用在发请求前返回 Err（调用方降级为普通合批）
pub struct AbstractClient {
    client: reqwest::Client,
    base_url: ArcStr,
    api_key: ArcStr,
}

impl AbstractClient {
    /// 从进程级单例读取配置构造（ApiConfig.memory_struct_abstract_url / SecurityConfig.api_key）
    pub fn new() -> Self {
        Self::with_url(
            kissbot_api::ApiConfig::get().memory_struct_abstract_url.clone(),
            kissbot_security::SecurityConfig::get().api_key.clone(),
        )
    }

    /// 显式地址构造（单测用；不依赖进程级配置单例）
    pub fn with_url(base_url: ArcStr, api_key: ArcStr) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            api_key,
        }
    }

    /// POST /abstract/locate：定位一批消息涉及的 case（摘要与原文副本由调用方按 case 标识另取）
    pub async fn locate(&self, agent_id: &str, role_event: &RoleEvent, messages: Vec<LocateMessage>) -> Result<LocateResponse> {
        self.post(
            "/abstract/locate",
            &LocateRequest {
                agent_id: ArcStr::from(agent_id),
                role_event: role_event.clone(),
                messages,
            },
        )
        .await
    }

    /// POST /abstract/case/summary：取 case 摘要（返回节点本身：摘要文本 + 层级 + 记录数 + 起止时间）
    pub async fn case_summary(&self, agent_id: &str, role_event: &RoleEvent, case_name: &str) -> Result<CaseNode> {
        let resp: CaseSummaryResponse = self
            .post(
                "/abstract/case/summary",
                &CaseSummaryRequest {
                    agent_id: ArcStr::from(agent_id),
                    role_event: role_event.clone(),
                    case_name: ArcStr::from(case_name),
                },
            )
            .await?;
        Ok(resp.case.as_ref().clone())
    }

    /// POST /abstract/case/records-tail：取 case 结尾 count 条原文副本（升序）
    pub async fn case_records_tail(&self, agent_id: &str, role_event: &RoleEvent, case_name: &str, count: u64) -> Result<CaseRecordsResponse> {
        self.post(
            "/abstract/case/records-tail",
            &CaseRecordsTailRequest {
                agent_id: ArcStr::from(agent_id),
                role_event: role_event.clone(),
                case_name: ArcStr::from(case_name),
                count,
            },
        )
        .await
    }

    /// 统一 POST + ApiResponse 解包：未配置地址 / 网络失败 / 解析失败 / 业务失败都返回 Err
    async fn post<Req, Resp>(&self, path: &str, request: &Req) -> Result<Resp>
    where
        Req: Serialize + Sync,
        Resp: DeserializeOwned,
    {
        if self.base_url.is_empty() {
            return Err(Error::AbstractError(
                "摘要记忆未配置（api.memory_struct_abstract_url 为空）".into(),
            ));
        }
        let resp = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header(&*HEADER_API_KEY, self.api_key.as_str())
            .json(request)
            .send()
            .await
            .map_err(|e| Error::AbstractError(format!("{} 请求失败: {}", path, e).into()))?;
        let envelope = resp
            .json::<ApiResponse<Resp>>()
            .await
            .map_err(|e| Error::AbstractError(format!("{} 响应解析失败: {}", path, e).into()))?;
        envelope.data.ok_or_else(|| {
            Error::AbstractError(format!("{} 返回失败: {}", path, envelope.error.unwrap_or_default()).into())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_base_url_returns_abstract_error() {
        // 摘要记忆未配置：请求前直接返回 Err（调用方降级为普通合批），不发网络请求
        let client = AbstractClient::with_url(ArcStr::new(), ArcStr::new());
        let err = client.locate("0", &RoleEvent::new(ArcStr::from("dev")), vec![]).await.unwrap_err();
        assert!(matches!(err, Error::AbstractError(_)), "未配置地址应返回 AbstractError: {:?}", err);
    }
}

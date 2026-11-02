//! 摘要记忆（memory-struct-summary）REST 客户端，与 memory_ego_client 同模式：
//! 共享 reqwest client + 组件地址（api.memory_struct_summary_url）+ api_key（security.api_key）。
//! 接口约定见 [memory-struct-summary 技术规格](../../docs/spec/memory-struct-summary.md)。

use std::sync::Arc;

use arcstr::ArcStr;
use kissbot_api::{
    ApiResponse, CaseNode, CaseRecordsResponse, CaseRecordsTailRequest, CaseSummaryRequest,
    ChannelKey, LocateMessage, LocateRequest, LocateResponse,
};
use kissbot_security::HEADER_API_KEY;
use serde::{Serialize, de::DeserializeOwned};

use crate::types::{Error, MemoryKey, Result};

/// 摘要记忆 REST 客户端：请求走 `X-Api-Key`；`memory_struct_summary_url` 为空时
/// 所有调用在发请求前返回 Err（调用方降级为普通合批）
pub struct SummaryClient {
    client: reqwest::Client,
    base_url: ArcStr,
    api_key: ArcStr,
}

impl SummaryClient {
    /// 从进程级单例读取配置构造（ApiConfig.memory_struct_summary_url / SecurityConfig.api_key）
    pub fn new() -> Self {
        Self::with_url(
            kissbot_api::ApiConfig::get().memory_struct_summary_url.clone(),
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

    /// POST /summary/locate：定位一批消息（同一通道）涉及的 case（摘要与原文副本由调用方按 case 标识另取）
    pub async fn locate(
        &self,
        memory_key: &MemoryKey,
        channel: Arc<ChannelKey>,
        messages: Vec<LocateMessage>,
    ) -> Result<LocateResponse> {
        self.post(
            "/summary/locate",
            &LocateRequest {
                agent_id: memory_key.agent_id.clone(),
                role_event: memory_key.role_event.clone(),
                channel,
                messages,
            },
        ).await
    }

    /// POST /summary/case/summary：取 case 摘要（返回节点本身：摘要文本 + 层级 + 记录数 + 起止时间）
    pub async fn case_summary(&self, memory_key: &MemoryKey, case_name: ArcStr) -> Result<CaseNode> {
        self.post(
            "/summary/case/summary",
            &CaseSummaryRequest {
                agent_id: memory_key.agent_id.clone(),
                role_event: memory_key.role_event.clone(),
                case_name,
            },
        ).await
    }

    /// POST /summary/case/records-tail：取 case 结尾 count 条原文副本（升序）
    pub async fn case_records_tail(&self, memory_key: &MemoryKey, case_name: ArcStr, count: u64) -> Result<CaseRecordsResponse> {
        self.post(
            "/summary/case/records-tail",
            &CaseRecordsTailRequest {
                agent_id: memory_key.agent_id.clone(),
                role_event: memory_key.role_event.clone(),
                case_name,
                count,
            },
        ).await
    }

    /// 统一 POST + ApiResponse 解包：未配置地址 / 网络失败 / 解析失败 / 业务失败都返回 Err
    async fn post<Req, Resp>(&self, path: &str, request: &Req) -> Result<Resp>
    where
        Req: Serialize + Sync,
        Resp: DeserializeOwned,
    {
        if self.base_url.is_empty() {
            return Err(Error::SummaryError(
                "摘要记忆未配置（api.memory_struct_summary_url 为空）".into(),
            ));
        }
        let resp = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header(&*HEADER_API_KEY, self.api_key.as_str())
            .json(request)
            .send()
            .await
            .map_err(|e| Error::SummaryError(format!("{} 请求失败: {}", path, e).into()))?;
        let envelope = resp
            .json::<ApiResponse<Resp>>()
            .await
            .map_err(|e| Error::SummaryError(format!("{} 响应解析失败: {}", path, e).into()))?;
        envelope.data.ok_or_else(|| {
            Error::SummaryError(format!("{} 返回失败: {}", path, envelope.error.unwrap_or_default()).into())
        })
    }
}

#[cfg(test)]
mod tests {
    use kissbot_api::RoleEvent;

    use super::*;

    #[tokio::test]
    async fn empty_base_url_returns_summary_error() {
        // 摘要记忆未配置：请求前直接返回 Err（调用方降级为普通合批），不发网络请求
        let client = SummaryClient::with_url(ArcStr::new(), ArcStr::new());
        let channel = Arc::new(ChannelKey::new(ArcStr::from("web"), ArcStr::from("self"), ArcStr::from("g1")));
        let memory_key = MemoryKey { agent_id: ArcStr::from("0"), role_event: Arc::new(RoleEvent::new(ArcStr::from("dev"))) };
        let err = client.locate(&memory_key, channel, vec![]).await.unwrap_err();
        assert!(matches!(err, Error::SummaryError(_)), "未配置地址应返回 SummaryError: {:?}", err);
    }
}

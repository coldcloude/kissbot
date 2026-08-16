//! ego 服务 REST 客户端（与 memory_store_client 同模式）：共享 reqwest client + ego 配置单例读取

use std::sync::Arc;

use kissbot_api::{AgentMetadata, GetAgentRequest, GetIndividualsRequest, GetRoleRequest, IndividualRecognition, RolePlay};

use crate::types::{Error, Result};

/// ego 服务 REST 客户端：共享 reqwest client（连接池复用，替代原每次 Client::new()）+
/// ego base_url + api_key；方法封装 /agent、/individual、/role 接口
pub struct MemoryEgoClient {
    client: reqwest::Client,
    base_url: String,   // 构造时从 ApiConfig::get().memory_ego_url 读取
    api_key: String,    // 构造时从 SecurityConfig::get().api_key 读取
}

impl MemoryEgoClient {
    /// 从进程级单例读取 ego 配置构造（ApiConfig.memory_ego_url / SecurityConfig.api_key）
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: kissbot_api::ApiConfig::get().memory_ego_url.clone(),
            api_key: kissbot_security::SecurityConfig::get().api_key.to_string(),
        }
    }

    /// POST /agent/get 查询 agent 元数据；data null（agent 不存在）→ Ok(None)；网络/解析失败 → Err
    pub async fn get_agent(&self, agent_id: &str) -> Result<Option<AgentMetadata>> {
        let resp = self.client.post(format!("{}/agent/get", self.base_url))
            .header(kissbot_security::HEADER_API_KEY, self.api_key.as_str())
            .json(&GetAgentRequest { agent_id: Arc::new(agent_id.to_string()) })
            .send()
            .await
            .map_err(|e| Error::MemoryEgoError(format!("agent/get 请求失败: {}", e)))?;
        let envelope = resp.json::<kissbot_api::ApiResponse<AgentMetadata>>().await
            .map_err(|e| Error::MemoryEgoError(format!("agent/get 响应解析失败: {}", e)))?;
        Ok(envelope.data)
    }

    /// POST /individual/get-all 查询个体识别；网络/解析失败 → Err
    pub async fn get_individuals(&self, agent_id: &str) -> Result<Option<IndividualRecognition>> {
        let resp = self.client.post(format!("{}/individual/get-all", self.base_url))
            .header(kissbot_security::HEADER_API_KEY, self.api_key.as_str())
            .json(&GetIndividualsRequest { agent_id: Arc::new(agent_id.to_string()) })
            .send()
            .await
            .map_err(|e| Error::MemoryEgoError(format!("individual/get-all 请求失败: {}", e)))?;
        let envelope = resp.json::<kissbot_api::ApiResponse<IndividualRecognition>>().await
            .map_err(|e| Error::MemoryEgoError(format!("individual/get-all 响应解析失败: {}", e)))?;
        Ok(envelope.data)
    }

    /// POST /role/get 查询角色设定（role_name 空由调用方决定是否调用）；网络/解析失败 → Err
    pub async fn get_role(&self, agent_id: &str, role_name: &str) -> Result<Option<RolePlay>> {
        let resp = self.client.post(format!("{}/role/get", self.base_url))
            .header(kissbot_security::HEADER_API_KEY, self.api_key.as_str())
            .json(&GetRoleRequest {
                agent_id: Arc::new(agent_id.to_string()),
                role_name: Arc::new(role_name.to_string()),
            })
            .send()
            .await
            .map_err(|e| Error::MemoryEgoError(format!("role/get 请求失败: {}", e)))?;
        let envelope = resp.json::<kissbot_api::ApiResponse<RolePlay>>().await
            .map_err(|e| Error::MemoryEgoError(format!("role/get 响应解析失败: {}", e)))?;
        Ok(envelope.data)
    }

    /// agent 是否存在（data 非 null）；base_url 空 → Err("ego 未配置（memory_ego_url 为空）")
    pub async fn agent_exists(&self, agent_id: &str) -> Result<bool> {
        if self.base_url.is_empty() {
            return Err(Error::MemoryEgoError("ego 未配置（memory_ego_url 为空）".to_string()));
        }
        let resp = self.client.post(format!("{}/agent/get", self.base_url))
            .header(kissbot_security::HEADER_API_KEY, self.api_key.as_str())
            .json(&GetAgentRequest { agent_id: Arc::new(agent_id.to_string()) })
            .send()
            .await
            .map_err(|e| Error::MemoryEgoError(format!("agent/get 请求失败: {}", e)))?;
        let data: serde_json::Value = resp.json().await
            .map_err(|e| Error::MemoryEgoError(format!("agent/get 响应解析失败: {}", e)))?;
        Ok(!data["data"].is_null())
    }
}

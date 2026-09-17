use std::sync::Arc;

use async_trait::async_trait;

use crate::{nexus::Nexus, pipeline::AgentSystemPrompter, types::SessionKey};

pub const DEFAULT_SYSTEM_PROMPT: &str = "你是一个通用智能体。使用可用工具完成用户任务";

pub struct DefaultSystemPrompter {
    session_key: Arc<SessionKey>,
    prompt: Arc<String>,
}

impl DefaultSystemPrompter {
    pub fn new(session_key: Arc<SessionKey>) -> Self {
        Self {
            session_key,
            prompt: Arc::new(DEFAULT_SYSTEM_PROMPT.to_string()),
        }
    }

    fn from(session_key: Arc<SessionKey>, value: &str) -> Self {
        Self {
            session_key,
            prompt: Arc::new(value.to_string()),
        }
    }
}

#[async_trait]
impl AgentSystemPrompter for DefaultSystemPrompter {
    async fn reset_system_prompt(&self) {
        let session = Nexus::get().ensure_session(self.session_key.as_ref()).await;
        session.set_system_message(self.prompt.as_str().to_string());
    }
}

pub struct MemoryEgoSystemPrompter {
    session_key: Arc<SessionKey>,
}

impl MemoryEgoSystemPrompter {
    pub fn new(session_key: Arc<SessionKey>) -> Self {
        Self {
            session_key,
        }
    }
}

#[async_trait]
impl AgentSystemPrompter for MemoryEgoSystemPrompter {
    async fn reset_system_prompt(&self) {
        let nexus = Nexus::get();
        let prompt = if let Some(prompt) = nexus.system_prompt_for_agent(self.session_key.agent_id.as_str(), self.session_key.role_name.as_str()).await {
            prompt
        } else {
            DEFAULT_SYSTEM_PROMPT.to_string()
        };
        let session = nexus.ensure_session(self.session_key.as_ref()).await;
        session.set_system_message(prompt);
    }
}

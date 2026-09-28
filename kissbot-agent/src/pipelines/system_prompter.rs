use std::sync::Arc;

use async_trait::async_trait;

use kissbot_llm::Session;

use crate::{nexus::Nexus, pipeline::{AgentSystemPrompter, AgentTrigger}, types::SessionKey};

pub const DEFAULT_SYSTEM_PROMPT: &str = "你是一个通用智能体。使用可用工具完成用户任务";

pub struct DefaultSystemPrompter {
    session: Arc<Session>,
    prompt: Arc<String>,
}

impl DefaultSystemPrompter {
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            prompt: Arc::new(DEFAULT_SYSTEM_PROMPT.to_string()),
        }
    }

    fn from(session: Arc<Session>, value: &str) -> Self {
        Self {
            session,
            prompt: Arc::new(value.to_string()),
        }
    }
}

impl AgentTrigger for DefaultSystemPrompter {}

#[async_trait]
impl AgentSystemPrompter for DefaultSystemPrompter {
    async fn reset_system_prompt(&self) {
        self.session.set_system_message(self.prompt.as_str().to_string()).await;
    }
}

pub struct MemoryEgoSystemPrompter {
    session_key: Arc<SessionKey>,
    session: Arc<Session>,
}

impl MemoryEgoSystemPrompter {
    pub fn new(session_key: Arc<SessionKey>, session: Arc<Session>) -> Self {
        Self {
            session_key,
            session,
        }
    }
}

impl AgentTrigger for MemoryEgoSystemPrompter {}

#[async_trait]
impl AgentSystemPrompter for MemoryEgoSystemPrompter {
    async fn reset_system_prompt(&self) {
        let nexus = Nexus::get();
        let prompt = if let Some(prompt) = nexus.system_prompt_for_agent(self.session_key.agent_id.as_str(), self.session_key.role_event.role_name.as_str()).await {
            prompt
        } else {
            DEFAULT_SYSTEM_PROMPT.to_string()
        };
        self.session.set_system_message(prompt).await;
    }
}

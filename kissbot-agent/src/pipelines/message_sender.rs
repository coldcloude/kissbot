use std::sync::{Arc, atomic::{AtomicU64, Ordering::Relaxed}};

use arcstr::ArcStr;
use async_trait::async_trait;
use kissbot_llm::{EffectiveLLMConfig, LLMConfig, Message, ModelResponse, Session, ToolConfig};

use crate::{config_manager::ConfigManager, configs::{CompressConfig, EffectiveCompressConfig}, nexus::Nexus, pipeline::AgentMessageSender, types::{Error, Result, SessionKey}};

pub struct RawMessagerSender {
    pub session_key: Arc<SessionKey>,
    pub session: Arc<Session>,
}

#[async_trait]
impl AgentMessageSender for RawMessagerSender {
    async fn send_messages(&self, messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>) -> Result<ModelResponse> {
        self.session.context_append(messages).await;
        let full_messages = self.session.build_context().await;
        let llm_cfg = ConfigManager::get().session_config::<LLMConfig,EffectiveLLMConfig>(self.session_key.clone()).await;
        Nexus::get().call_provider_model(&llm_cfg, full_messages, tools).await
    }
}

#[async_trait]
trait TokenReducer: Send + Sync + 'static {
    async fn reduce(&self, messages: Vec<Message>) -> (u64, Vec<Message>);
}

pub struct TokenLimitMessageSender {
    session_key: Arc<SessionKey>,
    session: Arc<Session>,
    last_total_tokens: AtomicU64,
    reducer: Box<dyn TokenReducer>,
}

#[async_trait]
impl AgentMessageSender for TokenLimitMessageSender {
    async fn send_messages(&self, mut messages: Vec<Message>, tools: &Vec<Arc<ToolConfig>>) -> Result<ModelResponse> {
        let cfg_mngr = ConfigManager::get();
        let llm_cfg = cfg_mngr.session_config::<LLMConfig,EffectiveLLMConfig>(self.session_key.clone()).await;
        let compress_cfg = cfg_mngr.session_config::<CompressConfig,EffectiveCompressConfig>(self.session_key.clone()).await;
        let provider = llm_cfg.provider.as_str();
        let model = llm_cfg.model.as_str();
        let model_cfg = cfg_mngr.provider_model_config(provider, model).await
        .ok_or_else(|| Error::ModelProviderNotFound(ArcStr::from(provider), ArcStr::from(model)))?;

        // 1. 检查上下文 token 占用超限
        //    阈值来自会话模型的 max_tokens_usage
        //    上次模型响应的 usage.total_tokens 超过阈值触发
        //    无新消息不触发
        if model_cfg.max_tokens_usage > 0 {
            let mut tokens_usage = self.last_total_tokens.load(Relaxed);
            let max_tokens_usage = ((model_cfg.max_tokens_usage as f64) * compress_cfg.compress_threshold) as u64;
            if tokens_usage > max_tokens_usage {
                (tokens_usage, messages) = self.reducer.reduce(messages).await;
                // 保存本次请求 token 占用
                self.last_total_tokens.store(tokens_usage, Relaxed);
            }
        }
        // 2. 正常的发送流程
        self.session.context_append(messages).await;
        let full_messages = self.session.build_context().await;
        let response = Nexus::get().call_provider_model(&llm_cfg, full_messages, tools).await;
        if let Ok(model_resp) = response.as_ref() {
            // 保存本次请求 token 占用
            self.last_total_tokens.store(model_resp.total_tokens, Relaxed);
        }
        response
    }
}

struct CompressTokenReducer {
    session_key: Arc<SessionKey>,
    session: Arc<Session>,
}

#[async_trait]
impl TokenReducer for CompressTokenReducer {
    async fn reduce(&self, messages: Vec<Message>) -> (u64, Vec<Message>) {
        let cfg_mngr = ConfigManager::get();
        let llm_cfg = cfg_mngr.session_config::<LLMConfig,EffectiveLLMConfig>(self.session_key.clone()).await;
        let compress_cfg = cfg_mngr.session_config::<CompressConfig,EffectiveCompressConfig>(self.session_key.clone()).await;
        let c_msg = Message::User { content: compress_cfg.compress_prompt.clone() };
        self.session.context_append(vec![c_msg.clone()]).await;
        let compress_messages = self.session.build_context().await;
        let response = Nexus::get().call_provider_model(&llm_cfg, compress_messages, &vec![]).await;
        let mut tokens = 0 as u64;
        if let Ok(mut model_resp) = response {
            let sum_msg = Message::Assistant {
                content: model_resp.raw_content.take(),
                reasoning_content: model_resp.raw_reasoning_content.take(),
                tool_calls: model_resp.raw_tool_calls.take(),
            };
            self.session.context_archive_and_clear_cache_and_reset_messages(vec![c_msg,sum_msg]).await;
            tokens = model_resp.total_tokens;
        }
        (tokens, messages)
    }
}

struct MemoryRecoverTokenReducer {
    session_key: Arc<SessionKey>,
    session: Arc<Session>,
}

#[async_trait]
impl TokenReducer for MemoryRecoverTokenReducer {
    async fn reduce(&self, messages: Vec<Message>) -> (u64, Vec<Message>) {
        // 记忆打包：组合查询 + 每组合全史查询 + 并集算法（最后 N 条 ∪ [M, T_N] 同时间组，窗口内早于 T_N 的记录不含），
        // 按 is_self 合并为交替的 User/Assistant 消息（结尾为 User 时已补空 Assistant）；生成 OK 时直接使用打包结果
        let new_messages = Nexus::get().build_context_from_memory_store(self.session_key.clone()).await;
        // 归档旧上下文（新建时无内容幂等跳过）+ 清空缓存 → 重建（清空内存 + 从内存写回缓存；无消息不落盘）
        self.session.context_archive_and_clear_cache_and_reset_messages(new_messages).await;
        (0 as u64, messages)
    }
}

impl TokenLimitMessageSender {
    pub fn new_compress(session_key: Arc<SessionKey>, session: Arc<Session>) -> Self {
        let reducer = CompressTokenReducer {
            session_key: session_key.clone(),
            session: session.clone(),
        };
        Self {
            session_key,
            session,
            last_total_tokens: AtomicU64::new(0),
            reducer: Box::new(reducer),
        }
    }

    pub fn new_memory_recover(session_key: Arc<SessionKey>, session: Arc<Session>) -> Self {
        let reducer = MemoryRecoverTokenReducer {
            session_key: session_key.clone(),
            session: session.clone(),
        };
        Self {
            session_key,
            session,
            last_total_tokens: AtomicU64::new(0),
            reducer: Box::new(reducer),
        }
    }
}
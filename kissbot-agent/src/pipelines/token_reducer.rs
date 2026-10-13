use async_trait::async_trait;
use kissbot_llm::{EffectiveLLMConfig, LLMConfig, Message, Session};

use crate::{config_manager::ConfigManager, configs::{CompressConfig, EffectiveCompressConfig}, nexus::Nexus, pipeline::TokenReducer, pipelines::build_context_from_memory_store, types::SessionKey};

pub struct CompressTokenReducer;

#[async_trait]
impl TokenReducer for CompressTokenReducer {
    async fn reduce(&self, session_key: SessionKey, session: &Session, messages: Vec<Message>) -> (u64, Vec<Message>) {
        let cfg_mngr = ConfigManager::get();
        let llm_cfg = cfg_mngr.session_config::<LLMConfig,EffectiveLLMConfig>(session_key.clone()).await;
        let compress_cfg = cfg_mngr.session_config::<CompressConfig,EffectiveCompressConfig>(session_key.clone()).await;
        let c_msg = Message::User { content: compress_cfg.compress_prompt.clone() };
        session.context_append(vec![c_msg.clone()]).await;
        let compress_messages = session.build_context().await;
        let response = Nexus::get().provider_manager.call(&llm_cfg, compress_messages, &vec![]).await;
        let mut tokens = 0 as u64;
        if let Ok(mut model_resp) = response {
            let sum_msg = Message::Assistant {
                content: model_resp.raw_content.take(),
                reasoning_content: model_resp.raw_reasoning_content.take(),
                tool_calls: model_resp.raw_tool_calls.take(),
            };
            session.context_archive_and_clear_cache_and_reset_messages(vec![c_msg,sum_msg]).await;
            tokens = model_resp.total_tokens;
        }
        (tokens, messages)
    }
}

pub struct MemoryRecoverTokenReducer;

#[async_trait]
impl TokenReducer for MemoryRecoverTokenReducer {
    async fn reduce(&self, session_key: SessionKey, session: &Session, messages: Vec<Message>) -> (u64, Vec<Message>) {
        // 记忆打包：组合查询 + 每组合全史查询 + 并集算法（最后 N 条 ∪ [M, T_N] 同时间组，窗口内早于 T_N 的记录不含），
        // 按 is_self 合并为交替的 User/Assistant 消息（结尾为 User 时已补空 Assistant）；生成 OK 时直接使用打包结果
        if let SessionKey::Memory(mk) = session_key {
            let new_messages = build_context_from_memory_store(mk).await;
            // 归档旧上下文（新建时无内容幂等跳过）+ 清空缓存 → 重建（清空内存 + 从内存写回缓存；无消息不落盘）
            session.context_archive_and_clear_cache_and_reset_messages(new_messages).await;
        }
        (0 as u64, messages)
    }
}

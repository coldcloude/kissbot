use std::sync::Arc;

use async_trait::async_trait;
use arcstr::ArcStr;
use kissbot_llm::{EffectiveLLMConfig, Message, Session};

use crate::{config_manager::ConfigManager, configs::{EffectiveCompressConfig}, nexus::Nexus, pipeline::TokenReducer, pipelines::build_context_from_memory_store, types::MemoryKey};

pub fn end_by_trim_last_assistent(context: &Vec<Message>) -> usize {
    let mut curr = context.len() as i64 -1;
    while curr >= 0 {
        let msg = &context[curr as usize];
        curr -= 1;
        if matches!(msg, Message::Assistant { .. }) {
            return curr as usize + 1;
        }
    }
    context.len()
}

pub struct CompressTokenReducer;

impl CompressTokenReducer {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl TokenReducer for CompressTokenReducer {
    async fn reduce(&self, session_id: ArcStr, session: &Session, messages: Vec<Message>) -> (u64, Vec<Message>) {
        let cfg_mngr = ConfigManager::get();
        let llm_cfg = cfg_mngr.session_config::<EffectiveLLMConfig>(session_id.as_str()).await
            .unwrap_or_else(|| Arc::new(EffectiveLLMConfig::new()));
        let compress_cfg = cfg_mngr.session_config::<EffectiveCompressConfig>(session_id.as_str()).await
            .unwrap_or_else(|| Arc::new(EffectiveCompressConfig::new()));
        let c_msg = Message::User { content: compress_cfg.compress_prompt.clone() };

        // 1. 只读 build_context（读锁）：以最后一条 assistant（不含）为界切出要保留的消息
        let mut context = session.build_context().await;
        let end = end_by_trim_last_assistent(&context);
        let popped = context.split_off(end);

        // 2. 剩余上下文 + 压缩指令 → 模型总结
        context.push(c_msg.clone());
        let response = Nexus::get().provider_manager.call(&llm_cfg, context, &vec![]).await;
        let mut tokens = 0 as u64;
        if let Ok(mut model_resp) = response {
            let sum_msg = Message::Assistant {
                content: model_resp.raw_content.take(),
                reasoning_content: model_resp.raw_reasoning_content.take(),
                tool_calls: model_resp.raw_tool_calls.take(),
            };
            // 3. 一次重置（重置时应用待定系统消息）：压缩指令 + 总结 + 弹出的消息 + 新消息（新消息已并入 → 返回空）
            let mut new_messages = Vec::with_capacity(popped.len() + 2);
            new_messages.push(c_msg);
            new_messages.push(sum_msg);
            new_messages.extend(popped);
            let _ = session.context_archive_and_clear_cache_and_reset_messages(new_messages).await;
            tokens = model_resp.total_tokens;
        }
        (tokens, messages)
    }
}

pub struct MemoryRecoverTokenReducer {
    memory_key: Arc<MemoryKey>,
}

impl MemoryRecoverTokenReducer {
    pub fn new(memory_key: Arc<MemoryKey>) -> Self {
        Self { memory_key }
    }
}

#[async_trait]
impl TokenReducer for MemoryRecoverTokenReducer {
    /// 记忆回溯：不用发模型，只读取出「最后一条 assistant（不含）之后」的会话消息（尚未进记忆），
    /// 与 memory-store 打包的记忆上下文拼接后一次重置会话；新消息仍交回调用方追加。
    async fn reduce(&self, session_id: ArcStr, session: &Session, messages: Vec<Message>) -> (u64, Vec<Message>) {
        // 记忆打包：组合查询 + 每组合全史查询 + 并集算法（最后 N 条 ∪ [M, T_N] 同时间组，窗口内早于 T_N 的记录不含），
        // 按 is_self 合并为交替的 User/Assistant 消息（结尾为 User 时已补空 Assistant）；生成 OK 时直接使用打包结果
        // 1. 只读取出最后一条 assistant（含）之后的会话消息：这些还在当前对话里，回溯后要接在记忆之后
        let tail = session.context_read(|msgs| {
            let end = end_by_trim_last_assistent(msgs);
            msgs.iter().skip(end).cloned().collect::<Vec<_>>()
        }).await;
        // 2. 记忆上下文 + 会话尾部消息：尾部以 assistant 开头且记忆也以 assistant 结尾时去掉记忆最后一条
        let mut new_messages = build_context_from_memory_store(self.memory_key.clone(), session_id).await;
        new_messages.extend(tail);
        // 3. 一次重置（归档旧上下文 + 应用待定系统消息 + 清空缓存 + 写入拼接结果）
        let _ = session.context_archive_and_clear_cache_and_reset_messages(new_messages).await;
        (0 as u64, messages)
    }
}

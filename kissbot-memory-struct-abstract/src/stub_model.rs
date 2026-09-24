//! 桩模型：`provider_type = "stub"` 时启用，用确定性判定替代模型调用。
//!
//! 供集成测试与手动验证使用（spec 验证：模型调用以桩实现替换）：分组轮把整批消息归为一组
//! （有当前事件时标记为当前事件组，直接并入当前事件）、分层归并一律新开叶子事件、
//! 摘要为固定前缀，定位命中第一个候选事件。

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use kissbot_api::EventBrief;

use crate::error::Result;
use crate::model::{DecisionModel, LevelDecision, MessageGroup};
use crate::scope::ScopeId;

pub const STUB_SUMMARY_PREFIX: &str = "桩摘要";

#[derive(Default)]
pub struct StubModel {
    summary_count: AtomicU64,
}

impl StubModel {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl DecisionModel for StubModel {
    async fn group(&self, _scope: &ScopeId, _batch: &str, total: usize, current: Option<&EventBrief>) -> Result<Vec<MessageGroup>> {
        // 桩：整批归为一组；有当前事件即视为当前事件的延续 → 直接并入当前事件
        Ok(vec![MessageGroup { messages: (0..total).collect(), is_current: current.is_some() }])
    }

    async fn decide_level(&self, _scope: &ScopeId, _batch: &str, _level: &[EventBrief]) -> Result<LevelDecision> {
        Ok(LevelDecision::New)
    }

    async fn summarize(&self, _scope: &ScopeId, _content: &str) -> Result<String> {
        let next = self.summary_count.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(format!("{}{}", STUB_SUMMARY_PREFIX, next))
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::ScopeId;

    #[tokio::test]
    async fn stub_is_deterministic() {
        let model = StubModel::new();
        let scope = ScopeId::new("0", "dev");
        let current = vec![EventBrief {
            event_id: std::sync::Arc::new("e1".into()),
            summary: std::sync::Arc::new("当前事件".into()),
        }];
        let groups = model.group(&scope, "batch", 2, current.first()).await.unwrap();
        assert_eq!(groups, vec![MessageGroup { messages: vec![0, 1], is_current: true }]);
        let groups = model.group(&scope, "batch", 1, None).await.unwrap();
        assert_eq!(groups, vec![MessageGroup { messages: vec![0], is_current: false }], "没有当前事件时不标记");
        assert_eq!(model.decide_level(&scope, "seg", &[]).await.unwrap(), LevelDecision::New);
        assert_eq!(model.summarize(&scope, "content").await.unwrap(), "桩摘要1");
        assert_eq!(model.summarize(&scope, "content").await.unwrap(), "桩摘要2");
    }
}

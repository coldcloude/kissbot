//! 桩模型：`provider_type = "stub"` 时启用，用确定性判定替代模型调用。
//!
//! 供集成测试与手动验证使用（spec 验证：模型调用以桩实现替换）：分组轮把整批消息归为一组
//! （有当前 case 时标记为当前 case 组，直接并入当前 case）、分层归并一律新开叶子 case（同时给出名称与摘要）、
//! 摘要为固定前缀，定位命中第一个候选 case。

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use kissbot_api::CaseBrief;

use crate::error::Result;
use crate::model::{DecisionModel, LevelDecision, MessageGroup};
use crate::scope::ModelScope;

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
    async fn group(&self, _scope: &ModelScope, _batch: &str, total: usize, current: Option<&CaseBrief>) -> Result<Vec<MessageGroup>> {
        // 桩：整批归为一组；有当前 case 即视为当前 case 的延续 → 直接并入当前 case
        Ok(vec![MessageGroup { messages: (0..total).collect(), is_current: current.is_some() }])
    }

    async fn decide_level(&self, _scope: &ModelScope, _batch: &str, _level: &[CaseBrief]) -> Result<LevelDecision> {
        // 桩：一律新建 case，名称固定 "case"（重名时组件追加 -2、-3…），摘要与 summarize 同前缀
        Ok(LevelDecision::new_case("case", STUB_SUMMARY_PREFIX))
    }

    async fn summarize(&self, _scope: &ModelScope, _content: &str) -> Result<String> {
        let next = self.summary_count.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(format!("{}{}", STUB_SUMMARY_PREFIX, next))
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::{AgentRole, RoleEvent};

    #[tokio::test]
    async fn stub_is_deterministic() {
        let model = StubModel::new();
        let scope = ModelScope::new(&AgentRole::new("0", "dev"), &RoleEvent::parse("dev"));
        let current = vec![CaseBrief {
            case_id: std::sync::Arc::new("role-1".into()),
            summary: std::sync::Arc::new("当前 case".into()),
        }];
        let groups = model.group(&scope, "batch", 2, current.first()).await.unwrap();
        assert_eq!(groups, vec![MessageGroup { messages: vec![0, 1], is_current: true }]);
        let groups = model.group(&scope, "batch", 1, None).await.unwrap();
        assert_eq!(groups, vec![MessageGroup { messages: vec![0], is_current: false }], "没有当前 case 时不标记");
        assert_eq!(model.decide_level(&scope, "seg", &[]).await.unwrap(), LevelDecision::new_case("case", STUB_SUMMARY_PREFIX));
        assert_eq!(model.summarize(&scope, "content").await.unwrap(), "桩摘要1");
        assert_eq!(model.summarize(&scope, "content").await.unwrap(), "桩摘要2");
    }
}

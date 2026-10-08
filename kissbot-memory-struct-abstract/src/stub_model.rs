//! 桩模型：`provider_type = "stub"` 时启用，用确定性判定替代模型调用。
//!
//! 供集成测试与手动验证使用（spec 验证：模型调用以桩实现替换）：分组轮把整批消息归为一组
//! （有当前 case 时标记为当前 case 组，直接并入当前 case）、分层归并一律新开叶子 case（同时给出名称与摘要）、
//! 摘要为固定前缀，定位命中第一个候选 case。

use std::sync::atomic::{AtomicU64, Ordering};

use arcstr::ArcStr;
use async_trait::async_trait;
use kissbot_api::CaseBrief;

use crate::error::Result;
use crate::model::{DecisionModel, LevelDecision, MessageGroup};
use crate::agent_role::AgentRole;

pub const STUB_SUMMARY_PREFIX: ArcStr = arcstr::literal!("桩摘要");

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
    async fn group(&self, _agent_role: &AgentRole, _event_id: Option<ArcStr>, _batch: &str, total: usize, current: Option<&CaseBrief>) -> Result<Vec<MessageGroup>> {
        // 桩：整批归为一组；有当前 case 即视为当前 case 的延续 → 直接并入当前 case
        Ok(vec![MessageGroup { messages: (0..total).collect(), is_current: current.is_some() }])
    }

    async fn decide_level(&self, _agent_role: &AgentRole, _event_id: Option<ArcStr>, _batch: &str, _level: &[CaseBrief]) -> Result<LevelDecision> {
        // 桩：一律新建 case，名称固定 "case"（重名时组件追加 -2、-3…），摘要与 summarize 同前缀
        Ok(LevelDecision::new_case("case", STUB_SUMMARY_PREFIX.as_str()))
    }

    async fn summarize(&self, _agent_role: &AgentRole, _event_id: Option<ArcStr>, _content: &str) -> Result<ArcStr> {
        let next = self.summary_count.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(ArcStr::from(format!("{}{}", STUB_SUMMARY_PREFIX, next)))
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stub_is_deterministic() {
        let model = StubModel::new();
        let agent_role = AgentRole::new(ArcStr::from("0"), ArcStr::from("dev"));
        let current = vec![CaseBrief {
            case_name: ArcStr::from("role-1"),
            summary: ArcStr::from("当前 case"),
        }];
        let groups = model.group(&agent_role, None, "batch", 2, current.first()).await.unwrap();
        assert_eq!(groups, vec![MessageGroup { messages: vec![0, 1], is_current: true }]);
        let groups = model.group(&agent_role, None, "batch", 1, None).await.unwrap();
        assert_eq!(groups, vec![MessageGroup { messages: vec![0], is_current: false }], "没有当前 case 时不标记");
        assert_eq!(
            model.decide_level(&agent_role, None, "seg", &[]).await.unwrap(),
            LevelDecision::new_case("case", STUB_SUMMARY_PREFIX.as_str())
        );
        assert_eq!(model.summarize(&agent_role, None, "content").await.unwrap(), "桩摘要1");
        assert_eq!(model.summarize(&agent_role, None, "content").await.unwrap(), "桩摘要2");
    }
}

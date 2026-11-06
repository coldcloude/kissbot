//! 桩模型：`provider_type = "stub"` 时启用，用确定性判定替代模型调用。
//!
//! 供集成测试与手动验证使用（spec 验证：模型调用以桩实现替换）：分组轮把整批消息归为一组
//! （有当前 case 时标记为当前 case 组，直接并入当前 case）、逐层判定一律返回新名称 `case`；
//! 摘要为固定前缀。

use std::sync::atomic::{AtomicU64, Ordering};

use arcstr::ArcStr;
use async_trait::async_trait;
use kissbot_api::CaseBrief;

use crate::error::Result;
use crate::model::{DecisionModel, GroupItem, MessageGroup};
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
    async fn group(
        &self,
        _agent_role: &AgentRole,
        _event_id: Option<ArcStr>,
        items: &[GroupItem],
        current: Option<&CaseBrief>,
    ) -> Result<Vec<MessageGroup>> {
        // 桩：整批归为一组；有当前 case 即视为当前 case 的延续 → 直接并入当前 case
        Ok(vec![MessageGroup {
            msg_ids: items.iter().map(|item| item.msg_id.clone()).collect(),
            is_current: current.is_some(),
        }])
    }

    async fn decide_level(&self, _agent_role: &AgentRole, _event_id: Option<ArcStr>, _batch: &str, _level: &[CaseBrief]) -> Result<ArcStr> {
        // 桩：一律新建名为 "case" 的 case；同名兄弟由组件复用同一个节点
        Ok(ArcStr::from("case"))
    }

    async fn summarize(&self, _agent_role: &AgentRole, _event_id: Option<ArcStr>, _content: &str) -> Result<ArcStr> {
        let next = self.summary_count.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(ArcStr::from(format!("{}{}", STUB_SUMMARY_PREFIX, next)))
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(msg_id: &str) -> GroupItem {
        GroupItem {
            msg_id: ArcStr::from(msg_id),
            time: ArcStr::from("2026-09-22 10:00:00"),
            user_id: ArcStr::from("u1"),
            content: ArcStr::from("内容"),
        }
    }

    #[tokio::test]
    async fn stub_is_deterministic() {
        let model = StubModel::new();
        let agent_role = AgentRole::new(ArcStr::from("0"), ArcStr::from("dev"));
        let current = vec![CaseBrief {
            case_name: ArcStr::from("case"),
            summary: ArcStr::from("当前 case"),
        }];
        let groups = model.group(&agent_role, None, &[item("m1"), item("m2")], current.first()).await.unwrap();
        assert_eq!(
            groups,
            vec![MessageGroup { msg_ids: vec![ArcStr::from("m1"), ArcStr::from("m2")], is_current: true }]
        );
        let groups = model.group(&agent_role, None, &[item("m1")], None).await.unwrap();
        assert_eq!(groups, vec![MessageGroup { msg_ids: vec![ArcStr::from("m1")], is_current: false }], "没有当前 case 时不标记");
        assert_eq!(
            model.decide_level(&agent_role, None, "seg", &[]).await.unwrap(),
            ArcStr::from("case")
        );
        assert_eq!(model.summarize(&agent_role, None, "content").await.unwrap(), "桩摘要1");
        assert_eq!(model.summarize(&agent_role, None, "content").await.unwrap(), "桩摘要2");
    }
}

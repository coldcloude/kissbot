//! 测试用模型桩：按队列给出判定结果（默认整批一组并新开叶子 case），摘要按序号生成（spec 验证：模型调用以桩实现替换）。

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use arcstr::ArcStr;
use async_trait::async_trait;
use kissbot_api::CaseBrief;

use crate::error::Result;
use crate::model::{DecisionModel, GroupItem, LevelDecision, MessageGroup};
use crate::agent_role::AgentRole;

/// 预置分组：批次下标（组内消息在本批中的位置）+ 是否当前 case
pub type StubGroup = (Vec<usize>, bool);

#[derive(Default)]
pub struct StubModel {
    /// 第 1 轮（分组轮）结果；队列为空时默认整批一组（有当前 case 即标记为当前 case 组）
    pub group_decisions: Mutex<VecDeque<Vec<StubGroup>>>,
    pub level_decisions: Mutex<VecDeque<LevelDecision>>,
    /// 每次 summarize 的输入（断言级联时父 case 能看到子 case 摘要）
    pub summary_inputs: Mutex<Vec<ArcStr>>,
    /// 每次判定调用收到的提示词（断言调用顺序与候选集合）
    pub prompts: Mutex<Vec<ArcStr>>,
    /// 预置的摘要失败次数（各消耗一次，测试模型失败时状态不被破坏）
    pub fail_summarize: Mutex<VecDeque<()>>,
    pub summary_count: AtomicU64,
}

impl StubModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// 预置第 1 轮分组结果（下标范围必须完整覆盖批次，否则归档按失败处理）
    pub fn push_groups(&self, groups: Vec<StubGroup>) -> &Self {
        self.group_decisions.lock().unwrap().push_back(groups);
        self
    }

    /// 便捷写法：把一批消息分成一组
    pub fn push_group(&self, messages: &[usize], is_current: bool) -> &Self {
        self.push_groups(vec![(messages.to_vec(), is_current)])
    }

    pub fn push_level(&self, decision: LevelDecision) -> &Self {
        self.level_decisions.lock().unwrap().push_back(decision);
        self
    }

    pub fn push_summarize_failure(&self) -> &Self {
        self.fail_summarize.lock().unwrap().push_back(());
        self
    }

    pub fn prompts(&self) -> Vec<ArcStr> {
        self.prompts.lock().unwrap().clone()
    }

    pub fn summary_inputs(&self) -> Vec<ArcStr> {
        self.summary_inputs.lock().unwrap().clone()
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
        self.prompts.lock().unwrap().push(ArcStr::from(crate::model::grouping_prompt(items, current)));
        if let Some(groups) = self.group_decisions.lock().unwrap().pop_front() {
            // 与真实实现一致地校验覆盖：预置不完整时立即暴露，而不是静默丢记录
            let mut covered = vec![false; items.len()];
            for (indices, _) in &groups {
                for &index in indices {
                    if index >= items.len() || covered[index] {
                        return Err(crate::error::Error::ModelApiError(format!(
                            "桩分组下标非法或重复: {}（批次共 {} 条）",
                            index,
                            items.len()
                        ).into()));
                    }
                    covered[index] = true;
                }
            }
            if let Some(missing) = covered.iter().position(|covered| !covered) {
                return Err(crate::error::Error::ModelApiError(format!(
                    "桩分组未完整覆盖批次：漏掉下标 {}",
                    missing
                ).into()));
            }
            return Ok(groups
                .into_iter()
                .map(|(indices, is_current)| MessageGroup {
                    msg_ids: indices.into_iter().map(|index| items[index].msg_id.clone()).collect(),
                    is_current,
                })
                .collect());
        }
        // 默认：整批一组；有当前 case 即视为当前 case 的延续
        Ok(vec![MessageGroup {
            msg_ids: items.iter().map(|item| item.msg_id.clone()).collect(),
            is_current: current.is_some(),
        }])
    }

    async fn decide_level(&self, _agent_role: &AgentRole, _event_id: Option<ArcStr>, batch: &str, level: &[CaseBrief]) -> Result<LevelDecision> {
        self.prompts.lock().unwrap().push(ArcStr::from(crate::model::level_prompt(batch, level)));
        // 默认新开叶子 case：名称固定 "case"（重名时组件追加 -2、-3…），摘要一并给出
        Ok(self.level_decisions.lock().unwrap().pop_front().unwrap_or_else(|| LevelDecision::new_case("case", "新 case 摘要")))
    }

    async fn summarize(&self, _agent_role: &AgentRole, _event_id: Option<ArcStr>, content: &str) -> Result<ArcStr> {
        self.summary_inputs.lock().unwrap().push(ArcStr::from(content));
        if self.fail_summarize.lock().unwrap().pop_front().is_some() {
            return Err(crate::error::Error::ModelApiError("stub summarize failure".into()));
        }
        let next = self.summary_count.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(ArcStr::from(format!("摘要{}", next)))
    }

}

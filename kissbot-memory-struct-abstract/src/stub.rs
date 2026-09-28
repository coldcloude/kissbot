//! 测试用模型桩：按队列给出判定结果（默认整批一组并新开叶子 case），摘要按序号生成（spec 验证：模型调用以桩实现替换）。

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use arcstr::ArcStr;
use async_trait::async_trait;
use kissbot_api::CaseBrief;

use crate::error::Result;
use crate::model::{DecisionModel, LevelDecision, MessageGroup};
use crate::scope::ModelScope;

#[derive(Default)]
pub struct StubModel {
    /// 第 1 轮（分组轮）结果；队列为空时默认整批一组（有当前 case 即标记为当前 case 组）
    pub group_decisions: Mutex<VecDeque<Vec<MessageGroup>>>,
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
    pub fn push_groups(&self, groups: Vec<MessageGroup>) -> &Self {
        self.group_decisions.lock().unwrap().push_back(groups);
        self
    }

    /// 便捷写法：把一批消息分成「当前 case 组 + 若干普通组」
    pub fn push_group(&self, messages: &[usize], is_current: bool) -> &Self {
        self.push_groups(vec![MessageGroup { messages: messages.to_vec(), is_current }])
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
    async fn group(&self, _scope: &ModelScope, batch: &str, total: usize, current: Option<&CaseBrief>) -> Result<Vec<MessageGroup>> {
        self.prompts.lock().unwrap().push(ArcStr::from(crate::model::grouping_prompt(batch, current)));
        if let Some(groups) = self.group_decisions.lock().unwrap().pop_front() {
            // 与真实实现一致地校验覆盖：预置不完整时立即暴露，而不是静默丢记录
            let mut next = 0usize;
            for group in &groups {
                if group.messages.first() != Some(&next) {
                    return Err(crate::error::Error::ModelApiError(format!(
                        "桩分组未完整覆盖批次：期望下一组从下标 {} 开始，实际为 {:?}",
                        next, group.messages
                    ).into()));
                }
                next = group.messages.last().expect("桩分组非空") + 1;
            }
            if next != total {
                return Err(crate::error::Error::ModelApiError(format!(
                    "桩分组未完整覆盖批次：覆盖到下标 {}，批次共 {} 条",
                    next, total
                ).into()));
            }
            return Ok(groups);
        }
        // 默认：整批一组；有当前 case 即视为当前 case 的延续
        Ok(vec![MessageGroup { messages: (0..total).collect(), is_current: current.is_some() }])
    }

    async fn decide_level(&self, _scope: &ModelScope, batch: &str, level: &[CaseBrief]) -> Result<LevelDecision> {
        self.prompts.lock().unwrap().push(ArcStr::from(crate::model::level_prompt(batch, level)));
        // 默认新开叶子 case：名称固定 "case"（重名时组件追加 -2、-3…），摘要一并给出
        Ok(self.level_decisions.lock().unwrap().pop_front().unwrap_or_else(|| LevelDecision::new_case("case", "新 case 摘要")))
    }

    async fn summarize(&self, _scope: &ModelScope, content: &str) -> Result<ArcStr> {
        self.summary_inputs.lock().unwrap().push(ArcStr::from(content));
        if self.fail_summarize.lock().unwrap().pop_front().is_some() {
            return Err(crate::error::Error::ModelApiError("stub summarize failure".into()));
        }
        let next = self.summary_count.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(ArcStr::from(format!("摘要{}", next)))
    }

}

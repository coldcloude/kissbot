
// ========== 配置继承与 session 配置 ==========
//
// 两层存储，职责分开：
// - AgentRoleConfig / SessionConfigMergeMap：agent → role 的可继承配置（含 pipeline 选择）
// - SessionConfigMap：按 session_id 存的运行期有效配置（session 级覆盖或 pipeline 固化）
//
// 两者之间没有隐式继承：需要回退到 agent/role 时由 pipeline 自行组合。

use std::{collections::HashMap, sync::Arc};

use arcstr::ArcStr;
use serde::{Deserialize, Serialize};

use kissbot_api::{
    MergeBy, MergeEffectiveConfig, MergeSelf, OptionArcField, impl_option_arc_field, impl_option_arc_field_map,
};
use kissbot_llm::{EffectiveLLMConfig, LLMConfig};

use crate::{configs::nexus_repo::pipeline_config::*};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionConfigMergeMap {
    llm: Option<Arc<LLMConfig>>,
    compress: Option<Arc<CompressConfig>>,
    memory_recover: Option<Arc<MemoryRecoverConfig>>,
    channel_batch: Option<Arc<ChannelBatchConfig>>,
    out_channel: Option<Arc<OutChannelConfig>>,
    group_set: Option<Arc<GroupSetConfig>>,
    pipeline: Option<Arc<PipelineConfig>>,
}

impl_option_arc_field!(LLMConfig => llm, SessionConfigMergeMap);
impl_option_arc_field!(CompressConfig => compress, SessionConfigMergeMap);
impl_option_arc_field!(MemoryRecoverConfig => memory_recover, SessionConfigMergeMap);
impl_option_arc_field!(ChannelBatchConfig => channel_batch, SessionConfigMergeMap);
impl_option_arc_field!(OutChannelConfig => out_channel, SessionConfigMergeMap);
impl_option_arc_field!(GroupSetConfig => group_set, SessionConfigMergeMap);
impl_option_arc_field!(PipelineConfig => pipeline, SessionConfigMergeMap);

impl_option_arc_field_map!(SessionConfigMergeMap);

impl SessionConfigMergeMap {
    pub fn set<C>(&mut self, config: Arc<C>)
    where
        C: MergeSelf + Clone,
        SessionConfigMergeMap: OptionArcField<C>,
    {
        let config = if let Some(mut old_config) = self.take::<C>() {
            let old_config_mut = Arc::make_mut(&mut old_config);
            old_config_mut.merge(config.as_ref());
            old_config
        } else {
            config
        };
        self.insert::<C>(config);
    }
}

/// agent 级 context 配置（key = agent_id，覆盖全局默认；未配字段回落全局默认常量）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentRoleConfig {
    pub config_map: Arc<SessionConfigMergeMap>,
    /// key = role_name（role 覆盖 agent 默认）
    pub roles: Arc<HashMap<ArcStr, Arc<SessionConfigMergeMap>>>,
}

impl AgentRoleConfig {
    pub fn build<C>(&self, role_name: &str) -> C
    where
        C: MergeSelf,
        SessionConfigMergeMap: OptionArcField<C>,
    {
        let mut config = C::default();
        if let Some(agent_config) = self.config_map.get_deref::<C>() {
            config.merge(&agent_config);
        }
        if let Some(role_config_map) = self.roles.get(role_name) {
            if let Some(role_config) = role_config_map.get_deref::<C>() {
                config.merge(&role_config);
            }
        }
        config
    }

    pub fn set<C>(&mut self, role_name: ArcStr, config: Arc<C>)
    where
        C: MergeSelf + Clone,
        SessionConfigMergeMap: OptionArcField<C>,
    {
        if role_name.is_empty() {
            // 设置 agent 级配置
            let config_map = Arc::make_mut(&mut self.config_map);
            config_map.set::<C>(config);
        }
        else {
            let role_map = Arc::make_mut(&mut self.roles);
            // role 条目不存在则新建
            let role_config_map = if let Some(mut role_config_map) = role_map.remove(&role_name) {
                let role_config_map_mut = Arc::make_mut(&mut role_config_map);
                role_config_map_mut.set::<C>(config);
                role_config_map
            } else {
                let mut role_config_map = SessionConfigMergeMap::default();
                role_config_map.insert::<C>(config);
                Arc::new(role_config_map)
            };
            role_map.insert(role_name, role_config_map);
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionConfigMap {
    llm: Option<Arc<EffectiveLLMConfig>>,
    compress: Option<Arc<EffectiveCompressConfig>>,
    memory_recover: Option<Arc<EffectiveMemoryRecoverConfig>>,
    channel_batch: Option<Arc<ChannelBatchConfig>>,
    out_channel: Option<Arc<OutChannelConfig>>,
    group_set: Option<Arc<GroupSetConfig>>,
}

impl_option_arc_field!(EffectiveLLMConfig => llm, SessionConfigMap);
impl_option_arc_field!(EffectiveCompressConfig => compress, SessionConfigMap);
impl_option_arc_field!(EffectiveMemoryRecoverConfig => memory_recover, SessionConfigMap);
impl_option_arc_field!(ChannelBatchConfig => channel_batch, SessionConfigMap);
impl_option_arc_field!(OutChannelConfig => out_channel, SessionConfigMap);
impl_option_arc_field!(GroupSetConfig => group_set, SessionConfigMap);

impl_option_arc_field_map!(SessionConfigMap);

impl SessionConfigMap {
    pub fn get_or_insert<E>(&mut self, config: Arc<E>) -> Arc<E>
    where
        SessionConfigMap: OptionArcField<E>,
    {
        if let Some(exist_config) = self.get::<E>() {
            exist_config
        } else {
            self.insert::<E>(config.clone());
            config
        }
    }

    pub fn merge<C, E>(&mut self, config: &C)
    where
        C: MergeSelf + MergeEffectiveConfig<E>,
        E: MergeBy<C> + Clone,
        SessionConfigMap: OptionArcField<E>,
    {
        let eff_config = if let Some(mut eff_config) = self.take::<E>() {
            let eff_config_mut = Arc::make_mut(&mut eff_config);
            eff_config_mut.merge(config);
            eff_config
        } else {
            // 新建 session 配置
            let mut def_config = C::default();
            def_config.merge(config);
            Arc::new(def_config.get_effective_config())
        };
        self.insert::<E>(eff_config);
    }
}

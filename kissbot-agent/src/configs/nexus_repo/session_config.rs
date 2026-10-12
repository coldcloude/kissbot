
// ========== Context 配置（agent→role 三层继承） ==========

use std::{collections::HashMap, sync::Arc};

use arcstr::ArcStr;
use serde::{Deserialize, Serialize};

use kissbot_api::{
    MergeBy, MergeEffectiveConfig, MergeSelf, OptionArcField, impl_option_arc_field,
    impl_option_arc_field_map,
};
use kissbot_llm::{EffectiveLLMConfig, LLMConfig};

use crate::{configs::nexus_repo::pipeline_config::*, types::MemoryKey};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SessionConfigMergeMap {
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

trait SessionConfigMergeMapField<C: MergeSelf> {
    fn set(&mut self, config: Arc<C>);
}

macro_rules! impl_session_config_merge_map_field {
    ($ty:ty) => {
        impl SessionConfigMergeMapField<$ty> for SessionConfigMergeMap {
            fn set(&mut self, config: Arc<$ty>) {
                let config = if let Some(mut old_config) = self.take::<$ty>() {
                    let old_config_mut = Arc::make_mut(&mut old_config);
                    old_config_mut.merge(config.as_ref());
                    old_config
                } else {
                    config
                };
                self.insert::<$ty>(config);
            }
        }
    };
}

impl_session_config_merge_map_field!(LLMConfig);
impl_session_config_merge_map_field!(CompressConfig);
impl_session_config_merge_map_field!(MemoryRecoverConfig);
impl_session_config_merge_map_field!(ChannelBatchConfig);
impl_session_config_merge_map_field!(OutChannelConfig);
impl_session_config_merge_map_field!(GroupSetConfig);
impl_session_config_merge_map_field!(PipelineConfig);

impl SessionConfigMergeMap {
    pub fn set<C: MergeSelf>(&mut self, config: Arc<C>)
    where
        Self: SessionConfigMergeMapField<C>,
    {
        <Self as SessionConfigMergeMapField<C>>::set(self, config)
    }
}

/// agent 级 context 配置（key = agent_id，覆盖全局默认；未配字段回落全局默认常量）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentRoleConfig {
    config_map: Arc<SessionConfigMergeMap>,
    /// key = role_name（role 覆盖 agent 默认）
    roles: Arc<HashMap<ArcStr, Arc<SessionConfigMergeMap>>>,
}


pub trait AgentRoleConfigField<C: MergeSelf> {
    fn build(&self, role_name: &str) -> C;
    fn set(&mut self, role_name: ArcStr, config: Arc<C>);
}

macro_rules! impl_agent_role_config_field {
    ($ty:ty) => {
        impl AgentRoleConfigField<$ty> for AgentRoleConfig {
            fn build(&self, role_name: &str) -> $ty {
                let mut config = <$ty>::default();
                if let Some(agent_config) = self.config_map.get_deref::<$ty>() {
                    config.merge(&agent_config);
                }
                if let Some(role_config_map) = self.roles.get(role_name) {
                    if let Some(role_config) = role_config_map.get_deref::<$ty>() {
                        config.merge(&role_config);
                    }
                }
                config
            }
            fn set(&mut self, role_name: ArcStr, config: Arc<$ty>) {
                if role_name.is_empty() {
                    // 设置 agent 级配置
                    let config_map = Arc::make_mut(&mut self.config_map);
                    config_map.set::<$ty>(config);
                }
                else {
                    let role_map = Arc::make_mut(&mut self.roles);
                    // role 条目不存在则新建
                    let role_config_map = if let Some(mut role_config_map) = role_map.remove(&role_name) {
                        let role_config_map_mut = Arc::make_mut(&mut role_config_map);
                        role_config_map_mut.set::<$ty>(config);
                        role_config_map
                    } else {
                        let mut role_config_map = SessionConfigMergeMap::default();
                        role_config_map.insert::<$ty>(config);
                        Arc::new(role_config_map)
                    };
                    role_map.insert(role_name, role_config_map);
                }
            }
        }
    };
}

impl_agent_role_config_field!(LLMConfig);
impl_agent_role_config_field!(CompressConfig);
impl_agent_role_config_field!(MemoryRecoverConfig);
impl_agent_role_config_field!(ChannelBatchConfig);
impl_agent_role_config_field!(OutChannelConfig);
impl_agent_role_config_field!(GroupSetConfig);
impl_agent_role_config_field!(PipelineConfig);

impl AgentRoleConfig {
    pub fn build<C: MergeSelf>(&self, role_name: &str) -> C
    where
        Self: AgentRoleConfigField<C>,
    {
        <Self as AgentRoleConfigField<C>>::build(self, role_name)
    }
    pub fn set<C: MergeSelf>(&mut self, role_name: ArcStr, config: Arc<C>)
    where
        Self: AgentRoleConfigField<C>,
    {
        <Self as AgentRoleConfigField<C>>::set(self, role_name, config)
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
    pipeline: Option<Arc<PipelineConfig>>,
}

impl_option_arc_field!(EffectiveLLMConfig => llm, SessionConfigMap);
impl_option_arc_field!(EffectiveCompressConfig => compress, SessionConfigMap);
impl_option_arc_field!(EffectiveMemoryRecoverConfig => memory_recover, SessionConfigMap);
impl_option_arc_field!(ChannelBatchConfig => channel_batch, SessionConfigMap);
impl_option_arc_field!(OutChannelConfig => out_channel, SessionConfigMap);
impl_option_arc_field!(GroupSetConfig => group_set, SessionConfigMap);
impl_option_arc_field!(PipelineConfig => pipeline, SessionConfigMap);

impl_option_arc_field_map!(SessionConfigMap);

pub trait AgentRoleSessionConfigMapField<C: MergeSelf + MergeEffectiveConfig<E>, E> {
    fn create(agents: &HashMap<ArcStr, Arc<AgentRoleConfig>>, memory_key: &MemoryKey) -> E;
}

macro_rules! impl_agent_role_session_config_map_field {
    ($ct:ty, $et:ty) => {
        impl AgentRoleSessionConfigMapField<$ct, $et> for SessionConfigMap {
            fn create(agents: &HashMap<ArcStr, Arc<AgentRoleConfig>>, memory_key: &MemoryKey) -> $et {
                let merge_config = if let Some(agent_role_config) = agents.get(memory_key.agent_id.as_str()) {
                    agent_role_config.build::<$ct>(memory_key.role_event.role_name.as_str())
                } else {
                    <$ct>::default()
                };
                merge_config.get_effective_config()
            }
        }
    }
}

impl_agent_role_session_config_map_field!(LLMConfig, EffectiveLLMConfig);
impl_agent_role_session_config_map_field!(CompressConfig, EffectiveCompressConfig);
impl_agent_role_session_config_map_field!(MemoryRecoverConfig, EffectiveMemoryRecoverConfig);
impl_agent_role_session_config_map_field!(ChannelBatchConfig, ChannelBatchConfig);
impl_agent_role_session_config_map_field!(OutChannelConfig, OutChannelConfig);
impl_agent_role_session_config_map_field!(GroupSetConfig, GroupSetConfig);
impl_agent_role_session_config_map_field!(PipelineConfig, PipelineConfig);

impl SessionConfigMap {
    pub fn create<C, E>(agents: &HashMap<ArcStr, Arc<AgentRoleConfig>>, memory_key: &MemoryKey) -> E
    where
        C: MergeSelf + MergeEffectiveConfig<E>,
        Self: AgentRoleSessionConfigMapField<C, E>,
    {
        <Self as AgentRoleSessionConfigMapField<C, E>>::create(agents, memory_key)
    }
}

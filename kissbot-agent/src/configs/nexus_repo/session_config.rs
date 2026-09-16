
// ========== Context 配置（agent→role 三层继承） ==========

use std::{collections::HashMap, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::configs::{nexus_repo::pipeline_config::*, common::{MergeSelf, MergeBy, OptionArcField}};
use crate::{impl_option_arc_field, impl_option_arc_field_map};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AgentRoleConfigMap {
    llm: Option<Arc<LLMConfig>>,
    compress: Option<Arc<CompressConfig>>,
    memory_recover: Option<Arc<MemoryRecoverConfig>>,
    channel_batch: Option<Arc<ChannelBatchConfig>>,
    out_channel: Option<Arc<OutChannelConfig>>,
    toolkit_set: Option<Arc<ToolkitSetConfig>>,
}

impl_option_arc_field!(LLMConfig => llm, AgentRoleConfigMap);
impl_option_arc_field!(CompressConfig => compress, AgentRoleConfigMap);
impl_option_arc_field!(MemoryRecoverConfig => memory_recover, AgentRoleConfigMap);
impl_option_arc_field!(ChannelBatchConfig => channel_batch, AgentRoleConfigMap);
impl_option_arc_field!(OutChannelConfig => out_channel, AgentRoleConfigMap);
impl_option_arc_field!(ToolkitSetConfig => toolkit_set, AgentRoleConfigMap);

impl_option_arc_field_map!(AgentRoleConfigMap);

trait AgentRoleConfigMapField<C: MergeSelf> {
    fn set(&mut self, config: Arc<C>);
}

macro_rules! impl_agent_role_config_map_field {
    ($ty:ty) => {
        impl AgentRoleConfigMapField<$ty> for AgentRoleConfigMap {
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

impl_agent_role_config_map_field!(LLMConfig);
impl_agent_role_config_map_field!(CompressConfig);
impl_agent_role_config_map_field!(MemoryRecoverConfig);
impl_agent_role_config_map_field!(ChannelBatchConfig);
impl_agent_role_config_map_field!(OutChannelConfig);
impl_agent_role_config_map_field!(ToolkitSetConfig);

impl AgentRoleConfigMap {
    pub fn set<C: MergeSelf>(&mut self, config: Arc<C>)
    where
        Self: AgentRoleConfigMapField<C>,
    {
        <Self as AgentRoleConfigMapField<C>>::set(self, config)
    }
}

/// agent 级 context 配置（key = agent_id，覆盖全局默认；未配字段回落全局默认常量）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentRoleConfig {
    config_map: Arc<AgentRoleConfigMap>,
    /// key = role_name（role 覆盖 agent 默认）
    roles: Arc<HashMap<String, Arc<AgentRoleConfigMap>>>,
}


trait AgentRoleConfigField<C: MergeSelf> {
    fn merge(&self, role_name: &str) -> C;
    fn set(&mut self, role_name: &str, config: Arc<C>);
}

macro_rules! impl_agent_role_config_field {
    ($ty:ty) => {
        impl AgentRoleConfigField<$ty> for AgentRoleConfig {
            fn merge(&self, role_name: &str) -> $ty {
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
            fn set(&mut self, role_name: &str, config: Arc<$ty>) {
                if role_name.is_empty() {
                    // 设置 agent 级配置
                    let config_map = Arc::make_mut(&mut self.config_map);
                    config_map.set::<$ty>(config);
                }
                else {
                    let role_map = Arc::make_mut(&mut self.roles);
                    // role 条目不存在则新建
                    let role_config_map = if let Some(mut role_config_map) = role_map.remove(role_name) {
                        let role_config_map_mut = Arc::make_mut(&mut role_config_map);
                        role_config_map_mut.set::<$ty>(config);
                        role_config_map
                    } else {
                        let mut role_config_map = AgentRoleConfigMap::default();
                        role_config_map.insert::<$ty>(config);
                        Arc::new(role_config_map)
                    };
                    role_map.insert(role_name.to_string(), role_config_map);
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
impl_agent_role_config_field!(ToolkitSetConfig);

impl AgentRoleConfig {
    pub fn merge<C: MergeSelf>(&self, role_name: &str) -> C
    where
        Self: AgentRoleConfigField<C>,
    {
        <Self as AgentRoleConfigField<C>>::merge(self, role_name)
    }
    pub fn set<C: MergeSelf>(&mut self, role_name: &str, config: Arc<C>)
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
    toolkit_set: Option<Arc<ToolkitSetConfig>>,
}

impl_option_arc_field!(EffectiveLLMConfig => llm, SessionConfigMap);
impl_option_arc_field!(EffectiveCompressConfig => compress, SessionConfigMap);
impl_option_arc_field!(EffectiveMemoryRecoverConfig => memory_recover, SessionConfigMap);
impl_option_arc_field!(ChannelBatchConfig => channel_batch, SessionConfigMap);
impl_option_arc_field!(OutChannelConfig => out_channel, SessionConfigMap);
impl_option_arc_field!(ToolkitSetConfig => toolkit_set, SessionConfigMap);

impl_option_arc_field_map!(SessionConfigMap);

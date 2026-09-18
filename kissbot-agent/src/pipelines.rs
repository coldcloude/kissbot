pub mod input_processor;
pub mod output_processor;
pub mod message_sender;
pub mod tool_caller;
pub mod system_prompter;

use arc_swap::ArcSwap;
pub use input_processor::*;
use dashmap::DashMap;
use kissbot_api::IncomingMessageEvent;
pub use output_processor::*;
pub use message_sender::*;
pub use tool_caller::*;
pub use system_prompter::*;

use std::sync::Arc;

use crate::{config_manager::ConfigManager, configs::{PipelineConfig, ToolConfig}, pipeline::{AgentInputProcessor, AgentMessageSender, AgentOutputProcessor, AgentPipeline, AgentSystemPrompter, AgentToolCaller, AgentTrgger}, types::{Error, Message, Mode, Result, SessionKey}};

pub const PP_IN_BATCH: &str = "batch_input_processor";
pub const PP_OUT_CHANNEL: &str = "channel_output_processor";
pub const PP_MSG_RAW: &str = "raw_message_sender";
pub const PP_MSG_COMPRESS: &str = "compress_message_sender";
pub const PP_MSG_MEMORY_RECOVER: &str = "memory_recover_message_sender";
pub const PP_TOOL_STATION: &str = "station_tool_caller";
pub const PP_SYS_DEFAULT: &str = "default_system_prompter";
pub const PP_SYS_MEMORY_EGO: &str = "memory_ego_system_prompter";

pub const PP_PRESET_ROLE: &str = "preset_role";
pub const PP_PRESET_EVENT: &str = "preset_event";

pub async fn create_input_processor(session_key: Arc<SessionKey>, name: &str) -> Option<Box<dyn AgentInputProcessor>> {
    match name {
        PP_IN_BATCH => Some(Box::new(BatchAgentInputProcessor::new(session_key))),
        _ => None,
    }
}

pub fn create_system_prompter(session_key: Arc<SessionKey>, name: &str) -> Option<Box<dyn AgentSystemPrompter>> {
    match name {
        PP_SYS_DEFAULT => Some(Box::new(DefaultSystemPrompter::new(session_key))),
        PP_SYS_MEMORY_EGO => Some(Box::new(MemoryEgoSystemPrompter::new(session_key))),
        _ => None,
    }
}

pub fn create_message_sender(session_key: Arc<SessionKey>, name: &str) -> Option<Box<dyn AgentMessageSender>> {
    match name {
        PP_MSG_RAW => Some(Box::new(RawMessagerSender { session_key })),
        PP_MSG_COMPRESS => Some(Box::new(TokenLimitMessageSender::new_compress(session_key))),
        PP_MSG_MEMORY_RECOVER => Some(Box::new(TokenLimitMessageSender::new_memory_recover(session_key))),
        _ => None,
    }
}
pub fn create_tool_caller(name: &str) -> Option<Box<dyn AgentToolCaller>> {
    match name {
        PP_TOOL_STATION => Some(Box::new(StationToolCaller)),
        _ => None,
    }
}

pub fn create_output_processor(session_key: Arc<SessionKey>, name: &str) -> Option<Box<dyn AgentOutputProcessor>> {
    match name {
        PP_OUT_CHANNEL => Some(Box::new(ChannelOutputProcessor::new(session_key))),
        _ => None,
    }
}

pub async fn create_agent_pipeline(session_key: Arc<SessionKey>) -> Option<AgentPipeline> {
    let config = ConfigManager::get().session_config::<PipelineConfig,PipelineConfig>(session_key.as_ref()).await;
    let mut message_sender_cfg: Option<&str> = None;
    let mut tool_caller_cfg: Option<&str> = None;
    let mut output_processor_cfg: Option<&str> = None;
    if let Some(preset) = config.preset.as_ref() {
        let preset = if preset.as_str().is_empty() {
            match session_key.mode {
                Mode::Role => PP_PRESET_ROLE,
                Mode::Event(_) => PP_PRESET_EVENT,
            }
        } else {
            preset.as_str()
        };
        match preset {
            PP_PRESET_ROLE => {
                message_sender_cfg = Some(PP_MSG_MEMORY_RECOVER);
                tool_caller_cfg = Some(PP_TOOL_STATION);
                output_processor_cfg = Some(PP_OUT_CHANNEL);
            },
            PP_PRESET_EVENT => {
                message_sender_cfg = Some(PP_MSG_COMPRESS);
                tool_caller_cfg = Some(PP_TOOL_STATION);
                output_processor_cfg = Some(PP_OUT_CHANNEL);
            },
            _ => {},
        };
    }
    if let Some(cfg) = config.message_sender.as_ref() {
        message_sender_cfg = Some(cfg.as_str());
    }
    if let Some(cfg) = config.tool_caller.as_ref() {
        tool_caller_cfg = Some(cfg.as_str());
    }
    if let Some(cfg) = config.output_processor.as_ref() {
        output_processor_cfg = Some(cfg.as_str());
    }
    let mut message_sender: Option<Box<dyn AgentMessageSender>> = None;
    let mut tool_caller: Option<Box<dyn AgentToolCaller>> = None;
    let mut output_processor: Option<Box<dyn AgentOutputProcessor>> = None;
    if let Some(name) = message_sender_cfg {
        message_sender = create_message_sender(session_key.clone(), name);
    }
    if let Some(name) = tool_caller_cfg {
        tool_caller = create_tool_caller(name);
    }
    if let Some(name) = output_processor_cfg {
        output_processor = create_output_processor(session_key.clone(), name);
    }
    let Some(message_sender) = message_sender else { return None; };
    let Some(tool_caller) = tool_caller else { return None; };
    let Some(output_processor) = output_processor else { return None; };
    Some(AgentPipeline {
        session_key,
        message_sender: ArcSwap::from_pointee(message_sender),
        tool_caller: ArcSwap::from_pointee(tool_caller),
        output_processor: ArcSwap::from_pointee(output_processor),
    })
}

pub async fn create_agent_trigger(session_key: Arc<SessionKey>) -> Option<AgentTrgger> {
    let config = ConfigManager::get().session_config::<PipelineConfig,PipelineConfig>(session_key.as_ref()).await;
    let mut input_processor_cfg: Option<&str> = None;
    let mut system_prompter_cfg: Option<&str> = None;
    if let Some(preset) = config.preset.as_ref() {
        let preset = if preset.as_str().is_empty() {
            match session_key.mode {
                Mode::Role => PP_PRESET_ROLE,
                Mode::Event(_) => PP_PRESET_EVENT,
            }
        } else {
            preset.as_str()
        };
        match preset {
            PP_PRESET_ROLE => {
                input_processor_cfg = Some(PP_IN_BATCH);
                system_prompter_cfg = Some(PP_SYS_MEMORY_EGO);
            },
            PP_PRESET_EVENT => {
                input_processor_cfg = Some(PP_IN_BATCH);
                system_prompter_cfg = Some(PP_SYS_MEMORY_EGO);
            },
            _ => {},
        };
    }
    if let Some(cfg) = config.input_processor.as_ref() {
        input_processor_cfg = Some(cfg.as_str());
    }
    if let Some(cfg) = config.system_prompter.as_ref() {
        system_prompter_cfg = Some(cfg.as_str());
    }
    let mut input_processor: Option<Box<dyn AgentInputProcessor>> = None;
    let mut system_prompter: Option<Box<dyn AgentSystemPrompter>> = None;
    if let Some(name) = input_processor_cfg {
        input_processor = create_input_processor(session_key.clone(), name).await;
    }
    if let Some(name) = system_prompter_cfg {
        system_prompter = create_system_prompter(session_key.clone(), name);

    }
    let Some(input_processor) = input_processor else { return None; };
    let Some(system_prompter) = system_prompter else { return None; };
    Some(AgentTrgger {
        session_key,
        input_processor: ArcSwap::from_pointee(input_processor),
        system_prompter: ArcSwap::from_pointee(system_prompter),
    })
}

/// 会话 pipeline 注册表：trigger（输入处理 + 系统提示词）与 pipeline（发送/工具/输出）按会话 key 各存一份
/// 用 DashMap 而非 ArcSwap<HashMap>：并发 sync 同一 key 时按 key 原子替换，
/// 不会出现「读-改-写整张 map」丢失其它 key 刚插入条目的问题
pub struct PipelineManager {
    trigger_map: DashMap<SessionKey, Arc<AgentTrgger>>,
    pipeline_map: DashMap<SessionKey, Arc<AgentPipeline>>,
}

impl PipelineManager {
    pub fn new() -> Self {
        Self {
            trigger_map: DashMap::new(),
            pipeline_map: DashMap::new(),
        }
    }

    pub async fn sync_pipeline(&self, session_key: Arc<SessionKey>) -> Result<()> {
        let Some(trigger) = create_agent_trigger(session_key.clone()).await else {
            return Err(Error::PipelineNotFound(session_key.as_ref().clone()));
        };
        let Some(pipeline) = create_agent_pipeline(session_key.clone()).await else {
            return Err(Error::PipelineNotFound(session_key.as_ref().clone()));
        };
        // 按 key 原子替换（同 key 并发 sync 时后者生效；不同 key 互不影响）
        self.trigger_map.insert(session_key.as_ref().clone(), Arc::new(trigger));
        self.pipeline_map.insert(session_key.as_ref().clone(), Arc::new(pipeline));
        Ok(())
    }

    /// 取 trigger 快照（Arc 克隆后即释放 DashMap 分片读锁，不跨 await 持锁——被调方可能回写本表）
    fn trigger_of(&self, session_key: &SessionKey) -> Option<Arc<AgentTrgger>> {
        self.trigger_map.get(session_key).map(|t| t.value().clone())
    }

    /// 取 pipeline 快照（同上：不跨 await 持锁）
    fn pipeline_of(&self, session_key: &SessionKey) -> Option<Arc<AgentPipeline>> {
        self.pipeline_map.get(session_key).map(|p| p.value().clone())
    }

    pub async fn incoming_message(&self, session_key: &SessionKey, event: Arc<IncomingMessageEvent>) -> Result<()> {
        if let Some(trigger) = self.trigger_of(session_key) {
            trigger.input_processor.load().accept(event).await;
            Ok(())
        } else {
            Err(Error::PipelineNotFound(session_key.clone()))
        }
    }

    pub async fn reset_system_prompt(&self, session_key: &SessionKey) -> Result<()> {
        if let Some(trigger) = self.trigger_of(session_key) {
            trigger.system_prompter.load().reset_system_prompt().await;
            Ok(())
        } else {
            Err(Error::PipelineNotFound(session_key.clone()))
        }
    }

    pub async fn run_pipeline(&self, session_key: &SessionKey, message: Message, tools: &Vec<Arc<ToolConfig>>) -> Result<()> {
        if let Some(pipeline) = self.pipeline_of(session_key) {
            pipeline.run(message, tools).await;
            Ok(())
        } else {
            Err(Error::PipelineNotFound(session_key.clone()))
        }
    }
}
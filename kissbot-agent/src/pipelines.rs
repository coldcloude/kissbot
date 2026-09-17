pub mod input_processor;
pub mod output_processor;
pub mod message_sender;
pub mod tool_caller;
pub mod system_prompter;

pub use input_processor::*;
pub use output_processor::*;
pub use message_sender::*;
pub use tool_caller::*;
pub use system_prompter::*;

use std::sync::Arc;

use crate::{config_manager::ConfigManager, configs::PipelineConfig, pipeline::{AgentInputProcessor, AgentMessageSender, AgentOutputProcessor, AgentPipeline, AgentSystemPrompter, AgentToolCaller, AgentTrgger}, types::{Mode, SessionKey}};

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

struct ProcessorSet<'l> {
    input_processor: &'l str,
    system_prompter: &'l str,
    message_sender: &'l str,
    tool_caller: &'l str,
    output_processor: &'l str,
}

pub struct AgentPipelineFactory;

pub async fn create_input_processor(session_key: Arc<SessionKey>, name: &str) -> Option<Arc<dyn AgentInputProcessor>> {
    match name {
        PP_IN_BATCH => Some(Arc::new(BatchAgentInputProcessor::new(session_key).await)),
        _ => None,
    }
}

pub fn create_system_prompter(session_key: Arc<SessionKey>, name: &str) -> Option<Arc<dyn AgentSystemPrompter>> {
    match name {
        PP_SYS_DEFAULT => Some(Arc::new(DefaultSystemPrompter::new(session_key))),
        PP_SYS_MEMORY_EGO => Some(Arc::new(MemoryEgoSystemPrompter::new(session_key))),
        _ => None,
    }
}

pub fn create_message_sender(session_key: Arc<SessionKey>, name: &str) -> Option<Arc<dyn AgentMessageSender>> {
    match name {
        PP_MSG_RAW => Some(Arc::new(RawMessagerSender { session_key })),
        PP_MSG_COMPRESS => Some(Arc::new(TokenLimitMessageSender::new_compress(session_key))),
        PP_MSG_MEMORY_RECOVER => Some(Arc::new(TokenLimitMessageSender::new_memory_recover(session_key))),
        _ => None,
    }
}
pub fn create_tool_caller(name: &str) -> Option<Arc<dyn AgentToolCaller>> {
    match name {
        PP_TOOL_STATION => Some(Arc::new(StationToolCaller)),
        _ => None,
    }
}

pub fn create_output_processor(session_key: Arc<SessionKey>, name: &str) -> Option<Arc<dyn AgentOutputProcessor>> {
    match name {
        PP_OUT_CHANNEL => Some(Arc::new(ChannelOutputProcessor::new(session_key))),
        _ => None,
    }
}

pub async fn create_agent_pipeline(session_key: Arc<SessionKey>) -> Option<Arc<AgentPipeline>> {
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
    let mut message_sender: Option<Arc<dyn AgentMessageSender>> = None;
    let mut tool_caller: Option<Arc<dyn AgentToolCaller>> = None;
    let mut output_processor: Option<Arc<dyn AgentOutputProcessor>> = None;
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
    Some(Arc::new(AgentPipeline {
        session_key,
        message_sender,
        tool_caller,
        output_processor,
    }))
}

pub async fn create_agent_trigger(session_key: Arc<SessionKey>) -> Option<Arc<AgentTrgger>> {
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
    let mut input_processor: Option<Arc<dyn AgentInputProcessor>> = None;
    let mut system_prompter: Option<Arc<dyn AgentSystemPrompter>> = None;
    if let Some(name) = input_processor_cfg {
        input_processor = create_input_processor(session_key.clone(), name).await;
    }
    if let Some(name) = system_prompter_cfg {
        system_prompter = create_system_prompter(session_key.clone(), name);

    }
    let Some(input_processor) = input_processor else { return None; };
    let Some(system_prompter) = system_prompter else { return None; };
    Some(Arc::new(AgentTrgger {
        session_key,
        input_processor,
        system_prompter,
    }))
}

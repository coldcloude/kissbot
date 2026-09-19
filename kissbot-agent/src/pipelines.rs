pub mod input_processor;
pub mod output_processor;
pub mod message_sender;
pub mod tool_caller;
pub mod system_prompter;

use futures_util::future;
pub use input_processor::*;
use dashmap::DashMap;
use kissbot_api::IncomingMessageEvent;
pub use output_processor::*;
pub use message_sender::*;
use tokio::sync::RwLock;
pub use tool_caller::*;
pub use system_prompter::*;

use std::collections::HashSet;
use std::sync::Arc;

use crate::{config_manager::ConfigManager, configs::{PipelineConfig, ToolConfig}, pipeline::{AgentInputProcessor, AgentMessageSender, AgentOutputProcessor, AgentPipeline, AgentSystemPrompter, AgentToolCaller, AgentTrigger}, types::{Error, Message, Mode, Result, SessionKey}};

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

pub fn create_input_processor(session_key: Arc<SessionKey>, name: &str) -> Result<Arc<dyn AgentInputProcessor>> {
    match name {
        PP_IN_BATCH => Ok(Arc::new(BatchAgentInputProcessor::new(session_key))),
        _ => Err(Error::PipelineNotFound("input processor".to_string(), name.to_string(), session_key.as_ref().clone())),
    }
}

pub fn create_system_prompter(session_key: Arc<SessionKey>, name: &str) -> Result<Arc<dyn AgentSystemPrompter>> {
    match name {
        PP_SYS_DEFAULT => Ok(Arc::new(DefaultSystemPrompter::new(session_key))),
        PP_SYS_MEMORY_EGO => Ok(Arc::new(MemoryEgoSystemPrompter::new(session_key))),
        _ => Err(Error::PipelineNotFound("system_prompter".to_string(), name.to_string(), session_key.as_ref().clone())),
    }
}

pub fn create_message_sender(session_key: Arc<SessionKey>, name: &str) -> Result<Arc<dyn AgentMessageSender>> {
    match name {
        PP_MSG_RAW => Ok(Arc::new(RawMessagerSender { session_key })),
        PP_MSG_COMPRESS => Ok(Arc::new(TokenLimitMessageSender::new_compress(session_key))),
        PP_MSG_MEMORY_RECOVER => Ok(Arc::new(TokenLimitMessageSender::new_memory_recover(session_key))),
        _ => Err(Error::PipelineNotFound("message sender".to_string(), name.to_string(), session_key.as_ref().clone())),
    }
}
pub fn create_tool_caller(session_key: Arc<SessionKey>, name: &str) -> Result<Arc<dyn AgentToolCaller>> {
    match name {
        PP_TOOL_STATION => Ok(Arc::new(StationToolCaller)),
        _ => Err(Error::PipelineNotFound("tool caller".to_string(), name.to_string(), session_key.as_ref().clone())),
    }
}

pub fn create_output_processor(session_key: Arc<SessionKey>, name: &str) -> Result<Arc<dyn AgentOutputProcessor>> {
    match name {
        PP_OUT_CHANNEL => Ok(Arc::new(ChannelOutputProcessor::new(session_key))),
        _ => Err(Error::PipelineNotFound("output processor".to_string(), name.to_string(), session_key.as_ref().clone())),
    }
}

pub struct SessionPipeline {
    input_processors: Vec<Arc<dyn AgentInputProcessor>>,
    system_prompters: Vec<Arc<dyn AgentSystemPrompter>>,
    triggers: Vec<Arc<dyn AgentTrigger>>,
    pipeline: AgentPipeline,
}

pub async fn create_agent_pipeline(session_key: Arc<SessionKey>) -> Result<AgentPipeline> {
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
    let message_sender: Arc<dyn AgentMessageSender> = if let Some(name) = message_sender_cfg {
        create_message_sender(session_key.clone(), name)?
    } else {
        return Err(Error::PipelineNotFound("message sender".to_string(), "NULL".to_string(), session_key.as_ref().clone()))
    };
    let tool_caller: Arc<dyn AgentToolCaller> = if let Some(name) = tool_caller_cfg {
        create_tool_caller(session_key.clone(), name)?
    } else {
        return Err(Error::PipelineNotFound("ttool caller".to_string(), "NULL".to_string(), session_key.as_ref().clone()))
    };
    let output_processor: Arc<dyn AgentOutputProcessor> = if let Some(name) = output_processor_cfg {
        create_output_processor(session_key.clone(), name)?
    } else {
        return Err(Error::PipelineNotFound("out channel".to_string(), "NULL".to_string(), session_key.as_ref().clone()))
    };
    Ok(AgentPipeline {
        session_key,
        message_sender: message_sender,
        tool_caller: tool_caller,
        output_processor: output_processor,
    })
}

pub async fn create_agent_input_processors(session_key: Arc<SessionKey>) -> Result<Vec<Arc<dyn AgentInputProcessor>>> {
    let config = ConfigManager::get().session_config::<PipelineConfig,PipelineConfig>(session_key.as_ref()).await;
    let mut names = Vec::new();
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
                names.push(Arc::new(PP_IN_BATCH.to_string()));
            },
            PP_PRESET_EVENT => {
                names.push(Arc::new(PP_IN_BATCH.to_string()));
            },
            _ => {},
        };
    }
    if let Some(cfg) = config.input_processor.as_ref() {
        names = cfg.clone();
    }
    let mut input_processors = Vec::with_capacity(names.len());
    for name in names {
        let input_processor = create_input_processor(session_key.clone(), name.as_str())?;
        input_processors.push(input_processor);
    }
    Ok(input_processors)
}

pub async fn create_agent_system_prompters(session_key: Arc<SessionKey>) -> Result<Vec<Arc<dyn AgentSystemPrompter>>> {
    let config = ConfigManager::get().session_config::<PipelineConfig,PipelineConfig>(session_key.as_ref()).await;
    let mut names = Vec::new();
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
                names.push(Arc::new(PP_IN_BATCH.to_string()));
            },
            PP_PRESET_EVENT => {
                names.push(Arc::new(PP_IN_BATCH.to_string()));
            },
            _ => {},
        };
    }
    if let Some(cfg) = config.system_prompter.as_ref() {
        names = cfg.clone();
    }
    let mut system_prompters = Vec::with_capacity(names.len());
    for name in names {
        let system_prompter = create_system_prompter(session_key.clone(), name.as_str())?;
        system_prompters.push(system_prompter);
    }
    Ok(system_prompters)
}

/// 会话 pipeline 注册表：trigger（输入处理 + 系统提示词）与 pipeline（发送/工具/输出）按会话 key 各存一份
/// 用 DashMap 而非 ArcSwap<HashMap>：并发 sync 同一 key 时按 key 原子替换，
/// 不会出现「读-改-写整张 map」丢失其它 key 刚插入条目的问题
pub struct PipelineManager {
    pipeline_map: DashMap<SessionKey, Arc<RwLock<SessionPipeline>>>,
}

impl PipelineManager {
    pub fn new() -> Self {
        Self {
            pipeline_map: DashMap::new(),
        }
    }

    pub async fn sync_pipeline(&self, session_key: Arc<SessionKey>) -> Result<()> {
        let input_processors = create_agent_input_processors(session_key.clone()).await?;
        let system_prompters = create_agent_system_prompters(session_key.clone()).await?;
        let pipeline = create_agent_pipeline(session_key.clone()).await?;
        let session_pipeline = SessionPipeline {
            input_processors,
            system_prompters,
            triggers: Vec::new(),
            pipeline,
        };
        self.pipeline_map.insert(session_key.as_ref().clone(), Arc::new(RwLock::new(session_pipeline)));
        Ok(())
    }

    /// 只保留仍在绑定集合中的会话的 trigger/pipeline（与 SessionManager::retain 成对调用，
    /// 会话被清理后其 trigger/pipeline 一并丢弃；trigger 丢弃会触发输入处理器的 Drop → 合批任务退出）
    pub fn retain(&self, keys: &HashSet<SessionKey>) {
        self.pipeline_map.retain(|k, _| keys.contains(k));
    }

    pub async fn incoming_message(&self, session_key: &SessionKey, event: Arc<IncomingMessageEvent>) -> Result<()> {
        if let Some(pipeline) = self.pipeline_map.get(session_key) {
            let input_processors = {
                let gaurd = pipeline.read().await;
                gaurd.input_processors.clone()
            };
            let mut futs = Vec::new();
            for input_processor in input_processors.iter() {
                let f = input_processor.accept(event.clone());
                futs.push(f);
            }
            future::join_all(futs).await;
            Ok(())
        } else {
            Err(Error::PipelineNotFound("any pipeline".to_string(), "CURRENT".to_string(), session_key.clone()))
        }
    }

    pub async fn reset_system_prompt(&self, session_key: &SessionKey) -> Result<()> {
        if let Some(pipeline) = self.pipeline_map.get(session_key) {
            let system_prompters = {
                let gaurd = pipeline.read().await;
                gaurd.system_prompters.clone()
            };
            let mut futs = Vec::new();
            for system_prompter in system_prompters.iter() {
                let f = system_prompter.reset_system_prompt();
                futs.push(f);
            }
            future::join_all(futs).await;
            Ok(())
        } else {
            Err(Error::PipelineNotFound("any pipeline".to_string(), "CURRENT".to_string(), session_key.clone()))
        }
    }

    pub async fn run_pipeline(&self, session_key: &SessionKey, message: Message, tools: &Vec<Arc<ToolConfig>>) -> Result<()> {
        if let Some(pipeline) = self.pipeline_map.get(session_key) {
            let gaurd = pipeline.read().await;
            gaurd.pipeline.run(message, tools).await;
            Ok(())
        } else {
            Err(Error::PipelineNotFound("any pipeline".to_string(), "CURRENT".to_string(), session_key.clone()))
        }
    }
}
pub mod abstract_input_processor;
pub mod input_processor;
pub mod output_processor;
pub mod message_sender;
pub mod tool_caller;
pub mod system_prompter;

use futures_util::future;
pub use abstract_input_processor::*;
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

use arcstr::ArcStr;
use kissbot_llm::{Message, Session, ToolConfig};

use crate::{config_manager::ConfigManager, configs::PipelineConfig, pipeline::{AgentInputProcessor, AgentMessageSender, AgentOutputProcessor, AgentPipeline, AgentSystemPrompter, AgentToolCaller, AgentTrigger}, types::{Error, Result, SessionKey}};

pub const PP_IN_BATCH: ArcStr = arcstr::literal!("batch_input_processor");
pub const PP_IN_ABSTRACT: ArcStr = arcstr::literal!("abstract_input_processor");
pub const PP_OUT_CHANNEL: ArcStr = arcstr::literal!("channel_output_processor");
pub const PP_MSG_RAW: ArcStr = arcstr::literal!("raw_message_sender");
pub const PP_MSG_COMPRESS: ArcStr = arcstr::literal!("compress_message_sender");
pub const PP_MSG_MEMORY_RECOVER: ArcStr = arcstr::literal!("memory_recover_message_sender");
pub const PP_TOOL_STATION: ArcStr = arcstr::literal!("station_tool_caller");
pub const PP_SYS_DEFAULT: ArcStr = arcstr::literal!("default_system_prompter");
pub const PP_SYS_MEMORY_EGO: ArcStr = arcstr::literal!("memory_ego_system_prompter");

pub const PP_PRESET_ROLE: ArcStr = arcstr::literal!("preset_role");
pub const PP_PRESET_EVENT: ArcStr = arcstr::literal!("preset_event");

pub fn create_input_processor(session_key: Arc<SessionKey>, session: Arc<Session>, name: &str) -> Result<Arc<dyn AgentInputProcessor>> {
    match name {
        n if n == PP_IN_BATCH => Ok(Arc::new(BatchAgentInputProcessor::new(session_key))),
        n if n == PP_IN_ABSTRACT => Ok(Arc::new(AbstractAgentInputProcessor::new(session_key, session))),
        _ => Err(Error::PipelineNotFound(ArcStr::from("input processor"), ArcStr::from(name), session_key.as_ref().clone())),
    }
}

pub fn create_system_prompter(session_key: Arc<SessionKey>, session: Arc<Session>, name: &str) -> Result<Arc<dyn AgentSystemPrompter>> {
    match name {
        n if n == PP_SYS_DEFAULT => Ok(Arc::new(DefaultSystemPrompter::new(session))),
        n if n == PP_SYS_MEMORY_EGO => Ok(Arc::new(MemoryEgoSystemPrompter::new(session_key, session))),
        _ => Err(Error::PipelineNotFound(ArcStr::from("system_prompter"), ArcStr::from(name), session_key.as_ref().clone())),
    }
}

pub fn create_message_sender(session_key: Arc<SessionKey>, session: Arc<Session>, name: &str) -> Result<Arc<dyn AgentMessageSender>> {
    match name {
        n if n == PP_MSG_RAW => Ok(Arc::new(RawMessagerSender { session_key, session })),
        n if n == PP_MSG_COMPRESS => Ok(Arc::new(TokenLimitMessageSender::new_compress(session_key, session))),
        n if n == PP_MSG_MEMORY_RECOVER => Ok(Arc::new(TokenLimitMessageSender::new_memory_recover(session_key, session))),
        _ => Err(Error::PipelineNotFound(ArcStr::from("message sender"), ArcStr::from(name), session_key.as_ref().clone())),
    }
}
pub fn create_tool_caller(session_key: Arc<SessionKey>, name: &str) -> Result<Arc<dyn AgentToolCaller>> {
    match name {
        n if n == PP_TOOL_STATION => Ok(Arc::new(StationToolCaller)),
        _ => Err(Error::PipelineNotFound(ArcStr::from("tool caller"), ArcStr::from(name), session_key.as_ref().clone())),
    }
}

pub fn create_output_processor(session_key: Arc<SessionKey>, name: &str) -> Result<Arc<dyn AgentOutputProcessor>> {
    match name {
        n if n == PP_OUT_CHANNEL => Ok(Arc::new(ChannelOutputProcessor::new(session_key))),
        _ => Err(Error::PipelineNotFound(ArcStr::from("output processor"), ArcStr::from(name), session_key.as_ref().clone())),
    }
}

pub struct SessionPipeline {
    pub input_processors: DashMap<ArcStr, Arc<dyn AgentInputProcessor>>,
    pub system_prompters: DashMap<ArcStr, Arc<dyn AgentSystemPrompter>>,
    pub triggers: DashMap<ArcStr, Arc<dyn AgentTrigger>>,
    pub pipeline: RwLock<AgentPipeline>,
}

pub async fn create_agent_pipeline(session_key: Arc<SessionKey>, session: Arc<Session>) -> Result<AgentPipeline> {
    let config = ConfigManager::get().session_config::<PipelineConfig,PipelineConfig>(session_key.as_ref()).await;
    let mut message_sender_cfg: Option<ArcStr> = None;
    let mut tool_caller_cfg: Option<ArcStr> = None;
    let mut output_processor_cfg: Option<ArcStr> = None;
    if let Some(preset) = config.preset.as_ref() {
        let preset: ArcStr = if preset.as_str().is_empty() {
            if session_key.role_event.event_id.is_some() { PP_PRESET_EVENT.clone() } else { PP_PRESET_ROLE.clone() }
        } else {
            preset.clone()
        };
        match preset {
            n if n == PP_PRESET_ROLE => {
                message_sender_cfg = Some(PP_MSG_MEMORY_RECOVER.clone());
                tool_caller_cfg = Some(PP_TOOL_STATION.clone());
                output_processor_cfg = Some(PP_OUT_CHANNEL.clone());
            },
            n if n == PP_PRESET_EVENT => {
                message_sender_cfg = Some(PP_MSG_COMPRESS.clone());
                tool_caller_cfg = Some(PP_TOOL_STATION.clone());
                output_processor_cfg = Some(PP_OUT_CHANNEL.clone());
            },
            _ => {},
        };
    }
    if let Some(cfg) = config.message_sender.as_ref() {
        message_sender_cfg = Some(cfg.clone());
    }
    if let Some(cfg) = config.tool_caller.as_ref() {
        tool_caller_cfg = Some(cfg.clone());
    }
    if let Some(cfg) = config.output_processor.as_ref() {
        output_processor_cfg = Some(cfg.clone());
    }
    let message_sender: Arc<dyn AgentMessageSender> = if let Some(name) = message_sender_cfg {
        create_message_sender(session_key.clone(), session.clone(), name.as_str())?
    } else {
        return Err(Error::PipelineNotFound(ArcStr::from("message sender"), ArcStr::from("NULL"), session_key.as_ref().clone()))
    };
    let tool_caller: Arc<dyn AgentToolCaller> = if let Some(name) = tool_caller_cfg {
        create_tool_caller(session_key.clone(), name.as_str())?
    } else {
        return Err(Error::PipelineNotFound(ArcStr::from("ttool caller"), ArcStr::from("NULL"), session_key.as_ref().clone()))
    };
    let output_processor: Arc<dyn AgentOutputProcessor> = if let Some(name) = output_processor_cfg {
        create_output_processor(session_key.clone(), name.as_str())?
    } else {
        return Err(Error::PipelineNotFound(ArcStr::from("out channel"), ArcStr::from("NULL"), session_key.as_ref().clone()))
    };
    Ok(AgentPipeline {
        session_key,
        session,
        message_sender: message_sender,
        tool_caller: tool_caller,
        output_processor: output_processor,
    })
}

pub async fn create_agent_input_processors(session_key: Arc<SessionKey>, session: Arc<Session>) -> Result<DashMap<ArcStr, Arc<dyn AgentInputProcessor>>> {
    let config = ConfigManager::get().session_config::<PipelineConfig,PipelineConfig>(session_key.as_ref()).await;
    let mut names = Vec::new();
    if let Some(preset) = config.preset.as_ref() {
        let preset: ArcStr = if preset.as_str().is_empty() {
            if session_key.role_event.event_id.is_some() { PP_PRESET_EVENT.clone() } else { PP_PRESET_ROLE.clone() }
        } else {
            preset.clone()
        };
        match preset {
            n if n == PP_PRESET_ROLE => {
                names.push(PP_IN_BATCH.clone());
            },
            n if n == PP_PRESET_EVENT => {
                names.push(PP_IN_BATCH.clone());
            },
            _ => {},
        };
    }
    if let Some(cfg) = config.input_processor.as_ref() {
        names = cfg.clone();
    }
    let input_processors = DashMap::new();
    for name in names.drain(..) {
        let input_processor = create_input_processor(session_key.clone(), session.clone(), name.as_str())?;
        input_processors.insert(name, input_processor);
    }
    Ok(input_processors)
}

pub async fn create_agent_system_prompters(session_key: Arc<SessionKey>, session: Arc<Session>) -> Result<DashMap<ArcStr, Arc<dyn AgentSystemPrompter>>> {
    let config = ConfigManager::get().session_config::<PipelineConfig,PipelineConfig>(session_key.as_ref()).await;
    let mut names = Vec::new();
    if let Some(preset) = config.preset.as_ref() {
        let preset: ArcStr = if preset.as_str().is_empty() {
            if session_key.role_event.event_id.is_some() { PP_PRESET_EVENT.clone() } else { PP_PRESET_ROLE.clone() }
        } else {
            preset.clone()
        };
        match preset {
            n if n == PP_PRESET_ROLE => {
                names.push(PP_SYS_MEMORY_EGO.clone());
            },
            n if n == PP_PRESET_EVENT => {
                names.push(PP_SYS_MEMORY_EGO.clone());
            },
            _ => {},
        };
    }
    if let Some(cfg) = config.system_prompter.as_ref() {
        names = cfg.clone();
    }
    let system_prompters = DashMap::new();
    for name in names.drain(..) {
        let system_prompter = create_system_prompter(session_key.clone(), session.clone(), name.as_str())?;
        system_prompters.insert(name, system_prompter);
    }
    Ok(system_prompters)
}

/// 会话 pipeline 注册表：trigger（输入处理 + 系统提示词）与 pipeline（发送/工具/输出）按会话 key 各存一份
/// 用 DashMap 而非 ArcSwap<HashMap>：并发 sync 同一 key 时按 key 原子替换，
/// 不会出现「读-改-写整张 map」丢失其它 key 刚插入条目的问题
pub struct PipelineManager {
    pipeline_map: DashMap<SessionKey, Arc<SessionPipeline>>,
}

impl PipelineManager {
    pub fn new() -> Self {
        Self {
            pipeline_map: DashMap::new(),
        }
    }

    pub async fn sync_pipeline(&self, session_key: Arc<SessionKey>, session: Arc<Session>) -> Result<()> {
        let input_processors = create_agent_input_processors(session_key.clone(), session.clone()).await?;
        let system_prompters = create_agent_system_prompters(session_key.clone(), session.clone()).await?;
        let pipeline = create_agent_pipeline(session_key.clone(), session.clone()).await?;
        let session_pipeline = SessionPipeline {
            input_processors,
            system_prompters,
            triggers: DashMap::new(),
            pipeline: RwLock::new(pipeline),
        };
        self.pipeline_map.insert(session_key.as_ref().clone(), Arc::new(session_pipeline));
        Ok(())
    }

    /// 只保留仍在绑定集合中的会话的 trigger/pipeline（与 SessionManager::retain 成对调用，
    /// 会话被清理后其 trigger/pipeline 一并丢弃；trigger 丢弃会触发输入处理器的 Drop → 合批任务退出）
    pub fn retain(&self, keys: &HashSet<SessionKey>) {
        self.pipeline_map.retain(|k, _| keys.contains(k));
    }

    pub async fn incoming_message(&self, session_key: &SessionKey, event: Arc<IncomingMessageEvent>) -> Result<()> {
        let input_processors = if let Some(pipeline) = self.pipeline_map.get(session_key) {
            let mut rs = Vec::new();
            for r in pipeline.input_processors.iter() {
                rs.push(r.clone());
            }
            rs
        } else {
            return Err(Error::PipelineNotFound(ArcStr::from("any pipeline"), ArcStr::from("CURRENT"), session_key.clone()))
        };
        let mut futs = Vec::new();
        for input_processor in input_processors.iter() {
            let f = input_processor.accept(event.clone());
            futs.push(f);
        }
        future::join_all(futs).await;
        Ok(())
    }

    pub async fn reset_system_prompt(&self, session_key: &SessionKey) -> Result<()> {
        let system_prompters = if let Some(pipeline) = self.pipeline_map.get(session_key) {
            let mut rs = Vec::new();
            for r in pipeline.system_prompters.iter() {
                rs.push(r.clone());
            }
            rs
        } else {
            return Err(Error::PipelineNotFound(ArcStr::from("any pipeline"), ArcStr::from("CURRENT"), session_key.clone()))
        };
        let mut futs = Vec::new();
        for system_prompter in system_prompters.iter() {
            let f = system_prompter.reset_system_prompt();
            futs.push(f);
        }
        future::join_all(futs).await;
        Ok(())
    }

    pub async fn run_pipeline(&self, session_key: &SessionKey, message: Message, tools: &Vec<Arc<ToolConfig>>) -> Result<()> {
        let pipeline = if let Some(pipeline) = self.pipeline_map.get(session_key) {
            pipeline.clone()
        } else {
            return Err(Error::PipelineNotFound(ArcStr::from("any pipeline"), ArcStr::from("CURRENT"), session_key.clone()))
        };
        let guard = pipeline.pipeline.read().await;
        guard.run(message, tools).await;
        Ok(())
    }
}
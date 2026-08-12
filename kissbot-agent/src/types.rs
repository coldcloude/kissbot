use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::config_manager::ProviderModel;

// ========== 错误类型 ==========

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Config not found: {0}")]
    ConfigNotFound(String),

    #[error("Config parse error: {0}")]
    ConfigParseError(String),

    #[error("Model API error: {0}")]
    ModelApiError(String),

    #[error("Model provider not supported: {0}")]
    ModelProviderNotSupported(String),

    #[error("Memory store error: {0}")]
    MemoryStoreError(String),

    #[error("Memory time window error: {0}")]
    MemoryTimeWindow(String),

    #[allow(dead_code)]
    #[error("Memory ego error: {0}")]
    MemoryEgoError(String),

    #[allow(dead_code)]
    #[error("WS connection error: {0}")]
    WsConnectionError(String),

    #[allow(dead_code)]
    #[error("WS bind error: {0}")]
    WsBindError(String),

    #[allow(dead_code)]
    #[error("Station connection error: {0}")]
    StationConnectionError(String),

    #[error("Invalid command: {0}")]
    InvalidCommand(String),

    #[allow(dead_code)]
    #[error("Permission denied")]
    PermissionDenied,

    #[allow(dead_code)]
    #[error("Mode conflict: {0}")]
    ModeConflict(String),

    #[allow(dead_code)]
    #[error("Context overflow")]
    ContextOverflow,

    #[allow(dead_code)]
    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("IO error: {0}")]
    IoError(String),

    #[error("Reqwest error: {0}")]
    ReqwestError(#[from] reqwest::Error),

    #[error("Serde JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("WS error: {0}")]
    WsError(#[from] kai_ws::Error),

    #[allow(dead_code)]
    #[error("Internal error: {0}")]
    InternalError(String),
}

pub type Result<T> = std::result::Result<T, Error>;

// ========== 模式状态 ==========

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mode {
    Role,
    Event(String),
}

// ========== 会话标识 ==========

/// 保留 agent 的 memory-store/ego agent_id（"0"）：配置缺省/空串归一化目标，会话构建判保留用
pub const RESERVED_AGENT_ID: &str = "0";

/// 会话唯一标识：agent_id + role_name + mode 三元组
/// 所有绑定 channel 的信息去重，每个三元组 = 一个会话
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub agent_id: String,
    pub role_name: String,
    pub mode: Mode,
}

/// 记忆读写边界的 role 编码：事件模式拼 {role}-{event}（对 memory-store 透明），角色模式原样
/// role_name/mode 从 Session 运行态字段读（SessionKey 只做去重）
pub fn memory_role(role_name: &str, mode: &Mode) -> String {
    match mode {
        Mode::Event(event_id) => format!("{}-{}", role_name, event_id),
        Mode::Role => role_name.to_string(),
    }
}

// ========== 管理命令类型 ==========

#[derive(Debug)]
pub enum AdminCommand {
    Bind { messenger_id: String, user_id: String },
    // unbind 按 (messenger_id, user_id) 双字段移除 ChannelUser；若移除的是 outgoing 引用身份则清空 outgoing
    Unbind { messenger_id: String, user_id: String },
    /// 设/清空 out_channel：Some 设（覆盖 + 同 agent/role 唯一），None 清空
    BindOutgoing(Option<OutChannelParams>),
    Admin { messenger_id: String, user_id: String },
    Unadmin { messenger_id: String, user_id: String },
    SetRole(Option<String>),
    ModeEvent(Option<String>),
    ModeRole,
    Reenter(String),
    Model(ProviderModel, bool),   // /model <provider> <model> [true|false]；true 时写入 NexusRepo 默认模型
    /// 设置 channel 绑定的 agent 与 role（缺省用保留值：agent_id="0"、role_name=""）
    SetAgent { agent_id: Option<String>, role: Option<String> },
}

/// /bind-outgoing 命令参数（转 OutChannelConfig 持久化）
#[derive(Debug, Clone)]
pub struct OutChannelParams {
    pub messenger_id: String,
    pub user_id: String,
    pub group_id: String,
}

// ========== 模型相关 ==========

/// OpenAI function call：wire 为 {id, type:"function", function:{name, arguments(JSON 字符串)}}
/// 自定义 serde：序列化/反序列化即 OpenAI wire 形状（缓存/历史与 wire 一致）
/// 字段按编码规范用 Arc<String>/Arc<Value>（与 ToolCallRequest.tool_params 先例一致）
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: Arc<String>,
    pub name: Arc<String>,
    /// 参数对象；wire 时序列化为 JSON 字符串（function.arguments）
    pub arguments: Arc<serde_json::Value>,
}

/// ToolCall 序列化辅助：function 内嵌对象（name + arguments 字符串）
struct ToolCallFunction<'a> {
    name: &'a Arc<String>,
    arguments: &'a Arc<serde_json::Value>,
}

impl serde::Serialize for ToolCallFunction<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = serializer.serialize_struct("ToolCallFunction", 2)?;
        st.serialize_field("name", self.name)?;
        // arguments：参数对象序列化为 JSON 字符串（OpenAI wire 约定）
        st.serialize_field("arguments", &serde_json::to_string(&**self.arguments).map_err(serde::ser::Error::custom)?)?;
        st.end()
    }
}

impl serde::Serialize for ToolCall {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = serializer.serialize_struct("ToolCall", 3)?;
        st.serialize_field("id", &self.id)?;
        st.serialize_field("type", "function")?;
        st.serialize_field("function", &ToolCallFunction { name: &self.name, arguments: &self.arguments })?;
        st.end()
    }
}

/// 反序列化：容错解析 OpenAI wire 形状——id 必填；function.name 必填；
/// function.arguments 为字符串则 JSON 解析为对象、非字符串则直接用、缺失回退 Null；type 忽略
impl<'de> serde::Deserialize<'de> for ToolCall {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(deserializer)?;
        let id = v.get("id").and_then(|x| x.as_str()).map(String::from)
            .ok_or_else(|| serde::de::Error::custom("ToolCall 缺少 id"))?;
        let name = v["function"]["name"].as_str().map(String::from)
            .ok_or_else(|| serde::de::Error::custom("ToolCall 缺少 function.name"))?;
        // arguments：字符串 → JSON 解析；对象/其他值 → 直接用；缺失 → Null
        let arguments = match v["function"]["arguments"].clone() {
            serde_json::Value::String(s) => serde_json::from_str(&s).unwrap_or(serde_json::Value::Null),
            other => other,
        };
        Ok(ToolCall {
            id: Arc::new(id),
            name: Arc::new(name),
            arguments: Arc::new(arguments),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub content: Arc<String>,
    /// 思考内容：API 字段（DeepSeek reasoning_content / anthropic thinking block）
    pub reasoning_content: Arc<String>,
    /// 思考内容：<think> 标签解析（去标签）；与 reasoning_content 独立，不合并
    pub thinking: Arc<String>,
    #[allow(dead_code)]
    pub tool_calls: Vec<Arc<ToolCall>>,
    #[allow(dead_code)]
    pub finish_reason: Arc<String>,
}

/// OpenAI 兼容上下文消息：role 即枚举变体（内部标签序列化，role 与其他字段平级）
/// 字段按编码规范用 Arc<String>（Option 内同样 Arc 包裹）；tool_calls 为 Vec 不包裹
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    System { content: Arc<String> },
    User { content: Arc<String> },
    Assistant {
        content: Arc<String>,
        /// 工具调用场景必须随请求回传（DeepSeek：带 tools 的请求须完整回传否则 400；Kimi：单轮工具循环保留并回传）；
        /// 非工具调用可选（API 忽略）；openai_body 自动序列化；同时思考内容经步骤 7 写 memory-store think 记录
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<Arc<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<Arc<ToolCall>>>,
    },
    Tool {
        tool_call_id: Arc<String>,
        /// 调用的工具名（内部元数据）
        name: Arc<String>,
        /// 调用结果（JSON 字符串或文本）
        content: Arc<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_key_hash_eq_by_value() {
        use std::collections::HashSet;
        let a = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        let b = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Role };
        let c = SessionKey { agent_id: "a1".into(), role_name: "r1".into(), mode: Mode::Event("e1".into()) };
        let mut set = HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&b), "等值 SessionKey 应命中 HashSet");
        assert!(!set.contains(&c), "不同 mode 不应命中");
    }

    #[test]
    fn memory_role_encodes_event_only() {
        assert_eq!(memory_role("dev", &Mode::Role), "dev");
        assert_eq!(memory_role("dev", &Mode::Event("e1".into())), "dev-e1");
    }

    #[test]
    fn message_serialization_role_tag_same_level() {
        // 序列化：role 为平级标签字段（内部标签 tag=role + lowercase；键序为 serde 派生插入序，文本为探针确认结果）
        let cases: Vec<(Message, &str)> = vec![
            (Message::System { content: Arc::new("你是助手".into()) }, r#"{"role":"system","content":"你是助手"}"#),
            (Message::User { content: Arc::new("你好".into()) }, r#"{"role":"user","content":"你好"}"#),
            (Message::Assistant { content: Arc::new(String::new()), reasoning_content: Some(Arc::new("思考".into())), tool_calls: None }, r#"{"role":"assistant","content":"","reasoning_content":"思考"}"#),
            (Message::Assistant {
                content: Arc::new(String::new()),
                reasoning_content: None,
                tool_calls: Some(vec![Arc::new(ToolCall { id: Arc::new("call_1".into()), name: Arc::new("read".into()), arguments: Arc::new(serde_json::json!({"path": "/tmp/a.txt"})) })]),
            }, r#"{"role":"assistant","content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"/tmp/a.txt\"}"}}]}"#),
            (Message::Tool { tool_call_id: Arc::new("call_1".into()), name: Arc::new("read".into()), content: Arc::new("内容".into()) }, r#"{"role":"tool","tool_call_id":"call_1","name":"read","content":"内容"}"#),
        ];
        for (m, expected) in cases {
            assert_eq!(serde_json::to_string(&m).unwrap(), expected);
        }
    }

    #[test]
    fn message_deserialization_role_tag_same_level() {
        // 反序列化：role 标签定位变体，None 字段缺省
        let sys: Message = serde_json::from_str(r#"{"role":"system","content":"你是助手"}"#).unwrap();
        assert!(matches!(sys, Message::System { content } if content.as_str() == "你是助手"));

        let user: Message = serde_json::from_str(r#"{"role":"user","content":"你好"}"#).unwrap();
        assert!(matches!(user, Message::User { content } if content.as_str() == "你好"));

        let asst: Message = serde_json::from_str(r#"{"role":"assistant","content":"","reasoning_content":"思考"}"#).unwrap();
        assert!(matches!(asst, Message::Assistant { reasoning_content: Some(r), tool_calls: None, .. } if r.as_str() == "思考"));

        let asst2: Message = serde_json::from_str(r#"{"role":"assistant","content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"/tmp/a.txt\"}"}}]}"#).unwrap();
        assert!(matches!(&asst2, Message::Assistant { reasoning_content: None, tool_calls: Some(tcs), .. }
            if tcs[0].id.as_str() == "call_1" && tcs[0].name.as_str() == "read" && tcs[0].arguments["path"] == "/tmp/a.txt"));

        let tool: Message = serde_json::from_str(r#"{"role":"tool","tool_call_id":"call_1","name":"read","content":"内容"}"#).unwrap();
        assert!(matches!(tool, Message::Tool { tool_call_id, name, content }
            if tool_call_id.as_str() == "call_1" && name.as_str() == "read" && content.as_str() == "内容"));
    }

    #[test]
    fn message_assistant_optional_fields_omitted() {
        let m = Message::Assistant { content: Arc::new("回答".into()), reasoning_content: None, tool_calls: None };
        let v = serde_json::to_value(&m).unwrap();
        assert!(v.get("reasoning_content").is_none(), "None 字段不应序列化");
        assert!(v.get("tool_calls").is_none(), "None 字段不应序列化");
    }

    #[test]
    fn tool_call_wire_serde_roundtrip_and_tolerance() {
        // 序列化：wire 形状 {id, type:"function", function:{name, arguments(JSON 字符串)}}
        let tc = ToolCall { id: Arc::new("call_1".into()), name: Arc::new("read".into()), arguments: Arc::new(serde_json::json!({"path": "/tmp/a.txt"})) };
        let json = serde_json::to_string(&tc).unwrap();
        assert_eq!(json, r#"{"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"/tmp/a.txt\"}"}}"#, "arguments 序列化为 JSON 字符串");
        // 反序列化：wire 形状还原（arguments 解析回对象）
        let back: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id.as_str(), "call_1");
        assert_eq!(back.name.as_str(), "read");
        assert_eq!(back.arguments["path"], "/tmp/a.txt", "arguments 字符串解析回 JSON 对象");
        // 容错：arguments 为对象直接用；缺失回退 Null；type 忽略
        let obj_arg: ToolCall = serde_json::from_str(r#"{"id":"c","function":{"name":"n","arguments":{"a":1}}}"#).unwrap();
        assert_eq!(obj_arg.arguments["a"], 1);
        let no_arg: ToolCall = serde_json::from_str(r#"{"id":"c","function":{"name":"n"}}"#).unwrap();
        assert!(no_arg.arguments.is_null(), "缺失 arguments 回退 Null");
        // 缺 id / 缺 function.name 报错（parse_openai_response 整体回退空）
        assert!(serde_json::from_str::<ToolCall>(r#"{"type":"function"}"#).is_err());
        assert!(serde_json::from_str::<ToolCall>(r#"{"id":"c"}"#).is_err());
    }
}

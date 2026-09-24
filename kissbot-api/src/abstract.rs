//! 摘要记忆实现模块（memory-struct-abstract）的请求/响应类型。
//!
//! 事件与事件树查询、事件定位、归档状态与重建；接口路径与字段约定见
//! [memory-struct-abstract 技术规格](../../docs/spec/memory-struct-abstract.md)。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    ChannelKey, ChannelRecord, Content, RecordKind, ThinkRecord, ToolCallRecord, ToolResultRecord,
};

// ========== 事件原文副本 ==========

/// 事件原文副本：按记录类型分文件的原始记录（行内容与记忆存储的记录格式一致，
/// 类型由所在文件的类型段决定；API 上以 `record_type` 标签暴露）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "record_type", rename_all = "kebab-case")]
pub enum EventRecord {
    Channel(Arc<ChannelRecord>),
    Think(Arc<ThinkRecord>),
    ToolCall(Arc<ToolCallRecord>),
    ToolResult(Arc<ToolResultRecord>),
}

impl EventRecord {
    pub fn kind(&self) -> RecordKind {
        match self {
            EventRecord::Channel(_) => RecordKind::Channel,
            EventRecord::Think(_) => RecordKind::Think,
            EventRecord::ToolCall(_) => RecordKind::ToolCall,
            EventRecord::ToolResult(_) => RecordKind::ToolResult,
        }
    }

    pub fn time(&self) -> &str {
        match self {
            EventRecord::Channel(r) => r.time.as_str(),
            EventRecord::Think(r) => r.time.as_str(),
            EventRecord::ToolCall(r) => r.time.as_str(),
            EventRecord::ToolResult(r) => r.time.as_str(),
        }
    }

    pub fn sn(&self) -> u64 {
        match self {
            EventRecord::Channel(r) => r.sn,
            EventRecord::Think(r) => r.sn,
            EventRecord::ToolCall(r) => r.sn,
            EventRecord::ToolResult(r) => r.sn,
        }
    }

    /// 排序键（时间 + 序号）
    pub fn sort_key(&self) -> (&str, u64) {
        (self.time(), self.sn())
    }
}

// ========== 事件与事件树 ==========

/// 事件模式：角色模式（记录经模型判定切分为事件、可多层归并）与事件模式（作用域整体一个顶层叶子事件）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventMode {
    Role,
    Event,
}

/// 事件节点（tree.json 的节点形状）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventNode {
    pub event_id: Arc<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Arc<String>>,
    #[serde(default)]
    pub children: Vec<Arc<String>>,
    pub mode: EventMode,
    pub summary: Arc<String>,
    /// 本事件直接持有的原文副本条数（不含子事件）
    pub record_count: u64,
    pub start_time: Arc<String>,
    pub end_time: Arc<String>,
    pub created_at: Arc<String>,
    pub updated_at: Arc<String>,
}

impl EventNode {
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }
}

/// 事件摘要（分层归并/上下文装配用的最小事件描述）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventBrief {
    pub event_id: Arc<String>,
    pub summary: Arc<String>,
}

impl From<&EventNode> for EventBrief {
    fn from(node: &EventNode) -> Self {
        Self {
            event_id: node.event_id.clone(),
            summary: node.summary.clone(),
        }
    }
}

// ========== 定位 ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocateRequest {
    pub agent_id: Arc<String>,
    /// 记忆后缀；事件模式下形如 `dev-e1`
    pub role: Arc<String>,
    /// 待定位消息（可多条、可跨通道）：每条自带通道三元组，按通道分别与各自的当前事件比对
    pub messages: Vec<LocateMessage>,
}

/// 定位用新消息（公共消息格式中判定所需的字段）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocateMessage {
    /// 消息标识：与已存记录去重只按「通道三元组 + msg_id」判定；为空则不参与去重
    #[serde(default)]
    pub msg_id: Arc<String>,
    pub time: Arc<String>,
    pub user_name: Arc<String>,
    pub is_self: usize,
    /// 通道三元组：决定用哪个通道的当前事件指针做比对
    pub messenger_id: Arc<String>,
    pub self_user_id: Arc<String>,
    pub group_id: Arc<String>,
    pub content: Content,
}

impl LocateMessage {
    /// 新消息所属通道
    pub fn channel(&self) -> ChannelKey {
        ChannelKey {
            messenger_id: self.messenger_id.clone(),
            self_user_id: self.self_user_id.clone(),
            group_id: self.group_id.clone(),
        }
    }
}

/// 消息涉及的事件
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocatedEventRef {
    pub event_id: Arc<String>,
    /// 是否是涉及该事件的通道的当前事件
    #[serde(default)]
    pub is_current: bool,
}

/// 定位分组：一组属于同一事件的消息
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocatedGroup {
    /// 消息在请求 `messages` 中的下标（升序）
    pub messages: Vec<usize>,
    /// 该组涉及的事件；没有对应的已有事件时缺省（该组是新话题）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<Arc<String>>,
    #[serde(default)]
    pub is_current: bool,
}

/// 定位结果：消息涉及的全部事件 + 分组（哪几条消息属于哪个事件）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocateResponse {
    /// 消息涉及的全部事件（按首次出现顺序去重；都不涉及已有事件时为空）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<Arc<LocatedEventRef>>,
    /// 分组结果（按各组首条消息的下标升序）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Arc<LocatedGroup>>,
}

impl LocateResponse {
    pub fn empty() -> Self {
        Self { events: Vec::new(), groups: Vec::new() }
    }
}

// ========== 事件查询 ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventListRequest {
    pub agent_id: Arc<String>,
    pub role: Arc<String>,
    /// 父事件 id；缺省或 null 表示查询顶层事件
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Arc<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventListResponse {
    pub events: Vec<Arc<EventNode>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSummaryRequest {
    pub agent_id: Arc<String>,
    pub role: Arc<String>,
    pub event_id: Arc<String>,
}

/// 事件摘要：节点本身即摘要侧表示（摘要文本 + 层级 + 记录数 + 起止时间）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSummaryResponse {
    pub event: Arc<EventNode>,
}

/// 事件原文副本的位置（分页游标）：与某一条原文副本一一对应，
/// 调用方把上一条记录的 `time` / `sn` / `record_type` 原样回传
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventCursor {
    pub time: Arc<String>,
    pub sn: u64,
    /// 记录类型（`channel` / `think` / `tool-call` / `tool-result`）：
    /// 各类型文件的 `sn` 相互独立，同一时刻的先后只在同类型内可比
    pub record_type: RecordKind,
}

impl EventCursor {
    pub fn of(record: &EventRecord) -> Self {
        Self {
            time: Arc::new(record.time().to_string()),
            sn: record.sn(),
            record_type: record.kind(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecordsRequest {
    pub agent_id: Arc<String>,
    pub role: Arc<String>,
    pub event_id: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecordsAfterRequest {
    pub agent_id: Arc<String>,
    pub role: Arc<String>,
    pub event_id: Arc<String>,
    /// 只返回该位置之后的原文副本
    pub after: EventCursor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecordsTailRequest {
    pub agent_id: Arc<String>,
    pub role: Arc<String>,
    pub event_id: Arc<String>,
    /// 只返回结尾若干条原文副本
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecordsResponse {
    /// 原文副本（四类记录按 (time, sn) 升序）
    pub records: Vec<Arc<EventRecord>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeGetRequest {
    pub agent_id: Arc<String>,
    pub role: Arc<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeGetResponse {
    /// 各通道的当前事件指针（无指针的通道不出现）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub current_events: Vec<Arc<ChannelEventPointer>>,
    pub nodes: Vec<Arc<EventNode>>,
}

// ========== 状态与重建 ==========

/// 各记录类型的已处理位置（时间 + 序号）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WatermarkPoint {
    pub time: Arc<String>,
    pub sn: u64,
}

impl Default for WatermarkPoint {
    fn default() -> Self {
        Self {
            time: Arc::new(WATERMARK_ZERO_TIME.to_string()),
            sn: 0,
        }
    }
}

/// 水位起点（早于任何真实记录）
pub const WATERMARK_ZERO_TIME: &str = "2000-01-01 00:00:00";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Watermark {
    pub channel: WatermarkPoint,
    pub think: WatermarkPoint,
    #[serde(rename = "tool-call")]
    pub tool_call: WatermarkPoint,
    #[serde(rename = "tool-result")]
    pub tool_result: WatermarkPoint,
}

impl Default for Watermark {
    fn default() -> Self {
        Self {
            channel: WatermarkPoint::default(),
            think: WatermarkPoint::default(),
            tool_call: WatermarkPoint::default(),
            tool_result: WatermarkPoint::default(),
        }
    }
}

impl Watermark {
    pub fn point(&self, kind: crate::RecordKind) -> &WatermarkPoint {
        match kind {
            crate::RecordKind::Channel => &self.channel,
            crate::RecordKind::Think => &self.think,
            crate::RecordKind::ToolCall => &self.tool_call,
            crate::RecordKind::ToolResult => &self.tool_result,
        }
    }

    pub fn point_mut(&mut self, kind: crate::RecordKind) -> &mut WatermarkPoint {
        match kind {
            crate::RecordKind::Channel => &mut self.channel,
            crate::RecordKind::Think => &mut self.think,
            crate::RecordKind::ToolCall => &mut self.tool_call,
            crate::RecordKind::ToolResult => &mut self.tool_result,
        }
    }
}

/// 通道的当前事件指针（current.json 的一条）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelEventPointer {
    pub channel: ChannelKey,
    pub event_id: Arc<String>,
}

/// 通道水位（watermark.json 的一条）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelWatermark {
    pub channel: ChannelKey,
    pub watermark: Arc<Watermark>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebuildRequest {
    pub agent_id: Arc<String>,
    pub role: Arc<String>,
    /// false：手动触发一次归档（等同通知触发的判定）；true：清空事件树后按共享记忆文件重建
    #[serde(default)]
    pub rebuild: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebuildResponse {
    /// 本次归档的记录条数
    pub archived: u64,
    /// 作用域当前事件节点总数
    pub event_count: u64,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::RecordKind;

    fn message() -> LocateMessage {
        LocateMessage {
            time: Arc::new("2026-09-22 13:48:37".into()),
            user_name: Arc::new("alice".into()),
            is_self: 0,
            msg_id: Arc::new(String::new()),
            messenger_id: Arc::new("web".into()),
            self_user_id: Arc::new("self1".into()),
            group_id: Arc::new("g1".into()),
            content: Content::Text(Arc::new("你好".into())),
        }
    }

    #[test]
    fn test_watermark_serde_wire_shape() {
        let wm = Watermark::default();
        let v = serde_json::to_value(&wm).unwrap();
        assert_eq!(
            v,
            json!({
                "channel": {"time": "2000-01-01 00:00:00", "sn": 0},
                "think": {"time": "2000-01-01 00:00:00", "sn": 0},
                "tool-call": {"time": "2000-01-01 00:00:00", "sn": 0},
                "tool-result": {"time": "2000-01-01 00:00:00", "sn": 0}
            })
        );
        let back: Watermark = serde_json::from_value(v).unwrap();
        assert_eq!(back, wm);
    }

    #[test]
    fn test_watermark_point_accessors_cover_all_kinds() {
        let mut wm = Watermark::default();
        for kind in RecordKind::ALL {
            wm.point_mut(kind).sn = kind as u64 + 1;
        }
        for kind in RecordKind::ALL {
            assert_eq!(wm.point(kind).sn, kind as u64 + 1, "按类型读写水位");
        }
    }

    #[test]
    fn test_locate_request_response_serde() {
        let req = LocateRequest {
            agent_id: Arc::new("0".into()),
            role: Arc::new("dev".into()),
            messages: vec![message(), message()],
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["agent_id"], "0");
        assert_eq!(v["role"], "dev");
        assert_eq!(v["messages"].as_array().unwrap().len(), 2);
        assert_eq!(v["messages"][0]["messenger_id"], "web");
        assert_eq!(v["messages"][0]["self_user_id"], "self1");
        assert_eq!(v["messages"][0]["content"]["msg_type"], "Text");
        let back: LocateRequest = serde_json::from_value(v).unwrap();
        assert_eq!(back.messages.len(), 2);
        assert_eq!(back.messages[0].is_self, 0);
        assert_eq!(back.messages[0].channel(), ChannelKey::new("web", "self1", "g1"));

        // 都不涉及已有事件：events 与 groups 都为空
        assert_eq!(serde_json::to_value(LocateResponse::empty()).unwrap(), json!({}));

        // 命中：涉及的事件 + 分组（新话题的组没有 event_id）
        let hit = LocateResponse {
            events: vec![
                Arc::new(LocatedEventRef { event_id: Arc::new("e12".into()), is_current: true }),
                Arc::new(LocatedEventRef { event_id: Arc::new("e13".into()), is_current: false }),
            ],
            groups: vec![
                Arc::new(LocatedGroup { messages: vec![0, 1], event_id: Some(Arc::new("e12".into())), is_current: true }),
                Arc::new(LocatedGroup { messages: vec![2], event_id: Some(Arc::new("e13".into())), is_current: false }),
                Arc::new(LocatedGroup { messages: vec![3], event_id: None, is_current: false }),
            ],
        };
        let v = serde_json::to_value(&hit).unwrap();
        assert_eq!(v["events"][0], json!({"event_id": "e12", "is_current": true}));
        assert_eq!(v["groups"][0], json!({"messages": [0, 1], "event_id": "e12", "is_current": true}));
        assert!(v["groups"][2].get("event_id").is_none(), "新话题的组不带 event_id");
        let back: LocateResponse = serde_json::from_value(v).unwrap();
        assert_eq!(back.events.len(), 2);
        assert_eq!(back.groups.len(), 3);
        assert!(back.groups[2].event_id.is_none());
    }

    #[test]
    fn test_event_cursor_and_records_requests_serde() {
        let cursor = EventCursor::of(&EventRecord::Think(Arc::new(ThinkRecord {
            reasoning_content: Some(Arc::new("推理".into())),
            thinking: None,
            key: Arc::new("k1".into()),
            time: Arc::new("2026-09-22 13:48:37".into()),
            sn: 7,
        })));
        assert_eq!(cursor.time.as_str(), "2026-09-22 13:48:37");
        assert_eq!(cursor.sn, 7);
        assert_eq!(cursor.record_type, RecordKind::Think);
        let v = serde_json::to_value(&cursor).unwrap();
        assert_eq!(v, json!({"time": "2026-09-22 13:48:37", "sn": 7, "record_type": "think"}));

        let after: EventRecordsAfterRequest =
            serde_json::from_value(json!({"agent_id": "0", "role": "dev", "event_id": "e1", "after": v})).unwrap();
        assert_eq!(after.after.record_type, RecordKind::Think);

        let tail: EventRecordsTailRequest =
            serde_json::from_value(json!({"agent_id": "0", "role": "dev", "event_id": "e1", "count": 5})).unwrap();
        assert_eq!(tail.count, 5);

        // 摘要响应即节点本身（摘要文本 + 层级 + 记录数 + 起止时间）
        let summary = EventSummaryResponse { event: Arc::new(node()) };
        let v = serde_json::to_value(&summary).unwrap();
        assert_eq!(v["event"]["event_id"], "e12");
        assert_eq!(v["event"]["summary"], "摘要");
        assert!(v["event"].get("records").is_none(), "摘要响应不含原文副本");
    }

    fn node() -> EventNode {
        EventNode {
            event_id: Arc::new("e12".into()),
            parent_id: None,
            children: vec![Arc::new("e13".into())],
            mode: EventMode::Role,
            summary: Arc::new("摘要".into()),
            record_count: 42,
            start_time: Arc::new("2026-09-22 10:00:00".into()),
            end_time: Arc::new("2026-09-22 13:48:37".into()),
            created_at: Arc::new("2026-09-22 10:05:00".into()),
            updated_at: Arc::new("2026-09-22 13:49:00".into()),
        }
    }

    #[test]
    fn test_event_node_serde_and_leaf() {
        let node = EventNode {
            event_id: Arc::new("e12".into()),
            parent_id: None,
            children: vec![Arc::new("e13".into())],
            mode: EventMode::Role,
            summary: Arc::new("摘要".into()),
            record_count: 42,
            start_time: Arc::new("2026-09-22 10:00:00".into()),
            end_time: Arc::new("2026-09-22 13:48:37".into()),
            created_at: Arc::new("2026-09-22 10:05:00".into()),
            updated_at: Arc::new("2026-09-22 13:49:00".into()),
        };
        let v = serde_json::to_value(&node).unwrap();
        assert_eq!(v["mode"], "role");
        assert!(v["parent_id"].is_null(), "parent_id 为 null 时序列化为 null（tree.json 形状）");
        assert!(!node.is_leaf());
        assert_eq!(EventBrief::from(&node).event_id.as_str(), "e12");

        let leaf: EventNode = serde_json::from_value(json!({
            "event_id": "e13", "mode": "event", "summary": "s", "record_count": 0,
            "start_time": "t", "end_time": "t", "created_at": "t", "updated_at": "t"
        })).unwrap();
        assert!(leaf.is_leaf());
        assert!(leaf.parent_id.is_none());
        assert_eq!(leaf.mode, EventMode::Event);
    }

    #[test]
    fn test_event_list_request_parent_optional() {
        let top = serde_json::from_value::<EventListRequest>(json!({"agent_id": "0", "role": "dev"})).unwrap();
        assert!(top.parent_id.is_none(), "缺省查询顶层事件");
        let child = serde_json::from_value::<EventListRequest>(json!({"agent_id": "0", "role": "dev", "parent_id": "e12"})).unwrap();
        assert_eq!(child.parent_id.unwrap().as_str(), "e12");
    }

    #[test]
    fn test_rebuild_request_default_flag() {
        let req = serde_json::from_value::<RebuildRequest>(json!({"agent_id": "0", "role": "dev"})).unwrap();
        assert!(!req.rebuild, "缺省为手动触发");
        let req = serde_json::from_value::<RebuildRequest>(json!({"agent_id": "0", "role": "dev", "rebuild": true})).unwrap();
        assert!(req.rebuild);
    }

    #[test]
    fn test_event_record_tagged_shape() {
        let record = EventRecord::Channel(Arc::new(ChannelRecord {
            user_id: Arc::new("u1".into()),
            self_user_id: Arc::new("self".into()),
            messenger_id: Arc::new("web".into()),
            group_id: Arc::new("g1".into()),
            is_self: 1,
            msg_id: Arc::new(String::new()),
            messenger_name: Arc::new(String::new()),
            user_name: Arc::new("alice".into()),
            group_name: Arc::new(String::new()),
            content: Content::Text(Arc::new("你好".into())),
            time: Arc::new("2026-09-22 10:00:00".into()),
            sn: 3,
        }));
        let value = serde_json::to_value(&record).unwrap();
        // 内部标签：record_type 与记录字段平级，字段形状与记忆存储一致
        assert_eq!(value["record_type"], "channel");
        assert_eq!(value["sn"], 3);
        assert_eq!(value["content"]["msg_type"], "Text");
        let back: EventRecord = serde_json::from_value(value).unwrap();
        assert_eq!(back.kind(), RecordKind::Channel);
        assert_eq!(back.time(), "2026-09-22 10:00:00");
        assert_eq!(back.sn(), 3);
        assert_eq!(back.sort_key(), ("2026-09-22 10:00:00", 3));

        // 四类类型标签
        for (kind, label) in [
            (RecordKind::Channel, "channel"),
            (RecordKind::Think, "think"),
            (RecordKind::ToolCall, "tool-call"),
            (RecordKind::ToolResult, "tool-result"),
        ] {
            let record = match kind {
                RecordKind::Channel => EventRecord::Channel(Arc::new(ChannelRecord {
                    user_id: Arc::new("u".into()), self_user_id: Arc::new("s".into()),
                    messenger_id: Arc::new("m".into()), group_id: Arc::new("g".into()),
                    is_self: 0, msg_id: Arc::new(String::new()),
                    messenger_name: Arc::new(String::new()),
                    user_name: Arc::new("u".into()), group_name: Arc::new(String::new()),
                    content: Content::Text(Arc::new("t".into())),
                    time: Arc::new("t".into()), sn: 1,
                })),
                RecordKind::Think => EventRecord::Think(Arc::new(ThinkRecord {
                    reasoning_content: None, thinking: None,
                    key: Arc::new("k".into()), time: Arc::new("t".into()), sn: 1,
                })),
                RecordKind::ToolCall => EventRecord::ToolCall(Arc::new(ToolCallRecord {
                    tool_name: Arc::new("read".into()), tool_params: Arc::new(Value::Null),
                    key: Arc::new("k".into()), time: Arc::new("t".into()), sn: 1,
                })),
                RecordKind::ToolResult => EventRecord::ToolResult(Arc::new(ToolResultRecord {
                    tool_result: Arc::new(Value::Null), tool_error: Arc::new(Value::Null),
                    key: Arc::new("k".into()), time: Arc::new("t".into()), sn: 1,
                })),
            };
            assert_eq!(serde_json::to_value(&record).unwrap()["record_type"], label);
            assert_eq!(record.kind(), kind);
        }
    }

    #[test]
    fn test_channel_event_pointer_serde() {
        let p = ChannelEventPointer {
            channel: ChannelKey::new("web", "self1", "g1"),
            event_id: Arc::new("e13".into()),
        };
        assert_eq!(serde_json::to_value(&p).unwrap(), json!({
            "channel": {"messenger_id": "web", "self_user_id": "self1", "group_id": "g1"},
            "event_id": "e13"
        }));
        let back: ChannelEventPointer = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(back.event_id.as_str(), "e13");
    }
}

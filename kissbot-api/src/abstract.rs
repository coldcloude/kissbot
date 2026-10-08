//! 摘要记忆实现模块（memory-struct-abstract）的请求/响应类型。
//!
//! Case 与 case 树查询、定位、归档状态与重建；接口路径与字段约定见
//! [memory-struct-abstract 技术规格](../../docs/spec/memory-struct-abstract.md)。

use std::sync::Arc;

use arcstr::ArcStr;
use serde::{Deserialize, Serialize};

use crate::{
    ChannelKey, ChannelRecord, Content, RecordKind, RoleEvent, ThinkRecord, ToolCallRecord, ToolResultRecord,
};

// ========== Case 原文副本 ==========

/// Case 原文副本：按记录类型分文件的原始记录（行内容与记忆存储的记录格式一致，
/// 类型由所在文件的类型段决定；API 上以 `record_type` 标签暴露）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "record_type", rename_all = "kebab-case")]
pub enum CaseRecord {
    Channel(Arc<ChannelRecord>),
    Think(Arc<ThinkRecord>),
    ToolCall(Arc<ToolCallRecord>),
    ToolResult(Arc<ToolResultRecord>),
}

impl CaseRecord {
    pub fn kind(&self) -> RecordKind {
        match self {
            CaseRecord::Channel(_) => RecordKind::Channel,
            CaseRecord::Think(_) => RecordKind::Think,
            CaseRecord::ToolCall(_) => RecordKind::ToolCall,
            CaseRecord::ToolResult(_) => RecordKind::ToolResult,
        }
    }

    pub fn time(&self) -> &str {
        match self {
            CaseRecord::Channel(r) => r.time.as_str(),
            CaseRecord::Think(r) => r.time.as_str(),
            CaseRecord::ToolCall(r) => r.time.as_str(),
            CaseRecord::ToolResult(r) => r.time.as_str(),
        }
    }

    pub fn sn(&self) -> u64 {
        match self {
            CaseRecord::Channel(r) => r.sn,
            CaseRecord::Think(r) => r.sn,
            CaseRecord::ToolCall(r) => r.sn,
            CaseRecord::ToolResult(r) => r.sn,
        }
    }

    /// 排序键（时间 + 序号）
    pub fn sort_key(&self) -> (&str, u64) {
        (self.time(), self.sn())
    }
}

// ========== Case 与 case 树 ==========

/// Case 节点（tree.json 的节点形状）：只描述 case 本身，不带模式/事件信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseNode {
    pub case_name: ArcStr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<ArcStr>,
    #[serde(default)]
    pub children: Vec<ArcStr>,
    pub summary: ArcStr,
    /// 本 case 直接持有的原文副本条数（不含子 case）
    pub record_count: u64,
    pub start_time: ArcStr,
    pub end_time: ArcStr,
    pub created_at: ArcStr,
    pub updated_at: ArcStr,
}

impl CaseNode {
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }
}

/// Case 摘要（分层归并/上下文装配用的最小 case 描述）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaseBrief {
    pub case_name: ArcStr,
    pub summary: ArcStr,
}

impl From<&CaseNode> for CaseBrief {
    fn from(node: &CaseNode) -> Self {
        Self {
            case_name: node.case_name.clone(),
            summary: node.summary.clone(),
        }
    }
}

// ========== 定位 ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocateRequest {
    pub agent_id: ArcStr,
    /// 记忆后缀：角色模式（无事件）或事件模式（角色 + 事件标识）
    pub role_event: RoleEvent,
    /// 待定位消息（可多条、可跨通道）：每条自带通道三元组，按通道分别与各自的当前 case 比对
    pub messages: Vec<LocateMessage>,
}

/// 定位用新消息（公共消息格式中判定所需的字段）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocateMessage {
    /// 消息标识：与已存记录去重只按「通道三元组 + msg_id」判定；为空则不参与去重
    #[serde(default)]
    pub msg_id: ArcStr,
    pub time: ArcStr,
    pub user_name: ArcStr,
    pub is_self: usize,
    /// 通道三元组：决定用哪个通道的当前 case 指针做比对
    pub messenger_id: ArcStr,
    pub self_user_id: ArcStr,
    pub group_id: ArcStr,
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

/// 消息涉及的 case
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocatedCaseRef {
    pub case_name: ArcStr,
    /// 是否是涉及该 case 的通道的当前 case
    #[serde(default)]
    pub is_current: bool,
}

/// 定位分组：一组属于同一 case 的消息
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocatedGroup {
    /// 消息在请求 `messages` 中的下标（升序）
    pub messages: Vec<usize>,
    /// 该组所属 case：命中已有 case 即该 case；新话题会在本次定位中建出 case（记忆条数为 0）
    pub case_name: ArcStr,
    #[serde(default)]
    pub is_current: bool,
}

/// 定位结果：消息涉及的全部 case + 分组（哪几条消息属于哪个 case）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocateResponse {
    /// 消息涉及的全部 case（按首次出现顺序去重；都不涉及已有 case 时为空）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cases: Vec<Arc<LocatedCaseRef>>,
    /// 分组结果（按各组首条消息的下标升序）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Arc<LocatedGroup>>,
}

impl LocateResponse {
    pub fn empty() -> Self {
        Self { cases: Vec::new(), groups: Vec::new() }
    }
}

// ========== Case 查询 ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseListRequest {
    pub agent_id: ArcStr,
    pub role_event: RoleEvent,
    /// 父 case id；缺省或 null 表示查询顶层 case
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<ArcStr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseListResponse {
    pub cases: Vec<Arc<CaseNode>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseSummaryRequest {
    pub agent_id: ArcStr,
    pub role_event: RoleEvent,
    pub case_name: ArcStr,
}

/// Case 摘要：节点本身即摘要侧表示（摘要文本 + 层级 + 记录数 + 起止时间）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseSummaryResponse {
    pub case: Arc<CaseNode>,
}

/// Case 原文副本的位置（分页游标）：与某一条原文副本一一对应，
/// 调用方把上一条记录的 `time` / `sn` / `record_type` 原样回传
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseCursor {
    pub time: ArcStr,
    pub sn: u64,
    /// 记录类型（`channel` / `think` / `tool-call` / `tool-result`）：
    /// 各类型文件的 `sn` 相互独立，同一时刻的先后只在同类型内可比
    pub record_type: RecordKind,
}

impl CaseCursor {
    pub fn of(record: &CaseRecord) -> Self {
        Self {
            time: ArcStr::from(record.time()),
            sn: record.sn(),
            record_type: record.kind(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseRecordsRequest {
    pub agent_id: ArcStr,
    pub role_event: RoleEvent,
    pub case_name: ArcStr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseRecordsAfterRequest {
    pub agent_id: ArcStr,
    pub role_event: RoleEvent,
    pub case_name: ArcStr,
    /// 只返回该位置之后的原文副本
    pub after: CaseCursor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseRecordsTailRequest {
    pub agent_id: ArcStr,
    pub role_event: RoleEvent,
    pub case_name: ArcStr,
    /// 只返回结尾若干条原文副本
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseRecordsResponse {
    /// 原文副本（四类记录按 (time, sn) 升序）
    pub records: Vec<Arc<CaseRecord>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeGetRequest {
    pub agent_id: ArcStr,
    pub role_event: RoleEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeGetResponse {
    /// 各通道的当前 case 指针（无指针的通道不出现）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub current_cases: Vec<Arc<ChannelCasePointer>>,
    pub nodes: Vec<Arc<CaseNode>>,
}

// ========== 状态与重建 ==========

/// 各记录类型的已处理位置（时间 + 序号）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WatermarkPoint {
    pub time: ArcStr,
    pub sn: u64,
}

impl Default for WatermarkPoint {
    fn default() -> Self {
        Self {
            time: WATERMARK_ZERO_TIME.clone(),
            sn: 0,
        }
    }
}

/// 水位起点（早于任何真实记录）
pub const WATERMARK_ZERO_TIME: ArcStr = arcstr::literal!("2000-01-01 00:00:00");

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

/// 通道的当前 case 指针（current.json 的一条）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelCasePointer {
    pub channel: ChannelKey,
    pub case_name: ArcStr,
}

/// 通道水位（watermark.json 的一条）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelWatermark {
    pub channel: ChannelKey,
    pub watermark: Arc<Watermark>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebuildRequest {
    pub agent_id: ArcStr,
    pub role_event: RoleEvent,
    /// false：手动触发一次归档（等同通知触发的判定）；true：清空本次记忆后缀所属模式的子树后按共享记忆文件重建
    #[serde(default)]
    pub rebuild: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebuildResponse {
    /// 本次归档的记录条数
    pub archived: u64,
    /// 角色 case 树节点总数
    pub case_count: u64,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::RecordKind;

    fn message() -> LocateMessage {
        LocateMessage {
            time: ArcStr::from("2026-09-22 13:48:37"),
            user_name: ArcStr::from("alice"),
            is_self: 0,
            msg_id: ArcStr::new(),
            messenger_id: ArcStr::from("web"),
            self_user_id: ArcStr::from("self1"),
            group_id: ArcStr::from("g1"),
            content: Content::Text(ArcStr::from("你好")),
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
            agent_id: ArcStr::from("0"),
            role_event: RoleEvent::new("dev"),
            messages: vec![message(), message()],
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["agent_id"], "0");
        assert_eq!(v["role_event"], json!({"role_name": "dev"}));
        assert_eq!(v["messages"].as_array().unwrap().len(), 2);
        assert_eq!(v["messages"][0]["messenger_id"], "web");
        assert_eq!(v["messages"][0]["self_user_id"], "self1");
        assert_eq!(v["messages"][0]["content"]["msg_type"], "Text");
        let back: LocateRequest = serde_json::from_value(v).unwrap();
        assert_eq!(back.messages.len(), 2);
        assert_eq!(back.messages[0].is_self, 0);
        assert_eq!(back.messages[0].channel(), ChannelKey::new("web", "self1", "g1"));

        // 都不涉及已有 case：cases 与 groups 都为空
        assert_eq!(serde_json::to_value(LocateResponse::empty()).unwrap(), json!({}));

        // 命中：涉及的 case + 分组（每个组都命中或建出 case，case_name 必填）
        let hit = LocateResponse {
            cases: vec![
                Arc::new(LocatedCaseRef { case_name: ArcStr::from("e12"), is_current: true }),
                Arc::new(LocatedCaseRef { case_name: ArcStr::from("e13"), is_current: false }),
            ],
            groups: vec![
                Arc::new(LocatedGroup { messages: vec![0, 1], case_name: ArcStr::from("e12"), is_current: true }),
                Arc::new(LocatedGroup { messages: vec![2], case_name: ArcStr::from("e13"), is_current: false }),
                Arc::new(LocatedGroup { messages: vec![3], case_name: ArcStr::from("e14"), is_current: true }),
            ],
        };
        let v = serde_json::to_value(&hit).unwrap();
        assert_eq!(v["cases"][0], json!({"case_name": "e12", "is_current": true}));
        assert_eq!(v["groups"][0], json!({"messages": [0, 1], "case_name": "e12", "is_current": true}));
        assert_eq!(v["groups"][2], json!({"messages": [3], "case_name": "e14", "is_current": true}));
        let back: LocateResponse = serde_json::from_value(v).unwrap();
        assert_eq!(back.cases.len(), 2);
        assert_eq!(back.groups.len(), 3);
        assert_eq!(back.groups[2].case_name.as_str(), "e14");
    }

    #[test]
    fn test_case_cursor_and_records_requests_serde() {
        let cursor = CaseCursor::of(&CaseRecord::Think(Arc::new(ThinkRecord {
            reasoning_content: Some(ArcStr::from("推理")),
            thinking: None,
            key: ArcStr::from("k1"),
            time: ArcStr::from("2026-09-22 13:48:37"),
            sn: 7,
        })));
        assert_eq!(cursor.time.as_str(), "2026-09-22 13:48:37");
        assert_eq!(cursor.sn, 7);
        assert_eq!(cursor.record_type, RecordKind::Think);
        let v = serde_json::to_value(&cursor).unwrap();
        assert_eq!(v, json!({"time": "2026-09-22 13:48:37", "sn": 7, "record_type": "think"}));

        let after: CaseRecordsAfterRequest =
            serde_json::from_value(json!({"agent_id": "0", "role_event": {"role_name": "dev"}, "case_name": "e1", "after": v})).unwrap();
        assert_eq!(after.after.record_type, RecordKind::Think);

        let tail: CaseRecordsTailRequest =
            serde_json::from_value(json!({"agent_id": "0", "role_event": {"role_name": "dev"}, "case_name": "e1", "count": 5})).unwrap();
        assert_eq!(tail.count, 5);

        // 摘要响应即节点本身（摘要文本 + 层级 + 记录数 + 起止时间）
        let summary = CaseSummaryResponse { case: Arc::new(node()) };
        let v = serde_json::to_value(&summary).unwrap();
        assert_eq!(v["case"]["case_name"], "e12");
        assert_eq!(v["case"]["summary"], "摘要");
        assert!(v["case"].get("records").is_none(), "摘要响应不含原文副本");
    }

    fn node() -> CaseNode {
        CaseNode {
            case_name: ArcStr::from("e12"),
            parent_id: None,
            children: vec![ArcStr::from("e13")],
            summary: ArcStr::from("摘要"),
            record_count: 42,
            start_time: ArcStr::from("2026-09-22 10:00:00"),
            end_time: ArcStr::from("2026-09-22 13:48:37"),
            created_at: ArcStr::from("2026-09-22 10:05:00"),
            updated_at: ArcStr::from("2026-09-22 13:49:00"),
        }
    }

    #[test]
    fn test_case_node_serde_and_leaf() {
        let node = CaseNode {
            case_name: ArcStr::from("e12"),
            parent_id: None,
            children: vec![ArcStr::from("e13")],
            summary: ArcStr::from("摘要"),
            record_count: 42,
            start_time: ArcStr::from("2026-09-22 10:00:00"),
            end_time: ArcStr::from("2026-09-22 13:48:37"),
            created_at: ArcStr::from("2026-09-22 10:05:00"),
            updated_at: ArcStr::from("2026-09-22 13:49:00"),
        };
        let v = serde_json::to_value(&node).unwrap();
        assert!(v.get("mode").is_none(), "case 节点不带模式/事件信息");
        assert!(v["parent_id"].is_null(), "parent_id 为 null 时序列化为 null（tree.json 形状）");
        assert!(!node.is_leaf());
        assert_eq!(CaseBrief::from(&node).case_name.as_str(), "e12");

        // 旧 tree.json 里的 mode 字段被忽略，不影响加载
        let leaf: CaseNode = serde_json::from_value(json!({
            "case_name": "e13", "mode": "event", "summary": "s", "record_count": 0,
            "start_time": "t", "end_time": "t", "created_at": "t", "updated_at": "t"
        })).unwrap();
        assert!(leaf.is_leaf());
        assert!(leaf.parent_id.is_none());
        assert_eq!(leaf.summary.as_str(), "s");
    }

    #[test]
    fn test_case_list_request_parent_optional() {
        let top = serde_json::from_value::<CaseListRequest>(json!({"agent_id": "0", "role_event": {"role_name": "dev"}})).unwrap();
        assert!(top.parent_id.is_none(), "缺省查询顶层 case");
        let child = serde_json::from_value::<CaseListRequest>(json!({"agent_id": "0", "role_event": {"role_name": "dev"}, "parent_id": "e12"})).unwrap();
        assert_eq!(child.parent_id.unwrap().as_str(), "e12");
    }

    #[test]
    fn test_rebuild_request_default_flag() {
        let req = serde_json::from_value::<RebuildRequest>(json!({"agent_id": "0", "role_event": {"role_name": "dev"}})).unwrap();
        assert!(!req.rebuild, "缺省为手动触发");
        let req = serde_json::from_value::<RebuildRequest>(json!({"agent_id": "0", "role_event": {"role_name": "dev"}, "rebuild": true})).unwrap();
        assert!(req.rebuild);
    }

    #[test]
    fn test_case_record_tagged_shape() {
        let record = CaseRecord::Channel(Arc::new(ChannelRecord {
            user_id: ArcStr::from("u1"),
            self_user_id: ArcStr::from("self"),
            messenger_id: ArcStr::from("web"),
            group_id: ArcStr::from("g1"),
            is_self: 1,
            msg_id: ArcStr::new(),
            messenger_name: ArcStr::new(),
            user_name: ArcStr::from("alice"),
            group_name: ArcStr::new(),
            content: Content::Text(ArcStr::from("你好")),
            time: ArcStr::from("2026-09-22 10:00:00"),
            sn: 3,
        }));
        let value = serde_json::to_value(&record).unwrap();
        // 内部标签：record_type 与记录字段平级，字段形状与记忆存储一致
        assert_eq!(value["record_type"], "channel");
        assert_eq!(value["sn"], 3);
        assert_eq!(value["content"]["msg_type"], "Text");
        let back: CaseRecord = serde_json::from_value(value).unwrap();
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
                RecordKind::Channel => CaseRecord::Channel(Arc::new(ChannelRecord {
                    user_id: ArcStr::from("u"), self_user_id: ArcStr::from("s"),
                    messenger_id: ArcStr::from("m"), group_id: ArcStr::from("g"),
                    is_self: 0, msg_id: ArcStr::new(),
                    messenger_name: ArcStr::new(),
                    user_name: ArcStr::from("u"), group_name: ArcStr::new(),
                    content: Content::Text(ArcStr::from("t")),
                    time: ArcStr::from("t"), sn: 1,
                })),
                RecordKind::Think => CaseRecord::Think(Arc::new(ThinkRecord {
                    reasoning_content: None, thinking: None,
                    key: ArcStr::from("k"), time: ArcStr::from("t"), sn: 1,
                })),
                RecordKind::ToolCall => CaseRecord::ToolCall(Arc::new(ToolCallRecord {
                    tool_name: ArcStr::from("read"), tool_params: Arc::new(Value::Null),
                    key: ArcStr::from("k"), time: ArcStr::from("t"), sn: 1,
                })),
                RecordKind::ToolResult => CaseRecord::ToolResult(Arc::new(ToolResultRecord {
                    tool_result: Arc::new(Value::Null), tool_error: Arc::new(Value::Null),
                    key: ArcStr::from("k"), time: ArcStr::from("t"), sn: 1,
                })),
            };
            assert_eq!(serde_json::to_value(&record).unwrap()["record_type"], label);
            assert_eq!(record.kind(), kind);
        }
    }

    #[test]
    fn test_channel_event_pointer_serde() {
        let p = ChannelCasePointer {
            channel: ChannelKey::new("web", "self1", "g1"),
            case_name: ArcStr::from("e13"),
        };
        assert_eq!(serde_json::to_value(&p).unwrap(), json!({
            "channel": {"messenger_id": "web", "self_user_id": "self1", "group_id": "g1"},
            "case_name": "e13"
        }));
        let back: ChannelCasePointer = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(back.case_name.as_str(), "e13");
    }
}

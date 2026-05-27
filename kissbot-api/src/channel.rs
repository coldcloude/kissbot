use kai_ws::{LEN_PAYLOAD_TYPE, OFFSET_PAYLOAD_TYPE};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{LocalMap, LocalString, MapKind, StringKind, error::Result};

pub const TYPE_JOIN_GROUP: u32 = 0x00010001;

pub const TYPE_LEAVE_GROUP: u32 = 0x00010002;

pub const TYPE_INCOMING_MESSAGE: u32 = 0x00020003;

pub const TYPE_OUTGOING_MESSAGE: u32 = 0x00020004;

// ========== Messenger -> User -> Group-> Channel ==========

// ========== Channel Info ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfoGeneric<S>
where
    S: StringKind,
{
    pub channel_id: S::Type,
    pub messenger_id: S::Type,
    pub group_id: S::Type,
    pub user_id: S::Type,
}

pub trait ChannelInfoKind {
    type Type: Clone + Serialize + DeserializeOwned;
}

pub type ChannelInfoDTO = ChannelInfoGeneric<LocalString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalChannelInfo;

impl ChannelInfoKind for LocalChannelInfo {
    type Type = ChannelInfoDTO;
}

// ========== User Channel Map ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserChannelMapGeneric<M, C>
where
    M: MapKind,
    C: ChannelInfoKind,
{
    pub group_channel_map: M::Map<String, C::Type>,
}

pub type UserChannelMapDTO = UserChannelMapGeneric<LocalMap, LocalChannelInfo>;

// ========== Group Info ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupInfoGeneric<S>
where
    S: StringKind,
{
    pub group_id: S::Type,
    pub group_name: S::Type,
}

pub trait GroupInfoKind {
    type Type: Clone + Serialize + DeserializeOwned;
}

pub type GroupInfoDTO = GroupInfoGeneric<LocalString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalGroupInfo;

impl GroupInfoKind for LocalGroupInfo {
    type Type = GroupInfoDTO;
}

// ========== User Info ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfoGeneric<S, M, G>
where
    S: StringKind,
    M: MapKind,
    G: GroupInfoKind,
{
    pub user_id: S::Type,
    pub user_name: S::Type,
    pub group_map: M::Map<String, G::Type>,
}

pub trait UserInfoKind {
    type Type: Clone + Serialize + DeserializeOwned;
}

pub type UserInfoDTO = UserInfoGeneric<LocalString, LocalMap, LocalGroupInfo>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalUserInfo;

impl UserInfoKind for LocalUserInfo {
    type Type = UserInfoDTO;
}

// ========== Messenger Info ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessengerInfoGeneric<S, M, U>
where
    S: StringKind,
    M: MapKind,
    U: UserInfoKind,
{
    pub messenger_id: S::Type,
    pub messenger_name: S::Type,
    pub user_map: M::Map<String, U::Type>,
}

pub type MessengerInfoDTO = MessengerInfoGeneric<LocalString, LocalMap, LocalUserInfo>;

// ========================= Message & Attachment ==========================

// ========== Attachment Info ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentInfoGeneric<S>
where
    S: StringKind,
{
    pub att_id: S::Type,
    pub mime_type: S::Type,
    pub size_bytes: u64,
}

pub trait AttachmentInfoKind {
    type Type: Clone + Serialize + DeserializeOwned;
}

pub type AttachmentInfoDTO = AttachmentInfoGeneric<LocalString>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalAttachmentInfo;

impl AttachmentInfoKind for LocalAttachmentInfo {
    type Type = AttachmentInfoDTO;
}

// ========== Outgoing Message ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessageGeneric<S, M, A>
where
    S: StringKind,
    M: MapKind,
    A: AttachmentInfoKind,
{
    pub channel_id: S::Type,
    pub user_id: S::Type,
    pub is_self: usize,
    pub msg_type: S::Type,
    pub content: S::Type,
    pub time: S::Type,
    pub attachment_map: M::Map<String, A::Type>,
}

pub type OutgoingMessageDTO = OutgoingMessageGeneric<LocalString, LocalMap, LocalAttachmentInfo>;

// ========== Outgoing Message Response ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessageResponseGeneric<S, M>
where
    S: StringKind,
    M: MapKind,
{
    pub msg_id: S::Type,
    pub attachment_upload_id_map: M::Map<String, u32>,
}

pub type OutgoingMessageResponseDTO = OutgoingMessageResponseGeneric<LocalString, LocalMap>;

// ========== Attachment Download Request ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentDownloadRequestGeneric<S>
where
    S: StringKind,
{
    pub channel_id: S::Type,
    pub key: S::Type,
}

pub type AttachmentDownloadRequestDTO = AttachmentDownloadRequestGeneric<LocalString>;

// ========== Attachment Download Response Header ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentDownloadResponseHeaderGeneric<A>
where
    A: AttachmentInfoKind,
{
    pub download_id: u32,
    pub metadata: A::Type,
}

pub type AttachmentDownloadResponseHeaderDTO = AttachmentDownloadResponseHeaderGeneric<LocalAttachmentInfo>;

//========================= Attachment Binary ==========================

pub const BIN_TYPE_ATTACHMENT: u16 = 0x0001;

const OFFSET_ATT_ID: usize = OFFSET_PAYLOAD_TYPE + LEN_PAYLOAD_TYPE;
const LEN_ATT_ID: usize = 4;
const OFFSET_ATT_SIZE: usize = OFFSET_ATT_ID + LEN_ATT_ID;
const LEN_ATT_SIZE: usize = 4;
const OFFSET_ATT_POS: usize = OFFSET_ATT_SIZE + LEN_ATT_SIZE;
const LEN_ATT_POS: usize = 8;
const OFFSET_ATT_DATA: usize = OFFSET_ATT_POS + LEN_ATT_POS;

pub trait AttachmentProcessor {
    fn process_attachment(&self, id: u32, size: u32, pos: u64, data: &[u8]) -> Result<()>;
}

pub fn process_attachment<P: AttachmentProcessor>(data: &[u8], processor: &mut P) -> Result<()> {
    let id_bin: [u8; 4] = data[OFFSET_ATT_ID..OFFSET_ATT_ID + LEN_ATT_ID].try_into()?;
    let id = u32::from_be_bytes(id_bin);
    let size_bin: [u8; 4] = data[OFFSET_ATT_SIZE..OFFSET_ATT_SIZE + LEN_ATT_SIZE].try_into()?;
    let size = u32::from_be_bytes(size_bin);
    let pos_bin: [u8; 8] = data[OFFSET_ATT_POS..OFFSET_ATT_POS + LEN_ATT_POS].try_into()?;
    let pos = u64::from_be_bytes(pos_bin);
    let data = &data[OFFSET_ATT_DATA..];
    processor.process_attachment(id, size, pos, data)?;
    Ok(())
}

// ================= Receiving Message ==========================

// ========== Message Record ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessageGeneric<S>
where
    S: StringKind,
{
    pub msg_id: S::Type,
    pub channel_id: S::Type,
    pub user_id: S::Type,
    pub is_self: usize,
    pub msg_type: S::Type,
    pub content: S::Type,
    pub time: S::Type,
}

pub type IncomingMessageDTO = IncomingMessageGeneric<LocalString>;

pub type IncomingMessagesDTO = Vec<IncomingMessageDTO>;

// ========== Query & Bind ==========

// ========== Query Messenger Name Response ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryMessengerNamesResponseGeneric<S, M>
where
    S: StringKind,
    M: MapKind,
{
    pub messenger_map: M::Map<String, S::Type>,
}

pub type QueryMessengerNamesResponseDTO = QueryMessengerNamesResponseGeneric<LocalString, LocalMap>;

// ========== Query User Name Request ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryUserNamesRequestGeneric<S>
where
    S: StringKind,
{
    pub messenger_id: S::Type,
}

pub type QueryUserNamesRequestDTO = QueryUserNamesRequestGeneric<LocalString>;

// ========== Query User Name Response ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryUserNamesResponseGeneric<S, M>
where
    S: StringKind,
    M: MapKind,
{
    pub messenger_id: S::Type,
    pub user_map: M::Map<String, S::Type>,
}

pub type QueryUserNamesResponseDTO = QueryUserNamesResponseGeneric<LocalString, LocalMap>;

// ========== Bind Request ==========
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindRequestGeneric<S>
where
    S: StringKind,
{
    pub agent_id: S::Type,
    pub role_name: S::Type,
    pub messenger_id: S::Type,
    pub user_id: S::Type,
}

pub type BindRequestDTO = BindRequestGeneric<LocalString>;

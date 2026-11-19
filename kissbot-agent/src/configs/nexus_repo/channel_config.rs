use std::{collections::HashSet, sync::Arc};

use arcstr::ArcStr;
use kissbot_api::ChannelUser;
use serde::{Deserialize, Serialize};

/// channel 的连接与管理员配置；绑定身份由引用该 channel 的 pipeline 的 input_channels 决定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    /// agent 内部唯一标识，与消息方 messenger 无关
    pub channel_id: ArcStr,
    pub ws_url: ArcStr,
    pub admins: Arc<HashSet<Arc<ChannelUser>>>,
    /// 是否启用（连接由 enabled 控制）
    pub enabled: bool,
}

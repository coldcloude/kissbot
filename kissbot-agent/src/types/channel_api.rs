use kissbot_api::channel::ChannelUser;

// ========== 管理命令参数 ==========

/// channel 配置变更任务（纯数据；CommandRouter 构造，Nexus 排队调 ChannelManager 执行）
/// /bind、/unbind 统一走此枚举（out_channel 属 (agent, role) context，由 /bind-outgoing 纯配置写，不走此队列）
pub enum ChannelCommand {
    /// 绑定 channel 用户（bind_users 追加，HashSet 天然去重幂等）
    BindUser { channel_id: String, user: ChannelUser },
    /// 解绑 channel 用户（移除 bind_users）
    UnbindUser { channel_id: String, user: ChannelUser },
}

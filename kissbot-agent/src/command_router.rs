use std::sync::Arc;

use crate::types::{AdminCommand, CommandEffect, Error, Mode, Result};
use crate::config_manager::{ConfigManager, ProviderModel};
use kissbot_api::ChannelUser;
use crate::coordinator::{AgentCoordinator, RESERVED_AGENT_NAME, RESERVED_ROLE_NAME};

pub struct CommandRouter;

impl CommandRouter {
    /// 检查消息是否为管理命令（以 "/" 开头）
    pub fn is_command(content: &str) -> bool {
        content.starts_with('/')
    }

    /// 检查发送者是否为该来源 channel 的管理权限用户（per-channel，避免跨 channel 提权）
    pub async fn check_admin(
        config: &ConfigManager,
        channel_id: &str,
        messenger_id: &str,
        user_id: &str,
    ) -> bool {
        let admins = config.channel_admins(channel_id).await;
        admins.iter().any(|a| a.messenger_id == messenger_id && a.user_id == user_id)
    }

    /// 解析管理命令
    pub fn parse(content: &str) -> Result<AdminCommand> {
        let trimmed = content.trim();
        if !trimmed.starts_with('/') {
            return Err(Error::InvalidCommand("命令必须以 / 开头".to_string()));
        }

        let without_prefix = &trimmed[1..];
        let parts: Vec<&str> = without_prefix.split_whitespace().collect();
        if parts.is_empty() {
            return Err(Error::InvalidCommand("空命令".to_string()));
        }

        match parts[0] {
            "bind" => {
                if parts.len() < 4 || parts[1] != "messenger" {
                    return Err(Error::InvalidCommand(
                        "格式: /bind messenger <messenger_id> <user_id>".to_string()
                    ));
                }
                Ok(AdminCommand::Bind {
                    messenger_id: parts[2].to_string(),
                    user_id: parts[3].to_string(),
                })
            }
            "unbind" => {
                if parts.len() < 3 || parts[1] != "messenger" {
                    return Err(Error::InvalidCommand(
                        "格式: /unbind messenger <messenger_id>".to_string()
                    ));
                }
                Ok(AdminCommand::Unbind {
                    messenger_id: parts[2].to_string(),
                })
            }
            "admin" => {
                if parts.len() < 3 {
                    return Err(Error::InvalidCommand(
                        "格式: /admin <messenger_id> <user_id>".to_string()
                    ));
                }
                Ok(AdminCommand::Admin {
                    messenger_id: parts[1].to_string(),
                    user_id: parts[2].to_string(),
                })
            }
            "unadmin" => {
                if parts.len() < 3 {
                    return Err(Error::InvalidCommand(
                        "格式: /unadmin <messenger_id> <user_id>".to_string()
                    ));
                }
                Ok(AdminCommand::Unadmin {
                    messenger_id: parts[1].to_string(),
                    user_id: parts[2].to_string(),
                })
            }
            "agent" => {
                // /agent [name] [role]：缺省 agent_name 用保留 agent（空），缺省 role 用保留 role（空）
                let agent_name = parts.get(1).map(|s| s.to_string());
                let role = parts.get(2).map(|s| s.to_string());
                Ok(AdminCommand::SetAgent { agent_name, role })
            }
            "role" => {
                // /role [name]：缺省用保留 role（空）
                let role = parts.get(1).map(|s| s.to_string());
                Ok(AdminCommand::SetRole(role))
            }
            "mode" => {
                if parts.len() < 2 {
                    return Err(Error::InvalidCommand(
                        "格式: /mode event [event-id] 或 /mode role".to_string()
                    ));
                }
                match parts[1] {
                    "event" => {
                        if parts.len() >= 3 {
                            Ok(AdminCommand::ModeEvent(Some(parts[2].to_string())))
                        } else {
                            Ok(AdminCommand::ModeEvent(None))
                        }
                    }
                    "role" => Ok(AdminCommand::ModeRole),
                    _ => Err(Error::InvalidCommand(format!("未知模式: {}", parts[1]))),
                }
            }
            "reenter" => {
                if parts.len() < 2 {
                    return Err(Error::InvalidCommand(
                        "格式: /reenter <event-id>".to_string()
                    ));
                }
                Ok(AdminCommand::Reenter(parts[1].to_string()))
            }
            "send-channel" => {
                if parts.len() < 2 || !matches!(parts[1], "on" | "off") {
                    return Err(Error::InvalidCommand(
                        "格式: /send-channel on|off".to_string()
                    ));
                }
                Ok(AdminCommand::SendChannel(parts[1] == "on"))
            }
            "events" => Ok(AdminCommand::Events),
            "reset" => Ok(AdminCommand::Reset),
            "model" => {
                if parts.len() != 3 {
                    return Err(Error::InvalidCommand("格式: /model <provider> <model>".to_string()));
                }
                Ok(AdminCommand::Model(ProviderModel {
                    provider: parts[1].to_string(),
                    model: parts[2].to_string(),
                }))
            }
            _ => Err(Error::InvalidCommand(format!("未知命令: {}", parts[0]))),
        }
    }

    /// 执行管理命令（返回回复文本和协调器后续动作）
    /// bind/agent/role/send-channel/admin/unadmin 走 ConfigManager 回写；
    /// mode/reenter 改运行态模式（coordinator）；model 改会话模型（运行态）。
    pub async fn execute(
        command: &AdminCommand,
        config: &ConfigManager,
        coordinator: &AgentCoordinator,
        channel_id: &str,
    ) -> Result<(String, CommandEffect)> {
        match command {
            AdminCommand::Bind { messenger_id, user_id } => {
                config.update_channel(channel_id, |c| {
                    c.bind_user = ChannelUser {
                        messenger_id: messenger_id.clone(),
                        user_id: user_id.clone(),
                    };
                }).await?;
                Ok((format!("✅ 已绑定 channel 用户: {} / {}", messenger_id, user_id), CommandEffect::Relocate))
            }
            AdminCommand::Unbind { .. } => {
                // 当前阶段 /unbind 暂不进行任何操作（channel 必须保持 bind 状态）
                Ok(("ℹ️ /unbind 暂不支持，channel 需保持绑定状态".to_string(), CommandEffect::None))
            }
            AdminCommand::Admin { messenger_id, user_id } => {
                config.add_admin(channel_id, &ChannelUser {
                    messenger_id: messenger_id.clone(),
                    user_id: user_id.clone(),
                }).await?;
                Ok((format!("✅ 已添加管理权限: {} / {}", messenger_id, user_id), CommandEffect::None))
            }
            AdminCommand::Unadmin { messenger_id, user_id } => {
                config.remove_admin(channel_id, messenger_id, user_id).await?;
                Ok((format!("✅ 已移除管理权限: {} / {}", messenger_id, user_id), CommandEffect::None))
            }
            AdminCommand::SetAgent { agent_name, role } => {
                let new_agent = agent_name.clone().unwrap_or_else(|| RESERVED_AGENT_NAME.to_string());
                let new_role = role.clone().unwrap_or_else(|| RESERVED_ROLE_NAME.to_string());
                // 切换前先解析新 agent：失败则保持原有 agent 不变（配置与运行态均不改）
                let agent_id = coordinator.resolve_agent_id_for_bind(&new_agent).await?;
                config.update_channel(channel_id, |c| {
                    c.agent_name = Arc::new(new_agent.clone());
                    c.role_name = Arc::new(new_role.clone());
                }).await?;
                // 切换成功：写入 channel 运行态 agent_id
                coordinator.set_channel_runtime(channel_id, agent_id).await;
                Ok((format!("✅ 已设置 agent: {} / role: {}", new_agent, new_role), CommandEffect::Relocate))
            }
            AdminCommand::SetRole(role) => {
                let new_role = role.clone().unwrap_or_else(|| RESERVED_ROLE_NAME.to_string());
                config.update_channel(channel_id, |c| {
                    c.role_name = Arc::new(new_role.clone());
                }).await?;
                Ok((format!("✅ 已设置 role: {}", new_role), CommandEffect::Relocate))
            }
            AdminCommand::ModeEvent(event_id) => {
                let id = event_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                coordinator.set_channel_mode(channel_id, Mode::Event(id.clone())).await;
                Ok((format!("✅ 新事件 ID: {}", id), CommandEffect::Relocate))
            }
            AdminCommand::ModeRole => {
                coordinator.set_channel_mode(channel_id, Mode::Role).await;
                Ok(("✅ 已切换为角色模式".to_string(), CommandEffect::Relocate))
            }
            AdminCommand::Reenter(event_id) => {
                coordinator.set_channel_mode(channel_id, Mode::Event(event_id.clone())).await;
                Ok((format!("✅ 将重进事件: {}", event_id), CommandEffect::Relocate))
            }
            AdminCommand::SendChannel(on) => {
                coordinator.set_send_channel(channel_id, *on).await?;
                Ok((
                    if *on { "✅ 已设为发送 channel".to_string() } else { "✅ 已取消发送 channel".to_string() },
                    CommandEffect::None,
                ))
            }
            AdminCommand::Events => {
                let reply = coordinator.list_events(channel_id).await?;
                Ok((reply, CommandEffect::None))
            }
            AdminCommand::Reset => {
                Ok(("🔄 正在重置上下文...".to_string(), CommandEffect::ResetSession))
            }
            AdminCommand::Model(pm) => {
                coordinator.set_session_model(channel_id, pm.clone()).await?;
                Ok((format!("✅ 已切换模型为: {}/{}", pm.provider, pm.model), CommandEffect::None))
            }
        }
    }
}

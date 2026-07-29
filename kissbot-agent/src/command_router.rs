use crate::types::{AdminCommand, Error, Result};
use crate::config_manager::{ConfigManager, ChannelBinding, AdminUser};

pub struct CommandRouter;

impl CommandRouter {
    /// 检查消息是否为管理命令（以 "/" 开头）
    pub fn is_command(content: &str) -> bool {
        content.starts_with('/')
    }

    /// 检查发送者是否为管理权限用户
    pub async fn check_admin(
        config: &ConfigManager,
        messenger_id: &str,
        user_id: &str,
    ) -> bool {
        let admins = config.admin_users().await;
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
            "role" => {
                if parts.len() >= 2 {
                    Ok(AdminCommand::SetRole(Some(parts[1].to_string())))
                } else {
                    Ok(AdminCommand::SetRole(None))
                }
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
            "events" => Ok(AdminCommand::Events),
            "reset" => Ok(AdminCommand::Reset),
            _ => Err(Error::InvalidCommand(format!("未知命令: {}", parts[0]))),
        }
    }

    /// 执行管理命令（返回回复文本和是否需要触发上下文重建）
    pub async fn execute(
        command: &AdminCommand,
        config: &ConfigManager,
    ) -> Result<(String, bool)> {
        match command {
            AdminCommand::Bind { messenger_id, user_id } => {
                config.add_binding(ChannelBinding {
                    messenger_id: messenger_id.clone(),
                    user_id: user_id.clone(),
                }).await?;
                Ok((format!("✅ 已绑定 channel 用户: {} / {}", messenger_id, user_id), false))
            }
            AdminCommand::Unbind { messenger_id } => {
                config.remove_binding(messenger_id).await?;
                Ok((format!("✅ 已解绑 messenger: {}", messenger_id), false))
            }
            AdminCommand::Admin { messenger_id, user_id } => {
                config.add_admin(AdminUser {
                    messenger_id: messenger_id.clone(),
                    user_id: user_id.clone(),
                }).await?;
                Ok((format!("✅ 已添加管理权限: {} / {}", messenger_id, user_id), false))
            }
            AdminCommand::Unadmin { messenger_id, user_id } => {
                config.remove_admin(messenger_id, user_id).await?;
                Ok((format!("✅ 已移除管理权限: {} / {}", messenger_id, user_id), false))
            }
            AdminCommand::SetRole(role) => {
                config.set_current_role(role.clone()).await?;
                let msg = match role {
                    Some(name) => format!("✅ 已切换角色为: {}", name),
                    None => "✅ 已取消角色".to_string(),
                };
                Ok((msg, true))  // 角色切换触发上下文重建
            }
            AdminCommand::ModeEvent(event_id) => {
                let id = match event_id {
                    Some(id) => id.clone(),
                    None => uuid::Uuid::new_v4().to_string(),
                };
                // 模式切换由 Coordinator 处理，这里只返回新 event_id
                Ok((format!("✅ 新事件 ID: {}", id), true))
            }
            AdminCommand::ModeRole => {
                Ok(("✅ 已切换为角色模式".to_string(), true))
            }
            AdminCommand::Reenter(event_id) => {
                Ok((format!("✅ 将重进事件: {}", event_id), true))
            }
            AdminCommand::Events => {
                // Events 由 Coordinator 通过 MemoryReader 查询
                Ok(("📋 查询事件列表中...".to_string(), false))
            }
            AdminCommand::Reset => {
                Ok(("🔄 正在重置上下文...".to_string(), true))
            }
        }
    }
}

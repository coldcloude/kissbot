use std::sync::Arc;

use crate::types::{AdminCommand, CommandEffect, Error, Mode, OutChannelParams, Result};
use crate::config_manager::{ConfigManager, OutChannelConfig, ProviderModel};
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
                if parts.len() < 4 || parts[1] != "messenger" {
                    return Err(Error::InvalidCommand(
                        "格式: /unbind messenger <messenger_id> <user_id>".to_string()
                    ));
                }
                Ok(AdminCommand::Unbind {
                    messenger_id: parts[2].to_string(),
                    user_id: parts[3].to_string(),
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
            "bind-outgoing" => {
                // /bind-outgoing <messenger_id> <user_id> <group_id> 或 /bind-outgoing off
                if parts.len() == 2 && parts[1] == "off" {
                    return Ok(AdminCommand::BindOutgoing(None));
                }
                if parts.len() < 4 {
                    return Err(Error::InvalidCommand(
                        "格式: /bind-outgoing <messenger_id> <user_id> <group_id> 或 /bind-outgoing off".to_string()
                    ));
                }
                Ok(AdminCommand::BindOutgoing(Some(OutChannelParams {
                    messenger_id: parts[1].to_string(),
                    user_id: parts[2].to_string(),
                    group_id: parts[3].to_string(),
                })))
            }
            "events" => Ok(AdminCommand::Events),
            "reset" => Ok(AdminCommand::Reset),
            "model" => {
                // /model <provider> <model> [true|false]：第 4 段省略时默认 false（true 则写入 NexusRepo 默认模型）
                if parts.len() < 3 || parts.len() > 4 {
                    return Err(Error::InvalidCommand("格式: /model <provider> <model> [true|false]".to_string()));
                }
                let set_default = match parts.get(3) {
                    None => false,
                    Some(v) => match *v {
                        "true" => true,
                        "false" => false,
                        _ => return Err(Error::InvalidCommand("格式: /model <provider> <model> [true|false]".to_string())),
                    },
                };
                Ok(AdminCommand::Model(ProviderModel {
                    provider: parts[1].to_string(),
                    model: parts[2].to_string(),
                }, set_default))
            }
            _ => Err(Error::InvalidCommand(format!("未知命令: {}", parts[0]))),
        }
    }

    /// 执行管理命令（返回回复文本和协调器后续动作）
    /// bind/agent/role/bind-outgoing/admin/unadmin 走 ConfigManager 回写；
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
                    // 追加去重：已存在则幂等忽略
                    let cu = ChannelUser { messenger_id: messenger_id.clone(), user_id: user_id.clone() };
                    if !c.bind_users.iter().any(|b| b == &cu) {
                        c.bind_users.push(cu);
                    }
                }).await?;
                Ok((format!("✅ 已绑定 channel 用户: {} / {}", messenger_id, user_id), CommandEffect::None))
            }
            AdminCommand::Unbind { messenger_id, user_id } => {
                config.update_channel(channel_id, |c| {
                    // 移除指定 ChannelUser
                    c.bind_users.retain(|b| !(b.messenger_id == *messenger_id && b.user_id == *user_id));
                    // 移除的是 outgoing 引用身份则清空 outgoing（避免悬空引用）
                    if let Some(out) = &c.outgoing {
                        if out.messenger_id.as_str() == messenger_id && out.user_id.as_str() == user_id {
                            c.outgoing = None;
                        }
                    }
                }).await?;
                Ok((format!("✅ 已移除 channel 用户: {} / {}", messenger_id, user_id), CommandEffect::None))
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
                // 切换前先解析新 agent：失败则保持原有 agent 不变（只读 API，队列外，避免阻塞变更队列）
                let agent_id = coordinator.resolve_agent_id_for_bind(&new_agent).await?;
                // 写 config + 运行态 + 会话重定位走串行队列（防写-写竞态），返回时已生效
                coordinator.change_channel_agent_role(channel_id, &new_agent, &new_role, agent_id).await?;
                Ok((format!("✅ 已设置 agent: {} / role: {}", new_agent, new_role), CommandEffect::None))
            }
            AdminCommand::SetRole(role) => {
                let new_role = role.clone().unwrap_or_else(|| RESERVED_ROLE_NAME.to_string());
                coordinator.change_channel_role(channel_id, &new_role).await?;
                Ok((format!("✅ 已设置 role: {}", new_role), CommandEffect::None))
            }
            AdminCommand::ModeEvent(event_id) => {
                let id = event_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                coordinator.change_channel_mode(channel_id, Mode::Event(id.clone())).await?;
                Ok((format!("✅ 新事件 ID: {}", id), CommandEffect::None))
            }
            AdminCommand::ModeRole => {
                coordinator.change_channel_mode(channel_id, Mode::Role).await?;
                Ok(("✅ 已切换为角色模式".to_string(), CommandEffect::None))
            }
            AdminCommand::Reenter(event_id) => {
                coordinator.change_channel_mode(channel_id, Mode::Event(event_id.clone())).await?;
                Ok((format!("✅ 将重进事件: {}", event_id), CommandEffect::None))
            }
            AdminCommand::BindOutgoing(params) => {
                match params {
                    Some(p) => {
                        // 1. 校验 ChannelUser 已绑定
                        let channels = config.channels().await;
                        let src = channels.iter().find(|(id, _)| id == channel_id).map(|(_, c)| c.clone())
                            .ok_or_else(|| Error::ConfigNotFound(format!("channel 不存在: {}", channel_id)))?;
                        let bound = src.bind_users.iter()
                            .any(|b| b.messenger_id == p.messenger_id && b.user_id == p.user_id);
                        if !bound {
                            return Err(Error::InvalidCommand(format!(
                                "ChannelUser 未绑定: {} / {}", p.messenger_id, p.user_id)));
                        }
                        // 2. 清空同 (agent_name, role_name) 其他 channel 的 outgoing（保证至多 1 个）
                        for (cid, c) in channels.iter() {
                            if cid != channel_id && c.agent_name == src.agent_name && c.role_name == src.role_name {
                                if c.outgoing.is_some() {
                                    config.update_channel(cid, |cc| cc.outgoing = None).await?;
                                }
                            }
                        }
                        // 3. 设来源 channel 的 outgoing
                        config.update_channel(channel_id, |c| {
                            c.outgoing = Some(OutChannelConfig {
                                messenger_id: Arc::new(p.messenger_id.clone()),
                                user_id: Arc::new(p.user_id.clone()),
                                group_id: Arc::new(p.group_id.clone()),
                            });
                        }).await?;
                        Ok((format!("✅ 已设发送通道: {} / {} -> {}", p.messenger_id, p.user_id, p.group_id), CommandEffect::None))
                    }
                    None => {
                        config.update_channel(channel_id, |c| c.outgoing = None).await?;
                        Ok(("✅ 已取消发送通道（只存不回复）".to_string(), CommandEffect::None))
                    }
                }
            }
            AdminCommand::Events => {
                let reply = coordinator.list_events(channel_id).await?;
                Ok((reply, CommandEffect::None))
            }
            AdminCommand::Reset => {
                Ok(("🔄 正在重置上下文...".to_string(), CommandEffect::ResetSession))
            }
            AdminCommand::Model(pm, set_default) => {
                // 先切换会话模型（含 API 校验，失败保持原模型）；设为默认则写入 NexusRepo
                coordinator.set_session_model(channel_id, pm.clone()).await?;
                if *set_default {
                    config.set_default_model(pm.clone()).await?;
                }
                let mut reply = format!("✅ 已切换模型为: {}/{}", pm.provider, pm.model);
                if *set_default {
                    reply.push_str("（已设为默认）");
                }
                Ok((reply, CommandEffect::None))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== /model 解析：第 4 段可选 [true|false]，缺省 false =====

    #[test]
    fn parse_model_without_default_flag() {
        let cmd = CommandRouter::parse("/model deepseek deepseek-v4-flash").unwrap();
        match cmd {
            AdminCommand::Model(pm, set_default) => {
                assert_eq!(pm.provider, "deepseek");
                assert_eq!(pm.model, "deepseek-v4-flash");
                assert!(!set_default);
            }
            _ => panic!("expected Model"),
        }
    }

    #[test]
    fn parse_model_with_true_flag() {
        let cmd = CommandRouter::parse("/model deepseek deepseek-v4-flash true").unwrap();
        match cmd {
            AdminCommand::Model(pm, set_default) => {
                assert_eq!(pm.provider, "deepseek");
                assert_eq!(pm.model, "deepseek-v4-flash");
                assert!(set_default);
            }
            _ => panic!("expected Model"),
        }
    }

    #[test]
    fn parse_model_with_false_flag() {
        let cmd = CommandRouter::parse("/model deepseek deepseek-v4-flash false").unwrap();
        match cmd {
            AdminCommand::Model(_, set_default) => assert!(!set_default),
            _ => panic!("expected Model"),
        }
    }

    #[test]
    fn parse_model_rejects_invalid_flag() {
        let err = CommandRouter::parse("/model deepseek deepseek-v4-flash maybe").unwrap_err();
        assert!(matches!(err, Error::InvalidCommand(_)));
    }

    #[test]
    fn parse_model_rejects_missing_provider_or_model() {
        assert!(CommandRouter::parse("/model deepseek").is_err());
        assert!(CommandRouter::parse("/model deepseek deepseek-v4-flash true extra").is_err());
    }

    // ===== /bind-outgoing 解析：<m> <u> <g> 或 off =====

    #[test]
    fn parse_bind_outgoing_params_and_off() {
        let cmd = CommandRouter::parse("/bind-outgoing web u1 g1").unwrap();
        match cmd {
            AdminCommand::BindOutgoing(Some(p)) => {
                assert_eq!(p.messenger_id, "web");
                assert_eq!(p.user_id, "u1");
                assert_eq!(p.group_id, "g1");
            }
            _ => panic!("expected BindOutgoing(Some)"),
        }
        let off = CommandRouter::parse("/bind-outgoing off").unwrap();
        assert!(matches!(off, AdminCommand::BindOutgoing(None)), "off 应清空");
        // 参数不足拒绝
        assert!(CommandRouter::parse("/bind-outgoing web u1").is_err());
    }

    // ===== /unbind 解析：必须带 user_id =====

    #[test]
    fn parse_unbind_requires_user_id() {
        let cmd = CommandRouter::parse("/unbind messenger web u1").unwrap();
        match cmd {
            AdminCommand::Unbind { messenger_id, user_id } => {
                assert_eq!(messenger_id, "web");
                assert_eq!(user_id, "u1");
            }
            _ => panic!("expected Unbind"),
        }
        assert!(CommandRouter::parse("/unbind messenger web").is_err(), "缺 user_id 应拒绝");
    }

    // ===== /send-channel 已删除 =====

    #[test]
    fn parse_send_channel_removed() {
        assert!(CommandRouter::parse("/send-channel on").is_err(), "/send-channel 已删除");
    }
}

use std::sync::Arc;

use crate::types::{ChannelCommand, Error, Mode, OutChannelParams, RESERVED_AGENT_ID, Result};
use crate::config_manager::{ConfigManager, ProviderModel};
use kissbot_api::ChannelUser;
use crate::nexus::{Nexus, RESERVED_ROLE_NAME};

pub struct CommandRouter;

impl CommandRouter {
    /// 执行管理命令（nexus 已校验管理员后调用）：内联解析命令参数 + 直接执行，返回回复文本
    /// 错误经 Result 返回：InvalidCommand（解析/格式错误）由调用方拼 "⚠️ {}"，其余拼 "❌ 命令执行失败: {}"
    /// bind/unbind/bind-outgoing/admin/unadmin 走 ConfigManager 回写（bind 类经 nexus.channel_command 队列串行）；
    /// agent/role/mode/reenter 走 change_channel_key 队列；model 改会话模型（运行态）。
    /// Nexus 一律从单例取（不传参数）
    pub async fn execute(content: &str, channel_id: &str) -> Result<String> {
        let trimmed = content.trim();
        if !trimmed.starts_with('/') {
            return Err(Error::InvalidCommand("命令必须以 / 开头".to_string()));
        }
        let parts: Vec<&str> = trimmed[1..].split_whitespace().collect();
        if parts.is_empty() {
            return Err(Error::InvalidCommand("空命令".to_string()));
        }
        let nexus = Nexus::get();
        match parts[0] {
            "bind" => {
                if parts.len() < 4 || parts[1] != "messenger" {
                    return Err(Error::InvalidCommand(
                        "格式: /bind messenger <messenger_id> <user_id>".to_string()
                    ));
                }
                let cu = ChannelUser { messenger_id: parts[2].to_string(), user_id: parts[3].to_string() };
                // 统一走串行队列应用（防写-写竞态；bind_users 追加，HashSet 天然去重幂等）
                nexus.channel_command(ChannelCommand::BindUser { channel_id: channel_id.to_string(), user: cu }).await
            }
            "unbind" => {
                if parts.len() < 4 || parts[1] != "messenger" {
                    return Err(Error::InvalidCommand(
                        "格式: /unbind messenger <messenger_id> <user_id>".to_string()
                    ));
                }
                let cu = ChannelUser { messenger_id: parts[2].to_string(), user_id: parts[3].to_string() };
                // 统一走串行队列应用（防写-写竞态；移除 bind_users，outgoing 引用该身份则一并清空；回复文本队列内生成）
                nexus.channel_command(ChannelCommand::UnbindUser { channel_id: channel_id.to_string(), user: cu }).await
            }
            "admin" => {
                if parts.len() < 3 {
                    return Err(Error::InvalidCommand(
                        "格式: /admin <messenger_id> <user_id>".to_string()
                    ));
                }
                let messenger_id = parts[1].to_string();
                let user_id = parts[2].to_string();
                let reply = format!("✅ 已添加管理权限: {} / {}", messenger_id, user_id);
                ConfigManager::get().add_admin(channel_id, &ChannelUser {
                    messenger_id: messenger_id,
                    user_id: user_id,
                }).await?;
                Ok(reply)
            }
            "unadmin" => {
                if parts.len() < 3 {
                    return Err(Error::InvalidCommand(
                        "格式: /unadmin <messenger_id> <user_id>".to_string()
                    ));
                }
                let messenger_id = parts[1].to_string();
                let user_id = parts[2].to_string();
                let reply = format!("✅ 已移除管理权限: {} / {}", messenger_id, user_id);
                ConfigManager::get().remove_admin(channel_id, &messenger_id, &user_id).await?;
                Ok(reply)
            }
            "agent" => {
                // /agent [agent_id] [role]：缺省 agent_id 用保留 agent（"0"），缺省 role 用保留 role（空）
                let agent_id = parts.get(1).map(|s| s.to_string());
                let role = parts.get(2).map(|s| s.to_string());
                let new_agent_id = Arc::new(agent_id.filter(|s| !s.is_empty()).unwrap_or_else(|| RESERVED_AGENT_ID.to_string()));
                let new_role = Arc::new(role.unwrap_or_else(|| RESERVED_ROLE_NAME.to_string()));
                // 校验 agent 存在（空/保留 id 直接通过）；role 非空时一并校验（agent+role 联合校验）
                nexus.verify_agent_exists(new_agent_id.as_str()).await?;
                if !new_role.is_empty() {
                    nexus.verify_role_exists(new_agent_id.as_str(), new_role.as_str()).await?;
                }
                // None = 保持当前值（mode 保持当前运行态；Arc clone 浅拷贝入队）
                nexus.change_channel_key(channel_id, Some(new_agent_id.clone()), Some(new_role.clone()), None).await?;
                Ok(format!("✅ 已设置 agent: {} / role: {}", new_agent_id, new_role))
            }
            "role" => {
                // /role [name]：缺省用保留 role（空）
                let role = parts.get(1).map(|s| s.to_string());
                let new_role = Arc::new(role.unwrap_or_else(|| RESERVED_ROLE_NAME.to_string()));
                // 经 channel_id 校验 role 存在（内部取当前 agent_id；空串保留 role 直接通过；channel 不存在报错）
                nexus.verify_role_exists_for_channel(channel_id, new_role.as_str()).await?;
                // None = 保持当前值（agent/mode 不变；Arc clone 浅拷贝入队）
                nexus.change_channel_key(channel_id, None, Some(new_role.clone()), None).await?;
                Ok(format!("✅ 已设置 role: {}", new_role))
            }
            "mode" => {
                if parts.len() < 2 {
                    return Err(Error::InvalidCommand(
                        "格式: /mode event [event-id] 或 /mode role".to_string()
                    ));
                }
                match parts[1] {
                    "event" => {
                        // 缺省 event-id 生成新事件 ID；agent/role 保持当前值
                        let id = Arc::new(parts.get(2).map(|s| s.to_string()).unwrap_or_else(|| uuid::Uuid::new_v4().to_string()));
                        nexus.change_channel_key(channel_id, None, None, Some(Arc::new(Mode::Event(id.as_str().to_string())))).await?;
                        Ok(format!("✅ 新事件 ID: {}", id))
                    }
                    "role" => {
                        // agent/role 保持当前值
                        nexus.change_channel_key(channel_id, None, None, Some(Arc::new(Mode::Role))).await?;
                        Ok("✅ 已切换为角色模式".to_string())
                    }
                    _ => Err(Error::InvalidCommand(format!("未知模式: {}", parts[1]))),
                }
            }
            "reenter" => {
                if parts.len() < 2 {
                    return Err(Error::InvalidCommand(
                        "格式: /reenter <event-id>".to_string()
                    ));
                }
                // agent/role 保持当前值；mode 经 Arc 传入
                nexus.change_channel_key(channel_id, None, None, Some(Arc::new(Mode::Event(parts[1].to_string())))).await?;
                Ok(format!("✅ 将重进事件: {}", parts[1]))
            }
            "bind-outgoing" => {
                // /bind-outgoing <messenger_id> <user_id> <group_id>
                if parts.len() < 4 {
                    return Err(Error::InvalidCommand(
                        "格式: /bind-outgoing <messenger_id> <user_id> <group_id>".to_string()
                    ));
                }
                let params = OutChannelParams {
                    messenger_id: parts[1].to_string(),
                    user_id: parts[2].to_string(),
                    group_id: parts[3].to_string(),
                };
                // 校验 + 清同 agent/role 其他 channel + 设来源全部移入队列内 ChannelManager.bind_outgoing 原子执行（回复文本队列内生成）
                nexus.channel_command(ChannelCommand::BindOutgoing { channel_id: channel_id.to_string(), params }).await
            }
            "unbind-outgoing" => {
                // /unbind-outgoing：清空 out_channel（回到只存不回复模式）
                if parts.len() > 1 {
                    return Err(Error::InvalidCommand(
                        "格式: /unbind-outgoing（无参数）".to_string()
                    ));
                }
                // 清空经队列内 ChannelManager.clear_outgoing 执行（回到只存不回复模式；回复文本队列内生成）
                nexus.channel_command(ChannelCommand::ClearOutgoing { channel_id: channel_id.to_string() }).await
            }
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
                let pm = ProviderModel { provider: parts[1].to_string(), model: parts[2].to_string() };
                // 先切换会话模型（含 API 校验，失败保持原模型）；设为默认则写入 NexusRepo
                nexus.set_session_model(channel_id, pm.clone()).await?;
                if set_default {
                    ConfigManager::get().set_default_model(pm.clone()).await?;
                }
                let mut reply = format!("✅ 已切换模型为: {}/{}", pm.provider, pm.model);
                if set_default {
                    reply.push_str("（已设为默认）");
                }
                Ok(reply)
            }
            _ => Err(Error::InvalidCommand(format!("未知命令: {}", parts[0]))),
        }
    }
}

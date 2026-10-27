use std::sync::Arc;

use arcstr::ArcStr;
use crate::types::{ChannelCommand, Error, Result};
use crate::config_manager::ConfigManager;
use kissbot_api::{ChannelUser, RESERVED_AGENT_ID, RESERVED_ROLE_NAME};
use crate::nexus::Nexus;

pub struct CommandRouter;

impl CommandRouter {
    /// 默认管理命令检测与执行（nexus 已校验管理员后调用）：内联解析命令参数 + 直接执行，返回回复文本。
    /// `Ok(None)` 表示不是默认命令，由 nexus 交给来源 channel 的 pipeline 识别执行。
    /// 错误经 Result 返回：InvalidCommand（解析/格式错误）由调用方拼 "⚠️ {}"，其余拼 "❌ 命令执行失败: {}"
    /// bind/unbind 走 channel_manager.channel_command 队列串行（bind_users 回写）；
    /// agent/role/事件 走 channel_manager.change_channel_key 队列。
    /// Nexus 一律从单例取（不传参数）
    pub async fn execute(content: &str, channel_id: ArcStr) -> Result<Option<ArcStr>> {
        let parts: Vec<ArcStr> = content.trim().split_whitespace().map(|s| {ArcStr::from(s)}).collect();
        if parts.is_empty() {
            return Err(Error::InvalidCommand("空命令".into()));
        }
        let nexus = Nexus::get();
        match parts[0].as_str() {
            "/bind" => {
                if parts.len() < 4 || parts[1] != "messenger" {
                    return Err(Error::InvalidCommand(
                        "格式: /bind messenger <messenger_id> <user_id>".into()
                    ));
                }
                let cu = ChannelUser { messenger_id: parts[2].clone(), user_id: parts[3].clone() };
                // 统一走串行队列应用（防写-写竞态；bind_users 追加，HashSet 天然去重幂等）
                nexus.channel_manager().channel_command(ChannelCommand::BindUser { channel_id, user: cu }).await.map(Some)
            }
            "/unbind" => {
                if parts.len() < 4 || parts[1] != "messenger" {
                    return Err(Error::InvalidCommand(
                        "格式: /unbind messenger <messenger_id> <user_id>".into()
                    ));
                }
                let cu = ChannelUser { messenger_id: parts[2].clone(), user_id: parts[3].clone() };
                // 统一走串行队列应用（防写-写竞态；移除 bind_users；回复文本队列内生成）
                nexus.channel_manager().channel_command(ChannelCommand::UnbindUser { channel_id, user: cu }).await.map(Some)
            }
            "/admin" => {
                if parts.len() < 3 {
                    return Err(Error::InvalidCommand(
                        "格式: /admin <messenger_id> <user_id>".into()
                    ));
                }
                ConfigManager::get().add_admin(channel_id.as_str(), Arc::new(ChannelUser {
                    messenger_id: parts[1].clone(),
                    user_id: parts[2].clone(),
                })).await?;
                Ok(Some(ArcStr::from(format!("✅ 已添加管理权限: {} / {}", parts[1], parts[2]))))
            }
            "/unadmin" => {
                if parts.len() < 3 {
                    return Err(Error::InvalidCommand(
                        "格式: /unadmin <messenger_id> <user_id>".into()
                    ));
                }
                ConfigManager::get().remove_admin(channel_id.as_str(), &ChannelUser {
                    messenger_id: parts[1].clone(),
                    user_id: parts[2].clone()
                }).await?;
                Ok(Some(ArcStr::from(format!("✅ 已移除管理权限: {} / {}", parts[1], parts[2]))))
            }
            "/agent" => {
                // /agent [agent_id] [role]：缺省 agent_id 用保留 agent（"0"），缺省 role 用保留 role（空）
                let agent_id = parts.get(1);
                let role = parts.get(2);
                let new_agent_id = agent_id.filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| RESERVED_AGENT_ID);
                let new_role = role.cloned().unwrap_or_else(|| RESERVED_ROLE_NAME);
                // 校验 agent 存在（空/保留 id 直接通过）；role 非空时一并校验（agent+role 联合校验）
                nexus.verify_agent_exists(new_agent_id.clone()).await?;
                if !new_role.is_empty() {
                    nexus.verify_role_exists(new_agent_id.clone(), new_role.clone()).await?;
                }
                // None = 保持当前值（mode 保持当前运行态；Arc clone 浅拷贝入队）
                nexus.channel_manager().change_channel_key(channel_id, Some(new_agent_id.clone()), Some(new_role.clone()), None).await?;
                Ok(Some(ArcStr::from(format!("✅ 已设置 agent: {} / role: {}", new_agent_id, new_role))))
            }
            "/role" => {
                // /role [name]：缺省用保留 role（空）
                let role = parts.get(1);
                let new_role = role.cloned().unwrap_or_else(|| RESERVED_ROLE_NAME);
                // 经 channel_id 校验 role 存在（内部取当前 agent_id；空串保留 role 直接通过；channel 不存在报错）
                nexus.verify_role_exists_for_channel(channel_id.as_str(), new_role.clone()).await?;
                // None = 保持当前值（agent/mode 不变；Arc clone 浅拷贝入队）
                nexus.channel_manager().change_channel_key(channel_id, None, Some(new_role.clone()), None).await?;
                Ok(Some(ArcStr::from(format!("✅ 已设置 role: {}", new_role))))
            }
            "/mode" => {
                if parts.len() < 2 {
                    return Err(Error::InvalidCommand(
                        "格式: /mode event [event-id] 或 /mode role".into()
                    ));
                }
                match parts[1].as_str() {
                    "event" => {
                        // 缺省 event-id 生成新事件 ID；agent/role 保持当前值
                        let id = parts.get(2).cloned().unwrap_or_else(|| ArcStr::from(uuid::Uuid::new_v4().to_string()));
                        nexus.channel_manager().change_channel_key(channel_id, None, None, Some(Some(id.clone()))).await?;
                        Ok(Some(ArcStr::from(format!("✅ 新事件 ID: {}", id))))
                    }
                    "role" => {
                        // agent/role 保持当前值
                        nexus.channel_manager().change_channel_key(channel_id, None, None, Some(None)).await?;
                        Ok(Some(ArcStr::from("✅ 已切换为角色模式")))
                    }
                    _ => Err(Error::InvalidCommand(format!("未知模式: {}", parts[1]).into())),
                }
            }
            // 不是默认命令：交给来源 channel 的 pipeline 识别执行（/model、outgoing 等）
            _ => Ok(None),
        }
    }
}

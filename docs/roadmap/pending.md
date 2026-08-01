# 遗留事项

## 流程变更
1. [x] channel消息也改为由agent写入记忆
2. [x] 一个nexus管理不止一个会话上下文，每个channel都有绑定的agent_id、role_name、mode，nexus将所有活动channel的绑定去重，每个agent_id+role_name+mode为一个会话，每个会话对应的多个channel中，应该选择一个作为发送回复消息的channel
3. [ ] 重新设计会话和记忆的关系
4. [ ] 增加auto_bind

## 处理优化
1. [x] 去掉msg_type，直接使用Content枚举值的表示方法
2. [ ] memory record乱序时，不重写整个文件，而是回溯到非乱序的记录，仅重写乱序+新记录
3. [ ] 重构上下文长度控制以符合实际情况

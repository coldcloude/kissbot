# 遗留事项

## 流程变更
1. [x] channel消息也改为由agent写入记忆
2. [x] 一个nexus管理不止一个会话上下文，每个channel都有绑定的agent_id、role_name、mode，nexus将所有活动channel的绑定去重，每个agent_id+role_name+mode为一个会话，每个会话对应的多个channel中，应该选择一个作为发送回复消息的channel
3. [ ] 重新设计会话和记忆的关系
4. [ ] 增加auto_bind
5. [x] 移植ego.md到agent，删除channel中的memory_store_client（9681a6b 已完成，含 load_ego_info 改用 ego_md 三个 builder）

## 处理优化
1. [x] 去掉msg_type，直接使用Content枚举值的表示方法
2. [ ] memory record乱序时，不重写整个文件，而是回溯到非乱序的记录，仅重写乱序+新记录
3. [ ] 重构上下文长度控制以符合实际情况

## Station 系统（2026-08-16 嵌套化改造遗留）
1. [ ] MCP 真实实现（McpConfig 目前仅占位结构，mcps 查询无生产消费方）
2. [ ] 子 Station HTTP 协议实现（StationClient list_tools/list_mcps/call_tool 骨架已就位，调用返回未实现）
3. [ ] 跨进程工具名唯一性校验（本地硬约束已实现；跨进程由部署保证，查询时发现工具名冲突应报错）
4. [ ] 配置热更新监听接入（ConfigManager 已预留 add_listener/notify_listeners，Station/Nexus 尚未订阅）

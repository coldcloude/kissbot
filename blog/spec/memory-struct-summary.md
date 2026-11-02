# 记忆摘要实现

## 实现细节

- 为了和 agent 的 Event 模式区分，记忆摘要中的事件称为 case，用 case_name 作为标识
- 对应 agent 的 Role 和 Event 两种模式，Role 模式的 case_name 以“role-”作为前缀，Event 模式的 case_name 以“event-”作为前缀

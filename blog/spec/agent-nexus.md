# Nexus 实现

## 工作流程

- 默认记忆 Pipeline 读取 memory-store 中的记忆时，采用 最近时间 和 最后条数 取并集
- 记忆摘要 Pipeline 异步设计：
  - Channel 批次合并后发送到队列执行，同时解决并发和等待的问题
  - 一个批次划分为多个 case 后，每个 case 发送到队列执行，同时解决并发和等待的问题

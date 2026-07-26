# 记忆索引机制

> 对应设计文档：`docs/design/components-design/kissbot-memory.md`
> 记录的写入顺序与乱序处理见 [memory-store.md](memory-store.md)，索引的底层实现见 kai-rs/docs/kai-file.md

## 索引结构

- 四类记录（channel / think / tool-call / tool-result）各维护独立的索引上下文
- 每个 key 的索引包含两部分：
  - **时间索引**：分钟粒度的有序映射，记录每分钟覆盖的字节范围和行号范围，支持按时间范围快速定位
  - **页索引**：每千行记录起始字节位置，支持按行号定位

## 懒加载

- 收到变更通知时仅记录变更级别，查询时才按级别更新索引
- 变更级别分两种：
  - **追加级别**：从上次索引位置继续读取新增记录，补充索引
  - **重建级别**：重新构建整个索引（发生在文件全量重写后）
- 索引更新失败时恢复变更标记，下次查询重试

## 变更通知

- memory-store 写入记录后，按写入方式（追加 / 全量重写）向本进程内的 MemoryIndexer 发出对应级别的变更标记（见 [memory-store.md](memory-store.md)）
- 向 memory-struct 等外部模块推送变更通知：未实现

## 查询能力

- 按时间范围查询
- 查询结果携带文件内的全局行号

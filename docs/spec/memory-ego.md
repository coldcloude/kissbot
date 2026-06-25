# 自我认知模块

> 对应设计文档：`docs/design/components-design/kissbot-memory-ego.md`

## 读写方式

- 使用缓存降低 IO 开销
- 使用锁防止数据竞争

## 搜索索引

- 使用倒排索引实现全文搜索
- 脏标记机制延迟更新搜索索引
- 启动时自动加载所有 agent 到搜索索引


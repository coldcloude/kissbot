# 自我认知模块

> 对应设计文档：`docs/design/components-design/kissbot-memory-ego.md`

## 读写方式

- 使用缓存降低 IO 开销
- 使用锁防止数据竞争

## 搜索索引

- 使用倒排索引实现全文搜索
- 脏标记机制延迟更新搜索索引
- 启动时自动加载所有 agent 到搜索索引

## SearchManager 初始化时机原则

所有需要 mark_identity_dirty 或 mark_role_dirty 的方法，**必须在方法开头先调用 SearchManager::get().await?**，而不是在业务操作成功后才 get。

理由：如果业务操作成功（写文件、更新缓存），但后续 SearchManager::get() 因意外失败，方法会返回 Err，此时业务端的修改已经完成但调用方看到的是失败——导致状态不一致。提前 get 保证：要么 SearchManager 可用 + 业务全部成功 + mark 成功 = 整体成功，要么在业务开始前就提前 fail，不产生半成功状态。

这个原则适用于 AgentManager（update_agent_name、update_agent_description）和 RolePlayManager（create_role、create_role_from、remove_role、rename_role、update_role_description）中所有需要 SearchManager 标记脏数据的方法。

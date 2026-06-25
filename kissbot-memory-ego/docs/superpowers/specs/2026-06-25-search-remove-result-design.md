# search.rs 移除 Result 改造设计

## 改造成因

`memory-ego` 的 search 模块中，`force_sync_identity` 和 `force_sync_role` 在获取元数据失败时会传播错误，导致 `SearchManager::get()` 初始化也会失败。实际业务中，元数据缺失应该被视为"该对象已删除"而非错误，因此将这两个函数改为无返回值，内部容错。

## 改动点

### 1. `force_sync_identity` 无返回值 + 容错

**改动前**：`async fn force_sync_identity(&self, agent_id: &str) -> Result<()>`，`get_agent` 失败时直接 `?` 传播错误。

**改动后**：`async fn force_sync_identity(&self, agent_id: &str)`（无返回），`get_agent` 失败时走移除索引分支（从 `name_index`、`name_descr_index`、`name_completion`、`search_metadata` 中移除该 agent 的旧数据），与 `force_sync_role` 的删除行为一致。

### 2. `force_sync_role` 无返回值

去掉 `Result<()>` 返回类型和末尾的 `Ok(())`，内部逻辑不变（已正确处理 `get_role` 失败时移除索引）。

### 3. `sync_identity` / `sync_role` 无返回值

内部调用 `force_sync_identity`/`force_sync_role` 已无返回值，同步去掉 `Result<()>` 和 `Ok(())`。

### 4. `SearchManager::get()` 返回 `&'static Self`

**改动前**：`async fn get() -> Result<&'static Self>`，`list_agents` 失败时传播错误。

**改动后**：`async fn get() -> &'static Self`，`list_agents` 失败时返回空初始化的实例。内部对 `force_sync_identity`/`force_sync_role` 的调用也不再使用 `?`。

### 5. 各 Manager 中延迟 `SearchManager::get()`

`agent.rs` 和 `role_play.rs` 中，所有 `mark_identity_dirty`/`mark_role_dirty` 调用不再提前通过 `let search_manager = SearchManager::get().await?;` 获取实例，而是直接在调用处用 `SearchManager::get().await.mark_identity_dirty(...)` 一步到位。

### 6. API handler 简化

`api.rs` 中 8 个 search handler 不再需要 `match SearchManager::get().await { Ok(mgr) => ..., Err(e) => ... }`，直接获取后调用即可。

### 7. 测试简化

- 移除 `test_util.rs` 中的 `ensure_agent_metadata` 函数（原本用于防止 SearchManager 初始化因缺少 metadata 而失败）
- 移除 `agent.rs`、`role_play.rs`、`individual_recognition.rs` 测试中对 `ensure_agent_metadata` 的调用
- `search.rs` 测试中 `force_sync_identity(...).await.unwrap()` → `force_sync_identity(...).await`
- `search.rs` 测试中 `force_sync_role(...).await.unwrap()` → `force_sync_role(...).await`

## 影响范围

| 文件 | 改动类型 |
|------|---------|
| `search.rs` | 核心：4 个函数签名变更 + force_sync_identity 新增 else 分支 |
| `agent.rs` | 2 个函数中 SearchManager::get() 调用位置优化 |
| `role_play.rs` | 6 个函数中 SearchManager::get() 调用位置优化 |
| `api.rs` | 8 个 handler 移除 result 处理 |
| `test_util.rs` | 移除 `ensure_agent_metadata` 函数 |
| `agent.rs` 测试 | 移除 `ensure_agent_metadata` 调用 |
| `role_play.rs` 测试 | 移除 `ensure_agent_metadata` 调用 |
| `individual_recognition.rs` 测试 | 移除 `ensure_agent_metadata` 调用 |

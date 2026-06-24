# kissbot-memory-ego 单元测试实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 kissbot-memory-ego 的 5 个模块（config、agent、individual_recognition、role_play、search）添加 33 个单元测试

**Architecture:** 所有测试使用内联 `#[cfg(test)] mod tests`。共享初始化放在 `test_util.rs` 中通过 `Once::call_once` 完成。需要文件 IO 的测试用 `#[tokio::test]`，纯逻辑用 `#[test]`。

**Tech Stack:** Rust 2024, tokio, tempfile, kissbot_memory (DirectoryManager/Config), dashmap

---

### Task 0: 前置修改（前置条件，需先执行）

**前置条件已在 brainstorm 阶段完成，直接从 Task 1 开始。**

已完成的修改：
- `agent.rs`: `create_agent` 返回 `Result<Arc<String>>`，`copy_agent` 返回 `Result<Arc<String>>`
- `api.rs`: create_agent/copy_agent handler 匹配新的返回类型
- 已通过 `cargo check` 验证

---

### Task 1: 添加测试基础设施（Cargo.toml + test_util.rs + main.rs）

**Files:**
- Modify: `kissbot-memory-ego/Cargo.toml`
- Create: `kissbot-memory-ego/src/test_util.rs`
- Modify: `kissbot-memory-ego/src/main.rs`

- [ ] **Step 1: Cargo.toml 添加 tempfile 到 dev-dependencies**

```toml
# Cargo.toml 文件末尾
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: 新建 test_util.rs**

`kissbot-memory-ego/src/test_util.rs`：

```rust
use std::sync::Once;

/// 初始化 kissbot_memory::Config 使其指向一个临时目录。
/// 多次调用只生效一次。
/// TempDir 在闭包结束时 drop，但 Config::get() 已将 root_dir 读入 OnceLock 内存。
pub fn init_test_config() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let dir = tempfile::tempdir().expect("create tempdir");
        let config_path = dir.path().join("config.json");
        let root_dir_str = dir.path().display().to_string();
        std::fs::write(&config_path, format!(r#"{{"root_dir":"{}"}}"#, root_dir_str))
            .expect("write config");
        // SAFETY: 单线程测试环境，无并发 env 访问
        unsafe { std::env::set_var("KISSBOT_MEMORY_CONFIG", config_path.to_str().unwrap()); }
        kissbot_memory::Config::get();
    });
}
```

- [ ] **Step 3: main.rs 加条件编译引入**

在 `kissbot-memory-ego/src/main.rs` 开头（`mod error;` 下方）添加：

```rust
#[cfg(test)]
mod test_util;
```

- [ ] **Step 4: 编译验证**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego && cargo check
```

Expected: 编译成功，无 warning

- [ ] **Step 5: 提交**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego
git add Cargo.toml src/test_util.rs src/main.rs
git commit -m "test: 添加测试基础设施（tempfile、test_util.rs、条件编译引入）"
```

---

### Task 2: config.rs — 2 个测试

**Files:**
- Modify: `kissbot-memory-ego/src/config.rs`（追加 `#[cfg(test)] mod tests`）

- [ ] **Step 1: 在 config.rs 末尾添加测试模块**

在 `kissbot-memory-ego/src/config.rs` 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ego_config_with_values() {
        let config = Config {
            listen_addr: "0.0.0.0".to_string(),
            listen_port: 9999,
            api_key: "test-key".to_string(),
        };
        assert_eq!(config.listen_addr, "0.0.0.0");
        assert_eq!(config.listen_port, 9999);
        assert_eq!(config.api_key, "test-key");
    }

    #[test]
    fn test_ego_config_load() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("test-ego-config.json");
        std::fs::write(&config_path,
            r#"{"listen_addr":"127.0.0.1","listen_port":3001,"api_key":"abc123"}"#).unwrap();
        // SAFETY: 单线程测试
        unsafe { std::env::set_var("KISSBOT_MEMORY_EGO_CONFIG", config_path.to_str().unwrap()); }
        let config = Config::load().unwrap();
        assert_eq!(config.listen_addr, "127.0.0.1");
        assert_eq!(config.listen_port, 3001);
        assert_eq!(config.api_key, "abc123");
        unsafe { std::env::remove_var("KISSBOT_MEMORY_EGO_CONFIG"); }
    }
}
```

- [ ] **Step 2: 运行测试验证**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego && cargo test -- test_ego_config 2>&1
```

Expected: 2 passed

- [ ] **Step 3: 提交**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego
git add src/config.rs
git commit -m "test: config.rs 添加 2 个测试（构造 + load）"
```

---

### Task 3: agent.rs — 6 个测试

**Files:**
- Modify: `kissbot-memory-ego/src/agent.rs`（追加 `#[cfg(test)] mod tests`）

- [ ] **Step 1: 添加 import 和 test_create_agent**

在 `kissbot-memory-ego/src/agent.rs` 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_agent() {
        crate::test_util::init_test_config();
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("Alice".to_string()),
            Arc::new("Test agent".to_string()),
        ).await.unwrap();
        let agent = manager.get_agent(&agent_id).await.unwrap();
        assert_eq!(*agent.individual_name, "Alice");
        assert_eq!(*agent.description, "Test agent");
        assert_eq!(*agent.agent_id, *agent_id);
    }
}
```

- [ ] **Step 2: 添加 test_get_agent_not_found**

```rust
    #[tokio::test]
    async fn test_get_agent_not_found() {
        crate::test_util::init_test_config();
        let result = AgentManager::get().get_agent("nonexistent").await;
        assert!(matches!(result, Err(Error::AgentNotFound(_))));
    }
```

- [ ] **Step 3: 添加 test_update_agent_name**

```rust
    #[tokio::test]
    async fn test_update_agent_name() {
        crate::test_util::init_test_config();
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("Alice".to_string()),
            Arc::new("Original description".to_string()),
        ).await.unwrap();
        manager.update_agent_name(&agent_id, Arc::new("Alice2".to_string())).await.unwrap();
        let agent = manager.get_agent(&agent_id).await.unwrap();
        assert_eq!(*agent.individual_name, "Alice2");
        assert_eq!(*agent.description, "Original description");
    }
```

- [ ] **Step 4: 添加 test_update_agent_description**

```rust
    #[tokio::test]
    async fn test_update_agent_description() {
        crate::test_util::init_test_config();
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("Alice".to_string()),
            Arc::new("Original description".to_string()),
        ).await.unwrap();
        manager.update_agent_description(&agent_id, Arc::new("New description".to_string())).await.unwrap();
        let agent = manager.get_agent(&agent_id).await.unwrap();
        assert_eq!(*agent.description, "New description");
        assert_eq!(*agent.individual_name, "Alice");
    }
```

- [ ] **Step 5: 添加 test_copy_agent**

```rust
    #[tokio::test]
    async fn test_copy_agent() {
        crate::test_util::init_test_config();
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("Alice".to_string()),
            Arc::new("Test".to_string()),
        ).await.unwrap();
        let new_id = manager.copy_agent(&agent_id).await.unwrap();
        assert_ne!(*agent_id, *new_id);
        let original = manager.get_agent(&agent_id).await.unwrap();
        let copy = manager.get_agent(&new_id).await.unwrap();
        assert_eq!(*original.individual_name, *copy.individual_name);
    }
```

- [ ] **Step 6: 添加 test_crud_chain**

```rust
    #[tokio::test]
    async fn test_crud_chain() {
        crate::test_util::init_test_config();
        let manager = AgentManager::get();
        let agent_id = manager.create_agent(
            Arc::new("Alice".to_string()),
            Arc::new("Original".to_string()),
        ).await.unwrap();
        manager.update_agent_name(&agent_id, Arc::new("Alice2".to_string())).await.unwrap();
        manager.update_agent_description(&agent_id, Arc::new("Updated".to_string())).await.unwrap();
        let agent = manager.get_agent(&agent_id).await.unwrap();
        assert_eq!(*agent.individual_name, "Alice2");
        assert_eq!(*agent.description, "Updated");
    }
}
```

- [ ] **Step 7: 运行测试验证**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego && cargo test -- test_ 2>&1
```

Expected: 全部 tests passed（包含 Task 2 的 2 个 + Task 3 的 6 个 = 8 passed）

- [ ] **Step 8: 提交**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego
git add src/agent.rs
git commit -m "test: agent.rs 添加 6 个测试（create/get/update/copy/CRUD链/not_found）"
```

---

### Task 4: individual_recognition.rs — 7 个测试

**Files:**
- Modify: `kissbot-memory-ego/src/individual_recognition.rs`（追加 `#[cfg(test)] mod tests`）

- [ ] **Step 1: 添加 test_ego_individual_recognition_path**（纯函数）

在 `kissbot-memory-ego/src/individual_recognition.rs` 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ego_individual_recognition_path() {
        let path = ego_individual_recognition_path("/tmp/ego");
        assert_eq!(path, std::path::Path::new("/tmp/ego").join("individual-recognition-.json"));
    }
}
```

- [ ] **Step 2: 添加 test_get_individuals_new_agent**

```rust
    #[tokio::test]
    async fn test_get_individuals_new_agent() {
        crate::test_util::init_test_config();
        kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent1").await.unwrap();
        let result = IndividualRecognitionManager::get().get_individuals("agent1").await.unwrap();
        assert!(result.individual_map.is_empty());
    }
```

- [ ] **Step 3: 添加 test_get_individuals_not_found**

```rust
    #[tokio::test]
    async fn test_get_individuals_not_found() {
        crate::test_util::init_test_config();
        let result = IndividualRecognitionManager::get().get_individuals("nonexistent").await;
        assert!(matches!(result, Err(Error::AgentNotFound(_))));
    }
```

- [ ] **Step 4: 添加 test_replace_individuals_insert**

```rust
    #[tokio::test]
    async fn test_replace_individuals_insert() {
        crate::test_util::init_test_config();
        kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent1").await.unwrap();
        let manager = IndividualRecognitionManager::get();
        let individual = Arc::new(Individual {
            identifiers: Arc::new(dashmap::DashSet::new()),
            relation: Arc::new(IndividualRelation {
                relation: Arc::new("friend".to_string()),
                description: Arc::new("best friend".to_string()),
            }),
            other_relations: Arc::new(dashmap::DashMap::new()),
        });
        manager.replace_individuals(
            "agent1",
            vec![],
            vec![(Arc::new("Alice".to_string()), individual)],
        ).await.unwrap();
        let alice = manager.get_individual("agent1", "Alice").await.unwrap();
        assert_eq!(*alice.relation.relation, "friend");
    }
```

- [ ] **Step 5: 添加 test_replace_individuals_remove**

```rust
    #[tokio::test]
    async fn test_replace_individuals_remove() {
        crate::test_util::init_test_config();
        kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent1").await.unwrap();
        let manager = IndividualRecognitionManager::get();
        let individual = Arc::new(Individual {
            identifiers: Arc::new(dashmap::DashSet::new()),
            relation: Arc::new(IndividualRelation {
                relation: Arc::new("friend".to_string()),
                description: Arc::new("best friend".to_string()),
            }),
            other_relations: Arc::new(dashmap::DashMap::new()),
        });
        manager.replace_individuals(
            "agent1",
            vec![],
            vec![(Arc::new("Alice".to_string()), individual)],
        ).await.unwrap();
        manager.replace_individuals(
            "agent1",
            vec![Arc::new("Alice".to_string())],
            vec![],
        ).await.unwrap();
        let result = manager.get_individual("agent1", "Alice").await;
        assert!(matches!(result, Err(Error::AgentIndividualNotFound(_, _))));
    }
```

- [ ] **Step 6: 添加 test_rename_individual**

```rust
    #[tokio::test]
    async fn test_rename_individual() {
        crate::test_util::init_test_config();
        kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent1").await.unwrap();
        let manager = IndividualRecognitionManager::get();
        let individual = Arc::new(Individual {
            identifiers: Arc::new(dashmap::DashSet::new()),
            relation: Arc::new(IndividualRelation {
                relation: Arc::new("friend".to_string()),
                description: Arc::new("best friend".to_string()),
            }),
            other_relations: Arc::new(dashmap::DashMap::new()),
        });
        manager.replace_individuals(
            "agent1",
            vec![],
            vec![(Arc::new("Alice".to_string()), individual)],
        ).await.unwrap();
        manager.rename_individual("agent1", "Alice", "Bob").await.unwrap();
        let bob = manager.get_individual("agent1", "Bob").await.unwrap();
        assert_eq!(*bob.relation.relation, "friend");
        let result = manager.get_individual("agent1", "Alice").await;
        assert!(matches!(result, Err(Error::AgentIndividualNotFound(_, _))));
    }
```

- [ ] **Step 7: 添加 test_rename_individual_already_exists**

```rust
    #[tokio::test]
    async fn test_rename_individual_already_exists() {
        crate::test_util::init_test_config();
        kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent1").await.unwrap();
        let manager = IndividualRecognitionManager::get();
        let alice = Arc::new(Individual {
            identifiers: Arc::new(dashmap::DashSet::new()),
            relation: Arc::new(IndividualRelation {
                relation: Arc::new("friend".to_string()),
                description: Arc::new("".to_string()),
            }),
            other_relations: Arc::new(dashmap::DashMap::new()),
        });
        let bob = Arc::new(Individual {
            identifiers: Arc::new(dashmap::DashSet::new()),
            relation: Arc::new(IndividualRelation {
                relation: Arc::new("colleague".to_string()),
                description: Arc::new("".to_string()),
            }),
            other_relations: Arc::new(dashmap::DashMap::new()),
        });
        manager.replace_individuals(
            "agent1",
            vec![],
            vec![
                (Arc::new("Alice".to_string()), alice),
                (Arc::new("Bob".to_string()), bob),
            ],
        ).await.unwrap();
        let result = manager.rename_individual("agent1", "Alice", "Bob").await;
        assert!(matches!(result, Err(Error::AgentIndividualAlreadyExists(_, _))));
    }
}
```

注意：`Individual` `IndividualRelation` `IndividualRecognition` 类型来自 `kissbot_api` crate。`dashmap::DashSet`/`dashmap::DashMap` 已通过 `use kissbot_api::*` 引入，但 `DashSet`/`DashMap` 本身也在 `dashmap` crate 中。上面的测试代码使用 `dashmap::DashSet::new()` 和 `dashmap::DashMap::new()` 构造。如果 `individual_recognition.rs` 文件头没有 `use dashmap;`，则需要用 `use dashmap::{DashMap, DashSet};` 或者直接用 `kissbot_api::Individual { ... }` 结合 `Default`。

由于 `Individual` 的字段类型是 `Arc<DashSet<IndividualIdentifier>>` 等，需要在测试代码中正确 import。查看 `individual_recognition.rs` 的现有 `use` 语句，它没有引入 `dashmap`。需要在测试模块内 `use dashmap::{DashMap, DashSet};`。

- [ ] **Step 8: 运行测试验证**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego && cargo test -- test_ 2>&1
```

Expected: 全部 15 passed（config 2 + agent 6 + individual_recognition 7）

- [ ] **Step 9: 提交**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego
git add src/individual_recognition.rs
git commit -m "test: individual_recognition.rs 添加 7 个测试（路径函数 + CRUD + rename）"
```

---

### Task 5: role_play.rs — 11 个测试

**Files:**
- Modify: `kissbot-memory-ego/src/role_play.rs`（追加 `#[cfg(test)] mod tests`）

- [ ] **Step 1: 添加纯函数测试 + 基本 CRUD**

在 `kissbot-memory-ego/src/role_play.rs` 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ego_role_play_path() {
        let path = ego_role_play_path("/tmp/ego", "admin");
        assert_eq!(path, std::path::Path::new("/tmp/ego").join("role-play-admin.json"));
    }

    #[tokio::test]
    async fn test_create_role() {
        crate::test_util::init_test_config();
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent1").await.unwrap();
        dm.ensure_agent_ego_dir("agent1").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent1", Arc::new("admin".to_string()), Arc::new("Administrator".to_string())).await.unwrap();
        let role = manager.get_role("agent1", "admin").await.unwrap();
        assert_eq!(*role.role.role_name, "admin");
        assert_eq!(*role.role.description, "Administrator");
    }

    #[tokio::test]
    async fn test_create_role_duplicate() {
        crate::test_util::init_test_config();
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent1").await.unwrap();
        dm.ensure_agent_ego_dir("agent1").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent1", Arc::new("admin".to_string()), Arc::new("Desc".to_string())).await.unwrap();
        let result = manager.create_role("agent1", Arc::new("admin".to_string()), Arc::new("Other".to_string())).await;
        assert!(matches!(result, Err(Error::AgentRoleAlreadyExists(_, _))));
    }

    #[tokio::test]
    async fn test_create_role_from() {
        crate::test_util::init_test_config();
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent1").await.unwrap();
        dm.ensure_agent_ego_dir("agent1").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent1", Arc::new("admin".to_string()), Arc::new("Original desc".to_string())).await.unwrap();
        manager.create_role_from("agent1", "admin", Arc::new("mod".to_string())).await.unwrap();
        let new_role = manager.get_role("agent1", "mod").await.unwrap();
        assert_eq!(*new_role.role.description, "Original desc");
    }

    #[tokio::test]
    async fn test_list_roles() {
        crate::test_util::init_test_config();
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent1").await.unwrap();
        dm.ensure_agent_ego_dir("agent1").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent1", Arc::new("admin".to_string()), Arc::new("".to_string())).await.unwrap();
        manager.create_role("agent1", Arc::new("mod".to_string()), Arc::new("".to_string())).await.unwrap();
        let roles = manager.list_roles("agent1").await.unwrap();
        assert_eq!(roles.len(), 2);
        assert!(roles.contains(&"admin".to_string()));
        assert!(roles.contains(&"mod".to_string()));
    }

    #[tokio::test]
    async fn test_get_role_not_found() {
        crate::test_util::init_test_config();
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent1").await.unwrap();
        dm.ensure_agent_ego_dir("agent1").await.unwrap();
        let result = RolePlayManager::get().get_role("agent1", "nonexistent").await;
        assert!(matches!(result, Err(Error::AgentRoleNotFound(_, _))));
    }

    #[tokio::test]
    async fn test_remove_role() {
        crate::test_util::init_test_config();
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent1").await.unwrap();
        dm.ensure_agent_ego_dir("agent1").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent1", Arc::new("admin".to_string()), Arc::new("Desc".to_string())).await.unwrap();
        manager.remove_role("agent1", "admin").await.unwrap();
        let result = manager.get_role("agent1", "admin").await;
        assert!(matches!(result, Err(Error::AgentRoleNotFound(_, _))));
    }

    #[tokio::test]
    async fn test_rename_role() {
        crate::test_util::init_test_config();
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent1").await.unwrap();
        dm.ensure_agent_ego_dir("agent1").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent1", Arc::new("admin".to_string()), Arc::new("Desc".to_string())).await.unwrap();
        manager.rename_role("agent1", "admin", Arc::new("mod".to_string())).await.unwrap();
        let role = manager.get_role("agent1", "mod").await.unwrap();
        assert_eq!(*role.role.role_name, "mod");
        let result = manager.get_role("agent1", "admin").await;
        assert!(matches!(result, Err(Error::AgentRoleNotFound(_, _))));
    }

    #[tokio::test]
    async fn test_update_role_description() {
        crate::test_util::init_test_config();
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent1").await.unwrap();
        dm.ensure_agent_ego_dir("agent1").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent1", Arc::new("admin".to_string()), Arc::new("Old".to_string())).await.unwrap();
        manager.update_role_description("agent1", "admin", Arc::new("New desc".to_string())).await.unwrap();
        let role = manager.get_role("agent1", "admin").await.unwrap();
        assert_eq!(*role.role.description, "New desc");
    }
}
```

- [ ] **Step 2: 添加 other_role 相关测试**

```rust
    #[tokio::test]
    async fn test_other_role_replace() {
        crate::test_util::init_test_config();
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent1").await.unwrap();
        dm.ensure_agent_ego_dir("agent1").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent1", Arc::new("admin".to_string()), Arc::new("Desc".to_string())).await.unwrap();
        let other_role = Arc::new(OtherRole {
            individual_name: Arc::new("Bob".to_string()),
            role_relation: Arc::new(RoleRelation {
                relation: Arc::new("colleague".to_string()),
                description: Arc::new("Works together".to_string()),
            }),
            other_role_relations: Arc::new(dashmap::DashMap::new()),
            description: Arc::new("A colleague".to_string()),
        });
        manager.replace_other_roles(
            "agent1", "admin",
            vec![],
            vec![(Arc::new("Bob".to_string()), other_role)],
        ).await.unwrap();
        let result = manager.get_other_role("agent1", "admin", "Bob").await.unwrap();
        assert_eq!(*result.individual_name, "Bob");
    }

    #[tokio::test]
    async fn test_rename_other_role() {
        crate::test_util::init_test_config();
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent1").await.unwrap();
        dm.ensure_agent_ego_dir("agent1").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent1", Arc::new("admin".to_string()), Arc::new("Desc".to_string())).await.unwrap();
        let other_role = Arc::new(OtherRole {
            individual_name: Arc::new("Bob".to_string()),
            role_relation: Arc::new(RoleRelation {
                relation: Arc::new("colleague".to_string()),
                description: Arc::new("".to_string()),
            }),
            other_role_relations: Arc::new(dashmap::DashMap::new()),
            description: Arc::new("".to_string()),
        });
        manager.replace_other_roles(
            "agent1", "admin",
            vec![],
            vec![(Arc::new("Bob".to_string()), other_role)],
        ).await.unwrap();
        manager.rename_other_role("agent1", "admin", "Bob", "Robert").await.unwrap();
        let robert = manager.get_other_role("agent1", "admin", "Robert").await.unwrap();
        assert_eq!(*robert.individual_name, "Bob");
        let result = manager.get_other_role("agent1", "admin", "Bob").await;
        assert!(matches!(result, Err(Error::AgentRoleOtherRoleNotFound(_, _, _))));
    }

    #[tokio::test]
    async fn test_replace_other_role_relations() {
        crate::test_util::init_test_config();
        let dm = kissbot_memory::DirectoryManager::get();
        dm.ensure_agent_dir("agent1").await.unwrap();
        dm.ensure_agent_ego_dir("agent1").await.unwrap();
        let manager = RolePlayManager::get();
        manager.create_role("agent1", Arc::new("admin".to_string()), Arc::new("Desc".to_string())).await.unwrap();
        let other_role = Arc::new(OtherRole {
            individual_name: Arc::new("Bob".to_string()),
            role_relation: Arc::new(RoleRelation {
                relation: Arc::new("colleague".to_string()),
                description: Arc::new("".to_string()),
            }),
            other_role_relations: Arc::new(dashmap::DashMap::new()),
            description: Arc::new("".to_string()),
        });
        manager.replace_other_roles(
            "agent1", "admin",
            vec![],
            vec![(Arc::new("Bob".to_string()), other_role)],
        ).await.unwrap();
        let relation = Arc::new(RoleRelation {
            relation: Arc::new("friend".to_string()),
            description: Arc::new("close friend".to_string()),
        });
        manager.replace_other_role_relations(
            "agent1", "admin", "Bob",
            vec![],
            vec![(Arc::new("enemy".to_string()), relation)],
        ).await.unwrap();
        let bob = manager.get_other_role("agent1", "admin", "Bob").await.unwrap();
        let rel = bob.other_role_relations.get("enemy").unwrap();
        assert_eq!(*rel.relation, "friend");
    }
}
```

- [ ] **Step 3: 运行测试验证**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego && cargo test -- test_ 2>&1
```

Expected: 26 passed（config 2 + agent 6 + individual_recognition 7 + role_play 11）

- [ ] **Step 4: 提交**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego
git add src/role_play.rs
git commit -m "test: role_play.rs 添加 11 个测试（CRUD + other_role 操作）"
```

---

### Task 6: search.rs — 7 个测试

**Files:**
- Modify: `kissbot-memory-ego/src/search.rs`（追加 `#[cfg(test)] mod tests`）

SearchManager 的测试比较特殊：需要先通过 `DirectoryManager` 创建 agent 目录结构，写入 `metadata.json` 和 `role-play-*.json` 文件，再调 `SearchManager::get().await` 让它自动建索引。

- [ ] **Step 1: 添加辅助函数和 name 搜索测试**

在 `kissbot-memory-ego/src/search.rs` 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use kissbot_memory::DirectoryManager;

    async fn create_test_agent(agent_id: &str, name: &str, description: &str) {
        let dm = DirectoryManager::get();
        let agent_dir = dm.ensure_agent_dir(agent_id).await.unwrap();
        let metadata = serde_json::json!({
            "agent_id": agent_id,
            "individual_name": name,
            "description": description,
            "created_at": "2026-06-25 10:00:00"
        });
        tokio::fs::write(
            agent_dir.join("metadata.json"),
            serde_json::to_string_pretty(&metadata).unwrap(),
        ).await.unwrap();
    }

    async fn create_test_role(agent_id: &str, role_name: &str, description: &str) {
        let dm = DirectoryManager::get();
        let ego_dir = dm.ensure_agent_ego_dir(agent_id).await.unwrap();
        let role_play = serde_json::json!({
            "role": {
                "agent_id": agent_id,
                "role_name": role_name,
                "description": description
            },
            "other_roles": {}
        });
        let file_name = format!("role-play-{}.json", role_name);
        tokio::fs::write(
            ego_dir.join(&file_name),
            serde_json::to_string_pretty(&role_play).unwrap(),
        ).await.unwrap();
    }

    #[tokio::test]
    async fn test_search_by_name() {
        crate::test_util::init_test_config();
        create_test_agent("agent1", "Alice", "Test user").await;
        create_test_agent("agent2", "Bob", "Another user").await;
        let manager = SearchManager::get().await.unwrap();
        let results = manager.search_by_name("Alice").await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "agent1");
    }

    #[tokio::test]
    async fn test_search_by_name_no_match() {
        crate::test_util::init_test_config();
        create_test_agent("agent1", "Alice", "Test").await;
        let manager = SearchManager::get().await.unwrap();
        let results = manager.search_by_name("Nonexistent").await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_search_by_description() {
        crate::test_util::init_test_config();
        create_test_agent("agent1", "Alice", "Some searchable text here").await;
        let manager = SearchManager::get().await.unwrap();
        let results = manager.search_by_description("searchable").await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "agent1");
    }

    #[tokio::test]
    async fn test_agent_name_completion() {
        crate::test_util::init_test_config();
        create_test_agent("agent1", "Alice", "").await;
        create_test_agent("agent2", "Albert", "").await;
        create_test_agent("agent3", "Bob", "").await;
        let manager = SearchManager::get().await.unwrap();
        let results = manager.name_completion("Al").await;
        assert_eq!(results.len(), 2);
        let ids: Vec<&str> = results.iter().map(|r| r.key.as_str()).collect();
        assert!(ids.contains(&"agent1"));
        assert!(ids.contains(&"agent2"));
    }

    #[tokio::test]
    async fn test_search_role_by_name() {
        crate::test_util::init_test_config();
        create_test_agent("agent1", "Alice", "").await;
        create_test_role("agent1", "admin", "Administrator").await;
        let manager = SearchManager::get().await.unwrap();
        let results = manager.search_role_by_name("admin", None).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].role_name, "admin");
    }

    #[tokio::test]
    async fn test_search_role_by_description() {
        crate::test_util::init_test_config();
        create_test_agent("agent1", "Alice", "").await;
        create_test_role("agent1", "admin", "Special role description").await;
        let manager = SearchManager::get().await.unwrap();
        let results = manager.search_role_by_description("Special", None).await;
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_retrieve_agents() {
        crate::test_util::init_test_config();
        create_test_agent("agent1", "Alice", "Desc1").await;
        create_test_agent("agent2", "Bob", "Desc2").await;
        let manager = SearchManager::get().await.unwrap();
        let results = manager.retrieve_agents(vec![
            Arc::new("agent1".to_string()),
            Arc::new("agent2".to_string()),
        ]).await;
        assert_eq!(results.len(), 2);
        let names: Vec<&str> = results.iter().map(|a| a.individual_name.as_str()).collect();
        assert!(names.contains(&"Alice"));
        assert!(names.contains(&"Bob"));
    }
}
```

- [ ] **Step 2: 运行测试验证**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego && cargo test -- test_ 2>&1
```

Expected: 33 passed（config 2 + agent 6 + individual_recognition 7 + role_play 11 + search 7）

- [ ] **Step 3: 提交**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego
git add src/search.rs
git commit -m "test: search.rs 添加 7 个测试（name/desc 搜索 + completion + role 搜索 + retrieve）"
```

---

### Task 7: 最终编译与全部验证

- [ ] **Step 1: 完整编译检查**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego && cargo check 2>&1
```

Expected: 编译成功，无 warning

- [ ] **Step 2: 运行全部测试**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego && cargo test 2>&1
```

Expected: 33 passed, 0 failed

- [ ] **Step 3: 确认 git 状态**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego && git status
```

Expected: 干净的 working tree（所有修改已提交），或有未跟踪的 spec/plan 文件

- [ ] **Step 4: 提交 spec 和 plan 文档**

```bash
cd /home/admin/project/kissbot/kissbot-memory-ego
git add docs/
git commit -m "docs: 添加 memory-ego 单元测试 spec 和 plan 文档"
```

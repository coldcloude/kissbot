# kissbot-memory-ego 单元测试设计

## 概述

为 `kissbot-memory-ego` crate 添加单元测试，覆盖 config、agent、individual_recognition、role_play、search 五个模块。error.rs 和 ego_md.rs 不覆盖。

## 基本原则

- 所有测试使用 `#[cfg(test)] mod tests` 内联在每个 `.rs` 文件中
- 同步纯函数用 `#[test]`，需要文件 IO 和全局单例的用 `#[tokio::test]`
- 测试之间共享 `kissbot_memory::Config` 的单例（通过 `Once::call_once` 初始化一次）
- 使用 `tempfile` 创建临时配置文件用于 config load 测试
- 共享的测试初始化放在 `test_util.rs` 中

## 前置修改

### create_agent / copy_agent 返回类型

修改 `AgentManager::create_agent` 和 `AgentManager::copy_agent` 的返回类型从 `Result<()>` 改为 `Result<Arc<String>>`（返回 agent_id），方便测试直接拿到 agent_id 进行后续操作。

### Cargo.toml

在 `[dev-dependencies]` 中添加 `tempfile = "3"`。

## 初始化方案

### test_util.rs

新增 `kissbot-memory-ego/src/test_util.rs`，条件编译 `#[cfg(test)]`。在 `main.rs` 中用 `#[cfg(test)] mod test_util;` 引入。

```rust
use std::sync::Once;

/// 初始化 kissbot_memory::Config 使其指向一个临时目录。
/// 多次调用只生效一次，TempDir 在函数结束时会 drop（Config 已将 root_dir 读入内存）。
/// 后续测试可通过 DirectoryManager::get().ensure_agent_dir() 在 root_dir 路径下重建目录。
pub fn init_test_config() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let dir = tempfile::tempdir().expect("create tempdir");
        let config_path = dir.path().join("config.json");
        let root_dir_str = dir.path().display().to_string();
        std::fs::write(&config_path, format!(r#"{{"root_dir":"{}"}}"#, root_dir_str))
            .expect("write config");
        // SAFETY: 单线程测试环境
        unsafe { std::env::set_var("KISSBOT_MEMORY_CONFIG", config_path.to_str().unwrap()); }
        kissbot_memory::Config::get();
    });
}
```

注意：`TempDir` 在闭包结束时 drop，其目录被删除。但 `kissbot_memory::Config::get()` 已将 `root_dir`（PathBuf）读入 `OnceLock` 内存。后续测试中 `DirectoryManager::get().ensure_agent_dir()` 会在相同路径下重新创建目录。

### 各模块调用方式

每个需要文件 IO 的测试模块在第一个 `#[tokio::test]` 中（或通过共享的 setup）调用 `crate::test_util::init_test_config()`。`Once::call_once` 确保只会初始化一次。

## 测试分布

| 模块 | 测试数 | 说明 |
|------|--------|------|
| config.rs | 2 | 构造 + load |
| agent.rs | 6 | create/get/update/copy/not_found |
| individual_recognition.rs | 7 | 路径函数 + CRUD |
| role_play.rs | 11 | 路径函数 + CRUD + other_role 操作 |
| search.rs | 7 | name/desc 搜索 + completion + role 搜索 + retrieve |
| **合计** | **33** | |

---

## config.rs — 2 tests

### test_ego_config_with_values

直接构造 Config 结构体，验证三个字段。

```rust
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
```

### test_ego_config_load

写临时 JSON 配置文件，设环境变量，调 `Config::load()`，验证字段，清理环境变量。

```rust
#[test]
fn test_ego_config_load() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("test-ego-config.json");
    std::fs::write(&config_path,
        r#"{"listen_addr":"127.0.0.1","listen_port":3001,"api_key":"abc123"}"#).unwrap();
    unsafe { std::env::set_var("KISSBOT_MEMORY_EGO_CONFIG", config_path.to_str().unwrap()); }
    let config = Config::load().unwrap();
    assert_eq!(config.listen_addr, "127.0.0.1");
    assert_eq!(config.listen_port, 3001);
    assert_eq!(config.api_key, "abc123");
    unsafe { std::env::remove_var("KISSBOT_MEMORY_EGO_CONFIG"); }
}
```

---

## agent.rs — 6 tests

前置：每个测试开头调 `crate::test_util::init_test_config();`

### test_create_agent

验证 create_agent 返回 agent_id，且 get_agent 能取回正确的 metadata。

```rust
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
```

### test_get_agent_not_found

```rust
#[tokio::test]
async fn test_get_agent_not_found() {
    crate::test_util::init_test_config();
    let result = AgentManager::get().get_agent("nonexistent").await;
    assert!(matches!(result, Err(Error::AgentNotFound(_))));
}
```

### test_update_agent_name

```rust
#[tokio::test]
async fn test_update_agent_name() {
    crate::test_util::init_test_config();
    let manager = AgentManager::get();
    let agent_id = manager.create_agent(
        Arc::new("Alice".to_string()),
        Arc::new("Test".to_string()),
    ).await.unwrap();
    manager.update_agent_name(&agent_id, Arc::new("Alice2".to_string())).await.unwrap();
    let agent = manager.get_agent(&agent_id).await.unwrap();
    assert_eq!(*agent.individual_name, "Alice2");
    assert_eq!(*agent.description, "Test");
}
```

### test_update_agent_description

```rust
#[tokio::test]
async fn test_update_agent_description() {
    crate::test_util::init_test_config();
    let manager = AgentManager::get();
    let agent_id = manager.create_agent(
        Arc::new("Alice".to_string()),
        Arc::new("Old desc".to_string()),
    ).await.unwrap();
    manager.update_agent_description(&agent_id, Arc::new("New desc".to_string())).await.unwrap();
    let agent = manager.get_agent(&agent_id).await.unwrap();
    assert_eq!(*agent.description, "New desc");
    assert_eq!(*agent.individual_name, "Alice");
}
```

### test_copy_agent

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

### test_crud_chain

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
```

---

## individual_recognition.rs — 7 tests

前置：需要 IO 的测试调 `crate::test_util::init_test_config();`，然后 `DirectoryManager::get().ensure_agent_dir("agent1").await.unwrap();` 创建 agent 目录结构。

### test_ego_individual_recognition_path

纯函数同步测试。

```rust
#[test]
fn test_ego_individual_recognition_path() {
    let path = ego_individual_recognition_path("/tmp/ego");
    assert_eq!(path, std::path::Path::new("/tmp/ego").join("individual-recognition-.json"));
}
```

### test_get_individuals_new_agent

新 agent 可获取到空的 IndividualRecognition。

```rust
#[tokio::test]
async fn test_get_individuals_new_agent() {
    crate::test_util::init_test_config();
    kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent1").await.unwrap();
    let result = IndividualRecognitionManager::get().get_individuals("agent1").await.unwrap();
    assert!(result.individual_map.is_empty());
}
```

### test_get_individuals_not_found

```rust
#[tokio::test]
async fn test_get_individuals_not_found() {
    crate::test_util::init_test_config();
    let result = IndividualRecognitionManager::get().get_individuals("nonexistent").await;
    assert!(matches!(result, Err(Error::AgentNotFound(_))));
}
```

### test_replace_individuals_insert

```rust
#[tokio::test]
async fn test_replace_individuals_insert() {
    crate::test_util::init_test_config();
    kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent1").await.unwrap();
    let manager = IndividualRecognitionManager::get();
    let individual = Arc::new(Individual {
        identifiers: Arc::new(DashSet::new()),
        relation: Arc::new(IndividualRelation {
            relation: Arc::new("friend".to_string()),
            description: Arc::new("best friend".to_string()),
        }),
        other_relations: Arc::new(DashMap::new()),
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

### test_replace_individuals_remove

```rust
#[tokio::test]
async fn test_replace_individuals_remove() {
    crate::test_util::init_test_config();
    kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent1").await.unwrap();
    let manager = IndividualRecognitionManager::get();
    let individual = Arc::new(Individual {
        identifiers: Arc::new(DashSet::new()),
        relation: Arc::new(IndividualRelation {
            relation: Arc::new("friend".to_string()),
            description: Arc::new("best friend".to_string()),
        }),
        other_relations: Arc::new(DashMap::new()),
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

### test_rename_individual

```rust
#[tokio::test]
async fn test_rename_individual() {
    crate::test_util::init_test_config();
    kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent1").await.unwrap();
    let manager = IndividualRecognitionManager::get();
    let individual = Arc::new(Individual {
        identifiers: Arc::new(DashSet::new()),
        relation: Arc::new(IndividualRelation {
            relation: Arc::new("friend".to_string()),
            description: Arc::new("best friend".to_string()),
        }),
        other_relations: Arc::new(DashMap::new()),
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

### test_rename_individual_already_exists

```rust
#[tokio::test]
async fn test_rename_individual_already_exists() {
    crate::test_util::init_test_config();
    kissbot_memory::DirectoryManager::get().ensure_agent_dir("agent1").await.unwrap();
    let manager = IndividualRecognitionManager::get();
    let alice = Arc::new(Individual {
        identifiers: Arc::new(DashSet::new()),
        relation: Arc::new(IndividualRelation {
            relation: Arc::new("friend".to_string()),
            description: Arc::new("".to_string()),
        }),
        other_relations: Arc::new(DashMap::new()),
    });
    let bob = Arc::new(Individual {
        identifiers: Arc::new(DashSet::new()),
        relation: Arc::new(IndividualRelation {
            relation: Arc::new("colleague".to_string()),
            description: Arc::new("".to_string()),
        }),
        other_relations: Arc::new(DashMap::new()),
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
```

---

## role_play.rs — 11 tests

前置：需要 IO 的测试调 `crate::test_util::init_test_config();`，然后 `DirectoryManager::get().ensure_agent_dir("agent1").await.unwrap()` + `ensure_agent_ego_dir("agent1").await.unwrap()`。

### test_ego_role_play_path

```rust
#[test]
fn test_ego_role_play_path() {
    let path = ego_role_play_path("/tmp/ego", "admin");
    assert_eq!(path, std::path::Path::new("/tmp/ego").join("role-play-admin.json"));
}
```

### test_create_role

```rust
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
```

### test_create_role_duplicate

```rust
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
```

### test_create_role_from

```rust
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
```

### test_list_roles

```rust
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
```

### test_get_role_not_found

```rust
#[tokio::test]
async fn test_get_role_not_found() {
    crate::test_util::init_test_config();
    let dm = kissbot_memory::DirectoryManager::get();
    dm.ensure_agent_dir("agent1").await.unwrap();
    dm.ensure_agent_ego_dir("agent1").await.unwrap();
    let result = RolePlayManager::get().get_role("agent1", "nonexistent").await;
    assert!(matches!(result, Err(Error::AgentRoleNotFound(_, _))));
}
```

### test_remove_role

```rust
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
```

### test_rename_role

```rust
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
```

### test_update_role_description

```rust
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
```

### test_other_role_replace

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
        other_role_relations: Arc::new(DashMap::new()),
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
```

### test_rename_other_role

```rust
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
        other_role_relations: Arc::new(DashMap::new()),
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
```

### test_replace_other_role_relations

```rust
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
        other_role_relations: Arc::new(DashMap::new()),
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
```

---

## search.rs — 7 tests

前置：需要 IO 的测试调 `crate::test_util::init_test_config();`，然后手动创建 agent 目录结构并写 metadata.json 和 role 文件。

辅助函数：

```rust
async fn create_test_agent(agent_id: &str, name: &str, description: &str) {
    let dm = kissbot_memory::DirectoryManager::get();
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
    let dm = kissbot_memory::DirectoryManager::get();
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
```

### test_search_by_name

```rust
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
```

### test_search_by_name_no_match

```rust
#[tokio::test]
async fn test_search_by_name_no_match() {
    crate::test_util::init_test_config();
    create_test_agent("agent1", "Alice", "Test").await;
    let manager = SearchManager::get().await.unwrap();
    let results = manager.search_by_name("Nonexistent").await;
    assert!(results.is_empty());
}
```

### test_search_by_description

```rust
#[tokio::test]
async fn test_search_by_description() {
    crate::test_util::init_test_config();
    create_test_agent("agent1", "Alice", "Some searchable text here").await;
    let manager = SearchManager::get().await.unwrap();
    let results = manager.search_by_description("searchable").await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], "agent1");
}
```

### test_agent_name_completion

```rust
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
```

### test_search_role_by_name

```rust
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
```

### test_search_role_by_description

```rust
#[tokio::test]
async fn test_search_role_by_description() {
    crate::test_util::init_test_config();
    create_test_agent("agent1", "Alice", "").await;
    create_test_role("agent1", "admin", "Special role description").await;
    let manager = SearchManager::get().await.unwrap();
    let results = manager.search_role_by_description("Special", None).await;
    assert_eq!(results.len(), 1);
}
```

### test_retrieve_agents

```rust
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
```

---

## test_util.rs

完整实现：

```rust
use std::sync::Once;

/// 初始化 kissbot_memory::Config 使其指向一个临时目录。
/// 多次调用只生效一次。
/// TempDir 在闭包结束时 drop，但 kissbot_memory::Config::get() 已将 root_dir 读入 OnceLock 内存。
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

## 注意事项

1. **`kissbot-memory-ego` 没有 lib.rs**，`test_util.rs` 通过 `main.rs` 中的 `#[cfg(test)] mod test_util;` 引入。
2. 所有测试模块通过 `crate::test_util::init_test_config()` 调用初始化。
3. `SearchManager::get().await` 必须在 agent 目录和 metadata.json 写好后调用，以确保初始化时能扫描到所有数据。
4. `update_agent_name` 和 `update_agent_description` 内部调用 `SearchManager::get().await`，因此测试中第一次 update 时会触发 SearchManager 的初始化。
5. `create_role`、`remove_role` 等也调用了 `SearchManager::get().await`，同理。
6. 由于 `Once::call_once` 确保初始化仅一次，且 `TempDir` 在初始化结束后 drop，所有测试共享同一个 `kissbot_memory::Config` 中的 root_dir 路径。测试需要在该路径下通过 `ensure_agent_dir` 重建目录结构。

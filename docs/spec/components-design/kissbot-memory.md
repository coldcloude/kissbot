# kissbot-memory 组件设计

## 概述
记忆基础模块，定义记忆系统的文件存储目录结构，提供目录管理和索引查询功能，以及路径构造接口，作为程序库供其他记忆模块（记忆存储模块、记忆结构模块、自我认知模块）使用。

记忆系统支持两种组织模式——角色记忆和事件记忆。两种模式共用同一套存储和索引机制，区别仅在于目录名的构造方式。推送方和查询方拼接 `{role-name}` 或 `{role-name}-{event-id}` 作为后缀传给记忆基础模块，由本模块构造完整路径。

## 内部模块

### 1. DirectoryManager - 目录管理器
- 管理记忆系统根目录
- 创建和管理 agent 目录及其子目录（memory-ego、memory-store、memory-struct-*）
- 通过检查 agent 标识文件判断目录有效性
- 提供 list_agents 查询所有有效 agent
- 目录自动创建

### 2. MemoryIndexer - 记忆索引器
- 为记忆记录构建和维护索引（记录在文件中的位置）
- 支持索引过期处理：小过期（新记录追加）、大过期（文件重写）
- 支持按时间范围快速查询
- 支持自动索引重建

### 3. PathBuilder - 路径构造器
- 提供路径构造接口：接收 `(agent-id, year, suffix)`，返回完整路径
- 角色记忆：`{agent-id}/memory-store/{year}-{role-name}/`
- 事件记忆：`{agent-id}/memory-store/{year}-{role-name}-{event-id}/`
- `suffix` 由调用方（nexus、memory-struct）拼接，本模块不做解析
- 提供 `build_store_path(agent_id, year, suffix)` 和 `build_struct_path(agent_id, year, suffix, struct_type)` 等方法

## 目录结构

### 角色记忆模式
```
{记忆系统根目录}/
├── {agent-id}/
│   ├── agent-{agent-id}          # agent 存在标识文件
│   ├── metadata.json             # agent 元数据
│   ├── memory-ego/               # 自我认知数据
│   ├── memory-store/
│   │   └── {year}-{role-name}/
│   │       ├── channel-{channel_id}-records-{date}.jsonl
│   │       ├── think-records-{date}.jsonl
│   │       ├── tool-call-records-{date}.jsonl
│   │       └── tool-result-records-{date}.jsonl
│   ├── memory-struct-*/          # 各记忆结构实现的数据
│   └── ...
```

### 事件记忆模式
```
{记忆系统根目录}/
├── {agent-id}/
│   ├── agent-{agent-id}
│   ├── metadata.json
│   ├── memory-ego/
│   ├── memory-store/
│   │   └── {year}-{role-name}-{event-id}/
│   │       ├── channel-{channel_id}-records-{date}.jsonl
│   │       ├── think-records-{date}.jsonl
│   │       ├── tool-call-records-{date}.jsonl
│   │       └── tool-result-records-{date}.jsonl
│   └── memory-struct-*/
```

## 对外接口
以库形式提供：
- `DirectoryManager` → 目录管理单例
- `MemoryIndexer` → 索引器单例
- `PathBuilder` → 路径构造器，提供 `build_store_path()` 等方法
- 路径常量集合

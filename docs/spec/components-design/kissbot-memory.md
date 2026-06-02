# kissbot-memory 组件设计

## 概述
记忆基础模块，定义记忆系统的文件存储目录结构，提供目录管理和索引查询功能，作为程序库供其他记忆模块（记忆存储模块、记忆结构模块、自我认知模块）使用。

## 内部模块

### 1. DirectoryManager - 目录管理器
- 管理记忆系统根目录
- 创建和管理 agent ID 目录及其子目录（memory-ego、memory-store、memory-struct-*）
- 通过检查 agent-{agent-id} 标识文件判断目录有效性
- 提供 list_agents 查询所有有效 agent
- 目录自动创建

### 2. MemoryIndexer - 记忆索引器
- 为记忆记录构建和维护索引（记录在文件中的位置）
- 支持索引过期处理：小过期（新记录追加）、大过期（文件重写）
- 支持按时间范围快速查询
- 支持自动索引重建

## 目录结构
```
{记忆系统根目录}/
├── {agent-id}/
│   ├── agent-{agent-id}          # agent 存在标识文件
│   ├── metadata.json             # agent 元数据
│   ├── memory-ego/               # 自我认知数据
│   ├── memory-store/             # 原始记忆数据
│   └── memory-struct-*/          # 各记忆结构实现的数据
└── ...
```

## 对外接口
以库形式提供：
- `DirectoryManager` → 目录管理单例
- `MemoryIndexer` → 索引器单例
- 路径常量集合

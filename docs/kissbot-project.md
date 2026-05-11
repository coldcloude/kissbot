# kissbot-project 模块设计

## 模块概述
工程模块，作为Rust库提供给其他模块使用，负责管理一个工程中的各个职位，并为工程模式的agent提供各种tool。

## 职责
- 管理工程中的职位配置
- 读取和写入工作区内的职位配置文件
- 为工程模式的agent提供职位设定
- 提供agent必须遵守的规范和禁止事项
- 从工作区内加载自定义指导文件（如AGENTS.md）
- 为工程模式的agent提供tool集合：
  - 文件操作类tool（Read、Write、Edit）
  - 命令执行类tool（Bash）
  - 扩展类tool（Skill）
- 管理工程笔记的读写

## 架构设计

```
kissbot-project (Rust Lib)
├── ProjectManager          # 工程管理器
├── RoleManager             # 职位管理器
├── ToolProvider            # Tool提供者
├── NoteManager             # 笔记管理器
├── GuideLoader             # 指导文件加载器
└── types/                  # 数据类型定义
    ├── ProjectConfig       # 工程配置
    ├── RoleSetting         # 职位设定
    ├── ToolDefinition      # Tool定义
    └── Note                # 笔记
```

## 工作区目录结构

工程绑定的本地工作区目录结构：

```
{workspace-path}/
├── .kissbot/               # kissbot工程配置目录
│   ├── project.json        # 工程配置文件
│   ├── roles/              # 职位配置目录
│   │   ├── developer.json  # 开发者职位配置
│   │   ├── designer.json   # 设计师职位配置
│   │   └── ...
│   ├── notes/              # 笔记目录
│   │   ├── developer/      # 开发者笔记
│   │   ├── designer/       # 设计师笔记
│   │   └── ...
│   └── tools/              # 自定义tool配置（可选）
├── AGENTS.md               # 自定义指导文件
└── (工程实际文件)
```

## 核心组件设计

### 1. ProjectManager - 工程管理器
- 管理工程的整体配置
- 初始化工程目录结构
- 读取和写入project.json配置文件
- 协调其他组件的工作

#### ProjectConfig - 工程配置
```rust
struct ProjectConfig {
    project_name: String,
    project_description: String,
    created_at: String,
    updated_at: String,
    basic_principles: String,           // 基本原则和禁止事项
    available_roles: Vec<String>,       // 可用职位列表
    current_role: Option<String>,       // 当前选中的职位
}
```

### 2. RoleManager - 职位管理器
- 管理工程中的各个职位
- 读取和写入职位配置文件
- 提供职位列表
- 切换当前职位
- 为agent提供职位设定

#### RoleSetting - 职位设定
```rust
struct RoleSetting {
    role_name: String,
    role_description: String,
    system_prompt_addition: String,    // 系统提示词附加内容
    responsibilities: Vec<String>,     // 职责列表
    skills: Vec<String>,               // 技能列表
    note_path: String,                 // 该职位的笔记路径
    tools: Vec<ToolConfig>,            // 该职位可用的tool配置
    created_at: String,
    updated_at: String,
}
```

#### ToolConfig - Tool配置
```rust
struct ToolConfig {
    tool_name: String,
    enabled: bool,
    config: serde_json::Value,         // Tool特定配置
}
```

### 3. ToolProvider - Tool提供者
- 为工程模式的agent提供tool集合
- 根据职位配置启用/禁用tool
- 提供标准tool：
  - Read：读取文件
  - Write：写入文件
  - Edit：编辑文件
  - Bash：执行命令
  - Skill：扩展skill
- 支持自定义tool

#### 标准Tool定义

##### Read Tool
```rust
struct ReadToolInput {
    file_path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

struct ReadToolOutput {
    content: String,
    file_path: String,
    size_bytes: usize,
}
```

##### Write Tool
```rust
struct WriteToolInput {
    file_path: String,
    content: String,
    overwrite: bool,
}

struct WriteToolOutput {
    file_path: String,
    success: bool,
    message: String,
}
```

##### Edit Tool
```rust
struct EditToolInput {
    file_path: String,
    old_string: String,
    new_string: String,
    replace_all: bool,
}

struct EditToolOutput {
    file_path: String,
    success: bool,
    replacements_made: usize,
    message: String,
}
```

##### Bash Tool
```rust
struct BashToolInput {
    command: String,
    args: Vec<String>,
    working_dir: Option<String>,
    timeout_seconds: Option<u64>,
}

struct BashToolOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
}
```

##### Skill Tool
```rust
struct SkillToolInput {
    skill_name: String,
    params: serde_json::Value,
}

struct SkillToolOutput {
    result: serde_json::Value,
    success: bool,
    message: String,
}
```

### 4. NoteManager - 笔记管理器
- 管理各个职位的笔记
- 读取笔记
- 写入笔记
- 按职位组织笔记

#### Note - 笔记
```rust
struct Note {
    note_id: String,
    title: String,
    content: String,
    tags: Vec<String>,
    created_at: String,
    updated_at: String,
}
```

### 5. GuideLoader - 指导文件加载器
- 从工作区内加载自定义指导文件
- 支持AGENTS.md等格式
- 提取指导内容用于系统提示词

## 工程配置文件格式

### .kissbot/project.json
```json
{
    "project_name": "My Project",
    "project_description": "A sample project",
    "created_at": "2024-01-01 12:00:00",
    "updated_at": "2024-01-01 12:00:00",
    "basic_principles": "1. 不要删除重要文件\n2. 提交前先测试\n3. ...",
    "available_roles": ["developer", "designer"],
    "current_role": "developer"
}
```

### .kissbot/roles/{role-name}.json
```json
{
    "role_name": "developer",
    "role_description": "负责代码开发和维护",
    "system_prompt_addition": "你是一个专业的开发者，注重代码质量和可维护性...",
    "responsibilities": [
        "编写高质量代码",
        "进行代码审查",
        "编写单元测试"
    ],
    "skills": ["Rust", "TypeScript", "Git"],
    "note_path": ".kissbot/notes/developer",
    "tools": [
        {
            "tool_name": "Read",
            "enabled": true,
            "config": {}
        },
        {
            "tool_name": "Write",
            "enabled": true,
            "config": {}
        },
        {
            "tool_name": "Edit",
            "enabled": true,
            "config": {}
        },
        {
            "tool_name": "Bash",
            "enabled": true,
            "config": {
                "allowed_commands": ["cargo", "npm", "git"]
            }
        }
    ],
    "created_at": "2024-01-01 12:00:00",
    "updated_at": "2024-01-01 12:00:00"
}
```

## 公共API设计

### ProjectManager API

#### 初始化工程
```rust
impl ProjectManager {
    /// 在指定路径初始化新工程
    pub async fn init_project(
        workspace_path: &str,
        project_name: &str,
        project_description: &str,
    ) -> Result<Self, ProjectError>;

    /// 加载现有工程
    pub async fn load_project(workspace_path: &str) -> Result<Self, ProjectError>;

    /// 获取工程配置
    pub fn get_config(&self) -> &ProjectConfig;

    /// 更新工程配置
    pub async fn update_config(&mut self, config: ProjectConfig) -> Result<(), ProjectError>;
}
```

### RoleManager API

```rust
impl RoleManager {
    /// 获取所有可用职位列表
    pub fn get_available_roles(&self) -> Vec<String>;

    /// 获取职位设定
    pub async fn get_role_setting(&self, role_name: &str) -> Result<RoleSetting, ProjectError>;

    /// 创建新职位
    pub async fn create_role(&mut self, setting: RoleSetting) -> Result<(), ProjectError>;

    /// 更新职位设定
    pub async fn update_role(&mut self, setting: RoleSetting) -> Result<(), ProjectError>;

    /// 删除职位
    pub async fn delete_role(&mut self, role_name: &str) -> Result<(), ProjectError>;

    /// 切换当前职位
    pub async fn switch_role(&mut self, role_name: &str) -> Result<(), ProjectError>;

    /// 获取当前职位设定
    pub fn get_current_role(&self) -> Option<&RoleSetting>;
}
```

### ToolProvider API

```rust
impl ToolProvider {
    /// 获取当前职位可用的tool列表
    pub fn get_available_tools(&self) -> Vec<ToolDefinition>;

    /// 执行Read tool
    pub async fn read_file(&self, input: ReadToolInput) -> Result<ReadToolOutput, ToolError>;

    /// 执行Write tool
    pub async fn write_file(&self, input: WriteToolInput) -> Result<WriteToolOutput, ToolError>;

    /// 执行Edit tool
    pub async fn edit_file(&self, input: EditToolInput) -> Result<EditToolOutput, ToolError>;

    /// 执行Bash tool
    pub async fn execute_bash(&self, input: BashToolInput) -> Result<BashToolOutput, ToolError>;

    /// 执行Skill tool
    pub async fn execute_skill(&self, input: SkillToolInput) -> Result<SkillToolOutput, ToolError>;
}
```

### NoteManager API

```rust
impl NoteManager {
    /// 获取当前职位的所有笔记
    pub async fn list_notes(&self, role_name: &str) -> Result<Vec<Note>, NoteError>;

    /// 获取单个笔记
    pub async fn get_note(&self, role_name: &str, note_id: &str) -> Result<Note, NoteError>;

    /// 创建笔记
    pub async fn create_note(&mut self, role_name: &str, note: Note) -> Result<(), NoteError>;

    /// 更新笔记
    pub async fn update_note(&mut self, role_name: &str, note: Note) -> Result<(), NoteError>;

    /// 删除笔记
    pub async fn delete_note(&mut self, role_name: &str, note_id: &str) -> Result<(), NoteError>;
}
```

### GuideLoader API

```rust
impl GuideLoader {
    /// 加载AGENTS.md指导文件
    pub async fn load_agents_md(&self) -> Result<Option<String>, GuideError>;

    /// 加载指定路径的指导文件
    pub async fn load_guide_file(&self, file_path: &str) -> Result<String, GuideError>;
}
```

## 错误类型定义

```rust
enum ProjectError {
    IoError(std::io::Error),
    SerializeError(serde_json::Error),
    WorkspaceNotFound,
    InvalidConfig,
    RoleNotFound,
    RoleAlreadyExists,
}

enum ToolError {
    FileNotFound,
    PermissionDenied,
    CommandExecutionFailed,
    Timeout,
    InvalidInput,
}

enum NoteError {
    NoteNotFound,
    NoteAlreadyExists,
    InvalidNote,
}

enum GuideError {
    FileNotFound,
    InvalidFormat,
}
```

## 实现决策

- 使用tokio作为异步运行时
- 使用serde进行JSON序列化
- 使用tokio::fs进行文件操作
- 使用std::sync::Arc共享配置
- 路径处理考虑跨平台兼容性（Windows/Unix）
- 提供 Builder 模式用于创建配置
- 所有错误统一使用thiserror定义

## 使用示例

### 初始化新工程
```rust
let project_manager = ProjectManager::init_project(
    "/path/to/workspace",
    "My Project",
    "A sample project description"
).await?;
```

### 加载现有工程
```rust
let mut project_manager = ProjectManager::load_project("/path/to/workspace").await?;
```

### 创建职位
```rust
let role_setting = RoleSetting {
    role_name: "developer".to_string(),
    role_description: "负责代码开发".to_string(),
    system_prompt_addition: "你是一个专业的开发者...".to_string(),
    responsibilities: vec!["编写代码".to_string()],
    skills: vec!["Rust".to_string()],
    note_path: ".kissbot/notes/developer".to_string(),
    tools: vec![],
    created_at: "2024-01-01 12:00:00".to_string(),
    updated_at: "2024-01-01 12:00:00".to_string(),
};

project_manager.role_manager.create_role(role_setting).await?;
```

### 切换职位并获取系统提示词
```rust
project_manager.role_manager.switch_role("developer").await?;
let system_prompt = project_manager.build_system_prompt().await?;
```

### 使用Tool
```rust
let tool_provider = project_manager.get_tool_provider();

// 读取文件
let read_output = tool_provider.read_file(ReadToolInput {
    file_path: "src/main.rs".to_string(),
    offset: None,
    limit: None,
}).await?;

// 执行命令
let bash_output = tool_provider.execute_bash(BashToolInput {
    command: "cargo".to_string(),
    args: vec!["build".to_string()],
    working_dir: None,
    timeout_seconds: Some(300),
}).await?;
```

## 开发计划（仅设计，暂不实现）

### 第1阶段：基础结构
- 配置Cargo.toml，添加依赖
- 定义模块结构
- 定义错误类型
- 定义核心数据结构

### 第2阶段：工程管理器
- 实现ProjectManager
- 实现工程初始化
- 实现配置文件读写

### 第3阶段：职位管理器
- 实现RoleManager
- 实现职位配置文件读写
- 实现职位切换

### 第4阶段：Tool提供者
- 实现ToolProvider
- 实现Read/Write/Edit tool
- 实现Bash tool
- 实现Skill tool框架

### 第5阶段：笔记和指导文件
- 实现NoteManager
- 实现GuideLoader
- 实现系统提示词构建

### 第6阶段：测试和完善
- 单元测试
- 集成测试
- 文档完善

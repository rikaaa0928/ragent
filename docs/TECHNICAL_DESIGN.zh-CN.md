# ragent 详细技术方案

状态：Draft

版本：0.1

日期：2026-08-31

关联文档：

- [产品需求文档](./PRODUCT_REQUIREMENTS.zh-CN.md)
- [架构改造实施流程](./ARCHITECTURE_REWRITE_ROADMAP.zh-CN.md)

## 1. 方案摘要

ragent 采用轻量、本地优先的控制面架构：

```text
CLI / TUI / WebUI
        │
        │ Control Plane Protocol
        ▼
┌─────────────────────────────────────────────┐
│ ragentd                                     │
│                                             │
│  Command Router ── Session Controller       │
│        │                  │                 │
│        │                  ▼                 │
│        │            Session Runner(s)       │
│        │                  │                 │
│        │                  ▼                 │
│        │             Agent Core             │
│        │        Model I/O + Loop + Hooks    │
│        │                                    │
│        └────────── Store Writer ─────────┐  │
└──────────────────────────────────────────│──┘
                                           ▼
                                  Directory Control Store
                                           │
                                           └── Shared Workspace(s)
```

Session 是持久化的最小工作台，Agent Runner 是可随时重建的执行器。所有模型上下文直接保存为 Open Responses `Item`，按不可变 Item Batch 原子追加。Session 的来源在创建时冻结为固定 Context Slice，从而支持零复制 fork、压缩和多来源派生。

第一版只允许一个 ragentd 进程写 Directory Store。不同 Session 可以并行执行，但所有事实提交经过单个 Store Writer 串行落盘。

## 2. 目标与非目标

### 2.1 技术目标

- 使用普通目录实现可检查、可恢复、只追加的事实存储。
- 核心上下文路径只使用 Open Responses 原生类型。
- 保证 Item Batch 的原子提交。
- 保证已提交上下文不可修改。
- 将 Session 和 Agent 进程生命周期解耦。
- 支持异步 Activation、取消、观察和恢复。
- 支持 Session DAG 和固定范围的零复制派生。
- 为 CLI、TUI、WebUI 和第三方前端提供统一协议。
- 为 WASM Component Extension 提供稳定的生命周期和 Hook 边界。
- 保持实现模块少、依赖少、数据结构直接。

### 2.2 非目标

- 不兼容或迁移任何已有代码和 Session 数据。
- 不实现分布式共识、多机调度或高可用 Store。
- 不允许多个进程直接写 Store。
- 不实现 Session Exchange、消息队列或共享消费游标。
- 不追踪共享目录文件交互。
- 不实现模型 Token streaming。
- 不提供通用工作流 DSL。
- 不在核心中捆绑业务工具。
- 不以数据库为第一版默认存储。

## 3. 架构原则与硬性不变量

以下规则由类型、Store API 和恢复逻辑共同强制，不能只依靠调用约定。

### INV-001 Session Spec 不可变

Session 创建成功后，`spec.json` 永不覆盖。需要改变 System Prompt、Source 顺序或身份属性时创建新 Session。

### INV-002 Context 只追加

核心只提供 `append_item_batch`，不提供 update、delete、replace、truncate、clear 或 reorder API。

### INV-003 Batch 是最小提交单位

一个 Batch 内的 metadata 和 `Vec<Item>` 必须一起出现或一起不存在。

### INV-004 原生 Open Responses Item

持久化和核心内存中的上下文类型都是 `openresponses_rust::Item`。不得引入同构的持久化消息类型。

### INV-005 Item ID 与存储位置分离

`Item.id` 保持 Open Responses 原始含义。本地定位使用 Session Local Item Sequence。

### INV-006 单 Session 单 active Activation

同一 Session 的两个 Activation 不得同时处于 running、waiting_for_interaction 或 cancelling。

### INV-007 Source 固定

Session Source 创建时必须是固定 Context Position 闭区间，之后不随来源 Session 增长而改变。

### INV-008 Source 图无环

新 Session 只能引用创建时已存在的 Session。创建操作必须拒绝自引用和循环引用。

### INV-009 Projection 不写历史

Context Projection 只存在于一次模型请求过程中。Projection Hook 的结果不得回写为历史替换。

### INV-010 单一 Store Writer

只有 ragentd 内的 Store Writer 可以创建或 rename Store 事实文件。Runner、前端和 Extension 不得直接写 Store。

### INV-011 状态可重建

所有 `status.json` 和内存索引都是投影。删除后必须能够从 Spec、Batch 和 Event 重建。

### INV-012 动态配置有历史引用

每个 Activation 记录实际 `config_ref`。配置文件内容变化不得改变历史 Activation 的解释。

### INV-013 权限先解析后执行

文件、HTTP 和命令工作目录能力在交给 Extension 或工具前必须完成结构化权限判定。每个 Activation 始终引用一个当前有效的内容寻址 Permission Snapshot；人工授权通过追加事实切换到新快照，不能改写历史决定。

## 4. 项目结构

第一版保持单个 Rust package，避免过早拆分 workspace：

```text
ragent/
  Cargo.toml
  src/
    lib.rs

    core/
      agent.rs
      context.rs
      event.rs
      model.rs

    domain/
      ids.rs
      session.rs
      activation.rs
      batch.rs
      config.rs
      permission.rs
      interaction.rs
      workspace.rs

    store/
      mod.rs
      directory.rs
      layout.rs
      recovery.rs
      writer.rs
      projection.rs

    control/
      command.rs
      service.rs
      session_controller.rs
      permission_controller.rs
      runner.rs
      registry.rs

    hooks/
      manager.rs
      protocol.rs
      runtime.rs

    protocol/
      envelope.rs
      request.rs
      response.rs
      event.rs

    transport/
      local.rs
      unix.rs

    bin/
      ragent.rs
      ragentd.rs
      ragent-tui.rs
      ragent-http.rs

  wit/
    ragent-extension.wit

  web/
    ...

  docs/
    PRODUCT_REQUIREMENTS.zh-CN.md
    TECHNICAL_DESIGN.zh-CN.md
```

等第三方前端真实出现后，再将 `protocol` 抽成独立 crate。第一版不为潜在复用增加 package 数量。

## 5. 标识符和序号

### 5.1 ID

所有 ID 对外表现为不透明字符串，建议内部使用 UUIDv7：

```text
sess_<uuid-v7>
act_<uuid-v7>
turn_<uuid-v7>
evt_<uuid-v7>
cmd_<uuid-v7>
permreq_<uuid-v7>
```

要求：

- 全局唯一。
- 不依赖文件路径外的隐含含义。
- 可以安全用于文件名。
- 外部调用者不得解析 ID 中的时间或结构。

### 5.2 序号类型

使用不同 newtype，禁止相互混用：

```rust
struct LocalItemSeq(u64);
struct BatchSeq(u64);
struct EventSeq(u64);
struct ContextPos(u64);
```

- 所有序号从 0 开始。
- 范围统一使用闭区间 `[from, through]`。
- 空 Session 没有 tail sequence，以 `Option` 表示，不使用 `-1` 或最大整数哨兵。

### 5.3 Local Item Sequence 与 Context Position

两者含义不同：

- `LocalItemSeq`：只定位当前 Session 自己持久化的 Item。
- `ContextPos`：定位 Session 的有效上下文，即有序 Sources 展开后再拼接本地 Item 的结果。

例如：

```text
Session B sources:
  A context 0..9       -> B context 0..9
  C context 20..24     -> B context 10..14

Session B local items:
  local 0              -> B context 15
  local 1              -> B context 16
```

对外的 fork 范围使用 `ContextPos`，本地 Batch 使用 `LocalItemSeq`。

## 6. 核心领域模型

### 6.1 SessionSpec

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSpec {
    pub format_version: u32,
    pub id: SessionId,
    pub created_at: DateTime<Utc>,

    pub basic_system_prompt: String,
    pub default_config_ref: ConfigRef,
    pub workspace_ref: WorkspaceRef,

    #[serde(default)]
    pub sources: Vec<SessionSource>,

    #[serde(default)]
    pub labels: BTreeMap<String, String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<ProducerRef>,
}
```

约束：

- `format_version` 第一版为 1。
- `basic_system_prompt` 直接用于 Open Responses `instructions` 的基础值。
- `sources` 顺序具有语义，不得排序。
- labels 只用于创建时分类，不作为可变 metadata。
- Spec 不保存 title、updated_at、item_count 或 phase；这些属于投影。

### 6.2 SessionSource

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSource {
    pub kind: SessionSourceKind,
    pub session_id: SessionId,
    pub from_context_pos: ContextPos,
    pub through_context_pos: ContextPos,
}

pub enum SessionSourceKind {
    ForkedFrom,
    DerivedFrom,
    SummaryOf,
    ContinuedFrom,
}
```

Source 解析规则：

1. 读取来源 Session 的 Spec。
2. 递归展开来源 Session 的有效上下文。
3. 选取固定 Context Position 闭区间。
4. 按当前 Session `sources` 顺序拼接。
5. 最后追加当前 Session 本地 Item。

Store 在创建 Session 时验证范围存在。来源 Session 后续追加不会改变已经选定的范围。

同一个祖先 Item 因多个 Source 重叠而重复出现是允许的；系统不隐式去重。

### 6.3 ProducerRef

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProducerRef {
    pub session_id: SessionId,
    pub activation_id: ActivationId,
    pub turn_id: Option<TurnId>,
    pub call_id: Option<String>,
    pub command_id: CommandId,
}
```

它回答“谁创建了这个 Session”，不承载 Source 上下文。

### 6.4 ItemBatch

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemBatch {
    pub format_version: u32,
    pub batch_seq: BatchSeq,
    pub first_local_item_seq: LocalItemSeq,
    pub kind: ItemBatchKind,

    pub activation_id: ActivationId,
    pub turn_id: Option<TurnId>,
    pub created_at: DateTime<Utc>,

    pub response_id: Option<String>,
    pub activation_request: Option<ActivationRequestMeta>,
    pub items: Vec<openresponses_rust::Item>,
}

pub struct ActivationRequestMeta {
    pub command_id: CommandId,
    pub idempotency_key: String,
    pub config_ref: ConfigRef,
}

pub enum ItemBatchKind {
    Input,
    ModelOutput,
    ToolOutput,
}
```

约束：

- `items` 不得为空。
- Batch 内 Item 按数组顺序分配连续 `LocalItemSeq`。
- 最后序号由 `first_local_item_seq + items.len() - 1` 派生，不重复持久化。
- `response_id` 只用于 ModelOutput。
- `activation_request` 只用于创建 Activation 的第一个 Input Batch；steer Input Batch 不携带它。
- ToolOutput Item 必须保留其 `call_id`，并能在有效上下文中找到对应 FunctionCall。
- Input 必须是 Open Responses 可接受的输入 Item。

Batch 文件中的 `items` 使用 Open Responses 原生 JSON，不再包裹单个 StoredItem。

### 6.5 Activation

Activation 状态不保存为可覆盖的事实对象，而由追加事件投影：

```rust
pub enum ActivationPhase {
    Queued,
    Running,
    WaitingForInteraction,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}
```

每个 Activation 必须记录：

- Activation ID。
- 创建命令 ID 和幂等键。
- effective Config Ref。
- effective Permission Snapshot Ref。
- 请求时间与最终时间。
- 当前 phase。
- 总 Usage 投影。
- 最后错误投影。

### 6.6 SessionEventEnvelope

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEventEnvelope {
    pub format_version: u32,
    pub event_seq: EventSeq,
    pub event_id: EventId,
    pub session_id: SessionId,
    pub activation_id: Option<ActivationId>,
    pub turn_id: Option<TurnId>,
    pub created_at: DateTime<Utc>,
    pub event: SessionEvent,
}
```

第一版事件：

```rust
pub enum SessionEvent {
    SessionCreated {
        command_id: CommandId,
        idempotency_key: String,
    },
    SessionClosed,
    SessionArchived,

    ActivationRequested {
        config_ref: ConfigRef,
        permission_snapshot_ref: PermissionSnapshotRef,
        command_id: CommandId,
        idempotency_key: String,
        input_batch_seq: BatchSeq,
    },
    ActivationStarted,
    ActivationCancellationRequested,
    ActivationCompleted {
        usage: Option<openresponses_rust::Usage>,
    },
    ActivationFailed {
        error: String,
    },
    ActivationCancelled,
    ActivationInterrupted {
        reason: String,
    },

    TurnStarted,
    TurnCompleted {
        response_id: String,
        appended_batch_seq: Option<BatchSeq>,
        usage: Option<openresponses_rust::Usage>,
    },

    ToolCallStarted {
        call_id: String,
        name: String,
    },
    ToolCallFinished {
        call_id: String,
        name: String,
        success: bool,
        duration_ms: u64,
        output_batch_seq: Option<BatchSeq>,
    },

    PermissionRequested {
        request: PermissionRequest,
    },
    PermissionResolved {
        request_id: PermissionRequestId,
        decision: PermissionDecision,
        session_rule: Option<ScopedPermissionRule>,
        resulting_snapshot_ref: PermissionSnapshotRef,
    },
    SessionPermissionBootstrapped {
        rules: Vec<ScopedPermissionRule>,
    },

    SessionSpawnRequested {
        command_id: CommandId,
    },
    SessionSpawned {
        child_session_id: SessionId,
        command_id: CommandId,
    },
}
```

Event 不重复保存完整 Item、模型文本、reasoning 或工具输出，只引用 Batch。

### 6.7 SessionStatus

```rust
pub enum SessionPhase {
    Open,
    Closed,
    Archived,
    Corrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatus {
    pub projection_version: u32,
    pub phase: SessionPhase,
    pub active_activation_id: Option<ActivationId>,
    pub queued_activation_count: usize,

    pub local_item_count: u64,
    pub effective_context_item_count: u64,
    pub batch_count: u64,
    pub event_count: u64,

    pub updated_at: DateTime<Utc>,
    pub title: Option<String>,
    pub last_error: Option<String>,
}
```

允许的正常状态转换：

```text
Open → Closed → Archived
```

- 有 active Activation 时关闭 Session 必须被拒绝；调用者应先取消并等待最终状态。
- Archived Session 只从默认列表隐藏，仍可读取和作为 Source。
- Corrupted 是恢复过程产生的保护状态，不允许继续追加事实。
- 第一版不提供 reopen 或 unarchive；需要继续工作时创建派生 Session。

`status.json` 是投影缓存，允许原子覆盖。任何字段都必须能从 Spec、Sources、Batches 和 Events 重建。

### 6.8 ConfigRevision

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigRevision {
    pub format_version: u32,
    pub config_ref: ConfigRef,
    pub created_at: DateTime<Utc>,

    pub response_template: openresponses_rust::CreateResponseBody,
    pub extensions: Vec<ExtensionConfig>,
}
```

`response_template` 尽量直接复用 Open Responses 字段。保存前必须满足：

- `input` 为空。
- `instructions` 为空。
- `previous_response_id` 为空，除非未来明确启用服务端状态模式。
- `stream` 只能为空或 false。
- `background` 只能为空或 false。
- tools 可以为空；实际 tools 由 Extension 在 Agent 初始化和请求构建阶段提供。

Config Ref 生成：

1. 清除 `config_ref` 和 `created_at` 等非语义字段。
2. 对 JSON object key 做稳定排序。
3. 使用 UTF-8、无多余空白的规范 JSON。
4. 计算 SHA-256。
5. 生成 `sha256:<lowercase-hex>`。

如果相同 Ref 文件已存在，比较去除 `config_ref`、`created_at` 后的规范语义内容；一致则保留第一次写入的文件和创建时间并直接复用，不要求整份文件逐字节相同。

### 6.9 WorkspaceSpec

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSpec {
    pub format_version: u32,
    pub id: WorkspaceId,
    pub root: PathBuf,
    pub created_at: DateTime<Utc>,
}
```

Workspace 只定义身份和根目录；权限由独立 Permission Policy 解析，不进入 Session Item，也不重复嵌入 WorkspaceSpec。

### 6.10 Permission 模型

```rust
pub struct PermissionRequest {
    pub id: PermissionRequestId,
    pub session_id: SessionId,
    pub activation_id: ActivationId,
    pub extension_id: Option<ExtensionId>,
    pub capability: CapabilityRequest,
    pub reason: Option<String>,
}

pub enum CapabilityRequest {
    FileRead { path: PathBuf },
    FileWrite { path: PathBuf },
    Http {
        scheme: String,
        host: String,
        port: Option<u16>,
        method: String,
    },
    CommandWorkDir { work_dir: PathBuf },
}

pub enum DefaultPermissionDecision {
    Allow,
    Deny,
    Ask,
}

pub struct PermissionPolicyLayer {
    pub file_read: Option<RuleSet<PathRule>>,
    pub file_write: Option<RuleSet<PathRule>>,
    pub http: Option<RuleSet<HttpRule>>,
    pub command_work_dir: Option<RuleSet<PathRule>>,
}

pub struct RuleSet<R> {
    pub deny: Option<Vec<R>>,
    pub allow: Option<Vec<R>>,
    pub default: Option<DefaultPermissionDecision>,
}

pub enum PermissionRule {
    FileRead(PathRule),
    FileWrite(PathRule),
    Http(HttpRule),
    CommandWorkDir(PathRule),
}

pub struct ScopedPermissionRule {
    pub extension_id: Option<ExtensionId>,
    pub rule: PermissionRule,
}

pub struct ScopedPermissionPolicy {
    pub general: PermissionPolicyLayer,
    pub extensions: BTreeMap<ExtensionId, PermissionPolicyLayer>,
}

pub struct PermissionSnapshot {
    pub format_version: u32,
    pub permission_snapshot_ref: PermissionSnapshotRef,
    pub global_revision: String,
    pub project_revision: Option<String>,
    pub session_event_through: EventSeq,
    pub effective: ScopedPermissionPolicy,
}

pub enum PermissionDecision {
    Deny,
    AllowOnce,
    AllowSession,
    AllowProject,
    AllowAlways,
}
```

`Option` 是配置语义的一部分：`None` 表示继承低优先级层，`Some(vec![])` 表示显式清空。解析顺序按字段从低到高覆盖：

```text
global.general
  → global.extensions[extension]
  → project.general
  → project.extensions[extension]
  → session.general
  → session.extensions[extension]
```

最终 RuleSet 的判断顺序固定为 deny、allow、default。拒绝规则优先于允许规则。Session 层不是 SessionSpec 中的可变字段，而是从 `SessionPermissionBootstrapped` 和包含 `session_rule` 的 `PermissionResolved` Event 投影得到。

## 7. Directory Store

### 7.1 根目录布局

```text
<store-root>/
  format.json
  writer.lock
  .tmp/

  configs/
    sha256-<hex>.json

  permission-snapshots/
    sha256-<hex>.json

  workspaces/
    <workspace-id>.json

  sessions/
    <session-id>/
      spec.json
      status.json

      batches/
        00000000000000000000.json
        00000000000000000001.json

      events/
        00000000000000000000.json
        00000000000000000001.json
```

规则：

- 序号文件名固定 20 位十进制补零，字典序等于数值顺序。
- `.tmp/` 和目标目录中的 `.tmp-*` 内容从不视为已提交事实。
- Store 根目录和 Session 目录默认权限为 `0700`。
- JSON 文件默认权限为 `0600`。
- `status.json` 丢失或损坏时忽略并重建。
- Permission Snapshot 使用与 Config Revision 相同的规范 JSON 和 SHA-256 内容寻址规则；Activation Event 只保存引用。

### 7.2 Store 格式文件

```json
{
  "store_format": "ragent-directory-store",
  "major": 1,
  "minor": 0,
  "open_responses_schema": "openresponses-rust-2026.7.26"
}
```

规则：

- major 不支持时拒绝打开。
- minor 高于当前实现时，只有明确声明向前兼容才允许只读打开。
- 第一版不实现旧格式迁移。

### 7.3 单 Writer 所有权

ragentd 启动时必须获取 `writer.lock` 的 OS advisory exclusive lock。获取失败意味着已有 writer，进程必须退出，不能降级为无锁写入。

只读诊断工具可以在明确的只读模式下打开 Store，但不得创建临时事实文件。

### 7.4 原子文件提交

所有事实提交使用：

```text
serialize to bytes
  → create tmp file with create_new
  → write_all
  → flush
  → sync_all according to durability mode
  → rename to final path
  → optionally fsync parent directory
```

Durability 模式：

```rust
pub enum DurabilityMode {
    Strict,
    Balanced,
}
```

- Strict：文件和父目录都同步，适合关键 Session。
- Balanced：保证进程崩溃一致性，但允许操作系统延后最终落盘。
- 第一版默认 Balanced，并允许全局配置 Strict。

目标路径已存在时：

- 内容完全相同：视为幂等成功。
- 内容不同：返回 Store Corruption/Conflict，禁止覆盖。

### 7.5 创建 Session

创建流程：

1. 验证 ID、Config Ref、Workspace Ref 和所有 Source；Workspace 必须已经信任。
2. 验证 Source 图无环。
3. 创建稳定路径 `/tmp/ragent/<session-id>` 的 Session 临时目录。
4. 解析现有全局和项目权限层，生成该临时目录的文件读、文件写和命令工作目录三条最小允许规则。
5. 在 Store 根级 `.tmp/` 创建完整 Session 目录。
6. 写入 `spec.json`。
7. 写入 Event 0 `SessionCreated`，随后写入一个包含三条规则的 `SessionPermissionBootstrapped` Event。
8. 生成初始 `status.json`。
9. 原子 rename 临时 Session 目录为最终目录。
10. 更新内存 Session/Relation 索引。
11. 向订阅者发布 committed event。

Session 临时目录可以被操作系统清理，因此 Runner 启动时负责按相同路径重建；授权事实不需要重写。若创建 Session 目录事实失败，应尽力清理刚创建但尚未对外提交的临时目录。

Session 目录已存在时：

- Spec 完全相同：幂等成功。
- Spec 不同：返回 ID Conflict。

### 7.6 追加 Batch

Store Writer 维护每个 Session 的：

```rust
struct SessionTail {
    next_batch_seq: BatchSeq,
    next_local_item_seq: LocalItemSeq,
    next_event_seq: EventSeq,
}
```

追加命令必须携带：

```rust
struct AppendBatchCommand {
    command_id: CommandId,
    idempotency_key: String,
    session_id: SessionId,
    expected_next_local_item_seq: LocalItemSeq,
    batch: NewItemBatch,
}
```

提交步骤：

1. 检查 Session open。
2. 检查幂等键。
3. 检查 expected tail。
4. 验证每个 Open Responses Item。
5. 验证 Batch kind 与 metadata 约束。
6. 分配 Batch Seq 和 Local Item Seq。
7. 原子写 Batch 文件。
8. 更新内存 tail 和状态投影。
9. 原子更新 `status.json`。
10. 发布 `batch.committed` 内存事件。

`context.append.prepare` 必须由 Agent Core 或 Session Controller 在调用 Store 前完成。Store 只接受最终候选并执行独立验证，不加载 Extension。提交成功后，调用方再执行 `context.append.observe`；即使 status 更新或 Observer 失败，Batch 仍然有效，恢复时重建投影。

### 7.7 追加 Event

Event 使用同样的临时文件加 rename 流程。Event 和 Batch 是两个独立事实流。

当一个业务步骤需要同时提交 Batch 和 Event 时，事实优先级为：

1. 先提交 Batch。
2. 再提交引用该 Batch 的 Event。

如果 Event 提交失败，恢复逻辑可以根据 Batch metadata 补充缺失事件。反向顺序会产生引用不存在 Batch 的 Event，因此禁止。

第一版不试图实现跨文件 ACID transaction，而使用幂等 Controller 和恢复规则保持最终一致。这是轻量方案的明确取舍。

### 7.8 幂等记录

为避免单独维护可变幂等数据库：

- Session 创建使用确定的 Session ID 作为幂等边界。
- ActivationRequested Event 保存 Command ID 和幂等键。
- Store Writer 启动时构建 `idempotency_key → result` 内存索引。
- 重复命令返回第一次提交的资源 ID 或序号。

对于非常大的 Store，未来可增加可重建的幂等索引文件；第一版不需要。

### 7.9 状态和关系索引

启动时扫描：

- `sessions/*/spec.json` 构建 Session 索引。
- Spec Sources 构建正向和反向关系索引。
- 每个 Session 最后一个 Batch/Event 文件构建 tail。
- `status.json` 有效时使用缓存，否则完整重放该 Session。

内存索引：

```rust
struct StoreIndex {
    sessions: HashMap<SessionId, SessionSummary>,
    children_by_source: HashMap<SessionId, Vec<SessionId>>,
    tails: HashMap<SessionId, SessionTail>,
    active_activations: HashMap<SessionId, ActivationId>,
}
```

索引不是事实源。

### 7.10 物理删除

正常 Control Plane API 不提供事实文件删除。物理清理通过离线 maintenance 命令完成，并要求：

1. Session 已处于 Archived。
2. 没有 active 或 queued Activation。
3. 反向关系索引中不存在引用该 Session 的 Source。
4. ragentd 停止写入或 maintenance 命令持有 writer lock。
5. 先将 Session 目录原子移动到 Store 外的 trash 目录，再执行实际删除。

如果存在子 Session，默认拒绝；第一版不提供级联删除。Context append-only 保证适用于仍由 Store 管理的 Session，不阻止用户执行明确的离线数据清理。

## 8. ControlStore API

领域接口保持小而明确，不设计通用 Resource API：

```rust
pub trait ControlStore: Send + Sync {
    async fn create_session(
        &self,
        spec: SessionSpec,
        command: CommandMeta,
    ) -> Result<CreateSessionResult, StoreError>;

    async fn get_session(
        &self,
        id: &SessionId,
    ) -> Result<Option<SessionSnapshot>, StoreError>;

    async fn list_sessions(
        &self,
        query: SessionQuery,
    ) -> Result<Vec<SessionSummary>, StoreError>;

    async fn append_batch(
        &self,
        command: AppendBatchCommand,
    ) -> Result<CommittedBatch, StoreError>;

    async fn append_event(
        &self,
        command: AppendEventCommand,
    ) -> Result<SessionEventEnvelope, StoreError>;

    async fn put_permission_snapshot(
        &self,
        snapshot: PermissionSnapshot,
    ) -> Result<PermissionSnapshotRef, StoreError>;

    async fn get_permission_snapshot(
        &self,
        reference: &PermissionSnapshotRef,
    ) -> Result<Option<PermissionSnapshot>, StoreError>;

    async fn resolve_permission(
        &self,
        command: ResolvePermissionCommand,
    ) -> Result<SessionEventEnvelope, StoreError>;

    async fn read_local_items(
        &self,
        session: &SessionId,
        range: LocalItemRange,
    ) -> Result<Vec<Item>, StoreError>;

    async fn read_effective_context(
        &self,
        session: &SessionId,
        range: Option<ContextRange>,
    ) -> Result<Vec<Item>, StoreError>;

    async fn read_events(
        &self,
        session: &SessionId,
        after: Option<EventSeq>,
    ) -> Result<Vec<SessionEventEnvelope>, StoreError>;

    async fn list_children(
        &self,
        source: &SessionId,
    ) -> Result<Vec<SessionSummary>, StoreError>;
}
```

`DirectoryControlStore` 对外是异步接口，内部所有写命令发送给单独 Store Writer task。读取可以使用 `spawn_blocking` 或受控同步文件读取，不能在 Tokio executor 上进行大文件阻塞读取。

`resolve_permission` 不是普通 `append_event` 的语法糖：Store Writer 必须在同一串行临界区确认目标 Request 已存在且尚未解决，再追加 Resolution，从而保证多前端竞争时只有一个 Decision 生效。

## 9. Context Projection

### 9.1 装配过程

```text
Session Sources
  → recursively resolve fixed Context Slices
  → concatenate in declared order
  → append local Session Items
  → apply core request policy
  → context.project.prepare Hook
  → Input::Items(Vec<Item>)
```

System Prompt 不作为 Item 混入上下文：

```rust
request.instructions = Some(prepared_system_prompt);
request.input = Some(Input::Items(projected_items));
```

### 9.2 Projection 约束

- Projection 可以过滤、重排或临时增加 Item。
- Projection 结果必须通过 Open Responses Item validation。
- Projection 不生成 Batch，不改变 Local Item Seq。
- Projection 不影响 Session fork 所引用的事实上下文。
- Projection 只对当前 Turn 有效。

### 9.3 Source 解析缓存

可以缓存：

```text
(session_id, through_context_pos) → resolved ordered local segments
```

缓存结果只包含 Segment 引用，不复制 Item 内容：

```rust
struct ResolvedSegment {
    session_id: SessionId,
    from_local_item_seq: LocalItemSeq,
    through_local_item_seq: LocalItemSeq,
}
```

因为 Spec 和历史都不可变，该缓存无需失效；只有当前 Session 新增本地 Item 时扩展尾部即可。

## 10. Agent Core

### 10.1 职责

Agent Core 只负责：

- 接收 Context Projection。
- 构建和发送 Open Responses 请求。
- 解析完整 `ResponseResource`。
- 调用工具 Action。
- 生成待追加 Item Batch。
- 调度 ReAct loop。
- 发出结构化运行事件。

Agent Core 不负责：

- Session 文件布局。
- Session 列表和派生关系。
- 前端展示。
- 跨 Session 调度。
- 直接写 Directory Store。
- 文件协作追踪。

### 10.2 请求构建

```rust
let mut request = config.response_template.clone();
request.input = Some(Input::Items(projected_items));
request.instructions = Some(prepared_system_prompt);
request.tools = active_tools;
request.stream = Some(false);
request.background = Some(false);
```

之后执行 `model.request.prepare` Transform，并再次验证：

- stream 不得为 true。
- background 不得为 true。
- input 必须是 Items。
- FunctionCallOutput 必须对应之前的 FunctionCall。
- 不允许重复 call_id。

### 10.3 响应处理

```text
Client::create_response
  → ResponseResource
  → model.response.observe
  → validate status/error
  → take response.output: Vec<Item>
  → model.response.prepare
  → append ModelOutput Batch
  → locate FunctionCall Items
  → execute tools
  → append ToolOutput Batch
  → continue or finish
```

核心不生成与 Item 重复的 response text。CLI/TUI/WebUI 从 `Item::Message` 和 `MessageContent` 渲染文本。

完整 Response 默认不持久化，以避免与 `output: Vec<Item>` 重复。Event 只保存 response ID、Usage、状态和 Batch 引用。

成功 Response 的 `output` 为空时不创建空 Batch，TurnCompleted 的 `appended_batch_seq` 为 `None`。

### 10.4 Tool 调用

每个 FunctionCall：

1. 发出 ToolCallStarted Event。
2. 执行 `tool.call.prepare`。
3. 调用唯一 Action owner。
4. 将结果构造为 Open Responses `Item::FunctionCallOutput`。
5. 执行 `tool.result.prepare`。
6. 按当前 Turn 将一个或多个结果组成 ToolOutput Batch。
7. 发出 ToolCallFinished Event。

工具失败也必须产生合法 FunctionCallOutput，使模型能够观察失败。系统级不可恢复错误才终止 Activation。

### 10.5 外部副作用

无法在本地 Store 与外部系统之间实现通用 exactly-once。

如果工具已产生副作用，但 Runner 在提交 FunctionCallOutput 前崩溃：

- Activation 恢复为 interrupted。
- 不自动重试该 ToolCall。
- 前端展示 call_id 和未知结果状态。
- 支持幂等键的 Extension 可以显式查询或重试。

## 11. Activation 生命周期

### 11.1 状态机

```text
                    ┌───────────────┐
                    │    queued     │
                    └───────┬───────┘
                            │ claim
                            ▼
                    ┌───────────────┐
              ┌─────│    running    │──────┐
              │     └───────┬───────┘      │
              │             │              │
              │             ▼              │
              │   waiting_for_interaction  │
              │             │              │
              │             └──────────────┤
              │                            │
       cancel │                            │ success/error
              ▼                            ▼
         cancelling            succeeded / failed
              │
              ▼
          cancelled

进程失联且结果不确定：interrupted
```

### 11.2 创建 Activation

`activation.submit` 命令包含：

```rust
struct SubmitActivation {
    command_id: CommandId,
    idempotency_key: String,
    session_id: SessionId,
    config_ref: Option<ConfigRef>,
    input: Vec<Item>,
}
```

处理：

1. 校验 Session open。
2. 校验 input 非空。
3. 解析 effective Config Ref。
4. 确认 Workspace 已信任并解析全局、项目和 Session 权限层。
5. 生成内容寻址的 Permission Snapshot。
6. Session Controller 为本次提交创建短生命周期 Extension admission context，执行 `input.prepare`。
7. 执行 `context.append.prepare` 并验证结果。
8. 分配 Activation ID，提交对应的 Input Batch。
9. 追加引用 Input Batch、Config Ref 和 Permission Snapshot Ref 的 ActivationRequested Event。
10. 返回 Activation ID。
11. Session Controller 将其加入该 Session 队列。

Input Batch 是 Activation 请求的持久化载体。系统必须先提交 Input Batch，再提交引用它的 ActivationRequested Event，因此 daemon 在接受命令后崩溃也不会丢失用户输入。如果 Batch 已存在而 Event 缺失，恢复 Controller 根据 Batch 的 `activation_id` 和 `activation_request` 补写请求 Event。

如果 input.prepare、context.append.prepare 或 Store 校验拒绝输入，则既不创建 Activation，也不提交 Input Batch。

### 11.3 Steering 和 Follow-up

对外提供明确语义：

- `activation.steer`：为当前 active Activation 注入输入，在下一个安全边界追加 Input Batch。
- `activation.follow_up`：创建排在当前 Activation 后的新 Activation。

如果 steer 到达时目标 Activation 已结束，返回 `activation_not_active`，不得静默转换成 follow-up。

### 11.4 取消

取消命令必须指定 Session ID 和 Activation ID：

1. 追加 ActivationCancellationRequested。
2. 触发对应 Runner 的 CancellationToken。
3. Runner 停止当前模型、Hook 或 Tool 工作。
4. 执行 Extension shutdown。
5. 追加 ActivationCancelled 或 ActivationFailed。

二次强制终止属于进程管理行为，不伪造为正常 cancelled。

## 12. Session Controller 和 Runner

### 12.1 SessionController

职责：

- 观察 ActivationRequested。
- 保证单 Session 单 active Activation。
- 维护每个 Session 的 FIFO 队列。
- 创建 Runner task。
- 处理 session.spawn Command。
- 进程启动时标记遗留 active Activation 为 interrupted。
- 更新可重建状态投影。

### 12.2 RunnerRegistry

```rust
struct RunnerRegistry {
    active: HashMap<SessionId, ActiveRunner>,
}

struct ActiveRunner {
    activation_id: ActivationId,
    cancellation: CancellationToken,
    join_handle: JoinHandle<()>,
}
```

Registry 仅存在于内存，不是事实源。

### 12.3 Runner 构建

1. 加载 Session Spec。
2. 加载 Config Revision。
3. 加载 Workspace Spec。
4. 创建 Session 临时目录。
5. 验证 Activation 引用的 Permission Snapshot 已包含 Session 临时目录授权。
6. 加载并初始化 Extension；默认不授予环境能力。
7. 解析 Sources 并构建有效上下文。
8. 应用 agent.prepare 和 context.project.prepare。
9. 加载 Activation 已提交的 Input Batch。
10. 运行 Agent loop；每次受控能力调用先经过 Permission Controller。
11. 追加最终 Event。
12. shutdown Extension。

Runner 不长期持有 Session 所有权；Activation 完成后销毁。

## 13. Session 创建、fork 和 spawn

### 13.1 普通创建

无 Source 的 Session 有效上下文初始为空。基础 System Prompt 不作为 Item。

### 13.2 Fork

外部请求可以使用动态 Selector：

```rust
pub enum ContextSelector {
    All,
    Range { from: ContextPos, through: ContextPos },
    Activation { id: ActivationId },
    Turn { id: TurnId },
    TailActivations { count: u32 },
}
```

Controller 在创建子 Session 前解析为固定 `SessionSource`。Spec 中不保存动态 Selector。

### 13.3 多来源派生

多个 Source 按用户声明顺序拼接。系统不自动添加分隔 Message；如需分隔，由新 Session 的 System Prompt、初始输入或 Projection Hook 明确提供。

### 13.4 Spawn Command

Extension 不直接调用 `create_session`，而返回：

```rust
pub struct SpawnSessionCommand {
    pub command_id: CommandId,
    pub requested_by: ProducerRef,
    pub basic_system_prompt: String,
    pub config_ref: Option<ConfigRef>,
    pub workspace_ref: Option<WorkspaceRef>,
    pub sources: Vec<RequestedSource>,
    pub initial_input: Vec<Item>,
    pub labels: BTreeMap<String, String>,
}
```

SessionController：

1. 运行 session.create.prepare/admission Hook。
2. 解析和验证 Source。
3. 使用 Command ID 派生或关联确定的 child Session ID。
4. 创建子 Session。
5. 在父 Session 追加 SessionSpawned Event。
6. 如果 initial_input 非空，提交子 Session Activation。

崩溃恢复时，如果子 Session 已存在但父 Event 缺失，Controller 根据 ProducerRef 补写父 Event。

## 14. Hook 和 Extension

### 14.1 Extension 生命周期

WASM Component world 提供：

```text
metadata() -> metadata
initialize(config) -> result
invoke(hook_request) -> hook_result
shutdown()
```

Extension 的控制实例生命周期限定在一个 Runner/Activation 内。需要文件或网络 capability 的 Action 使用短生命周期执行实例，以便按单次调用构造精确的 WASI P2 context；未来如需复用实例，必须保证 capability 不会泄漏到其他调用。

### 14.2 Hook 类型

```rust
pub enum HookKind {
    Transform,
    Observer,
    Action,
}
```

- Transform：按 priority 顺序串行执行，输出成为下一个输入。
- Observer：不能改变结果；失败策略可以是 abort 或 ignore。
- Action：一个 Action 只能有一个 owner。

### 14.3 Hook 数据结构

Hook 尽量直接传递 Open Responses 类型：

| Hook | Payload |
|---|---|
| `agent.prepare` | 请求模板、基础 instructions、tools |
| `input.prepare` | `Vec<Item>` |
| `turn.prepare` | Turn metadata + Context view |
| `context.project.prepare` | `Vec<Item>` |
| `model.request.prepare` | `CreateResponseBody` |
| `model.response.observe` | `ResponseResource` |
| `model.response.prepare` | `Vec<Item>` |
| `tool.call.prepare` | `Item::FunctionCall` 等价 JSON |
| `permission.requirements` | 工具名、参数和候选 capability 请求 |
| `tools.call` | FunctionCall request |
| `tool.result.prepare` | `Item::FunctionCallOutput` 等价 JSON |
| `context.append.prepare` | New Item Batch candidate |
| `context.append.observe` | Committed Batch metadata |
| `turn.complete` | Turn result metadata |
| `agent.error` | Error metadata |
| `agent.shutdown` | Activation metadata |

WASM 边界需要 JSON 序列化，但 Host 内部序列化前后仍还原为同一个 Open Responses 类型。

### 14.4 历史保护

`context.append.prepare` 请求中不提供可替换的完整 `next`：

```json
{
  "session_id": "sess_...",
  "kind": "model_output",
  "items": [ ... ]
}
```

Hook 只能返回新的 `items`。Store 最终分配序号并追加。

`context.project.prepare` 可以看到完整临时 Items 并返回新的 Projection，但不产生 Store 写入。

### 14.5 Control Command

需要产生控制面副作用的 Action 返回结构化 Command。Host 验证 Command 类型、调用者、权限和幂等键后提交给 Controller。

Extension 发起的第一版 Control Command 只支持 `session.spawn`。Permission Response 是用户前端调用 Control Plane 的交互协议，不允许 Extension 自行回答，也不开放任意 Store write。

### 14.6 Capability 交付

工具执行分为两个阶段：

1. Host 调用无环境 capability 的 `permission.requirements`，由 Extension 根据工具名和参数返回精确的 `CapabilityRequest` 列表。
2. Permission Controller 逐项判定；全部允许后，Host 才创建工具 Action 的执行实例并调用 `tools.call`。

文件能力通过 WASI P2 filesystem 的目录 preopen 交付，读取和写入分别配置。HTTP 通过 WASI P2 outgoing HTTP 提供，Host 在请求发出前再次校验 scheme、host、port 和 method。第一版只允许 `http` 和 `https`，不暴露任意 TCP/UDP socket。

命令执行不通过 ambient WASI process capability，而通过 ragent Host Action：

```rust
pub struct ExecuteCommandRequest {
    pub command: String,
    pub work_dir: Option<PathBuf>,
}
```

`work_dir` 缺省为 Runner 当前工作目录；Runner 初始化时将当前工作目录设为 Workspace root。第一版只规范化并校验这个字段是否命中允许目录，不解释 `command` 内的 `cd`、绝对路径或子进程行为，因此它是目录参数准入而不是完整命令沙箱。

如果 `permission.requirements` 未声明某项能力，而 Action 实际尝试使用它，WASI Host 必须拒绝，不得在执行期间临时扩大 capability。需要新权限时，本次工具调用以可识别的 permission error 结束，由 Agent 或用户显式重试，避免自动重放未知副作用。

## 15. Control Plane Protocol

### 15.1 Transport

第一版：

- ragentd 监听 Unix Domain Socket。
- Socket 默认位于 Store 根目录之外的运行时目录。
- Socket 权限仅当前用户可访问。
- 使用 UTF-8 JSON Lines；每一行是一个完整 Envelope。
- 请求和响应通过 `request_id` 关联。
- 事件是独立 Envelope。

JSONL 足够，因为 JSON 字符串内换行会被转义，不会破坏帧边界。

### 15.2 请求 Envelope

```json
{
  "protocol_version": 1,
  "request_id": "req_...",
  "method": "activation.submit",
  "params": {}
}
```

### 15.3 响应 Envelope

```json
{
  "protocol_version": 1,
  "request_id": "req_...",
  "ok": true,
  "result": {}
}
```

错误：

```json
{
  "protocol_version": 1,
  "request_id": "req_...",
  "ok": false,
  "error": {
    "code": "session_not_found",
    "message": "...",
    "details": {}
  }
}
```

### 15.4 第一版方法

```text
session.create
session.get
session.list
session.close
session.archive
session.children
session.fork

context.read

activation.submit
activation.get
activation.cancel
activation.steer
activation.follow_up

interaction.get
interaction.list
interaction.respond

permission.explain

events.read
events.subscribe

config.put
config.get

workspace.put
workspace.get
workspace.list
workspace.trust
```

`interaction.respond` 必须携带 Permission Request ID、Decision 和调用者看到的 Event Seq。Store Writer 以 Request ID 做 compare-and-append：只有尚未解决的请求可以追加 `PermissionResolved`，并发回答中的其余请求返回 `interaction_already_resolved`。

`permission.explain` 是只读接口，返回匹配到的拒绝规则、允许规则或默认策略，以及每个最终字段来自 global、project、session 还是 extension override；它不得返回未经信任的项目配置内容。

### 15.5 Event 推送

```json
{
  "protocol_version": 1,
  "type": "event",
  "session_id": "sess_...",
  "event": { ... }
}
```

订阅者过慢时：

- ragentd 不得阻塞 Agent Runner。
- 断开慢订阅者并返回最后成功发送的 Event Seq。
- 客户端通过 `events.read(after_seq)` 恢复。

第一版不持久化全局事件序号。订阅所有 Session 的客户端重连后重新执行 `session.list`，再按每个 Session 的 Event Seq 恢复。

## 16. 前端架构

### 16.1 CLI

CLI 是 Control Plane client，不直接构建 Agent 或写 Session 文件。

支持：

- 创建/选择 Session。
- 提交和取消 Activation。
- 查看原生 Item。
- fork。
- 列出来源和子 Session。
- 展示并回答权限询问。
- 审查和确认项目目录信任。
- 启动嵌入式 ragentd 作为便利模式。

### 16.2 TUI

TUI 连接相同 Unix Socket，维护的只是界面状态：

- 当前选择 Session。
- 折叠项。
- 滚动位置。
- 本地输入草稿。

这些状态丢失不影响 Session。

### 16.3 WebUI

WebUI 不直接连接 Directory Store。独立 `ragent-http` Adapter 将 Control Plane Protocol 映射为：

- HTTP query/command。
- SSE event stream。
- 可选 WebSocket 双向交互。

默认只监听 `127.0.0.1`。远程监听必须显式配置认证。

### 16.4 Item 渲染

前端直接按 Open Responses Item variant 渲染：

- Message → role/content。
- Reasoning → summary 和可选 carrier metadata。
- FunctionCall → name/arguments/status。
- FunctionCallOutput → output/status。
- Compaction → compaction metadata。
- Extension → extension-specific fallback view。

大 output 默认只读取或显示摘要，用户展开时再读取完整 Batch。

### 16.5 Interaction 渲染

前端从 Event 或 `interaction.list` 渲染待处理权限，不自己重算规则。至少展示：请求 Extension、能力类型、原始资源、规范化资源、请求原因、匹配规则来源和五种回答。

选择 AllowSession、AllowProject 或 AllowAlways 时，前端先调用 `permission.explain` 展示将写入的最窄规则，再提交 `interaction.respond`。TUI 与 WebUI 可以使用不同布局，但 Decision 枚举和并发语义必须一致。

## 17. Workspace 和文件协作

Workspace 是 Store 外部的数据面：

```text
Session Runner A ─┐
                  ├── /path/to/workspace
Session Runner B ─┘
```

Control Plane 不记录：

- 哪个 Session 读取了哪个文件。
- 文件发布和消费关系。
- 文件版本。
- Session 间消息游标。

约定：

- 完整文件写入推荐临时文件加原子 rename。
- 多 Runner 写同一目标时由工具或工作流使用锁文件、独立子目录或 Git 协调。
- 需要进入模型上下文的文件内容，通过工具结果形成 Item。
- 需要表达 Session 血缘时显式创建派生 Session，不从文件行为推断。

每个 Session 的临时目录位于系统临时根目录，不能作为持久事实源。

## 18. 崩溃恢复

### 18.1 启动恢复步骤

1. 获取 writer lock。
2. 读取并验证 Store Format。
3. 清理或隔离所有临时文件和临时 Session 目录。
4. 扫描 Config、Permission Snapshot 和 Workspace。
5. 扫描 Session Spec，构建 DAG。
6. 验证无环和 Source 范围。
7. 扫描 Batch/Event tail。
8. 验证序号连续、文件名与内容序号一致。
9. 校验或重建 status projection。
10. 找出没有最终状态的 active Activation。
11. 追加 ActivationInterrupted。
12. 重建幂等索引和 Activation 队列。
13. 开始接受客户端连接。

### 18.2 损坏策略

- 临时文件：忽略并移动到 quarantine 或删除。
- 非法 JSON 事实文件：该 Session 标记 corrupted，禁止继续写。
- Batch 序号缺口：标记 corrupted，不自动猜测。
- Item 验证失败：标记 corrupted。
- status 损坏：删除并重建，不标记 Session corrupted。
- Config Ref 内容 hash 不匹配：拒绝使用所有引用该配置的 Activation。

第一版不自动修复或重写事实历史。

### 18.3 Batch/Event 不一致

- Batch 存在、引用 Event 缺失：可以由恢复 Controller 补写 Event。
- Event 引用不存在 Batch：视为损坏，因为提交顺序违反协议。
- ToolCallStarted 后无 ToolCallFinished：Activation 标记 interrupted，禁止自动重试未知副作用。

## 19. 并发和背压

### 19.1 写入

- 所有写操作通过 bounded MPSC 进入 Store Writer。
- Store Writer 串行处理并分配序号。
- 队列满时对命令提交产生背压，不丢事实。

### 19.2 读取

- 不同 Session 的读取可以并行。
- 单个大 Batch 的 JSON 解析在 blocking worker 上执行。
- Context Projection 应按 Batch 顺序增量读取，避免一次扫描整个 Store。

### 19.3 Runner

- 每个 Session 最多一个 active Runner。
- 全局 Runner 并发数由配置限制。
- 超出限制的 Activation 保持 queued。
- 工具调用默认按模型输出顺序串行执行；只有未来明确定义并行语义后才并行。

### 19.4 事件订阅

- Store commit 先完成，再向内存 broadcast 发布。
- broadcast 丢失不影响事实。
- 慢客户端断开后通过持久 Event 恢复。

## 20. 安全模型

### 20.1 本地 Store

- 根目录 `0700`。
- 文件 `0600`。
- writer socket 仅当前用户可访问。
- 日志默认不打印完整 Item JSON。
- reasoning carrier、工具参数和输出均按敏感数据处理。

### 20.2 Extension

- Extension 能力通过 WASI host imports 显式提供。
- Extension 不获得 Store 根目录访问权。
- Extension 不获得 Control Plane socket 访问权。
- Extension 控制面动作必须经过结构化 Command 和权限校验。

### 20.3 配置来源和快照

可编辑配置来源为：

```text
$XDG_CONFIG_HOME/ragent/config.toml       # 全局策略
$XDG_CONFIG_HOME/ragent/trusted.toml      # 已信任项目目录
<workspace>/.ragent/config.toml           # 项目增量策略
Session Permission Events                 # Session 增量策略
```

macOS 在未设置 XDG 路径时使用平台标准用户配置目录。所有配置文件默认 `0600`，通过临时文件、flush 和原子 rename 更新。

这些文件只是配置输入。Control Plane 解析后在 Directory Store 写入内容寻址的 `PermissionSnapshot`，Activation、Runner 和 Extension 只使用该快照，不各自读取或合并 TOML。因此运行时仍只有一份明确的有效配置，外部文件变化只影响之后生成的新快照。

合并是逐叶字段覆盖，不是对象整体替换。以 TOML 为例：

```toml
[permissions.file_read]
default = "ask"
allow = ["/work/project"]

[permissions.extensions.shell.command_work_dir]
allow = []
```

缺少 `allow` 表示继承；`allow = []` 表示显式清空继承到的允许列表。未知字段、重复 Extension 名称和非法规则必须拒绝，不得静默忽略。

### 20.4 项目目录信任

`trusted.toml` 保存规范化后的项目根目录，不使用相对路径：

```toml
version = 1
directories = ["/absolute/path/to/project"]
```

Workspace 第一次进入时的 admission 流程：

1. 规范化当前目录，并检查它是否精确命中 trusted directories。
2. 未命中时只检查 `.ragent/config.toml` 是否存在，不打开、不解析其内容。
3. 创建 Trust Interaction，展示完整目录和“已有项目配置/尚无项目配置”。已有配置时明确要求用户先检查文件内容。
4. 用户拒绝则不得读取项目配置，也不得启动使用该 Workspace 的 Session。
5. 用户信任后，先原子更新 `trusted.toml`，再读取已有项目配置。
6. 若原本不存在项目配置，原子创建最小 `.ragent/config.toml`，仅将项目根目录加入 file read、file write 和 command work-dir allowlist。

目录信任不等于允许项目内任何行为；它只允许读取项目配置。项目配置中的具体 capability 仍按权限规则判断。已有文件绝不自动重写。

### 20.5 规则匹配

文件和命令工作目录规则使用规范化绝对路径前缀，并保证路径组件边界匹配；`/work/a` 不得匹配 `/work/ab`。对已存在路径解析符号链接后再判断。对尚不存在的写入目标，解析最近的已存在父目录，再拼接剩余规范化组件；无法安全规范化时进入 deny 或 ask，不自动允许。

HTTP Rule 至少包含 scheme、host、可选 port 和可选 method。Host 规范化域名大小写和默认端口后匹配；URL path、DNS 重绑定防护和代理策略属于后续加固，但第一版不得开放非 HTTP(S) socket。

每项请求只按以下顺序判断一次：

```text
any deny match  → deny
else allow match → allow
else default     → allow | deny | ask
```

`permission.explain` 返回命中的规则来源和规范化后的资源，便于用户理解决策。

### 20.6 人工询问状态机

```text
running
  → PermissionRequested committed
  → waiting_for_interaction
  → PermissionResolved committed
  → running | failed | cancelled
```

`PermissionRequested` 必须先写 Store，再广播给前端。等待不占用模型请求，但 Runner task 保留可取消的 continuation。前端断线重连后可从 Event 恢复并回答。daemon 重启仍遵守统一恢复规则：active Activation 标记为 interrupted，历史请求可查看但不可继续回答，系统不重建或自动重放未知副作用的 continuation。

五种回答的效果：

| Decision | 当前请求 | 持久化位置 |
|---|---|---|
| Deny | 拒绝 | 仅 PermissionResolved Event |
| AllowOnce | 允许一次 | 仅 PermissionResolved Event |
| AllowSession | 允许并形成最窄规则 | PermissionResolved Event 中的 Session rule |
| AllowProject | 允许并形成最窄规则 | `.ragent/config.toml` |
| AllowAlways | 允许并形成最窄规则 | 全局 `config.toml` |

“最窄规则”表示文件请求默认写入规范化目标或用户确认的目录、HTTP 请求写入当前 endpoint 范围、命令请求写入当前 `work_dir`。前端必须在提交前展示最终规则和作用域，允许用户收窄，但扩大范围需要显式编辑。

AllowSession 的 rule 与 Resolution 在同一个 Event 中提交，投影直接将它纳入 Session 权限层。AllowProject/AllowAlways 先以原子文件更新配置，再追加 Resolution；若两步之间崩溃，恢复逻辑以 Request ID 和目标规则检测已落盘更新并幂等补写 Resolution。AllowSession、AllowProject 和 AllowAlways 生成新的 Permission Snapshot；Deny 和 AllowOnce 保持原 Ref，其中 AllowOnce 的 Resolution 本身就是只对该 Request 有效的一次性凭证。

### 20.7 Session 临时目录

Session ID 决定稳定临时路径 `/tmp/ragent/<session-id>`。创建 Session 时先解析 global/project 层，再以 Session Event 追加该路径的 file read、file write 和 command work-dir allow rules。Runner 每次启动按需重建目录，并把路径和使用说明加入基础 System Prompt 的运行时附加部分；这段附加内容不改变 Session Spec。

临时目录不是事实源，可以清理；其中需要保留的内容必须通过工具结果进入 Item，或由 Agent 显式移入 Workspace。

### 20.8 已知安全边界

- 命令权限第一版只验证显式 `work_dir`；不分析命令中的 `cd`、绝对路径、shell 重定向或子进程。
- Workspace 文件协作仍不追踪读写过程，但每次实际能力调用受权限控制。
- 项目信任仅保护项目配置加载，不替代操作系统文件权限。
- Permission Policy 是应用层 capability 控制，不承诺对恶意本机原生进程提供强沙箱；WASM Extension 则不得获得未授予的 ambient capability。

## 21. Schema 和协议演进

独立版本：

```text
Directory Store Format
Session Spec Format
Item Batch Format
Session Event Format
Control Plane Protocol
Extension Protocol
Config Format
```

规则：

- 主版本变化可以不兼容。
- 第一版不承担任何历史兼容责任。
- 未知 Event variant 可以由只读前端展示为 generic JSON，但 writer 必须理解后才能修改相关 Session。
- Open Responses crate 版本固定在项目版本中。
- 升级 Open Responses 类型时必须用 golden fixtures 验证 Item JSON round-trip。
- 不把内部 Rust enum discriminant 或二进制布局写入 Store。

## 22. 可观测性

核心提供结构化诊断，不建立独立遥测数据库：

- Session/Activation/Turn ID。
- 模型延迟。
- Tool 调用耗时。
- Store append 延迟。
- Batch Item 数和字节数。
- Context Projection Item 数和字节数。
- Usage。
- Runner queue depth。
- Store Writer queue depth。

默认日志不得包含：

- 完整 System Prompt。
- 完整用户输入。
- reasoning carrier。
- 工具参数中的秘密。
- 完整工具输出。

## 23. 测试方案

### 23.1 领域测试

- Local Item Seq 连续分配。
- Context Pos 到 Resolved Segment 的递归映射。
- 多 Source 顺序保持。
- Source 范围固定，不受来源后续追加影响。
- DAG 循环拒绝。
- 重叠 Source 不被隐式去重。

### 23.2 Open Responses 测试

- 每个支持的 Item variant JSON round-trip。
- MessageContent 多模态 round-trip。
- FunctionCall/FunctionCallOutput 配对验证。
- Reasoning encrypted content byte-for-byte 保持。
- Item Extension 未知 extra 字段保持。
- Store read 后可以直接构造 `Input::Items`。

### 23.3 Store 测试

- Session 目录原子创建。
- Batch 原子 append。
- 目标文件冲突检测。
- 临时文件恢复。
- status 删除后重建。
- 非法 JSON 和序号缺口检测。
- 文件权限。
- writer lock 独占。
- 幂等命令重复提交。

### 23.4 崩溃注入测试

在以下位置强制终止：

- 临时 Batch 创建前。
- 写入一半后。
- sync 后 rename 前。
- rename 后 status 更新前。
- Batch 后 Event 前。
- Tool 副作用后 ToolOutput Batch 前。

恢复结果必须符合本方案的状态矩阵。

### 23.5 Runner 测试

- 同 Session Activation 串行。
- 不同 Session Activation 并行。
- cancel 指向准确 Activation。
- steer 只进入 active Activation。
- follow-up 排队。
- 非流式 Response 只产生完整 ModelOutput Batch。
- Extension shutdown 在正常、失败和取消路径执行。

### 23.6 Protocol 测试

- JSONL framing。
- request/response 关联。
- 协议版本拒绝。
- 慢订阅者不会阻塞 Runner。
- Event Seq 恢复无丢失、无错误重复。
- 未知前端 Event fallback 展示。

### 23.7 权限测试

- global → extension → project → extension → session → extension 逐字段覆盖顺序。
- 缺失字段继承，显式空数组清空，二者结果不同。
- 同时命中 deny 和 allow 时 deny 优先。
- 文件路径组件边界、符号链接和不存在写入目标规范化。
- HTTP scheme、host、port 和 method 匹配；任意 socket 被拒绝。
- Command `work_dir` 缺省 Workspace，并只校验声明目录。
- 未信任目录不读取已有 `.ragent/config.toml`。
- 信任无配置项目后只初始化项目根目录的 file read/write 和 command allow rules。
- 已有项目配置在信任后不被覆盖。
- 新 Session 自动获得 `session_tmp` 三类允许规则，重启后由 Event 重建。
- Deny、AllowOnce、AllowSession、AllowProject、AllowAlways 分别落到正确作用域。
- 两个前端同时回答同一 Request 时只有一个成功。
- 前端断线不结束等待；daemon 重启则 Activation 进入 interrupted 且不重放工具。
- Extension 未声明 capability 时，WASI P2 file/HTTP 和 Host command 均拒绝。

### 23.8 端到端验收

1. 创建 Session。
2. 提交用户 Item。
3. 模型返回 reasoning 和 FunctionCall。
4. Extension 执行工具。
5. 保存 FunctionCallOutput。
6. 模型返回最终 Message。
7. 停止 ragentd。
8. 删除 status cache。
9. 重启并得到相同上下文、状态和 Usage。
10. 从第一轮结束位置 fork。
11. 验证子 Session 没有复制父 Batch，且模型输入正确。

## 24. 实施阶段

### Milestone 1：领域和 Store

- Open Responses 依赖和原生 Item round-trip。
- ID 和序号 newtype。
- SessionSpec、ItemBatch、SessionEvent、ConfigRevision。
- Directory layout。
- Store Writer。
- 原子文件提交。
- 启动恢复和 status projection。

### Milestone 2：Agent 纵向链路

- Context Projection。
- 非流式 CreateResponseBody/ResponseResource。
- Input、ModelOutput、ToolOutput Batch。
- Activation 状态机。
- Cancellation。
- 最小 Extension Hook。

### Milestone 3：Control Plane 和 CLI

- Unix Socket JSONL protocol。
- Session/Activation API。
- Event subscription。
- CLI client。
- 嵌入式 daemon 便利模式。

### Milestone 4：派生和 TUI

- Context Selector 解析。
- 多 Source 和零复制 fork。
- 正反关系索引。
- Session spawn Command。
- TUI。

### Milestone 5：Web 与权限系统

- HTTP/SSE Adapter。
- WebUI。
- 可恢复的 Permission Request 和多前端人工回答。
- global/project/session/extension 权限解析与内容寻址快照。
- 项目目录信任和安全初始化。
- WASI P2 filesystem、outgoing HTTP 和受控 command work-dir capability。

## 25. 未来 Store 升级路径

只有出现实际瓶颈时才增加后端：

- Session 数量大到启动扫描不可接受。
- 需要多进程直接并发写。
- 需要复杂关系查询。
- 需要跨多个资源的严格事务。
- 需要多机 Runner。

升级时保持领域 API 和语义：

```text
DirectoryControlStore
  → SqliteControlStore
  → optional PostgresControlStore
```

不承诺第一版磁盘文件可以被其他后端直接打开，但 SessionSpec、ItemBatch、SessionEvent 和 SessionSource 的 JSON 语义保持稳定，可通过显式导入导出迁移。

## 26. 最终架构边界

```text
Open Responses
  定义模型请求、响应、Item、Tool 和 Usage

ragent Core
  定义模型 I/O、Context Projection、ReAct loop 和 Hook 调度

Control Plane
  定义 Session、Activation、Runner、命令和状态协调

Directory Store
  保存不可变 Spec、Item Batch、Event 和配置修订

Extension
  提供工具、变换、观察、策略和受控 Action

Frontend
  查询、提交命令和展示，不持有唯一状态

Workspace
  提供 Runner 文件协作，不进入控制面交互建模
```

任何新增功能都应先判断属于哪一层。若功能不能在不破坏上述边界的情况下实现，应优先重新审视功能设计，而不是扩张 Agent Core。

## 附录 A：需求追踪

| PRD 需求组 | 技术方案章节 |
|---|---|
| Session 管理 | 6.1、6.6、6.7、7.5、7.10、13 |
| 上下文存储 | 5.2、5.3、6.4、7.6、8、9 |
| Activation 和执行 | 6.5、10、11、12、18、19 |
| Session 派生 | 5.3、6.2、6.3、9.3、13 |
| 配置 | 6.8、7.1、10.2 |
| Workspace | 6.9、17、20.4 |
| Hook 和 Extension | 10、14、20.2 |
| CLI、TUI 和 WebUI | 15、16 |
| 权限和人工交互 | 6.10、7.1、11、14.6、15.4、20、23.7 |
| 可靠性与恢复 | 7、18、19、23 |
| 安全和演进 | 20、21、22 |

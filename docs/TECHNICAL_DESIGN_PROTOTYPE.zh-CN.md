# ragent Prototype 技术设计

状态：Draft

版本：0.1

日期：2026-09-01

关联文档：

- [产品需求文档](./PRODUCT_REQUIREMENTS.zh-CN.md)
- [完整技术方案](./TECHNICAL_DESIGN.zh-CN.md)
- [架构改造实施路线图](./ARCHITECTURE_REWRITE_ROADMAP.zh-CN.md)

## 1. 文档定位

本文定义完整技术方案的最小可运行子集。目标不是先造一个随后丢弃的临时实现，而是尽快建立可以真实创建 Session、调用模型、执行工具、持久化历史并在重启后继续使用的纵向框架。

Prototype 固定长期架构中最难返工的边界，放宽并发、服务化、权限交互和血缘能力。后续完整版在这些边界内补齐能力，不重新引入覆盖式 Session 或第二套上下文模型。

冲突处理：

- Prototype 实现范围和阶段验收以本文为准。
- 最终领域语义、安全目标和完整能力以[完整技术方案](./TECHNICAL_DESIGN.zh-CN.md)为准。
- Prototype 明确标记为“放宽”或“延期”的部分，不得被解释为最终设计变更。

## 2. Prototype 交付结果

Prototype 完成后，用户可以通过同一个 `ragent` CLI：

1. 创建、列出和查看 Session。
2. 向一个 Session 提交用户输入。
3. 使用非流式 Open Responses 模型完成多轮 ReAct loop。
4. 调用已有 WASM Extension，并把 FunctionCall 和 FunctionCallOutput 保存为原生 Item。
5. 退出进程后重新打开同一 Session，继续追加上下文。
6. 查看按提交顺序保存的 Batch、Event 和最终状态。

以下场景不属于 Prototype 完成条件：

- CLI 退出后任务继续后台运行。
- 多前端同时观察或操作。
- 多 Session 并行执行。
- steer、follow-up、可恢复的人工权限询问。
- 多来源 fork、压缩派生和 Session spawn。
- TUI、WebUI、HTTP/SSE Adapter。

## 3. 保留与放宽的边界

### 3.1 Prototype 必须保留

- Session Spec 创建后不可变。
- 上下文只允许通过完整 Item Batch 追加。
- 持久化类型直接使用 `openresponses_rust::Item`。
- Batch 和引用它的 Event 在同一个 SQLite 事务提交。
- `session_status` 是可从 Spec、Batch 和 Event 重建的投影。
- AgentCore 不读取 Store、不理解 CLI，也不管理 Session 生命周期。
- Extension 不直接写 Store。
- 新旧 Store 不双写；新执行路径不回退旧 Session 语义。
- 模型请求保持非流式，完整 `ResponseResource.output` 形成 ModelOutput Batch。

### 3.2 Prototype 允许放宽

| 完整版要求 | Prototype 取舍 | 后续升级点 |
|---|---|---|
| ragentd + Unix Socket | CLI 进程内创建 `ControlService` | 增加 Transport，不改变 Service API |
| 异步 Activation | `session run` 阻塞等待结果 | 命令改为立即返回 Activation ID |
| 跨 Session 并行 | 全局一次只运行一个 Activation | 引入 RunnerRegistry 和并发限制 |
| 单独 Store Writer thread | 进程内单连接、同步串行写 | 在 Store facade 后增加 Writer thread |
| 完整命令幂等 | 只依赖事务和唯一约束防重复 | 增加 `command_results` 和请求 hash |
| 完整权限快照与询问 | 显式信任 Workspace，使用静态 Prototype Policy | 增加 PermissionSnapshot 和 Interaction |
| 固定范围多 Source DAG | Prototype 不开放派生 API | 增加 `session_sources` 和 Projection 解析 |
| cancel/steer/follow-up | 只支持当前进程内 Ctrl-C 取消 | 增加持久命令和状态机 |
| 慢订阅者恢复 | CLI 直接读取已提交 Event | 增加 Event subscription 和 after-seq |
| 完整启动审计 | Schema/version 检查和投影重建 | 增加 quick_check、关系和引用审计 |

放宽项只减少能力，不得破坏 3.1 的数据和模块边界。

## 4. 最小架构

```text
ragent CLI
    │ in-process command
    ▼
ControlService
    ├── SessionService
    └── SessionRunner
            │
            ▼
        AgentCore ── HookManager ── WASM Extensions
            │
            │ pending Item batches / run events
            ▼
     SqliteControlStore
            │
            └── Workspace
```

调用方向固定为：

```text
CLI → ControlService → Runner → AgentCore/HookManager
                   └────────────→ ControlStore
```

禁止反向依赖：

- AgentCore 不调用 ControlService 或 Store。
- Store 不加载 Hook 或 Extension。
- Extension 不连接 SQLite。
- CLI 不直接执行 SQL，也不自己拼装上下文。

## 5. Prototype 模块

第一轮只创建实际使用的文件：

```text
src/
  core/
    agent.rs
    context.rs
    model.rs
  domain/
    ids.rs
    session.rs
    batch.rs
    event.rs
    activation.rs
    config.rs
    workspace.rs
  store/
    mod.rs
    schema.rs
    sqlite.rs
    projection.rs
  control/
    service.rs
    runner.rs
  hooks/
    manager.rs
    protocol.rs
    runtime.rs
  cli/
    command.rs
    render.rs
```

不为未来目录提交空模块。Unix transport、PermissionController、RunnerRegistry 和前端 Adapter 在对应能力开始实现时再加入。

## 6. 最小领域模型

### 6.1 ID 和序号

至少定义以下不透明 newtype：

```rust
struct SessionId(String);
struct ActivationId(String);
struct TurnId(String);
struct BatchSeq(u64);
struct LocalItemSeq(u64);
struct EventSeq(u64);
struct ContextPos(u64);
```

序号从 0 开始。空 Session 的 tail 使用 `Option`，不使用整数哨兵。

### 6.2 SessionSpec

```rust
pub struct SessionSpec {
    pub format_version: u32,
    pub id: SessionId,
    pub created_at: DateTime<Utc>,
    pub basic_system_prompt: String,
    pub default_config_ref: ConfigRef,
    pub workspace_ref: WorkspaceRef,
    pub sources: Vec<SessionSource>,
}

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

Prototype 创建 API 要求 `sources` 为空，但字段和固定范围 `SessionSource` 的序列化结构从第一版开始保留，避免 Full 阶段重写已有 Spec。Prototype 不解析非空 Source；该能力在 Full 阶段启用。可变 title 和运行状态不进入 Spec。需要修改 prompt、config 或 workspace 时创建新 Session。

### 6.3 ItemBatch

```rust
pub struct ItemBatch {
    pub format_version: u32,
    pub session_id: SessionId,
    pub batch_seq: BatchSeq,
    pub first_local_item_seq: LocalItemSeq,
    pub kind: ItemBatchKind,
    pub activation_id: ActivationId,
    pub turn_id: Option<TurnId>,
    pub response_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub items: Vec<openresponses_rust::Item>,
}

pub enum ItemBatchKind {
    Input,
    ModelOutput,
    ToolOutput,
}
```

约束：

- `items` 不得为空。
- Item 数组顺序就是本地上下文顺序。
- Batch 内连续分配 `LocalItemSeq`。
- ModelOutput 保留 `ResponseResource.output` 的原生 Item。
- ToolOutput 必须保留 FunctionCallOutput 的 `call_id`。
- Store 不提供修改、删除、替换或重排 Batch 的 API。

### 6.4 SessionEvent

Prototype 只定义闭环必需事件：

```rust
pub enum SessionEvent {
    SessionCreated,
    ActivationRequested {
        input_batch_seq: BatchSeq,
        config_ref: ConfigRef,
    },
    ActivationStarted,
    TurnStarted,
    TurnCompleted {
        output_batch_seq: Option<BatchSeq>,
        usage: Option<openresponses_rust::Usage>,
    },
    ToolCallStarted { call_id: String, name: String },
    ToolCallFinished {
        call_id: String,
        name: String,
        success: bool,
        output_batch_seq: BatchSeq,
    },
    ActivationCompleted,
    ActivationFailed { error: String },
    ActivationCancelled,
    ActivationInterrupted { reason: String },
}
```

Event 只保存状态变化和 Batch 引用，不复制 Item 内容。

### 6.5 SessionStatus

Prototype 投影至少包含：

```rust
pub struct SessionStatus {
    pub phase: SessionPhase,
    pub active_activation_id: Option<ActivationId>,
    pub local_item_count: u64,
    pub batch_count: u64,
    pub event_count: u64,
    pub updated_at: DateTime<Utc>,
    pub last_error: Option<String>,
}
```

Prototype 的 Session phase 只需要 `Open` 和 `Corrupted`。close/archive 在完整版阶段加入。

## 7. 最小 SQLite Store

### 7.1 布局和连接

```text
.ragent/store/
  control.sqlite3
```

Prototype 由单个 CLI 进程持有一个读写连接：

- 启用 foreign keys、WAL 和明确的 busy timeout。
- 连接生命周期内串行执行所有写事务。
- 第二个写进程遇到锁冲突时明确失败，不自动抢占。

Prototype 暂不增加独立 `writer.lock`、Writer thread、Online Backup API 和多只读连接。这些能力通过 `ControlStore` 边界后续加入。

### 7.2 最小表

```text
store_meta(key PRIMARY KEY, value)
configs(config_ref PRIMARY KEY, payload_json)
workspaces(workspace_id PRIMARY KEY, payload_json)
sessions(session_id PRIMARY KEY, created_at, spec_json)
batches(session_id, batch_seq, first_local_item_seq, last_local_item_seq,
        kind, activation_id, turn_id, response_id, payload_json)
events(session_id, event_seq, kind, activation_id, batch_seq,
       turn_id, call_id, payload_json)
session_status(session_id PRIMARY KEY, projected_through_event_seq,
               projected_through_batch_seq, payload_json)
```

必须有 Session foreign key、Session 内 Batch/Event sequence 唯一约束和 Local Item range 唯一约束。JSON payload 保持与完整方案相同的领域结构。

Prototype schema 版本独立记录。后续新增表和索引使用显式 migration；不静默猜测未知版本。

### 7.3 事务边界

以下操作必须分别在一个事务中完成：

1. 创建 Session、写入 `SessionCreated`、建立初始投影。
2. 写入 Input Batch 和 `ActivationRequested`。
3. 写入 ModelOutput Batch 和 `TurnCompleted`。
4. 写入 ToolOutput Batch 和 `ToolCallFinished`。
5. 写入 Activation 最终 Event并更新投影。

事务内读取当前 tail 并分配下一序号。COMMIT 成功后才允许 Runner 或 CLI 报告该事实已完成。Prototype 不实现跨进程重试幂等，但同一事务不得产生 Event 引用不存在的 Batch。

### 7.4 Store API 和投影

`ControlStore` 至少提供：

```rust
trait ControlStore {
    fn create_session(...);
    fn get_session(...);
    fn list_sessions(...);
    fn commit_input(...);
    fn commit_model_output(...);
    fn commit_tool_output(...);
    fn append_event(...);
    fn read_local_items(...);
    fn read_events(...);
    fn rebuild_status(...);
}
```

每个业务方法表达一个事务不变量，不设计通用 `insert_resource` 或任意 SQL escape hatch。`append_event` 只允许提交不需要关联 Batch 的状态事件；Input、ModelOutput 和 ToolOutput 必须走对应的原子提交方法。

启动时校验 schema 版本，并为缺失或落后的 `session_status` 重放事件。无法解析的事实不得被静默跳过；对应 Session 标记为 corrupted 并拒绝继续追加。

## 8. Context Projection 和 AgentCore

### 8.1 Prototype Projection

Prototype 没有 Source，Projection 是当前 Session 所有本地 Batch 的有序拼接：

```text
read batches by batch_seq
  → concatenate native Items
  → context.project.prepare
  → Input::Items(Vec<Item>)
```

System Prompt 继续走 `request.instructions`，不作为 Message Item 写入历史。Projection Hook 只能影响当前请求，不能回写或替换已提交历史。

### 8.2 AgentCore 输入输出

AgentCore 输入：prepared instructions、`Vec<Item>` Context Projection、Open Responses `CreateResponseBody` 模板和当前可用 Tools。

AgentCore 输出：完整模型响应中的 output Items、待执行 FunctionCall、构造完成的 FunctionCallOutput Items、Usage、错误和取消结果。

AgentCore 不分配持久序号，不提交事务，不读取 Workspace，也不渲染 CLI。

### 8.3 最小 ReAct loop

```text
Input Batch committed
  → build Projection
  → non-streaming model request
  → ModelOutput Batch committed
  → if FunctionCall:
       execute tools serially
       → ToolOutput Batch committed
       → next model request
  → else ActivationCompleted
```

工具按模型输出顺序串行执行。单次 Activation 设置明确的最大 Turn 数，避免无限循环。

## 9. ControlService 和 Runner

### 9.1 ControlService

Prototype 公开进程内命令：

```text
session.create
session.get
session.list
session.run
context.read
events.read
```

`session.run`：

1. 校验 Session open 且当前没有 active Activation。
2. 执行输入 Hook，原子提交 Input Batch + ActivationRequested。
3. 创建 SessionRunner，追加 ActivationStarted。
4. 等待 Runner 完成、失败或被当前进程取消。
5. 返回最终状态和新增 Item。

虽然 Prototype 是阻塞命令，Activation ID 和事件仍必须真实存在，避免后续异步化时重写领域模型。

`session.create` 接收已经规范化的 Workspace root 和当前 effective model/extension config。ControlService 先确保 `WorkspaceSpec` 和内容寻址的 `ConfigRevision` 已存在，再用其 Ref 创建 Session；CLI 不自行生成或覆盖这些记录。任何前置写入失败时都不得创建引用缺失对象的 Session。

### 9.2 SessionRunner

Prototype 全局只运行一个 SessionRunner，因此不需要 FIFO 队列和 RunnerRegistry。Runner 生命周期仍与 Activation 绑定：

- 每次运行重新加载 Session、Config 和 Workspace。
- 每次运行重新构造 Extension 实例。
- 正常、失败和取消路径都调用 shutdown。
- 进程启动时发现没有最终事件的 Activation，追加 `ActivationInterrupted`，不自动重放模型或工具。

## 10. Hook、Extension 和最小权限

### 10.1 Hook 迁移范围

Prototype 保留现有 Extension 生命周期：

```text
metadata → initialize → invoke → shutdown
```

必须在 Prototype 内完成的语义修正：

- `context.commit` 改为 `context.append.prepare`。
- Hook 输入只包含本次候选 Items，不包含可覆盖的完整 `next`。
- Model response 和 Tool result 使用原生 Open Responses Item。
- Extension 只能返回 Tool result 或结构化 Hook result，不能直接写 Store。

Prototype 不要求一次性加入完整 Hook 集合；只迁移当前模型和三个已有 Extension 真正使用的 Hook。

### 10.2 Prototype Permission Policy

Prototype 只允许用户显式传入并确认一个 Workspace 根目录。静态策略为：

- 文件读写限制在规范化后的 Workspace root 和 Session 临时目录。
- command 的显式 `work_dir` 必须位于 Workspace root 或 Session 临时目录。
- HTTP 默认拒绝；需要网络的 Extension 在 Prototype 中不可用。
- 不提供运行中 Ask、AllowOnce、AllowSession、AllowProject、AllowAlways。

这是开发期策略，不是完整版 Permission 系统。权限判断必须经过独立的 `PrototypePermissionPolicy` 接口，Hook/Extension 不得自己读取配置或扩权，以便后续替换为 PermissionController。

命令策略仍只校验结构化 `work_dir`，不承诺分析 shell 字符串内的绝对路径、重定向或子进程行为。CLI 必须提示这一边界。

## 11. CLI

Prototype CLI 至少提供：

```text
ragent session create --workspace <path> [--prompt <text>]
ragent session list
ragent session show <session-id>
ragent session run <session-id> <input>
ragent session history <session-id>
```

CLI 可以在进程内打开 Store 和 ControlService，但不得直接执行 SQL、直接创建 AgentCore、覆盖保存 Session 聚合对象或根据旧 Session 文件自动 fallback。

Prototype 完成后，新 CLI 成为唯一写入口；同一个切换阶段删除旧 Session 写路径。旧数据不迁移。

## 12. 错误与恢复

Prototype 只保证本地单进程恢复：

- SQLite 未 COMMIT 的事务由 SQLite 回滚。
- 已 COMMIT 的 Batch/Event 必须完整可见。
- 启动时将未结束 Activation 标记为 interrupted。
- interrupted Activation 不自动继续模型请求或 ToolCall。
- `session_status` 缺失或落后时重建。
- schema 不支持、JSON 无法解析或序号不连续时拒绝继续写相关 Session。

Prototype 不保证外部工具副作用 exactly-once。如果工具可能已经执行但 ToolOutput 尚未提交，历史保留 ToolCallStarted，Activation 进入 interrupted，由用户人工判断。

## 13. Prototype 测试与验收

### 13.1 必测项

- SessionSpec 创建后没有 update API。
- Input、ModelOutput、ToolOutput 原生 Item JSON round-trip。
- Batch/Event 同事务全有或全无。
- Batch、Event 和 Local Item 序号连续。
- FunctionCallOutput `call_id` 能匹配已有 FunctionCall。
- Projection 严格按 Batch 和 Item 顺序装配。
- `context.append.prepare` 不能替换已提交历史。
- 无 Tool 单轮请求和有 Tool 多轮请求。
- shell、file_editor、image_viewer 的现有关键行为。
- Extension 在成功、失败和取消路径 shutdown。
- 清空 `session_status` 后得到相同投影。
- 遗留 active Activation 在重启后变为 interrupted。
- 新 CLI 不产生旧 Session JSON。

### 13.2 端到端验收

1. 在临时 Store 创建 Workspace、Config 和 Session。
2. 提交用户 Message Item。
3. 模型返回 Reasoning 和 FunctionCall。
4. Extension 在 Workspace 内执行工具。
5. ToolOutput 以 FunctionCallOutput Item 提交。
6. 模型返回最终 Message。
7. 退出 CLI 进程。
8. 清空或故意落后 `session_status`。
9. 重新运行 CLI，得到相同历史和状态。
10. 再提交一轮输入，确认只追加新 Batch。

### 13.3 Prototype 完成定义

以下条件全部满足才算 Prototype 完成：

1. 上述端到端验收通过。
2. 新 CLI 已切换到 ControlService，旧写入口已删除。
3. AgentCore 不依赖 Store、CLI 或 Session 文件。
4. SQLite 是新架构唯一事实源。
5. 历史只追加且使用原生 Open Responses Item。
6. 三个现有 Extension 在静态 Prototype Policy 下通过验证。
7. 重启不会自动重放未知工具副作用。
8. `cargo fmt`、测试和严格 Clippy 通过。

## 14. 到完整版的升级接口

Prototype 完成后按以下方向扩展，禁止推倒重来：

| Prototype 边界 | 完整版扩展 |
|---|---|
| `ControlStore` 同步串行写 | StoreWriter thread、持久幂等和完整审计 |
| 本地 Session Items | 固定范围 SessionSource 和多来源 Projection |
| 阻塞 `session.run` | 异步 Activation、队列、取消、steer、follow-up |
| 进程内 ControlService | JSONL Protocol、Unix Socket、ragentd |
| 静态 Workspace Policy | PermissionSnapshot、Interaction、WASI scoped capability |
| CLI 直接读取 Event | Event subscription、after-seq 和多前端恢复 |
| 单 CLI | TUI、HTTP/SSE Adapter 和 WebUI |

升级完成后，以[完整技术方案](./TECHNICAL_DESIGN.zh-CN.md)中的不变量、协议、安全和验收要求作为最终完成定义。

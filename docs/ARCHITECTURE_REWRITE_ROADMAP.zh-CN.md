# ragent 架构改造实施流程

状态：Draft

版本：0.1

日期：2026-08-31

关联文档：

- [产品需求文档](./PRODUCT_REQUIREMENTS.zh-CN.md)
- [详细技术方案](./TECHNICAL_DESIGN.zh-CN.md)

## 1. 文档目的

本文定义如何在当前 ragent 仓库内实施新架构。

改造采用以下总体策略：

- 保留当前仓库、Cargo package、Open Responses 集成、WASM Component/WASI P2 基础和三个已有扩展。
- 不兼容旧 Session 文件和旧运行时 API，不设计数据迁移器或双写层。
- 新模块先与旧模块短期并存，使用测试完成纵向链路；新 CLI 切换后立即删除旧实现。
- 每个阶段必须保持仓库可编译、可测试，不允许长期存在两套同时对外工作的 Session 语义。
- 先完成最小纵向闭环，再增加权限、Unix Socket、TUI 和 WebUI。

本文是实施顺序，不重新定义产品和领域语义。出现冲突时，以产品需求和详细技术方案为准。

## 2. 改造边界

### 2.1 保留并改造

- `openresponses-rust` 非流式请求和响应处理。
- Tokio 异步运行时和 CancellationToken。
- WASM Component、WIT 和 WASI P2 集成。
- `shell`、`file_editor`、`image_viewer` 三个扩展。
- 全局和项目 TOML 配置加载的有效部分。
- 模型、Hook、Extension 和 CLI 的现有测试素材。

### 2.2 整体替换

- 单文件 `SessionData`。
- 覆盖式 `SessionStore::save`。
- 可替换完整上下文的 `AgentContext` 和 `context.commit`。
- `AgentBuilder::from_session`。
- CLI 直接加载 Session、创建 Agent 和写文件的执行路径。
- WASI Runtime 默认预授权整个当前目录的方式。
- 前端直接依赖 Agent 内部 EventHandler 的状态模型。

### 2.3 明确不做

- 不读取或迁移旧 `.ragent/sessions/*.json`。
- 不提供旧 Session API 兼容层。
- 不同时向旧 Store 和新 Directory Store 双写。
- 不为了改造过程提前拆成多个 Cargo package。
- 不在核心链路稳定前实现 TUI 或 WebUI。

## 3. 目标目录结构

```text
src/
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

  cli/
    mod.rs
    command.rs
    render.rs

  bin/
    ragent.rs
    ragentd.rs
    ragent-tui.rs
    ragent-http.rs
```

第一轮只创建实际需要的文件。不得为了目录完整而提交空模块。

## 4. 总体执行规则

### 4.1 每阶段一个可验证边界

每个阶段完成时必须满足：

1. 新增代码具有对应测试。
2. 所有已迁移路径使用新语义，不在内部回退旧模型。
3. 未迁移的旧入口仍能编译，或已在同一阶段完整删除。
4. `cargo fmt`、测试和严格 Clippy 通过。
5. 文档和公开类型没有明显过期描述。

### 4.2 不做横跨多阶段的大提交

建议按阶段提交，避免在一次提交中同时修改 Domain、Store、WASM ABI、CLI 和权限配置。推荐提交边界：

```text
domain
directory store
agent core
local control vertical slice
hooks migration
permission system
daemon protocol
CLI cutover
lineage and frontend adapters
cleanup
```

### 4.3 新旧代码并存限制

允许短期存在：

- 旧 `src/session/` 与新 `src/domain/`、`src/store/` 同时编译。
- 旧 `src/wasm/` 与新 `src/hooks/` 在迁移提交前后短期存在。
- 旧 CLI 继续作为当前入口，新 Control Service 仅由测试调用。

禁止长期存在：

- 一个命令可能随机使用旧 Store 或新 Store。
- 一个 Activation 同时写入两种 Session 格式。
- 新 Control Plane 调用旧 `AgentBuilder::from_session`。
- 新 Hook 和旧 Hook 同时收到同一生命周期事件。

## 5. Phase 0：冻结基线与建立改造护栏

### 5.1 目标

在修改核心前保存当前可复用行为的基线，明确哪些旧行为不会保留。

### 5.2 工作项

- 确认 PRD、技术方案和本文档中的术语一致。
- 记录当前三个扩展的 metadata、tools 和关键输出。
- 为现有非流式模型请求建立最小回归测试。
- 为 Extension lifecycle 建立 metadata → initialize → invoke → shutdown 回归测试。
- 确认新 Store 默认根目录使用 `.ragent/store/`，与旧 `.ragent/sessions/` 隔离。
- 在新 Store 写入 `format.json`；格式不匹配时拒绝打开。
- 明确旧 Session 文件不会自动导入。

### 5.3 验收门槛

- 当前测试基线结果已记录。
- 三个扩展均可构建和加载。
- 新旧 Store 路径不会发生覆盖。
- 不引入任何兼容或双写代码。

## 6. Phase 1：Domain 模型与不变量

### 6.1 目标

建立不依赖文件系统、CLI、WASM 和 Tokio task 的纯领域模型。

### 6.2 文件范围

```text
src/domain/ids.rs
src/domain/session.rs
src/domain/activation.rs
src/domain/batch.rs
src/domain/config.rs
src/domain/permission.rs
src/domain/interaction.rs
src/domain/workspace.rs
src/domain/mod.rs
```

### 6.3 工作项

- 使用 newtype 定义：
  - `SessionId`
  - `ActivationId`
  - `TurnId`
  - `CommandId`
  - `BatchSeq`
  - `LocalItemSeq`
  - `EventSeq`
  - `ContextPos`
- 定义不可变 `SessionSpec`。
- 定义有序 `SessionSource` 和固定上下文范围。
- 定义以 `openresponses_rust::Item` 为核心的 `ItemBatch`。
- 定义 `SessionEventEnvelope` 和第一版 `SessionEvent`。
- 定义 Activation 状态投影。
- 定义 `ConfigRevision`、`WorkspaceSpec` 和 `PermissionSnapshot`。
- 为所有磁盘类型增加 `format_version`。
- 为 ID、序号和范围提供显式校验，不使用裸 `String`/`u64` 互换。

### 6.4 必须先固定的不变量

- SessionSpec 创建后不可修改。
- Item 只通过完整 Batch 追加。
- LocalItemSeq 连续递增且永不重排。
- Source 创建后指向固定范围。
- Source 图无环。
- Permission Snapshot 内容寻址且不可修改。
- Open Responses Item 不转换成第二套持久化消息结构。

### 6.5 测试

- ID 序列化与非法值拒绝。
- 不同序号 newtype 不能误用。
- Batch 序号范围计算。
- SessionSource 固定范围校验。
- Open Responses Item JSON round-trip。
- Permission `None` 与显式空列表语义不同。

### 6.6 验收门槛

- Domain 不依赖 `store`、`control`、`hooks`、`cli`。
- Domain 测试不访问真实用户目录。
- 不存在 `StoredMessage`、`StoredToolCall` 等同构模型。

## 7. Phase 2：Directory Store

### 7.1 目标

实现单 Writer、追加式、可崩溃恢复的唯一事实源。

### 7.2 文件范围

```text
src/store/mod.rs
src/store/layout.rs
src/store/directory.rs
src/store/writer.rs
src/store/recovery.rs
src/store/projection.rs
```

### 7.3 磁盘布局

```text
.ragent/store/
  format.json
  writer.lock
  .tmp/
  configs/
  permission-snapshots/
  workspaces/
  sessions/<session-id>/
    spec.json
    status.json
    batches/
      00000000000000000000.json
    events/
      00000000000000000000.json
```

不使用 `events.jsonl`。Batch 和 Event 都使用独立、不可变、固定宽度序号文件，以获得相同的原子提交和恢复语义。

### 7.4 工作项

- 定义小而明确的 `ControlStore` trait。
- 实现单独的异步 `StoreWriter` task。
- 所有写命令通过有界 Channel 进入 StoreWriter。
- 实现临时文件、flush、原子 rename。
- 实现 Session 目录原子创建。
- 实现 Batch 和 Event 原子追加。
- 实现目标文件已存在时的幂等比较和冲突拒绝。
- 实现 Config Revision 和 Permission Snapshot 内容寻址写入。
- 实现由 Spec、Batch、Event 重建 `status.json`。
- 实现 SessionSource 正反关系内存索引。
- 启动时隔离或清理未提交临时文件。

### 7.5 故障注入点

- 临时文件创建前。
- 写入一半后。
- flush 后、rename 前。
- rename 后、投影更新前。
- Batch 完成后、引用 Event 写入前。

### 7.6 验收门槛

- 崩溃后只可能看到完整 Batch/Event 或完全看不到。
- 删除 `status.json` 后得到相同状态。
- 同一 Session 的序号没有缺口或重复。
- 多个写调用实际由单 Writer 串行提交。
- Store 测试全部使用临时目录。

## 8. Phase 3：纯 AgentCore 与 Context Projection

### 8.1 目标

把当前 `Agent` 提炼为不持有 Session 持久状态的执行内核。

### 8.2 文件范围

```text
src/core/agent.rs
src/core/context.rs
src/core/model.rs
src/core/event.rs
```

### 8.3 AgentCore 输入

```rust
pub struct AgentRunRequest {
    pub instructions: String,
    pub projection: ContextProjection,
    pub response_template: CreateResponseBody,
    pub tools: Vec<Tool>,
}
```

实际结构可以调整，但必须满足：

- 输入上下文已经装配完成。
- 核心不知道 Source 如何解析。
- 核心不知道 Store 路径。
- 核心不知道 Session 文件格式。
- 核心不知道 CLI、TUI 或 WebUI。

### 8.4 AgentCore 输出

核心向 Runner 产生结构化结果：

- 完整 `ResponseResource`。
- 待追加的模型输出 Items。
- 待执行的 Tool Call。
- Tool Output Items。
- Usage。
- 可分类错误和取消结果。

AgentCore 不直接提交 Store；Runner 决定何时把完整 Batch 交给 StoreWriter。

### 8.5 Context Projection

- 从零个或多个固定 SessionSource 读取 Context Slice。
- 按 Source 声明顺序拼接。
- 最后追加当前 Session 本地 Items。
- Projection 可以临时变换，但不回写历史。
- 删除 `AgentContext::replace_items` 和 `clear_history` 在持久链路中的使用。
- `context.append.prepare` 只能变换本次待追加 Items，不再返回完整 `next`。

### 8.6 验收门槛

- 无 Tool 的单轮非流式模型请求通过。
- 多轮响应按 Batch 边界输出。
- AgentCore 测试不创建 Session 文件。
- 已提交历史无法通过 Core API 替换。

## 9. Phase 4：最小 Control Plane 纵向链路

### 9.1 目标

在加入 Unix Socket 和完整权限前，先用本地 Channel 打通可运行闭环。

### 9.2 文件范围

```text
src/control/command.rs
src/control/service.rs
src/control/session_controller.rs
src/control/runner.rs
src/control/registry.rs
src/transport/local.rs
```

### 9.3 工作项

- 实现 `SessionController`。
- 实现每个 Session FIFO Activation 队列。
- 实现全局 Runner 并发限制。
- 实现单 Session 单 active Activation。
- 实现 `SessionRunner`。
- 实现 Activation submit、start、complete、fail、cancel、interrupt Event。
- 实现 Config Ref 和 Permission Snapshot Ref 固化。
- 实现启动时将遗留 active Activation 标记为 interrupted。
- 使用本地 Channel 实现与未来协议一致的 Service 方法。

### 9.4 最小纵向验收

```text
session.create
→ activation.submit
→ append Input Batch
→ Runner claim
→ Context Projection
→ AgentCore
→ append ModelOutput Batch
→ ActivationCompleted
```

### 9.5 Lineage 同步实现

- 实现 Source selector 到固定范围解析。
- 实现零复制 fork。
- 实现多 Source 顺序装配。
- 实现 children 反向查询。
- 实现 DAG 校验。
- 实现最小 `session.spawn` Command，但可以暂不让 Extension 调用。

### 9.6 验收门槛

- 同 Session 两个 Activation 不并行。
- 不同 Session 可以并行。
- CLI 尚未切换时，本地集成测试已经能完成完整 Activation。
- fork 不复制父 Batch。
- daemon/Runner 重启不会自动重放未知副作用。

## 10. Phase 5：Hooks 与 WASM Runtime 迁移

### 10.1 目标

把现有 `src/wasm/` 移入新架构，同时先保持已有扩展行为，再单独增加权限隔离。

### 10.2 第一次迁移：保持行为

- 将 manager、types、runtime 分别迁移为 hooks manager、protocol、runtime。
- 保持 metadata、initialize、invoke、shutdown 生命周期。
- 将 Hook payload 改为新的 Domain/Open Responses 类型。
- 将旧 `context.commit` 替换为 `context.append.prepare`。
- 确认 Transform、Observer、Action 的顺序和 owner 规则。
- Control Command 只能返回结构化命令，Extension 不得直接写 Store。

### 10.3 扩展验证

分别验证：

- shell metadata、工具声明、成功命令、失败命令和 timeout=0。
- file_editor 文件写入和唯一匹配替换。
- image_viewer 有效图片、非法图片和大小限制。
- Extension shutdown 在成功、失败和取消路径执行。

### 10.4 验收门槛

- 三个扩展均能从新 HookManager 加载。
- AgentCore 无 Extension 时工具列表为空。
- Hook 无法替换已提交历史。
- Store 和 Control Plane 不依赖 WASM 具体实现。

## 11. Phase 6：权限与人工交互

### 11.1 子阶段 A：纯权限解析器

- 实现 file read、file write、HTTP、command work-dir Rule。
- 实现 global、project、session 和 extension 层级。
- 保留 `None` 与显式空列表差异。
- 固定 deny → allow → default 顺序。
- 实现 `permission.explain`。
- 生成内容寻址 Permission Snapshot。

这一阶段只做纯函数和配置测试，不修改 WASI Runtime。

### 11.2 子阶段 B：项目目录信任

- 增加全局 `trusted.toml`。
- 未信任时只检查项目配置是否存在，不读取内容。
- 通过 Interaction 请求用户确认。
- 无项目配置时初始化最小 file read/write 和 command work-dir allowlist。
- 有项目配置时不自动覆盖。
- 新 Session 追加 `session_tmp` 三类允许规则。

### 11.3 子阶段 C：Permission Interaction

- 实现 `PermissionRequested` 和 `PermissionResolved` Event。
- Activation 进入 `waiting_for_interaction`。
- 实现 Deny、AllowOnce、AllowSession、AllowProject、AllowAlways。
- StoreWriter 对同一个 Request 执行 compare-and-append。
- 多前端并发回答只接受第一个有效 Decision。
- 前端断线后请求继续存在。
- daemon 重启后 Activation 进入 interrupted，不自动恢复 Tool continuation。

### 11.4 子阶段 D：Capability 交付

```text
permission.requirements
→ PermissionController
→ scoped WASI P2 context
→ tools.call
```

- requirements probe 不获得文件、网络或命令能力。
- file capability 通过 WASI P2 filesystem preopen 提供。
- HTTP 通过 WASI P2 outgoing HTTP 提供。
- 不开放任意 TCP/UDP socket。
- command 使用 Host Action，并要求显式 `work_dir`。
- `work_dir` 缺省为 Runner 当前目录，Runner 初始目录为 Workspace root。
- 第一版只校验结构化 work_dir，不分析命令中的 `cd` 或绝对路径。
- 未声明 capability 的实际调用由 Host 再次拒绝。

### 11.5 验收门槛

- 三类 capability 均覆盖 allow、deny、ask。
- 五种 Decision 落入正确作用域。
- Extension 不再默认拥有整个当前目录。
- Permission Snapshot 能解释每个字段来源。
- 三个扩展在新权限路径下再次通过完整验证。

## 12. Phase 7：Unix Socket、协议与 ragentd

### 12.1 目标

把已经验证的本地 Control Service 暴露为稳定、前端无关的异步协议。

### 12.2 文件范围

```text
src/protocol/envelope.rs
src/protocol/request.rs
src/protocol/response.rs
src/protocol/event.rs
src/transport/unix.rs
src/bin/ragentd.rs
```

### 12.3 工作项

- 实现带 `protocol_version` 和 `request_id` 的 JSONL Envelope。
- 实现 Session、Context、Activation、Event、Config、Workspace、Interaction 方法。
- 实现异步 Activation submit，立即返回 Activation ID。
- 实现 Event subscribe 和 after-seq 恢复。
- 慢订阅者不得阻塞 Runner。
- Socket 默认仅当前用户可访问。
- 本地 Channel 和 Unix Socket 共用同一个 ControlService，不复制业务逻辑。

### 12.4 验收门槛

- daemon 独立启动后可以创建和运行 Session。
- 客户端断线不取消 Activation。
- 重连能按 EventSeq 恢复。
- 协议版本不兼容时明确拒绝。
- 停止 daemon 后 Store 中事实完整，重启投影一致。

## 13. Phase 8：CLI 切换

### 13.1 目标

将 `ragent` 从 Agent 入口变为纯 Control Plane 客户端。

### 13.2 工作项

- CLI 只负责参数解析、命令发送、事件订阅和渲染。
- CLI 不加载 Extension。
- CLI 不创建 AgentCore。
- CLI 不直接读取或写入 Store。
- CLI 不持有唯一 Activation 状态。
- 支持 Session create/list/get/close/archive/fork。
- 支持 Activation submit/get/cancel/steer/follow-up。
- 支持 Permission Interaction 展示和回答。
- 支持嵌入式 daemon 便利模式，但仍调用同一个 ControlService。

### 13.3 切换提交中删除

- 旧 Session CLI handler。
- `AgentBuilder::from_session`。
- 旧 `SessionStore` 的写入入口。
- CLI 到 EventHandler 的直接状态依赖。
- 从 `lib.rs` 导出的旧 Session API。

### 13.4 验收门槛

- CLI 退出后 Activation 继续运行。
- 另一个 CLI 可以观察并取消同一 Activation。
- CLI 与 daemon 模式使用完全相同的 Session 语义。
- 任何 CLI 命令都不会生成旧 Session JSON。

## 14. Phase 9：TUI、WebUI 与 Adapter

### 14.1 TUI

- 使用同一 Control Plane client。
- 只保存选择项、滚动位置、折叠状态和输入草稿。
- 支持 Session/Activation/Event/Permission 页面。
- 断线后按 EventSeq 恢复。

### 14.2 WebUI

- 通过独立 `ragent-http` Adapter 连接 Control Plane。
- HTTP 用于 query/command，SSE 用于 Event。
- 默认只监听 loopback。
- 远程监听必须显式配置认证。
- 浏览器端不读取 Directory Store。

### 14.3 验收门槛

- CLI、TUI、WebUI 同时观察时状态一致。
- 任一前端退出不影响 Runner。
- 两个前端回答同一个 Permission Request 时只有一个成功。
- 前端未知 Item/Event 能进行 fallback 展示，而不是导致崩溃。

## 15. Phase 10：清理、审计与发布准备

### 15.1 删除旧模块

确认没有引用后删除或完全替换：

```text
src/session/model.rs
src/session/store.rs
旧 src/context.rs
旧 src/builder.rs
旧 src/sender.rs
旧 src/wasm/
旧 CLI direct-run handler
```

具体文件可以被新实现占用同名路径；判断标准是旧类型和旧语义消失，而不是机械删除文件名。

### 15.2 全仓审计

- 搜索 `SessionData`、`SessionStore`、`replace_items`、`clear_history`。
- 搜索直接写 `.ragent/sessions` 的代码。
- 搜索绕过 StoreWriter 的事实文件写入。
- 搜索 Extension 直接访问 Store 或 Control socket 的路径。
- 搜索 `stream=true`、`background=true` 和旧 streaming hook。
- 搜索默认 full current-directory preopen。
- 检查所有文档中的旧 CLI 和旧 Session 格式描述。

### 15.3 最终验证

```text
cargo fmt --all -- --check
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
./scripts/build-extensions.sh
```

另外必须完成：

- Store 故障注入测试。
- daemon 重启恢复测试。
- 三个 Extension 真实入口测试。
- fork/derive 多来源测试。
- 五种权限回答端到端测试。
- CLI 退出和多客户端重连测试。
- 删除 status cache 后的投影一致性测试。

## 16. 旧代码到新代码的映射

| 旧代码 | 新归属 | 处理方式 |
|---|---|---|
| `session/model.rs::SessionData` | `domain::SessionSpec` + Batch/Event | 删除覆盖式聚合模型 |
| `session/store.rs::SessionStore` | `store::DirectoryControlStore` | 整体替换 |
| `context.rs::AgentContext` | `core::ContextProjection` | 改为临时只读视图 |
| `agent.rs::Agent` | `core::AgentCore` + `control::SessionRunner` | 拆分执行与协调 |
| `builder.rs::AgentBuilder` | Runner construction | 删除 Session 恢复职责 |
| `event.rs::AgentEvent` | `domain::SessionEvent` + protocol event | 区分事实与界面事件 |
| `sender.rs::AgentSender` | Control commands | 由 Activation API 替换 |
| `wasm/manager.rs` | `hooks/manager.rs` | 迁移并收紧 payload |
| `wasm/runtime.rs` | `hooks/runtime.rs` | 改为 capability-scoped instance |
| `wasm/types.rs` | `hooks/protocol.rs` | 对齐新 Hook contract |
| `cli/handler.rs` | Control Plane client | 删除直接 Agent/Store 操作 |

## 17. 关键风险与处理顺序

### 17.1 新 Store 完成但没有纵向入口

风险：Domain 和 Store 做得很完整，却在接入 Agent 时发现接口不合适。

处理：Phase 3 后立即实施本地 Control 纵向链路，不等待 daemon 和 CLI。

### 17.2 Hook 与权限同时改造

风险：WIT、Hook payload、实例生命周期和权限错误混在一起，无法定位回归。

处理：先迁移 Hook 并保持行为，再单独加入 Permission Controller 和 capability isolation。

### 17.3 新旧 Session 语义长期共存

风险：维护者无法判断某个命令会写哪种格式。

处理：新路径只由测试和新 ControlService 使用；CLI 切换时删除旧写入口，不提供运行时选择开关。

### 17.4 权限快照与实时配置混用

风险：Runner 执行中重新读取 TOML，历史无法解释。

处理：只有 PermissionController 解析配置；Runner 和 Extension 只持有 PermissionSnapshot Ref。

### 17.5 工具副作用自动重放

风险：daemon 崩溃后重复执行命令、网络请求或文件写入。

处理：不自动恢复未知副作用 continuation；遗留 active Activation 标记为 interrupted。

## 18. 完成定义

只有同时满足以下条件，架构改造才算完成：

1. Session 使用不可变 Spec、追加式 Batch 和追加式 Event。
2. StoreWriter 是事实文件的唯一写入者。
3. `status.json` 和内存索引可以从事实重建。
4. AgentCore 不知道 Store、Session 路径和前端。
5. SessionSource 支持固定范围、零复制和 DAG 血缘。
6. Runner 支持单 Session 串行、跨 Session 并行和取消。
7. Extension 不直接写 Store，且只获得已授权 capability。
8. 文件、HTTP 和命令工作目录权限完整接入。
9. Permission Interaction 可以由任意前端回答。
10. CLI、TUI、WebUI 都只依赖 Control Plane。
11. 三个现有 WASM 扩展在新架构下通过验证。
12. 旧 Session、旧 Store 和旧 CLI 直接执行路径已经删除。
13. 仓库中不存在旧格式兼容、双写或隐式 fallback。
14. 所有最终验证命令和端到端场景通过。

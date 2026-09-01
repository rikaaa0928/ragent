# ragent 架构改造实施路线图

状态：Draft

版本：0.2

日期：2026-09-01

关联文档：

- [产品需求文档](./PRODUCT_REQUIREMENTS.zh-CN.md)
- [Prototype 技术设计](./TECHNICAL_DESIGN_PROTOTYPE.zh-CN.md)
- [完整技术方案](./TECHNICAL_DESIGN.zh-CN.md)

## 1. 路线图目标

本路线图将架构改造分成两个连续交付段：

```text
当前实现
  → Prototype：框架明确、基础可用、单进程纵向闭环
  → Full：并发可靠、权限完整、服务化、多前端与派生能力完备
```

Prototype 不是旁路实现。它必须直接使用最终的 Domain、ControlStore、ControlService、AgentCore 和 Hook 边界；Full 阶段只在这些边界内增加能力和收紧保证。

出现冲突时：

- Prototype 阶段范围以 Prototype 技术设计为准。
- Full 阶段目标以完整技术方案和 PRD 为准。
- 本文只定义实施顺序和阶段出口，不重新定义领域语义。

## 2. 总体改造规则

### 2.1 先纵向、后横向

优先打通一条可运行链路：

```text
session.create
  → session.run
  → Input Batch
  → Context Projection
  → AgentCore
  → ModelOutput Batch
  → optional ToolOutput Batch
  → ActivationCompleted
```

只有这条链路通过真实 CLI 和重启验收后，才开始多 Session 并发、完整权限、daemon 和前端工作。

### 2.2 不兼容旧 Session

- 不读取或迁移旧 `.ragent/sessions/*.json`。
- 不提供旧 API adapter、双写或运行时格式开关。
- 新旧代码可以为迁移短期同时编译，但同一个命令只能走一条写路径。
- Prototype CLI 切换时删除旧 Session 写入口。

### 2.3 每阶段都可验证

每个阶段结束时：

1. 阶段新增行为有测试。
2. 已迁移路径不回退旧语义。
3. 仓库可编译，既有无关能力没有被静默破坏。
4. `cargo fmt --all -- --check`、相关测试和严格 Clippy 通过。
5. 当前阶段使用的文档、公开类型和 CLI help 一致。

### 2.4 控制提交范围

推荐一个阶段一个或少量聚焦提交。禁止在同一提交里同时引入 SQLite、重写 WASM ABI、增加 Unix Socket、实现权限交互并切换 CLI。

### 2.5 不创建空架构

目录和抽象在第一个真实调用者出现时创建。Prototype 不提交空的 Unix transport、PermissionController、RunnerRegistry、TUI 或 HTTP Adapter。

## 3. 保留、替换和延期

### 3.1 保留并迁移

- `openresponses-rust` 非流式请求和响应处理。
- Tokio 和 CancellationToken。
- WASM Component、WIT、WASI P2 基础。
- `shell`、`file_editor`、`image_viewer` 三个 Extension。
- 有效的全局/项目配置解析逻辑和测试素材。
- 模型、Hook、Extension 的现有回归样例。

### 3.2 Prototype 内整体替换

- `SessionData` 覆盖式聚合。
- `SessionStore::save`。
- 持久链路中的 `replace_items`、`clear_history` 和 `context.commit`。
- `AgentBuilder::from_session` 的恢复职责。
- CLI 直接加载 Session、创建 Agent 和覆盖写文件的路径。

### 3.3 Full 阶段再加入

- 多来源 SessionSource、fork、derive 和 spawn。
- StoreWriter thread、writer lock、持久幂等、备份和深度恢复。
- Runner 队列、跨 Session 并发、steer 和 follow-up。
- 完整 Permission Snapshot、Interaction 和 capability isolation。
- Unix Socket、ragentd、事件订阅、TUI 和 WebUI。

## 4. 目标模块演进

Prototype 实际需要：

```text
src/
  core/
  domain/
  store/
  control/
  hooks/
  cli/
```

Full 按需增加：

```text
src/
  control/permission_controller.rs
  control/registry.rs
  protocol/
  transport/unix.rs
  bin/ragentd.rs
  bin/ragent-tui.rs
  bin/ragent-http.rs
web/
```

保持单个 Cargo package，直到真实复用或构建边界证明需要拆分。

## 5. Prototype 阶段总览

| 阶段 | 可观察结果 | 是否阻塞下一阶段 |
|---|---|---|
| P0 基线与护栏 | 明确旧行为和新 Store 隔离 | 是 |
| P1 最小领域与 SQLite | 可以创建和读取空 Session | 是 |
| P2 无工具纵向闭环 | CLI 可完成一次模型对话 | 是 |
| P3 Tool/Hook 闭环 | 三个 Extension 进入新路径 | 是 |
| P4 CLI 切换与恢复 | 新路径成为唯一入口，重启可续写 | 是 |
| P5 Prototype 验收 | 发布基础可用 Prototype | 是，之后进入 Full |

## 6. P0：冻结基线与建立护栏

### 6.1 工作项

- 记录当前非流式模型请求的最小成功样例。
- 记录三个 Extension 的 metadata、tools、关键成功/失败输出和 shutdown 行为。
- 记录配置合并、timeout=0 和 Open Responses Item round-trip 行为。
- 确认新 Store 固定为 `.ragent/store/control.sqlite3`。
- 增加防护测试，确保新路径不写 `.ragent/sessions/`。
- 固定 Prototype schema version 和不支持版本的拒绝行为。
- 标注现有未迁移入口，避免误认为已经完成。

### 6.2 验收门槛

- 基线测试可以重复运行。
- 三个 Extension 均可构建和加载。
- 新 Store 路径不会覆盖旧 Session。
- 没有兼容器、导入器或双写代码。

## 7. P1：最小领域模型与 SQLite Store

### 7.1 目标

先实现可支撑纵向链路的最小事实模型，不一次性完成完整控制面。

### 7.2 领域范围

- ID 与 `BatchSeq`、`LocalItemSeq`、`EventSeq` newtype。
- 不可变 `SessionSpec`，其中预留序列化稳定但 Prototype 必须为空的 `sources`。
- 原生 `openresponses_rust::Item` 的 `ItemBatch`。
- Prototype 所需 `SessionEvent` 和 `SessionStatus`。
- `ConfigRevision` 和 `WorkspaceSpec`。

此阶段固定 `SessionSource` 的数据结构，但不实现非空 Source 的解析和创建；PermissionSnapshot 和 Interaction 延后。

### 7.3 Store 范围

实现：

```text
store_meta
configs
workspaces
sessions
batches
events
session_status
```

工作项：

- 使用 `rusqlite`，启用 foreign keys、WAL 和 busy timeout。
- 定义面向业务事务的 `ControlStore`，不暴露通用 SQL API。
- 实现 Session + SessionCreated + 初始投影原子创建。
- 实现 Input/ModelOutput/ToolOutput Batch 与引用 Event 的原子提交方法。
- 在事务内分配 Session-local sequence。
- 为 Session、Batch、Event 和 Item range 建立约束和索引。
- 实现按 Batch 顺序读取本地 Items。
- 实现从 Spec、Batch 和 Event 重建 `session_status`。
- 实现 WorkspaceSpec 和内容寻址 ConfigRevision 的确保写入及引用校验。
- 所有测试使用临时 SQLite 数据库。

### 7.4 验收门槛

- 可以创建、列出和读取空 Session。
- SessionSpec 没有 update API。
- Batch/Event 提交要么全部可见，要么全部不可见。
- 清空 `session_status` 后可以重建。
- 原生 Item JSON round-trip，不存在第二套 StoredMessage。
- 当前阶段不需要 StoreWriter thread 或多进程并发。

## 8. P2：无工具纵向闭环

### 8.1 目标

用真实模型接口完成一次无 Tool 的 Session run，尽早验证 Store 与 AgentCore 的接口是否合适。

### 8.2 工作项

- 从现有 `Agent` 提炼 `AgentCore` 的非流式模型 I/O。
- 实现 Prototype Context Projection：按 Batch/Item 顺序拼接本地历史。
- System Prompt 只进入 `request.instructions`。
- AgentCore 接收 Projection 和 Config，返回完整 response output Items 和 Usage。
- 实现进程内 `ControlService` 和 `SessionRunner`。
- 实现 `session.create/get/list/run`、`context.read` 和 `events.read`。
- `session.run` 提交 Input Batch、运行模型、提交 ModelOutput Batch 和最终 Event。
- 设置最大 Turn 数和当前进程 Ctrl-C 取消。
- 启动时把没有最终 Event 的 Activation 标记为 interrupted。

### 8.3 验收门槛

- CLI 测试入口可以完成一轮真实或契约级非流式请求。
- Input 和 ModelOutput 都以原生 Item Batch 保存。
- 第二轮请求能读取第一轮历史并只追加新 Batch。
- AgentCore 测试不打开 SQLite、不访问 CLI。
- 进程中断后不自动重放模型请求。

## 9. P3：Hook 与 Tool 闭环

### 9.1 Hook 迁移

- 保留 metadata → initialize → invoke → shutdown 生命周期。
- 将 `context.commit` 替换为 `context.append.prepare`。
- Hook 只能变换本次候选 Items，不能接收或返回完整历史 `next`。
- Model response 和 Tool result 使用原生 Open Responses Item。
- Extension 不得调用 ControlStore。
- 只迁移现有闭环实际使用的 Hook；其他 Hook 在 Full 阶段按需求加入。

### 9.2 Tool loop

- 从 ModelOutput 中识别 FunctionCall。
- 按输出顺序串行执行 Tool。
- 工具成功和失败都生成合法 FunctionCallOutput。
- ToolOutput Batch 与 ToolCallFinished 在同一事务提交。
- 回到模型，直到得到最终 Message 或达到最大 Turn 数。
- Tool 副作用发生但输出未提交时，Activation 进入 interrupted，不自动重试。

### 9.3 静态 Prototype Policy

- CLI 要求用户显式确认规范化 Workspace root。
- 文件读写限制在 Workspace 和 Session 临时目录。
- command 的结构化 `work_dir` 必须位于允许目录。
- HTTP 默认拒绝。
- 权限判断经过独立策略接口，Extension 不自授权。

### 9.4 验收门槛

- shell、file_editor、image_viewer 在新 HookManager 下通过关键入口测试。
- shell 覆盖成功、失败和 `timeout=0`；file_editor 覆盖写入和唯一匹配替换；image_viewer 覆盖有效图片、非法图片和大小限制。
- 成功、失败、取消路径都调用 Extension shutdown。
- FunctionCallOutput 的 `call_id` 能匹配已有 FunctionCall。
- Hook 无法替换已提交历史。
- Store 和 ControlService 不依赖具体 Extension 实现。

## 10. P4：CLI 切换与最小恢复

### 10.1 CLI 命令

至少完成：

```text
ragent session create
ragent session list
ragent session show
ragent session run
ragent session history
```

CLI 在进程内调用 ControlService，不直接执行 SQL，不直接创建 AgentCore。

### 10.2 切换阶段删除

- 旧 Session CLI handler。
- `AgentBuilder::from_session` 的 Session 恢复入口。
- `SessionStore` 的覆盖式写入口。
- 持久链路中的 `replace_items` 和 `clear_history`。
- `context.commit` Hook 与完整 `next` 语义。
- 从公开 API 导出的旧 Session 写能力。

旧文件可以在前一提交短期编译，但 CLI 切换后不得保留可达的旧写路径。

### 10.3 恢复检查

- 校验 Store schema version。
- 校验 Session 内 Batch/Event sequence 连续。
- 重建缺失或落后的 `session_status`。
- 将未结束 Activation 标记为 interrupted。
- 遇到非法 JSON 或引用不变量失败时拒绝继续写对应 Session。

### 10.4 验收门槛

- CLI 退出、重启后可以看到相同历史并继续追加。
- 新 CLI 不生成旧 Session JSON。
- 新路径没有 runtime fallback 或双写。
- 未知工具副作用不会被自动重放。

## 11. P5：Prototype 发布门槛

### 11.1 端到端场景

1. 创建 Workspace、Config 和 Session。
2. 提交用户 Message。
3. 模型返回 Reasoning 和 FunctionCall。
4. Extension 在 Workspace 内执行工具。
5. ToolOutput 以 FunctionCallOutput 提交。
6. 模型返回最终 Message。
7. 退出进程。
8. 清空或落后 `session_status`。
9. 重启并得到相同历史和状态。
10. 再运行一轮，确认历史只追加。

### 11.2 验证命令

```text
cargo fmt --all -- --check
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
./scripts/build-extensions.sh
```

### 11.3 Prototype 完成定义

- 不可变 Spec、追加式 Batch/Event 和原生 Item 已成为唯一 Session 语义。
- SQLite 是唯一事实源。
- AgentCore、ControlService、Store 和 Hook 边界已经真实被 CLI 使用。
- 三个现有 Extension 可用。
- 单进程退出和重启恢复通过。
- Prototype 技术设计中的所有必测项通过。

达到这些条件后打 Prototype 里程碑。不要因为 Full 阶段尚未完成而继续向 Prototype 阶段塞入并发或前端功能。

## 12. Full 阶段总览

| 阶段 | 在 Prototype 上增加的能力 |
|---|---|
| F1 Store 可靠性 | Writer thread、幂等、writer lock、备份、完整性审计 |
| F2 Session 血缘 | 固定范围 Source、多来源、fork、derive、spawn |
| F3 异步调度 | 队列、跨 Session 并发、取消、steer、follow-up |
| F4 完整权限 | 快照、信任、Interaction、scoped capability |
| F5 服务化协议 | ragentd、Unix Socket、JSONL、订阅恢复 |
| F6 多前端 | CLI client、TUI、HTTP/SSE、WebUI |
| F7 清理发布 | 全仓审计、故障注入、最终验收 |

F1 和 F2 可在模块边界清晰后独立推进，但 F3 依赖 F1 的写入与幂等保证；F4 必须在 Extension 对外能力扩大前完成；F5 依赖 F1 和 F3；F6 依赖 F5。

## 13. F1：补齐 Store 可靠性

### 13.1 工作项

- 增加专用 blocking StoreWriter thread 和 bounded channel。
- 写连接前获取 `writer.lock` advisory exclusive lock。
- 增加 `command_results`，保存 Command ID、幂等键、request hash 和首次结果。
- 所有多事实命令使用同一事务并写入幂等结果。
- 增加 expected tail 检查和冲突错误。
- 增加 `foreign_key_check`、`quick_check`、投影游标检查和离线 `integrity_check`。
- 增加 Strict/Balanced durability 配置。
- 增加一致备份入口，不在运行中只复制主数据库文件。
- 读查询和大 JSON 解析移到受控 blocking worker。

### 13.2 验收门槛

- COMMIT 后丢失响应再重试，不产生重复事实并返回首次结果。
- 两个 writer 不能同时写 Store。
- 未 COMMIT 的多行事实全部不可见，已 COMMIT 的全部可见。
- 清空投影后可重建一致状态。
- 故障注入覆盖事务开始、各插入点、COMMIT 前后和 broadcast 前。

## 14. F2：Session 血缘与 Context Projection

### 14.1 工作项

- 开放 SessionSpec 中已有的有序 `SessionSource` 字段，允许创建非空 Source。
- 增加 `ContextPos` 和固定闭区间 Source。
- 增加 `session_sources` 表、正反关系索引和无环校验。
- 将 Projection 从“本地 Items”扩展为“递归 Source slices + 本地 Items”。
- 实现 selector 到固定范围的创建时解析。
- 实现零复制 fork、多来源 derive、children 查询。
- 实现结构化 `session.spawn` Command 和 ProducerRef。
- 压缩通过创建派生 Session 表达，不修改父历史。
- 补齐 Session close、archive、children 和只读历史查询；正常在线 API 仍不删除事实。

### 14.2 验收门槛

- Source 范围不随父 Session 后续增长。
- 多 Source 严格保持声明顺序。
- fork 不复制父 Batch。
- 重叠 Source 不隐式去重。
- 循环引用和越界范围被拒绝。

## 15. F3：异步 Activation 与并发

### 15.1 工作项

- `session.run` 内部能力拆为 `activation.submit/get/cancel/steer/follow_up`。
- submit 提交事实后立即返回 Activation ID。
- 增加 Session FIFO 队列和 RunnerRegistry。
- 保证单 Session 最多一个 active Activation。
- 增加全局 Runner 并发上限和背压。
- 不同 Session 可以并行，同一 Session 串行。
- 取消必须指向准确 Activation 并贯穿模型、Hook 和 Tool 边界。
- steer 只进入 active Activation；结束后不得静默变成 follow-up。
- 恢复时遗留 active Activation 进入 interrupted，不自动重放未知副作用。
- 按完整技术方案补齐 Runner 生命周期 Hook 和结构化运行事件，但不让 Hook 获得 Store 写能力。

### 15.2 验收门槛

- 同 Session 两个 Activation 不并行。
- 不同 Session 在限制内并行。
- 队列满时产生背压，不丢命令。
- cancel、steer、follow-up 的边界测试通过。
- Tool 副作用未知时不会自动恢复执行。

## 16. F4：完整权限与人工交互

### 16.1 纯权限解析

- 实现 file read、file write、HTTP、command work-dir Rule。
- 实现 global → extension → project → extension → session → extension 层级。
- 保留 `None` 与显式空列表差异。
- 固定 deny → allow → default 判断顺序。
- 生成内容寻址 PermissionSnapshot，并由 Activation 固定引用。
- 实现 `permission.explain`。

### 16.2 项目目录信任

- 增加 `trusted.toml`。
- 未信任时只检查项目配置存在性，不读取内容。
- 用户信任后才读取已有项目配置。
- 没有配置时只初始化最小 Workspace allowlist。
- 已有配置不得被自动覆盖。
- 新 Session 通过追加权限事实获得 Session 临时目录的最小读、写和 command work-dir 规则。

### 16.3 Interaction

- 增加 PermissionRequested/PermissionResolved Event。
- 实现 waiting_for_interaction 状态。
- 实现 Deny、AllowOnce、AllowSession、AllowProject、AllowAlways。
- 同一请求用 compare-and-append 只接受一个回答。
- 前端断线后请求保持；daemon 重启后不恢复未知 continuation。

### 16.4 Capability 交付

```text
permission.requirements
  → PermissionController
  → scoped WASI P2 / Host capability
  → tools.call
```

- requirements 阶段无环境能力。
- 文件按读写分别提供精确 preopen。
- HTTP 只开放批准的 scheme/host/port/method，不开放任意 socket。
- command 经 Host Action 执行并校验结构化 `work_dir`。
- 未声明的实际能力调用由 Host 拒绝。

### 16.5 验收门槛

- 所有能力覆盖 allow、deny、ask。
- 五种 Decision 落到正确作用域。
- 多前端竞争回答只有一个成功。
- Extension 不再获得默认全 cwd ambient capability。
- PermissionSnapshot 能解释每个最终字段来源。

## 17. F5：Unix Socket、协议与 ragentd

### 17.1 工作项

- 定义带 `protocol_version` 和 `request_id` 的 JSONL Envelope。
- 进程内和 Unix transport 共用同一个 ControlService。
- 实现 Session、Context、Activation、Event、Config、Workspace、Interaction 方法。
- 实现 Event subscribe 和 `after_seq` 恢复。
- 慢订阅者断开，不阻塞 Runner；事实可从 Store 补读。
- Socket 默认仅当前用户可访问。
- ragentd 独占 writer，客户端不直接打开 SQLite 写连接。

### 17.2 验收门槛

- daemon 独立运行时可创建和执行 Session。
- 客户端退出不取消 Activation。
- 重连可按 EventSeq 恢复。
- 协议版本不兼容时明确拒绝。
- daemon 重启后事实和投影一致。

## 18. F6：CLI 客户端、TUI 与 WebUI

### 18.1 CLI 二次切换

Prototype CLI 的用户命令保持，内部从进程内 Service 切换为 Control Plane client。可保留嵌入式 daemon 便利模式，但仍调用同一 Service，不恢复直接 Agent/Store 路径。

### 18.2 TUI

- 只保存选择、滚动、折叠和输入草稿。
- 使用相同协议观察 Session、Activation、Event 和 Permission。
- 断线后按 EventSeq 恢复。

### 18.3 WebUI

- 独立 `ragent-http` Adapter 映射 HTTP command/query 和 SSE Event。
- 默认只监听 loopback。
- 远程监听必须显式配置认证。
- 浏览器不直接连接 SQLite。

### 18.4 验收门槛

- CLI、TUI、WebUI 同时观察时状态一致。
- 任一前端退出不影响 Runner。
- 未知 Item/Event 有 fallback 展示。
- 两个前端回答同一 Interaction 时只有一个成功。

## 19. F7：清理、审计与发布

### 19.1 旧代码审计

确认没有引用后删除或完全替换：

```text
SessionData
SessionStore
AgentBuilder::from_session
context.commit
replace_items / clear_history 持久语义
旧 CLI direct-run handler
旧 Session JSON 写路径
默认 full-cwd preopen
```

同名文件可以承载新实现；判断标准是旧类型和旧语义不可达，而不是机械删除路径。

### 19.2 全仓一致性搜索

- 旧 Session 路径、格式和 API。
- Directory Store、事实文件 rename 和 Batch/Event 补写术语。
- 绕过 StoreWriter 的 SQLite 写连接。
- Extension 直接访问 Store 或 Control socket。
- `stream=true`、`background=true` 和已删除 streaming Hook。
- CLI/TUI/WebUI 直接持有唯一运行状态。
- 文档中把 Prototype 放宽项误写为最终保证的描述。
- 未经离线 maintenance 前置检查就物理删除 Session 事实的路径。

### 19.3 最终验证

除 P5 命令外，完成：

- Store 故障注入和幂等重试测试。
- daemon 重启与 EventSeq 恢复测试。
- 固定范围、多来源和 spawn 测试。
- 三个 Extension 真实入口和 scoped capability 测试。
- 五种权限回答端到端测试。
- 多客户端观察、取消和 Interaction 竞争测试。
- 清空 `session_status` 后的投影一致性测试。

## 20. 旧代码到新代码的迁移映射

| 当前代码 | Prototype 归属 | Full 扩展 |
|---|---|---|
| `session/model.rs::SessionData` | `domain::SessionSpec` + Batch/Event | 增加 Source 和完整状态投影 |
| `session/store.rs::SessionStore` | `store::SqliteControlStore` | StoreWriter、幂等、备份 |
| `context.rs::AgentContext` | `core::ContextProjection` | 多来源 segment projection |
| `agent.rs::Agent` | `core::AgentCore` + `control::SessionRunner` | 异步调度与取消 |
| `builder.rs::AgentBuilder` | Runner construction | Config/Permission Snapshot 装配 |
| `event.rs::AgentEvent` | `domain::SessionEvent` | Protocol Event |
| `sender.rs::AgentSender` | 进程内 Control commands | Unix client commands |
| `wasm/manager.rs` | `hooks::manager` | 完整 Hook 和 PermissionController |
| `wasm/runtime.rs` | `hooks::runtime` | per-call scoped capability |
| `wasm/types.rs` | `hooks::protocol` | 稳定 Extension protocol |
| `cli/handler.rs` | 进程内 ControlService client | ragentd client |

## 21. 关键风险与停止条件

### 21.1 水平模块很多但没有可运行入口

处理：P1 之后立即进入 P2。无工具纵向闭环没有通过前，不建设完整 Event 枚举、权限系统或协议层。

### 21.2 Prototype 变成第二套架构

处理：Prototype 必须使用不可变 Spec、追加 Batch/Event、原生 Item、ControlStore 和 AgentCore。任何为了“先跑起来”恢复覆盖式 Session 的方案停止实施并回到设计评审。

### 21.3 Hook 与权限同时重写

处理：P3 先在静态策略下迁移 Hook 行为；F4 再加入完整权限解析、Interaction 和 scoped capability。

### 21.4 CLI 切换后旧路径仍可写

处理：P4 的同一个切换阶段删除旧公开写入口，并用搜索和集成测试证明新 CLI 不生成旧格式。

### 21.5 并发提前扩散

处理：Prototype 全局串行。只有 P5 稳定后才在 F1/F3 中增加 Writer thread、队列和跨 Session 并行。

### 21.6 工具副作用自动重放

处理：任何阶段都不自动恢复结果未知的 ToolCall；遗留 Activation 标记为 interrupted。

## 22. 最终完成定义

架构改造分两次判定完成。

### 22.1 Prototype 完成

满足 P5 和 Prototype 技术设计的完成定义，即可作为“框架明确、基础可用”的版本交付。

### 22.2 Full 完成

只有同时满足以下条件，完整版才完成：

1. Session 使用不可变 Spec、追加式 Batch/Event 和原生 Open Responses Item。
2. StoreWriter 是 SQLite 的唯一写入者，事务、幂等、恢复和备份通过验证。
3. `session_status` 和所有缓存可以从事实重建。
4. AgentCore 不知道 Store、Session 路径和前端。
5. SessionSource 支持固定范围、零复制、多来源和无环血缘。
6. Runner 支持单 Session 串行、跨 Session 并行、取消、steer 和 follow-up。
7. Extension 不直接写 Store，且只获得已授权 capability。
8. 文件、HTTP、命令工作目录权限和五种人工回答完整接入。
9. CLI、TUI、WebUI 都只依赖 Control Plane。
10. 三个现有 Extension 在新架构和权限路径下通过验证。
11. 旧 Session、旧 Store 和旧 CLI 直接执行路径已经删除。
12. 仓库不存在兼容、双写、隐式 fallback 或过期架构术语。
13. 完整技术方案中的故障、并发、安全和端到端验收全部通过。

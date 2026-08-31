# ragent 产品需求文档

状态：Draft

版本：0.1

日期：2026-08-31

## 1. 产品摘要

ragent 是一个轻量、可扩展、以 Session 为核心的本地 Agent 运行系统。

它不把 Agent 进程视为长期状态持有者，而是将 Session 的定义、Open Responses 上下文、生命周期事件和派生关系保存在统一的本地 Control Store 中。Agent Runner、CLI、TUI、WebUI 和扩展都围绕这份事实数据工作，可随时启动、退出和恢复。

产品的基础构想类似一个被大幅简化的 Kubernetes：

- Session 是最小工作台，类似 Pod。
- Directory Store 是唯一事实源，承担轻量的控制面存储职责。
- Session Runner 是可重建的执行器。
- 生命周期 Hook 提供扩展点。
- CLI、TUI 和 WebUI 是无状态客户端。
- Session Runner 可以通过普通目录文件自由协作，但这类文件交互不进入控制面建模。

首要目标不是分布式能力或极致吞吐，而是保持核心足够小、数据足够直观、执行过程可以恢复、未来功能能够在不破坏核心边界的前提下自然扩展。

## 2. 产品愿景

让用户能够用一个小而稳定的 Agent 内核，组合出不同类型的长期工作流：

- 从 CLI 发起一次简单任务。
- 在 TUI 中管理多个长期 Session。
- 在 WebUI 中观察运行状态和历史。
- 从任意一段上下文 fork 新 Session。
- 通过新 Session 完成压缩、总结、审查或派生任务。
- 允许 Agent 在某一轮执行中请求创建新 Session。
- 让多个 Session Runner 通过共享工作目录协作。
- 通过 Hook 和外部 Controller 扩展工具、策略、权限、存储观察和生命周期行为。

无论功能如何增加，ragent 核心都只维持模型 I/O、上下文装配、执行循环和最小控制面协议。

## 3. 设计原则

### 3.1 轻量优先

- 默认部署是一个本地进程和一个普通目录。
- 第一版不依赖 SQLite、PostgreSQL、etcd、消息队列或分布式协调服务。
- 不为尚未出现的规模问题提前引入复杂基础设施。
- 不为了形式上的通用性建设 CRD、通用资源引擎或任意 JSON 工作流系统。

### 3.2 单一事实源

- Session 的不可变定义、上下文追加批次、生命周期事件和配置修订都保存在 Directory Store。
- Runner 和前端不保存不可恢复的业务状态。
- 可变的状态文件仅作为缓存，删除后必须能够从事实数据重建。
- 同一份配置只保存一次，其余数据通过内容寻址引用。

### 3.3 上下文只追加

- Session 的 Open Responses Item 历史只允许追加，不允许覆盖、删除或重排。
- 需要删除、替换或重构历史时，创建新的派生 Session。
- 模型请求可以基于历史构建临时 Context Projection，但 Projection 不修改原始历史。

### 3.4 Open Responses 原生数据优先

- Open Responses `Item` 是 Session 上下文的唯一核心数据结构。
- 不再建立同构的 Message、Reasoning、ToolCall 或 ToolResult 持久化模型。
- 模型请求使用 `CreateResponseBody`，模型响应使用 `ResponseResource`，Token 用量使用 `Usage`。
- ragent 只补充 Session ID、序号、批次边界、生命周期和派生关系等 Open Responses 没有定义的信息。

### 3.5 进程无状态，Session 有状态

- Agent Runner 可以退出并由另一个 Runner 重建。
- CLI、TUI 和 WebUI 可以断线重连。
- 同一 Session 同时只允许一个 active Activation。
- 不同 Session 可以并行执行。

### 3.6 显式血缘，非侵入式协作

- fork、derive、summary、spawn 等 Session 血缘必须显式保存并可回溯。
- Session 之间通过共享目录文件协作时，不记录发布、消费或读取流程。
- 共享目录交互不会自动建立 Session 关系。

### 3.7 扩展不破坏历史

- Hook 可以变换待提交数据或临时模型视图。
- Hook 不得修改已提交的 Item。
- Hook 不得直接写 Control Store。
- Extension 发起控制面动作时，必须提交结构化命令，由 Controller 校验并执行。

## 4. 目标用户

### 4.1 本地 Agent 用户

希望以 CLI、TUI 或 WebUI 管理长期任务，同时保留完整上下文和派生历史。

### 4.2 Agent 工作流开发者

希望通过 Hook、工具扩展和 Session 派生组合复杂流程，而不修改核心循环。

### 4.3 前端开发者

希望独立实现 TUI、WebUI 或其他界面，只依赖稳定的 Control Plane 协议。

### 4.4 扩展开发者

希望通过 WASM Component 实现工具、请求变换、策略、观察和生命周期行为。

## 5. 核心术语

### Session

持久化的最小 Agent 工作台，由不可变 Session Spec、追加式 Item Batch、追加式 Event 和派生来源共同定义。

### Activation

Session 的一次外部激活，通常由一次用户提交、Controller 请求或父 Session spawn 请求触发。一个 Activation 可以包含多个模型 Turn 和工具调用。

### Turn

一次完整的模型请求、模型响应和其后工具执行阶段。

### Item Batch

一次原子追加的一组 Open Responses Item。Batch 是 Directory Store 的最小上下文提交单位。

### Context Projection

根据 Session 来源、当前 Item 历史和 Hook 临时构建的模型输入视图。Projection 可以选择或变换 Item，但不修改事实历史。

### Session Source

新 Session 创建时冻结的上下文来源，指向已有 Session 的一个确定范围。

### Controller

观察 Control Store 中的目标和状态，并通过追加事实推动 Session 生命周期的组件。

### Runner

领取 Activation、装配上下文、运行 Agent loop 并提交结果的执行器。

### Workspace

Runner 可访问的普通目录。多个 Session 可以引用同一 Workspace 并通过文件协作。

### Permission Request

Runner 或 Extension 在执行文件访问、HTTP 请求或命令前提交的结构化能力请求。请求必须明确资源和发起者，不能只表达“需要更多权限”。

### Permission Snapshot

一次 Activation 实际使用的不可变权限解析结果。它由全局、项目、Session 和 Extension 级配置合并产生，并以内容引用固化到事实数据中。

## 6. 产品范围

### 6.1 产品必须提供

- 创建、读取、列出、关闭和归档 Session。
- 为 Session 提交 Activation。
- 以 Open Responses Item Batch 形式追加上下文。
- 从固定上下文范围 fork 或派生 Session。
- 查询 Session 的正向和反向血缘关系。
- 运行非流式 Open Responses 模型请求。
- 调用扩展提供的工具和 Hook。
- 取消正在运行的 Activation。
- 管理文件、HTTP 网络和命令工作目录三类权限。
- 在无法自动决定权限时暂停 Activation，并通过任意前端完成统一的人工询问。
- 管理项目目录信任，并固化每次 Activation 实际使用的权限快照。
- 观察 Session 和 Activation 的生命周期事件。
- 使用共享目录作为 Runner 的协作数据面。
- 提供 CLI 和稳定的本地 Control Plane API。
- 支持后续实现无状态 TUI 和 WebUI。

### 6.2 第一阶段不提供

- 旧版 Session 文件迁移或兼容。
- 多机器调度和分布式一致性。
- etcd、数据库集群或消息队列。
- Session 间 Exchange、发布订阅和消费游标。
- 多进程直接写 Directory Store。
- 自动追踪共享目录中的 Session 交互。
- 模型 Token 流式输出。
- 通用工作流 DSL。
- 内置业务工具或内置 Agent 能力。
- 自动压缩算法；第一阶段只保证数据模型可以自然实现。

## 7. 核心用户场景

### 7.1 创建并运行 Session

1. 用户通过 CLI、TUI 或 WebUI 创建 Session。
2. 用户选择 Workspace、配置修订和可选来源。
3. Control Plane 创建不可变 Session Spec。
4. 用户提交输入，系统创建 Activation。
5. Runner 领取 Activation，装配 Context Projection 并运行 Agent。
6. 用户输入、模型输出和工具结果分别以 Item Batch 原子追加。
7. Activation 完成后，前端展示最终 Item 和 Usage。

### 7.2 恢复 Session

1. 用户重新打开客户端。
2. 客户端读取 Session Spec、状态投影和事件游标。
3. 客户端按需加载 Item Batch。
4. 如果 Session 有未完成 Activation，展示 running、cancelling 或 interrupted 状态。
5. Runner 重启后可以从事实数据重建可恢复状态。

### 7.3 从指定位置 fork

1. 用户选择父 Session。
2. 用户选择全部上下文、某个 Activation、某个 Turn 或明确范围。
3. Control Plane 将选择器解析为固定上下文范围。
4. 新 Session Spec 保存该固定 Source。
5. 新 Session 不复制父 Session Item。
6. Runner 装配新 Session 上下文时读取 Source，再追加新 Session 自身 Item。

### 7.4 压缩和派生

1. 用户或 Controller 从原 Session 创建压缩任务 Session。
2. 压缩 Session 读取原 Session 的固定上下文范围。
3. 压缩结果作为普通 Open Responses Item 保存在压缩 Session。
4. 系统再创建一个新工作 Session，将压缩结果和可选的原 Session 尾部范围作为 Source。
5. 原 Session 和压缩过程永久保留，可以完整回溯。

### 7.5 Session 在执行中请求创建 Session

1. Extension 或 Agent Action 生成 `session.spawn` 控制面命令。
2. 命令包含创建者 Session、Activation、Turn、调用位置、目标要求和 Source。
3. Session Controller 校验命令和权限。
4. Controller 以幂等方式创建子 Session。
5. 父 Session 记录 spawn 结果，子 Session Spec 记录 producer。

### 7.6 多 Session 文件协作

1. 多个 Session 引用同一 Workspace。
2. Runner 通过工具读写约定目录。
3. 文件交互不进入 Session 血缘和控制面事件。
4. 需要永久进入模型上下文的文件内容，通过正常工具结果或用户输入成为 Open Responses Item。
5. 需要可回溯的派生关系时，显式创建新 Session Source。

### 7.7 多前端观察

1. CLI、TUI 和 WebUI 可以同时连接 Control Plane。
2. 前端读取相同的 Session 数据和状态投影。
3. 前端断线不会取消 Activation。
4. 前端重新连接后使用事件序号恢复，或重新读取完整状态。

### 7.8 权限询问与持久授权

1. Runner 在执行文件、HTTP 或命令能力前产生结构化 Permission Request。
2. Permission Controller 按 Session、项目、全局和 Extension 覆盖规则解析权限。
3. 命中拒绝名单时直接拒绝，命中允许名单时直接通过；均未命中时按默认策略允许、拒绝或询问。
4. 需要询问时 Activation 进入 `waiting_for_interaction`，CLI、TUI 或 WebUI 均可展示同一请求。
5. 用户可以选择拒绝、本次同意、本 Session 同意、本项目同意或始终同意。
6. 持久授权写入对应作用域，Runner 继续执行；前端断线不影响等待状态。
7. 第一次使用项目配置前，系统必须先取得用户对项目目录的信任。

## 8. 功能需求

### 8.1 Session 管理

#### PRD-SESSION-001 创建 Session

系统必须支持创建具有以下内容的 Session：

- 唯一 Session ID。
- 创建时间。
- 固化的基础 System Prompt。
- 默认配置引用。
- Workspace 引用。
- 零个或多个有序 Session Source。
- 可选 labels。

#### PRD-SESSION-002 不可变 Spec

Session Spec 创建后不得修改。需要更换来源、System Prompt 或其他身份属性时必须创建新 Session。

#### PRD-SESSION-003 生命周期

Session 至少支持 open、closed、archived 三种产品状态。关闭后不得创建新 Activation；归档只影响默认展示，不删除历史。

#### PRD-SESSION-004 物理删除

物理删除必须是显式维护操作，不属于正常 Session 生命周期。删除前必须能够检测其他 Session 对该 Session 的 Source 引用。

### 8.2 上下文存储

#### PRD-CONTEXT-001 原生 Item

所有模型上下文必须以 Open Responses `Item` 保存，不得转换为 ragent 自定义消息模型。

#### PRD-CONTEXT-002 只追加

已提交 Item 不得覆盖、删除、替换或重排。核心不得暴露此类 API。

#### PRD-CONTEXT-003 原子 Batch

一次输入提交、模型输出或工具结果必须作为完整 Item Batch 原子保存。崩溃后 Batch 必须处于完整存在或完全不存在两种状态之一。

#### PRD-CONTEXT-004 稳定序号

每个 Session 的本地 Item 必须具有从 0 开始、连续递增且永久稳定的序号。Open Responses `Item.id` 不得作为本地序号或存储主键。

#### PRD-CONTEXT-005 Projection

系统必须支持在不修改历史的情况下构建临时 Context Projection。Projection Hook 只能影响当前模型请求。

### 8.3 Activation 和执行

#### PRD-RUN-001 异步提交

提交 Activation 后，Control Plane 必须立即返回 Activation ID，不等待模型完成。

#### PRD-RUN-002 单 Session 单运行

一个 Session 同时最多只能有一个 active Activation。额外提交必须进入队列或被明确拒绝，不得并行修改同一 Session。

#### PRD-RUN-003 跨 Session 并行

不同 Session 的 Activation 应当能够并行执行。

#### PRD-RUN-004 非流式模型响应

核心只支持完整的非流式 Open Responses 响应。前端可以实时显示生命周期和工具事件，但不得伪造 Token Delta。

#### PRD-RUN-005 取消

用户必须能够按 Activation ID 请求取消。取消应尽快终止模型、Hook 或工具工作，并追加明确的最终状态。

#### PRD-RUN-006 恢复

进程退出时尚未完成且无法自动确认结果的 Activation，重启后必须标记为 interrupted，不得自动重复具有外部副作用的操作。

### 8.4 Session 派生

#### PRD-LINEAGE-001 固定 Source

Session Source 必须在创建时解析为稳定范围，不能永久保存“最后一轮”之类会变化的选择器。

#### PRD-LINEAGE-002 多来源

Session 必须允许零个或多个有序 Source，以支持 fork、压缩、合并和多来源派生。

#### PRD-LINEAGE-003 零复制

创建派生 Session 时不得复制来源 Item。上下文装配时按 Source 读取。

#### PRD-LINEAGE-004 可回溯

系统必须能够查询：

- Session 的直接来源。
- 引用了某个 Session 的直接子 Session。
- 创建 Session 的 producer Session、Activation、Turn 和调用位置。

#### PRD-LINEAGE-005 无环

Session 来源图必须是 DAG。新 Session 只能引用创建时已经存在的稳定 Session 范围。

### 8.5 配置

#### PRD-CONFIG-001 内容寻址

配置必须规范化后按内容 hash 保存。同一份配置只保存一次。

#### PRD-CONFIG-002 执行配置固化

每个 Activation 必须记录实际使用的 Config Ref，保证后续配置变化不会改变历史含义。

#### PRD-CONFIG-003 接近 Open Responses

模型配置结构必须尽量贴近 `CreateResponseBody`，避免重复定义 model、temperature、reasoning 等同构字段。

### 8.6 Workspace

#### PRD-WORKSPACE-001 集中定义

Workspace 路径和基础权限应集中定义，Session 只保存 Workspace Ref。

#### PRD-WORKSPACE-002 文件协作

多个 Session 可以引用同一 Workspace。Control Plane 不追踪文件读取、写入、发布或消费流程。

#### PRD-WORKSPACE-003 临时目录

每个 Session 可以拥有独立临时目录。临时目录不是事实源，系统重启或清理后可能消失。

### 8.7 Hook 和 Extension

#### PRD-EXT-001 外部扩展

业务工具和策略必须通过外部 WASM Component Extension 提供，核心不捆绑内置业务扩展。

#### PRD-EXT-002 生命周期

Extension 使用版本化生命周期：metadata、initialize、invoke、shutdown。

#### PRD-EXT-003 Hook 语义

- Transform 按优先级顺序执行。
- Observer 不得修改主流程。
- 一个 Action 只能有一个 owner。
- 拒绝或停止必须产生明确结果。

#### PRD-EXT-004 历史保护

Extension 只能变换待追加 Item 或 Context Projection，不得修改 Store 中已提交历史。

#### PRD-EXT-005 控制面动作

Extension 创建 Session 或执行其他控制面动作时，只能返回结构化 Command，不得直接操作 Store 文件。

### 8.8 CLI、TUI 和 WebUI

#### PRD-UI-001 统一协议

所有前端必须使用同一 Control Plane 语义，不得各自实现 Session 生命周期。

#### PRD-UI-002 无状态

前端不得成为 Session 状态唯一持有者。关闭前端不得导致 Session 数据丢失。

#### PRD-UI-003 原生 Item 展示

前端应直接基于 Open Responses Item 渲染消息、reasoning、工具调用和工具结果，不要求核心生成第二套 View Message。

#### PRD-UI-004 大内容保护

工具输出和 reasoning 默认折叠或截断展示，用户可以显式展开。前端不得默认完整打印敏感或超大 Item。

#### PRD-UI-005 Session 图

TUI/WebUI 的后续版本应能够展示 Session 来源和派生关系，并允许从选定范围创建新 Session。

### 8.9 权限与人工交互

#### PRD-PERM-001 能力范围

权限系统必须覆盖以下三类能力：

- 文件读取和文件写入，分别判断。
- Extension 通过 WASI P2 发起的 HTTP 网络访问；第一版不支持任意 TCP/UDP socket。
- 命令执行时声明的工作目录。

#### PRD-PERM-002 命令工作目录

命令执行请求必须包含 `work_dir`，缺省为 Runner 当前工作目录；Runner 初始化时其当前工作目录为 Workspace root。第一版只校验该结构化参数是否位于允许目录，不解析命令文本中的 `cd`、绝对路径或其他绕过方式；界面和文档必须明确这不是完整的命令沙箱。

#### PRD-PERM-003 黑白名单

每类能力均使用拒绝名单、允许名单和默认策略：

1. 命中任一拒绝规则时直接拒绝。
2. 未命中拒绝规则且命中允许规则时直接允许。
3. 均未命中时执行默认策略 `allow`、`deny` 或 `ask`。

同一请求同时命中允许和拒绝规则时，拒绝优先。

#### PRD-PERM-004 配置层级

权限配置必须支持全局、项目和 Session 三个作用域，按 Session 高于项目、项目高于全局逐字段覆盖。每个作用域还可以为具体 Extension 提供覆盖项。

#### PRD-PERM-005 None 与空值

权限配置字段缺失表示继承下一层，显式空列表表示清空该字段，不得将两者合并为同一语义。Extension 专属字段以同样规则覆盖通用字段。

#### PRD-PERM-006 执行快照

Control Plane 必须在 Activation 开始前解析出不可变 Permission Snapshot，并记录其引用。运行中不得因外部配置文件变化而静默改变既有授权；新持久授权通过新快照后生效。

#### PRD-PERM-007 询问选项

Permission Request 进入人工询问时必须提供：

- 拒绝：拒绝当前请求，不改配置。
- 本次同意：只允许当前请求，不改持久配置。
- 本 Session 同意：追加 Session 级允许规则。
- 本项目同意：写入项目级允许规则。
- 始终同意：写入全局允许规则。

持久允许规则默认使用当前请求可表达的最窄资源范围，界面必须展示实际将写入的规则。

#### PRD-PERM-008 追加式 Session 授权

Session 级权限不得改写 Session Spec 或既有 Event。系统必须以追加式权限事件保存授权，并由投影形成 Session 权限层；这等价于 Session meta 的可恢复配置，但遵守上下文和事实只追加原则。

#### PRD-PERM-009 前端无关的交互

权限询问必须是 Control Plane 中可恢复的结构化交互。CLI、TUI 和 WebUI 使用相同 Request ID 回答；同时回答时仅第一个有效决议成功，其余返回已解决。前端断线不丢失请求；daemon 重启则遵循 active Activation 进入 interrupted 的统一恢复规则，不自动重放工具调用。

#### PRD-PERM-010 项目信任

项目配置的新建和首次读取必须先由用户信任当前项目目录。系统只允许在取得信任前探测配置文件是否存在，不得解析或应用其内容。

#### PRD-PERM-011 信任清单

全局 `trusted.toml` 必须保存已信任目录。未信任目录启动时必须展示规范化目录路径，并明确提示该目录是否已有 `.ragent/config.toml`；若已有，要求用户先检查其内容再确认。

#### PRD-PERM-012 项目配置初始化

用户信任一个尚无项目配置的目录后，系统必须初始化 `.ragent/config.toml`，默认仅允许当前项目目录的文件读写和命令工作目录。已有项目配置不得被自动覆盖。

#### PRD-PERM-013 Session 临时目录

创建 Session 时，系统必须在解析现有全局和项目配置后，将该 Session 的 `session_tmp` 目录作为文件读写和命令工作目录允许规则追加到 Session 权限层，并写入追加式事实。

#### PRD-PERM-014 扩展隔离

Extension 不得获得未声明的环境能力。文件访问和 HTTP 通过受限 WASI P2 capability 提供，命令执行通过受控 Host Action 提供；所有能力在交付给 Extension 前必须完成权限解析。

## 9. 非功能需求

### 9.1 可理解性

- 核心热路径中不得存在同构数据模型转换链。
- Store 中的 Open Responses Item 必须可以直接用普通 JSON 工具阅读。
- Directory Store 的事实文件命名和顺序必须确定且可检查。

### 9.2 可靠性

- 所有事实文件必须通过临时文件加原子 rename 提交。
- 进程崩溃不得产生半个 JSON Batch。
- 临时文件不得被当作已提交事实。
- 状态缓存损坏不得影响事实数据恢复。

### 9.3 安全性

- Store 根目录默认权限为仅当前用户可访问。
- Session、配置和事件文件默认权限为仅当前用户可读写。
- Extension 默认没有环境文件、网络或命令能力，只获得 Permission Snapshot 明确授予的 capability。
- 项目配置未经目录信任不得读取或应用。
- 权限询问、回答和持久授权必须可回溯，且不得依赖某个前端进程存活。
- 命令工作目录校验不是命令内容沙箱；第一版必须明确记录这一安全边界。
- 前端默认不得展示完整敏感工具输出。
- Web Adapter 默认只监听 loopback，并要求显式认证配置后才允许远程访问。

### 9.4 性能

- 单个 Session 的范围读取应按 Batch 顺序完成，不要求解析无关 Session。
- 创建 fork 不得随父 Session 大小线性复制数据。
- Session 列表和反向关系索引可以在进程内缓存，并能从 Spec 重建。
- 第一阶段不为百万级 Session 或多机吞吐优化。

### 9.5 可演进性

- Store Format、Control Plane Protocol 和 Extension Protocol 必须独立版本化。
- 不支持的主版本必须明确拒绝，不得静默解释。
- 后续增加 SQLite 或远程 Store 时，不得改变 Session、Batch 和 Source 的领域语义。

## 10. 产品成功标准

完成最小控制面与 Session 派生能力（Phase 1-2）时必须满足：

1. 可以创建一个无来源 Session，并异步执行一次包含工具调用的 Activation。
2. 用户输入、模型输出和工具结果均以原生 Open Responses Item Batch 保存。
3. 删除状态缓存后可以恢复相同 Session 状态。
4. 在任意 Batch 提交期间强制终止进程，恢复后只能看到完整 Batch 或看不到该 Batch。
5. 可以从父 Session 的固定上下文范围创建子 Session，且不复制父 Item。
6. 子 Session 模型输入能够按 Source 顺序加自身 Item 正确装配。
7. 同一 Session 的并发 Activation 不会同时运行。
8. 不同 Session 可以并行运行。
9. CLI 退出后 Activation 和 Session 数据仍可恢复。
10. Hook 无法删除或替换已提交 Item。
11. Store 中没有 ragent 自定义 Message/ToolCall/Reasoning 持久化模型。
12. 无 Extension 时核心工具列表为空。
13. 文件、HTTP 和命令工作目录请求均能经过统一权限判定。
14. 权限询问可在一个前端发起、另一个前端回答，并在重连后恢复。
15. Session、项目和全局授权分别持久化到正确作用域，且 `None` 与空列表语义可验证。
16. 未信任项目配置不会被读取；信任空项目后生成最小允许规则而不授予项目外能力。

## 11. 分阶段范围

### Phase 1：最小控制面

- Directory Store。
- Session Spec。
- Config Revision。
- Item Batch。
- Session Event。
- Context Projection。
- 单进程 Store Writer。
- 单 Session 单 Activation。
- CLI 和本地 Control Plane API。
- 基础 WASM Hook。

### Phase 2：多前端与派生

- TUI。
- Session Source 和零复制 fork。
- Session 血缘查询和内存索引。
- Session spawn Command。
- Activation 队列和更完整的恢复。

### Phase 3：Web 与高级工作流

- HTTP/SSE Adapter。
- WebUI。
- 压缩任务 Session 模板。
- 通用人工交互和完整权限 Controller。
- 文件、WASI P2 HTTP 和命令工作目录权限。
- 项目信任、全局/项目/Session 配置层和权限快照。
- Session 图可视化。

### 未来按实际需求评估

- SQLite Control Store。
- PostgreSQL 或远程 Control Store。
- 多机 Runner。
- 自动压缩 Controller。
- 命令内容级隔离、任意 socket 和系统调用沙箱。
- 内容寻址 Artifact Store。

## 12. 明确不做的设计

- 不建设 Exchange 或 Session 消息总线。
- 不追踪 Session Runner 通过目录文件发生的交互。
- 不把 Open Responses Item 转换为内部消息模型。
- 不允许原地压缩、删除或替换 Session 历史。
- 不让前端直接写 Store 文件。
- 不让 Extension 直接写 Store 文件。
- 不将 Agent Runner 作为 Session 的持久状态容器。
- 不在第一阶段引入数据库、分布式协调或网络服务依赖。

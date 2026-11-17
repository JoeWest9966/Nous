# harness 的形状（`#14` 的第一个 commit —— 只有这张表，没有代码）

先看这张表，再动代码。理由和 `#13` 一样：`#13` 一次过是因为先出类型清单后写实现；
harness 是本库最后一个大未知，先出形状能省掉两轮返工。

读法：

1. §1 四件各自「上游文件 + 行数 / 取哪一段 / 本库落点 / 公开面逐签名」。
2. §2 投影表 —— **`#14` 与 `#22` 共用这一份**，`#22` 只引用、不重写。
3. §3 `tokens` / `retry` 摘录边界（只抄真用到的行）。
4. §4 本票不做（写下来是为了不在实现时慢慢膨胀）。
5. §5 落地的判据名，§6 三条要人拍的。

---

## 0. 四条修正（都会改工作量，先看这个）

1. **上游没有 `harness/abort.ts`。** `AGENTS.md` §一路由表里的 `abort` 是我上一轮的误记。
   上游的 abort 不是一个模块，是三处：`runtime.signal`（`AbortSignal`，贯穿 commit / 请求 /
   工具）、`generation.ts:397` 的 `abort:` 任务钩子、`generation.ts:668` 的 `convertPartial`
   （把在流的 partial 落成一条 `stopReason: "aborted"` 的 assistant entry）。
   所以本库的 `harness/abort.rs` 是**自写**，不是提货；它只做 `convertPartial` 那一件事。

2. **`MessageChange` 在 `pi@v1.5.1` 已被上游删除。** `durable/CHANGELOG.md:76`：
   *Removed `watchEvents()` and its types `AgentEvent`, `AgentEventStream`, `SnapshotEvent`,
   `MessageChange`, and `ToolEventCall`, and spec §9.4*（earendil-works/pi#11327，理由是
   「渲染器渲染状态，不是拿事件拼状态」）。`wire.md` 引的 `dist/harness/events.d.ts` 是**旧
   dist**。结论：`nous-wire` 的 8 个变体（`#8` 已提货、已冻结）仍然有效，但**投影逻辑没有任何
   上游源码可抄** —— 它是自写的，锚点只有三样：`wire.md`（宪法）、`nous-wire::MessageChange`
   （已冻结的形状）、`PartialChanges` / `assignJson` 的 plan 语义（上游还有，见 §1①）。

3. **`generation.ts` 的 1,197 行不是「一个模块」，单搬不了。** 它是一个
   `defineTask` 状态机，7 个 checkpoint phase（`prepare` / `request` / `retry` / `poll` /
   `tools` / `finish` / `drain`），跑在 `scheduler.ts`（**3,193 行**）+ `task-graph.ts`（313）
   + `define.ts`（61）之上。所以 `#14` 从它只取三段（§1③），**不取 checkpoint 机器**。

4. **节流提交用的是 deferred，而 `#13` 明确拒绝 deferred。** `live.ts:236`：
   `PROGRESS_COMMIT = { persistence: { kind: "deferred" } }`，`generation.ts:697` 的
   `streamResponse` 用它提交每条 partial；而 `Session::commit` 现在对
   `CommitPersistence::Deferred` 报 `SessionError::DeferredUnsupported`（有测试证明**什么都没写**）。
   于是 `#14` 只有两条路（§6 第 1 条要你拍）。

---

## 1. 四件

行数是 `pi@v1.5.2` / `pi-rust@v0.24.2` 的 `wc -l`，不是估计。

### ① `harness/partial-changes.ts`（185 行，全文都取）

| 项 | 内容 |
|---|---|
| 上游 | `packages/durable/src/harness/partial-changes.ts:42-185`（类 `PartialChanges`）+ 依赖 `harness/json.ts`（62 行：`AssignPlan` / `assignJson`） |
| 取什么 | 全文。它回答的是「一次 partial 提交要写多少」：text/thinking 只追加增量，tool-call 参数只写解析器报的 change，从而提交成本跟着新文本走，不跟着字符串总长走 |
| 本库落点 | `crates/nous-durable/src/harness/partial.rs` + `crates/nous-durable/src/harness/json.rs` |
| 公开面 | `PartialChanges`：`new()` / `observe(&AssistantMessageEvent)` / `take(&mut PartialMessage) -> PartialCommit` / `commit(&mut LiveGeneration, &PartialMessage, PartialCommit)` / `restore(PartialCommit)`；类型 `PartialCommit { plan: AssignPlan, tool_calls: BTreeMap<u64, ToolCallTake> }`、`ToolCallTake { changes: Vec<JsonChange>, snapshot: Value }`；`json.rs`：`AssignPlan::{Append{text}, Keep, Fields(BTreeMap<Seg, AssignPlan>)}` + `assign_json(...)` |
| 依赖 | `nous_chord::delta::{append_text, ...}`、`nous_ai::types::AssistantMessageEvent`（已存在）、`nous_ai::Content` |
| 阻塞点 | `StreamingJsonParser` / `JsonChange` 住在 `packages/ai/src/utils/streaming-json.ts`（**559 行**），不在 `AGENTS.md` §一 的契约面里（那里只写 `google-generative-ai.ts`）。见 §6 第 2 条 |

### ② `harness/live.ts`（452 行，取 139-410，不取 411-452）

| 项 | 内容 |
|---|---|
| 上游 | `live.ts:139-236`（`LiveRun` / `LiveState` / `LiveDoc` / `PROGRESS_COMMIT`）+ `:250-410`（`cutOffTooOften` / `salvageable` / `cutOffCount` / `RunEnd` / `endRun` / `addCompactionStatus` / `compactionStatus` / `removeCompactionStatus` / `failCompactionStatus` / `toolSlot` / `finishSlot` / `removeNestedSlots` / `clearProgress`） |
| 不取 | `:411-452` `settleSchedulerOutcome` —— 它的入参是 scheduler 的 `TaskOutcome`，搬它等于搬调度器 |
| 本库落点 | `crates/nous-durable/src/harness/live.rs` |
| 公开面 | `LiveState { run, generation, tools, nested_tools, compactions, compaction_failure, run_failure }`；`LiveRun { task_id, inputs, turns, first, after_run }`；`LiveGeneration { attempt, message, retry, deferred }`；`ToolSlot` / `NestedToolSlot` / `CompactionStatus` / `CompactionFailure` / `RunFailure` / `RunFailureReason` / `RunEnd`；函数 7 个：`end_run` / `add_compaction_status` / `remove_compaction_status` / `fail_compaction_status` / `tool_slot` / `finish_slot` / `clear_progress`；常量 `LIVE_DOC`（`kind: "pi.live"`、`version: 3`、`scope: conversation`、`history: latest`、`fork: initial`）+ `checkpoint_when(&LiveState) -> bool` |
| 注意 | `checkpoint_when` 是一条**真规则**（上游注释写死「不要加 delta 计数上限」，工具输出基准在测它）：空闲 / 交接 / 两个工具之间必须是完整 base。我们要照抄这条判据，别改成「每 N 条 delta 一个 base」 |

### ③ `harness/generation.ts`（1,197 行，只取三段 ≈ 230 行）

| 段 | 上游 | 取什么 | 本库落点 |
|---|---|---|---|
| 节流单飞 | `:697-764` `streamResponse` | 100ms（`settings.progress.partialIntervalMs`）定时器 + **同时只有一次提交在飞** + `finally` 停表并等那次提交（否则过期 partial 会盖住终态）+ 失败 `restore` | `harness/generation.rs::stream_response` |
| abort 落库 | `:668-674` `convertPartial` | 把 `live.generation.message` 复制成一条 `stopReason: "aborted"` 的 assistant entry | `harness/abort.rs::convert_partial` |
| 分类 | `:765-868` `classify` | 终态消息 → 四种去向：`deferred`（转 `poll`，`pollAt = max(now + pollAfterMs ?? 5000, 上次 + 1)`）、可重试错误（转 `retry`，退避）、溢出（转 `prepare` + 阻塞压缩）、答完（`answer`/工具轮） | `harness/generation.rs::classify` |
| 公开面（提议） | — | `pub async fn stream_response(session: &Session, conversation: Id, live: &mut LiveState, request: &RunRequest<'_>) -> Result<AssistantMessage, HarnessError>`；`pub async fn classify(session: &Session, request: &RunRequest<'_>, message: &AssistantMessage) -> Result<Classification, HarnessError>`；`pub enum Classification { Answered, Retry { at_ms: u64, error: String }, Poll { poll_at_ms: u64 }, Overflow { error: String } }`；`pub struct RunRequest<'a> { model, messages, options, attempt, interval_ms, persist_within_ms }` | — |
| 不取 | 7 个 checkpoint phase、`runtime.commit`/`runtime.hooks` 那层、`beforeRequest`/`afterRun` 钩子链、`startRun`/`handOver`（`createTask` 属调度器） | — | — |

### ④ abort（无上游模块 —— 见 §0.1）

| 项 | 内容 |
|---|---|
| 上游事实 | `runtime.signal: AbortSignal` 在 `commit` / provider 流 / 工具里被读（`harness.ts:594`、`generation.ts:216`、`tool.ts:480`、`view.ts:191/211/243`）；`generation.ts:397` 是 `abort:` 任务钩子；`generation.ts:668` 是把在流 partial 落库 |
| 本库落点 | `crates/nous-durable/src/harness/abort.rs`（**自写**） |
| 公开面 | `pub async fn convert_partial(session: &Session, conversation: Id, live: &LiveState) -> Result<Option<Id>, HarnessError>`（无 partial 时 `Ok(None)`）；`pub struct AbortMark`（`Session` 侧用 `CancellationToken`，不用 `AbortSignal` —— `nous-ai` 已经是 `CancellationToken`） |
| 判据 | `convert_partial_writes_an_aborted_assistant_entry`（构造一条 partial → 关 → 读回 entry 的 `stopReason == "aborted"`） |

---

## 2. 投影表（`#14` 与 `#22` 共用这一份）

**输入**是 `CommitPublication`（`crates/nous-durable/src/session/publish.rs`，形状逐字段照抄上游
`types.ts:936-982`）：

```rust
CommitPublication { seq: Seq, changes: Vec<CommitChange> }
CommitChange::{ Conversation{..}, Entry{..}, Task{..}, Submission{..}, Document(DocumentCommitChange) }
DocumentCommitChange::{ Change{record, conversation_id, version, value, ops}, Retire{record, conversation_id}, Copy{record, conversation_id, source} }
```

**输出**是 `Vec<nous_wire::MessageChange>`（8 个变体，`#8` 已冻结）。

### 2.1 每一格产出什么（含「不产出」）

| `CommitChange` 格 | 产出 | 为什么 |
|---|---|---|
| `Document::Change`，`record.kind == "pi.live"`，`ops` 触及 `generation.message` | **0..N**，按 §2.2 逐条 op 生成 | **唯一**产出 `MessageChange` 的格子 |
| `Document::Change`，`pi.live`，`ops` 只触及 `run` / `tools` / `compactions` / `retry` / `deferred` | **0** | 工具卡片、重试提示走各自的 SSE 事件（`tool_execution_*`、`auto_retry_*`），不是 `message_update.changes[]` |
| `Document::Change`，其它 `record.kind`（`pi.transcript` / `pi.agent` / `pi.usage` / `pi.inbox` …） | **0** | 没有消息语义 |
| `Document::Retire` | **0** | 退役后客户端只能读到「没有值了」；`wire.md` 没有「文档退役」帧，硬发一条 `message` 会让气泡凭空清空 |
| `Document::Copy` | **0** | 复制产生一条新 base，客户端首次拿到的就是值，不回放 ops（`#13` 12d 已定：复制结果是一条 base） |
| `Entry`（任何 `kind`） | **0** | entry 是「定稿」。`message_end` / `entry_appended` 是 `#22` 从 entry 变化生成的**另一类帧**，不在 `changes[]` 里 |
| `Conversation`（含 `unread` 计数、`parent`、`owner`） | **0** | `wire.md` 没有会话元数据帧；未读数是 `/v1/sessions` 快照字段 |
| `Task` / `Submission` | **0** | 任务与提交不是消息。`submission` 帧由 `#22` 从 `Submission` 变化生成，形状也不同 |

**这一节的意义**：`#22` 不能假设「每条 commit 都有 `message_update`」。绝大多数 commit **不发消息帧**，
只有 `pi.live.generation.message` 那一路发。反过来，`#22` 也不能根据 `changes.is_empty()` 判断
「安静」—— `Entry` 变化照样要发 `message_end` / `entry_appended`。

### 2.2 op → `MessageChange`（`DocumentCommitChange::Change.ops` 是 `nous_chord::delta::Op`）

| op（`Op` 变体 + 路径） | 产出 | 规则/理由 |
|---|---|---|
| `Append(path, text)`，`path == ["content", i, "text"]` | `TextDelta { content_index: i, delta: text }` | 节流提交的主力：`PartialChanges::take` 的 `AssignPlan::Append` 就落在这一条 |
| `Append`，`path == ["content", i, "thinking"]` | `ThinkingDelta { content_index: i, delta: text }` | 思考不是正文，客户端不要塞气泡（`wire.md:112`） |
| `Append`，`path == ["content", i, "arguments", ..rest]` | `ToolcallDelta { content_index: i, path: rest 拼成 "arguments.…", delta: text }` | `path` 是 JSON 指针式子路径、`delta` 是追加文本 —— **只有 `a` 能这样表达** |
| `Set(["content", i], block)`，`i == 已发生过的 content 长度`（**新块**） | `TextStart` / `ThinkingStart` / `ToolcallStart`，`block` 取 `value.content[i]` | 按 `block.type` 选变体。`*_start` 带全文，**跳过它等于把第一段吞掉** |
| `Set(["content", i], block)`，`i` 已存在，或 `d` / `p` / `m` / `t` 动了 `content` | `Block { content_index: i, block: value.content[i] }` | 整块替换。**保守但不会错**：客户端以它为准 |
| `Replace(value)`，或任何触及根的 op | `Message { message: value }` | 整条替换 |
| `Set` / `Splice` / `Move` / `Truncate` / `Remove` 落在 `generation.message` **之外**（`run.*`、`tools.*`） | 0 | 见 §2.1 第二行 |
| `AssignPlan::Keep`（上游「arguments 由 changes 写，不整块重写」） | 0 | 它的效果已经体现在同一批的 `Append` 上 |

**一条铁律**（写进代码注释，别只写在文档里）：**`Set` 永远不编码成 `*_delta`。**
`toolcall_delta` 的语义是「追加文本」，用 `set` 去表达「设值」会让客户端拼出坏 JSON，
而且不会报错 —— 就是那种「谁都查不出来」的降级。`set` 一律落 `Block` / `Message`。

### 2.3 8 个变体的证据来源（**3 条 fixture 给不了**）

fixture 机器查证（`python3` + `sqlite3.connect('file:…?mode=ro', uri=True)`）：

- `document_revisions` **969 行，全部 `kind = 'base'`，`0` 条 delta** —— 真库里没有一个 delta op 可读。
- `entries.record.model[0].content` 的块类型：`text` 88、`thinking` 76、`toolCall` 36。

| 变体 | 证据档 | 怎么验 |
|---|---|---|
| `TextStart` / `ThinkingStart` / `ToolcallStart` | **真数据** | 从 fixture 取真 assistant 消息，喂「一次提交写下 `/content` 该值」的 op，按真实 `block.type` 产出 start |
| `Block` | **真数据** | 同一批数据，第二次覆盖已存在的 index |
| `Message` | **真数据** | 同一批数据 + 根替换 `Replace(value)` |
| `TextDelta` / `ThinkingDelta` / `ToolcallDelta` | **构造**（标注为构造） | 输入 op 必须来自 `PartialChanges::take` 的真实产出，不是手写 op —— 这样至少「谁产出 `Append`」有真代码在验 |

所以你要的那条「8 个变体每个至少一条 fixture 驱动测试」**做不到 8 条，只能 5 条**。剩下 3 条
要么构造（我倾向这个，测试名带 `_constructed_`），要么承认这 3 行没人验。你说哪个。

---

## 3. `tokens` / `retry` 摘录边界

### 3.1 tokens

| 项 | 内容 |
|---|---|
| 上游 | `pi-rust@v0.24.2` 的 `crates/rpi-harness/src/compaction/tokens.rs`（**479 行**） |
| 取什么 | 纯函数那一半：`estimate_tokens` 按消息估算（text / thinking / toolCall / image 各自的系数）、`estimate_context_tokens`、以及「按字符数还是按 token 数」的常量表 |
| 不取 | `compaction.rs`（1,003 行）、`branch_summary.rs`（470）、`cut_point.rs`（246）、`settings.rs`（82）—— 压缩整体属另一张票，本票只要 est 函数来判溢出 |
| 落点 | `crates/nous-durable/src/harness/tokens.rs` |
| 注意 | `nous-ai` 的 `anthropic::estimate_context_tokens` 已经有一份（`#5` 提货时带的）。**两份会漂移** —— 本票只保留 `nous-ai` 那份，`harness` 直接调它；`rpi-harness` 的 479 行里，只抄 `nous-ai` 那份**没有的**部分（如果有） |

### 3.2 retry

| 项 | 内容 |
|---|---|
| 上游 | `rpi-harness/src/types.rs:289-311`（`RetryPolicy`，23 行）+ `rpi-harness/src/settle.rs:148-158`（`will_retry`，11 行）+ `compaction::is_retryable_assistant_error` |
| 取什么 | 这三段，逐字：`enabled && attempt < max_retries && stop_reason == Error && is_retryable(error)`；默认 `enabled: true`、`max_retries: 10`、`base_delay_ms: 2000`、`max_agent_delay_ms: 60000` |
| 不取 | `agent_harness.rs`（**8,341 行**）的 `retry_pending` journal / `died_awaiting_retry` / 恢复扫描 —— 那是 rpi 的会话日志，本库的重试用 `pi.live.generation.retry` + 任务记录，语义不同 |
| 落点 | `crates/nous-durable/src/harness/retry.rs` |

**边界一句话**：`tokens` 只抄估算，`retry` 只抄判定与退避参数；**两边的「恢复」都不抄**
（rpi 用 journal，本库用 `pi.live` + 任务记录）。

---

## 4. 本票不做

- `generation.ts` 的 7 个 checkpoint phase、`defineTask`、`scheduler.ts`(3,193) / `task-graph.ts`(313) / `define.ts`(61)
- 钩子链（`beforeRequest` / `beforeProviderRequest` / `afterResponse` / `afterRun` / `onYield`）
- `submissions.ts`(317) / `tool.ts`(1,146) / `compaction.ts`(1,003) / `prompt.ts`(158) / `inbox.ts` / `registry.ts`
- `settleSchedulerOutcome`（要调度器）
- deferred 持久化**本身** —— 除非 §6 第 1 条选 (a)
- `streaming-json.ts` 的提货 —— 除非 §6 第 2 条选 (a)
- SSE 帧、16ms 合帧、HTTP 端点（`#22`）
- `watch` / `observe*` 订阅（`#19`）
- `scan_*`（`#21`）

---

## 5. `#14` 落地的判据名（先定名，代码跟着走）

| 判据 | 验什么 |
|---|---|
| `partial_changes_appends_text_instead_of_reassigning` | `take()` 对 text 产出 `Append`（不是整块 `Set`）；`Set` 那天成本就退化成 O(总长) |
| `partial_changes_restores_what_a_failed_commit_took` | 提交失败后 `restore()` 把 changes 放回去，下一次提交先写它们 |
| `live_doc_is_a_complete_base_when_nothing_runs` | `checkpoint_when` 在空闲 / 交接 / 两工具之间为真（上游那条不许改成 delta 计数） |
| `convert_partial_writes_an_aborted_assistant_entry` | abort 落库的形状 |
| `projection_covers_every_message_change_variant` | 8 变体逐个（5 真数据 + 3 构造） |
| `projection_emits_no_change_for_storage_only_commits` | §2.1 的 7 行「不产出」各一条 |
| `projection_never_encodes_a_set_as_a_delta` | §2.2 那条铁律 |
| `harness_stream_commits_partials_at_most_every_interval` | 节流 + 单飞 + `finally` 等飞完（时间线见证，不用 `sleep` 猜） |

---

## 6. 三条要你拍的

1. **deferred 持久化：补还是不补？**
   (a) 把 `session/transaction.rs` 的 `plans` / `holds` 补上（`#13` 时判定「等真有并行提交需求再看」）。
       好处：与上游同形，`persistWithinMs` 有真语义；代价：`#14` 连带改 `#13` 的关闭语义（关会话时
       hold 里的东西要落），判据要加。
   (b) harness 只做 immediate，`partialIntervalMs` 退化成「每帧一次提交」。
       好处：`#14` 小一半；代价：长回答的存储事务数从 ~1/秒 变成 ~10/秒（上游注释给的量级是 5.7x 事务、
       2.9x 磁盘写），语义不变、只是贵。
   **我倾向 (b) 先落地，(a) 单开一张票**，因为 (a) 会把 `#14` 胀成两张票，而且 `#22` 等不起。
2. **`streaming-json.ts`（559 行）提不提？**
   它是 `PartialChanges` 跟踪 tool-call 参数增量的唯一办法，住在 `packages/ai` 里 —— 而
   `AGENTS.md` §一 只允许我们从那里拿 `google-generative-ai.ts`。
   (a) 提货进 `nous-ai/src/utils/streaming_json.rs`，并把 §一 那行改成「+ `utils/streaming-json.ts`」。
   (b) 不提：harness 里每次提交把 `arguments` 整块重写（`Set`，落 `Block`）。正确性一样，代价是长
       参数工具调用每次部分提交都写整个参数对象。
   **我倾向 (a)**：它是纯函数、有上游测试可对拍，而且 (b) 会让「提交成本跟着新文本走」这个性质
   在工具调用上失效 —— 那正是这条路径存在的理由。
3. **3 条 delta 变体用构造数据**（§2.3）—— 认还是不认？不认的话这 3 行就永远没人验。

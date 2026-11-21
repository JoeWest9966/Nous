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

## 0.5 `nous-ai` 里会有两个「streaming json」，别删错

提货之后 `nous-ai` 有两个名字里都带 streaming json 的东西，**分工不同、都有人用**：

| | `json_parse::parse_streaming_json(Option<&str>) -> Value`（已有，304 行文件） | `utils::streaming_json::StreamingJsonParser`（本次提货） |
|---|---|---|
| 输入 | 一段前缀文本，**每次整段重解** | 增量片段，一次读一遍 |
| 输出 | 一个「当前快照」 | 快照 **+ 这一段造成的 `JsonChange`（路径 + 值/文本）** |
| 成本 | 与前缀长度成正比，**每个前缀都是 O(n)**，整条流 O(n²) | 整条流 O(n)（模块注释写明了复制额度机制） |
| 谁在用 | `frames.rs`、`anthropic/mapper`、`openai_completions`、`openai_responses`（要的是「能拼出合法 JSON 的当前值」） | `ToolCallArgumentsStream`（provider 的流状态）、`harness/partial.rs` 的 `assign_json`（要的是「增量改了哪里」） |
| 判据 | 任意前缀都给得出快照，不 panic | `changes` 依次 apply 到空值 == 同一时刻的 snapshot |

上游把这条分工写在注释里（`packages/ai/src/utils/json-parse.ts:100-103`，`parseToolCallArguments`
的文档）：

> Unlike `parseStreamingJson`, this never keeps a partially parsed prefix: once the call is finished,
> text that is not a valid JSON object means the model sent malformed arguments.

换成我们的说法：`parse_streaming_json` 是**容错前缀解析**（喂它半句也要给出「现在读到哪」），
`StreamingJsonParser` 是**增量解析器**（喂它一片，它告诉你「这一片改了哪个路径」）。
前者做不到后者：拿新前缀和旧快照做 diff 也能得到 change，但那是 O(n²) 且会把「同值重复出现」
误判成 change —— 而 `#14` 的整个前提正是「提交成本跟着新文本走」。

**危险的动作**：看到两个 streaming json，删掉一个「重复的」。两个都有人用，删任何一个都会红，
但**只看名字看不出这一点** —— 所以写在这里。

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
| 上游依赖 | `StreamingJsonParser` / `JsonChange` 住在 `packages/ai/src/utils/streaming-json.ts`（**559 行**）。**已提货**（用户 2026-11-17 拍板，`AGENTS.md` §一 随之加了这行，commit `223cf61`）→ `nous-ai/src/utils/streaming_json.rs`，只取四个导出。它与 `nous-ai` 里已有的 `parse_streaming_json` **不是一回事**，见 §0.5 |

### ② `harness/live.ts`（452 行，取 139-410，不取 411-452）

| 项 | 内容 |
|---|---|
| 上游 | `live.ts:139-236`（`LiveRun` / `LiveState` / `LiveDoc` / `PROGRESS_COMMIT`）+ `:250-410`（`cutOffTooOften` / `salvageable` / `cutOffCount` / `RunEnd` / `endRun` / `addCompactionStatus` / `compactionStatus` / `removeCompactionStatus` / `failCompactionStatus` / `toolSlot` / `finishSlot` / `removeNestedSlots` / `clearProgress`） |
| 不取 | `:411-452` `settleSchedulerOutcome` —— 它的入参是 scheduler 的 `TaskOutcome`，搬它等于搬调度器 |
| 本库落点 | `crates/nous-durable/src/harness/live.rs`（**进度 2026-11-17：已落**）—— 全部类型 + 九个 draft 变更函数（`end_run` / `add_compaction_status` / `compaction_status` / `remove_compaction_status` / `fail_compaction_status` / `tool_slot` / `finish_slot` / `remove_nested_slots` / `clear_progress`）+ `checkpoint_when`。三处转译：交叉类型 → `SlotProgress` + `flatten`；`Draft<ToolSlot \| NestedToolSlot>` → `SlotRef` 枚举；`tx.settleSubmission` → `end_run` 把结算清单返回给调用方。不落：`LiveDoc.version`/`migrate`（本库没有 per-document 版本号，也没写过 v1/v2）、`defineDoc` 地址盖章、任务类型判定那组 |
| 公开面 | `LiveState { run, generation, tools, nested_tools, compactions, compaction_failure, run_failure }`；`LiveRun { task_id, inputs, turns, first, after_run }`；`LiveGeneration { attempt, message, retry, deferred }`；`ToolSlot` / `NestedToolSlot` / `CompactionStatus` / `CompactionFailure` / `RunFailure` / `RunFailureReason` / `RunEnd`；函数 7 个：`end_run` / `add_compaction_status` / `remove_compaction_status` / `fail_compaction_status` / `tool_slot` / `finish_slot` / `clear_progress`；常量 `LIVE_DOC`（`kind: "pi.live"`、`version: 3`、`scope: conversation`、`history: latest`、`fork: initial`）+ `checkpoint_when(&LiveState) -> bool` |
| 注意 | `checkpoint_when` 是一条**真规则**（上游注释写死「不要加 delta 计数上限」，工具输出基准在测它）：空闲 / 交接 / 两个工具之间必须是完整 base。我们要照抄这条判据，别改成「每 N 条 delta 一个 base」 |

### ③ `harness/generation.ts`（1,197 行，只取三段 ≈ 230 行）

| 段 | 上游 | 取什么 | 本库落点 |
|---|---|---|---|
| 节流单飞 | `:697-764` `streamResponse` | 100ms（`settings.progress.partialIntervalMs`）定时器 + **同时只有一次提交在飞** + 失败 `restore`；`finally` 只做三件：`stopped = true`、取消定时器、`await inFlight` —— **不提交 pending**（见下面那段，尾帧是故意丢的） | `harness/generation.rs::stream_response` |
| abort 落库 | `:668-674` `convertPartial` | 把 `live.generation.message` 复制成一条 `stopReason: "aborted"` 的 assistant entry | `harness/abort.rs::convert_partial` |
| 分类 | `:765-868` `classify` | 终态消息 → 四种去向：`deferred`（转 `poll`，`pollAt = max(now + pollAfterMs ?? 5000, 上次 + 1)`）、可重试错误（转 `retry`，退避）、溢出（转 `prepare` + 阻塞压缩）、答完（`answer`/工具轮） | `harness/generation.rs::classify` |
| 公开面（**2026-11-17 已落，签名以此为准**） | — | `pub async fn stream_response(session: &Session, request: &StreamRequest, events: AssistantMessageEventStream, timeline: Option<&Timeline>) -> Result<AssistantMessage, HarnessError>`；`pub async fn classify(session: &Session, request: &ClassifyRequest, message: &AssistantMessage, now_ms: u64, timeline: Option<&Timeline>) -> Result<Classification, HarnessError>`；`pub enum Classification { Answered, Retry { at_ms, error }, Poll { poll_at_ms }, Overflow { error } }`；`pub struct StreamRequest { live: Id, conversation: Id, attempt: u64, interval_ms: u64 }`；`pub struct ClassifyRequest { live, conversation, attempt, entry_id, retry: RetryPolicy, retryable: bool, overflow: Option<String> }`；`pub fn live_record(id: Id, conversation: Id) -> Value`（`pi.live` 的五个地址字段，别在调用点手抄）；`pub struct Timeline` + `pub enum Beat { PartialStart, PartialEnd, Terminal }` | — |
| 不取 | 7 个 checkpoint phase、`runtime.commit`/`runtime.hooks` 那层、`beforeRequest`/`afterRun` 钩子链、`startRun`/`handOver`（`createTask` 属调度器） | — | — |

#### ③ 附：提交节奏的五条事实（2026-11-17 核过上游，别再翻案）

**(1) `deferred` 不做 ⇒ 走 immediate + 时间闸。** 上游 partial 用 `PROGRESS_COMMIT`
（`live.ts:236` `{ persistence: { kind: "deferred" } }`），本库 `#13` 的
`CommitPersistence::Deferred` 明确拒绝（`docs/decisions.md` deferred 那节）。所以每次 partial
都是一次真提交，节拍只由 100 ms 时间闸与单飞决定。

上游那三个数一并记在这里，免得再猜（`harness/agent.ts:36-40` `DEFAULT_PROGRESS_POLICY`）：
`partialIntervalMs = 100`、`outputIntervalMs = 100`（工具输出，另按
`PROGRESS_BYTES_PER_SECOND = 100 KiB/s` 配速，`chord/output.ts:306`）、`persistWithinMs = 1_000`
（deferred 的**最长窗口，不是节拍**）。**`pi.live` 没有「每 N 毫秒一个 revision」这种节拍** ——
我们这边 revision 只有两个来源：这次 partial 闸，和调用方自己的提交。完整表格见
`docs/storage.md` §6。

**(2) 尾帧必交是错的 —— 上游故意丢最后那一批增量。** `generation.ts:735-770`：

```ts
} finally {
    stopped = true;
    timer?.();       // 取消定时器
    await inFlight;  // 只等在飞的，**不提交 pending**
}
```

上游自己的 docstring（`:690-695`）写着 "`finally` stops the throttle and awaits that commit,
so no stale partial lands after the outcome"。紧接着 `classify` 会一次提交终态消息并
`delete live.generation`（`:816` 一带），补那一帧只是**覆盖一次再被覆盖**。

所以判据不是「最后一批增量必被提交」，而是**终态之后不许再落 partial**：`classify` 那次提交
之后 `live.generation` 已被清掉，时间线上不许出现「先终态、后 partial」。已并入
`harness_stream_commits_partials_at_most_every_interval` 的同一条时间线断言（见 §5）。

**(3) 两次提交之间不许在持锁区间里做 `materialize` 重放。** 提交在流循环里 `await`，写盘
（ext4 `NORMAL` 约 1 ms，见 `docs/bench-commit.md`）直接压在 provider 事件处理上 —— 这是
可以接受的代价。但**不要把「重放整条修订链」放进持锁区间**：那就是 `#25` 那个 O(n²)，而
今天测到的 110 ms 是**当前链长**（160 次提交、链被 base 切断）下的数，不是上界。

**(4) 停表要真停，竞争要串起来。** 上游 `timer?.()` 用的是 `runtime.setTimer` 返回的取消函数；
本库的定时器是 `select!` 里的 `sleep_until` 临时 future，select 一结束就 drop，**所以「停表」
是结构性的**。判据不看实现，只看提交次数：`stopped` 之后冷却三个 `interval_ms`，`PartialStart`
的次数不许再涨（若有人把提交挪进 `spawn`、定时器只让回调 `return`，这条立刻红）。
abort 与 `classify` 的竞争同理：`Session` 的存储锁保证先到的那次提交整个跑完，`convert_partial`
读到的总是它**之后**的 `generation`；终态已经清掉 `generation` 时它写 `Ok(None)` ——
「committed state stays consistent」是一条判据（`an_abort_after_the_terminal_commit_writes_nothing`），
不是注释。

**(5) 书签类变更：每次状态转换自带一次提交，不许攒。** 上游靠 deferred 的 1 秒窗口兜住低频
变更（"stored with the next commit that touches `pi.live`, or after `persistWithinMs`"）。砍了
deferred 之后这个兜底没了，于是必须换一条规矩：**工具启动一次提交、结束一次提交、abort 一次
提交**；不许"攒到工具结束"，也不许"跟着下一个事件走"。

为什么不能"跟下一个事件走"：一个 `bash` 跑 60 秒，期间没有任何别的提交 —— 攒就等于客户端
60 秒看不见这个工具。代价是每个工具多一两次提交（110 ms / 160 次这个量级下看不见），换的是
**可见性不依赖"下一个事件什么时候来"**。

**这条的提交点不在 `live.rs`**：`live.rs` 的九个函数只改 `Draft<LiveState>`，不碰 `Session`
（上游签名同样只拿 `Tx` + `Draft`）；提交点在工具执行层 / 调度层。所以这是一条**跨票约定**，
`#22` 的判据直接用：

> SSE 上「工具启动」必须在启动后 ≤ 1 个事件循环出现，不许等到工具结束。

这条能红（把提交挪到结束时，客户端 60 秒看不到工具），而且它跨 `#14` 与工具执行层：两边各自
正确、接缝处仍可能出错，正是最容易没人负责的地方。

**(6) 尾帧提交抓得住了 —— 靠等号，不靠基准票。** 起初这里写的是「抓不住」（时间线上尾帧落在
终态之**前**，既不违反节拍也不违反「终态在后」）。**那是判据写宽了，不是判据写不了**：当时
断言的是区间 `2..=5`，而 `start_paused` 下虚拟时钟精确到毫秒、同一份输入每次得到同一个数 ——
区间恰好给「补一帧」（4 次）留了位置。现在写成 `assert_eq!(partials, 3)`（40 片 × 10 ms、
闸 100 ms ⇒ 110 / 210 / 310 ms 三次），并逐个钉住时刻；补一帧就是 4，实测立刻红。

**区间只有一个正当理由：虚拟时钟下仍有抖动。** tokio 的 paused 时钟不抖动，所以那个理由
不成立 —— 凡是能在虚拟时钟上算出来的次数，都写等号。

### ④ abort（无上游模块 —— 见 §0.1）

| 项 | 内容 |
|---|---|
| 上游事实 | `runtime.signal: AbortSignal` 在 `commit` / provider 流 / 工具里被读（`harness.ts:594`、`generation.ts:216`、`tool.ts:480`、`view.ts:191/211/243`）；`generation.ts:397` 是 `abort:` 任务钩子；`generation.ts:668` 是把在流 partial 落库 |
| 本库落点 | `crates/nous-durable/src/harness/abort.rs`（**自写**） |
| 公开面 | `pub async fn convert_partial(session: &Session, live: Id, conversation: Id, entry_id: Id) -> Result<Option<Id>, HarnessError>`（无 partial 时 `Ok(None)`；entry id 由调用方给 —— 上游 `tx.appendEntry` 自己 mint）。**`AbortMark` 还没落**：它要接 `Session` 的关停路径，属 `#19` 的观察者那一层，本票不开那个口子 |
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

### 2.2 op → `MessageChange`（实现在 `crates/nous-durable/src/harness/project.rs`）

`ops` 是**这次提交**的那批 `nous_chord::delta::Op`，`value` 是**批后**的完整文档值。表里的路径
都省略了 `generation.message` 前缀（`P = ["generation","message"]`）。

| op（`Op` 变体 + 路径） | 产出 | 规则/理由 |
|---|---|---|
| `Append`，`[P, "content", i, "text"]` | `TextDelta { content_index: i, delta }` | 节流提交的主力：`PartialChanges::take` 的 `AssignPlan::Append` 就落在这一条 |
| `Append`，`[P, "content", i, "thinking"]` | `ThinkingDelta { content_index: i, delta }` | 思考不是正文，客户端不要塞气泡（`wire.md:112`） |
| `Append`，`[P, "content", i, "arguments", …]` | `ToolcallDelta { content_index: i, path: "arguments.…", delta }` | 增量**只**来自 `StreamingJsonParser`（`harness/partial.rs::apply_change` 落到 `arguments` 上的 `a`）。**不许自己切字符串**：切出来的每一帧都合法，拼起来不是合法 JSON |
| `Set`，`[P, "content", i]`，`i == 本批第一个可能创建块的下标`（**新块**） | `TextStart` / `ThinkingStart` / `ToolcallStart`（按 `block.type`），`block` 取**该 op 的值** | `*_start` 带全文，**跳过它等于把第一段吞掉** |
| `Set`，`[P, "content", i]`，`i` 更靠后（**已有块**） | `Block { i, 该 op 的 block }` | 整块替换，取 op 自己的值 —— 后面同一批的追加还能接上 |
| `Splice`，`[P, "content"]`，`delete == 0` | 每个插入的块一条 `Block { start + k, items[k] }`，`content_index` 用**应用这条 op 时**的下标 | 真实流里「新块出现」就是这条形状（`assign_json` 的 `splice`）。整块替换对客户端永远成立 |
| `Set` 落在块**内部**（`content[i].text` / `content[i].arguments.x`）、`Append` 落在块内部但字段认不出、`Splice` 在块内部 | `Block { i, **批后**块 }`，然后**停下** | 表达不了单字段更新；整块永远对。停下见下面第 ① 条 |
| `Set` / `Remove` 落在 `[P]` 之上（`["generation"]`、`[P]` 本身）、或 `Replace(v)` | `Message { message: 批后的消息 }`，然后**停下** | 整条替换，「以 `message` 为准」 |
| `Remove` 落在 `[P]` 之上、或 `Set(…, null)` | **0 帧**，然后停下 | `wire.md` 没有「消息消失」帧；硬发一条空 `message` 会让气泡凭空清空。终态靠 `message_end` 收场 |
| `Set`，`[P, "content", i]`，`i > 下标`（中间缺块） | `Message`，然后停下 | 客户端没有 `下标..i` 这些块，只给它一个会静默丢字 |
| `Remove` / `Move` / `Truncate` 在块或 `["content"]` 上（删块、置换、截断整段） | `Message`，然后停下 | `MessageChange` 里没有「删掉第 i 个」 |
| 路径在 `generation.message` **之外**（`run.*` / `tools.*` / `compactions.*` / `generation.attempt`） | 0 | 见 §2.1 第二行 |
| `AssignPlan::Keep`（上游「arguments 由 changes 写，不整块重写」） | 0 | 它的效果已经体现在同一批的 `Append` 上 |

**三条配套规则**（都在代码注释里，别只写在这儿）：

① **`Message` / `Block(批后)` 之后要停下。** 这些帧带的是**批后**的值，同一批里后面的 op 已经
算在它里面；再发那些增量，客户端会把同一段文本追加两次。停下是"不重复"的唯一做法 ——
`Set(content[i])` 与 `Splice(content)` 那两行取的是 **op 自己的值**，所以它们可以继续（后面接
`TextDelta` 正好接上，这正是用户点名的那条判据）。

② **"新块"的下标从哪来。** 只有顶层 `Set([P,"content",i])` 与 `Splice([P,"content"], start…)`
**可能创建**块；落在块内部的 op 说明块一定已经存在。长度取「批后长度」与「所有创建候选下标」
里的最小值，然后按 op 顺序推进。用**批后的长度**当"应用时的长度"会漏掉新块那一帧，客户端就拿到
一个它还没创建的块号 —— 然后静默丢字。

③ **一处诚实的含糊。** 某个实现若用顶层 `s` 重写一个**已存在**的块，这里会读成新块，发
`*_start`。两种帧都带全文，客户端重置累加器后接着收增量仍然正确；更精确的推断做不到 ——
op 里没有 base 长度。

**一条铁律**（写进代码注释，别只写在文档里）：**`Set` 永远不编码成 `*_delta`。**
`toolcall_delta` 的语义是「追加文本」，用 `set` 去表达「设值」会让客户端拼出坏 JSON，
而且不会报错 —— 就是那种「谁都查不出来」的降级。`set` 一律落 `Block` / `Message`。

### 2.3 判据：5 条真数据 + 3 条构造 + 2 条（`tests/harness_projection.rs`，10 条）

fixture 机器查证（`python3` + `sqlite3.connect('file:…?mode=ro', uri=True)`）：

- `document_revisions` **969 行，全部 `kind = 'base'`，`0` 条 delta** —— 真库里没有一个 delta op 可读。
- `entries.record.model[0].content` 的块类型：`text` 88、`thinking` 76、`toolCall` 36。

| 判据 | 证据档 | 钉住什么 |
|---|---|---|
| `the_three_start_variants_come_from_the_real_blocks_of_fixture_messages` | **真数据**（遍历全部 assistant 消息） | 三种 `*_start` 按真块类型选，一帧不少；顺带证明三种块在 fixture 里都真出现过 |
| `an_existing_block_is_replaced_whole_when_one_of_its_fields_is_set` | **真数据** | 块内 `s` → 整块 `Block`（真形状是 provider 换掉整段文本） |
| `a_root_replace_yields_one_message_frame_with_the_real_message` | **真数据** | `r` → 一条 `Message`，且 `r` 之后的 op 不再发 |
| `a_real_text_append_becomes_one_text_delta` | **真数据** | 真中文文本按字符边界追加 → 一条 `TextDelta` |
| `a_terminal_commit_that_removes_the_generation_produces_no_frames` | **真数据** | 终态删 `generation` → 0 帧 |
| `_constructed_a_thinking_append_becomes_one_thinking_delta` | 构造 | `thinking` 字段的追加 |
| `_constructed_toolcall_deltas_are_the_parser_increments_not_sliced_text` | 构造（走 `PartialChanges` + `Draft` 真路径） | 增量**逐条等于** `StreamingJsonParser` 的 `Append`；把这些帧按客户端规则应用回去正好是最终参数 |
| `_constructed_a_splice_and_a_growing_text_in_one_batch_keep_the_apply_time_index` | 构造（op 手写，但先重放证明这批合法） | **应用时下标**：`[Block(i), TextDelta(i)]`，顺序与下标都不许错 |
| `a_live_change_that_misses_the_message_produces_no_frames` | 负向 | 触及 `run` / `tools` / `attempt` 的提交 → 0 帧（不能"反正有 changes 就发一帧"） |
| `the_dispatch_table_yields_no_frames_outside_a_live_document_change` | 分派表 | `Retire` / `Copy` / 非 `pi.live` → 0 帧；同一份 ops 在 `pi.live` 上会发（反证上面不是空转） |

**四条实测可红的变异**（跑过，改回去才提交）：① `Splice` 不认 → 下标那条红；② `toolcall_delta`
自己切字符串（取首字符）→ 增量那条红；③ 长度用**批后**值 → `*_start` 那条红；④ "只要这一批有
op 就发一帧" → 0 帧负向红。

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
- `streaming-json.ts` 里四个导出之外的任何东西（其余 400+ 行：`parseStreamingJson` 的容错细节、注释、`repairJson` 相关）
- SSE 帧、16ms 合帧、HTTP 端点（`#22`）
- `watch` / `observe*` 订阅（`#19`）
- `scan_*`（`#21`）

---

## 5. `#14` 的判据名（已全部落地）

| 判据 | 验什么 |
|---|---|
| `partial_changes_appends_text_instead_of_reassigning` | 体积判据三条：第 k 次提交只有一条 `a`、载荷里没有整段正文、`len(ops)` 不随正文变长而增长；外加**追加账本**（`Draft::appends()`）断言 —— 见下面那条更正 |
| `dropping_the_append_plan_is_caught_by_the_ledger_not_by_the_bytes` | 负向对照。**结论与设计时的预期相反**：整块 `set` 不会让体积断言红 —— `Draft::prepare` 每次都 `diff_revisions` 兜底，diff 自己就重新推导出那条 `a`，字节一模一样；会红的是账本。所以这条同时证明「度量本身不是空转」（手搭整块写必须被判据抓住） |
| `partial_changes_restores_what_a_failed_commit_took` | 提交失败后 `restore()` 把 changes 放回去，下一次提交先写它们 |
| `harness_json.rs` 的 `growing_an_array_keeps_the_append_record` | `assign_json` 里「先插尾巴、再递归前缀」的顺序（顺序反了这条红；它不改变 ops 也不改变值，是账本断言） |
| `harness_live.rs`（15 条） | 上半字段名：`pi.live` 的键逐字对上游（`callId` / `droppedBytes` / `pollAt` / `parentTaskId` / `unoffered` 缺席而非 `false`）、十种 `RunFailureReason` 与三档 `DiagnosticSeverity` 拼写、往返不变。下半行为：`checkpoint_when` 恰在没东西跑时成立（生成 / 工具 / 嵌套三种假各自能红）、`end_run` 结算输入并清掉这一轮呈现、只有没答上才记 `runFailure`（`afterRun` 那轮不记）、`tool_slot` 两种槽都找得到（嵌套二分）、`finish_slot` 标 done 并清进度、压缩状态排序 / 删空 / 失败记录、`remove_nested_slots` 连带子、`clear_progress` 只删那五样 |
| `live_doc_is_a_complete_base_when_nothing_runs` | `checkpoint_when` 在空闲 / 交接 / 两工具之间为真（上游那条不许改成 delta 计数） |
| `convert_partial_writes_an_aborted_assistant_entry` | abort 落库的形状（`stopReason: "aborted"`，正文是流到那一刻的那段，之后 `generation` 清掉） |
| `every_timeline_face_has_a_fake_that_turns_it_red` | 四条时间线断言的**反向对照**：造一条「10ms 隔两次提交」的时间线、一条重叠的、一条「先终态后 partial」的、一个泄漏的定时器，每条都必须被抓。没有这一条，另外四条只是「反正都通过」 |
| `the_terminal_commit_appends_the_answer_and_clears_the_partial` | 终态：entry 是完整消息，`generation` **整键消失**（不是 `null`） |
| `a_retryable_error_turns_the_generation_into_a_retry_marker` | `retry` 退避时间点 = `now + delay_for(attempt)` |
| `an_overflow_is_never_retried` | 溢出即使 `retryable` 也不重试（上游 `!overflow` 那一项） |
| `an_empty_partial_never_commits` | 第一条 `start` 不落空帧（上游 `partial.content.length === 0` 那条 `continue`） |
| `an_abort_after_the_terminal_commit_writes_nothing` | abort 落在终态之后不写第二条 entry（「committed state stays consistent」） |
| `harness_projection.rs`（10 条，逐个见 §2.3） | 8 变体逐个；「`Set` 不编码成 `*_delta`」不单开一条，而是**每条**判据都在验它（`Block` / `Message` / `*_start` 三种落法各出现多次）；0 帧负向与分派表各一条 |
| `harness_stream_commits_partials_at_most_every_interval` | 节流 + 单飞 + `finally` 等飞完（时间线见证，不用 `sleep` 猜）+ **「终态之后没有 partial」**（出现「先终态、后 partial」即红）+ **次数等号**：`== 3`，且三次时刻逐个等于 110 / 210 / 310 ms —— 于是「在 finally 里补一帧」是 4，红。不判「尾帧必交」：上游 `finally` 故意不提交 pending（§1 ③ 附 (2)）；不补是行为，「补了会红」是成本面，两条别混 |

---

## 6. 三条要拍的（**2026-11-17 已全部拍完**，留档：结论 + 我写错的两处更正）

1. **deferred 持久化：补还是不补？**
   (a) 把 `session/transaction.rs` 的 `plans` / `holds` 补上（`#13` 时判定「等真有并行提交需求再看」）。
       好处：与上游同形，`persistWithinMs` 有真语义；代价：`#14` 连带改 `#13` 的关闭语义（关会话时
       hold 里的东西要落），判据要加。
   (b) harness 只做 immediate，`partialIntervalMs` 退化成「每帧一次提交」。
       好处：`#14` 小一半；代价：长回答的存储事务数从 ~1/秒 变成 ~10/秒（上游注释给的量级是 5.7x 事务、
       2.9x 磁盘写），语义不变、只是贵。
   **我倾向 (b) 先落地，(a) 单开一张票**，因为 (a) 会把 `#14` 胀成两张票，而且 `#22` 等不起。
2. ~~**`streaming-json.ts`（559 行）提不提？**~~ **已定 (a)：提**（用户 2026-11-17）。
   `AGENTS.md` §一 随之从「只有 `google-generative-ai.ts`」改成「+ `utils/streaming-json.ts`」（`223cf61`）。
   落点 `nous-ai/src/utils/streaming_json.rs`，**只取四个导出** `JsonPath` / `JsonChange` /
   `StreamingJsonParser` / `ToolCallArgumentsStream`，其余不搬。
   两条更正（我自己写错过的）：
   - 「上游零命中」是错的措辞。真话是：**上游有实现、无测试**。`packages/ai` 里
     `ToolCallArgumentsStream` 被 `src/index.ts` 导出，并被 6 个 provider（`openai-completions` /
     `anthropic-messages` / `pi-messages` / `openai-responses-shared` / `mistral-conversations` /
     `bedrock-converse-stream`）+ `providers/faux.ts` 引用；`grep -rn StreamingJsonParser` 只命中定义
     文件本身（它只被 `ToolCallArgumentsStream` 内部用）。**没有任何测试文件**：
     `find . -name '*streaming*json*'` 只有源码本体。
     所以判据自建三条：(1) 完整合法输入上 `snapshot()` 等于 `serde_json` 的解析结果；(2) 把
     `append()` 返回的 `JsonChange` 依次应用到空值，**结果必须等于**同一时刻的 `snapshot()` ——
     这正是 `PartialChanges` 依赖的性质；(3) 任意前缀都不 panic、快照只增不减。
   - 「`rpi-ai` 无等价物」是对的（`/tmp/pi-rust/crates/rpi-ai/src` 全仓零命中
     `StreamingJsonParser` / `ToolCallArgumentsStream`），提货是新东西，不是重写旧的。

3. **3 条 delta 变体用构造数据**（§2.3）—— 认还是不认？不认的话这 3 行就永远没人验。

---

## 7. 依赖契约图（#89 收口：判据 4e 的单一真相）

`scripts/verify.sh` §4e 对 `cargo metadata` 里每个 `nous-*` 依赖做同一件事：
**若目标包名不在下表的 `allow` 集合里，就报「依赖边不在契约图表里」并判红。**
这张表的机器版是 `docs/dependency-graph.toml`（判据直接读它）；下表是给人看的同一份。
两处必须逐行相等；只改一处 = 空跑的绿。

| 包 | 允许的 `nous-*` 直接依赖 | 出处 |
|---|---|---|
| `nous-telemetry` `nous-chord` `nous-wire` `nous-store` | 无 | 契约 / 语义 / 第二个库，零内部依赖（#56） |
| `nous-env` | `nous-chord` | 路径与 shell 契约 |
| `nous-ai` | `nous-telemetry` `nous-ai-types` | provider trait 与纯类型 |
| `nous-tools` | `nous-ai` `nous-env` | 第一方工具 |
| `nous-runtime` | `nous-ai` | 执行绑定 |
| `nous-durable` | `nous-chord` `nous-ai` `nous-wire` `nous-telemetry` `nous-env` `nous-ai-types` | 语义层；永久禁 HTTP（判据另有断言） |
| `nous-server` | `nous-durable` `nous-wire` `nous-telemetry` `nous-ai` `nous-runtime` `nous-sessions` `nous-tools` `nous-documents` `nous-store` `nous-env` · **#65** `nous-device` · **#78** `nous-env-ssh` · **#81** `nous-ext` · **#72/#71/#83** `nous-prompts` `nous-skills` · **#75/#76/#77** `nous-repo` · **#62** `nous-stats` | 组装根：拼 HTTP 面就要摸到这些插件（#44/#47/#48/#49/#51/#57 原始边；#89 补齐 7 条实际边） |
| `nous-store` | 无 | 独立迁移链；多一条边（尤其 `nous-durable`）就是判据 1 破 |

**这张表不是许可证**：它只回答「这条边有没有人签字」，不替代判据 2（内核层不许依赖应用层）
或判据 §1.1（`nous-app` 的 normal 依赖里不许有 `rusqlite` / `nous-chord`）。扩边只在实现真的
需要该能力时做，并在上表标出票号；判据脚本里的阈值与断言不动。

### #89 为什么改读表方式

原表内嵌在 `verify.sh` 里，而票面要求「补 `docs/harness.md` 的图表并让判据读到」——内嵌表
使文档永远追不上判据。现在判据读 `docs/dependency-graph.toml`，文档 §7 是同一张表的说明；
新增“表为空即 RED”的哨兵，防止把表清空获得空跑的绿。断言循环逐字未变。

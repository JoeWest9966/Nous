# 决策台账（含**反悔点**）

这份文件只记「本库选了另一边，而上游/直觉在另一边」的决定。每条两段：

- **决定 / 理由** —— 现在凭什么。
- **反悔点** —— 什么信号出现时这条要翻，翻的时候动哪里。**没有反悔点的决定不许进这张表**：
  那不是决定，是习惯。

时间一律写「谁定的」。判据改动同样要记「谁改的、为什么、旧判据是什么」。

---

## 2026-11-17 · `message_update.changes[]` 的 8 个变体（上游已把它整条事件流砍掉）

**决定**：保留 `wire.md` 的 `message_update.changes[]` 与 `nous-wire` 的 8 个
`MessageChange` 变体（`text_start` / `thinking_start` / `toolcall_start` / `text_delta` /
`thinking_delta` / `toolcall_delta` / `block` / `message`），`#14` 自己写投影（上游源码不可抄），
`#22` 按它发 SSE 帧。

**理由**：这 8 个变体**不是从 pi 源码抄来的**，是从真流量（NousDesk ↔ 真引擎）的 SSE 帧里抄的。
pi 删不删不影响合同 —— 它删的是**它自己**的渲染路径。

**上游事实**（2026-11-17 核）：`packages/durable/CHANGELOG.md:76`

> Removed `watchEvents()` and its types `AgentEvent`, `AgentEventStream`, `SnapshotEvent`,
> `MessageChange`, and `ToolEventCall`, and spec §9.4 … A renderer renders state instead of
> assembling it from events.

`grep -rn MessageChange packages/` 全仓零命中；`wire.md` 里那条
「changes[] 全集来自 `pi-durable` 的 `dist/harness/events.d.ts`」的出处**已经过时**（旧 dist）。

**架构分歧的实质**：pi 认为「渲染器应该渲状态，不该拿事件拼状态」；我们反过来，因为
NousDesk 要的是**流式增量**（16 ms 合帧、按 contentIndex 拼文本），拿整帧状态做 diff 会把
客户端变成一个重放器，而且 diff 出来的 block 归属要靠猜。这是**客户端形态**的差异，不是对错。

**反悔点**：如果真引擎哪天也改成整帧状态（snapshot-diff，即一次 `message_update` 直接带整条
`message` 而不带 `changes`），`wire.md` 的 `changes[]` 那一段**整段作废**，`#22` 的投影换成
「状态差分」，`MessageChange` 8 个变体退役。

**判断依据（唯一的）**：连一次真引擎，看帧里还有没有 `changes`。不是看 pi 的仓库，也不是看
`wire.md` 自己的描述 —— 合同描述的是真引擎的行为，不是我们的偏好。

**这条为什么没有机器判据**：真引擎不在本仓，任何 `grep`/测试都只能看我们自己的代码，必然假红。
所以它是**人工哨兵**，写在 `docs/pitfalls-from-pi.md` 末尾（每轮动 `#22` 前连一次真引擎）。

---

## 2026-11-17 · deferred 持久化：不做（永久）

**决定**：不补 `session/transaction.rs` 的 `plans` / `holds`，`Session::commit` 继续对
`CommitPersistence::Deferred` 报 `SessionError::DeferredUnsupported`。harness 的 partial 一律
immediate 落库。上游的 `PROGRESS_COMMIT = { persistence: deferred }`（`live.ts:236`）**不提**。

**理由**：数字（`docs/bench-commit.md`）。真实节流是 100 ms 一次，16 秒流式输出 = 160 次提交，
ext4 + `synchronous=NORMAL` 下 **110 ms**；deferred 能省的只是把这 110 ms 摊薄，代价却是补一套
并行提交状态机 + 改 `#13` 的关闭语义（关会话时 hold 里的东西要落）。

**前提**：`synchronous=NORMAL` 已落进 `SqliteStore::open_writer`（同一天同一个数：ext4 上
默认 FULL 是 11,200 ms / 1,000 次提交，NORMAL 1,890 ms）。

**反悔点**：出现下面任一情况时重开：

1. 提交节奏从 100 ms 提到 16 ms（例如 `#22` 决定 partial 也按 SSE 合帧节拍落库）；
2. 单次提交的固定开销从 ~0.7 ms 涨到 >5 ms（再量一次 `commit_throughput`）；
3. 需要「一次事务里落多条 partial」（批量回放、导入）。

翻的时候先看 `docs/bench-commit.md` 的三条测试，**别凭印象**。

---

## 2026-11-17 · 提货 `packages/ai/src/utils/streaming-json.ts`

**决定**：从 `pi@v1.5.2` 提货 `streaming-json.ts`（559 行）进
`crates/nous-ai/src/utils/streaming_json.rs`，**只取四个导出**：`JsonPath` / `JsonChange` /
`StreamingJsonParser` / `ToolCallArgumentsStream`。`AGENTS.md` §一 的契约面据此从
「只有 google-generative-ai.ts」改为「+ utils/streaming-json.ts」（用户 `223cf61` 改的）。

**理由**：它是唯一能产出 `toolcall_delta` 的东西 —— 没有它，partial 提交只能整块重写
`arguments`，「提交成本跟着新文本走」这条性质在工具调用上就失效了。`rpi-ai` 里没有等价物
（`crates/rpi-ai/src/` 全仓零命中 `StreamingJsonParser`）。

**反悔点**：若 `nous-ai` 将来换成「整块 `arguments` 一次写」的协议（或者 provider 不再流式给
工具参数），这四个导出就没人用；那时删文件、删台账行，并把 §一 那行改回去。

---

## 2026-11-17 · 3 个 delta 变体的判据用构造数据

**决定**：`MessageChange` 的 8 个变体里，`text_start` / `thinking_start` / `toolcall_start` /
`block` / `message` 用 fixture 真数据驱动；`text_delta` / `thinking_delta` / `toolcall_delta`
用**构造**输入驱动，测试名带 `_constructed_`。

**理由**：fixture 的 `document_revisions` **969 行全是 `base`，0 条 delta**（
`python3 -c` + `sqlite3.connect('file:…?mode=ro', uri=True)` 查的，2026-11-17）。真库里读不出
一个 delta op，硬要「8 条全 fixture 驱动」只能造假数据还叫它 fixture —— 那是假绿。

**约束**（防止「构造」变成「随便写」）：构造的输入 op 必须来自 `PartialChanges::take()` 的
真产出；测试注释里带上那条 969/0 的查询，半年后有人看到 `_constructed_` 时知道为什么。

**反悔点**：等库里出现第一条 delta 修订（真跑起来一段流式之后），把这 3 条改成从库读；那时
`_constructed_` 后缀要一起删掉。

---

## 2026-11-17 · 成本类判据先说清测的是「输出的哪一面」

**决定**（两句）：

1. 凡判据的目的是成本 / 体积 / 延迟而非正确性，**必须说清它测的是输出的哪一面**：字节、
   账本、还是墙钟。选了错的面 = 假判据。
2. 配一条「把优化关掉就红」的负向；若实测出**关掉也不红**，说明判据测的不是这条优化 ——
   **改判据，不改说法**。

**留痕（老规矩三行）**：

- **谁刀的**：**助理起草、用户 2026-11-17 批准**（`64025bd` 之后那一轮）。压进 `AGENTS.md`
  §五 守卫表一行，全文留在这里。**「起草」不许省**：只写「用户」会让后来看台账的人以为这条是
  用户自己写的，而这条纪律的价值全在「谁做了什么能查出可追责的人」—— 省掉起草人这一层，
  就是在自己拆这条纪律的效力。
- **旧的说法**：「体积/性能判据必须配一条『把优化关掉就红』的负向对照」——**已证伪**，
  见下面那段反例。
- **为什么改**：漏了反例条件。反例一出现，旧说法只能靠「改说法」圆场，而这正是第 2 条要禁的
  事；新说法把「先选对面的」放在前面，反例就成了判据选错的信号，而不是规矩的例外。

**反例（这条规矩的来历，必须连着读）**：`#14` 落 `partial.rs` 时，原以为「把
`AssignPlan::Append` 换回整块 `set`，体积断言就会红」。实测**不红**，根因在
`crates/nous-chord/src/delta/draft.rs:164`：

```rust
// prepare() 永远先跑 diff
let diff = diff_revisions(&self.base, &self.value).into_ops();
if self.appends.is_empty() { return Ok(TrustedOps::unchecked(diff)); }
// 账本只是**覆写** diff 的 op（append_override），不产生 op
```

所以**账本省的是 CPU，不是字节**：它免掉的是 diff 里那条
`after.slice(0, before.len()) === before` 的 O(len) 前缀比较和 `overlap()` 最多 65,536 的
扫描。值相等时「逐叶写」与「整块写」的字节必然一样 —— 旧判据暗中假设了「计划被用上 ⇒ 输出
不同」，而计划的作用恰恰是**不重新推导**，不改变结果。

**实测证据**（`64025bd`）：把 `take()` 里的 `Append` 换成 `Keep`，7 条判据里**只有账本那条
红**，其余 6 条全绿。于是那条判据的正向断言改成「字节 + 追加账本」两面都测，负向对照拆两段：
(a) 整块写 → 断言「字节判据分辨不出」（把这条假判据钉在原地，防止下一个人又去指望字节）；
(b) 手搭「每次带整段正文」的 op → 判据必须抓住（证明**度量本身**不是空转）。

**同一条规矩的第二个数据点（用户 2026-11-17 指出、助理落实）**：`#14` 的节拍判据原来断言
「partial 提交次数在 `2..=5` 之间」。这是**同一种错**的另一种长相 —— 不是选了错的面，而是
**把还能收紧的判据放宽**。`start_paused` 下虚拟时钟精确，次数算得出来（40 片 × 10 ms、闸
100 ms ⇒ 恰好 3 次），区间 `2..=5` 恰好给「在 `finally` 里补一帧」（4 次）留了位置。改成
`assert_eq!(3)` + 逐个钉住 110 / 210 / 310 ms 之后，补一帧实测变红（4 ≠ 3）。**区间只有一个
正当理由：时钟抖动；虚拟时钟不抖，就没有理由。** 收紧后连「闸第一次在哪一刻触发」
（110 ms 还是 10 ms）也一起钉住了。

**同一类的第三面（同一天补记）**：这种「放宽」还有第三种长相 —— 判据写的面**对**，但**阈值
挑在噪声里**（`commit_throughput` 那 3,000 ms 预算刻意留了两倍余量，那是另一种取舍：宁可漏报
也不假红，且那里有散布区间记录）。三面的分野：选错面 → 改面；能收紧没收紧 → 收紧；阈值容噪 →
写下容噪的理由。第三种**不**适用于虚拟时钟。

**反悔点**：若哪天 `Draft::prepare` 不再用 diff 兜底（改成只信调用方记的 op），这个反例作废
——账本那时才真的决定字节，负向对照要回到「换回整块 `set`」那种最直白的写法。

---

## 2026-11-17 · `PRAGMA synchronous=NORMAL`

**决定**：`SqliteStore::open_writer` 设 `synchronous=NORMAL`。

**理由**：数字（`docs/bench-commit.md`）。ext4 上 1,000 次提交默认 FULL 11,200 ms、NORMAL
1,890 ms —— 82% 的时间在等 fsync。代价是掉电丢最近若干个提交（库不会坏）。

**反悔点**：若本库开始承载「掉电也不能丢最后一笔」的东西（例如账务类写入，或用户明确要求
durable-until-acked），把这条 pragma 去掉，同时重跑 `commit_throughput` —— 那时 deferred 的
决策要跟着重看（见上一条的反悔点 1）。

## 2026-11-17 · 冷启动判据的对象写死成 `nous-server`

**决定**：`verify.sh` 第 3 段不再从 `cargo metadata` 里取第一个 bin target，直接 `BIN=nous-server`；
`nous-fixture` 的逻辑测试留在第 1 段（`cargo test --workspace`），不占这一格。

**留痕（老规矩三行）**：

- **谁刀的**：用户，2026-11-17（`#22` 合并后那一轮），助理起草改动。
- **旧判据**：`BIN=$(cargo metadata … next((t["name"] … if t["kind"]==["bin"])))` —— 取第一个 bin target。
  单 bin 时期（只有 `nous-fixture`）这是对的，也因为只有那一个。
- **为什么改**：`#22` 加了 `nous-server`，第二个 bin 一出现，这一格取到谁**取决于包名遍历顺序**。
  实测它碰巧取到了 server（rc=124，一直活着），于是判据看起来「验了服务端」——但那是运气，不是判据。
  加一个 crate 就可能静默换对象，而这一格照旧绿。**判据不许依赖顺序。**

**反悔点**：哪天真的需要同时验两个 bin（例如给 `nous-fixture` 单独做退出码判据），就写成显式列表
`for BIN in nous-server nous-fixture`，每个对象一条断言——仍然不许回到「取第一个」。

**同一条判据的第二次修正（同日晚，假红逼出来的）**：改完立刻撞上一个假红 —— 我为了给你演示把服务
挂在 `0.0.0.0:4789` 上，于是 `verify.sh` 第 3 段报 `Address already in use (os error 98)`。
**那不是代码坏了，是判据挑了个会和真实服务抢的端口。** 改成 `NOUS_SERVER_ADDR=127.0.0.1:0`（临时端口），
并把「到底 bind 上了没有」变成断言：日志里必须出现 `listening on 127.0.0.1:`。
旧写法只看 `rc`，起不来但退出码是 0 也算过 —— 那是判据的另一半漏洞，一起补了。

---

## 2026-11-17 · `Draft` 的克隆成本过线 → 开 `overlay` 树，不做 `piece tree`

**决定**：`crates/nous-chord/src/delta/tracker.rs`（上游 `tracker.ts` 的 overlay 节点树）从
「不做」改为「#12 之后的阻塞项」；**不同时做** piece tree。

**理由**：数字（`docs/bench-delta-clone.md`）。20,000 次追加：N=100 时 p99 0.31 ms、N=1,000 时
0.47 ms、N=10,000（1 MB）时 **8.07 ms** —— 闸是 2 ms，过线点约 2,300 条消息 / 240 KiB。
形状判据（同样 1 MB，1 万节点 vs 1 节点）p50 差 12–13 倍（3,985 vs 338 µs）⇒ 成本跟**节点数**走；
1 MB 字符串整段 memcpy 只要 ~340 µs，本身就在闸线内。所以 piece tree 治的不是这里的瓶颈 ——
「两个都做」或「先做 piece tree」都是没看数字。

**反悔点**：某天出现单段 >5 MB 的正文，或 overlay 树落地后 N=10,000 的 p99 仍 >2 ms 且形状判据
翻到胖摆法那边，就重开 piece tree。判据已经写进 `delta_clone_cost_shape`，重跑一次比值会自己说出来，
不用凭感觉翻案。


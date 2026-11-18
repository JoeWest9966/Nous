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


## 2026-11-17 · 模块位置 `crates/<vendor>/<module>`，分层看清单不看路径

**决定**：目录只表达所有权（vendor），层写在 `docs/plugins.md` 的 `layer` 列，取值
`kernel` / `platform` / `app` / `dev`。所有判据的"内核层 / 应用层 / 要排除的夹具"都由
`scripts/layer-paths.sh` 从这一列算出来，**判据正文不写死模块名或目录名**。边界基线与
依赖允许边表只在本仓存在一份（`docs/boundary-baseline.txt` + `verify.sh` §4），spec 仓只留
指针。

**理由**：#14 之前同一个 crate 在不同判据里被三种写法点名，搬一次目录要改一遍判据；清单是
唯一可执行的真相，改名/搬家只动一行。`dev` 是给探针和夹具的第四个值——它是**信号源**
（哨兵要它命中），不是要放宽的对象，所以"减 dev 层"必须留下排除了哪些行的输出。

**反悔点**：出现「同一个模块同时属于两层」（例如一个 crate 既是平台又是夹具），或清单里
`layer` 列开始需要条件表达式 —— 说明四值不够，先改清单语义，别在判据里加特例。

## 2026-11-17 · 判据的排除必须可观测；关票前跑全量 verify

**决定**：凡是"排掉一些行再看是否为空"的判据，必须打印「排掉了 N 行，来自 <文件×次数>」并
断言 `N > 0`；凡是关票，必须跑一次**全量** `./scripts/verify.sh`，不是只跑自己那几条。

**理由**：排除会以两种方式骗人——路径拼错排掉 0 行（和排对了长得一样），或多写一个 `_`
少排一个文件（集合数字变了，看着像数据变）。不打印来源就分不清。而 `#18`（T2 拆包）关票时
只验了自己那几条，漏了 §4 依赖允许边表，导致 main 上至今红着两条——**一条红没人认领，就会
变成背景噪音**。

**反悔点**：某条判据的"排除量"本身不稳定（依赖环境、依赖网络）→ 那就不是排除，是环境依赖，
要改成 SKIP 并写明依赖什么；别为了 `N > 0` 去凑一个固定值。

---

## 2026-11-18 · `Session` 公开面从 5 改 6：建会话进 `Session`，不开第二条写通道

**决定**：`nous-durable::session::Session` 加第六个 async 方法 `create_conversation(req)`；
`POST /v1/sessions` 经它建会话，`DurableCommands` 只持一条 `Session`。判据
`tests/session.rs::session_public_face_is_exactly_the_a3_table` 的 `A3_METHODS` 从 5 条改 6 条，
理由在票面（#42）与本条同时签字。

**理由**：#42 起初为绕开「`Session` 公开面恰好五个方法」这条判据，在 `DurableCommands` 里
开了第二条裸 `Storage`（`create_storage`）。那不只是风格问题：裸 `Storage` 不挂
`ObserverRegistry`，建会话不广播任何帧，多客户端下别人看不到新会话。用架构迁就判据，
和禁掉的「单行豁免」是同一类东西。**建会话是写**（改库、要广播），和 `commit` / `fork`
同类，本就该走 `Session` 的同一把写锁与同一份 publication。请求/响应形状
（`CreateConversationRequest` / `CreatedConversation` / `ModelRef`）下沉到 durable 的
`session::commands`，端点层复用，不让 `Session` 的签名反向依赖 `nous-server`。

**反悔点**：如果 `create_conversation` 哪天退化成只写一条元数据、和 entry/task 提交
不共享同一个写者与观察者（即建会话不再需要广播帧），就把它从 `Session` 挪出去，
并把公开面判据改回 5 条 —— 同样要在这里签字、同时改票面，不许再偷偷开第二条通道。

## 2026-11-18 · `Session` 公开面 6 → 8：`mint_id` / `entries`（轮次要的两个原语）

**决定**：`Session` 加两个方法 —— `mint_id()`（取一个 entry/文档 id，随下一次 `commit` 落盘）和
`entries(conversation, limit)`（按 fork 链 + `head` 标记读历史，最老的在前）。
`tests/session.rs` 的 `A3_METHODS` 从 6 条改 8 条，签字在此。

**为什么**：轮次要两样东西，别处都拿不到 ——

- **id**：终态 assistant entry、`pi.live` 文档、每条 `pi.tool-result` 都有自己的 id，而这套 id
  必须由**同一个写者**发（`storage.mint_id` 在 `Session` 的锁里）。绕过去的唯一办法是应用层自己
  编号，那就是第二条写通道。
- **历史**：喂给模型的上下文必须包含 fork 祖先链上的 entry。按 id 硬扫
  （`for id in 1..100_000 { get_entry(id) }`，上一版交付就是这么写的）有两个后果：每轮最多
  10 万次加锁查询，且总条目超过 10 万之后**静默丢历史**。`Storage::scan_entries` 早就有正确实现，
  只是没从 `Session` 出去。

两条都是**读 / 取号**，不是写：写仍然只走 `commit`。

**反悔点**：如果哪天 `entries` 被用来做"窗口裁剪"（按 token 数截断），它就不再是薄封装 ——
那时该把裁剪放进单独的模块（`head` / 压缩），而不是让 `entries` 悄悄少返回几条。

## 2026-11-18 · 组装根的语义：解析不出 provider 就 503，不许"先受理再假装"

**决定**：`/v1/sessions/:id/messages` 的顺序是 **先解析 provider → 再受理 → 再 spawn 轮次**。
解析不出来（没配凭证、会话没有 `model`）返回 **503**，**不写 `pi.user` entry**。

**理由**：上一版交付把 `offline_provider()`（空脚本 faux）当生产 provider，`/messages` 里只有
`let _driver = runtime.driver.clone();` 一行 —— 受理照给 202，回复永远不会来。用户看到的是
"发出去了、没反应"，比直接报错难查一个量级。三种诚实做法里最容易做对的是**受理前就失败**：

- 409 不行：那不是状态冲突；
- 500 不行：我们没写错，是宿主没配；
- 503 = "现在起不来"，客户端该把入口灰掉，而不是重试到天黑。

配套的两条：

- `nous_sessions::turn::TurnHandle::faux` 是**离线一等公民**（生产装配走 `TurnHandle::new`），
  端到端判据靠它不联网地跑完整条链；
- `nous-server` 的库不依赖 `nous-ai`：句柄由 sessions 给，测试通过 `TurnFactory` 接缝注入 faux。
  `scripts/verify.sh` 4e 的依赖表**只算 normal/build 边**（dev 边不进产物），否则"给端点写一条
  端到端判据"和"依赖方向判据"会互相打死。

**反悔点**：如果哪天需要"离线/回放模式"给用户选（比如没有凭证时用 faux 演示），那就得**显式**
打开一个开关并在 `/v1/capabilities` 里声明 `provider: "offline"` —— 绝不能是默认行为。

## 2026-11-18 · capabilities 的 `name` 强制等于清单的 `module` 列

**决定**：`GET /v1/capabilities` 的 `modules[].name` **逐字**用 `docs/plugins.md` 的 `module` 列
（`ext` 就是 `ext`，不是 `extensions`）。`enabled` = **清单 ∩ 代码**：清单关掉 → `disabled_by_manifest`、
清单开但没实现 → `not_implemented`、冻结 → `frozen`、两者都有 → `enabled:true` + `endpoints`。
`reason` 是 `{code, message}`，`code` 四值封闭，**客户端只许按 `code` 分支**。

**为什么**：`AGENTS.md` 说"NOUS 不是代码，是一张清单"。表管**装什么**、注册表管**有哪些端点**，
capabilities 是交集 —— 这正是"一个发行版 = 一张清单"能被客户端看见的那一面。名字一旦各起一套
（表里 `ext`、协议里 `extensions`），两边迟早各按自己的叫法写分支，那就是两套真相的起点。
代价是：**新增功能面必须先加表行，再写端点**（不许先出端点后补行 —— `#46` 就是对着记忆里的
端点表开的票，票面数字本身是错的）。

**落地**：`#47` 的 11 个 `name` 已在 `docs/plugins.md` 补齐 8 行（`documents` `presets` `skills`
`archive` `combos` `triggers` `device` `selection`）。UI 文案（"扩展"）不算名字。

**反悔点**：如果哪天同一个端点族要按租户/发行版给不同名字，那就说明 `name` 被当成显示名用了 ——
那时该在响应里加 `label` 字段，不是让 `name` 变。

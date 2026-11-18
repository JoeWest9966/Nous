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

---

## 验收修复 · `qrd3wh.sessions` 是 session 作用域单例：建会话合并、不新建

**决定**：session 作用域的 `{kind:"qrd3wh.sessions", scope:{kind:"session"}}` 地址只允许一份
在世化身。`create_conversation` / `rename_session` / `set_pin` 共用 `session::session_document`
的 `session_scope_document` + `upsert_session_document`（`find_document` → `materialize` → 一次
`DocumentChange`；只有一份都没有才 `DocumentCreate`）。每次只改自己管的那个键（`names` 的
一条 / `pins` / `marks`），整份旧值写回；rename 不再用 `{"names": ...}` 覆盖掉 pins/archived。

配置字段：`create_conversation` 不再写顶层 `config` —— 全仓 grep 没有读者，写进单例文档只会
给第二份会话制造覆盖与歧义（既没有 `configs` 的消费方，就不假装有）。端点层 201 回包仍原样
带出 `config` 的生效值；`GET /v1/sessions` 仍只读 `names` / `pins`，旧 fixture 兼容不受影响。

**理由**：storage 按地址保证当前化身唯一，第二次 `DocumentCreate` 直接 500
（`Document address already has a current incarnation`）。名字表本来就是整个 session 一份，
不是「每条会话一份」；`fork` 带名字时也走同一条 `merge_session_document` —— 分出来的是一条
新会话，不是新地址。若以后真有人读配置，按 id 建 `configs` map 并合并写，别回到顶层单键。

**反悔点**：若将来 12c 给会话配置正式落文档契约（例如新的 kind 或 conversation 作用域），
这里只保留 names/pins 的兼容读取，新配置写新文档；不要在 `qrd3wh.sessions` 里复活 `config`。

## 2026-11-18 · 存储分离 = 两个文件：`nous.db` 走自己的迁移链（#56）

**决定**：触发器与触发账（`triggers` / `fires`）进**第二个库** `nous.db`，由新 crate
`crates/nous/store`（`nous-store`，layer = `platform`）独占所有权；`durable.db` 一个字节都不碰。
store 的迁移链是**自己的**：版本表叫 `schema_version`、`CURRENT_VERSION = 1`，跟 durable 的
`durable_schema`（= 2）各走各的。依赖面上 `nous-store` **只依赖 `rusqlite`**，不依赖 `nous-durable`。

**理由**：触发器是**我们自己的插件**造出来的实体，pi 的语义层（session / entry / document /
submission / task）里不该存在这个概念 —— 就像 Magento 2 的「商品」存在 Magento 的库，而不是
PHP 运行时的类型表里。表放同一个文件也不是不行，但那样「谁的实体」就没有物理边界了：
durable 的迁移一改，触发器的数据跟着承担风险。两个文件把这条边界变成可判的（判据 1/2）。

**为什么只能「幂等 + 补偿」**：WAL 下 `ATTACH` 跨库事务**不原子**（SQLite 文档：只有非 WAL 才保证），
所以「一个事务写两个库」这条路不存在。顺序因此写死：① 先插 `fires(state='pending')`（唯一键
`(trigger_code, fire_at)` 去重）→ ② 再调 durable 的 `submit_message(requestId = trigger:<code>:<fire_at>)`
→ 成功 `done`，失败/崩溃留在 `pending`，超宽限期扫成 `orphaned` 后重放 ②（幂等，安全）。
**反过来写不安全**：先提交后记账会留下「durable 里有 entry、Nous 不知道谁触发的」。

**文件名还没定死**：本票不接线，`Store::open(path)` 只收路径，代码里不认任何文件名。
今天 `deploy.sh`（#52）把它部署的那个库叫 `nous.db`（= durable 的库）；接线那天得把两个名字
理顺（durable 的叫 `durable.db`、store 的叫 `nous.db`，或者 store 直接叫 `triggers.db`），
在那之前任何一边写死文件名都会让部署脚本说两件事。

**一处额外选择**：`reclaim` 也重放**已经** `orphaned` 的行（不只 `pending` 超期的）。只认 `pending`
会在「标了 orphaned、还没重放就崩了」那个窗口把一次触发永远丢掉 —— 重放是幂等的，多补一次比丢掉便宜。

**反悔点**：如果哪天要「一个事务同时写触发器与 entry」（例如触发器状态必须和消息严格一致），
那得先把两个库合成一个文件，并把这个 crate 并回 `durable` 的迁移链 —— 同时删掉判据 1/2/7，
票面签字；合并之后日志/审计的边界也要跟着重画。反方向（把 `reclaim` 收回成只认 `pending`）
可以随时做，只要调度循环自己记「已重放过」。

## 2026-11-18 · 判据改动：数据面入口从「唯一」改成「**可数的**」（#56）

**决定**：`verify.sh` 判据 5 的标题与实现从「数据面唯一入口」改成「数据面入口**可数**」。
入口集合 = `$K`（内核层）∪ `[package.metadata.nous] sqlite-entry = true` 的 crate（今天只有
`crates/nous/store`）。名单从 `cargo metadata` 读，脚本里不点名目录；每一条入口都必须出现在
「排掉了 N 行」的清单里（豁免规则活着才绿）。

**留痕（老规矩三行）**：

- **谁刀的**：用户，2026-11-18（#56 票面「依赖分离 = 两个库」），助理起草改动。
- **旧判据**：`$K` 之外不许出现 `rusqlite|Connection::open|.sqlite|PRAGMA|BEGIN IMMEDIATE`。
  单库时期这是对的，也因为只有 `durable` 一个库。
- **为什么改**：第二个库（`nous.db`）一落地，旧判据必然红 —— 而它红的原因**不是**代码错了，
  是判据把「只有一个数据面」写死成了「只能有一个入口」。要么这条判据挡住第二张票，要么
  有人图省事把触发器塞进 durable（正是本票要防的）。所以判据跟着改口径，而不是代码跟着判据走。

同时收窄了 `scripts/check-separation.sh` 判据 1 的 grep：票面写的是
`grep -rn "trigger|fires|plugin_snapshot" == 0`，但 `crates/nous/durable/src/harness/live.rs`
里有一个 pi 自己的字段 `trigger: CompactionReason`（压缩触发，跟我们无关）。照票面字面扫，
判据一落地就假红；要它绿就得去改 pi 的字段名 —— **判据不许指挥代码**。所以实际 pattern 是
`trigger_code|triggers|fires|plugin_snapshot|nous_store`，并用「同一 pattern 在 store 里必须命中
≥ 1 行」当正向对照（pattern 写错就红）。

**反悔点**：第三个数据面入口出现时（比如审计自己的库），**先问它该不该是
`sqlite-entry`**；如果三个以上入口让「数据面可数」失去意义，就把这条判据改成
「每个入口一个 crate、每个 crate 一行 metadata」的清单体检，别再靠 `$K ∪ metadata` 的并集。

---

## 2026-11-18 · 收口：判据 4e 的契约图扩边 + `NOUS_TOKEN_FILE` 真的读

**背景**：`#47`–`#56` 十张票全在 `integration/desk-connect` 上，`origin/main` 一条都没有。
收口前先在 integration 上跑全量判据，抓出两件事：

**① 判据 4e 那张 allow 表是旧的，而且 main 早就红着。** `origin/main` 上实测 4 条越界边
（`nous-server → nous-ai / nous-runtime / nous-sessions / nous-tools`），integration 上 5 条
（多一条 `nous-documents`，来自 `#48`）。也就是说 `#44/#49` 合进 main 那天起 4e 就在红，
只是没人跑。这不是 #56 造成的（它的提交没碰 `nous-server/Cargo.toml`）。

**决定：给 `nous-server` 扩边**，理由是它现在是**组装根**（拼 HTTP 面就必须摸到这些层）。
要守的是「内核层不依赖应用层」（判据 2），那条没动。**判据段落只有人能改**，这次由用户签字。

```rust
'nous-server': {'nous-durable','nous-wire','nous-telemetry',
                'nous-ai','nous-runtime','nous-sessions','nous-tools','nous-documents'}
```

**学到的**：判据红着没人看 = 判据不存在。`verify.sh` 的退出码必须在 CI 里，否则这类红会躺一个月。

**② `deploy/nous-app.service` 说 token 在 `/etc/nous/token`，代码却读 `$HOME/.config/nous/token`。**
`NOUS_TOKEN_FILE` 只写在 unit 里，`auth.rs` 从来没读过。真部署的后果：`deploy.sh --rotate-token`
轮转的钥匙**根本没生效**，进程按"没有 token"回环放行 —— 认证静默失效。
（这个洞是被 `#51` 的 faux 测试抓出来的：它在我这台机器上 401，因为 `/root/.config/nous/token` 存在。）

**决定**：`auth::token_path()` = `NOUS_TOKEN_FILE` 优先，否则 `default_token_path()`。
`scripts/deploy.sh` 生成钥匙、unit 指路、代码只读 —— **三处一个路径，不许各猜一个**。
判据 `crates/nous/server/tests/token_path.rs`：`NOUS_TOKEN_FILE` 那把能用、`$HOME` 那把**不认**、
不带 header 401。负向哨兵验过（把 `token_path()` 换回 `default_token_path()` → 红）。

**反悔线**：如果哪天要做多把钥匙（按主体分权），先换 `auth.rs` 这一个文件，
别在 unit 里堆 `NOUS_TOKEN_FILE_2`。
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

## 2026-11-18 · 文档双语 = **单文件两段**；API 章节是**生成物**

**决定**：每个插件（= 一个 crate）带 `plugin.toml`（清单）与一份 md；md **同一个文件里**中文在上、
英文在下，顶部两个锚点 `[中文](#zh) · [English](#en)`。`## 3 清单` 与 `## 4 API` 两章由
`scripts/gen-plugin-docs.sh` 从注册表（`/v1/_routes`）+ `plugin.toml` **生成**，其余章节手写。

**为什么不拆成两个文件**：`README.md` / `README.zh.md` 那种写法，**必然只更新一份** —— 这不是洁癖，
是稳赔：改代码的人只改一半，半年后两份文档说的是两套东西。单文件两段至少让"改一处"是默认行为。

**为什么 API 表必须生成**：「文档里有这个端点、实际没有」**比没有文档更坏** —— 读文档的人（尤其 agent）
会照着做，然后撞一个不存在的接口。这跟 `#47`/`#48` 里「capabilities 的 endpoints 由注册表生成、
不许手抄字面量」是同一条规矩，只是延伸到文档。

**为什么新格式双语起步、旧文档按需迁移**：`docs/` 几十份全是中文。一次性全译，译完代码一动就开始过期，
而"过期译文"比"没有译文"更容易骗人（读的人以为权威）。

**落地**：`#55`（`plugin.toml` × 15 + 校验脚本 + 模板）；`## 4 API` 的生成器在 `#48` 的 `_routes`
落地之后单独接（在那之前第 4 章只写一行「待生成」，**禁止手写占位表格**）。

**反悔点**：如果哪天要给文档做站（点一下切语言），源文件**仍然是这一份**；生成 HTML 时再拆。
一旦发现有人开了第二个语言文件，就地合并回单文件，别两边并行。

## 2026-11-18 · 存储分离：**两个 SQLite 文件**，Nous 的东西不进 pi 的库

**决定**：两个文件、两个域，各自一条迁移链。

```
/var/lib/nous/
  durable.db   ← pi 的地盘：session / entry / document / submission / task（语义照抄上游，不加我们的概念）
  nous.db      ← Nous 的地盘：triggers / fires / plugin 清单快照 / 审计
  logs/*.jsonl ← 观测日志：只追加、按天轮转（不进任何库）
```

**为什么**（原话保留）：触发器是**我们自己开发的插件**造出来的东西，跟 pi 无关。就像 Magento 2 里
「商品」是 Magento 的实体、存在 Magento 的库 —— 触发器是 Nous 的实体，存在 Nous 的库。
pi durable 眼里**不该存在**触发器这个概念。分清楚域，才好维护、好扩展；将来 Nous 侧真要换 MySQL，
只换那一半，pi 那半边一动不动。

**为什么不 MySQL**：不是因为占资源（用户明确说不在乎），是因为**对当前形态没有收益**、且会把
`#52` 的整套"一个文件"设计（备份 / 回退 / schema 闸门）拆成两套运维动作。
日志也不是 MySQL 的活（行存 + 二级索引写放大）。

**技术硬事实（决定实现方式）**：SQLite 的 `ATTACH` 跨库事务原子**仅在非 WAL 时成立**，而我们是 WAL
（`storage/mod.rs:59`）。所以「一个事务同时写两个库」**这条路不存在**。跨库只能靠**幂等 + 补偿**。

**四条铁律**：
1. **独立迁移链**：`nous.db` 自己的 `CURRENT_VERSION`，与 durable 的 `2` 各走各的。
2. **单向依赖**：Nous 侧可以调 pi 的提交接口；**pi 侧永远不知道 Nous 侧存在**（不 import、不反向 ATTACH、不跨库 JOIN）。
3. **跨库只有一处，顺序写死**：先写 `nous.db` 的 `fires(trigger_code, fire_at)`（唯一键，天然去重），
   再调 `submit_message(requestId = trigger:<code>:<fireAt>)`；第二步失败留 `orphaned`，补偿扫描重放（幂等，安全）。
   **反过来写不安全**：会留下"durable 有条 entry、Nous 不知道谁触发的"。
4. **第二个写者要签字**：Nous 侧有自己的连接、自己的 `Storage` 语义，这是**明确允许**的第二写通道
   （跟 `#38`/`#42` 的处理方式一致：签字而不是绕），但它**只写 `nous.db`**，一个字节不碰 `durable.db`。

**命名**：`#52` 原来的 `nous.db`（实指 durable）改叫 `durable.db`，Nous 自己的才叫 `nous.db`。
「`nous.db` 里放的是 Nous 自己的东西」一眼就懂，省掉以后每一次误读。

**反悔点**（任一触发就重开）：① 出现第二个**进程**写库；② 单个库 > 10 GB 或写事务 p99 > 50ms（实测）；
③ 要跨多台机器**共享写**（不是多点读）。到那天**先换 Nous 侧那一个库**，durable 最后才动。

---

## 2026-11-18 · 收口合并：`#44` 存在**两份实现**，取 integration 那份

**事实**：`crates/nous/sessions` / `server` 在两条线上各被写了一遍 —— `main` 上的 `63f476a`
与 `integration/desk-connect` 上的 `f7e1ed5`。同一个目录、同一个目的、不同实现（`turn.rs` 各自
300+ 行），所以两边合并时每一处都是「添加/添加」冲突，`git` 没法替我们选。

**决定：整个 `crates/` 取 integration 那份**。理由不是谁写得好，而是 integration 是**十张票
一起跑绿**的那条线（`verify.sh` 55 条全绿、`cargo test --workspace` 1134 条全绿）；
main 那份只被 #44 一张票验过，合并进十票的实现里没人验过。

**代价（写清，别装作没有）**：main 那份的**时钟注入**（`sessions/src/clock.rs` + `with_clock`）没保留，
integration 用 `chrono_millis` 直接取时间。也就意味着 `nous-sessions` 那一层暂时无法用假时钟穷举测。
**这条列成待办**：把时钟注入补回 sessions（一个 `Clock` trait + 一处注入 + 一条判据），
不做就会在某次"时间相关 bug"上付出比现在更贵的代价。

**同时删掉的两个文件**：`crates/nous/server/tests/turn_end_to_end.rs`、`crates/nous/sessions/src/clock.rs`
—— 它们测/提供的是**已经不存在的 API**（`TurnHandle` / `TurnFactory`），留着会编译不过。
端到端覆盖没丢：integration 自己那份 `sessions/tests/turn_end_to_end.rs` 还在。

**反悔点**：如果哪天发现 integration 那份 #44 缺了 main 那份的某个能力（例如历史重放 `load_history`），
用 `git show 63f476a:crates/nous/sessions/src/turn.rs` 取回来当参照 —— 两份都还在 git 里，没有真丢。

---

## 2026-11-18 · 两个库的文件名定稿：`nous.db`（durable）+ `nous-plugins.db`（store）

`#56` 那条「存储分离」留了一句**文件名还没定死**，今天定掉，理由如下：

- **durable 的库继续叫 `nous.db`**：真机上已经在跑，而且 `deploy.sh` / 备份 manifest / `--check` /
  `--active-tasks` / 升级闸门全都引用这个名字。改名 = 数据迁移 + 备份链断一次，换不到任何好处。
- **store 的库改叫 `nous-plugins.db`**（原来是 `nous-triggers.db`）：这张库的所有者是
  `nous-store`（插件侧），触发只是它**第一个住户**；往后审计、别的插件数据还进这张库。
  **名字跟所有者走，不跟第一个用户走** —— 叫 `triggers` 的话第二个住户落地就得再改一次名，
  而改数据文件名是迁移，不是重命名。
- **环境变量与文件名同字**：`NOUS_PLUGINS_DB` / `nous-plugins.db`（原来 `NOUS_TRIGGER_DB`）。
  一条规则，少一次「变量叫 A、文件叫 B」的对照。

**落地面**：`crates/nous/server/src/main.rs` 的默认路径与变量名、`scripts/deploy-check.sh`
判据 12 的白名单（两个库都认，清单外任何文件仍然红）。

**反悔点**：如果哪天要「一个插件一个库」（真隔离），那是把 `nous-plugins.db` 拆成
`plugins/<module>.db`，判据 12 的白名单改成扫目录；别一个个加白名单条目 —— 那会变成
「数据目录里有什么都不奇怪」。

---

## 2026-11-18 · 旧面板下线完成（①–⑤），`content/` 从仓库移除

当初那条 ①–⑤ 的第 ⑤ 步写的是「只删壳、**保留 `content/`**」，前提是**引擎当时就住在
`content/new1/map/qrd3wh/`**（Node 版）。今天这个前提没了：引擎换成 `nous-app`（Rust，
`/opt/nous-app/current/nous-app`，数据 `/var/lib/nous/nous.db`），`content/` 只剩旧面板自己读。

所以按 ①–⑤ 做完最后两步：

1. **归档**：`/var/backups/nous-panel/content-20261118T094929Z.tar.gz`（6.2 MB，含
   `qrd3wh/data/agent.sqlite` 等旧引擎数据）。
2. **停用**：`systemctl stop + disable nous-panel`（unit 文件**保留**在原地，随时可回）；
   4790 不再监听，`nous-app` 全程 active 未受影响。
3. **移出仓库**：`content/`（8.1 MB、316 个文件）从版本控制里删除。**不是抹掉历史** ——
   任何一个 RFC 之前的提交里都还在。

**顺带**：GitHub 的语言占比此前是 Rust 71% / JS 18% / HTML 5%，因为按字节算 `content/` 比
Rust 还大。删掉之后这个仓就是它实际的样子（Rust 为主）。`.gitattributes` 里那条
`content/** linguist-vendored` 一起删了 —— 不删文件只把它标成「不算」是自欺。

**反悔点**：旧引擎的历史会话如果要迁进 `nous.db`，从那个 tar 里取 `agent.sqlite`
（`fixtures/agent.small.sqlite` 就是它的抽样）。别从 git 历史里翻 `content/` —— 那只是同一份。

---

## 2026-11-18 · tag 改用语义化版本，与 `Cargo.toml` 同源

**决定**：从这一版起 tag 用 `vMAJOR.MINOR.PATCH`（今天打的是 **`v0.1.0`**），**不再新增**日期编号的 tag。
旧的 `v2026.11.14` 留着当历史，不改名、不删。

**理由**：日期 tag 说得出「哪一天」，说不出「能不能升级」——`deploy.sh` 判幂等、`--check` 比 schema、
客户端显示版本，靠的都是**可比大小**的版本号。两套编号并存 = 两处真相，同一个提交有两个名字，
迟早有一天要回答「v2026.11.18 和 v0.2.0 哪个新」。

**唯一真相**：tag 的版本 == `Cargo.toml` 的 `[workspace.package] version` == `nous-app --version` 里那个。
升版本就三处一起动，tag 只打在这一个数上。

**反悔点**：真要按日期发（像发行版那样每天一版），就把 `Cargo.toml` 的版本也改成日期串 ——
**只许有一套**，别一边 semver 一边日期。

# 决策台账（含**反悔点**）

这份文件只记「本库选了另一边，而上游/直觉在另一边」的决定。每条两段：

- **决定 / 理由** —— 现在凭什么。
- **反悔点** —— 什么信号出现时这条要翻，翻的时候动哪里。**没有反悔点的决定不许进这张表**：
  那不是决定，是习惯。

时间一律写「谁定的」。判据改动同样要记「谁改的、为什么、旧判据是什么」。

---

## 2026-11-26 · 自省三件套：**原料归宿主，视图归插件**

**决定**：`/v1/_routes`、`/v1/capabilities`、`/v1/openapi.json` 搬进 `Device` 插件，但
「注册表」本身不进插件 —— 走宿主能力 `Host::manifest()` + `Host::registered()` 两份原料。
`enabled` / `reason` / `not_implemented` 的判定、JSON 键名、OpenAPI 的 paths 折叠，**全在
插件里**（`crates/Nous/Device/src/routes.rs` 的 `capabilities_body` / `routes_body` /
`openapi_body`，纯函数，5 条单测）。

**为什么不是「注册表也搬进插件」**：注册表 = 静态表 ∪ **所有**插件的 `routes()` 声明。
它天然是「谁看得见全部」的角色 —— 只有宿主。让 `Device` 去 collect 别的插件等于让
`Device` 依赖 15 个 crate（其中一个 `Sessions` 还反过来依赖 `Device`，直接成环）。

**为什么不是「视图也留在宿主」**：那样 `Device` 只是转发宿主算好的 JSON，
「端点的家」还是空的 —— 判据 12 会过，但搬家等于没搬。分界线画在**数据 vs 形状**：
宿主给「有哪些模块、每个模块有哪些端点」，插件决定「这些怎么变成一个响应体」。
依据是这三条端点的**形状是它们的语义**：`enabled` 的判定规则、`reason.code` 的四个值、
OpenAPI 的 `operationId` 拼法，任何一条改口径都是这三条端点的契约变更，不是宿主的事。

**副产品**：`meta::route_enabled` / `meta::endpoints` 变成两行转发（`line_enabled` /
`enabled_endpoints` 由插件导出），宿主侧判据（`capabilities_shapes.rs` 的负向哨兵）
调的还是同一份实现，不会漂。

**反悔点**：如果第三个插件也要读注册表（目前只有 `Device`），说明「注册表」该提成
`nous-routes` 里的一等公民（一个 `Registry` 类型，宿主构造一次、各插件只读）。
判据：`grep -rn 'fn registered(&self)' crates/*/*/src/` 数到 2 就该翻。

## 2026-11-26 · `messages` 留在宿主侧，不当「一票全搬」的牺牲品

**决定**：`/v1/sessions*` 十七条里十六条搬进 `nous-sessions`，`POST /v1/sessions/{id}/messages`
留在 `server/src/routes.rs`，归属仍报 `MODULE = "sessions"`（所以 capabilities 上还是 17 条）。

**理由**：这条 handler 要的不是会话语义，是**组装根**——`AssembledRuntime` 的 `driver` /
`completions`（起一轮、等一轮）与提示词注入（`nous-prompts` / skills 库 / `nous-env` 的两个根）。
把 `AssembledRuntime` 搬进插件等于让插件认识 `nous-runtime` + `nous-tools` + provider 装配，
那是「进程怎么组装」而不是「会话是什么」。**搬端点是为了让归属诚实，不是为了把目录搬空** ——
把组装根硬塞进会话语义里，换来的是 17/17 的整数，代价是一条永远解释不清的依赖边。

**反悔点**：如果 `AssembledRuntime` 被第三个插件需要（目前 `Nous_Extend` 也摸运行时），
那说明「跑一轮」应该是一层**独立的宿主能力**（`Host::run_turn(...)` 返回一个 future），
而不是每个插件各自摸 driver。判据：`grep -rn 'AssembledRuntime' crates/Nous/*/src/` 数到 2 就该翻。

## 2026-11-26 · 插件的错误类型就地定义，不做「公共 `AppError`」

**决定**：`nous-sessions` 自带 `SessionError`（十个变体照抄 `ServerError`），`server` 侧写一层
`From<SessionError> for ServerError` 接住过渡期的老调用点。

**理由**：`SessionSource` / `CommandSource` / handler 三层必须共用同一种错误，而这三层都要搬进插件。
「共用」有两个方向：要么把 `ServerError` 公共化（`nous-routes` 或新 crate），要么让拥有三层的
那个 crate 自己定义。前者会让每个插件都有第二个「我该用哪个错误」的问题，而且 `ServerError`
带着 `Validation{field, problem}` / `PreconditionFailed` / `Locked` 这些**私库与元素库**才有的语义 ——
会话用不上，却要被迫认识。后者多一层 `From`，但每个插件只认识自己的那十个变体。

**反悔点**：如果第三个插件的错误类型与 `ServerError` 逐字一致（十个变体一个不多一个不少），
那就是三份重复，该提成 `nous_routes` 里的共用枚举（`HttpRefusal` 加上 `Conversion` 就行）。
判据：`grep -c 'pub enum .*Error' crates/Nous/*/src/error.rs` 数到 3 就该翻。

## 2026-11-26 · `Refusal` 的富化格跟语义走，不跟 HTTP 壳走

**决定**：`RepoError::Conflict` 的 `{content, etag}` 两格补进 `nous_repo::Refusal::body()`，
**不放**在 `nous_routes::HttpRefusal` 里自动推导。

**理由**：`HttpRefusal` 只认识 `(status: u16, body: Value)` 两格，它的契约就是「传输壳，
不认领域错误类型」—— 让它去推 `content` 就得认出 `RepoError`，那一层立刻多一条 `nous-repo`
依赖，而成环判定（`env → routes → repo → env`）当初正是为了躲这个才把语义层放进 `nous-repo`。
富化格是**语义**（「409 时该给客户端看什么」），归拥有 `RepoError` 的那一层。

**反悔点**：如果第二个「非 `RepoError` 来源」的拒绝也需要带富化格（比如 `triggers` 的
`If-Match` 冲突也要回盘上当前正文），那就说明富化格的形状是跨领域的，该把它提成
`HttpRefusal::with_field(key, value)` 这种通用构造器 —— 但**仍然不推**，只让插件显式给。

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
`crates/Nous/Store`（`nous-store`，layer = `platform`）独占所有权；`durable.db` 一个字节都不碰。
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
`crates/Nous/Store`）。名单从 `cargo metadata` 读，脚本里不点名目录；每一条入口都必须出现在
「排掉了 N 行」的清单里（豁免规则活着才绿）。

**留痕（老规矩三行）**：

- **谁刀的**：用户，2026-11-18（#56 票面「依赖分离 = 两个库」），助理起草改动。
- **旧判据**：`$K` 之外不许出现 `rusqlite|Connection::open|.sqlite|PRAGMA|BEGIN IMMEDIATE`。
  单库时期这是对的，也因为只有 `durable` 一个库。
- **为什么改**：第二个库（`nous.db`）一落地，旧判据必然红 —— 而它红的原因**不是**代码错了，
  是判据把「只有一个数据面」写死成了「只能有一个入口」。要么这条判据挡住第二张票，要么
  有人图省事把触发器塞进 durable（正是本票要防的）。所以判据跟着改口径，而不是代码跟着判据走。

同时收窄了 `scripts/check-separation.sh` 判据 1 的 grep：票面写的是
`grep -rn "trigger|fires|plugin_snapshot" == 0`，但 `crates/Nous/Durable/src/harness/live.rs`
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
判据 `crates/Nous/Server/tests/token_path.rs`：`NOUS_TOKEN_FILE` 那把能用、`$HOME` 那把**不认**、
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

**事实**：`crates/Nous/Sessions` / `server` 在两条线上各被写了一遍 —— `main` 上的 `63f476a`
与 `integration/desk-connect` 上的 `f7e1ed5`。同一个目录、同一个目的、不同实现（`turn.rs` 各自
300+ 行），所以两边合并时每一处都是「添加/添加」冲突，`git` 没法替我们选。

**决定：整个 `crates/` 取 integration 那份**。理由不是谁写得好，而是 integration 是**十张票
一起跑绿**的那条线（`verify.sh` 55 条全绿、`cargo test --workspace` 1134 条全绿）；
main 那份只被 #44 一张票验过，合并进十票的实现里没人验过。

**代价（写清，别装作没有）**：main 那份的**时钟注入**（`sessions/src/clock.rs` + `with_clock`）没保留，
integration 用 `chrono_millis` 直接取时间。也就意味着 `nous-sessions` 那一层暂时无法用假时钟穷举测。
**这条列成待办**：把时钟注入补回 sessions（一个 `Clock` trait + 一处注入 + 一条判据），
不做就会在某次"时间相关 bug"上付出比现在更贵的代价。

**同时删掉的两个文件**：`crates/Nous/Server/tests/turn_end_to_end.rs`、`crates/Nous/Sessions/src/clock.rs`
—— 它们测/提供的是**已经不存在的 API**（`TurnHandle` / `TurnFactory`），留着会编译不过。
端到端覆盖没丢：integration 自己那份 `sessions/tests/turn_end_to_end.rs` 还在。

**反悔点**：如果哪天发现 integration 那份 #44 缺了 main 那份的某个能力（例如历史重放 `load_history`），
用 `git show 63f476a:crates/Nous/Sessions/src/turn.rs` 取回来当参照 —— 两份都还在 git 里，没有真丢。

---

## 2026-11-18 · 两个库的文件名定稿：`nous.db`（durable）+ `nous-plugins.db`（store）

`#56` 那条「存储分离」留了一句**文件名还没定死**，今天定掉，理由如下：

- **durable 的库继续叫 `nous.db`**：真机上已经在跑，而且 `deploy.sh` / 备份 manifest / `--check` /
  `--active-tasks` / 升级闸门全都引用这个名字。改名 = 数据迁移 + 备份链断一次，换不到任何好处。
- **store 的库改叫 `nous-plugins.db`**（原来是 `trigger-core.db`）：这张库的所有者是
  `nous-store`（插件侧），触发只是它**第一个住户**；往后审计、别的插件数据还进这张库。
  **名字跟所有者走，不跟第一个用户走** —— 叫 `triggers` 的话第二个住户落地就得再改一次名，
  而改数据文件名是迁移，不是重命名。
- **环境变量与文件名同字**：`NOUS_PLUGINS_DB` / `nous-plugins.db`（原来 `NOUS_TRIGGER_DB`）。
  一条规则，少一次「变量叫 A、文件叫 B」的对照。

**落地面**：`crates/Nous/Server/src/main.rs` 的默认路径与变量名、`scripts/deploy-check.sh`
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

---

## 2026-11-18 · 绑非回环（0.0.0.0:4789）：Desk 永远不和 Nous 同机

**决定**：`deploy/nous-app.service` 的 `NOUS_LISTEN` 从 `127.0.0.1:4789` 改成 **`0.0.0.0:4789`**。

**理由**：这条从一开始就是计划里写死的先后顺序 ——「改 0.0.0.0 是 `#50` 落地 + Desk `#8` 注入
header 之后的**显式动作**」。今天两个前置都到位了：

- `#50`：Bearer token + 非回环必须有 token（`refuse: no-token-on-non-loopback`，退出码 78）；
- Desk `#8`：token 一处注入、capabilities gate 已合入。

**判据 9b 的口径跟着换**（旧口径是「#50 前不许 0.0.0.0」，今天变成「绑非回环就必须有钥匙」）：

- 静态（`verify.sh`）：unit 里 `NOUS_LISTEN=0.0.0.0:4789` 与 `NOUS_TOKEN_FILE=/etc/nous/token`
  **必须成对出现**，缺一个就红。
- 部署前（`deploy.sh`）：同样成对检查，缺了直接拒绝部署（不等进程起来才 refuse）。
- 运行时（`deploy-check.sh`）：用**局域网地址**打一次，未认证必须 **401**，token 文件必须在。
  只听回环时跳过裸奔检查（回环本来就不需要钥匙）。

**实测**（从另一台机器打 `192.168.1.7:4789`）：不带 token → 401；错 token → 401；正确 token → 200；
`/health` → 404；`capabilities` 正常返回。

**另外修掉一个部署链的坑**：`deploy.sh` 的幂等判据原来只看**二进制 sha**，于是「只改 unit
（比如这次改监听地址）」会被判成"同 sha 已部署"直接退出，配置改动静默不生效。现在幂等 =
产物 + unit 两样都一样。

**反悔点**：回到只绑回环，就是改这一行 + 把判据 9b 的口径改回来；如果要做「只允许指定网段」，
别在应用层加白名单（那会变成第二套认证），交给 nftables/防火墙。

---

## 2026-11-24 · 客户端不留「模块名 → 中文名」对照表（模块名只有一处真相）

**现象**：NousDesk 的 `capability.rs` 里有一张 `模块名 → 中文` 的表（`stats` → 「数据监控」）。
引擎的 `/v1/capabilities` 只给 `{name, enabled}` —— 中文名是客户端自己存的第二份名字。
加一个新插件，客户端不改就显示半中半英；改了就是两处漂。

**决定**：客户端只显示**引擎报的英文模块名**，不存任何模块名常量、不做任何名字映射：
引擎报什么就显示什么，新插件自动出现在「关于 → 插件」列表里。中文只准活在模块自己的
`README.md`（规则 7）与 `docs/plugins.md` 的描述列里 —— 那是散文，不是名字的第二份拷贝。
Magento 2 同理：模块名永远是 `Magento_Catalog`，中文在 `i18n/zh_Hans.csv`。

> 注意区分两类中文：**页面名**（Desk 左列的「数据监控」「会话」）是客户端自己的导航，留着；
> **模块名**才是本决定要收掉的东西。

**理由**：这条用户只有一个（本人），看得懂 `stats`；两套名字的唯一收益（"给非开发者解释"）
不存在，成本（每加一个插件改两处、必然漂）一直在。

**反悔点**：哪天真的面向非开发者发行，**不许**把常量表加回客户端，而是走 i18n
（`i18n/zh_Hans.<csv|json>`，缺失回退英文），并且翻译文件的 key 就是模块名；
那时客户端仍是"英文名 + 可缺的翻译"，不是第二套名字。

---

## 2026-11-24 · `plugin.toml` 的 `version` 与发行版号**解绑**（推翻 2026-11-18 第 2 点）

**现象**：14 个插件的版本号永远是同一个数 —— 因为 `check-plugins.sh` 要求
`plugin.toml.version == Cargo.toml.version`，而 `Cargo.toml` 用的是 `version.workspace = true`。
一个仓一个号，机器上就再也不会出现「这个模块上次动是哪一版」。

**决定**：插件版本是**这个插件自己的** semver，独立演进。判据只剩两条机械规则
（合法 `X.Y.Z`、不比 `git show HEAD:` 里的低 = 只增不减）；跨度要求写进
`docs/plugins.md` 规则 8（端点契约 → minor，实现/文档 → patch）。
Magento 2 的模块也是各走各的版本线，根仓不压一个数 —— 这条是往 Magento 2 靠，不是背离。

**反悔点**：若某个插件真要独立发包（crates.io / 单独 tag / 客户端按版本协商能力），
现在的「不倒退」就不够了，要换成「发布记录逐条对得上」；那时判据改成读
`plugin.toml` 的 changelog 段，而不是比 HEAD。

---

## 2026-11-18 · tag 版本步长：默认 **+0.0.1**（patch 加一），除非用户另行说明

**规则**：每次打 tag，版本号默认 **patch +1**（`0.1.0` → `0.1.1` → `0.1.2` …），
不自动升 minor/major —— **升 minor/major 必须用户明确说**。

**一处真相，三处同步**（缺一处就红，`check-plugins.sh` 会拦）：

1. 根 `Cargo.toml` 的 `[workspace.package] version`
2. 17 个 `crates/*/*/plugin.toml` 的 `version`（构建期校验：必须等于第 1 项）
3. `git tag v<版本>` —— tag 只打在这一个数上

**顺带修掉一处两处真相**：`crates/Nous/ExtSdk/Cargo.toml` 是唯一一个把版本写成字面量的 crate
（其他都是 `version.workspace = true`）。升版本时它不会跟着动，`check-plugins.sh` 立刻报
`ext-sdk: version='0.1.1'（Cargo.toml 是 '0.1.0'）`。改成 workspace 继承 —— 外部消费者要的
是**一个能验的版本号**，不是「每个 crate 自己记一份」。

**反悔点**：如果 ext-sdk 真要独立发布（crates.io），它就得有自己的版本线；那时候
`check-plugins.sh` 的这条校验要改成「允许 `[package] publish = true` 的 crate 自持版本」，
而不是现在的一律相等。
## 2026-11-19（JoeWest9966 定的）· 旧面板的壳子退场：删 `server/` / `templates/` / Node 构建配置 / 旧 unit

**决定**：从编码仓删掉旧面板剩下的全部壳子：`server/*.ts` + `server/blank.mjs`（10 个文件）、
`templates/{agent-block,conversations,models}/`（10 个文件）、`vite.config.ts`、`tsconfig.json`、
`package.json` / `package-lock.json`、`.env.example`、`deploy/nous-panel.service`。
`.gitignore` 里的 `content/**/*.sqlite-shm` / `-wal` 两行**保留**，理由改成实话：主线上
`content/` 已于 2026-11-18 移出仓库（见上一条），这两行只对**本机 / j1900 上还没删的旧工作树**有用
—— 那儿跑着真 sqlite，去掉只会让边车变成 `??` 噪声。`fixtures/*.sqlite-wal` / `-shm` 同理。

**为什么现在能删**：前提有两条，都已经是事实 —— ① 引擎换成 Rust（`crates/Nous/*`，
`cargo test --workspace` 是唯一的"做完了"定义）；② 面板下线，`vite preview` 不再被任何
systemd unit 拉起，`NOUS_HOST` 全仓 0 处读取。这些文件不参与 cargo 构建、不被任何 Rust
代码读，只占着名字让新人（和 agent）以为 `server/` 里还有 `/v1` 的实现、`templates/` 里还有
模板、`package.json` 里还有构建脚本。删掉它们是把"仓里有什么"和"系统是什么"对齐，
不是顺手清垃圾。

**怎么确认不是巧合**：正反两个方向都试过（issue #59 判据 5）。反向哨兵：临时留回
`tsconfig.json` 再删，CI 判据不变红 —— 说明没人读它；正向哨兵：把 `server/tree.ts` 挪进
`crates/Nous/Server/src/`，CI 也不变红 —— 说明它本来就不参与编译。两个方向都确认
"删了没影响"不是因为测试瞎了。

**判据**：`git ls-files` 里上述路径 0 条；`cargo metadata --no-deps` 的 workspace members
仍是 23；`./scripts/verify.sh` 56 ✓、`./scripts/check-plugins.sh` `checked=17 mismatched=0`；
`git grep -nE "vite|tsconfig|nous-panel\.service" -- ':!docs' ':!*.md'` 的旧壳引用零命中
（`crates/Nous/Tools/shell_output.rs` 里的 `vite v6.4.3` 是日志清洗的**测试样本**；
`vendor/pi-env-daemon` 的 `tsconfig.build.json` 是 vendor 自己的构建配置；
以及 `invite` 里的 `vite` 子串误命中 —— 三处都与旧壳无关）。
**判据 4 的口径据此收窄**：判的是「旧壳的**路径**还有没有人引用」，不是「文件里出现过 `vite` 这个词」，
所以按路径名匹配（`vite\.config\.ts` / `tsconfig\.json` / `nous-panel\.service`），
并排除 `vendor`（第三方树）、`:!docs`、`:!*.md`。

**反悔点**：要回旧面板 —— unit 在 j1900 的 `/etc/systemd/system/nous-panel.service` 原地留着
（本票只删编码仓那份副本），代码和配置在 git 历史里（`git show HEAD~1:server/api.ts` 之类），
`npm ci` 照 `package-lock.json` 的旧内容还原依赖（本票提交 `abf1c0a`，用
`git show abf1c0a^:server/api.ts` 之类内容寻址的写法，不要用会随提交漂移的 `HEAD~1`）。
真要恢复之前先回答一个问题：
恢复的是面板本身，还是只是想找某个旧实现做参照？后者直接从历史读，不用把壳子请回来。

## 2026-11-20（票 #65）· device 从 server 里长成插件：编目有来源，默认模型有写者

**决定**：把 `meta.rs` 的 device/machines/models 段和 `machines.rs` / `models.rs` 搬进新插件
`crates/Nous/Device`（app 层，`nous-device`）；`server → device` 单向依赖，device 不认识 server。
`/v1/models` 的来源优先级是「装配注入的编目（faux 验收 / 快照测试）> `NOUS_MODELS` 内联 >
`NOUS_MODELS_FILE` 文件 > 诚实空表 + `catalog.state="missing"`」。
`PUT /v1/default` 只写文件编目（临时文件 + rename，先校验模型在 items 里），内联编目没有可写路径 → 400。

**为什么**：旧 `/v1/models` 永远 `{default:null, items:[]}` 而 `models.rs` 注释说「宿主会填」——
本仓里没有宿主，空表是死的。给它文件与内联两个来源 + 可诊断的 `catalog` 块，才让「没编目」
从「忘了填」变成一条能贴进 issue 的事实。默认模型必须能写回，且不许写一个 items 之外的
模型（写假模型比不写更坏）。机器编目同样：`NOUS_MACHINES_FILE` 优先，内置两台只是退化兜底。

**反悔点**：真接上远端 provider 编目（Anthropic/OpenAI）时，把来源换成「远端拉取 + 本地缓存」，
`catalog.source` 加一个 `"remote"` 值、`PUT /v1/default` 改为写缓存那一份；`state/source/path`
三字段的形状和「missing 就是诚实空表」不改。faux 注入优先是验收模式的特例，真 provider
装配不该再注入 catalog —— 否则 `NOUS_MODELS_FILE` 会被静默吞掉。

## 2026-11-20 · 四条私库路径的默认值回到 TS 版，缺目录不等于不知道在哪（#63）

**症状**：Desk 的资源页三处全 0；`/v1/skills` / `/v1/prompts` 回 `root: null`，
j1900 上其实还留着 940K 的技能副本。插件机制在跑，内容进不来 —— null 把
"没装" 和 "不知道数据根在哪" 混成同一个形状。

**决定**：变量名和默认值照 `Nous-ts` `server.js:77-83` 净移植，四个入口收进
`crates/Nous/Env/src/content.rs` 一处解析；旧变量名删除、不做兼容（查过没有任何
部署设过）。`root` 从此恒为字符串，目录在不在由 `present` 说；`missing` 只在
`present=false` 给一句人话。服务只读：不建目录、不写、不 spawn —— 写回是
`skills-admin` / `presets-admin` 的事。模板数据根走 `NOUS_PRESETS`，与 #60 的
端点实现共用，不再写死别的目录。

**为什么不做双名兼容**：一个路径只能有一个来源。同时认新旧两个名字，迟早会出现
"unit 设了旧的、代码读新的"却都显示绿的假象；而迁移成本此刻为零。

**反悔点**：只有在确认有部署真的设了旧变量名时才加兼容层 —— 那时候必须让两个
变量同时可读、且有一处判据断言"两者同时给出时谁赢"，不许默默 fallback。

## 2026-11-20 · 远端编目刷新失败留旧表，密钥只在内存（#79）

**症状**：TS 版靠 `NW_BASE/NW_TOKEN` 自动拉编目，Rust 侧 `/v1/models` 靠手填文件；
模型表要么空、要么是人肉维护的过期副本。`POST /v1/catalog/refresh` 缺失意味着密钥轮换后
只能重启进程。

**决定**：`NOUS_CATALOG_BASE` + `NOUS_CATALOG_TOKEN` 拉 `{base}/models.json`，成功原子写
`NOUS_MODELS_FILE`（无该变量则默认路径），回 `{default, items, fetchedAt}`；失败回 200 +
`error`，items 保持上一份（内存优先、否则从文件读回），不清空。token 只以 Bearer 头在内存里
过一手，不进缓存文件、不进日志、不进 HTTP 响应。诊断块从 `catalog: null` 变成 device 自己
交代的 `{state, source, path, fetchedAt, error}`。

**为什么**：失败时清空表等于把「这次没拉到」误报成「上游一个模型都没有」——客户端会据此
把模型选择整个抹掉，比旧表过期更坏。token 落盘一次就再也不能靠「别看那个文件」收回。

**反悔点**：若远端返回的是 TS 版 `/api/model?models=full` 的 envelope（`{ok, data.models}`），
在 `refresh_with_client` 里加一层展开即可；当前票面写的是 `models.json` 一类结构，不在这里
猜方言。若将来要支持多远端源，`source` 要从 `Option<&'static str>` 换成枚举/字符串，
判据要跟着加一条并列来源优先级。
## 2026-11-20 · #86 盘面形态：文档真相留在 `documents`，树语义另立实现票（已被 2026-11-21 #90 取代）

**决定**：v1 的文档真相继续是数据库里的 `documents`（`kind` / `section` / `name` + base/delta
修订），**不复活** `content/<board>/tree.json` 与 `content/<board>/map/<node>/*.md` 这套文件形态，
回收站也进数据库而不是 `.trash/`。TS 时代的「一个节点 = 一个文件夹」在 v1 里没有对应物：
`GLOSSARY.md` 已写明「块」这个概念消失，一个块要么是扩展、要么是内容、要么被删；
`#59` 又把 `content/` 移出仓库并归档，旧面板已停用（unit 留在 j1900 可回）。

**为什么**：`content/` 是旧面板自己读的那一层，引擎早已换成 `nous-app` + `/var/lib/nous/nous.db`。
再建第二套文件形态会让同一条文档有两个真相：一边是数据库文档（`#48` 的 12 条泛型端点，
`Desk#16` 已接），一边是盘上目录；而 `docs/decisions.md` 的两库分离已经把「谁是写者」钉死，
绑定资料只许有一处。树形结构不是非得用文件表达 —— 用 `documents` 的 `kind` 表达即可，
这只是投影形状的变化，不需要第二写通道。

**反悔点**：若出现下面任一事实，重开文件形态并先改 `wire.md` 与 `docs/plugins.md`：
① 用户要求文档能脱离 `nous-app` 直接用编辑器/git 管理、且必须看到真实目录；
② 需要把旧 `content/<board>/map/` 的文件夹结构原样迁进来（从 2026-11-18 的归档 tar 取）；
③ 数据库文档承受不了某个具体需求（如单文档 >512KB、需要 git diff/逐文件提交）。
反悔时先回答「数据库文档那 12 条端点怎么办」——过渡期只能双读单写，不许两个写者。

**后续开发票**：树 / 文件夹语义的契约、端点与迁移由 GitHub issue **#87**
（`documents`：盘面树/文件夹语义）实现，正文写明 `blocked_by #86`、与 Desk#16 / #69 的关系；
本票不写实现。

> **2026-11-21 #90 复核**：本条的「文档真相留在 `documents`」与「#87 树语义」已被
> 下方「内容真相只有四个私库」那条取代（两条冲突时保留历史，以后者为当前决定）。

## 2026-11-20 · #86 节点锁：单用户场景不复活服务端锁

**决定**：v1 **不复活** `.nous/locks.json` + `audit.log` 的服务端节点锁，也不在 `/v1/*` 加
lock/unlock 端点。单用户桌面端的替代是**单用户 + 无并发写**：同一时刻只有一个客户端写
盘面，冲突交给文档修订（`base` / `delta` + 版本号）与 `409` 自然拒绝，不另起一套锁文件。

**为什么**：锁解决的是「多人同盘编辑防手滑」。v1 没有多人盘面：`nous-app` 是单用户进程，
`Desk#16` 接的是数据库文档，修订本身已有单写者边界；再加一套 `.nous/locks.json` 会多出
一个必须和数据库同事务的写者，违反「不为过判据开第二条写通道」。锁文件还不是租约 ——
进程崩了会留下陈旧锁，反而增加恢复路径。

**反悔点**：一旦出现**多人同盘编辑**（两个客户端同时改同一份盘面、或一台机器上多个用户
共享同一 `NOUS_DB`），或出现「合并冲突把用户内容弄丢」的具体事故，就必须重开：
届时加的是**带租约与心跳的服务端锁**（不是把 `locks.json` 搬回来），锁状态进 `decisions.md`
的新条目并配过期回收判据。

## 2026-11-20 · #86 反代：块进程的路由由 `ext` 承担，不另起 `/p/:board/:id/*`

**决定**：v1 **不复活** `/p/:board/:id/<任意>` 这条按 board/node 反代到块进程的 Node 宿主路由，
也不在 `nous-app` 里加第二套「按盘面节点路由到子进程」的实现。需要挂进程时走 `ext` 插件：
扩展由清单装载，按 `名牌`（`skill:<名>` / `prompt:<名>`）加入会话，通过 `接缝` 帧协议调用；
`#81` 负责把这条运行时解冻（真清单 / 装载 / 熔断 / 热重载）。

**为什么**：TS 的反代绑在「块 = 目录兼进程单位」上，而 `GLOSSARY.md` 已定 v1 没有块的概念：
一个块要么是扩展、要么是内容、要么被删。反代同时引入未经鉴权的任意路径转发面（`/p/*`
不在 `wire.md` 的 `/v1/*` 契约里），与「客户端只转发 `/v1/*`」和 token 中间件覆盖全部端点的
现状冲突。`ext` 已经提供进程监管、熔断、帧协议与可观测状态，再起一条按 node 的路由就是
第二个进程面。

**反悔点**：若某个扩展确实需要 HTTP 入口（而不是帧协议），或出现必须按盘面节点寻址的
运行时（例如远端块进程、Desk 直接渲染块页面），重开时只能是 **`ext` 内部的一种传输适配**：
在 `/v1/extensions/{name}/proxy/*` 下按扩展名转发，沿用 token 与熔断、带路径白名单，
并先改 `wire.md`；不许恢复 `/p/:board/:id/*` 的裸反代。

## 2026-11-20 · #86 总账：恢复的是语义，不是旧宿主的形态

**决定**：`#69` 的恢复总账以「语义等价」判完成：盘面树 / 文件夹语义（**#87**）走 documents
的 `kind`，锁与反代的反悔条件分别写在上面三条里，`#81` 管扩展运行时。
`#86` 自身只产出决策与后续票号，不改 `.rs`、不改 `wire.md` 的端点形状。

**为什么**：TS 宿主是 Node 时代的形态（文件树 + 锁文件 + 反代），Rust 侧已经有数据库文档、
两库分离、`ext` 帧协议三个更窄的对应物。照搬形态会同时引入两套真相与第二个进程面，
照搬「不用」又会把能力从总账里抹掉。写成「语义在 #87/#81，形态不复活」两边都不丢。

**反悔点**：若 `#87` 的实现票证明 `documents` 的 `kind` 无法表达树（引用完整性、循环、
移动原子性三者有一样落不下去），就按上面第一条的反悔点重开文件形态，并同步改 `wire.md`、
`docs/plugins.md` 与 `GLOSSARY.md` 的「块」词条。

## 2026-11-20 · #64 purge 不删 record_ids（id 永不复用）

**决定**：`POST /v1/sessions/:id/purge` 的一次事务里删 conversations / entries / tasks /
submissions / documents / document_revisions 与该会话的 pins / unread 条目；**不删
`record_ids` 里这些 id 的行**，只写墓碑到 `qrd3wh.sessions`。

**为什么**：`record_ids` 是全库单调 id 的注册表（`storage/commit.rs` 用它拒绝重复铸造），
删掉这些行会让 `next_id` 下一次铸出同一个 id，撞上其它会话/文档里还活着的旧引用。票面列了
`record_ids`，但删它会破坏「id 一旦铸出永不复用」的硬规矩；墓碑让派生视图认得这条会话已死。

**反悔点**：若另票引入 id 命名空间（每段 id 带会话前缀，重铸不撞旧引用），可改成连
`record_ids` 一起删；那时必须同时改 `commit.rs` 的重复检查与本票的 purge 事务。

## 2026-11-21 · #73 的 PATCH 用 version 乐观锁，dispatcher 不自己起异步循环

**决定**：`PATCH /v1/triggers/{code}` 是稀疏覆盖（`schedule`/`params`/`enabled`），
请求体带当前 `version`，每次成功改动原子 `version+1`；可选 `requestId` 只回第一次回执。
调度侧只提供 `arm_next_tick(session, clock, …)`：把下一拍写成 durable 的 `sleeping` 任务行，
等待交给注入的 `Clock`。**现状（review #12 校正）**：`main.rs` 的 app 装配处已
`tokio::spawn(nous_dispatcher::run_dispatcher(...))` 常驻这条节拍（详见 2026-11-23 条）；
「生产进程何时拉起这条循环不在本票常驻产物里」是 #73 当时的范围说明，不是今天的进程形态。

**为什么**：`code` 是触发器身份，改它相当于把历史账本改挂到另一个商品上；库里的物理
触发器已经挡住，HTTP 不能留第二个入口。`version` 让 PATCH 和既有 `If-Match` PATCH 端点
同一口径；`requestId` 让网络重放不产生第二次定义变更。durable 的 `tasks` 表只存不跑，
把待办写成任务行而不是内存计时器，重启后才读得回「下一拍」；不自己写循环是 AGENTS.md
的铁律，也避免 app 层出现第二个时间源。

**反悔点**：若将来要在进程里常驻节拍，必须由新的装配票显式调用 `arm_next_tick` 并注入
生产 `Clock`，不得在 dispatcher 里藏 `tokio::time::interval`；若发现客户端需要整份替换
触发器定义，另开带 `If-Match` 的 PUT，不把 PATCH 悄悄改成覆盖语义。
## 2026-11-20 · #67 世代归档：独立 SQLite 文件（选项甲），不进 active 库

**决定**：`archive` 把会话闭包复印到活库同目录下的
`archives/gen-YYYYMMDD.db`（可配置目录由装配注入；测试走临时目录），源文件用 durable
自己的迁移链初始化并以**只读**方式取行；活库只写一个墓碑 `archived[id] =
{at,gen,purged:false}`。`restore{gen}` 从那份文件读回并写活库，抹掉墓碑；`purge`
必须先有墓碑。归档库**不进** durable 的迁移链、备份/`--check` 只认 `NOUS_DB` 那一份。
同日重复归档按会话 id 先清后写，不产生重复行。

**为什么**：归档的价值是「原件被撕了也能回来」；选项乙把复印件放在活库里，会跟活库
一起滚进备份、一起丢，正好丢掉这份价值。甲把 durable 的建表语句与读法复用为生成物
（`migrations()` 是唯一真相），代价只是一次按会话的闭包复制；不新增第二写者。

**反悔点**：若出现「归档库与活库 schema 分叉导致恢复失败」的实证（具体：同一份 generation
在新版 durable 的迁移链下打不开或丢行），或备份策略改成整目录快照且要求单文件可恢复，
就翻到选项乙（活库 `archived` 表 + generation 列，一次 v3 迁移）；翻的时候改本段、
`docs/storage.md`、`wire.md` 的 generation 语义与三条端点的存储实现，保留墓碑对外形状不变。
## 2026-11-20 · 变体绑定表落在私库根的 `data/` 下，不另设全局目录

**决定**：`skills` 用 `<NOUS_SKILLS>/data/skills.local.json`，`extend` 用
`<NOUS_EXTEND>/data/extensions.local.json`；表里只有 `{v:1, variants:{family:{id:{root,branch}}}}`。
**为什么**：TS 时代就是这个形状与位置；绑定是本地状态，不应进私库发布，也不该散在 `~` 里。
**反悔点**：若要把本地状态集中进 `/var/lib/nous/`，改 `repos::variant_tables_from_env` 一处即可；
现有表的 `root` 是绝对路径，搬家后要么迁移路径、要么重绑。

## 2026-11-21 · ext 运行时解冻：观测走运行时自己，不另造注册中心（#81）

**决定**：`GET /v1/extensions` 的清单直接投影 `nous_ext::Runtime` 持有的事实（清单、
进程 pid、状态机、错误），失败项也留一行；`POST /v1/extensions/reload` 打同一个实例的
`reload_all()`（扩展级 `reload` 的唯一入口）。server 只装配一次并读观测，不维护第二张表。
装配入口在 `main.rs`：源码根走 `nous_env::EXTEND`（#63 的四根之一），产物固定在根下
`artifacts/`，句柄只给 skills / prompts 的值（扩展拿到的是代读后的内容，不是路径）。

**为什么**：清单里的 `ext` 曾经 `frozen` 而代码完整在 —— 能力被藏起来不是没做。
再起一张 "extensions registry" 会跟运行时的状态机分叉（谁先看到崩溃、谁记 pid），
而且客户端会拿两张真相吵起来。`ext-probe` 的沙盒已经是同一形状（exts/ + artifacts/ +
句柄目录），照它装配比新发明一个数据目录安全。父进程退出兜底用 `PR_SET_PDEATHSIG`
（pre_exec 里装，另看一眼 ppid 关掉 spawn 竞态），与 kill/Drop 构成三级。

**反悔点**：若 #84 的管理面需要「装了但没启用 / 被禁用」这种配置态，不要往这个清单里塞
第二份状态：给 `Runtime` 加配置态并让 `state` 成为它的投影，先改 wire.md 再改代码。
若产物不放在源码根下（例如统一发行到 `/opt/nous-ext/`），只改 `assemble_extensions`
的 `artifacts_dir` 推导，不动清单形状。

## 2026-11-22 · 卸载是移动到垃圾站 + 删除名单，不是 git rm

**决定**：`POST /v1/extensions/uninstall` 把源码 `rename` 到 `data/extend-trash/<name>-<ms>/`，并在私库根写 `deleted-extensions.json`；scan 认名单跳过，install 从垃圾站最新一份装回。核心 `nous`/`skills`/`prompts` 一律 400。
**理由**：私库是 git pull 的；只删本地目录，下一次 pull 会把扩展复活。名单随私库发布，重启后仍生效；垃圾站在 data 下不随发布走，是 install 的回退来源。核心扩展是引擎自己装配的，删掉等于自残。
**反悔点**：若将来引入真正的包管理器（带 lockfile 与远端 registry），删除名单应当退役，改由 registry 声明已删除；那时 `deleted-extensions.json` 与 scan 的跳过分支一起删。
## 2026-11-22 · 触发器真删：账随定义一起销毁，行留 purged 墓碑占 code（#80）

**决定**：`POST /v1/triggers/{code}/purge` 先要求归档态，然后在一个事务里删光该 code 的
全部 `fires` 行，把 `triggers` 行标 `purged_at`（`deleted_at` 也填上），`code` 永不复用。
归档 / 捞回只翻墓碑、`version` 不动；整份改定义（PUT）才 +1。
**为什么**：触发器的定义只有 `triggers` 一行，不像会话另有 `gen-*.db` 复印件；留行只为
占住 code（requestId / actor.id / 审计都挂着它），账留成孤儿表没有任何读口。
**反悔点**：若将来要给已删触发器出审计报告，不能靠留 `fires`（它已删），得另开不可变
事件表并经签字 —— 那会是第二条写通道，先改 wire.md 再动库。

## 2026-11-23 · #73 常驻节拍在 app 装配处 spawn，零时钟判据限定调度逻辑

**决定**：`main.rs` 的 app 装配处 `tokio::spawn(nous_dispatcher::run_dispatcher(...))`，
注入 dispatcher 的 `RealClock` 与 `NOUS_DISPATCHER_INTERVAL_MS`（默认 60000）；令牌和
`Session::closed()` 都能让它收敛。dispatcher 暴露的 `RealClock` 是唯一的真实时钟实现，
`run_dispatcher` / `evaluate` / `arm_next_tick` 一律只认注入的 `Clock`，判据用 `ManualClock`。

**为什么**：#69 要「真实恢复」，只提供函数没人调用等于能力被藏起来。生产时钟总要有
一个地方实现；放在 dispatcher 的具名 `RealClock` 而不是让 server 自己读 `SystemTime`，
保持「调度逻辑零直接时钟」的判据能精确打在注入路径上，也让进程级判据真起 nous-app
就能验到 60s 节拍的可配置短间隔。

**反悔点**：若将来把节拍挪进专门的 daemon，不能复制一份时钟实现或再起第二条任务循环；
只把这一处 spawn 移到新入口，仍用同一个 `run_dispatcher`，并保留进程级判据。
## 2026-11-21 · #89 边界判据归位：搬 SQL，补基线，不放松判据

**决定**：presets 的回执/修订 SQL 与 stats 的插件库体检收进 `nous-store`
（`preset_receipt` / `append_preset_revision` / `quick_check`），app 层只调 API；
`rusqlite` 只留在 server / stats 的 dev-dependencies（判据自己开独立连接做正向哨兵）。
S8 词表 15 行逐行判定为 #72 template 与 #83 skills 的正当写面，按行附票号补基线；
spawn 集合 2→5，补 #78 的 env-ssh mux/remote 与 #75 的 repo git（都是 platform 层）。
§4e 图表改为脚本读 `docs/dependency-graph.toml`；阈值和断言未动。
**理由**：判据是唯一真相；红要么来自越界实现（搬走），要么来自基线漏收（补基线并写明票号），
不能靠放宽断言消红。`use rusqlite` 本身不改变 Cargo 依赖图——§1.1 看 normal 依赖、
§1.5 看源码，两条从不同源取数，哨兵要分别注入。
**反悔点**：若 #73 dispatcher / #84 ext-admin 之后又引入新的 `nous-*` 直接依赖，
只在 `docs/dependency-graph.toml` 与 `docs/harness.md` §7 两处同步补边并重跑全量判据；
不许把新边塞进「排除项」或关掉 §4e。

## 2026-11-24 · review #10：幂等回执与变更同提交，失败整体回滚

**决定**：`PATCH /v1/triggers/{code}` 的定义变更与 `trigger_patch_receipts` 行放进
`Store::patch_with_receipt` 的同一 `BEGIN IMMEDIATE`；`PATCH /v1/combos` 先把回执原子写就位、
再替换 `preset.json`，任一步失败就删回执，盘面回到变更前的字节。**重放回第一次回执；
第一次整体失败时盘/库未变，重放按全新请求执行**（已写进 `wire.md`）。

**为什么**：旧顺序先改盘/库再记回执，回执写失败会让重放拿到 412（改已发生却说没发生），
或反过来留下假回执。把两件事绑成一个提交边界，不需要第二写者，也不改两库分离的顺序。

**反悔点**：若将来某个端点的变更跨库（durable + store）无法同提交，必须先在 `wire.md`
定义「变更与回执分离」的补偿协议（重放语义、412/200 的判定），再动代码；
不许用「先写回执」的盘上技巧掩盖跨库不一致。

## 2026-11-24 · review #4：归档的 generation 与墓碑不是同一次 durable 提交，用补偿代替

**决定**：generation 是独立 SQLite 文件、墓碑是活库里的文档写，两者**不在同一个 durable 事务**；
用严格的顺序 + 补偿达到"失败不留半截状态"：先写 `gen-*.db.tmp` → rename 就位 → 写墓碑提交；
generation 写失败就删临时文件（库未动），墓碑提交失败就删已就位的 generation。
restore 方向相反：闭包先写回活库，抹墓碑失败时**保留 generation**（那是恢复的唯一来源）。

**为什么**：durable 的提交框架（`StorageWrite` / seq / 文档 revision / 观察者广播）只认活库事务，
把跨文件的 generation 字节搬进同一事务要给 `Storage::commit` 开专门的 hook，等于为归档造第二套提交边界。
判据按"外部可见状态"判定：注入失败后库里 generations / archived 标记都不变、盘上无孤儿文件。

**反悔点**：若将来 generation 改成活库里的表（不再是独立文件），必须把复制闭包与墓碑并进
`Storage::commit` 的同一批写，并删掉这里的补偿分支；那时 `docs/storage.md` 的表数与迁移链一起改。

---

## 2026-11-21（JoeWest9966 拍板）· 取代 2026-11-20「#86 documents 为真相」：内容真相只有四个私库（#90）

**决定**：内容（模板 / 提示词 / 技能 / 扩展）的真相只有四个私库的文件（`#63` 的四个根、`#75` 的 repo 层）。
引擎里的 `documents`（数据库文档 + 泛型端点）在 v1 没有产品面，从发行面下线：`MANIFEST` / `ROUTES` /
`capabilities` 不再声明它，`wire.md` 只留「v1 不提供」，crate 保留在 workspace 但 `nous-server` 不再依赖。
桌面端同步删协议层与 12 条登记（Desk#16 关闭时写明）。

**为什么**：`#60` 的实现写明「盘是唯一真相」，`#72` 的 prompts 同样读私库文件。保留 `documents` 会让
同一批模板有两条修订链（`document_revisions` vs `presets_revisions`）和两个写通道；用户在私库里改内容、
能 git 回滚，引擎不持有第二份。

**取代范围**：2026-11-20 的 `#86 盘面形态` 中「文档真相留在 documents」与「#87 树语义」作废；
「锁不复活」「反代不复活」仍有效。

**反悔点**：出现下列事实时重开 documents 并先改 `wire.md`：① 没有 git 私库却要编辑内容
（便携包 / 只读介质）；② 引擎必须持有半成品草稿而私库里不能出现；③ 私库文件被外部改坏，
需要引擎侧第二副本恢复。反悔时先回答「两个写者谁签字」——只能双读单写，不许两条链同时写。

## 2026-11-22 · combos 只认 `Ultimate-Template` 一段

**选了**：`combos::SECTIONS` 从 `["Session-Template", "Ultimate-Template"]` 收窄成只剩
`Ultimate-Template`；传别的段一律 `400`（不静默返回空清单）。`session_names()` 不再走
`section_path`，改成直接扫 `<root>/Session-Template`（它是组合的**引用目标**，不是组合段）。

**为什么**：`Session-Template` 的条目是「开一条会话用什么」，没有触发器可言。以前两段都扫，
结果 5 条会话模板全带「这条没钉触发器；这条没钉会话模板」的假 `problem`、`enabled` 恒 false ——
一个从定义上就不可能是组合的东西被报成「组合坏了」。Desk 只在终极模板段画开关，所以界面上
看不到这堆噪音，但它会污染 API 消费者与将来的任何前端。

**反悔点**：如果会话模板真的要带「默认触发器」（比如新建会话时自动跑一次），先改 `wire.md`
把那段语义写清，再放开白名单 —— 放开的同时必须让 `problem` 能区分「不是组合」与「组合坏了」。

## 2026-11-22 · 资源仓管理面（三个编辑页）落在 server 翻译层，文件与 git 全部复用 `nous-repo`

**决定**：`/v1/{skills,prompts,extensions}/{files,file,create,publish}` 由
`crates/Nous/Server/src/resource_admin.rs` 做 HTTP ↔ 盘面翻译（三仓共用一副骨架），
不新建 `library-skill-admin` crate；文件读写 / etag 撞闸 / 提交 / push
一律走 `nous_repo::Repo`（`/v1/repos/skills/*` 的同一套）。publish 先问 `pending()` 拿
dirty 路径再 `commit()`，不自己拼 `git add`。

**为什么**：`skills-admin` 那张表行剩的是「装/卸/variant」，编辑页要的四件事只是把既有
`repo` 能力按条目目录切片；另建 crate 会造第二套路径校验与第二个 git 调用面
（`AGENTS.md` 红线：`nous-repo` 是唯一的 git 进程调用面）。名字 / 路径规则沿用旧面板
`skills-api.js` 的 `checkName` / `resolveInside`，不发明第二套。

**反悔点**：出现「技能管理要走变体的独立 worktree / 独立分支」或「编辑页要的东西 repo 层
表达不了（例如目录级原子改名）」时，把写面提成 `library-skill-admin` crate，并在
`docs/plugins.md` 把该行从 `(待建)` 改成真 crate；那时 `skills` 模块只留只读投影。

## 2026-11-22 · 模板面回到 TS 内核同形，判定归 `nous-presets-admin`

**决定**：`/v1/presets` 的契约以 TS 内核（`presets.js` 750 + `presets-api.js` 947）为准：
`preset.json` 是**扁平**配置（`model` / `thinkingLevel` / `skills` / … 在顶层，不再包一层
`spec`），roster 回 `{ section, title, root, repo, presets, archived, categories }`，每行带
`spec` / `specError` / `tracked` / `dir` / `resolved`；段清单读 `<root>/sections.json`
（`manifest` / `checker` 决定 `preset.json` 还是 `ultimate.json`）。形状闸、引用解析、
名册投影落进新 crate `nous-presets-admin`（`docs/plugins.md` 里登记了很久的待建行），
server 只做 HTTP ↔ 盘面翻译。归档改成 TS 的**搬目录**（`<段>/<名>` ↔ `<段>/.archive/<名>`），
不再用 `archived.json`。

**为什么**：`#60`/Desk#11 把契约收窄成 13 条端点时，顺手把 `resolved` 与扁平 `spec` 丢了 ——
桌面端模板页因此画不出关系图，「开一条新对话」对引用失效的模板也照亮（判据没了）。
判定必须由引擎给（要查 skills / extensions / prompts / 模型编目，客户端查不了），所以
不是「客户端补算」，而是把 TS 时代的解析层归位。

**反悔点**：如果 `resolved` 的逐条查盘在 j1900 上成为 roster 的性能瓶颈（每段 N 条 × 4 个根），
先加按段缓存 + 写盘失效，别退化成「客户端自己算」。`sections.json` 若被证明是过度设计
（永远只有两段），可以退回目录名硬编码，但 `manifest` / `checker` 两列要保留在代码里的同一处。

## 2026-11-23 · 产物 triple 按架构收、隔离在非 root 下降级到 fd 表

**决定**：
1. `RuntimeConfig.host_triples` 默认给**同架构**的 `-musl` 与 `-gnu` 两条，别人架构照拒。
2. `spawn_one` 只在 `euid == 0` 时要求专用 uid；不是 root 就退到 fd 表 +
   `environ` 白名单，并打一行「uid 隔离不可用（宿主不是 root）」。

**为什么**：部署的内核是 `x86_64-unknown-linux-musl` 编的、跑在 `nous` 用户下。
`host_triples` 写死只认 `-gnu` 时，现成的静态扩展一个都装不上（`triple=…-musl` 被拒）；
就算 triple 放行，`pre_exec` 里的 `setuid(60001)` 也必然 EPERM，报出来是
`spawn: Operation not permitted`，看着像文件权限问题。两条都是「在 j1900 上真撞到」的。

**代价 / 反悔点**：非 root 宿主上扩展与内核同 uid，隔离只剩 fd 表与 `environ` ——
够挡「顺着 `/proc/self/fd` 写库」，挡不住「扩展自己去读别的文件」。要真 hardening 的做法是
给内核 `CAP_SETUID` 或改用 user namespace；那时把这段降级删掉、恢复「必须降权」即可。

**另一条**：句柄代读现在跳过 `.git` / `node_modules` / `target` / `artifacts`
（`handle::skipped_dir`）。不跳的话技能仓库的 `.git` 一动就是几 MB，
64 KiB 的句柄预算被它吃光，扩展拿到的 `files` 里一条 `SKILL.md` 都没有。
## 2026-11-23 · 元素库 = 第六套私库（不是 ext 插件），写一次提交一次

**决定**：旧面板 `k3skws` 的元素库（组 / 包）在 Rust 侧落成 `crates/Nous/Elements`
（app 层）+ 第六套私库 `NOUS_ELEMENTS`（默认 `/opt/Nous-Elements`）。盘上真相仍是
`groups/<id>.json` / `packages/<id>.json`，一份 JSON 一个元素；`/v1/elements*` 由
`server::elements` 只做 HTTP 翻译，写事务在 `nous-repo`：新增 `write_and_commit`
（一次持锁完成原子写 → 提交 → 尽力推）与 `find_request_id`（幂等回执）。同一个
`requestId` 只提交一次，重放回 `replay:true`、`commit:null`，一个字节都不写。

**为什么**：元素是**内容**，不是进程 —— `ext` 是「一扩展一进程 + 帧协议」，把一份 JSON
做成 ext 等于让它变成要编译、要监管的二进制；而旧面板的「块」这一层在 #49 已经砍了，
`content/` 块目录不是 Rust 侧的落点。做成第六套私库后，`/v1/repos/elements/*` 的文件面、
`pending` / `history` / 变体都不必重写，`capabilities` 由注册表自动带出。`write_and_commit`
放在 repo 层，是因为「唯一 git 进程调用面」必须在那一处；幂等靠提交信息里的
`Request-Id:` trailer，不引入第二个回执库（`nous.db` 是会话与触发器的事，元素库的历史
在 git 里，客户端重拉一次 `GET /v1/elements` 就是真相）。

**反悔点**：如果元素库真的需要「运行」（包里的 body 被注入会话或被执行），先看看是不是
该做成技能 / 提示词的内容（现有私库已覆盖那种用法），再考虑 ext；无论如何不要在
elements 里 spawn。若 `find_request_id` 的 1000 条窗口被证明不够（提交量远超预期），
改成按 trailer 建索引的本地缓存，窗口常量留在 `nous-repo`。

## 扩展的段进系统提示词：段名 `extensions`、entry id 先于模板/技能（2026-11-23）

**背景**：`runtime.render_turn` 之前只有 `ext-probe` 在调，轮次驱动器没调 —— 扩展跑得稳，
模型一个字看不到。现在 `routes.rs::messages` 组装轮次时调它。

**形状决定**：

- 段名 `extensions`（不复用 `prompts`）：模板/技能已经住在 `sections.prompts` 里，
  混在一起就分不清「哪段是扩展渲染的」，回放时也没法单独换。
- entry id `entry_id + 4`（模板/技能顺延到 `+5`）：段按 entry 顺序合并，扩展那条先合并
  → `nous` 扩展的 preamble 排在技能正文前面。反过来会得到「技能正文、然后 You are an agent
  inside Nous」这种前言后置的读法。
- `spawn_blocking` 里调 `render_turn_with`：它起线程等子进程（段预算 1s），占着异步执行器
  会让整个服务在这一秒里对其他请求变慢。
- **不按「有没有选中项」跳过**：`nous` 那条扩展产出的就是基础 preamble，它跟选中无关；
  跳过判据是「没装运行时」或「渲染出来是空串」。

**代价**：每轮多一次 `spawn_blocking` + 最多 1s 的段预算等待（并发取段、合计预算，实测
三个内置扩展常在 10ms 内回来）。

**反悔点**：哪天轮次要跑几十个扩展，1s 预算会让首 token 变慢 —— 那时把段渲染挪到
「会话创建时渲染一次、选中的东西变了才重渲」，而不是砍掉这条线。

## 2026-11-24 · 插件 README = 300 字人话；正文**不进二进制**，随产物发布

**决定**：每个插件 crate 带一份 `README.md`，中文 ≤300 汉字，只回答三件事 —— 它是什么 /
什么时候用 / 不负责什么（`docs/plugins.md` 规则 7，判据是 `check-plugins.sh` 的
`README_MISSING` / `README_TOO_LONG`）。桌面端「关于 → 插件」读的就是这份正文。

**它取代 2026-11-18「插件 md = 8 章双语 + 生成 API 表」那一版**：那份模板要求 8 章、双语、
API 表，与「300 字说清是什么」互斥；API 面已经由 `capabilities` 的六列 + `/v1/_routes`
覆盖，不需要在 README 里再抄一遍。旧的 8 章正文进 git 历史，模板 `docs/plugin-md-template.md`
已按新形状重写。

**正文不进二进制**：源文件随产物发布到 `<产物目录>/docs/plugins/<module>.md`
（`deploy.sh` 收集，`--docs-only` 只同步文档、不重编译），引擎端点
`GET /v1/plugins/:name/readme` 运行时读盘（mtime + 长度缓存）。
**为什么不 `include_str!`**：j1900 只跑产物，为一句话改文档要重编译 + 重发版，成本远高于
同步一个目录；文档跟着版本目录走，回退旧产物时文档一起回退，不会出现「二进制旧版、文档新版」。

**归属 `device`，不是 `server`**：`/v1/_routes` 与 `capabilities` 只吐**清单里开着的模块**，
`server` 不在 `meta::MANIFEST` 的 14 条里 —— 归它等于这条端点永远不出现在路由表，客户端会
一直报「路由漂移」（真踩过：第一版写 `module: "server"`，`_routes` 162 条里找不到它，
Desk 的关于页挂着「漂移 1 条」）。`device` 是 0.1.25 里唯一的「宿主级只读」模块。

**代价**：多一条部署路径（`docs/` 目录）；文件漏同步靠 404 `readme_not_installed` 说话
（客户端按「暂无文档」展示，不是错误弹窗）。源码侧由 `check-plugins.sh` 判，线上有没有装
由 `deploy-check.sh` 的 readme 判据报。

**反悔点**：如果文档涨到几十份、`docs/` 同步成为部署痛点，或者要支持「用户自己往插件页
写笔记」，那说明它该进可写存储（`nous.db` / 私库），而不是继续加文件。

---

## 2026-11-25 · 插件版本线从 git 痕迹机械推（`#93`），capabilities 报插件自己的版本

**现象**：规则 8 把 `plugin.toml` 的 `version` 与发行版号解绑之后，28 份 `plugin.toml`
全都停在 `0.1.26` —— 那是旧规则的残留，不是任何插件的经历。`telemetry` 只动过三次，
`sessions` 动过十几次，它们不该是同一个数。而且 `meta.rs` 报的 `modules[].version` 仍是
`env!("CARGO_PKG_VERSION")`：即便把清单推开，桌面端「关于 → 插件」里照样 14 行同一个数，
**这次考古在界面上等于没发生**。

**决定**（三件事，一起落）：

1. **版本线机械推**，不人工判读：`scripts/plugin-versions.py` 从
   `git log --name-status -M40%` 现算，起点 `0.1.0`，minor 只认两种痕迹（清单身份删改、
   `ROUTES` 里本模块的端点行增删改），其余 patch。表与口径写进 `docs/plugin-versions.md`，
   28 份 `plugin.toml` 一次性落盘。为什么机械：可复核 —— 每个数都能指到一段 `git log`；
   人工判读更"诚实"但也更不可反驳，评审时无从下嘴。
2. **`server/` 里还没搬进插件的 handler 提交算这个插件的经历**：端点属于模块（`ROUTES` 就是
   归属的唯一真相），起点取「目录入库」与「`ROUTES` 首次登记本模块」中更早的那个。
   搬家票（`#14` / `#75` 一族）往后照这个口径记账，端点先于目录不重复计。
3. **`/v1/capabilities` 报插件自己的版本**：`crates/Nous/Server/build.rs` 扫
   `crates/*/*/plugin.toml` 生成 `模块名 → version` 静态表（`include!` 进
   `src/plugin_versions.rs`），`meta.rs` 从表里取；取不到才回退发行版号并加
   `versionSource: "release"` 如实说明（回退不静默）。`build.rs` 对每份 `plugin.toml` 与
   `crates/` 两层目录都挂了 `cargo:rerun-if-changed` —— 少了这条，改版本不重编 = 假绿。

**为什么不用 `include_str!` 解析 TOML**：`plugin.toml` 的行格式就那几种（`version = "X.Y.Z"`），
`build.rs` 里一个十行的扫描器足够，不值得为它引 `toml` 依赖进 `nous-server` 的构建链
（那一层是唯一允许依赖 axum 的 crate，构建依赖也该一样克制）。

**代价**：`build.rs` 多跑一遍目录扫描（毫秒级）；`plugin-versions.py` 依赖完整 git 历史
（浅克隆上会算少 —— 所以 `.github/workflows/verify.yml` 的 checkout 显式写了 `fetch-depth: 0`，
这条不加就等着 CI 上「版本线全绿但其实只看到最近几条」）。

**破了谁（规则 8 要求写清）**：`#93` 这次重推里只有两处形状变化，其余全是加法 ——

- `documents` 的 `#90`（`705d1490`：从 `MANIFEST` / `ROUTES` / `capabilities` / 依赖里整体下线，
  17 行端点声明删掉）是**破坏性**的：破 NousDesk（`/v1/documents/*` 已无调用点，但「关于 →
  插件」会少一行）与旧面板调用方（2026-11-18 已下线，无活体）。
- `modules[].version` 的含义变了（发行版号 → 插件自己的版本），并新增 `versionSource`。
  这是**加法**：老桌面端只读 `name` / `enabled` / `endpoints` / `reason`，照旧能解析；
  新客户端按「有 `versionSource` 就是回退值」分支。破的是**任何拿 `modules[].version`
  当发行版号做比较的脚本**（仓里 0 处，`NousDesk` 只拿来显示）。

**反悔点**：如果哪天要「按版本协商能力」（客户端要求 `ext >= 0.6`），机械推的版本线就不够了，
得换成发布记录逐条对得上 —— 那时 `check-plugins.sh` 该读 `plugin.toml` 的 changelog 段，
而不是比 HEAD。已知低估也写在 `docs/plugin-versions.md` 里：端点只改回包形状 / 状态码
这类变更机械判据抓不到，minor 是下界不是完整清单。

## 2026-11-25 会话归档搬进插件（`crates/Data/Archive`，vendor=`data`）

**背景**：搬家票（`docs/magento2-migration.md`）按 §3.4 的标杆逐个插件推进。`archive` 是
第 4 个，3 条端点：`POST /v1/sessions/{id}/archive` · `GET /v1/archive` ·
`POST /v1/archive/{id}/restore`。同时它还是「换出品方」的第一个非 `data-manager` 例子。

**决定**：

- **宿主能力是「命令面」，不是库路径。** `Host::{archive_session, archive_generations,
  restore_session}` 三个方法各一个能力，实现只在 `server/src/plugins.rs`，全部落在同一个
  `CommandSource`（同一个 `Session` 写者）上 —— 插件不另开写通道，也就不会出现第二份
  gen 文件 / 墓碑口径。
- **状态码不在插件里重算。** `command_error` 已经把 `CommandError` 翻成
  `ServerError`（409 是「现在不行」、400 是「这么写不行」、404 是「没有这条」）；
  `plugins.rs` 只把 `ServerError` 的 `status()` + `message()` 原样装进
  `data_archive::routes::Refusal`。插件只补自己那两条 400（id 不是数字 / restore body 坏）。
- **供应商换 `data`。** 目录 `crates/Data/Archive`、包名 `data-archive`、`plugin.toml`
  的 `vendor = "data"` / `capability = "archive"`（规则 10 + 规则 11）。模块名保持
  `archive` —— `docs/plugins.md` 的 module 列、`capabilities` 的 `name`、NousDesk 的
  `Feature::Archive` 都按这个名字判能力，改它等于跨仓改契约，不在这一票里。
- **`wire.md` 一个字没动。** 三条路径与回包形状（`{gen,ids,rows}` /
  `{gens:[{gen,items}]}` / `{restored,gen,rows}`）逐字保留，所以客户端（NousDesk
  `nous-engine`）零改动 —— 已核：`client.rs` 只按路径拼 URL、按 `serde_json::Value` 收包。

**为什么**：`archive` 存量只有 3 条，但它是**第一个写面插件**。`data-manager` 证明的是
「只读聚合能搬」，这里要证明「写命令也能搬，且不破坏单写者」。选它而不是 `elements`（8 条、
零新接缝），是因为 `sessions`（17 条）、`triggers`（24 条）都要走同一类命令面接缝，
先把这条缝立起来，后面两票是抄。

**代价**：`Host` 是 `#[async_trait]`（命令是 async），插件因此多了 `async-trait` 依赖；
`data-manager` 那套 `Refusal` 枚举带语义分支，这里换成「状态码 + 文案」的窄结构 ——
插件不重算语义，但也不再拥有「哪些状态码合法」的知识。

**反悔点**：如果后面搬 `sessions` 时发现每个插件都要各写一份 `Refusal` → `ServerError`
的翻译，就把这对映射提到 `nous-routes`（契约层）里做一次；现在只有两个插件，提前抽会
造出一个只有一个实现的 trait。另外：真机上「归档 → 归档库页刷新 → 打捞」那一眼还没验
（见 `docs/` 的部署判据），部署后要补。

---

## 2026-11-26 · 三份私有 `Refusal` 归档进 `nous_routes::HttpRefusal`（第五个插件之后）

**背景**：`decisions.md` 在 `archive` / `elements` 那两票各记了一次反悔点 ——「等第三个插件也要
`RepoError → 状态码` 这套翻议时，提到公共层做一次」。`prompts` 那票踩中了，并且实测出落点不是
`nous-routes`：`env` 插件自带 `GET /v1/fs/dirs` 之后有了 `env → routes` 这条边，而
`repo → env` 已经在，把语义层放 `routes` 会成环 `env → routes → repo → env`。正解是
`nous_repo::Refusal`（拥有 `RepoError` 的那层），`nous-routes` 只出传输壳。

**这一步做的是什么**：把 `env` / `combos` / `archive` 三个插件各自的私有 `Refusal`
（形状都是「状态码 + 文案」，其中 `env` / `archive` 用 `StatusCode`，`combos` 用 `StatusCode`）
统一换成公共壳 `nous_routes::HttpRefusal`（`u16` + JSON 体）。

**为什么不全走 `nous_repo::Refusal`**：那层是给 `RepoError` 用的语义翻译，而这三个插件的
拒绝理由分别来自 `BrowseError`、`RemoteDirsError`、`ServerError`、盘面校验 —— 它们跟 git
无关。硬套 `nous_repo::Refusal` 会让插件为了一个回包形状去依赖 `nous-repo`，方向反了。

**两个新构造子留在插件里**：`HttpRefusal` 只有 400 / 404 / 409 / 423 / 500 五个现成构造子。
`combos` 需要 412（`If-Match` 版本不匹配）和 503（触发器运行时未装配），这两个**语义只在
`combos` 里**，所以放在 `combos` 而不是公共壳：
`precondition_refusal` / `service_unavailable_refusal`（`pub`，宿主的 `impl` 也要用）。

**代价**：`HttpRefusal` 的 `status` 是 `u16` 而不是 `StatusCode`；`IntoResponse` 里
`from_u16().unwrap_or(500)`。这意味着写错一个状态码数字不会在编译期被抓到。可接受：
新增状态码时必须穿过这层，而这里每个数字都有现成语义构造子可用。

**反悔点**：如果第四个插件也要 412 或 503，就把这两个构造子提到 `HttpRefusal` 里（那时
它们是公共语义了）；如果 `HttpRefusal` 的 `u16` 导致一次真实的状态码写错事故，就改回
`StatusCode`（代价是 `nous-routes` 仍然只依赖 axum，没有额外耦合）。

## 2026-11-25 元素库 8 条端点搬进插件（`nous-elements`，零新增接缝）

**背景**：搬家票第 5 个插件。`elements` 与前面四个都不同 —— 它**早就是一个独立 crate**
（`crates/Nous/Elements`，602 行），盘上真相、id 校验、锁语义、「写一次提交一次」的事务
全在里面；不搬的只有 8 条端点（`server/src/elements.rs`，377 行纯翻译层）。

**决定**：

- **宿主能力只有一个方法**：`Host::elements_repo() -> Option<Arc<Repo>>`。比 `data-manager`
  的 `stats_paths`（两个路径）还窄 —— 因为元素库要的不是路径，是那个已经开好的仓库句柄
  （写锁、`requestId` 回执、git 提交都在里面）。给路径等于让插件自己 `Repo::open` 第二份，
  两份写锁 = 两条提交线。
- **`RepoError → 状态码` 的翻译搬进插件**（`Refusal::from_repo`），不再留一份在
  `server/src/repos.rs` 旁边。搬家前后逐字一致：`BadRequest` / `InvalidPath` → 400、
  `Validation` → 400 + `{field, problem}`、`NotFound` → 404、`Conflict` → 409（那句
  「先重新打开这个文件，再存一次」原样保留）、`Busy` → 409、`Locked` → 423、`Io` → 500。
  分界只在一处判，8 个 handler 都走它。
- **`REPO_KEY` 跟着插件走**：`nous_elements::routes::REPO_KEY` 是「元素库在
  `AppState.repos` 里叫什么」的唯一真相。`models.rs`（编目适配）也改成读它 —— 于是
  插件、`element-models`、`models.rs` 三处看到的是同一个键，不可能有人另开一份 `NOUS_ELEMENTS`。
- **`wire.md` 一个字没动**：8 条路径与全部回包形状逐字保留，NousDesk 零改动。

**为什么**：这票的价值不是「又搬了 8 条」，而是证明**接缝可以有多窄**。`data-manager`
要两个路径、`archive` 要三条命令，`elements` 只要一个句柄 —— 三种接缝摆在一起，
后面搬 `skills` / `prompts`（同样是「领域逻辑已在 crate 里、只缺端点」）时直接照抄第三种。

**代价**：`Refusal` 从枚举变成「状态码 + 文案 + 可选 `{field, problem}`」的结构体 ——
插件不再拥有「哪些状态码合法」的知识，只搬运。这是刻意的：`Locked`(423) 这种码是
`nous-repo` 的语义，插件抄一遍就是第二份真相。

**反悔点**：如果第三个插件也开始需要「`RepoError` → 状态码」这套翻译（`skills` / `prompts`
很可能），就把 `Refusal::from_repo` 提到 `nous-routes` 里做一次共享实现；现在只有这一个
插件要，提前抽会造出一个只有一份实现的公共函数。另外：注册表写法从 `{kind}` 变成
`:kind`（`nous-routes` 折的），`capabilities` 的 `endpoints[]` 快照跟着变 —— 如果哪天
Desk 端有代码按 `{kind}` 字面比字符串，那处要改成认两种写法（现在只 `normalize_path`
折 `:`，已核 `capability.rs:108`）。

---

## 2026-11-26 技能六端点搬进插件（`library-skill`）—— 骨架先下沉，再搬端点

**背景**：搬家票第 6 个插件。`skills` 与前面五个都不同：6 条端点里 **5 条不住在自己的文件里**，
而是和 `prompts` / `extensions` 挤在 `server/src/resource_admin.rs` 那一副**三仓共用骨架**里
（一个 `Kind` 枚举 + 一套 `files/file/create/publish` handler）。

**决定**：

- **先下沉，再搬家。** 把骨架里的盘面规矩（`check_name` / `check_path` / 递归清单 /
  骨架落盘 / 发布）提到 `nous_repo::entries`（platform 层，本来就是「私库文件 CRUD + git 发布」
  那一层），三仓共用同一份；然后 `skills` 插件拿走自己那 6 条，`resource_admin.rs` 只剩
  prompts / extensions（522 → 309 行）。
- **不把整副骨架塞给一个插件。** 如果为了「搬 skills」把 `Kind::Prompts` / `Kind::Extensions`
  也搬进 `library-skill`，那 `skills` 插件就得依赖 `nous-ext`（运行时）—— 一个只读解析插件
  被迫背上扩展运行时，依赖图当场破。所以切分线画在「盘面规矩（共用）」与「HTTP 翻译（各仓各的）」之间。
- **宿主能力三个方法**：`skills_repo()`（写面要仓库句柄：写锁、etag、git 提交都在里面）、
  `skills_root()`（只读投影要路径：`library_skill::scan` 只吃 `&Path`）、
  `skills_usage()`（`?usage=1` 反查归 `SessionSource`，插件不自己开库）。
  前两个是同一份私库的两种取法，不是一个能力拆两半。
- **`meta::skills()` 整份删除**（投影逻辑搬进插件的 `skills` handler），静态表删 6 行
  （143 → 137）。
- **`publish` 的回包形状逐字不变**：`{commit, pushed, refs}`，`commit` 是 hash **字符串**
  （只推没提交时 `null`）。我第一版把它塞成了对象，`resource_admin_endpoints` 当场打红 ——
  这条形状是三仓共用的，改一处三个仓一起坏。

**为什么**：`elements` 证明的是「接缝可以只有一个句柄」，`skills` 证明的是
**「端点不在自己的文件里也能搬」** —— 前提是先把共用部分放到它该在的层，而不是让插件互相依赖。
后面 `prompts`（同样 5 条在骨架里）和 `ext`（4 条）是抄这一票：骨架已经下沉，它们只需
拿走自己的 HTTP 翻译。

**代价**：`nous-repo` 多了 `entries` 这一层（约 380 行），`library-skill` 从「零 IO 的只读解析」
变成「app 层的插件」—— 它的 `plugin.toml` `layer` 仍是 `app`（本来也是），但 `requires` 从空
变成三条。`resource_admin.rs` 里 `Kind::Skills` 那一支整支消失，`MAX_FILES` / `RESERVED` 等
常量也只剩一份（在 `nous_repo`）。

**反悔点**：如果 `prompts` 搬的时候发现骨架还剩「只有 skills 用」的分支（例如技能独有的
frontmatter 校验），那说明切分线画错了 —— 应当把那条分支挪回插件，而不是在 `nous_repo`
里加 `if kind == Skills`。另外：`nous_repo::entries` 现在同时服务「HTTP 端点」和「插件」，
如果哪天它开始需要知道 HTTP 状态码，那就说明它越界了（它只回 `RepoError`，状态码翻译在
各插件/`resource_admin` 的 `from_repo` / `repo_error`）。

---

## 2026-11-26 模板六端点搬进插件（`Nous_Prompts`）—— 第三次抄 `Refusal`，就该提上来了

**背景**：`skills` 那票把盘面规矩下沉到 `nous_repo::entries`，`resource_admin.rs` 从三仓
（skills / prompts / extensions）缩到两仓。这票搬走 `prompts`，那个文件只剩扩展一支。

**决定**：

- **照抄 `skills` 的接缝，一个字不改**：`Host::prompts_repo()`（写面要仓库句柄）+
  `Host::prompts_root()`（只读投影要路径 —— `nous_prompts::catalog_value` 只吃 `&Path`）。
  没有第三个方法：模板没有 `?usage=1` 那种反查。
- **`GET /v1/prompts` 跟着端点走**：它原来住在 `meta::prompts()`（静态元数据），现在由插件
  自己投影。`root` / `present` / `missing` 三格是 `#63` 的形状，`catalog_value` 的键原样透传。
- **`RepoError → 状态码` 提到 `nous-repo`**（`Refusal::from_repo`），不是提到
  `nous-routes`。`decisions.md` 在 `elements` 那票写的反悔点说「提上去」，但**放哪一层
  是这次才定的**：先试了 `nous-routes`，编译直接报环 ——
  `nous-env → nous-routes`（`env` 插件自带 `GET /v1/fs/dirs`，上一票刚加的边）
  而 `nous-repo → nous-env`，于是 `env → routes → repo → env` 成环。
  正解是**放到拥有 `RepoError` 的那一层**：`nous_repo::Refusal` 翻译自己家的错误，
  依赖方向永远对，不可能再成环。
- **`nous-routes` 保持零 `nous-*` 依赖**，只出 HTTP 回包壳 `HttpRefusal`（状态码数值 +
  JSON 体，不认领域错误类型）。插件做一次 `Refusal → HttpRefusal` 的搬运（`fn http(...)`，
  一个 crate 四行）。这样 `nous-env` 那条边可以继续存在，两边都不动。
- **`resource_admin.rs` 的 `create` handler 整块删除**：它只为 prompts 存在（扩展的 create
  在 `ext_admin`，带骨架与运行时）。`Kind` 枚举只剩 `Extensions` 一支，`CreateBody` 一并消失。
- **`wire.md` 一个字没动**：6 条路径与全部回包形状逐字保留，NousDesk 零改动。

**为什么**：这票本身没什么新东西 —— 它的价值是**验证「第三次就该抽象」这条判断**。
前两票各抄一份 `Refusal` 是对的（两份 = 还没定型），第三份出现时提上来也是对的（三份 =
形状稳了）。提前抽会造出一个只有一份实现的公共函数；拖到第四份就是四份拷贝要一起改。

**代价**：`Refusal` 与 `HttpRefusal` 分了两层 —— 一个语义、一个回包，插件里多一个四行的
`fn http(...)`。这是环逼出来的：`nous-routes` 必须保持零 `nous-*` 依赖（`nous-env` 依赖它），
而状态码语义必须住在拥有 `RepoError` 的地方。分成两层反而更干净：`nous-repo` 不需要
认识 axum，`nous-routes` 不需要认识 `RepoError`。

**反悔点**：如果哪天有插件只想要 `HttpRefusal` 的结构体、不想要 `nous-repo`（例如纯内存
端点），它直接用 `nous_routes::HttpRefusal` 就行，不必摸 `nous_repo::Refusal` —— 两层的
拆分正好让这件事免费。另外：`Refusal.status` 是 `u16` 而不是 `StatusCode`，是为了让
`nous-repo` 不依赖 axum；如果哪天 `nous-repo` 需要 HTTP 类型（不太可能），再换。
另外：`resource_admin.rs` 只剩扩展一仓了，`ext` 搬走时这个文件应当**整份删除**，
`Kind` 枚举随之消失 —— 如果那时有人选择「留着给未来的仓用」，那就是第四份拷贝的开始。

**同票顺带改名**：模块全名 `prompts` → `Nous_Prompts`（规则 12「老模块的小写连字符全名…
逐个改成全名」）。只有三处跟着变 —— `plugin.toml` 的 `name`、`MODULE` 常量
（`/v1/_routes` 的 `module` 标签、`capabilities.modules[].name`、
`GET /v1/plugins/:name/readme` 都按全名说话）、`meta::MANIFEST` 的那一行。
**目录 / 包名 / capability / URL 一个字没动**：`crates/Nous/Prompts`、`nous-prompts`、
`prompts`、`/v1/prompts*` 都是小写世界，跟全名是两回事（规则 12 的三套写法）。

代价：`docs/plugins/` 下的产物文档文件名跟着从 `prompts.md` 变成 `Nous_Prompts.md`
（`deploy.sh` 按全名落盘），**旧产物里那份 `prompts.md` 要重新同步一次**，否则
`GET /v1/plugins/Nous_Prompts/readme` 会回 404 `readme_not_installed`。这是改名的
可观测代价，记在这里免得当成 bug。

## 2026-11-26 扩展十五端点搬进插件（`Library_Extend`）+ 改名 `ext` → `Extend`

**为什么搬到插件里**：`ext` 早就是清单里的模块、crate 也一直在（`nous-ext`），
只有端点在 `server` 里当转发层 —— 那是「模块存在、端点不归它」的最后几个漏网之一。
搬完 `server/src/resource_admin.rs`（三仓共用骨架的最后一支）与 `ext_admin.rs`
**两个文件整份消失**，扩展系统的「运行时 + HTTP 面」终于是同一个 crate。

**接缝为什么比前几票厚（4 条宿主能力，不是 2 条）**：

| 端点组 | 条数 | 要什么 | 为什么不能合并成一条能力 |
|---|---|---|---|
| 编辑页 `files`/`file`/`publish` | 4 | `nous_repo::Repo`（键 `extend`） | 写锁 / etag / git 提交都在 `nous-repo` 里，插件不许自己 spawn git |
| 管理面 `create`/`duplicate`/`install`/`uninstall`/`categories`/`category`/`trash` | 9 | `AdminStore` 盘面 + `Runtime` | 目录动作在盘面、进程动作在运行时，**本来就是两件事**（`install` 两条都要） |
| 清单 / 重载 | 2 | `Runtime` | 只读投影 + 按盘上现状重装 |
| `?usage=1` | — | `Result<Value, HttpRefusal>` | 见下 |

**`ext_usage` 返回 `Result` 而不是像 `skills_usage` 那样 `Option`**：搬前是
`state.source().usage("extensions")?` —— 直传状态码（反查失败是 500）。折成 `None`
会把 500 说成「未装配」，那是一句假话。Trait 方法返回插件的 `HttpRefusal` 是可行的，
因为 `Host` 由宿主实现，宿主本来就有 `nous-routes`。

**改名（规则 12）：`ext` → `Library_Extend`，这是真改名，不是「改成全名」**。
`prompts` 那次只动全名（目录/包名/capability 本来就对）；这次全名、vendor 之外的
三套写法都要跟着变：

- 目录 `crates/Nous/Ext` → `crates/Library/Extend`
- 包名 `nous-ext` → `library-extend`，crate 标识 `nous_ext` → `library_extend`
- capability `ext` → `extend`（`check-plugins.sh` 要求它 == 目录名的小写机器名）
- **URL 段 `/v1/extensions*` 和私库键 `extend` 一个字没动** —— 它们是「小写世界」，
  跟全名不是一回事；`decisions.md` 2026-11-19 那条（`ext` 就是 `ext`，不是 `extensions`）
  说的是清单/协议两处的命名分歧，改名不推翻它。

**代价 / 反悔点**：

1. `docs/plugin-versions.md` 的考古会因目录改名重算一次档案（规则 8 把「目录改名」
   算身份变更 = minor，所以 `ext` 0.6.6 → `Library_Extend` 0.8.0：改名一个 minor、
   端点数一个 minor）。**别把它当 bug 修回 0.6.x。**
2. `docs/plugins.md` 的 module 列从 `ext` 变成 `Library_Extend`，NousDesk 的
   `capability::module_matches` 认「全名」或「`_` 之后的尾段」—— `Feature::Ext`
   登记的短名是 `ext`，**对不上 `Library_Extend` 的尾段 `Extend`**。所以 Desk 侧要同步
   把 `Feature::Ext::module_name()` 改成 `extend`（同票，Desk 分支）。不改的表现是
   「功能页整块灰掉、理由是未登记」，不是崩 —— 但一样是 bug。
3. 产物文档名从 `ext.md` 变成 `Library_Extend.md`（`deploy.sh` 按全名落盘）。

## 2026-11-26：新建 `Library` vendor，迁 `Library_Extend` 与 `Skill`（本票）

选择：把扩展运行时与技能插件从 `Nous` 出品方迁到新 vendor `Library`：
`Nous_Extend` → `Library_Extend`（目录 `crates/Library/Extend`、包 `library-extend`、
capability `extend` 不变、版本 0.8.0 → 0.9.0）；`skills` → `Skill`（单段 PascalCase，
目录 `crates/Library/Skill`、包 `library-skill`、capability `skills` → `skill`、
版本 0.3.1 → 0.4.0）。`git mv` 保住历史。

为什么：规则 10（vendor 是出品方）与规则 12（三套写法由全名推导）；规则 8 明写
capability 改名 = minor、身份/目录改名按最大跨度涨一次。单段 PascalCase 是规则 12
允许的形状（`Models` 是先例），因此同步修 `plugin_versions.rs` 的 `module_name_ok()`
让它认这一支，否则版本判据会跳过 `Skill` / `Models` 空转。

破坏的是谁（规则 8 要求写明）：`/v1/capabilities` 的 `modules[].name` 与 `vendor`
分别变成 `Skill` / `Library`、`Library_Extend` / `Library`；`GET /v1/plugins/:name/readme`
的旧 URL `/v1/plugins/skills/readme` 会 404 `unknown_module`（不做别名，先例见 `Models`）；
产物文档名变 `Skill.md` / `Library_Extend.md`。**端点路径 `/v1/skills*` / `/v1/extensions*`、
会话字段 `skills`、私库键、环境变量 `NOUS_SKILLS` / `NOUS_EXTEND`、数据根
`/opt/Nous-skills` / `/opt/Nous-extend`、`exts/` 外挂目录、`nous-ext-sdk` 全部不动** ——
它们是数据/URL 世界，不是模块身份；要改得另开契约票并迁移线上数据。
Desk 侧按 `module_matches` 的尾段匹配：`Skill` 对 `want="skill"` 成立；
`Library_Extend` 的尾段 `Extend` 与 Desk 的 `Feature::Ext` 短名 `ext` 不匹配，
Desk 需另票把 `module_name()` 改为 `extend`（与上一决定同口径）。

反悔条件：若客户端不能接受 `/v1/plugins/skills/readme` 404 与 capabilities 名字变化，
则回退整票（含 vendor 目录），而不是只回退 README 文件名。

3. 产物文档名从 `ext.md` 变成 `Nous_Extend.md`（`deploy.sh` 按全名落盘）。

## 2026-11-26 · `Trigger_Core`：24 条端点搬进新插件，两条宿主能力

**搬什么**：`/v1/triggers*`（11）+ `/v1/trigger-projects*`（13），合计 24 条，
落在新 crate `crates/Nous/Triggers`（包 `trigger-core`，capability `triggers`）。
（2026-11-26 后续：换供应商 Trigger、改名 `Trigger_Core`、目录迁到 `crates/Trigger/Core`、
capability 改成 `core` —— 见本文件最后一条。）
源文件 `server/src/triggers.rs`（950 行）+ `server/src/trigger_projects.rs`（1074 行）
整份消失，静态表 79 → 55 行。

**为什么新开 crate 而不是塞进现有插件**：`triggers` 在 `docs/plugins.md` 里本来就是个
app 层模块（还是两行：`triggers` 运行时外挂 + `trigger-catalog`），只是没有自己的 crate。
搬家票做的就是「补上那个 crate」—— 于是两行并成一行，形态从「运行时外挂」变「编译期」。

**接缝判断：为什么是两条能力、不是一条**。这是本票唯一值得争的地方。

24 条端点看着是同一个人家的，但它们操作的是**两种不同的东西**：

- `/v1/triggers*` = 库里的一行定义（`nous-store` 的 `triggers`/`fires`）+ 点火账 + 调度健康。
- `/v1/trigger-projects*` = 盘上的一个 cargo 项目目录（源码、能编译、能发布）。

合成一个「触发器宿主」大对象（`triggers_host() -> Trait` 之类）会更整齐，但那等于说
「能点火的人自然也拿到了 git 写锁、也能起 cargo」—— 假的。两条就两条：
`triggers_runtime()` 与 `triggers_repo()`。

`triggers_runtime` 返回 `Option<&TriggerRuntime>`（借用）而不是 `Option<Arc<..>>`：
`TriggerRuntime` 本来就住在 `AppState` 里（搬家前就是 `Option<TriggerRuntime>`），
handler 拿到手只读——`store.lock()` / `definitions()` / `fire()`，没有一个要所有权。
顺手加一层 `Arc` 只会让 `plugins.rs` 的 `impl Host` 多一次 clone。

**`FAUX_MODEL_ID` 下沉到 `nous-ai`**：点火成功后的 faux 轮次要构造
`TurnRequest.model`，用的是 `server::runtime::FAUX_MODEL_ID`（`"echo"`）。插件不能
依赖 `server`，但也不该抄一份字面量（faux 的回复正文里带着这个 id，抄一份就会漂）。
它本来就该归拥有 `FauxProvider` 的那一层 —— 落在 `nous-ai`（0.2.1），
`server/src/runtime.rs` 用 `pub use` 转出去，既有调用点一个字都不用改。

**`HttpRefusal` 补 `Display` + `Error`**：CLI `nous-app --triggers` 与
`GET /v1/triggers` 是**同一个函数**（#58 判据 4），搬走之后 CLI 那一侧 `?` 需要能转换。
不补这两个 impl 就得在 CLI 里包一层「同一份 JSON 的再装配」——那正是搬运票要消灭的东西。
`nous-routes` 0.2.1 → 0.2.2（纯加法，patch）。

**行为唯一的改动**：`RepoError::Conflict` 的回包文案统一了。
`trigger_projects.rs` 原来自己写「……—— 文件被别处改过了，先重新读盘再存」，
现在跟三套私库一样走 `nous_repo::Refusal::from_repo`（「……—— 先重新打开这个文件，
再存一次」）。两句话说的是同一件事，四处各写一份才是 bug；没有测试钉旧文案。

**代价 / 反悔点**：

1. 新 crate 出生即全名（规则 12），**没有改名那趟** —— 这是 `prompts`/`ext` 两次
   改名得出的结论：新东西直接叫对名字，比事后改便宜。
2. 静态表只剩 `repo`(30) / `sessions`(17) / `Nous_Device`(8)。`repo` 那 30 条是
   下一个，也是「`nous-repo` 该不该自带 HTTP」这个决定的最后机会 —— 现在它还是
   platform 层、零 axum，选择「宿主注入根路径」的话这条线能保住。
3. Desk 侧不用动：`Feature` 表里没有「触发器」这一项，`Trigger_Core` 只在
   `capabilities.modules` 里多一行（改名窗口的问题在 `Nous_Extend` 那次已经踩过）。

## 2026-11-27 模块全名 `Element_Models` → `Models`

**决定**：`crates/Element/Models/plugin.toml` 的 `name` 改成 `Models`，插件版本 `0.2.0` →
`0.3.0`（规则 8：身份行改了就是 minor —— `plugin-versions.py` 的 `MANIFEST_KEYS` 也认这一条）。

**只动了全名。** 目录 `crates/Element/Models`、包名 `element-models`、capability `models`、
元素库组 id `GROUP_ID = "element-models"`（`wire.md` 1016 规定固定）一个字没动。规则 12 的
三套写法各自独立，所以全名改变不影响磁盘、不影响 cargo、不影响盘上数据 —— 只影响
`/v1/capabilities` 的 `modules[].name`、`/v1/_routes` 的 `module` 标签、`/v1/plugins/:name/readme`
的路由名，以及桌面端插件页那一行（客户端只透传 `name`，不改代码、不存对照表，规则 9）。

**为什么**：全名 `Element_Models` 里 `Element` 是 vendor 段，模块段是 `Models`，读起来重复；
`Nous_Device` 那种两段式是为了 Magento 的 `Vendor_Module` 长相，但这里模块段本身就是
完整语义（「模型」），vendor 已经由 `modules[].vendor` 与目录第一段表达。

**代价**：旧的 URL 名字直接 404 `unknown_module` —— 不做别名、不保留兼容名。理由是「名字是
身份」（规则 12）：留一个别名就等于插件有两个名字，`/v1/_routes` 的 `module` 标签、
`docs/plugins.md`、`plugin.toml` 三处都得能对上一个，别名会立刻把这个不变量破掉。
桌面端没有硬编码，跟着引擎报的名字走，所以这一改动对它零成本。

**反悔点**：如果哪天发现全名第一段必须与 vendor 一致这条规则要被机器判据强制（目前
`check-plugins.sh` 只在 `_` 存在时比对第一段与 vendor），那 `Models` 这种无 vendor 段的全名
就会成为「老式小写连字符名」之外的第三种形态，判据得再补一档 —— 那时要么改成
`Element_Models`（本决定作废），要么把「全名可省略 vendor 段」写进规则 12。

## 2026-11-26 · 换供应商：`Nous_Triggers` → `Trigger_Core`

**做了什么**：触发器插件整份换出品方。按 `docs/plugins.md` 规则 10/12，**三套写法一起动**：

| 东西 | 之前 | 之后 |
|---|---|---|
| 目录 | `crates/Nous/Triggers` | `crates/Trigger/Core` |
| 包名（Cargo） | `nous-triggers` | `trigger-core` |
| crate 标识 | `nous_triggers` | `trigger_core` |
| 全名（清单 / `_routes` / readme） | `Nous_Triggers` | `Trigger_Core` |
| `vendor` 列 | `Nous` | `Trigger` |
| capability | `triggers` | `core` |
| **URL 段** | `/v1/triggers*`、`/v1/trigger-projects*` | **一个字没动** |
| **私库键** | `triggers`（`NOUS_TRIGGERS` 的根） | **一个字没动** |

**为什么 capability 跟着变成 `core`**：`check-plugins.sh` 要求 capability == 目录名的
小写机器名（`Core` → `core`）。这条不是装饰 —— capability 是「模块在 cargo 世界里叫什么」，
和 URL（协议世界）、私库键（数据世界）本来就是三套写法。前两次搬家已经在
`Nous_Extend` 上踩过同一条线（私库键 `extend` ≠ URL 段 `extensions`）。

**为什么这是「真换供应商」，不是改个名字**：`Nous` 现在是多数派，但规则 10 明写
「同一个装里可以有多家 vendor 并列」。触发器这一块有它自己的出品方语义（用户自己写
`/opt/Nous-Trigger` 里的 cargo 项目、自己发版），归到 `Trigger` 比塞在 `Nous` 底下更诚实。
代价是它得单独记账：`docs/plugin-versions.md` 的考古因为目录挪动会重算一次档案（0.1.0 → 0.2.0，
身份变更 = minor，规则 8），**别当 bug 修回去**。

**代价 / 反悔点**：

1. **版本 0.1.0 → 0.2.0**（身份变更 minor）。考古只看得见「目录挪了 + 全名变了」这一件，
   看不见「换了出品方」——两者在这张表上是同一个 minor，不再多加一档。
2. **桌面端要跟一条**：`Feature::Triggers` 的 `module_name` 从 `triggers` 改成 `core`
   （引擎 `capabilities` 报的全名尾段是 `Core`），并留 `triggers` 别名 —— 引擎与桌面两条
   发布线，谁先升级都不许把「触发器」整块灰掉（与 `Nous_Extend` 那次同形）。
   不改的表现是「功能页整块灰、理由未登记」，不是崩。
3. **产物文档改名**：`deploy.sh` 按 `plugin.toml` 的 `name` 落盘，部署后
   `docs/plugins/` 里是 `Trigger_Core.md`；旧 `Nous_Triggers.md` 会 404 `readme_not_installed`
   直到跑一次 `deploy.sh --docs-only`。
4. **`core` 这个尾段偏通用**：Desk 的匹配规则是「全名」或「`_` 之后的尾段」，
   以后真出现 `Foo_Core` 就会撞。撞之前把 Desk 的匹配改成「全名优先、尾段只在唯一时用」——
   现在只有一个 `_Core`，先不动。

## 2026-11-26 全名 `Library_Extend` → `Extend`

**决定**：`crates/Library/Extend/plugin.toml` 的 `name` 改成 `Extend`，插件版本 `0.9.0` →
`0.10.0`（规则 8：身份行改了就是 minor）。**目录、包名、vendor、capability 一律不动** ——
仍是 `crates/Library/Extend`、`library-extend`、`vendor = "Library"`、`capability = "extend"`。

**为什么**：`Library_Extend` 把出品方名字塞进了模块名里，读起来重复；`Extend` 本身就够清楚，
出品方由 `modules[].vendor` 与目录第一段表达（和 `Models` 同一个形态 —— 单段 PascalCase 全名，
`vendor` 是 `Element` / `Library`）。规则 12 允许这样：判断「全名第一段 == vendor」只在全名里
有 `_` 时做，无 `_` 的单段全名由 `modules[].vendor` 说清楚。

**代价**：`/v1/plugins/Library_Extend/readme` 从此 404 `unknown_module`（名字是身份，不给别名），
产物文档名从 `Library_Extend.md` 变成 `Extend.md`。桌面端零改动：它只透传 `name`，扩展页认的是
短名 `extend`，`Extend` 的尾段正好是 `extend`（`module_matches` 的 flatten 比对），不用改。

**反悔点**：如果哪天机器判据要求「全名必须带 vendor 段」（即 `Vendor_Module` 是唯一合法形态），
`Extend` / `Skill` / `Models` 三个单段名要一起补成 `Library_Extend` / `Library_Skill` /
`Element_Models` —— 那时改的就是规则 12 本身，不是这三个模块。
## 2026-11-26 · 模块关系网：**构建期抄清单 + 只表达 crate 边**

**决定**：新增插件 `AboutME_ModuleRelationshipNetwork`（vendor `AboutME`，目录
`crates/AboutME/ModuleRelationshipNetwork`，包 `aboutme-module-relationship-network`），
一条端点 `GET /v1/modules/graph` —— 把各插件 `plugin.toml [requires]` 投影成模块级依赖图。

三条具体选择：

1. **原料是 `plugin.toml [requires]`，不是运行时探测。** 那份字段由 `scripts/check-plugins.sh`
   与 `cargo metadata` **双向逐条比对**（少写 / 多写都红），所以它已经是「插件真依赖了谁」的
   可信抄本。运行时扫源码 / 动态挂载探测会给出不确定的结果，还在 `j1900` 上白烧启动时间。
2. **生成放在 `build.rs`（`include!`），不放仓库里的 JSON。** 与 `crates/Nous/Server/build.rs`
   （#93 的 `PLUGIN_VERSIONS`）同一套模式：编译期抄一次，图与产物同一份，启动零 IO；也不像
   `build.rs` 里跑 `cargo metadata` 那样递归调 cargo（增量构建会锁）。进仓的 JSON 需要一条 CI
   生成校验才不漂，那是多一个机制，收益只是「不重编译也能看到图变化」—— 现在不需要。
3. **只表达 crate 边，语义依赖不进图。** 「`Extend` 干活得靠 `env` 给路径」这类判断
   内核里一个字段都没有（`Extend` 自己调标准库拿 `temp_dir`，`[requires]` 里没有 `nous-env`）。
   本插件不替任何人编：缺的就不在图上，被依赖却没有对应模块的包名进 `unmapped`（不静默丢弃）。

**为什么不挂在 `Device` 上**：`Device` 管的是「模型 / 机器编目 + 自省视图」，依赖图是另一件事；
塞进去会让插件版本、README、归属声明都开始回答两个问题。

**命名**：全名 `AboutME_ModuleRelationshipNetwork` —— `check-plugins.sh` 的 `NAME_OK` 只允许
**一个**下划线（`Vendor_Module`），所以 `Module_Relationship_Network` 那种多段下划线写法过不了
判据；`Module` 作为模块段的第一个词保留，不是被删掉的。

**反悔点**：① 如果图的边开始被用来做**功能判定**（比如「依赖没启用就灰入口」），那它就不能是
只读投影，得先在内核里给语义依赖一个手写字段并对它做判据 —— 那时这条「只表达 crate 边」作废。
② 如果要求「不重编译就更新图」，改成读盘 + mtime 缓存（照 `GET /v1/plugins/{name}/readme`），
`generatedFor` 随之失去意义。③ 如果模块数涨到几百、图超过 1MB，一次给全会变成启动负担，
那时改分页 / 按模块取（客户端已经在按模块渲染）。

## 2026-11-27 · #90 收尾：documents crate 与 `spec-documents.md` 一并删除

**背景**：2026-11-21 的 #90（JoeWest9966 拍板）只把 `documents` 从发行面摘掉（`MANIFEST` /
`ROUTES` / `capabilities` 不声明、`nous-server` 不再依赖），crate 按「保留在 workspace 但不再是
产品面」留在盘上。结果是库里躺着一具**孤岛**：17 条 `RouteInfo` 模板、`DocumentWriter` /
`DocumentReader` 两个 trait、`service.rs` 的泛型服务，**零 handler、零依赖、零注册**，而
`docs/spec-documents.md` 顶部还写着「#48 已落地」。文档与实情相反，比没有文档更坏。

**决定**：把 `crates/Nous/Documents` 整个目录、`docs/spec-documents.md`、workspace `Cargo.toml`
里的 `nous-documents` 依赖、`docs/plugins.md` 的 `documents` 行与目录计数一并删除。
`documents_off.rs` **保留**，改写成回归守卫：将来谁把 `/v1/documents/*` 加回来，它立刻红。

**为什么**：内容真相只有四个 git 私库（#90 已定），`documents` 若活成产品面就会有第二条修订链
（`document_revisions` vs `presets_revisions`）和两个写通道。删干净让「盘是唯一真相」在代码结构
上也成立，不再靠一行 `frozen` 备注遮着。

**不动的东西**：`nous-durable` 的 `documents` 表与 `scan_documents` / `write_document` 是**数据库
存储层**，被 sessions 与 `Data_DataManager` 的统计用着，和这个应用层端点模块同名不同物，一个字
没动。`docs/decisions.md` 里 2026-11-20/21 的 #86/#90 历史叙述也原样保留。

**反悔点**：与 #90 的同一个反悔点（没有 git 私库却要编辑内容 / 引擎必须持有半成品草稿 / 私库被
外部改坏需要第二副本）。真到那天，重开 documents 要**先把 crate 从 git 历史里捞回来**，并先改
`wire.md`、回答「两个写者谁签字」——不能悄悄把 `documents_off.rs` 删了当没事。

**CI 按提交出包：推 main 就挂一份带 sha 的成品到滚动预发布 `ci`**（2026-11-27）

**决定**：`.github/workflows/package.yml` 在**每次推 main** 时编一份 musl 静态 `nous-app`，
命名 `nous-app-<sha8>-x86_64-unknown-linux-musl`（带 `.sha256`），上传到**一个**滚动预发布
`ci`，只留最近 5 份；打 `v*` tag 时另发永久 Release。**正式版本号不变**（`v*` 仍是唯一
发版口径），`ci` 里的是「某个提交的产物」，不是版本。

**为什么**：能部署的最小单位是整仓链接出的那一个 `nous-app`，所以「改一个插件」也要重新
链接；平时改插件不涨版本、也不该为拿一个二进制给整个内核打 tag。上一版只有 `verify.yml`
（跑测试）、一个 release 都没有，部署只能现场 `cargo build`，换台机器就是 6–15 分钟冷编译
外加环境差异。实测参考：改一个插件增量 release 链接 ≈30 秒，冷编译分钟级。

**为什么不「每个 sha 一个 Release」**：几百个 release 之后 `gh release list` 就没法看了，
而且每个都带 tag。滚动一个 pre-release + 文件名带 sha，既能按 sha 取货，也不污染 tag 空间。

**为什么不把产物进仓**：静态二进制几十兆，进 git 会让 clone 永久变重，历史也无法清理。

**反悔点**：① 如果 `ci` 那个 tag 碍事（`git describe` 变脏），把上传目标换成 `gh api` 直传
release assets 之外的存储，资产名规则不动；② 如果 5 份不够回滚（要按周一那版复现 bug），
把保留数改大或改成按天保留；③ 如果 CI 4 核编 musl 太慢（>20 分钟），退回「asus 本地编 +
`deploy.sh` 上传」，本工作流删掉即可，`deploy.sh` 一行不用改。

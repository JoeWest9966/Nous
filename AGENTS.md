# Nous-Package · 维护纲领

这是 Nous 的会话内核：Rust，私有，对外只出 `/v1/*`。**这份文件是它唯一的维护纲领，AI 改这个库之前必须先读完它。**

纲领的四个词，全文档按这四个词思考：

- **冻结** —— 每个外部来源 pin 在一个版本号上，之后只进不出。不 pull、不 merge、不追版。
- **提货** —— 从上游拿东西是单向一次动作：读 → 抄进本库 → 记进 `SOURCES.md`。提完就和上游断了。
- **宪法** —— `wire.md`（仓库根）是本库对外行为的唯一权威，**权威版本住在这个仓库里**，Nous 侧只放只读镜像。它和上游源码、和你的直觉冲突时，宪法说了算。
- **契约面** —— 本库对每个上游只依赖一张能列完的最小面孔；面孔之外的东西对本库不存在。

上游的价值是**读**，不是**合**。上游发版时你的动作是打开 changelog，不是打开 `git pull`。

### 边界

- **本库之外一律不读不写**：`/opt/NousDesk`（桌面端）、Nous 仓（面板 + `qrd3wh` 内核 JS）、j1900 上的 `qrd3wh` 目录 —— 不读、不参考、不改。理由：`qrd3wh` 钉在 `pi-durable@1.4.0`，照着它 port 就是把 1.4 的语义抄回本库。
- **契约只看本库的 `wire.md`**。数据可以读：`fixtures/agent.sqlite` 是数据不是源码，只读打开。
- **上游 checkout 可读**（`/tmp/pi-rust`、`/tmp/pi-check`），只读、只停在 pin 的 tag 上。
- **版本以 `SOURCES.md` 的 pin 为准**：`pi-rust@v0.24.2`、`pi@v1.5.2`，这两个就是当前最新 tag。上游出了更新的 tag **不动** —— 按 §4.1 在 `docs/pitfalls-from-pi.md` 记一条，等指令。

---

## 一、来源与各自的规矩

五个来源，五套规矩。改任何一行代码前先确定它属于哪个来源。

| 来源 | pin | 拿什么 | 规矩 |
|---|---|---|---|
| [`bigfish1913/pi-rust`](https://github.com/bigfish1913/pi-rust) | `v0.24.2` | `crates/rpi-ai`（23,461 行）、`rpi-tools`（17,138）、`rpi-runtime`（6,728）、`rpi-telemetry`（593），加两个小抄：`rpi-harness/src/compaction/tokens.rs`、`agent_harness.rs` 的 retry 段；`rpi-cli/src/remote/protocol.rs` 的事件类型 | **冻结**。拷一次，pin tag，之后自己改，永不回头合并（它 0.24、月均 700+ commit） |
| [`earendil-works/pi`](https://github.com/earendil-works/pi) · `packages/chord` | `v1.5.2` | **只六个目录**：`src/{context,json,delta,sqlite,watch,output}`（≈5,700 行） | 自己 port。`delta/` 最值钱（3,969 行），先做它 |
| `earendil-works/pi` · `packages/durable` | `v1.5.2` | `src/{storage,session,harness,tools,types.ts,entries.ts,documents.ts}`（27,259 行里取核心）；`src/testing/**`（3,444 行一致性套件）；`docs/spec.md`（7,296 行） | 自己 port。**只借语义和坑，不借结构**：`harness` 只取 `generation`/`abort`/`live`/`partial-changes`，`watch` 那套不要（事件走 `nous-wire`） |
| `earendil-works/pi` · `packages/env` | `v1.5.2` | `daemon/**`（5,768 行 Rust，已写好）+ `docs/{protocol.md,semantics.md}`（241 行） | daemon 当**外部二进制** build，不改它。客户端照 `protocol.md` 自己写 |
| `earendil-works/pi` · `packages/ai` | `v1.5.2` | **只有** `src/api/google-generative-ai.ts`（468 行 TS） | 翻成 Rust 补进 `nous-ai`。`packages/ai` 的其余部分与本库无关 |

**MIT 义务**：`LICENSE.rpi` 和 `LICENSE.pi` 放在仓库根，永不删除。`SOURCE` 台账记到行，见第七节。

---

## 二、crate 契约图

依赖只许向下，且只许用表里列出的。加一条新依赖就是改这张表，改表要走分支 + 说明理由。

```
nous-telemetry     无内部依赖
nous-chord         无内部依赖      json / delta / sqlite / watch / output / context
nous-wire          无内部依赖      RemoteEvent 等对外事件类型
nous-env           nous-chord     只有 trait：FileSystem / Shell / ExecutionEnv / FileChange；
                                 取消与进度用 chord 那份 Context（不另造第二套）
nous-ai            → telemetry
nous-tools         → ai, env
nous-runtime       → ai
nous-durable       → chord, ai, wire, telemetry, env
```

三条由此推出的规则：

1. **工具靠 trait 注入。** `nous-durable` 通过 `nous-ai` 的 `Tool` trait 使用工具，`cargo tree -p nous-durable` 里**不出现** `nous-tools`。工具实现在 `nous-tools`，装配在 Nous 的服务层。
2. **`ExecutionEnv` 只有一份。** 契约在 `nous-env`（`FileSystem + Shell`）。本地实现、SSH/远端实现、测试用 InMemory 都在这个 trait 之下。
3. **`wire.md` 的家在这里。** 本库的 `wire.md` 是宪法的唯一权威版本；Nous 仓库根那份是只读镜像。`nous-wire` 是它的 Rust 投影，两者不一致时以 `wire.md` 为准。

---

## 三、开动之前

三步，每次都要走：

1. 读 `SOURCES.md` 里你要碰的那个文件那一行 —— 它从哪来、抄了多少、改了什么。
2. `rg '<符号>' crates/` 确认它到底在哪个 crate、被谁用。
3. 判定这次属于下面哪一类任务。四类之外的改动先开 issue 说清楚，别直接动。

---

## 四、四类任务

### 4.1 上游发版了

**不改代码。** 逐条读上游 changelog（`pi` 的 chord/durable/env，加 `pi-rust` 的 release notes），对每条 `fix` 问一句：我们的 Rust 版有没有同一个毛病？

**看到上游有比 `SOURCES.md` 里更新的 tag，也不许换 pin。** 在 `docs/pitfalls-from-pi.md` 加一节记录它的存在和主要变动，然后停 —— 换 pin 是人的决定。

完成判据：`docs/pitfalls-from-pi.md` 末尾新增一节，**当期每一条 `fix` 都有一行判定**（`中` / `不中` / `未知 + 原因`），没有一条被跳过。判成"中"的转成 4.4。

### 4.2 加一个 provider 方言

改 `nous-ai` 一个实现文件；需要的话在 `nous-runtime` 的 `RuntimeFactories` 里加一条路由。编目里出现新 `api` 值时，跳过要说清原因，不能静悄悄少一个。

完成判据：编目里该方言下的**每一个**模型都能列出、都能发一条消息、SSE 收到一条完整的 assistant 消息；落库记录的字段名跟 pi 的记录字段名逐个对齐（`camelCase`）。

### 4.3 加一个工具

实现在 `nous-tools`，只依赖 `nous-ai` + `nous-env`。

完成判据：`cargo tree -p nous-durable` 里没有 `nous-tools`；该工具在只读会话里被拒绝；输出超长时走 `truncate` 而不是截断原样。

### 4.4 改存储或会话语义

先改本库的 `wire.md`（宪法），再改 `nous-wire`，最后改 `nous-durable`。Nous 侧是只读镜像，等同步，不在那边编辑。

完成判据：三条全过 —— ① 从上游 `packages/durable/src/testing/**` 改写来的一致性套件全绿；② `agent.sqlite` fixture（189 会话 / 209 entries）读得出跟 `/v1/sessions` 同形的 JSON；③ 上一版写出的库能被新版打开（迁移可逆或有明确单向说明）。

---

## 五、守卫

**不做的事**，每条都给替代动作，遇到就在这里转一次身：

| 遇到的诱惑 | 替代动作 |
|---|---|
| 想 `git pull` 上游或 `cargo add` pi-rust | 走 4.1 读 changelog；真要代码就**提货**，抄进来记 `SOURCES.md` |
| 想拿 chord 的整个包 | 只六个目录。缺什么先问"这个功能 Nous 用得到吗"，用不到就停 |
| 想 port `pi-ai` / `rpi-harness` / `rpi-cli` / `rpi-tui` / `rpi-extensions` | `nous-ai` 已经是 `rpi-ai` 的冻结版；其余四个与本库无关 |
| 想沿用上游的表结构、`record` 形状、`durable_schema.version` | 表结构和 `record` 形状**照抄**上游（DDL 取 `storage/sqlite/migrations.ts` 的 v2 + 一列 `writer`；`record` 存 camelCase JSON）；**版本号从 1 数起、迁移链单向**、分拆列规则本库自己定。全部定在 `docs/storage.md`，改这三样就是改契约，先改文档 |
| 想用"类型兼容"保证互通 | 互通只靠**序列化后的字段名**（`camelCase`，`Option` 字段 `skip_serializing_if`）。加一个字段先看 `wire.md` |
| 想顺手"改进"上游代码的风格 | 冻结的前 45 天里不改。改了要记 `SOURCES.md` 的"改了什么"列 |

---

## 六、验收

仓库根的 `scripts/verify.sh` 必须能跑通这三条，这是本库唯一的"做完了"定义：

```bash
cargo test --workspace            # 含改写自 pi testing 的一致性套件
cargo run -p nous-fixture -- fixtures/agent.sqlite   # 打 /v1/sessions 同形 JSON
cargo build --release && timeout 5 ./target/release/... # 冷启动不 panic
```

fixture 来自线上那份 `agent.sqlite`（1.2 MB，`conversations` 189 行、`entries` 209 行）。它是**只读的**，永远不写回去。

---

## 七、台账

### `SOURCES.md`（仓库根，必须存在）

一行一个文件，四列：

```
| 本库路径 | 来源 | 上下游路径 | 改动 |
| crates/nous-ai/src/types.rs | pi-rust@v0.24.2 | crates/rpi-ai/src/types.rs | 原样 |
| crates/nous-chord/src/delta/apply.rs | pi@v1.5.2 | packages/chord/src/delta/apply.ts | 翻成 Rust；去掉 Promise |
| crates/nous-durable/src/harness/abort.rs | pi@v1.5.2 | packages/durable/src/harness/abort.ts | 原样 + 补中文注释 |
```

判据：每一行都能用 `git -C <来源> show <tag>:<路径>` 对上；对不上的行是错的，改台账或删代码。

### 必须存在的配套文件

- `wire.md` —— 宪法，**权威版本住在这里**。Nous 仓库根那份是只读镜像，改契约只改这一份。
- `SOURCES.md` —— 见上。
- `docs/storage.md` —— 本库自己的表结构、版本号、迁移链。
- `docs/spec-from-pi-durable.md` —— 从官方 `spec.md` 摘的 checklist，只写"我们要哪几条"。
- `docs/pitfalls-from-pi.md` —— 上游 `fix` 的判定记录，只追加，不改旧条目。

---

## 八、这一版的一句话交接

> **`nous-ai` / `nous-tools` / `nous-runtime` / `nous-telemetry` / `nous-wire` 是冻结的 pi-rust 拷贝；`nous-chord` 是 chord 的六个目录；`nous-durable` 是 durable 的语义自己写；env 的 daemon 是官方二进制。四类任务之外，先开 issue。**

---

## Agent skills

### Issue tracker

Issues 在本仓库的 GitHub Issues 中（JoeWest9966/Nous-Package），用 `gh` CLI 读写。See `docs/agents/issue-tracker.md`.

### Triage labels

五个标准 triage 角色标签：`needs-triage`、`needs-info`、`ready-for-agent`、`ready-for-human`、`wontfix`。See `docs/agents/triage-labels.md`.

### Domain docs

单上下文布局：仓库根 `GLOSSARY.md` + `docs/adr/`。See `docs/agents/domain.md`.

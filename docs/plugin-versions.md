# 插件版本线（按仓库痕迹推出来的，一次性考古 —— 票 `#93`）

> 这份文件是 `#93` 的**交付物之一**：14 个插件（`docs/plugins.md` 里进发行面的那 14 行）以及
> 其余 14 个带 `plugin.toml` 的 crate，各自的版本号是**从 git 痕迹机械推出来的**，不是凭感觉填的。
> 表由 `scripts/plugin-versions.py` 现算，`scripts/check-plugins.sh` 之外再跑一次
> `scripts/plugin-versions.py --check` 就能验「盘上的 `plugin.toml` 与考古结论一致」。

## 口径（三个硬问题都拍了）

**1. 起点选 `0.1.0`，不选 `0.0.1`。**
`0.0.x` 的语义是「还没有可用形状」，而这些 crate 第一次入库时就已经能跑、能过判据
（`nous-chord` 的 delta 核心、`nous-durable` 的九表行层、`nous-server` 的 axum 骨架……
每一条的第一句都是 `feat(...)` 加一组测试）。起点即已可用 → 起点 `0.1.0`。
`0.x` 阶段的 minor 允许带破坏性变更（规则 8 已允许），所以 `0.1.0` 起并不欠一个「1.0 之前都是实验」的交代。

**2. 机械算，不人工判读。**
`scripts/plugin-versions.py` 从 `git log --name-status -M40%` 现算，判据全部可复核：

| 档 | 机械判据（只有这两条会升 minor） |
|---|---|
| minor | ① 本插件 `plugin.toml` 的 `name` / `layer` / `crate` / `vendor` / `provides` 有**删改**（整份新增不算 —— 那是清单出生）；② `crates/Nous/Server/src/meta.rs` 的 `ROUTES` 里 `module: "<本模块>"` 的行有增删改（端点新增 / 删除 / 换路径换方法） |
| patch | 其余一切：实现、日志、性能、错误文案、README、测试 |

**已知低估**：「端点只改回包形状 / 改状态码」不体现在 `ROUTES` 的行上，机械判据抓不到，
落 patch 档。宁可低估 minor，也不许凭印象升档 —— 这条写在这里，是为了让下一个人知道
表里 minor 是**下界**，不是完整清单。

**3. `server/` 里还没搬进插件的 handler 提交，算。**
端点属于模块（`ROUTES` 就是「端点归属」的唯一真相）。所以在 handler 还住在 `server/src/*.rs`、
但端点已经按 `module: "<名字>"` 登记的那些提交，算这个插件的经历；起点也因此取
「本目录第一次入库」与「`ROUTES` 第一次登记本模块」两者中更早的那个。
搬家票（`#14` / `#75` 一族）往后记账照这个口径：**端点先于目录，不重复计**。

## 不算「插件经历」的东西

机械剔除，都有明确理由（脚本里就是这四条，别在评审时再争）：

- **纯 `R100` 搬家**：`git mv` 只改路径，`crates/{kernel,platform,app}` 那三层目录改名占了
  每模块十来条。算经历等于「改个目录名就涨十版」。
- **只改 `plugin.toml` 的 `version` 行**：`chore(release)` / `release:` 一族把 28 个
  `plugin.toml` 追平发行版号，一次提交改 28 行 `version`。那不是任何插件动过。
- **`chore(release)` / `release:` / `chore(plugins): … 版本追平` / `chore(#14)`**：同上，
  按标题剔除（这些提交里除了 `version` 行只剩快照文件）。
- **同标题同日期的重复提交**：`#56` 那件事被 rebase 成 `249d729` + `b297220` 两个 sha，
  同一份内容。只算一次。

## 表

`提交数` = 计入的提交总数（含起点那次）；`minor 档 / patch 档` = 起点之后按上表分的档；
`推出` = `0.(1 + minor).(patch 累加，minor 归零)`；`现填` = 生成这张表那一刻盘上的值
（`#93` 那次两边相等；之后各插件按规则 8 自己涨，这一列会旧，`--doc-check` 不校它）；
`起点提交` = 记 `0.1.0` 的那一次，点进去就是一段 `git log`。

**这张表是生成物**：`python3 scripts/plugin-versions.py --write-doc` 重写它，别手改
（`--doc-check` 会拦手改）。逐条 patch 档的提交没在表里列 —— 表只钉「每个数怎么来的」，
要看细节就跑 `python3 scripts/plugin-versions.py --json`，那里每个提交都带 sha / 分档 / 理由。

**自指的代价（务必知道）**：这套判据把「改动本身」也算进考古，所以**任何**动到插件的提交
都会让表里的 `推出` 涨一格 —— 包括只改这个脚本或这份文档的提交（它们改到了 `server` 的
`build.rs` / `meta.rs`，`server` 就得再 +patch）。做法是照规则 8 老实涨、再
`--write-doc` 一次、三样一起提交；不这么做 `verify.sh` 的 0d 段会红并指名是哪个模块。
这不是判据太严，是「版本线从痕迹推」的必然：想让它不涨，就得让考古面看不见那次改动，
那才是真的骗人。

| module | 提交数 | minor 档 | patch 档 | 推出 | 现填 | 起点提交 |
|---|---|---|---|---|---|---|
| `Element_Models` | 13 | 1 | 11 | 0.2.0 | 0.2.0 | `a335681e` 2026-11-24 |
| `Nous_Device` | 19 | 1 | 17 | 0.2.2 | 0.9.0 | `a1a1d508` 2026-11-20 |
| `Nous_Extend` | 19 | 1 | 17 | 0.2.1 | 0.8.0 | `82662470` 2026-11-17 |
| `Nous_Prompts` | 6 | 2 | 3 | 0.3.0 | 0.4.0 | `4b0f8348` 2026-11-20 |
| `Nous_Triggers` | 1 | 0 | 0 | 0.1.0 | 0.1.0 | `6d2a4abb` 2026-11-26 |
| `ai` | 12 | 1 | 10 | 0.2.1 | 0.2.1 | `acbf79a7` 2026-11-16 |
| `ai-types` | 5 | 1 | 3 | 0.2.0 | 0.2.0 | `82662470` 2026-11-17 |
| `archive` | 3 | 1 | 1 | 0.2.1 | 0.2.1 | `61b78ec1` 2026-11-25 |
| `chord` | 21 | 1 | 19 | 0.2.0 | 0.2.0 | `d49c111b` 2026-11-16 |
| `combos` | 9 | 1 | 7 | 0.2.7 | 0.2.7 | `9e109e3a` 2026-11-25 |
| `data-manager` | 14 | 1 | 12 | 0.2.0 | 0.4.0 | `7ebfe9a3` 2026-11-20 |
| `dispatcher` | 7 | 1 | 5 | 0.2.0 | 0.2.0 | `6abbf38a` 2026-11-21 |
| `documents` | 11 | 1 | 9 | 0.2.0 | 0.4.2 | `9002c5a5` 2026-11-18 |
| `durable` | 72 | 1 | 70 | 0.2.0 | 0.2.0 | `4b7edc13` 2026-11-16 |
| `elements` | 11 | 2 | 8 | 0.3.1 | 0.3.1 | `648846d8` 2026-11-23 |
| `env` | 14 | 2 | 11 | 0.3.3 | 0.3.3 | `46e196c2` 2026-11-16 |
| `env-ssh` | 8 | 1 | 6 | 0.2.0 | 0.2.0 | `5d2eb78f` 2026-11-20 |
| `ext-probe` | 9 | 1 | 7 | 0.2.1 | 0.2.1 | `82662470` 2026-11-17 |
| `ext-sdk` | 7 | 1 | 5 | 0.2.0 | 0.2.0 | `82662470` 2026-11-17 |
| `fixture` | 5 | 1 | 3 | 0.2.0 | 0.2.0 | `ef562e95` 2026-11-17 |
| `presets-admin` | 9 | 2 | 6 | 0.3.4 | 0.3.4 | `e24ece4e` 2026-11-22 |
| `repo` | 39 | 2 | 36 | 0.3.1 | 0.7.0 | `c49abb76` 2026-11-20 |
| `routes` | 9 | 1 | 7 | 0.2.2 | 0.3.0 | `cfd815c8` 2026-11-24 |
| `runtime` | 6 | 1 | 4 | 0.2.0 | 0.2.0 | `9ab7ab9e` 2026-11-17 |
| `server` | 244 | 1 | 242 | 0.2.42 | 0.3.28 | `dea670b0` 2026-11-17 |
| `sessions` | 14 | 1 | 12 | 0.2.2 | 0.7.0 | `f7e1ed54` 2026-11-18 |
| `skills` | 7 | 2 | 4 | 0.3.1 | 0.3.1 | `bd682e9d` 2026-11-20 |
| `store` | 14 | 1 | 12 | 0.2.0 | 0.2.0 | `249d729f` 2026-11-18 |
| `telemetry` | 5 | 1 | 3 | 0.2.0 | 0.2.0 | `203d5d99` 2026-11-16 |
| `tools` | 10 | 1 | 8 | 0.2.0 | 0.2.0 | `f3c5b4db` 2026-11-17 |
| `wire` | 5 | 1 | 3 | 0.2.0 | 0.2.0 | `c754e96b` 2026-11-16 |

**为什么是 28 行而票面说「14 个插件」**：`plugin.toml` 有 28 份（`docs/plugins.md` 里
`crate` 列不是 `(待建)` / `—` 的行），其中 14 个进发行面（`MANIFEST`），另外 14 个是
kernel / platform / dev 层的 crate。规则 8 管的是**所有** `plugin.toml`，`check-plugins.sh`
也逐个查，所以 28 份一起推 —— 只推 14 个会让另外 14 个继续冒充发行版号。

## 每个 minor 档的来处（可逐条复核）

只列升过 minor 的模块；没列出的模块在起点之后**没有任何一条**提交动过它的 `ROUTES` 行或清单身份。

| module | 升档的提交 |
|---|---|
| `device` | `9bef3cca` PUT /v1/default · `b969f4fc` POST /v1/catalog/refresh · `7ebfe9a3` capabilities 的归属 · `a096b7fb` 生成式 OpenAPI · `cfd815c8` GET /v1/machines · `5650ba41` readme 端点从 server 归到 device |
| `documents` | `0841e2ae` 盘面树端点族 · `f46dea65` 归档 tree.json 导入 · `705d1490` 从发行面下线（删端点） |
| `elements` | `51949cc8` 锁（locked 进 JSON + 端点判据） |
| `env` | `385129a7` GET /v1/fs/dirs |
| `ext` | `58bd05b3` 端点族名改硬 · `a8736d61` 解冻（reload） · `250f7507` 管理面 7 条 · `1985977f` 编辑骨架 4 条 · `61259789` 垃圾站 2 条 |
| `prompts` | `58bd05b3` 端点族名改硬 · `1985977f` 编辑骨架 5 条 |
| `repo` | `03f0dd38` 10 条 /v1/repos · `55b9494c` 变体 7 条 · `ea4041ac` 上游追踪 12 条 · `ee77955b` 归档区读面 |
| `server` | `c8735a6f` readme 端点出生 · `5650ba41` 归到 device（换归属 = 契约变） |
| `sessions` | `58bd05b3` 10 条 · `c678e45c` 置顶 · `06ec89ec` origins · `2066c52a` search · `2c43856a` selection 写入面 |
| `skills` | `58bd05b3` 端点族名改硬 · `d9c13edc` 管理面 4 条 |
| `data-manager` | `d8b48fe8` 六屏明细 5 条（当时叫 `stats`）· `f7aab5e` 改名 `data/data-manager` · 8 条端点归插件 |

## 改名链

`data-manager` 的前世是 `stats`（`crates/Nous/stats` → `crates/Data/DataManager`，2026-11-25）。
脚本按 `rename_chain` 顺着 `git mv` 的 R 记录往回找，所以它的提交数与 minor 档把 `stats` 时期
一起算上了：**同一个插件的版本线，不因改名重新起算**。

## `0.x` 的破坏性变更破了谁（规则 8 要求写清）

这 28 条版本线里，**唯一的破坏性变更是 `documents` 的 `#90`**（`705d1490`：documents 从
`MANIFEST` / `ROUTES` / `capabilities` / 依赖里整体下线，17 行端点声明删掉）。破的是：

- **NousDesk**：`/v1/documents/*` 一族在客户端已无调用点（`wire.md` 记着「本服务不注册任何
  `/v1/documents/*`」），但客户端「关于 → 插件」列表会**少一行** `documents`。
- **旧面板的调用方**：已随 2026-11-18 下线，无活体。

`device` / `sessions` / `ext` 那些 minor 都是**加法**（新增端点、端点换归属），不破谁。

## 判据

```bash
python3 scripts/plugin-versions.py --check      # 版本不得低于考古结论（checked=28 mismatched=0）
python3 scripts/plugin-versions.py --doc-check  # 文档里的表 == 现算的考古结论
./scripts/check-plugins.sh                      # 合法 semver + 只增不减
cargo test -p nous-server --test plugin_versions   # capabilities 报的版本 == 各自 plugin.toml
```

`plugin-versions.py --check` 的语义是**不得低于考古结论**（`<`，不是 `!=`）：
`#93` 之后按规则 8 自己涨（高于）是正常的 —— 下一张票改了 `ext` 就得 +patch；
低于 = 有人把版本手改回去了，或者插件目录改名导致考古面变了，两种都要人看一眼。
`--doc-check` 才校「文档里的表 == 现算的考古结论」（表是生成物，别手抄）。
换句话说：**考古结论是快照，之后的演进按规则 8 自己涨。**

## 与 `/v1/capabilities` 的关系

规则 8 解绑之后，`/v1/capabilities` 的 `modules[].version` 必须报**插件自己的**版本，
否则 14 个模块在桌面端「关于 → 插件」里永远显示同一个数，这次考古在界面上等于没发生。
落法：`crates/Nous/Server/build.rs` 读 `crates/*/*/plugin.toml` 生成 `模块名 → version`
静态表（`include!` 进 `src/plugin_versions.rs`），`meta.rs` 从表里取；取不到才回退发行版号，
并加 `versionSource: "release"` 如实说明（回退不静默）。
`build.rs` 对每个 `plugin.toml` 和 `crates/` 两层的目录都挂了 `cargo:rerun-if-changed` ——
少了这条，改版本不重编 = 假绿。

## 票面边界与这次实际改动的对应

票面写「不许夹带代码改动：这张票只动 `version` 字段 + 票面表格」。实际提交里的每一处
非 `version` 改动都能指到票面某一句，列在这里免得评审时一条条问：

| 改动 | 票面依据 |
|---|---|
| 28 份 `plugin.toml` 的 `version` | 「一次性改 14 个 `plugin.toml` 的 `version`，**同一提交**里改」（本仓 `plugin.toml` 实为 28 份，见下） |
| `server/build.rs` + `src/plugin_versions.rs` | 补充评论：「`build.rs` 读 `crates/*/*/plugin.toml`，生成 `模块名 → version` 的静态表」 |
| `server/src/meta.rs` 的 capabilities 版本来源 | 补充评论：「`meta.rs` 从表里取，取不到才回退 `CARGO_PKG_VERSION` 并**在 capabilities 里如实说明**」 |
| `server/src/plugin_docs.rs` 的 `version` | 同一句话的延伸：`readme` 的 `version` 也是「文档所属的版本」，两处报不同的数就是又一份事实源 |
| `server/tests/plugin_versions.rs` | 补充评论的两条判据：「遍历 `docs/plugins.md` 的行，别点名模块」+「故意改高一个 patch，`cargo build` 后 capabilities 跟着变」 |
| `scripts/plugin-versions.py` + `scripts/verify.sh` 0d 段 | 判据「`scripts/check-plugins.sh` → `checked=28 mismatched=0`」与「`scripts/verify.sh` 全绿」要能复算，不然表是手抄的 |
| `docs/plugin-versions.md` | 「一张表放票面：`module \| 提交数 \| minor 档 \| patch 档 \| 推出 \| 现填`」—— 放在仓里（票面写不下 28 行且会漂） |
| `docs/decisions.md` 新条目 | 规则 8：「`0.x` 阶段允许 minor 带破坏性变更，但**必须在票面与 `decisions.md` 里写出破的是谁**」 |
| `docs/plugins.md` 规则 8 改写 | 原文写着「当前所有插件都停在 `0.1.26` 只是起点」—— 这张票做完这句就是假话 |
| `wire.md` 的 capabilities / readme 段 | `wire.md` 是契约的唯一真相；加了 `versionSource` 字段不写进去，下一个客户端就会当它是噪音 |
| `.gitignore` 的 `__pycache__/` | 新增了 `scripts/*.py`，不带这行每次跑判据都留 `??` 噪声 |
| `.github/workflows/verify.yml` 的 `fetch-depth: 0` | 版本线要从完整历史推；CI 是浅克隆，不加这条判据在 CI 上是假绿 |

**为什么是 28 而不是 14**：票面说「14 个插件」，指的是 `MANIFEST` 里进发行面的那 14 个；
但 `plugin.toml` 有 28 份（kernel / platform / dev 层的 crate 也各带一份），规则 8 管的是
**所有** `plugin.toml`，`check-plugins.sh` 也逐个查「合法 semver + 只增不减」。只推 14 个
会让另外 14 个继续冒充发行版号 —— 那正是这张票要消灭的东西。判据 `checked=28` 也是这么来的。

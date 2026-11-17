# Nous 内核 spec（骨架 v1）

**状态**：九章全部落笔，无未决项。已拍板条目见附录 A（31 条）；交接见 `docs/spec/HANDOFF.md`。

**本版三处方向变更**（写进正文，不是附录）：面板已死、`Nous-Package` 并入本仓、连接方案 = 三条可判定要求。

## 读法

- **形状听 `wire.md`，语义听本 spec。** 一份契约，住本仓根。
- 目录按 **`crates/<vendor>/<module>`** 管**所有权**，分层写在 `docs/plugins.md` 的 `layer` 列（§3.0）。**目录不是仓库边界，不是第二个部署单元；层不再是路径。**
- 术语以 `GLOSSARY.md` 为准。本 spec 只加词，不改已定义的词。
- 每章三样齐：**判据**（红/绿明确）/ **反悔点** / **不许做什么**。
- **判据不许空跑绿。** 任何判据引用的路径不存在时，结果是 **SKIP（打印出来）**，不是 PASS。`git grep -n "x" -- <不存在的目录>` 会返回 rc=0、无输出——那就是 `0 ≥ 0` 的假绿，明令禁止。每条判据都以这段守门开头（**不认 vendor 名，不写死目录**）：

  ```bash
  test -e docs/plugins.md         || { echo "RED: docs/plugins.md 缺失（清单是判据的一部分）"; exit 1; }
  test -x scripts/layer-paths.sh  || { echo "RED: scripts/layer-paths.sh 缺失"; exit 1; }
  K=$(scripts/layer-paths.sh kernel); A=$(scripts/layer-paths.sh app)
  ls -d crates/*/*/ >/dev/null 2>&1 || { echo "SKIP: crates/*/* 一个都没有，等 #14 搬迁"; exit 0; }
  test -n "$K"                    || { echo "SKIP: K 为空（清单里没有 layer=kernel 的行），等 #14 搬迁"; exit 0; }
  ```

  **三段退化语义写死**：

  ```
  docs/plugins.md 缺失 / 不可解析   → RED（清单是判据的一部分，缺了没法判）
  scripts/layer-paths.sh 缺失       → RED（同上）
  K 为空 或 crates/*/* 一个都没有    → SKIP（打印），因为 #14 还没搬
  ```

  只允许"打印出 SKIP 然后退出"，不许静默 `exit 0`；**配置缺失不算 SKIP**——把缺配当 SKIP 就是"路径不存在当 PASS"的第二季。判据跑完必须能在输出里数出红/绿/SKIP 各多少条。

## 章目

| # | 章 | 一句话结论 | 状态 |
|---|---|---|---|
| 1 | 边界与仲裁 | 一个仓、一个 workspace、一个产物；内核层可换产品复用，应用层持有客户端契约 | 已写 |
| 2 | 真相单位 | 真相是单库全局单调的**提交**；会话是命名空间 + 配置文档；流是可丢掉重算的投影 | 已写 |
| — | 契约形状 | `wire.md` 只是端点索引；字段形状的权威在测试里；冲突时**测试赢** | 已写 |
| 3 | 客户端面与部署 | 只有一个客户端（NousDesk，core/shell 分层）；`resolve` 只发 `{url, version}`、只认长期 token header；一个 systemd 单元、musl 静态、跑在 j1900；断线续传 + 全幂等 + 客户端零状态；**置顶进内核是阻塞项** | **已写** |
| 4 | 接缝 | 扩展是进程外应答者，四件能力，状态可丢弃，失败分三种；判据是"接入第二个工具时内核零改动" | 已写 |
| 5 | 取消语义 | 三个入口一条通道；取消必须落一条提交；终态一个字段五个值（含 `abandoned`）；幂等 | **已写** |
| 6 | 并发模型 | 一个进程、一个库 ⇒ 单写者成立；任务在应用层，内核层不 spawn、不碰时钟、不无限缓冲 | 已写 |
| 7 | 持久化与崩溃恢复 | 能丢的只有 ≤100ms 的 partial；提交、id 分配器、`commit_seq`、终态一律不能丢 | 已写 |
| 8 | 迁移 | `qrd3wh` 先只读上线、再切写；回滚只承诺回到第二段开始那一刻 | 已写 |
| 9 | 不做清单（v1） | 六组禁止 + 收口顺序 | 已写 |

---

## 1 · 边界与仲裁

**结论**：一个仓（`Nous`）、一个 workspace、一个产物（`nous-app` 进一个 systemd 单元）。分层写在 `docs/plugins.md` 的 `layer` 列，目录按 `crates/<vendor>/<module>` 管所有权（§3.0）——**路径不再表示层**。

**判据（15 条，全进 `scripts/verify.sh`；编号 1–13 里含 6b / 7b 两条插入项，实为 15 条）**

本层判据里的 `$K` / `$A` 是**路径选择子**，不是目录名：`K=$(scripts/layer-paths.sh kernel)`、`A=$(scripts/layer-paths.sh app)`（定义见 §3.0）。契约：**载体不存在 = SKIP（打印出来），不是 PASS；清单或选择子脚本缺失 = RED。** 下面 15 条每条都以这段守门开头：

```bash
test -e docs/plugins.md        || { echo "RED: docs/plugins.md 缺失（清单是判据的一部分）"; exit 1; }
test -x scripts/layer-paths.sh || { echo "RED: scripts/layer-paths.sh 缺失"; exit 1; }
K=$(scripts/layer-paths.sh kernel); A=$(scripts/layer-paths.sh app)
ls -d crates/*/*/ >/dev/null 2>&1 || { echo "SKIP: crates/*/* 一个都没有，等 #14 搬迁"; exit 0; }
test -n "$K"                   || { echo "SKIP: K 为空（清单里没有 layer=kernel 的行），等 #14 搬迁"; exit 0; }
```

三段退化语义：清单缺失/不可解析 → **RED**；选择子脚本缺失 → **RED**；`K` 为空或 `crates/*/*` 一个都没有 → **SKIP（打印）**。

- 覆盖范围：以 `git grep … -- $K` / `-- $A` 为载体的（3、4、5、6、8、11、12、13）与 `cargo tree` / `cargo metadata` 类（1、2、9、10）。
- **#14 的验收里必须包含"这 15 条从 RED/SKIP 变真绿"**：#14 第 10 步（清单 + 选择子脚本）之前一律 **RED**（缺配），之后仍 SKIP 即红、仍 RED 即未交付。

**边界基线只有一份，在代码仓**：`docs/boundary-baseline.txt`，由 #14 落定；改它（包括换成员）需要签字。本 spec 仓**不留副本**——两份同义文件 = 第二份真相（代码仓 `docs/spec/` 那 7 份副本也在被删，同一个道理）。判据 3 与判据 6 读的都是**代码仓那一份**（`verify.sh` 里 `BASE=docs/boundary-baseline.txt`）。

1. 正向依赖：`cargo tree -p nous-app --depth 1 -e normal` 含内核层的 `durable` / `wire` / `tools` / `runtime`，不含 `chord` / `rusqlite`。
2. 反向依赖：内核层任一 crate `--depth 1` 都不依赖应用层。
3. 词表基线：`git grep -nE '\b(skill|preset|prompt|template|board|block|resolve)\b' -- $K` 与**代码仓的** `docs/boundary-baseline.txt` 逐行比对，**新增一行即红**；基线每行带一句"为什么留着"；改基线单独一张票；被绕过两次换整文件哈希。**该文件首行写死「本文件只减不增」**（S8）。
4. 路由归属：`git grep -nE '\.route\(|axum::|tower::' -- $K` 为空。
5. 数据面唯一入口：`$K` 之外不许出现 `rusqlite` / `Connection::open` / `.sqlite` / `PRAGMA` / `BEGIN IMMEDIATE`。
6. 进程归应用层：`$K` 里 `std::process|tokio::process|Command::new|Child` **逐行输出必须为空**（不是计数），排除 `ExitCode` 与 `tests/`；命中即红、把行贴出来。**内核不认识"进程"。**
   全仓的进程创建点按**文件**数，不按入口数。**只数构造点，不数方法调用**（Rust 里起进程必须先有 `Command`，`.spawn()` / `.output()` / `.status()` 都是它的方法，数方法必漏）。生成命令（逐字执行，`verify.sh` 第 5 段就是它）：

   ```bash
   grep -rlE 'Command::new\(|::fork\(|posix_spawn\(|execve\(' crates --include=*.rs | sort
   ```

   **判定口径变更（2026-11-17）**：从"数方法调用"（`\.spawn\(`）改成"数构造点"（`Command::new` / `fork` / `posix_spawn` / `execve`）。理由：`os_env.rs:443` 是 `cmd.status().await`，**它也在创建进程**，`\.spawn\(` 与 `\.spawn\(\)` 都数不到；收紧到 `\.spawn\(\)` 会连 `.output()` / `.status()` 一起漏。数字与成员不变（仍 2 个），**不需要重新签字**。

   结果**逐行相等**于代码仓 `docs/boundary-baseline.txt` 的 `spawn_files=N` + 紧跟的成员清单（今天 **N=2**：`crates/nous/tools/src/os_env.rs`、`crates/nous/ext/src/spawn.rs`）。
   **排除项（只有三组，判据从集合里减掉；不许只写"排除 tests/"就完，也不许点名目录）**：
   - `tests/`、`*_test.rs`、`*_tests.rs`、`*tests.rs` —— 测试夹具（今天命中 `crates/nous/runtime/src/provider/tests.rs`）。**排除式写 `(/tests/|tests?\.rs$)`**：`_tests?\.rs$` 抓不到 `provider/tests.rs`（没有下划线前缀），这是锚点选错的经典复发；
   - `#[cfg(windows)]` 块 —— Linux 不编。**今天命中 0 个文件**（`output_decode.rs` 里那个 `chcp` 构造点已在 #14 移除）；规则留着，再出现 windows-only 构造点时自动不算；
   - **减 `layer = dev`**（`fixture` + `ext-probe`）：用 §3.0 的 `$D` 写法，**不点名目录**。`dev` 集合不是恰好那两个 → **红**（见 §3.0 正集哨兵，否则这条排除静默失效）。

   已知欠账（**不设判据**，只记在 baseline 注释里）：`os_env.rs` 一文件 3 个入口（363 / 412 / 915）；`ext/src/spawn.rs` 一文件 1 个入口（90 / 127）。口号仍是"其余工具一律经它"，但 `os_env.rs` 今天就是那个它本身。
   **成员变更要签字**："只减不增"管的是**成员**，不只是数字——`open_browser.rs` → `ext/src/spawn.rs` 换人时数字仍为 2，**换成员同样要签字**（数字相等不等于没变，这就是哨兵原则）。
6b. 依赖方向：`cargo tree -p nous-durable --depth 2 | grep providers` 为空 —— 真相层不许依赖**会 sleep 的那个 crate**，不是"不许依赖 `nous-ai`"（拆完 `types` 允许被依赖，S2）。
7. **端点索引唯一**：`git grep -l '^### GET /v1/'` 恰好 1 个文件（`wire.md`）。它只是索引——端点、用途、谁在用；**字段形状的权威在 `crates/*/*/tests/` 与 conformance 里**，两者冲突时**测试赢**。
7b. **索引不抄字段**：`wire.md` 里出现 `"type":` 或 camelCase 字段名清单的段落数 = 0（抄一遍就会漂）。
8. 代码不引文档：`git grep -n 'wire.md' -- $K` **去掉注释行后**为空（测试引形状常量，不引文档路径；注释里提一句不算，S8）。
9. 一个 workspace 根：全仓只有一个根 `Cargo.toml`；**进 systemd 的 bin target 恰好 1 个**（**减 `layer = dev`**：`fixture` 与 `ext-probe` 都不进 systemd；S4；不点名 crate）。
10. 旧仓清空：`git grep -c 'Nous-Package'` 在代码与文档里为 0（只许出现在归档 README 与历史提交里）。
11. 发现唯一：`grep -rn 'NOUS_SERVER_ADDR' -- $A | grep -vE "^($D)"` 为空（只许测试用 = 只许 `dev` 层；生产配置出现即红）。
12. 进程只有一处（应用层）：与判据 6 **同一个 pattern** + **同一份排除项**，`grep -rlE 'Command::new\(|::fork\(|posix_spawn\(|execve\(' -- $A | grep -vE "(/tests/|tests?\.rs$|^($D))"` 的文件集合**恰好 1 个**，且必须等于 `crates/nous/ext/src/spawn.rs`（扩展监管）。**`ext-probe` 归 `dev`，按层减掉**——不按层减就会数出 4 个。
13. 无文件锁：`git grep -nE 'flock|fs2|File::lock|\.lock\(\)' -- $A` 为空（单进程不需要文件锁）。

**反悔点**：出现"纯内核"部署形态（没有应用层、只跑 6 条路由）→ 契约拆两份。
**不许做**：不许用"反正改基线就行"绕过判据 3；不许为应用层的查询需求给内核层加列——应用层自己建投影表。

**两条出路（写死）**

- 内核层只原样存、原样取、按 `wire.md` 出形状，**不解释** `skills` 里那串名字是什么意思。要按 skills 查，应用层自己建投影表。
- `nous-ai/src/transform/system.rs` 只认"一串 system 消息"，**不认段名**（`section` / `cwd` / `skills` / `persona` 只许出现在注释里）。

---

## 2 · 真相单位

**结论**：提交不可分；会话是可寻址的身份与命名空间；其余全是投影。

**判据**

- entry 不是原子单位：一次提交可含多条 entry（`entries.commit_seq` 相等即同一次提交），`commit_seq` 单库全局单调（fixture 实测 3…5073，跨会话不分局）。
- 投影可丢可重算：清掉任何客户端本地状态与 SSE 状态，重连后列表树与 UI 与清掉前一致。
- 会话可空名：`name: ""` = "从未被命名"，任何写入路径都不把它改成占位名。

**反悔点**：出现必须**跨会话原子**的动作 → 会话成为事务边界，本章塌。
**不许做**：不许把 `?since=` 之外的内部游标写进契约；不许客户端生成默认名并写回；不许客户端存第二份真相（置顶、未读、选中一律现算或从提交日志重放）。

---

## 3 · 客户端面与部署

**结论**：客户端**现在只有一个**：NousDesk。移动端将来**复用 NousDesk 的 core**，不另建客户端；`bjes6b` 删除。客户端只看 `resolve` 给出的那一个 `/v1` 面，不链接库。内核层与应用层是**一个** systemd 单元、一个 musl 静态二进制，跑在 j1900。

### 3.0 目录：按 vendor/module（分层不在路径上）

对标 Magento 2 的 `app/code/<Vendor>/<Module>`：**目录管所有权，清单管分层。**

```
crates/nous/durable  chord  wire  env  tools  runtime
             ai-types  ai-providers  sessions  ext-host  server  telemetry
crates/acme/            ← 将来第三方（现在空）
```

- **crate 名不改**：`crates/nous/tools` 的包名仍是 `nous-tools`，`crates/nous/durable` 仍是 `nous-durable`。目录名与包名不必一致（目录管位置，包名管引用）。
- `layer` 四个值：`kernel` / `platform` / `app` / **`dev`**。**`dev` = 测试夹具与探针**（今天正好两个：`fixture`、`ext-probe`）：**不进 systemd、不进发布包**。判据里凡要"排除测试用的 crate"，一律写成**减 `dev` 层**，**不许点名目录**——点名是把"清单没分层对"的症状抄进判据（判据 6 / 9 / 11 / 12 各抄过一遍，已收回）。
- **正集哨兵**：`scripts/layer-paths.sh dev` 的模块集合**必须恰好是** `fixture` 与 `ext-probe`（集合相等）。`dev` 一旦为空，所有"减 dev"的排除会**静默失效**——所以这条是哨兵，不是提醒。
- 减 `dev` 的写法（判据里唯一允许的形式，不出现目录名）：

  ```bash
  test -n "$(scripts/layer-paths.sh dev)" || { echo "RED: layer=dev 为空（排除会静默失效）"; exit 1; }
  D=$(scripts/layer-paths.sh dev | tr ' ' '\n' | sed 's|$|/|' | paste -sd'|')
  # … | grep -vE "^($D)"
  ```

  **两种退化形态都必须先红再算**：`dev` 为空时脚本会打出空行，`D` 退化成 `/`（排除空转）或 `""`（`^()` 把集合清空）——两种都会让"排除"变成假绿/假红，所以第一行守卫不能省。

- 三层（kernel / platform / app）**不体现在路径上**，体现在 `docs/plugins.md` 的 `layer` 列 + 一条依赖判据：`layer = kernel` 的 crate，依赖树里不许出现 `layer = platform` 的 crate（`cargo metadata` 可查）。
- 判据里的路径选择子只有一个，不硬编码目录名：

  ```bash
  K=$(scripts/layer-paths.sh kernel)   # 输出 layer=kernel 的模块路径，空格分隔
  A=$(scripts/layer-paths.sh app)
  ```

  `git grep … -- $K` 就是本条 spec 里的"内核层"。`scripts/layer-paths.sh` 的成员**只从 `docs/plugins.md` 读**，脚本里不写死模块名。

**清单格式（列名定死）**：

```
| module | vendor | crate | layer | 形态 | API 面 | 必需 | 备注 |
```

- `crate` 列三种写法：`nous-durable`（已存在）/ `(待建) nous-sessions`（目录不存在**不算红**）/ `—`（不是 crate）。
- `必需=是` 的模块缺失 → 发行版**拒绝启动并指名缺谁**（fail-closed）。
- 清单与目录**双向相等**，但只算**非 `(待建)` 行**：目录在、清单没写 = 红；清单写了、目录不在 = 红（`(待建)` 行豁免）。

**三段退化语义（与读法一致，不得两边漂移）**：

```
docs/plugins.md 缺失 / 不可解析   → RED（清单是判据的一部分，缺了没法判）
scripts/layer-paths.sh 缺失       → RED（同上）
K 为空 或 crates/*/* 一个都没有    → SKIP（打印），因为 #14 还没搬
```

三条边界：

1. 模块内部**不**照搬 PHP 的 `Api/ Model/ etc/ Controller/`。Rust 用 `lib.rs` 只 re-export `api`；`Model/` 的等效物是 `src/` 里的内部模块一律 `pub(crate)`。
2. `vendor` 不许变成第二个分层：**不许**出现 `crates/nous-core`、`crates/nous-app` 这种按层起的目录名。
3. 第三方**不进源码树**，走 Cargo 依赖；`crates/acme/` 只放测试用的模拟第三方。

**判据**：`ls -d crates/*/*/` 的模块集合 == `docs/plugins.md` 里**非 `(待建)` 行**的模块集合（双向，逐行 `tee`）；`crates/` 下不存在名为 `core` / `app` / `kernel` / `platform` 的目录（那四个词是层名，不是路径）；`cargo metadata` 里 `layer = kernel` 的 crate 的依赖闭包不含 `layer = platform` 的 crate；`必需=是` 的模块在 `K`/`A` 里（缺失即发行版拒启）。

**反悔点**：`crates/nous/` 一级目录数超过 ~15、模块拆分明显失控 → 允许加一级 module group（`crates/nous/ai/types`），但 `layer` 列仍是唯一分层来源，路径仍不代表层。

**不许做**：不许把层写进路径；不许在 `vendor` 下再分一次层；不许让第三方 crate 出现在源码树里（`crates/acme/` 除外）；不许为了"目录好看"改包名。

### 3.1 客户端只剩一个（面板死、手机端也死）

- 面板前端**废**；手机端 `bjes6b` **删**，移动端将来**基于 NousDesk 做**。功能吃不掉的就是不存在的功能。
- 因此客户端只有一个：**NousDesk**。它内部必须分层，否则将来移动端只能复制粘贴：
  - `core`：协议客户端、会话模型、状态重算，**无 UI 依赖**
  - `shell`：GPUI 渲染、输入法、窗口
- 判据：`grep -rn 'gpui' NousDesk/crates/nous-engine` 为空（核心层真名是 `nous-engine`；写 `crates/core` = 路径不存在 = 假绿）；`nous-engine` 里不出现 `/p/<board>/`（只认 `resolve` + `/v1`）。
- 反悔点：移动端若最终必须用系统 WebView 而不是 core + 原生 shell，**ticket 机制要加回来**（形状照 §3.4 的反悔点）。
- 不许做：不许复制粘贴 shell 代码做第二个客户端；不许为了"将来移动端"在 core 里塞平台分支。

### 3.2 端点重判表（事实：`NousDesk@v0.9.5`；手机端已删）

| 端点 | NousDesk | 处置 |
|---|---|---|
| `/v1/sessions`（树）· `/:id/entries` · `/:id/messages` · `/:id/events` · `/:id/abort` · `/:id/fork` · `/:id/rename` · `/:id/archive` · `/:id/purge` · `/v1/archive` · `/v1/archive/:id/restore` | 用 | 留 |
| `/v1/sessions/:id/config` | 用 | 留 |
| `/v1/selection`（GET/POST） | 用（`nous-engine/src/client.rs:628`、`:633`） | 留 |
| `/v1/device` | 用 | 留 |
| `/v1/unread` · `/v1/read/:id` | 用 | 留 |
| `/v1/models` · `/v1/skills`（`/file` `/files` `/create` `/publish`）· `/v1/prompts`（`/file`）· `/v1/presets/:section` · `/v1/machines` · `/v1/fs/dirs` | 用 | 留 |
| `/v1/combos` · `/v1/combos/:name/enable` | 用 | **留，且必须补进 `wire.md`**（现在契约里没有） |
| `/v1/triggers` · `/v1/triggers/sync` | 用 | **留，且必须补进 `wire.md`** |
| `/v1/extensions/file` | 用 | **留，且必须补进 `wire.md`** |
| `/v1/{kind}/file` · `/{kind}/files` · `/{kind}/create` · `/{kind}/publish` · `/{kind}/{op}` | 用 | **留，且必须补进 `wire.md`** |
| `POST /v1/sessions/:id/pin` | **待实现**（现在写穿到 `bjes6b`） | 新增，见 §3.7 |

**结论：无人用 = 0 条，一条都不删。** 面板与手机端死掉没有释放任何端点——`NousDesk` 已经在读它们全部。反过来，契约缺 5 个家族。

**判据是成对的两条，缺一条不许过**（匹配前先把字面量归一化：去前导斜杠、按**路径段**匹配（`v1/selection` 任一段），**只算非注释命中**）：

- **(a) 无主端点必删**：`wire.md` 每个端点标题，在 `NousDesk`（含 `crates/core`）源码里归一化后至少命中一次（去注释）；命中 0 → 同一张票里删端点。
- **(b) 无契约调用必红**：`NousDesk` 里出现的每个 `/v1/…` 字面量，必须在 `wire.md` 里有对应标题；没有 → 要么补契约，要么这个调用是私有后门，删掉。

**补那 5 个家族的顺序不许反过来：先写快照测试（真实请求 + 真实响应），再把标题加进 `wire.md`。**

`/v1/machines` 另有两条（S10 / S11）：① 机器编目（机器 + 注解）**从块 `vq0f03` 搬进应用层**，搬完才允许删那个块；② 增补能力字段（os / arch / triple / rustc / 角色 / 兼容 target 列表），顺序同上——先快照测试，再进 `wire.md`。

**反悔点**：出现"这条端点给将来的某个客户端留着"→ 直接删；真需要时再加回。
**不许做**：不许按已死的面板/手机端的需求保留端点；不许在 `wire.md` 写字段清单；不许为了过判据 (a) 往客户端塞一个无人调用的函数。

### 3.4 地址发现与鉴权（发现不发钱）

**拆掉那个循环**：发现端点如果同时发凭证，它自己就得鉴权；而鉴权又需要先发现——结果只能把 `resolve` 做成不需鉴权的发钱机。

- `resolve` 响应**必须含** `url` 与 `version`，**不含任何秘密**；未鉴权访问也 200。
- `id` / `name` / `path` 是**块时代的遗留，标 deprecated**：客户端不许再用它们做业务判断；等没人读就删键。
- `version` 是**协议版本**，不是应用版本；客户端遇到未知 version **拒绝连接并报出双方版本**，不许猜着跑。
- **只有长期 token 走 header。`POST /v1/auth/ticket` 与那张一次性表整个删掉**——`?token=` 的唯一成因是"浏览器 `EventSource` 不能带 header"，而 WebView 不在 v1 里了。

判据（快照）：
1. 响应体含 `url`、`version`；`grep -iE 'token|secret|key|ticket'` 在 `resolve` 响应上命中 0。
2. `NousDesk` 对 resolve 响应**只取 `url`**（计数 1）。
3. 未知 `version` → 拒连，并报双方版本。
4. 日志里没有长期 token；`mv` 端口只改一处，客户端零改动即通。

**反悔点**：移动端若用系统 WebView，加回 `POST /v1/auth/ticket`（≤60s、用即废、绑客户端），长期 token 仍只走 header。
**不许做**：`resolve` 发 token；`NOUS_SERVER_ADDR` 进生产配置；为某个客户端造降级鉴权。

### 3.4b 中间层随手机端一起消失

`bjes6b` 删掉之后，那 80 行 BFF（`/api/*` → `/v1/*`）没有存在理由，**也不搬进 Rust**。翻译层存在的每一天，契约真相都在分裂。

- 判据：`git grep -c 'bjes6b'` 的系统引用 = 0；`NousDesk` 里 `/api/` 命中 0。
- **前置阻塞**：置顶必须在删除之前进内核，见 §3.7。

### 3.5 部署与升级

- **编译是带 target 的任务，派给"能力匹配且空闲"的机器**（S9）。每台机器登记 os / arch / target triple / rustc 版本 / 角色（`runtime` / `build` / 两者）+ **宿主兼容 target 列表**；运行时宿主**默认不收构建任务**（调度规则，不是物理限制；j1900 实测装有 `rustc 1.99.1`）。产物按 **target triple + 内容哈希**命名，装载前校验，不符即拒载（fail-closed，不重试不降级）。
  判据：`sha256sum` 一致 + `ldd` 无动态依赖 + `systemctl restart` 后 `curl -s localhost:<port>/v1/device` 200；**运行时宿主进程树里不出现 `cargo`/`rustc`**（除开 build 角色）；产物 triple 在宿主**兼容列表**里（不是相等：musl 静态产物在 gnu 宿主上能跑，反过来不行）。
- 一个单元：`nous-app` 是唯一进程，内核层是它的库。不搞两个 service。
- 升级：`systemctl restart`（亚秒级），不做热升级、不做工号传递。
- 资源硬顶，两类分开返回：连接数撞顶 → **503 + Retry-After**（可重试）；单文档/单库撞顶 → **413 / 507**（不可重试）。
  数值：全局 SSE 64 / 单会话 `pi.live` 4MB / 单库 200MB。
  判据不是"返回了码"，是**拒绝发生在分配之前**：撞顶时进程 RSS 不涨（涨了说明先分配再检查，上限是假的）。

**反悔点**：出现"长会话不能在重启时断"→ 才谈优雅停机与工号传递；出现必须双向或多路复用 → 在 `/v1` 上加通道，**仍然不是链接库**。
**不许做**：不许给任何客户端开链接库（绕过 `requestId` 幂等与 `whenBusy`，造出第二个真相入口）；不许面板复活；不许第二个监听。

---

## 3.6 · 块清算（Q26 的前置）

**结论**：**块不再是进程，也没有"崩"这回事。** 块分三类：并成路由/特性、删、真需隔离的叫扩展。

- 进程：进程创建点只有一处，且**只服务扩展**（判据 12 的同一个 pattern，文件集合恰好 1 个 = `crates/nous/ext/src/spawn.rs`）。"块崩溃只死自己"的监管逻辑**不用写**。
- content store：文件锁锁的是**多进程写同一份 `tree.json`**；块进程一消失，锁就没对手了，同一进程内 `Mutex<()>` 足够。**不许把文件锁移植进 Rust**——那是把多进程时代的问题带进单进程架构，还会在 j1900 上变成"CLI 一读就卡住"的新坑。

**判据**

1. 进程创建点（构造点 pattern，见判据 12）在应用层的文件集合恰好 1 个，且 == `crates/nous/ext/src/spawn.rs`。
2. `git grep -nE 'flock|fs2|File::lock|\.lock\(\)' -- $A` 为空。
3. `resolve` 由 nous-app 自己发，响应体 `url` 指向 nous-app 自己的监听。
4. 停掉 `nous-panel.service` 之后，NousDesk 冷启动仍能连上（第 8 章之前跑一次）。

**块清单（待确认后才能动壳；形态列实测：`package.json` 的 `scripts.start` / `public/`）**

| 块 | 是什么 | 形态 | 现在的读者 | 处置 |
|---|---|---|---|---|
| `qrd3wh` | session 引擎 | node + 服务 | 桌面端直连 | **被 nous-app 取代** |
| `bjes6b` | 手机端页 + 80 行 BFF | node + `public/` | 手机浏览器 | **并成 nous-app 的特性 + 静态页**，不再独立 4800 |
| `h58ga5` | NousDesk 文件镜像（无 `.git`） | 只有文件 | 面板文档页 | **删**（代码在 `/opt/NousDesk`） |
| `3699b9` `6ysg01` `jtjzw5` | 会话/终极/预设模板页 | node + `public/` | 面板前端 | **删**（NousDesk 已有原生预设 UI） |
| `68kazk` | skills 清单页 | node + `public/` | 面板前端 | **删**（NousDesk 已有 `/v1/skills` UI） |
| `k3skws` | 元素库页 | node + `public/` | 面板前端 | **删**（NousDesk 已有 `{kind}/*` UI） |
| `vbe4pt` | 扩展清单页 | node + `public/` | 面板前端 | **删**（NousDesk 已有 `/v1/extensions/file`） |
| `n8qkkj` | trigger 页 | node + `public/` | 面板前端 | **删**（NousDesk 已有 `/v1/triggers`） |
| `k5411g` | 模型编目页 | node + `public/` | 面板前端 | **删**（NousDesk 已有 `/v1/models`） |
| `d6s4v1` | storage X 光机 | node + `public/` | 面板前端 | **删**（诊断工具，不属于产品） |
| `zefsvf` | tailnet 面板 | node + `public/` | 运维本人 | **删**（运维不属于 Nous） |
| `vq0f03` | 机器编目（`/api/catalog`） | node + `public/` | `qrd3wh` 的 `/v1/machines`（缓存 60s） | **先搬后删**：编目搬进应用层 `/v1/machines`（S10），搬完才许删（S11） |
| `cmqt66` | 块自带 agent 后端（一会话 + 四工具 + SSE） | node | 无 | **删**（被 nous-app + 扩展取代） |
| `mcfpqm` `pxbgng` `hb7k3j` `7awa4h` | 归档库 / 对话列表 / 发起页 / 鱼骨（内容文档） | 只有文件 | 面板文档页 | **删**，需要的设计摘进 `docs/` |
| `4qpzmw` `8ven4x` `n45` `p34pgp` `s9hzsp` `vh79pm` | 空/无标题 | 只有文件 | 无 | **删**前先看一眼（可能藏数据） |

**反悔点**：某个删除项如果真的有人在用（不是"以后可能用"），它就不是块，是**特性**——先写成 `nous-app` 的 `/v1` 端点，再过 3.2 的判据 (b)。
**不许做**：不许为了保住块模型先写一套通用进程监管；不许把文件锁移植进 Rust；不许留"多块服务"兼容层。

---

### 3.7 置顶进内核（阻塞项，不是顺手做）

- 现状：置顶写穿到 `bjes6b` 的 `POST /p/demo/bjes6b/api/sessions/:id/pin`——**bjes6b 一停，置顶立刻坏**。
- 要求：`POST /v1/sessions/:id/pin`（带 `requestId`，落一条提交）；列表顺序由提交重算；客户端零状态。
- 判据：停掉 `bjes6b` 三进程之后置顶仍生效；同一 `requestId` 重放不产生第二条提交；删掉客户端配置目录后置顶不变。
- 反悔点：若"置顶"其实是**每个客户端各自的偏好**（不同设备不同），它就不是内核真相、是客户端配置——但那需要"用户身份"，v1 没有这个概念，所以进内核。
- 不许做：不许在客户端或中间层存内存态置顶。

## 4 · 接缝

**结论**：扩展是**进程外的应答者**，内核发起、扩展应答；内核只认 trait（`dyn Section` / `dyn Tool`），进程、spawn、reap、生命周期全归应用层。

**通道**：应用层 spawn 时给的一对管道 + 长度前缀 JSON 帧。管道断开即死亡，不需要心跳。不开第二个端口，不引入第二种编码。

**四件能力**

| 能力 | 一句话 | 判据 | 不许做 |
|---|---|---|---|
| ① section | 纯函数：同输入→同字节输出 | 同一轮请求跑两次，第二次 section 与第一次逐字节相等，且不产生第二条 `pi.system` 增量 | 不许碰盘、不许看时钟、不许发网络 |
| ② tool | 接入第二个工具时内核零改动 | 只写扩展清单 + 扩展进程，`git diff --stat -- $K` **为空** | 不许为某个工具改内核（要改 ⇒ 接缝形状错了，本章重开） |
| ③ 名牌 | 扩展注册具名槽位（`skill:<名>`、`prompt:<名>`），应用层按会话选中集装配 | 删除一个名牌后旧会话的选择静默失效、可由 `missing()` 点名，不自动改选 | 不许替用户改选择；不许名牌带默认值 |
| ④ 目录代读 | 扩展在清单里声明要哪个目录，**由内核代读并交付已解析内容** | 扩展进程里 `open(` / `readFile` 命中断言为空；同一目录两次交付一致 | 不许扩展自己 open 路径（等于没有权限边界） |

**状态归属**：缓存可以留在扩展进程，但**必须可丢弃**。判据：杀进程 → 重新拉起 → 同一会话继续，两次结果 `diff` 为空。

**失败三分（不许合并成"超时"）**

| 情形 | 结果 | 重试 |
|---|---|---|
| 明确返回错误 | 落一条 error 工具结果，模型看得到，继续 | 否 |
| 超时 / 无响应 | 同上 + 一条可观测诊断（谁的调用、等了多久），不许静默降级 | 否 |
| 崩溃 / 被 kill | 同上，允许自动重启**一次**；第二次崩 = **进程熔断**（标 broken、不再调用、明确告知；**无自动恢复**，只有重载能解）。段级「连续 3 次」熔断只用于**非崩**失败，**不因进程重启清零**。**进程熔断不写 `abandoned`** —— `abandoned` 只用于整机崩/重启的孤儿扫描（S6） | 仅"尚未开始执行"阶段 |

铁律：**超时值由内核给**（不许扩展自己声明）；**只对"尚未开始执行"重试**（工具可能写文件、发请求，重试 = 重复副作用）。

**反悔点**：扩展必须跑在另一台机器 → 通道换 UDS；扩展出现不可重建的状态 → 热加载对它退化成"重启 + 重建"，单列，不给统一数字。
**不许做**：不许自绘 UI；不许读别的会话；不许直连 provider；不许直写数据目录；不许后台长驻定时器；不许把"毫秒级热加载"当指标——指标是"重建后输出逐字节相同"。

---

## 5 · 取消语义

**结论**：三个触发源、一条通道；取消必须落一条提交；终止形状是**一条 entry + 一个终态字段四个值**；取消幂等，终态不可变。

**三个触发源，一个实现**

- 客户端 `POST /v1/sessions/:id/abort`、内核超时、扩展进程死 —— 三个源都调**同一个** `cancel`。
- 判据：取消**实现**只有一处（当前 `nous-chord/src/context.rs:323` `CancelHandle::cancel`；字面 `fn cancel` 另有 22 处是读取/测试/名字撞车，要逐行分类）；`abort` 的调用链上**必须出现它**（当前红：`nous-durable/src/session/commands.rs:172` 只写 `task["abortRequested"]`，全程没调 `cancel()`）；`grep -rn 'abortRequested'` 只命中一处函数；三个源各有一条测试打这个实现（S7）。
- 不许做：不许任一触发源自己写一套收尾逻辑；不许把取消做成"只发信号不落提交"——那样重连后无痕，"谁取消的"永远查不出来。

**终态形状：一个字段，四个值**

> 一条 entry，终态字段 `reason ∈ {done, user_cancel, timeout, extension_crash, abandoned}`。归因靠这个字段，不靠 entry 类型。

- 熔断读 `extension_crash`、告警读 `timeout`、启动扫孤儿读 `abandoned`、客户端渲染只读"有没有终态"。
- **`abandoned`** = 进程自己死掉（崩溃 / 重启时清出的孤儿），由启动扫描补写，不由任何触发源主动写。
- **终态与它的正文能同一条提交写完，就必须同一条**（否则 `kill -9` 会切出"有正文无终态"）；只有"取消发生在正文已落库之后"才允许第二次提交带终态。
- 判据：同一个会话分别制造四种终止，库里四种值各出现一次，且 entry 类型都相同（没有第二种 entry）。
- 不许做：不许用"两种 entry"表达终止（客户端会漏处理第二种）；不许新造平行表示——现有形状能表达就只加一个枚举值，加不出来才允许加字段，且必须在 spec 里写"为什么现有形状不够"。

**partial 落不落库**：落。取消时已产生的 partial 作为 normal entry 落库，并带终态标记。
- 判据：取消后重连，模型看到的是"有结局的一段文本"，不是"没有结局的一段文本"。
- 反悔点：如果 partial 落库让 `pi.live` 的 4MB 硬顶频繁撞顶，改为"取消时只落终态、partial 留在 live 里直到超时清理"，但那要一条新判据说明 partial 何时丢。

**幂等**：`abort` 两次 → 200 + 同一终态；**不报错、不改写终态，第一个写下的归因赢**。
- 判据：abort 两次，`entries` 只多一条提交，两次响应字节相等。
- 不许做：不许对已结束的会话返回 404/409；不许"第二次取消覆盖第一次的原因"。

---

## 6 · 并发模型

**结论**：一个进程、一个库，单写者成立；任务全在应用层；内核层不 `spawn`、不碰时钟、不无限缓冲。

**内核层零 `spawn`，且时钟由应用层注入**

- 内核层只许用同步原语（`Mutex`、`Notify`、`CancellationToken`），**不许**直接调 `tokio::time::{sleep, interval, timeout}`。理由是真话：`tokio::time` 在没有 runtime 的宿主里会 panic，所以"内核层能嵌入无 runtime 宿主"这条只有在时钟注入之后才成立。先例照抄 `nous-chord` 的 `watch`：注入时钟 + 显式 `poll()`。
- 判据：`git grep -nE 'tokio::time::(sleep|interval|timeout)' -- $K` 为空；时钟只出现在一个适配器文件里。

**每会话一个任务，任务身份是 `(会话 id, generation)`**

- 身份不能只是会话 id：否则"上一次生成迟到的提交"会盖到新一次生成上。
- 任务终结必须写终态（第 5 章），包括它自己崩掉的情况（→ `abandoned`，由启动扫描补）。
- 判据：4 个会话同时生成，每个会话的 SSE 帧顺序与它自己的提交顺序一致；`commit_seq` 全局单调但允许交错。
- 反悔点：若必须做全局并发配额（同时最多 N 个生成、按优先级排队）→ 那是应用层的准入控制，不是内核层形状，本章不用重开。

**单写者：只有一层**

- 一个进程、一个库、`BEGIN IMMEDIATE` + `busy_timeout`，写事务全库唯一。**不许**每会话一个 DB 连接——那不叫粒度细，那叫把 SQLite 的单写者前提拆了。
- 判据 A（结构）：写事务只包住一次提交调用，不跨 `await`；**`Session::commit` 是同步函数**（返回 `Result`，不是 `Future`）。同步 ⇒ 跨不了 await ⇒ 长生成持不住写锁。
- 判据 B（相对量）：4 会话并发时，B 会话提交的 p99 ≤ **5 × 它空闲时的 p99**。不写绝对毫秒数——j1900 上还没有基线，绝对阈值只会制造假红。
- 判据 C（留档，不是闸）：j1900 实测 p99 写进 `docs/bench-*.md`，只做基线。
- 判据 D：WAL 下读不被写阻塞——生成中 `GET /v1/sessions` 不超时。
- 反悔点：判据 B 红且 WAL 已开、批量写已按 `json_each` 合并 → 才允许引入"单写线程 + mpsc 队列"；或先切细提交（一次提交里别塞太多 entry），**不是加锁**。

**锁粒度：库级单写事务 + 会话级内存串行**

- `Session` 的 `Mutex<Option<Storage>>` 只保护这一个会话的提交顺序；跨会话互斥交给 SQLite 写锁。不做全局大锁，不做每会话一个连接。
- 判据：同判据 B（换成"会话 A 生成期间 B 提交 `rename`"）。

**背压：真正的风险不是任务数，是慢客户端**

- 每条 SSE 连接必须有**未发送字节上限**（建议 1MB，计入硬顶）；到上限就断这条连接（复用现有 `Lagged` 断流路径），**不许无限缓冲**。今天有界的是**帧数**不是字节（`routes.rs:214` `mpsc::channel(256)`、`hub.rs:59` `broadcast::channel`），所以这条现在是红的，且理由是"没有这个对象"（S5）。
- 判据：一个只连不读的客户端，进程 RSS 涨不过 64 × 1MB 的余量，且到点被断开。

**取消 vs 事务**

- 取消不打断事务：只打断生成与等待（I/O、provider、扩展）；事务一旦开始就让它走完，半途撤销等于踩掉"拒绝 ≠ 回滚"。取消的落库是事务结束之后的一条新提交。
- 判据：`kill -9` 与 `abort` 同时发生，库里不出现"有终态无正文"或"有正文无终态"；`PRAGMA integrity_check` 绿。
- 反悔点：若出现"取消后必须立刻释放写锁"的硬需求 → 把长事务拆成多次提交，仍不是"取消回滚事务"。
- 不许做：不许在内核层 `spawn`；不许直接读时钟；不许每会话一个 DB 连接；不许无上限的 SSE 未发送缓冲；不许用绝对毫秒数当判据（相对量 + 结构判据）。

## 7 · 持久化与崩溃恢复

**结论**：存什么/版本/迁移由 `docs/storage.md` 定；本 spec 只定崩溃代价。能丢的只有 ≤100ms 的 partial。

| 东西 | 能不能丢 | 为什么 |
|---|---|---|
| 正在流的那段（≤100ms 的 partial） | **能** | 重连靠 snapshot，模型看到的是"少了一小段"，不是错乱 |
| 已落库的提交 | **不能** | 那是真相本身 |
| id 分配器 / `commit_seq` 计数器 | **绝不能** | 丢了会复用 id → 数据损坏，事后查不出来 |
| 终态 | **不能** | 丢了那条消息永远"还在生成"，客户端永远转圈 |
| 待迁移的旧库副本 | **能** | 重新拷一份即可；只是回滚窗口变短 |

**孤儿回收（`abandoned`）**

- 启动时扫描"有正文、无终态"的 live 文档，补一条 `abandoned` 终态提交。
- **必须幂等**：连重启三次，只多一条，不是三条。
- 判据：`kill -9` 后重启一次 → 恰好一条 `abandoned`；再重启两次 → 条数不变。

**`kill -9` 矩阵**（每条红/绿，写明"丢了什么"）

| 崩在哪 | 丢了什么 | 恢复动作 | 能否接受 |
|---|---|---|---|
| 生成中（partial 未到 100ms 闸） | ≤100ms 的流 | 启动扫 `abandoned` | 能 |
| 提交事务内 | 无（事务原子） | 回滚即可 | 能 |
| partial 落库中 | ≤100ms 的流 | 同上 | 能 |
| 终态提交前一刻 | 一个终态 | 启动扫 `abandoned` 补 | 能（可见为"意外结束"） |
| 归档中 | 半完成的归档副本 | 归档是复制 + 墓碑，见 `docs/adr/0001` | 能（孤儿副本可删） |
| 迁移中 | 无（版本推进整批原子） | 重跑迁移；库比代码新则拒绝打开 | 能（但拒绝启动要 clearly 报错） |
| id 分配器写坏 | — | **无** | **绝不能发生**：分配器写入必须在提交事务内 |

**反悔点**：某一步的代价是"会话静默回退且客户端看不出来"→ 这一步要加显式标记。
**不许做**：不许为恢复引入第二份真相（旁路日志、影子库）；恢复能力不许靠"多写一份"解决；不许让终态与正文分两次提交（除非取消发生在正文落库之后）。

## 8 · 迁移

**结论**：两段切 —— 先只读上线（只服务 GET/SSE，写全 409），稳了再停机拷库、写 `writer='nous'`、开写。

**判据**：第一段读出 189 会话 / entries 分页 / SSE 首帧，所有 POST 返回 409；第二段开写后 `durable_schema.writer='nous'`。
**反悔点**：`docs/storage.md` 里那 3 条"不做"的用例若含线上真有的记录类型 → 不许切，先补类型。
**不许做**：不许双写；不许承诺"切回 node 且不丢数据"——回滚只到第二段开始那一刻。

## 9 · 不做清单（v1）

- 扩展：自绘 UI、自己 open 路径、直连 provider、直写数据目录、读别的会话、后台长驻定时器。
- 客户端：链接库、第二个监听、第二份真相、面板前端、手机端 `bjes6b`、一次性 ticket、客户端存置顶/未读。
- 内核层：产品语义（段名、名牌含义、skills 解释）、进程模型、路由名、`spawn` 长活任务、直接读时钟。
- 并发：每会话一个 DB 连接、无上限的 SSE 未发送缓冲、取消回滚事务。
- 部署：热升级、工号传递、多写者。
- 契约：在 `wire.md` 里重复字段清单（形状归测试）；`resolve` 发凭证；中间层翻译（BFF）。
- 块：块模型本身、“块崩溃”的监管逻辑、文件锁、多块服务兼容层。

**判据**：每一条都能落到一条 grep 或依赖断言——ch1 的 13 条 + §3.6 的 4 条 + §3.4 的 4 条 + §3.7 的 3 条。列在这里的条目若在 `verify.sh` 里找不到对应断言，就补断言，不许只写在文档里。
**反悔点**：任何一条被"这次特殊"绕开时，先改本清单（写清为什么），不许在票里就地破例——就地破例两次，这份清单就废了。

## 收口顺序（跨票，顺序不能换）

1. **现在**：杀掉 cwd 指向已删 worktree 的孤儿进程（已执行：`3276477` 已停；`pgrep -f 'node server.js'` 剩 `bjes6b`×2、`qrd3wh`×2、`/root` 一条，后两条不属于本仓范围）。
2. **删代码票**（独立票，spec 分支不碰）：`src/`（5,752 行前端）+ 块目录 + `bjes6b`（等第 4 步）。
3. **置顶进内核 → NousDesk 改读内核**（§3.7，单独一张票，先做）。**这是第 4 步的前置**，也是唯一一件"客户端现在就在用、必须无缝切换"的事。
4. 停 `bjes6b` 三进程 → 删 `bjes6b` 目录。判据：`pgrep -f 'node server.js'` 的 cwd 全部指向存在且属于 `/opt/Nous` 的目录，做完为空。
5. **块清算落地**（§3.6）：删树与块元数据（`__nous/ctx`、组/叶名字与 `kind`、`launch`、`package.json`、`server.js`）；**文档内容本身不删**（它是 NousDesk 文档页的素材，属于内容，不属于块）。判据：`content/` 下无 `package.json`、无 `server.js`、无 launch 配置；`__nous/ctx` 从 `wire.md` 与代码里消失；文档正文文件字节 diff 为空。
6. **第 8 章迁移**：读面先上线、再切写，`abandoned` 与终态五值一起验。
7. **票 #14 搬迁**：先搬再改设计。

## 已收口的一条（留作记录）

**面板壳不是面板前端。** 事实：`server/*.ts|mjs` = 1,890 行，挂在 vite preview 上，做四件事——块进程监管（`blocks.ts:128` `spawn`）、宿主路由（`/p/<board>/<id>/__nous/{ctx,resolve,doc}`，`ctx.ts:67`）、锁、content store；`resolve` 正是唯一的发现路径。结论：壳并进 `nous-app`，但**不是"通用的块监管"**——块在 §3.6 里被清算掉，`spawn` 只服务扩展。

---

## 附录 A · 已拍板清单（31 条）

**任何一条被重开，必须先给出"反悔点已命中"的证据，否则不许重新讨论。**

| # | 结论 | 一句话理由 | 反悔点 |
|---|---|---|---|
| Q1 | spec 在应用层之上：一个仓、两层（`layer` 列：kernel / app；路径不分层，见 §3.0），不是给内核做注脚 | 形状/语义分治，才不至于重写已验收的东西 | 事件面从"客户端投影"变成"两方消息总线" |
| Q2 | 最小不可分单元是**提交**；entry 可寻址但不是原子单位；会话是身份 + 命名空间；其余是投影 | 事务边界 = 提交，`commit_seq` 单库全局单调已是事实 | 出现必须跨会话原子的动作 |
| Q3 | `/v1` HTTP+SSE 是唯一客户端面，不给任何客户端开链接库 | 链接库绕过 `requestId` 幂等与 `whenBusy`，造第二个真相入口 | 需要双向或多路复用 → 在 `/v1` 加通道，仍不是链接库 |
| Q4 | v1 扩展能力四件：section / tool / 名牌 / 目录代读 | 用例取自线上四个扩展 | 出现"内核每轮主动推事件给扩展"的真用例 |
| Q5 | "扩展"是一个概念（不再有"插件"）；调用、接缝、名牌一并钉死 | 两个词就要两套生命周期 | 出现必须同进程、可崩内核的东西 → 那是内核变体，另立一章 |
| Q6 | 迁移两段切：先只读上线，再停机拷库切写 | 一步到位 + 24h 回滚是假安全（切后有写入） | `storage.md` 那 3 条"不做"的用例含线上真记录类型 |
| Q7 | 应用层拥有扩展进程；内核层只认 trait，不认识进程 | 内核是库，要能嵌入 | `layer = kernel` 的 crate 里出现 `std::process`（判据 6 红） |
| Q8 | 通道 = 应用层 spawn 的一对管道 + 长度前缀 JSON 帧 | 管道断开即死亡，不需要心跳；不引第二种编码 | 扩展必须跑在另一台机器 → UDS |
| Q9 | 扩展状态必须可丢弃：杀掉重建后输出逐字节相同 | 否则热加载 = 数据丢失 | 出现不可重建状态的扩展 → 单列"重启+重建" |
| Q10 | tool 进 v1 只留形状；判据 = 接入第二个工具时内核零改动 | 零用例不是降低判据的理由 | 接入需改内核 → 形状错，第 4 章重开 |
| Q11 | 失败三分：明确错误 / 超时 / 崩溃；超时值由内核给；只对"尚未开始执行"重试 | 工具可能有副作用，盲目重试 = 重复副作用 | 需要更早阶段重试 |
| Q12 | `resolve` 是唯一发现路径；不扩块清单 | 地址不写进客户端常量 | 需要面板之外的东西也能发现 → 独立域名 |
| Q13 | 鉴权只用长期 token 走 header；一次性 ticket 删除 | 客户端全是 Rust，能带 header | 移动端用系统 WebView → 加回 ticket |
| Q14 | 硬顶 64 连接 / 4MB `pi.live` / 200MB 库；连接 503+Retry-After，文档 413、库 507 | 按 j1900 可用内存倒推 | 真机撞上限 → 调数不改形状 |
| Q15 | 面板前端废；客户端只剩 NousDesk（core/shell 分层） | 面板是同一层的投影，不该持有语义 | 出现只能跑浏览器的业务判断 → 搬进 `/v1` 字段 |
| Q16 | `nous-app` 住 Nous 仓；仓转私有（deploy key 前置） | 公开仓 + 私有依赖 = 坏仓 | 要公开 → `nous-app` 拆独立私有仓 |
| Q17 | 一份 `wire.md` 住应用层；`nous-server` 降 lib 导出 `Router` | 一个客户端只看得到一个面 | 出现纯内核部署形态 → 拆两份契约 |
| Q18 | 边界判据 13 条（依赖双向 + 词表基线 + 路由 + 数据面 + 进程 + 契约 + 锁） | 词表基线只减不增才是判据 | 基线被绕过两次 → 换整文件哈希 |
| Q19 | `skills`/`template`/`archived` 字段留内核层，语义归应用层 | 原样存、原样取、不解释 | 要按 skills 查 → 应用层建投影表，不许内核懂语义 |
| Q20 | `collapse`/`prepare` 留内核层，装配归应用层；内核层不认段名 | 判据测的是装配，不是方言适配 | 某方言需要"段"的语义 → 搬 |
| Q21 | 内核层零 `spawn`，时钟由应用层注入 | `tokio::time` 无 runtime 会 panic | 每连接两任务打满 4 核 → 应用层单循环 + `select!` |
| Q22 | 每会话一个任务，身份 `(会话 id, generation)` | 取消 = 一次 token + 一次提交 | 需要全局并发配额 → 应用层准入控制 |
| Q23 | 单写者一层：`BEGIN IMMEDIATE`；`Session::commit` 是同步函数 | 同步 ⇒ 跨不了 await ⇒ 长生成持不住写锁 | 相对量判据红（>5× 空闲 p99）→ 单写线程 + mpsc |
| Q24 | 锁粒度：库级单写事务 + 会话级内存串行；不许每会话一个 DB 连接 | 每会话一连接拆掉单写者前提 | 写事务塞太多 entry → 先切细提交，不是加锁 |
| Q25 | 取消不打断事务；终态五值含 `abandoned`；启动扫孤儿幂等 | 半途撤销事务 = 踩掉"拒绝 ≠ 回滚" | 必须立刻释放写锁 → 拆多次提交 |
| Q26 | 面板壳并进 `nous-app`；块清算三类；`spawn` 只服务扩展；不移植文件锁 | 块不再是进程，没有"崩"这回事 | 某个删除项有真人在用 → 写成 `/v1` 特性 |
| Q27 | 手机端 BFF 随 `bjes6b` 一起消失，不搬进 Rust | 翻译层每天在分裂契约真相 | WebView 需持票 → 加回 ticket（同 Q13） |
| Q28 | `resolve` 不发凭证；`version` 是协议版本，未知即拒连 | 发凭证的发现端点只能是发钱机 | 客户端要展示块状态 → `/v1/blocks` |
| Q29 | `resolve` 必须含 `url`+`version`、禁含秘密；`id`/`name`/`path` 标 deprecated | 判据目的是"不发钱"，不是键越少越好 | 键越加越多且被用于业务判断 → 退回严格形状 |
| Q30 | 块树整个废掉；文档内容不删 | 树唯一读者是已死的面板前端 | 需要展示文档块 → `/v1/docs`，且过判据 (b) |
| Q31 | 先杀 cwd 指向已删 worktree 的孤儿；其余等 Q 置顶票 | 它服务的文件树已经不存在 | 杀掉后 4800 不通 → 先起干净的再杀 |

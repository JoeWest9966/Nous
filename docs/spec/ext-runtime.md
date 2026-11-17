# Nous 扩展运行时 spec（骨架 v1）

**状态**：章目已过（1–9 章 + 附录 A/B/C）。正文待落，从第 1 章开始，一章一改。
**与内核 spec 的关系**：并列，不互相引用正文；对内核的修改申请见 `docs/spec/seams-open.md`（S1–S11，**已签 · 2026-11-17**，已贴进 `kernel-rust.md`）。
**术语**：以 `GLOSSARY.md` 为准（`扩展` / `调用` / `接缝` / `名牌` / `句柄` / `值` / `内容`）。本文件只加词，不改已定义的词。

## 读法（总则五条，先于一切章）

1. **断言一律 `| tee` 输出命中行**，不许 `grep -c`（计数会把"命中了哪一行"藏起来）。
2. **无载体的断言：挂票号 + `SKIP` 前缀**；没票号 = 删。SKIP 必须打印出来，不许静默 `exit 0`。
3. **每条判据必须能单独起进程跑**（一条命令一次结果）。SKIP 是 `exit 0`，一进套件就会把后面的判据一起吞掉——那是换了个位置的假绿。
4. **判据里出现的每个路径，第一段必须是** `test -e <载体> || { echo "SKIP: 等 <票号>"; exit 0; }`。引用 `scripts/judge.sh` 的判据，**脚本本身也要 guard**（脚本不存在时是 SKIP，不是红——否则一开票就红一片，然后有人去改判据）。
5. **本仓只有样例扩展**，住 `Nous/exts/`（四小一大：`section` / `tool` / `nameplate` / `handle` / `combo`）。**真扩展住 `Nous-extend`，本文件的判据一律不引用它**——跨仓判据等于"跑不了 = 绿"。

## 章目

| # | 章 | 一句话结论 |
|---|---|---|
| 1 | 边界与形态 | 一个扩展 = 一份清单 + 一个入口 + 一个进程；句柄 / 值 / 内容三分 |
| 2 | 通道与调用 | 一对管道 + 长度前缀 JSON；call id 路由；一轮一次；帧里只有三样；单帧 1MB 双向 |
| 3 | 四件能力 | 三个定义（同一轮 / 名牌冲突 / 内容形状）+ section / tool / 名牌 / 句柄，每件 判据·反悔点·不许做 |
| 4 | 失败、熔断与预算 | 两条阶梯；诊断去重但计数不清零；三个预算数；两个硬顶 |
| 5 | 热加载与生命周期 | 常驻；三个触发源一个实现；重载不动命名空间 |
| 6 | 句柄与代读 | 装载时一次快照 + watch debounce；解析失败留旧 |
| 7 | 隔离与写边界 | 专用 uid；EACCES；内核唯一写者，扩展只能申请 entry / 文档 |
| 8 | 装载、禁用、审计、分发 | 哈希 + target；编译是带 target 的任务，派给能力匹配且空闲的机器 |
| 9 | 看不见什么 | 帧三键、environ 白名单、fd、别的会话 —— 每条都要能红 |

**第 8 章的前置（硬约束，写在章首）**：机器编目（机器 + 注解 + 能力字段）**必须先从块 `vq0f03` 搬进应用层、走 `/v1/machines`**，搬完才谈"派构建任务"。挂 **T1**。没搬之前，第 8 章所有判据是 SKIP。

---

## 1 · 边界与形态

**结论**：一个扩展 = **一份清单 + 一个入口 + 一个进程**。它住自己的仓（`Nous-extend`），本仓只有样例。清单声明**句柄名**（不写路径）；值只在应用层配置里一处；扩展只看得见**内容**。四件能力全空 ⇒ 拒载。

### 1.1 清单是能力声明，不是分发清单

清单只有这些字段：`name` / `version` / `protocol` / `file` / `entry` / `sections`（**有序**，每项带 `tag`）/ `handles`（句柄名）/ `summary` + `category`（给人看，v1 可省）。

- **`name` 就是 API**：会话里存的是这个名字，改它等于让所有老会话找不到自己。
- **`sections` 的顺序就是提示词里的顺序**，`tag` 也在清单里 —— 应答里的顺序与 `tag` 一律不算数（第 3 章 ①）。
- **四件能力全空 ⇒ 拒载**，并指名"这个扩展什么都不提供"。`123` 就是那个用例：装一个空壳不许看起来像验收通过。
- **判据**：EX-01（清单无本机路径）· EX-03（空扩展拒载）· EX-05（无 `kernel`/`timeout`/`concurrency` 字段）· EX-07（一扩展一进程、uid 两两不同）。
- **反悔点**：出现"一个目录要跑两个入口"—— 那不是变体，是**两次装载**（两个实例、各自 uid、各自内容）；出现"清单里要写代码路径以外的东西"—— 那属于分发（第 8 章），不进清单。
- **不许做**：`E7`（清单自报超时 / 并发 / 内核版本）；变体不许变成清单字段。

### 1.2 句柄 / 值 / 内容，三样不许混用

- **句柄**是清单里的名字（`skills`、`prompts`、`skills-upstream`）；**值**是应用层配置给这个句柄的目录，全库只一处；**内容**是内核代读后交给扩展的已解析数据。
- 扩展拿不到值，也拿不到路径；它只在装载时拿到**一次内容**（第 6 章定新鲜度）。
- **缺句柄 ⇒ 拒载并指名**。不许继承今天 `catch { return [] }` 那种静默空名册 —— 那是"测试永远绿、线上写错仓库"的形状。
- **判据**：EX-02（缺句柄拒载并指名）· EX-26（`/proc/<pid>/fd` 里没有句柄的值）· EX-46（`environ` 键集合 = 白名单）。
- **反悔点**：出现"扩展要在运行时按需读某个子路径"—— 那是"内容"的形状不够（扩它，第 6 章），**不是**给路径。
- **不许做**：`E1`（自己 open 路径）。

### 1.3 样例是四小一大，判据只跑它们

`exts/{section,tool,nameplate,handle,combo}`：每件能力一个最小样例，加一个组合样例。一个扩展压四件能力，坏了的时候说不清是哪件破了；四个小样例，红哪条就是哪件。

- **② 的载体**是 `exts/tool` 的 v1 → v2（只加文件、不改内核），不是把 `bash` 搬成扩展。
- **判据**：EX-04（v1→v2 之后 `git diff --stat -- crates/kernel` 为空）。
- **反悔点**：样例要改内核才能跑 → 接缝形状错，第 3 章重开（附录 A · Q10）。
- **不许做**：不许把真扩展写进本仓当判据载体（跨仓判据 = 跑不了 = 绿，读法第 5 条）。

### 1.4 与内核 spec 的边界

本文件不重复内核的判据。对内核的任何修改都走 `seams-open.md`（S1–S11，已签 · 2026-11-17，已贴进 `kernel-rust.md`）。**已签 = 定案**：以后要改这些条款，走反悔点，不许重新讨论。

---

## 2 · 通道与调用

**结论**：一对管道 + **长度前缀 JSON 帧**；帧里只有三样（`conversationId` / `cwd` / `extensions`）；**call id 路由，不靠 pid**；一轮内每扩展只调一次；**单帧上限 1MB，双向各自计数**，超限按崩溃处理。

通道本体是附录 A 外已拍板的（`seams-open` 无关，见内核附录 A 的 Q8）：一对管道、长度前缀帧、**不开第二个端口、不引入第二种编码、不做心跳**——管道断开即死亡。本章只定**帧里装什么、多大、归谁**。

**字段是后果，不是起点。** 下面的三条序列是唯一起草依据（用例先于字段）：

```
A · 一轮请求    A→E {call, kind:"section", input:{conversationId,cwd,extensions}}
                E→A {call, kind:"result", sections:[{key,text,tag}…]} | {call, kind:"result", error}
B · 一次工具    A→E {call, kind:"tool", tool, args, cwd}
                E→A {call, seq:0..N, kind:"output"} ×N  然后  {call, seq:N, kind:"final", result|cancelled:true}
取消（同一条 host→ext 管道，不是第二个通道）  A→E {call, kind:"cancel"}
C · 重载        A→E'{kind:"hello", protocol, manifestHash}   E'→A{kind:"hello", protocol}
                A→E' 内容（内核代读）              E'→A{kind:"nameplates", names:[…]}
```

### 2.1 帧里只有三样

`{conversationId, cwd, extensions}`。**`shown` 不进帧**（它是上一轮的输出，递进去等于允许扩展有跨轮状态，与"状态必须可丢弃"直接矛盾；它留在应用层的 `renderSections` 兜底）；**`read` 不进帧**（那是内核的文档读口，递过去等于给扩展开一个跨会话查询面）；**`model` 也不进**（模型选择是会话配置，扩展按模型分支就是产品决策渗进扩展的第一步）；`agent` 全量更不行（它带着这一轮所有工具，含别的扩展的工具）。

- **判据**：EX-08（键集合相等）· EX-12（`shown`/`read`/`model`/绝对路径 逐行空）。
- **反悔点**：某个扩展说"我需要 `shown`" → 那说明它想做状态机，答案是**把那份状态做成内核的提交**，不是把 `shown` 递过去。
- **不许做**：`E4`。

### 2.2 call id 路由，不靠 pid

调用的归属只认 `call` 字段。这是重载时在飞调用能活下来的**唯一**原因：进程换了、调用不换，旧调用的输出仍投给它自己。

- **判据**：EX-11（重载期在飞 tool：输出不丢、不乱序、不串到新调用）· EX-39（call id 递增不变、`seq` 不重置）。
- **反悔点**：出现"同一个 call id 被两个进程同时应答"—— 那不是并发，是路由错，先修路由。
- **不许做**：`E12`。

### 2.3 一轮内每扩展只许调一次

否则热路径变成可嵌套的循环：扩展在同一轮里再问宿主、宿主再叫它，预算与归因全部失效。

- **判据**：EX-09（每扩展 = 1 次）。
- **反悔点**：出现"一段 section 必须分两次拿"的真用例 → 那是"内容"的形状不够（第 6 章），不是加往返。
- **不许做**：`E12`。

### 2.4 单帧上限 1MB，**双向各自计数**

没有上限的长度前缀帧 = 一个 500MB 的 section 能把宿主撑死。

- 上限与 SSE 未发送上限**同数**（1MB），`host→ext` 与 `ext→host` **各算各的**；只管一侧的话，一个恶意扩展仍能用一个 frame 打穿宿主。
- 超限**不是截断、不是丢弃**，而是**按崩溃处理**：杀掉、重启一次、归因 `extension_crash`。截断会产生"看起来正常但内容少了一半"的帧 —— 那是这类协议最难查的一类 bug，也是**假绿**的温床。
- 拒绝必须发生在**读入之前**：先看长度前缀，超限就不读 payload。
- **判据**：EX-58（发 2MB 单帧的假扩展 → 被杀、归因 `extension_crash`、宿主 RSS 涨幅 < 一个上限量）。
- **反悔点**：出现"真的有一条合法 section 超过 1MB"（有实测数据）→ 调数不改形状；仍不许改为"截断"。
- **不许做**：不许无限缓冲；不许把超限当成"明确错误"（那会让模型看到一条正常但残缺的段）。

---

## 3 · 四件能力

**结论**：四件能力里三件是**纯函数**（section / 名牌声明 / 句柄内容的投影），一件会**产生副作用**（tool）。副作用那一件因此多两样东西：`mutations` 与取消。

### 3.0 先把三个词钉死

写到各自小节才发现定义不在，就晚了 —— 三个词先定，且每个都给可观测形状。

| 词 | 定义 | 可观测形状（判据靠它） |
|---|---|---|
| **同一轮** | **同一个 `(conversationId, generation)`**。`generation` 从生成开始到终态落库为止；没有活跃生成时，"同一轮"= 同一次请求 | 两次调用之间 `generation` **没变**，且两次之间**没有新的 `pi.system` 提交**。否则"两次相等"可以靠"这两次之间什么都没发生"作弊 |
| **名牌冲突** | 两个扩展声明同一个具名槽位（如都叫 `skill:tdd`） | **后装载的那个拒载**，并指名「`skill:tdd` 已被 X 占用」；**先装的那个照常工作**。不许静默去重、不许覆盖、不许改名、也**不许"按清单顺序优先"**（那只是把隐形规则换了个说法） |
| **内容** | 内核按句柄的值代读后交给扩展的已解析数据，形状固定为 `{ files: [{ path, text }], truncated: bool }` | `path` 是**相对句柄值目录**的相对路径，**不许出现 `..`、不许绝对路径**；`truncated=true` 表示撞了体积上限（**上限进配置，不进清单**）。所以扩展拼不出句柄之外的任何东西 —— 这让"看不见路径"从口号变成机械可验 |

- **判据**：EX-14（同一轮：`generation` 未变 + 无新 `pi.system` 提交）· EX-59（名牌冲突）· EX-60（内容路径越界）。
- **反悔点**：① 若出现"同一 `generation` 内合法地需要两次不同结果" → 那是这个段的入参不够（补 `input`），不是放宽"同一轮"。② 若真出现"一份内容要跨句柄拼路径" → 那是内容形状不够（加字段），**不是**允许 `..`。
- **不许做**：`E13`（名牌带默认值）。

### 3.1 ① section（纯函数）

本节挂 **EX-13 · EX-14 · EX-15 · EX-16**。

**结论**：同输入 → 同字节输出。它每轮都跑，所以它碰盘、看时钟、发网络任何一样，都会让每轮请求追加一条 `pi.system` 增量并打掉 provider 的提示词缓存 —— 账单上看不出来，缓存命中率上看得很清楚。

- **判据**：EX-13（两次逐字节相等）· EX-14（同一轮内 `generation` 未变、无新提交）· EX-15（**打乱应答顺序**，输出不变）· EX-16（**`tag` 故意写错**，输出不变 —— 顺序与 `tag` 只认清单）。
- **失败时的形状**是第 4 章的事（崩 → 保留 `shown` → 3 次进熔断），这里只钉"它本身必须是纯的"。
- **反悔点**：若"两次逐字节相等"在真机上不可达 → 说明这个段不纯，**先修纯度，不许放宽判据**。
- **不许做**：`E5`（后台长驻定时器）、`E10`（fail-closed 段 —— 需要"崩了就立刻不能出现"的段一律进内核层，扩筭的段永远 fail-open）。

### 3.2 ② tool（唯一有副作用的一件）

本节挂 **EX-17 · EX-18 · EX-19 · EX-20 · EX-21 · EX-22**。

**结论**：接入第二个工具时**内核零改动**。工具会写东西，所以它比 section 多三样：流式（`seq`）、取消（`final.cancelled`）、申请写（`mutations`）。

- **判据**：EX-17（`seq` 单调、无缺号）· EX-18（取消 → 终帧 `cancelled:true`）· EX-19（**取消之后只许发 `final`**，晚期 `output` 被拒且不落库）· EX-20（`result` 与 `mutations` **同一次提交**）· EX-21（同一组 mutation 重放只落一次）· EX-22（被拒的 mutation = 一条 error result 回给模型）。
- **`final` 二选一**：`result` 或 `cancelled:true`。**两者都没有 = 崩溃**（按 `extension_crash`）。
- **反悔点**：接入第二个工具要改内核 → 接缝形状错，**本章重开**（附录 A · Q10）。
- **不许做**：`E11`（申请第四种写形状 —— 只能追加 entry 或写文档，终态不在申请范围内）、`E3`（直写库）。

### 3.3 ③ 名牌（具名槽位）

本节挂 **EX-23 · EX-24 · EX-25 · EX-59**。

**结论**：名牌由**扩展在应答里自报**（`{kind:"nameplates", names:[…]}`），应用层装；因为名字是扫盘后才知道的（`skill:<名>` / `prompt:<名>`），清单里写不出来。

- **判据**：EX-23（杀进程 → 名牌全消失且 `missing()` **逐个点名**）· EX-24（重启 → 恢复，会话选择**未被改**）· EX-25（差集以"清单 **+** 应答"两份为准 —— 只算应答的话，应答丢了就是崩溃路径）· EX-59（同名冲突：后装拒载并指名双方、先装的照常工作）。
- **不自动改选**：盘上删掉一条，会话里的 `prompt:<名>` 留着静默失效，等用户自己换 —— 一次扫盘不许替人改选择。
- **反悔点**：出现"会话选了某名牌，但该扩展从未在这台机器上装过"的合法场景 → 才允许离线声明；仍不许带默认值。
- **不许做**：`E13`（名牌带默认值）。

### 3.4 ④ 句柄（内核代读）

本节挂 **EX-26 · EX-27 · EX-60**。

**结论**：扩展声明句柄名，内核按值代读并交付**内容**（形状见 3.0）。扩展一个文件句柄都没有。

- **判据**：EX-26（`/proc/<pid>/fd` 里没有句柄的值）· EX-27（同一目录两次交付逐字节一致）· EX-60（造一条 `../../etc/passwd` 的文件项 → **内核在代读时**就拒掉并落诊断，不是交给扩展再检查）。
- **新鲜度**是第 6 章的事（装载时一次快照 + watch debounce），本节只钉"路径不出内核"。
- **反悔点**：出现"几十 MB 内容、必须两个实例共享一份"（§Q22 反悔点）→ 才允许一个进程挂两个值，且要写清为什么不能装两次。
- **不许做**：`E1`（自己 open 路径）。

---

## 附录 · 索引

- **A · 判据总表**（**正文的唯一判据来源**，57 条）：四列 `编号 | 判据（可跑的命令）| 票号 | 谁签字`。正文各章只写"见附录 A · EX-nn"，**改判据只改附录 A，正文不抄命令**。
- **B · 不许做清单**：`K1–K8`（真相层）/ `L1–L6`（能力层）/ `A1–A5`（应用层）/ `E1–E13`（扩展）。
- **C · 接缝要改**：`docs/spec/seams-open.md`（S1–S11，已签）。

---

## 附录 A · 判据总表

载体约定：`crates/{kernel,platform,app}` = T1 落地后的三层；`exts/{section,tool,nameplate,handle,combo}` = 本仓样例；`scripts/judge.sh <case>` = 逐条起进程的执行器（`--list` 输出全部 case 名）；`$DATA` = 运行时数据目录。**`谁签字` 空着 = 未签 = 正文不许引用。**

| 编号 | 判据（可跑的命令） | 票号 | 谁签字 |
|---|---|---|---|
| EX-01 | `test -d exts \|\| { echo "SKIP: 等 T5"; exit 0; }; grep -rn '/opt/' -- exts/*/nous.json \| tee` 逐行空 | T5 | 待人拍 |
| EX-02 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh load-missing-handle \| tee` 含"缺句柄 X"且该扩展未装载 | T5 | 待人拍 |
| EX-03 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh load-empty-extension \| tee` 含"这个扩展什么都不提供" | T5 | 待人拍 |
| EX-04 | `test -d crates/kernel \|\| { echo "SKIP: 等 T5"; exit 0; }; git diff --stat -- crates/kernel \| tee` 在 `exts/tool` v1→v2 之后为空 | T5 | 待人拍 |
| EX-05 | `test -d exts \|\| { echo "SKIP: 等 T5"; exit 0; }; grep -rnE '"(kernel\|timeout\|concurrency)"' -- exts/*/nous.json \| tee` 逐行空 | T5 | 待人拍 |
| EX-06 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh protocol-mismatch \| tee` 含双方版本且旧进程仍在服务 | T5 | 待人拍 |
| EX-07 | `test -d crates/app \|\| { echo "SKIP: 等 T5"; exit 0; }; pgrep -af 'ext-' \| tee` 行数 = 装载数，且 uid 两两不同 | T5 | 待人拍 |
| EX-08 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh dump-frame-keys \| tee` 键集合 == `{conversationId,cwd,extensions}` | T5 | 待人拍 |
| EX-09 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh calls-per-turn \| tee` 每扩展 = 1 | T5 | 待人拍 |
| EX-10 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh truncated-frame \| tee` 在截止时间内退出（不挂死） | T5 | 待人拍 |
| EX-11 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh inflight-across-reload \| tee` 输出不丢、不乱序、不串 | T5 | 待人拍 |
| EX-12 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh dump-frame-keys \| tee \| grep -E '"/\|shown\|read\|model"'` 逐行空 | T5 | 待人拍 |
| EX-13 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh section-twice \| tee \| uniq \| wc -l` = 1（两次逐字节相等） | T5 | 待人拍 |
| EX-14 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh pi-system-count \| tee` 两轮后条目数不变，且两次之间 `generation` 未变 | T5 | 待人拍 |
| EX-15 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh shuffle-order \| diff - <(./scripts/judge.sh baseline) \| tee` 空 diff | T5 | 待人拍 |
| EX-16 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh wrong-tag \| diff - <(./scripts/judge.sh baseline) \| tee` 空 diff | T5 | 待人拍 |
| EX-17 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh tool-stream-seq \| tee` 单调递增、无缺号 | T5 | 待人拍 |
| EX-18 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh tool-cancel \| tee` 终帧 `cancelled:true` | T5 | 待人拍 |
| EX-19 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh tool-cancel-late-output \| tee` 含"取消后收到 output"并拒绝落库 | T5 | 待人拍 |
| EX-20 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh result-and-mutations \| tee` 同一次提交（`commit_seq` 相等） | T5 | 待人拍 |
| EX-21 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh mutation-replay \| tee` 同一组重放只落一次 | T5 | 待人拍 |
| EX-22 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh reject-mutation \| tee` 含 error result 且原因回给模型 | T5 | 待人拍 |
| EX-23 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh kill-extension \| tee` 名牌全消失 + `missing()` 逐个点名 | T5 | 待人拍 |
| EX-24 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh restart-extension \| tee` 名牌恢复、会话选择未变 | T5 | 待人拍 |
| EX-25 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh nameplate-diffset \| tee` 差集以"清单 + 应答"两份为准 | T5 | 待人拍 |
| EX-26 | `test -d crates/app \|\| { echo "SKIP: 等 T5"; exit 0; }; ls -l /proc/$(pgrep -f ext-handle)/fd \| tee \| grep "$(./scripts/judge.sh handle-value)"` 逐行空 | T5 | 待人拍 |
| EX-27 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh deliver-twice \| tee \| uniq \| wc -l` = 1 | T5 | 待人拍 |
| EX-28 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh crash-once \| tee` 提示词逐字节不变 + 条目数不变 + 恰一条诊断 | T5 | 待人拍 |
| EX-29 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh third-failure-breaker \| tee` 恰一条删除增量 | T5 | 待人拍 |
| EX-30 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh breaker-10-turns \| tee` 条目数不再变 | T5 | 待人拍 |
| EX-31 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh diagnostic-dedup \| tee` 10 轮 1 条诊断，且计数显示 3 | T5 | 待人拍 |
| EX-32 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh always-crash \| tee` 第 2 次崩进进程熔断、段状态无"第 3 次" | T5 | 待人拍 |
| EX-33 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh breaker-idle-calls \| tee` 10 分钟内调用 = 0 | T5 | 待人拍 |
| EX-34 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh concurrency-20 \| tee` 16 在飞 / 4 排队并点名、无丢弃 | T5 | 待人拍 |
| EX-35 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh output-1mb \| tee` 撞顶被杀，归因 `extension_crash` | T5 | 待人拍 |
| EX-36 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh rss-256mb \| tee` 撞顶被杀，归因 `extension_crash` | T5 | 待人拍 |
| EX-37 | `test -d crates/app \|\| { echo "SKIP: 等 T5"; exit 0; }; grep -rn 'fn reload' -- crates/app \| tee` 恰好 1 行 | T5 | 待人拍 |
| EX-38 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh reload-triggers \| tee \| wc -l` = 3（三源各一条） | T5 | 待人拍 |
| EX-39 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh callid-across-reload \| tee` 递增不变、`seq` 不重置 | T5 | 待人拍 |
| EX-40 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh idle-wakeups \| tee` 60s 合计 < 10 | T5 | 待人拍 |
| EX-41 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh reload-fails-keeps-old \| tee` 旧版本继续服务 | T5 | 待人拍 |
| EX-42 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh partial-write \| tee` 写一半不重载 | T5 | 待人拍 |
| EX-43 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh write-complete \| tee \| grep -c reload` = 1（恰好一次） | T5 | 待人拍 |
| EX-44 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh parse-fails \| tee` 继续用旧快照 + 一条诊断 | T5 | 待人拍 |
| EX-45 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh out-of-cwd-write \| tee` 得到 `EACCES`（不是"成功但记录"） | T5 | 待人拍 |
| EX-46 | `test -d crates/app \|\| { echo "SKIP: 等 T5"; exit 0; }; cat /proc/$(pgrep -f ext-section)/environ \| tr '\0' '\n' \| grep -vE '^(cwd\|conversationId\|extensions\|PATH)=' \| tee` 空 | T5 | 待人拍 |
| EX-47 | `test -d crates/app \|\| { echo "SKIP: 等 T5"; exit 0; }; ls -l /proc/$(pgrep -f ext-tool)/fd \| tee \| grep -E '\.sqlite\|\.db'` 逐行空 | T5 | 待人拍 |
| EX-48 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh byte-flip \| tee` 拒载并指名，无第二条装载路径 | T5 | 待人拍 |
| EX-49 | `test -d crates/app \|\| { echo "SKIP: 等 T5"; exit 0; }; ps -o comm= -g $(pgrep -f nous-app) \| tee \| grep -E 'cargo\|rustc'` 逐行空（除开 build 角色） | T5 | 已签 · 2026-11-17（S1/S9） |
| EX-50 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh triple-not-compatible \| tee` 拒载（兼容列表，不是相等） | T5 | 已签 · 2026-11-17（S9） |
| EX-51 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh audit-kinds \| tee \| wc -l` = 4（装/卸/禁用/熔断） | T5 | 待人拍 |
| EX-52 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh audit-replay-10 \| tee` 提交数 +1 | T5 | 待人拍 |
| EX-53 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh entries-filter \| tee` 按 `pi.extension` 过滤出该条 | T5 | 待人拍 |
| EX-54 | `test -d crates/app \|\| { echo "SKIP: 等 T1"; exit 0; }; grep -rn 'api/catalog' -- crates server \| tee` 逐行空；`curl -s localhost:PORT/v1/machines \| tee` 含兼容 target 列表 | T1 | 已签 · 2026-11-17（S10/S11） |
| EX-55 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh two-sessions-isolated \| tee` A 的输出不含 B 的任何字节 | T5 | 待人拍 |
| EX-56 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh disable-extension \| tee` 0 次调用 + 名牌全 `missing()` + 诊断写"已禁用" | T5 | 待人拍 |
| EX-57 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; diff <(grep -oE 'judge\.sh [a-z0-9-]+' docs/spec/ext-runtime.md \| awk '{print $2}' \| sort -u) <(./scripts/judge.sh --list \| sort -u) \| tee` 空 diff（判据表引用的 case 名与 `--list` 集合相等） | T5 | 待人拍 |
| EX-58 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh oversized-frame \| tee` 扩展被杀、归因 `extension_crash`、宿主 RSS 涨幅 < 1MB | T5 | 待人拍 |
| EX-59 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh nameplate-conflict \| tee` 第二个拒载并点名双方，第一个照常工作 | T5 | 待人拍 |
| EX-60 | `test -e scripts/judge.sh \|\| { echo "SKIP: judge.sh 未建（T5）"; exit 0; }; ./scripts/judge.sh handle-path-escape \| tee` 内核代读时拒掉 `..`/绝对路径并落诊断 | T5 | 待人拍 |

**统计**：60 条。59 条挂 T5（扩展运行时本体），1 条挂 T1（编目搬迁）。
**已签 3 条**（EX-49 / EX-50 / EX-54，来自 S1/S9/S10/S11）；其余待签。**T5 未开票之前，全表 SKIP** —— 这是如实状态，不是"接近完成"。

## 附录 B · 不许做清单

**真相层 `crates/kernel/`**：`K1` 进程 · `K2` 网络 · `K3` 直接时钟（含注释） · `K4` 路由 · `K5` 产品词（baseline 只减不增） · `K6` 依赖 providers · `K7` 解释 skills/template 语义 · `K8` 文件锁
**能力层 `crates/platform/`**：`L1` 被真相层依赖（方向：providers，不是 nous-ai） · `L2` 拥有监听 · `L3` 碰数据库 · `L4` 无 IO 替身 · `L5` 自报为扩展 · `L6` 工具子进程绕开 `os_env`（全仓进程创建点恰好两个）
**应用层 `crates/app/`**：`A1` 第二个扩展 spawn 点 · `A2` 第二个监听 · `A3` 第二份真相（旁路日志/影子库） · `A4` 缓存段文本当真相 · `A5` 文件锁
**扩展**：`E1` 自己 open 路径 · `E2` 直连 provider · `E3` 直写库 · `E4` 看见别的会话 · `E5` 后台长驻定时器 · `E6` 自绘 UI · `E7` 清单自报超时/并发/内核版本 · `E8` 写终态五字面量（作为字段值） · `E9` 越界写 · `E10` fail-closed 段 · `E11` 申请第四种写形状 · `E12` 一轮内多次往返 · `E13` 名牌带默认值

`K*`/`L*`/`A*` 的断言归内核 spec 与 `seams-open.md`（S1–S11，已签）；`E*` 的断言归本文件附录 A（EX-01 起）。

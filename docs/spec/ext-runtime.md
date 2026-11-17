# Nous 扩展运行时 spec（骨架 v1）

**状态**：**定稿 v1 · 2026-11-17**（九章正文 + 附录 A/B/C 齐）。**改动需要反悔点证据 + 人签字**，不许就地改。
**与内核 spec 的关系**：并列，不互相引用正文；对内核的修改申请见 `docs/spec/seams-open.md`（S1–S11，**已签 · 2026-11-17**，已贴进 `kernel-rust.md`）。
**术语**：以 `GLOSSARY.md` 为准（`扩展` / `调用` / `接缝` / `名牌` / `句柄` / `值` / `内容`）。本文件只加词，不改已定义的词。

## 读法（总则五条，先于一切章）

1. **断言一律 `| tee` 输出命中行**，不许 `grep -c`（计数会把"命中了哪一行"藏起来）。
2. **无载体的断言：挂票号 + `SKIP` 前缀**；没票号 = 删。SKIP 必须打印出来，不许静默 `exit 0`。
3. **每条判据必须能单独起进程跑**（一条命令一次结果）。SKIP 是 `exit 0`，一进套件就会把后面的判据一起吞掉——那是换了个位置的假绿。
4. **判据里出现的每个路径，第一段必须是** `test -e <载体> || { echo "SKIP: 等 <票号>"; exit 0; }`。引用 `scripts/judge.sh` 的判据，**脚本本身也要 guard**（脚本不存在时是 SKIP，不是红——否则一开票就红一片，然后有人去改判据）。
5. **本仓只有样例扩展**，住 `Nous/exts/`（四小一大：`section` / `tool` / `nameplate` / `handle` / `combo`）。**真扩展住 `Nous-extend`，本文件的判据一律不引用它**——跨仓判据等于"跑不了 = 绿"。
6. **哨兵原则。** 每条判据里必须有**一个只有真实实现才能产生的字节** —— 具体 `commit_seq` / entry id / 哈希前缀 / 错误原文 / `version` 数字，称**哨兵**；**附录 A 里要明写这条的哨兵是什么**。
   形式 = **计数断言 + 内容断言 + 哨兵值**，缺一不合格。哨兵必须出现在**命令里**，不许只写在散文里。
   *例*：EX-51 不能只数"提交 4 条"，还要断言其中一条的 `kind=pi.extension` **且** `manifestHash` 等于某个具体哈希前缀 —— 桩造不出来，必然红。
   *负向判据*（形如"逐行空"）哨兵换一种：**故意弄脏一次，它必须红**；红的那次就是哨兵。
7. **不同源原则。** 期望值**写死在判据里**，实测值**从真实系统读出来**（DB / 进程 / 文件 / 系统调用），两者**不许同源**。`judge.sh` 只负责"执行场景 + 打印观测"，**不许硬编码观测值**；判据负责"写死期望值 + 比对"。
   判据里不许出现 `<任意>` / `X` / `约等于` —— 哨兵必须是**具体字面量**。
   否则 `judge.sh` 里一句 `echo "commit_seq=5074"` 就能让全表绿 —— 那只是把桩从正门赶出去，又从后门放进来。**这条本身也是判据：EX-81。**
   证据：v0 骨架（每个 case 只 `echo` 一行 SKIP）让 EX-13/27/38/51 四条当场变绿 —— 一个只会打印一行的桩就骗过了计数型判据。审计见 `judgment-audit.md`。

## 章目

| # | 章 | 一句话结论 |
|---|---|---|
| 1 | 边界与形态 | 一个扩展 = 一份清单 + 一个入口 + 一个进程；句柄 / 值 / 内容三分 |
| 2 | 通道与调用 | 一对管道 + 长度前缀 JSON；call id 路由；一轮一次；帧里只有三样；单帧 1MB 双向 |
| 3 | 四件能力 | 三个定义（同一轮 / 名牌冲突 / 内容形状）+ section / tool / 名牌 / 句柄，每件 判据·反悔点·不许做 |
| 4 | 失败、熔断与预算 | 两条阶梯；五态状态机（转换才落提交，计数在内存）；诊断去重但计数不清零；熔断必须推 SSE；三个预算数；两个硬顶 + 并发 16 |
| 5 | 热加载与生命周期 | 常驻；三个触发源一个实现；**扩展级重载 + 窗口内合并**；在飞调用按能力分两种收法；开机装载失败必须可见 |
| 6 | 句柄与代读 | 值只在配置一处；装载时一次快照 + debounce；**代读三条硬边界（相对路径 / 不跟随符号链接 / 截断确定性）**；解析失败留旧 |
| 7 | 隔离与写边界 | 隔离只有三种可验证形式（uid / fd 表 / errno）；内核唯一写者（entry / 文档两原语）；恶意样例 `exts/evil` 是判据载体 |
| 8 | 装载、禁用、审计、分发 | 哈希 + target + **来源**；编译是带 target 的任务；**原子 rename** 切换；禁用是配置但**下游必须显式失败**；审计是提交不加端点 |
| 9 | 看不见什么 | 全靠负向断言：**金丝雀三处**（跨会话 / 跨扩展 / 跨请求）+ **延迟重复仍相等**（时钟与随机数）+ 四条出口 |

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

## 4 · 失败、熔断与预算

**结论**：失败分三种（明确错误 / 超时 / 崩溃）；**两条阶梯**互不混用；状态只有五个，转换才落提交；预算三个数、硬顶两个、并发一个上限 —— 全部由宿主给，清单无权声明。

本章的风险不是"写不清楚"，是**写清楚了但没人看得见**：静默降级是 ch4 那条"不许静默降级"明确要防的。

### 4.0 两条阶梯，互不混用

| 阶梯 | 管什么 | 阶梯 | 后果 |
|---|---|---|---|
| **进程级** | 崩溃（进程死 / 不回终帧 / 撞硬顶 / 帧超限） | 重启一次 → **第二次崩 = 进程熔断** | 该扩展**全部能力**停；靠重载恢复 |
| **段级** | **非崩**失败（超时 / 明确 error） | 同一段**连续 3 次** = 段熔断 | 该**段**停；靠重载恢复 |

- 段级计数**不因进程重启清零**（进程重启只清"进程熔断"那一条）。只有**重载**清全部。
- 铁律沿用 ch4：**超时值由内核给**（不许扩展声明）；**只对"尚未开始执行"重试一次**。

### 4.1 状态机（先有图，再有判据）

**状态只有五个**（不是每个错误一个状态）：

| 状态 | 含义 | 调用行为 |
|---|---|---|
| `normal` | 正常 | 照常 |
| `degraded` | 该段失败过（保留 `shown`，零增量） | 照常（继续试） |
| `tripped_section` | 该段熔断 | 该段不出现；其余段照常 |
| `tripped_process` | 进程熔断 | 该扩展 **0 次调用** |
| `disabled` | 人在配置里禁用（**不是失败**，不进熔断） | 该扩展 **0 次调用** |

**转换表**（每一行 = 一个若漏就会变成"判据迁就实现"的边）：

| 从 | 事件 | 到 | 副作用 |
|---|---|---|---|
| `normal` | 调用崩 **#1** | `degraded`（重启一次） | 保留 `shown`、零 `pi.system` 增量、一条诊断、**落一条提交** |
| `degraded` | 同一扩展**再崩**（不论哪段） | `tripped_process` | 写**一次**删除增量（该扩展全部段）、落提交、无自动恢复 |
| `degraded` | 非崩失败在同一段累计 **3** 次 | `tripped_section` | 写**一次**删除增量（仅该段）、落提交 |
| `degraded` | 非崩失败累计 < 3 | `degraded`（不变） | **只增内存计数**，不落提交（状态没变） |
| `tripped_*` | 任何调用 | 不变 | **0 次调用**（不试、不刷日志） |
| `tripped_*` | 重载**失败** | 不变 | 旧进程继续服务（不丢状态） |
| `tripped_*` | 重载**成功** | `normal` | 计数归零、一条加回增量、**一条恢复提交** |
| 任意（除 `disabled`） | 人在配置里禁用 | `disabled` | 一条禁用提交；诊断写"已禁用"（不是崩溃） |
| `disabled` | 人取消禁用 | `normal` | 一条提交；计数归零 |

### 4.2 计数在内存，状态在提交

| 东西 | 存哪 | 能不能丢 |
|---|---|---|
| 连续失败计数、上次诊断时间（`(扩展, 段, 错误类) → {count, lastSeen}`） | **内存** | **能丢**（重启归零，可接受：它是"观测"不是"真相"） |
| 状态转换（降级 / 熔断 / 恢复 / 禁用 / 装卸） | **提交**（`kind = pi.extension`） | **不能丢** |

理由一句话：**诊断如果进提交，一个坏扩展能把提交日志刷成一万条，把真正的对话埋掉。**

- 日志本身去重：同一 `(扩展, 段, 错误类)` **60 秒内一条**；**去重只去条数，不去计数**（3 次崩必须数得出来）。
- **判据**：EX-28（崩那轮提示词逐字节不变 + 条目数不变 + 恰一条诊断）· EX-31（10 轮失败 → `pi.extension` 提交**恰好 1 条**、内存计数 = 10）· EX-61（重启后计数 = 0，但降级状态**仍在**）。

### 4.3 熔断必须推到客户端

熔断 = 某扩展的能力**从模型眼前消失**。用户不知道 = 静默降级。

- 降级 / 熔断 / 恢复，各**推一条 SSE `entry_appended`（`kind = pi.extension`）**；客户端据此显示"某扩展已停用"。**不新增端点**（Q23 已拍：不加 audit 端点）。
- **判据**：EX-62（已连着的 SSE 客户端在**一个事件循环内**收到那条 `entry_appended`）· EX-63（重连后 `snapshot` 里的 entries 也含它 —— 历史可查）。
- **反悔点**：若推送改成"客户端轮询"，则必须能在一个固定期限内看到 —— 但"不发"永远不是选项。

### 4.4 三个预算数（全部由宿主给）

| 名字 | 值 | 为什么是总预算而不是每扩展 |
|---|---|---|
| section 装配 | **整轮所有扩展合计 1s** | 每扩展一个上限会被扩展数量线性放大；4 个扩展就是 8s 启动延迟，而且你不知道是哪一个拖的 |
| tool 单次 | **默认无限**，靠取消 | 工具可能跑很久；取消是唯一正确的收尾方式 |
| 握手 / 重载 | **固定 10s** | 只用于"新进程起不来" |

- 撞 section 预算被砍的那些，归因 `timeout`（走段级阶梯：非崩失败）。
- **判据**：EX-29（非崩失败 3 次 → 恰一条删除增量）· EX-30（熔断后 10 轮条目数不变）。

### 4.5 两个硬顶 + 一个并发上限

| 名字 | 值 | 撞顶算什么 |
|---|---|---|
| 单次调用输出字节 | **1MB**（与第 2 章单帧上限同数） | 崩溃（杀、重启一次、`extension_crash`） |
| 每扩展 RSS | **256MB** | 同上；且**拒绝发生在分配之前**（撞顶时 RSS 不涨） |
| 每扩展并发调用 | **16**（宿主给，**不进清单**） | 超出在应用层排队并记一条诊断（"排队中，等 X 次"），**不许静默丢弃** |

- **判据**：EX-32（每次都崩 → 第 2 次崩进进程熔断、段状态无"第 3 次"）· EX-33（熔断后 10 分钟调用次数 = 0）· EX-34（20 路并发 → 16 在飞 / 4 排队并点名）· EX-35（1MB 输出撞顶）· EX-36（256MB RSS 撞顶）。
- **反悔点**：真出现"旧段导致模型做错事"的**实例**（不是担心）→ 才改成"崩即删段"，并把线上纪律降级为"只在正常态成立"。（已批：没有实例不改。）
- **不许做**：`A6`（诊断进提交 —— 每轮一条）· `E7`（清单自报超时 / 并发）· `E8`（写终态五字面量）· `E10`（fail-closed 段）。

---

## 5 · 热加载与生命周期

**结论**：扩展**常驻**；三个触发源**收敛到一个实现**；重载粒度是**扩展级**且窗口内合并；在飞调用按能力分两种收法；开机装载失败**必须可见**。

### 5.1 常驻，不是按需

section 每轮都要跑，按需起就是冷启动进热路径。

- **判据**：EX-40（3 个常驻扩展空闲 60 秒，合计 voluntary wakeups < 10）。
- **反悔点**：出现"扩展数量把内存吃满"的实测 → 才谈按需起；仍不许 section 走冷启动。
- **不许做**：`E5`（后台长驻定时器 —— 静息零负载的扩展侧对应物）。

### 5.2 三个触发源，一个实现

watch（目录变）/ git HEAD 变 / 显式命令 —— 三个源都调**同一个**重载入口。

- **判据**：EX-37（`grep 'fn reload' crates/app` 恰好 1 行）· EX-38（三源各一条测试打它）。
- **反悔点**：出现第四个源（比如"客户端点一下"）→ 仍然只许调同一个入口，不许再写一条路径。
- **不许做**：`A7`（整仓重载）· `A8`（开机失败只打 stderr）。

### 5.3 重载粒度：**扩展级**，且窗口内合并

watch 触发是**目录级**的，重载必须是**扩展级**的。

- **一次重载只影响一个扩展**。不许整仓重载 —— 那会同时杀掉所有扩展的在飞调用，把"重载不动命名空间"这条作废。
- **同一扩展在 debounce 窗口内被写多次 → 合并成一次装载**。agent 一次改 20 个文件不该触发 20 次重启。
- **判据**：EX-64（同一扩展连写 20 个文件 → 重载**恰好 1 次**）· EX-65（两个不同扩展各写 → 各自重载 1 次，且重载 A 期间 B 的在飞调用不受影响）。
- **反证（能红）**：把合并去掉 → EX-64 立刻红成 20 次。这条反证要写进 `judge.sh` 的注释里，防后人"优化"掉合并。

### 5.4 在飞调用怎么办

调用归属只认 `call id`（第 2 章），所以**进程换了、调用不换**。

| 在飞的是什么 | 重载时怎么收 |
|---|---|
| tool | **等它跑完**；撞 tool 上限（默认无限）则**掐**，归因 `extension_crash`（**不许算成 `user_cancel`**） |
| section | 直接按 `timeout` 收（它们短，等它们只会把 1s 总预算用完） |
| 新调用 | 从新进程启动那一刻就走新进程；旧 call id 的输出仍投给它自己的调用 |

- **判据**：EX-39（重载前后 `call id` 递增不变、`seq` 不重置）· EX-11（重载期在飞 tool：输出不丢、不乱序、不串）。
- **反悔点**：真出现"等的那个 tool 把重载拖死"的实例 → 才给 tool 一个上限值；仍不许改成"直接掐"。

### 5.5 版本不符、重载失败、开机失败

| 情形 | 行为 | 判据 |
|---|---|---|
| 协议版本不符 | **拒载**，报双方版本；**旧进程不动、继续服务** | EX-06 |
| 重载解析 / 启动失败 | **旧的继续跑**（不丢旧快照、不丢状态）；落诊断 | EX-41 |
| **开机装载失败**（哈希不符 / 版本不符 / uid 分配失败） | **落一条 `pi.extension` 提交**（含名字 + 原因）+ **推一条 SSE**（走 4.3 那条可见路径）；**其余扩展照常**；**进程不许退** | EX-66 |

- **一个扩展坏了不许把 `nous-app` 拖死**。例外只有一条：**核心扩展**（随产品发布的那几个）坏了才允许拒绝启动 —— 这与"随便一个第三方扩展坏了"是两件事。
- **判据要能红**：把开机失败落提交那段去掉 → 只剩 stderr，客户端看不到；EX-66 立即红。
- **反悔点**：若"核心扩展"名单开始变长（超过一个手数得过来的枚举）→ 说明"核心"已被当成"重要的"在用，退回"所有扩展一视同仁 + 拒启需显式配置"。
- **不许做**：`A8`（开机失败只打 stderr）· `A7`（整仓重载）。

---

## 6 · 句柄与代读

**结论**：扩展拿不到路径、也拿不到值；它在装载时拿到**一份内容**，盘变了靠**扩展级重载**；代读本身有**三条硬边界**（相对路径不越界 / 不跟随符号链接 / 截断确定性）。

### 6.1 值只在配置一处

清单里写句柄**名**（`skills` / `prompts` / `skills-upstream`），值（目录）只在应用层配置里**一处**。扩展既拿不到值，也拿不到路径。

- **判据**：EX-26（`/proc/<pid>/fd` 里没有句柄的值）· EX-46（`environ` 键集合 = 白名单，不含任何值路径）。
- **反悔点**：出现"两个句柄指同一个目录"的真需求（比如 skills 与 skills-upstream 镜像到同一处）→ 允许，但**值条目仍只一条**，两个名字指同一行。
- **不许做**：`E1`（自己 open 路径）。

### 6.2 装载时一次快照 + watch debounce

**装载时**代读一次，之后盘变了由应用层 watch 触发**重载**（第 5 章 5.3：扩展级 + 合并）。扩展进程内**不 watch、不 open**。

- **debounce 是必须的**：`cat > SKILL.md` 这种写法会先截断再写，watch 会在半截时触发。
- **判据**：EX-42（`dd` 写到一半停住 → **不重载**）· EX-43（写完 + 窗口后 → **恰好重载一次**，不是 N 次）。
- **反悔点**：出现"内容必须秒级跟手"的需求（**有实测延迟数据**）→ 才谈每次调用重读，且必须重算第 4 章的 1s 总预算。
- **不许做**：`E1`。

### 6.3 代读的三条硬边界

| # | 边界 | 为什么字符串校验不够 | 行为（写死，不许"看情况"） |
|---|---|---|---|
| 1 | `path` 只许相对、不许 `..` | 拼路径就能出句柄 | 越界在**内核代读时**拒并落诊断（EX-60） |
| 2 | **不跟随符号链接** | `prompts/leak.md -> /etc/shadow` 的 `path` 依然是 `leak.md`，内容已在句柄之外 | `lstat` 是符号链接就**跳过**并落诊断点名 |
| 3 | **截断确定性** | 遍历顺序不固定 ⇒ 同一上限下每次截的不是同一批，3.1 的"两次逐字节相等"在撞顶时靠运气 | 固定遍历顺序（**按相对路径字符串排序**，不按 inode / 不按目录返回顺序）+ 同一上限下截断点相同 |

- **符号链接选一种写死：我们选"不跟随"**。另一种（跟随 + `realpath` 后必须以句柄值目录为前缀，否则拒）也成立，但两个混用就是没有规则；且 `realpath` 方案多一个 TOCTOU 窗口。
- **`truncated` 只是标注"有东西没给"，不是"每次给的不一样"。**
- **判据**：EX-60（`..` / 绝对路径）· EX-67（指句柄外的符号链接：结果**不含**它 + 一条诊断点名；**不许"读到了但标为可疑"** —— 那等于已经泄漏了）· EX-68（200 个文件、上限只装得下 50 → 连续两次代读的文件集合与顺序**逐字节相同**，均带 `truncated:true`）。
- **反悔点**：真出现"必须跟随目录内符号链接"的内容布局（仓库里就这么放）→ 改成 `realpath` + 前缀校验，并补一条"符号链接指向句柄内时被包含"的判据；**仍不许"读到后标记"**。
- **不许做**：`A9`（代读跟随符号链接不校验）· `A10`（遍历顺序依赖目录返回顺序 / 截断不确定）。

### 6.4 解析失败留旧快照

重载时新快照解析失败 → **继续用旧内容** + 落诊断。这是唯一允许"用旧值"的地方：它不是造第二份真相，是"没换成功"。

- **判据**：EX-44（解析失败 → 旧快照继续服务 + 一条诊断）· EX-27（同一目录两次交付逐字节一致）。
- **反悔点**：若"旧快照里含已被删的文件"被认为不可接受（人看不见它在过期）→ 那时才改成"降级为空内容"，且必须同时写一条可见诊断。
- **不许做**：`A4`（缓存段文本当真相 —— 快照是内容，不是真相）。

---

## 7 · 隔离与写边界

**结论**：隔离只有**三种可验证的形式** —— uid、fd 表、`errno`；其余都是"我们做了隔离"的自证。内核是**唯一写者**，扩展只能申请两件事（追加 entry / 写文档）。

**本章每条判据都带"怎么让它红"**（不只是"怎么验它是绿的"）：一条只能证明自己是绿的判据，等于没判据。

### 7.1 四条隔离判据（每条都写红法）

| # | 隔离了什么 | ✅ 绿（能跑的命令/观察） | ❗ 怎么让它红（反向） |
|---|---|---|---|
| 1 | **uid** | `getent passwd 60001` **为空**且进程能跑；三个扩展的 uid 两两不同 | 把 uid 分配改成"全用 nous-app uid" → 两两不同那条立红 |
| 2 | **提权能力** | `systemctl show nous-app -p AmbientCapabilities` 含 `CAP_SETUID`；扩展 uid **≠** `nous-app` 的 uid | 把 `AmbientCapabilities` 拿掉 → 扩展起不来（降权失败）**或** 起来但 uid 相同，两条都能打到 |
| 3 | **fd 继承（最常漏）** | 扩展 `/proc/<pid>/fd` 的条目**恰好** = `0,1,2` + 一对管道，**多一个即红** | spawn 前故意让应用层开一个文件 → 扩展 fd 表里必须**不**出现它 |
| 4 | **groups 与 env** | `/proc/<pid>/status` 的 `Groups:` 为空；`environ` 键集合 = 白名单 | 去掉 `setgroups([])` → `Groups:` 出现继承组即红；env 多传 `HOME` → environ 断言红 |

**fd 那条要特别写死**：uid 换了、env 清了、`setgroups([])` 做了，**但父进程的 fd 默认会被子进程继承**。nous-app 手里只要有一个 sqlite 句柄还开着，扩展就能通过 `/proc/self/fd/7` 直接写库 —— uid 一点用都没有。

- **要求**：spawn 时**显式关掉除 `0/1/2` 与帧管道之外的全部 fd**（`close_range` / `CLOEXEC` 全覆盖）。
- 原来那条"fd 里没有 `.sqlite`"（EX-47）**降级为注释级** —— 它被上面这条更强地覆盖了，但保留作诊断线索。
- **判据**：EX-69（uid）· EX-70（CAP_SETUID）· EX-71（fd 表恰好三项 + 反向）· EX-72（groups 空）。
- **反悔点**：单用户 / 容器环境里 uid 隔离做不出来 → **退回纯审计**，且 spec 里**明写**"v1 无强制写边界，只有事后可观测"（**ADR 就写这一条**），不许含糊。
- **不许做**：`A11`（spawn 时不显式关 fd，靠默认继承）。

### 7.2 越界写的可观测形状：`EACCES`，且拒绝发生在写之前

- ✅ **绿**：越界写拿到 `EACCES`，目标**不存在**也未被修改。
- ❗ **红法**：把 uid 改成同一个 → 越界写成功（判据必须当场红"成功但未拦截"）。
- **不许写成"成功但被记录"** —— 那叫事后审计，不叫边界；真要那样就必须按 7.1 的反悔点**明写降级**。
- **判据**：EX-73（`EACCES` + 目标未变）。
- **不许做**：`E9`（越界写）。

### 7.3 内核是唯一写者：两原语、三约束

扩展能申请的两件事：**追加 entry**、**写文档**。**终态不在申请范围内**（Q13：终态只由触发源写）。

| 约束 | 含义 | 判据 |
|---|---|---|
| **原子** | `result` 与 `mutations` 在**同一次提交**里 | EX-20（`commit_seq` 相等）—— 否则会出现"模型看到成功、文件没改" |
| **幂等** | mutation 带 `call id + 序号`，同一组重放只落一次 | EX-21 |
| **拒绝要可见** | 拒绝 = 这次调用是 **error result**，原因回给模型 | EX-22（不许静默丢弃） |

- **语义校验在应用层，结构校验在内核**（内核不认工具语义）。
- **不许做**：`E11`（申请第四种写形状）· `E8`（写终态五字面量）· `E3`（直写库）。

### 7.4 恶意样例是**判据载体**，必须进仓

`exts/evil/` 一个目录，专门用来**被拒**：越界写、跟随符号链接、发 2MB 单帧、声明超大 RSS、申请第四种写形状。

- 它**不是测试夹具**：它进仓、有清单、有一份 README 逐条对应 EX 编号，**并且永远不许装载成功**。
- **判据**：EX-74（`exts/evil` 在正常配置下装载失败或装载后被逐条拒绝，**每一条拒绝都能对上 EX 编号**）。
- ❗ **红法**：从 `exts/evil` 里拿掉一条坏能力（比如不再发巨帧）→ 对应的 EX（58）失去载体、当场变 SKIP —— **SKIP 不是绿**，这条红法靠的就是这个。

---

## 8 · 装载、禁用、审计、分发

**硬前置（再写一遍，章内判据全部依赖它）**：机器编目（机器 + 注解 + 能力字段）**先从块 `vq0f03` 搬进应用层、走 `/v1/machines`**（T1）。没搬之前，本章所有判据是 SKIP —— 这正是"SKIP 不是绿"的用法。

**结论**：分发物 = 目录 + 清单 + 哈希 + **目标 triple** + **来源**；编译是带 target 的任务，派给能力匹配且空闲的机器；装载是**原子 rename**；禁用是**配置**不是崩溃，且**下游必须显式失败**；审计是**提交**（可见），不加端点。

### 8.1 产物要记来源，不只是哈希

哈希只证明"内容没变"，**不证明"谁做的"**。而这条系统的将来是大量 agent 生成扩展，"这个扩展是谁、从哪个 commit、哪台机器编的"必须随产物走。

```
产物清单：{ hash, builtFrom: <git commit>, buildMachine: <机器名>, protocol }
```

- **装载时 hash 必须对**（不符即拒载，不重试、不降级）；
- **来源默认只记录、不拦截**；配置里可以打开"只接受来自这些机器 / 这些 commit 的产物"白名单（**默认关**）。
- **判据**：EX-75（装载后提交里能看到这三项；打开白名单后，来源不符的产物被拒载并指名）。
- **反悔点**：若来源白名单被当成"权限"用（开始有人问"怎么给某个 agent 开白"）→ 那是另一个机制，停，先回来改本节。
- **不许做**：`E7`（清单自报内核版本 —— 写内核版本等于每次内核升版都要重发所有扩展）。

### 8.2 装载前校验：四项，一次跑完

| 校验 | 不符时 | 判据 |
|---|---|---|
| **哈希** | 拒载、指名、不重试不降级 | EX-48（改一个字节） |
| **target triple** | 拒载；在宿主登记的**兼容列表**里即通过（不是相等：musl 静态产物在 gnu 宿主上能跑，反过来不行） | EX-50 |
| **protocol** | 拒载、报双方版本；**旧进程不动继续服务** | EX-06 |
| **清单字段** | 缺句柄拒载（EX-02）、能力全空拒载（EX-03）、无 `kernel`/`timeout`/`concurrency`（EX-05） | — |

- **运行时宿主进程树里不出现 `cargo` / `rustc`**（除显式打开该机器的 build 角色）：EX-49。
- **反悔点**：出现第二台 build 机器而 catalog 里没有 target 字段 → **先补 catalog**，不许在客户端硬编码 triple。

### 8.3 原子切换：任何时刻 kill 都只有两种结果

安装 / 升级 = "把新产物放到目录、切配置指向它"。定死：**先写临时目录 → `fsync` → 原子 `rename` → 再切配置**。

- 任何时刻被 `kill -9`，重启后要么是**完整旧版**、要么是**完整新版**；**不许出现"目录在、二进制不完整"**。
- **判据**：EX-77（写入过程中 `kill -9` 宿主 → 重启后仍能完整装载（旧版或新版），目录里无残留半成品；**连续 20 次随机时点 kill 都不出半装**）。
- ❗ **红法**：把 `rename` 改成直接写目标目录 → 随机时点 kill 必现半装（新版二进制截断）。
- **不许做**：`A12`（直接写目标目录，不原子 rename）。

### 8.4 禁用：不是崩溃，但下游必须显式失败

| 层 | 行为 | 判据 |
|---|---|---|
| 扩展自身 | **0 次调用**；名牌全部消失；诊断写"已禁用"（不是"崩溃"），不进熔断、不重启 | EX-56 |
| **下游会话**（配置里写着 `skill:tdd`，而提供它的扩展被禁用） | **明确失败并指名**："会话选了 `skill:tdd`，提供它的扩展已被禁用" | EX-76 |

- **不许静默把它从配置里去掉** —— 静默去掉 = 模型收到一个悄悄变小的能力集，用户以为还在用。
- **也不许改写提示词**（往提示词里塞一句"该技能不可用"）—— 那是另一种静默。
- **判据**：EX-76（禁用某扩展 → 选了它名牌的会话再请求一次 → 返回明确错误或落一条客户端可见的 `pi.extension` 诊断；**且提示词里不含"假装它没了"的痕迹**）。
- **不许做**：`E4`（看不见别的会话 —— 禁用信息只能经由该会话自己的路径交付，不许跨会话广播）。

### 8.5 审计：四种事件，一条提交，不加端点

装 / 卸 / 禁用 / 熔断 **各落一条** `pi.extension` 提交（含清单哈希、授予的句柄清单；8.1 之后再加来源三项）；同一份清单重放 10 次 → 提交数 **+1**（幂等）。

- 可见性靠已有的 `entries` 面按 `kind` 过滤，**不新增端点**（Q23 已拍）。
- **判据**：EX-51（四种事件各一条）· EX-52（重放 10 次 +1）· EX-53（`entries` 按 `pi.extension` 过滤出该条）。
- **反悔点**：真有人查"谁在什么时候拿到什么权限"（不是"以后可能"）→ 才走新端点的顺序（**先快照测试，再进 `wire.md`**）。
- **不许做**：`A3`（旁路日志当审计）。

### 8.6 机器编目（前置）

- **判据**：EX-54（`grep 'api/catalog'` 逐行空；`/v1/machines` 含兼容 target 列表）—— 挂 **T1**，本章唯一不挂 T5 的一条。
- **不许做**：`E7`。

---

## 9 · 看不见什么

**结论**：本章全是**负向断言**，所以每一条唯一的合法形式是"能让它红的那条观测"。三条工具：**金丝雀**（看不见的东西）、**fd / environ / 帧键**（看不见的通道）、**延迟重复仍相等**（看不见的时间）。

### 9.1 金丝雀：三处都要撒，三条独立判据

"看不见别的会话"这句话本身不可测。可测的形状只有一个：**往内容里埋一个唯一字符串，然后断言它不出现在任何出口。**

| # | 跨什么 | 埋哪 | 断言（出口） |
|---|---|---|---|
| 1 | **跨会话** | 会话 B 的正文里埋 `ZZ-9f3a-B` | 会话 A 的 section 输出、帧入参、stdout 里都不出现 |
| 2 | **跨扩展** | 扩展 Y 的内容目录里的文件名 | 扩展 X 的 cwd 与内容（`files[].path` + 内容）里**连文件名都不许出现** |
| 3 | **跨请求** | 上一轮的金丝雀 | 这一轮的句柄内容里不出现（防快照被全局缓存复用） |

- **判据**：EX-55（跨会话）· EX-78（跨扩展）· EX-79（跨请求）。
- ❗ **红法**：把帧的键集合放开成"所有会话" → 第一条立刻红；把内容缓存从"按扩展"改成"全局一份" → 第三条立刻红。
- **反悔点**：出现"两个扩展就应当看到同一份共享内容"的真需求 → 那是**一个句柄、两个装载**（§Q22），不是"共享缓存"。
- **不许做**：`E4`。

### 9.2 时钟与随机数：用"延迟后重复仍相等"测

扩展能读时钟、能取随机数 —— 这两样没法从外面禁掉，但它们同样破坏"同一轮两次逐字节相等"。所以本章收一条**不可见性判据**：

> 同一扩展、同一输入：**一次立刻调用、一次隔 ≥2 秒调用** → 输出逐字节相同；**连做 5 轮，5 次全相同**。

- 时钟 / 随机数一旦被读入输出，这条必然红。碰盘、碰网已被第 3 / 6 章的判据覆盖，不重复。
- **判据**：EX-80。
- **反悔点**：某扩展确实需要时间戳（比如"今天是几号"）→ **那是内核代它算并作为输入递进去**（帧加一个字段），不是让扩展自己读表。加字段要过第 2 章的帧键集合判据（EX-08）。
- **不许做**：`E5`。

### 9.3 其余四条出口（每条都已有判据，不重复）

| 出口 | 为什么它算"看见了" | 判据 |
|---|---|---|
| 帧 | 多一个键就是多一个面 | EX-08（键集合相等）· EX-12（无 `shown`/`read`/`model`/绝对路径） |
| `environ` | 凭证 / 路径会从这里泄 | EX-46 |
| `fd` | 一个继承来的句柄就能绕过 uid | EX-71（恰好 `0,1,2` + 一对管道） |
| 句柄内容 | 越界 / 符号链接 / 截断不确定 | EX-60 · EX-67 · EX-68 |

- **反悔点**：这四条任何一个需要"放宽一点点" → 先问"它能不能变成内核代算后递进来的输入"；能就走加字段（过 EX-08），不能就别放宽。
- **不许做**：`E1` `E2` `E4`。

---

## 附录 · 索引

- **A · 判据总表**（**正文的唯一判据来源**，57 条）：四列 `编号 | 判据（可跑的命令）| 票号 | 谁签字`。正文各章只写"见附录 A · EX-nn"，**改判据只改附录 A，正文不抄命令**。
- **B · 不许做清单**：`K1–K8`（真相层）/ `L1–L6`（能力层）/ `A1–A5`（应用层）/ `E1–E13`（扩展）。
- **C · 接缝要改**：`docs/spec/seams-open.md`（S1–S11，已签）。

---

## 附录 A · 判据总表

载体约定：`crates/{kernel,platform,app}` = T1 落地后的三层；`exts/{section,tool,nameplate,handle,combo}` = 本仓样例；`scripts/judge.sh <case>` = 逐条起进程的执行器（`--list` 输出全部 case 名）；`$DATA` = 运行时数据目录。**`谁签字` 空着 = 未签 = 正文不许引用。**

| 编号 | 判据（可跑的命令） | 哨兵（写死；只有真实现才产生） | 票号 | 谁签字 |
|---|---|---|---|---|
<!-- 单元格里的 `\|` 是 Markdown 表格转义，粘到 shell 要还原成 `|`。别用抽表器做全局反转义——会毁掉正则里的 `\|`。 -->
<!-- `judge.sh` 的 SKIP 一律走 **stderr**；stdout 只放观测（`key=value`）。否则任何 `| grep` 管道都会把 SKIP 吞掉，SKIP 变成假红。 -->
<!-- **`EX-30` / `EX-33` 已合并（进 `EX-29` / `EX-32`），编号不复用** —— 所以是 79 条却编到 `EX-81`。 -->

| EX-01 | 干净：`./scripts/judge.sh manifest-paths \| tee \| grep -F 'hits=0'`（`hits=0`）；脏（`--dirty`）：`./scripts/judge.sh manifest-paths --dirty \| tee \| grep -F 'hits=1 file=exts/section/nous.json'` **必须命中** | `hits=0` / `hits=1 file=exts/section/nous.json` | T5 | 待人拍 |
| EX-02 | `./scripts/judge.sh load-missing-handle \| tee \| grep -F 'missing_handle=skills'` | `missing_handle=skills` | T5 | 待人拍 |
| EX-03 | `./scripts/judge.sh load-empty-extension \| tee \| grep -F 'reason=这个扩展什么都不提供'` | `reason=这个扩展什么都不提供` | T5 | 待人拍 |
| EX-04 | `./scripts/judge.sh kernel-untouched-v2 \| tee \| grep -F 'tool_version=0.1.1 kernel_diff=0'` | `tool_version=0.1.1` + `kernel_diff=0` | T5 | 待人拍 |
| EX-05 | 干净：`./scripts/judge.sh manifest-forbidden-keys \| tee \| grep -F 'hits=0'`（`hits=0`）；脏（`--dirty`）：`./scripts/judge.sh manifest-forbidden-keys --dirty \| tee \| grep -F 'hits=1 file=exts/section/nous.json'` **必须命中** | `hits=0` / `hits=1 file=exts/section/nous.json` | T5 | 待人拍 |
| EX-06 | `./scripts/judge.sh protocol-mismatch \| tee \| grep -F 'protocol_host=3 protocol_ext=4 old_still_serving=1'` | `protocol_host=3 protocol_ext=4` | T5 | 待人拍 |
| EX-07 | `./scripts/judge.sh one-process-per-extension \| tee \| grep -F 'count=3 uids=60001,60002,60003'` | `uids=60001,60002,60003` | T5 | 待人拍 |
| EX-08 | `./scripts/judge.sh frame-keys \| tee \| grep -F 'keys=conversationId,cwd,extensions conversationId=7'` | `conversationId=7`（具体会话 id） | T5 | 待人拍 |
| EX-09 | `./scripts/judge.sh calls-per-turn \| tee \| grep -F 'calls_nous=1 calls_skills=1 calls_prompts=1'` | `calls_nous=1` | T5 | 待人拍 |
| EX-10 | `./scripts/judge.sh truncated-frame \| tee \| grep -F 'exit=0 declared=4096 read=4096'` | `declared=4096 read=4096`（必须相等） | T5 | 待人拍 |
| EX-11 | `./scripts/judge.sh inflight-across-reload \| tee \| grep -F 'lines=line-1,line-2,line-3,line-4,line-5 reloads=1'` | `lines=line-1,…,line-5` | T5 | 待人拍 |
| EX-12 | 干净：`./scripts/judge.sh frame-forbidden-keys \| tee \| grep -F 'hits=0'`（`hits=0`）；脏（`--dirty`）：`./scripts/judge.sh frame-forbidden-keys --dirty \| tee \| grep -F 'hits=1 file=frame'` **必须命中** | `hits=0` / `hits=1 file=frame` | T5 | 待人拍 |
| EX-13 | `./scripts/judge.sh section-twice \| tee \| uniq \| wc -l` = 1 且 `./scripts/judge.sh section-twice \| tee \| grep -F 'You are an agent inside Nous'` | `You are an agent inside Nous`（段首句） | T5 | 待人拍 |
| EX-14 | `./scripts/judge.sh pi-system-count \| tee \| grep -F 'pi_system=12 generation=7'` | `pi_system=12` + `generation=7`（哨兵：铺底 12 条 `pi.system`、第 7 代；两次调用之间必须不变） | T5 | T5 首次跑 · 2026-11-17 |
| EX-15 | `./scripts/judge.sh shuffle-order \| tee \| grep -F 'shuffle=1 diff_lines=0 baseline_first=preamble'` | `baseline_first=preamble` | T5 | 待人拍 |
| EX-16 | `./scripts/judge.sh wrong-tag \| tee \| grep -F 'tag_wrong=1 diff_lines=0 raw_first_char=Y'` | `raw_first_char=Y`（tag:false 段首字节） | T5 | 待人拍 |
| EX-17 | `./scripts/judge.sh tool-stream-seq \| tee \| grep -F 'seq=0,1,2,3,4 final=1'` | `seq=0,1,2,3,4` | T5 | 待人拍 |
| EX-18 | `./scripts/judge.sh tool-cancel \| tee \| grep -F 'call=1001 final=cancelled'` | `final=cancelled` + 具体 call id | T5 | 待人拍 |
| EX-19 | `./scripts/judge.sh tool-cancel-late-output \| tee \| grep -F 'rejected=1 reason=取消后收到 output committed=0'` | `reason=取消后收到 output` | T5 | 待人拍 |
| EX-20 | `./scripts/judge.sh result-and-mutations \| tee \| grep -F 'commit_seq_result=4 commit_seq_mutation=4'` | `commit_seq_result=4` + `commit_seq_mutation=4`（空库首跑：会话 1 提交 1、清单提交 2、`pi.extension` 3、工具结果 4） | T5 | T5 首次跑 · 2026-11-17 |
| EX-21 | `./scripts/judge.sh mutation-replay \| tee \| grep -F 'entries_after=3 entries_before=3 ids=8,9,10'` | `entries_after=3` + `entries_before=3` + `ids=8,9,10`（空库首跑：result 8、两条 mutation 9/10） | T5 | T5 首次跑 · 2026-11-17 |
| EX-22 | `./scripts/judge.sh reject-mutation \| tee \| grep -F 'kind=error result=mutations 必须有 kind'` | `reason=mutations[0].kind 不在 {entry,document}` | T5 | 待人拍 |
| EX-23 | `./scripts/judge.sh kill-extension \| tee \| grep -F 'nameplates=0 missing=skill:tdd'` | `missing=skill:tdd` | T5 | 待人拍 |
| EX-24 | `./scripts/judge.sh restart-extension \| tee \| grep -F 'nameplates=1 selected=skill:tdd changed=0'` | `selected=skill:tdd changed=0` | T5 | 待人拍 |
| EX-25 | `./scripts/judge.sh nameplate-diffset \| tee \| grep -F 'from_manifest=2 from_reply=3 added=skill:tdd'` | `added=skill:tdd`（清单与应答的差） | T5 | 待人拍 |
| EX-26 | 干净：`./scripts/judge.sh fd-no-handle-value \| tee \| grep -F 'hits=0'`；脏（`--dirty`）：`./scripts/judge.sh fd-no-handle-value --dirty \| tee \| grep -F 'hits=1 fd=7 target=/opt/Nous-skills'` **必须命中** | `hits=0` / `hits=1 fd=7 target=/opt/Nous-skills` | T5 | 待人拍 |
| EX-27 | `./scripts/judge.sh deliver-twice \| tee \| uniq \| wc -l` = 1 且 `./scripts/judge.sh deliver-twice \| tee \| grep -F 'files=2 first=SKILL.md'` | `files=2 first=SKILL.md` | T5 | 待人拍 |
| EX-28 | `./scripts/judge.sh crash-once \| tee \| grep -F 'pi_system=12 diagnostics=1 segment=skills reason=exit-code-1'` | `segment=skills reason=exit-code-1` | T5 | 待人拍 |
| EX-29 | `./scripts/judge.sh third-failure-breaker \| tee \| grep -F 'delta=1 null_key=skills commits=1'` | `null_key=skills commits=1` | T5 | 待人拍 |
| EX-31 | `./scripts/judge.sh diagnostic-dedup \| tee \| grep -F 'pi_extension_commits=1 memory_count=10 log_lines=1 manifestHash=5930'` | `manifestHash=5930`（清单哈希前 4 位，完整 64 位见审计 entry）+ `memory_count=10` + `log_lines=1` | T5 | T5 首次跑 · 2026-11-17 |
| EX-32 | `./scripts/judge.sh always-crash \| tee \| grep -F 'state=tripped_process crash=2 segment_state=normal'` | `state=tripped_process crash=2` | T5 | 待人拍 |
| EX-34 | `./scripts/judge.sh concurrency-20 \| tee \| grep -F 'inflight=16 queued=4 diag=排队中，等 4 次'` | `queued=4` + `排队中，等 4 次` | T5 | 待人拍 |
| EX-35 | `./scripts/judge.sh output-1mb \| tee \| grep -F 'killed=1 reason=extension_crash bytes=1048576'` | `bytes=1048576` | T5 | 待人拍 |
| EX-36 | `./scripts/judge.sh rss-256mb \| tee \| grep -F 'killed=1 reason=extension_crash rss=268435456'` | `rss=268435456` | T5 | 待人拍 |
| EX-37 | `./scripts/judge.sh single-reload-entry \| tee \| grep -F 'hits=1 path=crates/app/nous-ext/src/runtime.rs'` | `hits=1` + `path=crates/app/nous-ext/src/runtime.rs` + `line=1105`（`grep -rn 'fn reload(' crates/app` 恰好一行） | T5 | T5 首次跑 · 2026-11-17 |
| EX-38 | `./scripts/judge.sh reload-triggers \| tee \| grep -F 'tests=3 names=watch,head,explicit'` | `names=watch,head,explicit` | T5 | 待人拍 |
| EX-39 | `./scripts/judge.sh callid-across-reload \| tee \| grep -F 'call_before=1001 call_after=1002 seq_reset=0'` | `call_before=1001 seq_reset=0` | T5 | 待人拍 |
| EX-40 | `./scripts/judge.sh idle-cpu \| tee \| grep -F 'seconds=60 jiffies=0 groups=0'` | `seconds=60` + `jiffies=0`（三个常驻扩展空闲 60 秒的 utime+stime 增量，实测 0）+ `groups=0` | T5 | T5 首次跑 · 2026-11-17 |
| EX-41 | `./scripts/judge.sh reload-fails-keeps-old \| tee \| grep -F 'serving_version=0.1.0 reload=0'` | `serving_version=0.1.0`（旧版仍在服务） | T5 | 待人拍 |
| EX-42 | `./scripts/judge.sh partial-write \| tee \| grep -F 'reloads=0 seen=# 前半'` | `seen=# 前半`（半截内容） | T5 | 待人拍 |
| EX-43 | `./scripts/judge.sh write-complete \| tee \| grep -c 'reload=1'` = 1 且 `./scripts/judge.sh write-complete \| tee \| grep -F 'content_hash=0fc5'` | `content_hash=0fc5`（写入内容前 4 位）+ `reload=1`（同一扩展恰好一次） | T5 | T5 首次跑 · 2026-11-17 |
| EX-44 | `./scripts/judge.sh parse-fails \| tee \| grep -F 'serving=old diag=frontmatter 缺 name file=SKILL.md'` | `diag=frontmatter 缺 name` | T5 | 待人拍 |
| EX-45 | `./scripts/judge.sh out-of-cwd-write \| tee \| grep -F 'errno=13 target_exists=0'` | `errno=13`（EACCES） | T5 | 待人拍 |
| EX-46 | 干净：`./scripts/judge.sh environ-whitelist \| tee \| grep -F 'hits=0'`（`hits=0`）；脏（`--dirty`）：`./scripts/judge.sh environ-whitelist --dirty \| tee \| grep -F 'hits=1 file=environ'` **必须命中** | `hits=0` / 脏：多传 `HOME` 必红 | T5 | 待人拍 |
| EX-47 | 干净：`./scripts/judge.sh fd-no-db \| tee \| grep -F 'hits=0'`（`hits=0`）；脏（`--dirty`）：`./scripts/judge.sh fd-no-db --dirty \| tee \| grep -F 'hits=1 file=agent.sqlite'` **必须命中** | `hits=0` / 脏：fd 里开 `agent.sqlite` 必红 | T5 | 待人拍 |
| EX-48 | `./scripts/judge.sh byte-flip \| tee \| grep -F 'rejected=1 hash=8bc8'` | `rejected=1` + `hash=8bc8`（固定 fixtures 产物改一字节后的哈希前 4 位） | T5 | T5 首次跑 · 2026-11-17 |
| EX-49 | 干净：`./scripts/judge.sh no-toolchain-in-tree \| tee \| grep -F 'hits=0'`（`hits=0`）；脏（`--dirty`）：`./scripts/judge.sh no-toolchain-in-tree --dirty \| tee \| grep -F 'hits=1 file=rustc'` **必须命中** | `hits=0` / 脏：开 build 角色出现 `rustc` 必红 | T5 | 待人拍 |
| EX-50 | `./scripts/judge.sh triple-not-compatible \| tee \| grep -F 'rejected=1 triple=x86_64-unknown-linux-musl accepted=aarch64'` | `triple=x86_64-unknown-linux-musl` | T5 | 待人拍 |
| EX-51 | `./scripts/judge.sh audit-kinds \| tee \| grep -F 'kinds=install,uninstall,disable,breaker manifestHash=f752'` | `kinds=install,uninstall,disable,breaker` + `manifestHash=f752`（清单哈希前 4 位） | T5 | T5 首次跑 · 2026-11-17 |
| EX-52 | `./scripts/judge.sh audit-replay-10 \| tee \| grep -F 'entry_id_same=1 repeats=10 entries=4'` | `entry_id_same=1 repeats=10` | T5 | 待人拍 |
| EX-53 | `./scripts/judge.sh entries-filter \| tee \| grep -F 'kind=pi.extension entry_id=2'` | `kind=pi.extension` + `entry_id=2`（空库首跑的安装审计 entry id） | T5 | T5 首次跑 · 2026-11-17 |
| EX-54 | `./scripts/judge.sh catalog-moved \| tee \| grep -F 'api_catalog_hits=0 machines=runtime-1 triples=x86_64-unknown-linux-gnu,x86_64-unknown-linux-musl'` | `machines=runtime-1` + 兼容 triple 列表 | T1 | 待人拍 |
| EX-55 | `./scripts/judge.sh canary-cross-session \| tee \| grep -F 'canary=ZZ-9f3a-B hits=0 session=B'` | `canary=ZZ-9f3a-B hits=0` | T5 | 待人拍 |
| EX-56 | `./scripts/judge.sh disable-extension \| tee \| grep -F 'calls=0 missing=skill:tdd diag=已禁用'` | `diag=已禁用` + `missing=skill:tdd` | T5 | 待人拍 |
| EX-57 | `./scripts/judge.sh ex-id-set \| tee \| grep -F 'has_EX-81=1 has_EX-82=0'` | `has_EX-81=1 has_EX-82=0`（用 `--ids` 比，别用两段 `--list`）（自检，别指望它防大事） | T5 | 已签 · 2026-11-17（降级为保险） |
| EX-58 | `./scripts/judge.sh oversized-frame \| tee \| grep -F 'killed=1 reason=extension_crash frame_bytes=2097152 rss_growth=0'` | `frame_bytes=2097152` + `rss_growth=0` | T5 | 待人拍 |
| EX-59 | `./scripts/judge.sh nameplate-conflict \| tee \| grep -F 'conflict=skill:tdd winner=nous loser=evil-dup'` | `conflict=skill:tdd winner=nous loser=evil-dup` | T5 | 待人拍 |
| EX-60 | `./scripts/judge.sh handle-path-escape \| tee \| grep -F 'rejected=1 path=../../etc/passwd diag=1'` | `path=../../etc/passwd` | T5 | 待人拍 |
| EX-61 | `./scripts/judge.sh count-after-restart \| tee \| grep -F 'boot_id_changed=1 memory_count=0 state=degraded'` | `boot_id_changed=1 state=degraded` | T5 | 待人拍 |
| EX-62 | `./scripts/judge.sh breaker-push-sse \| tee \| grep -F 'event=entry_appended kind=pi.extension entry_id=3 loop=1'` | `entry_id=3`（空库首跑的降级提交）+ `loop=1` | T5 | T5 首次跑 · 2026-11-17 |
| EX-63 | `./scripts/judge.sh breaker-in-snapshot \| tee \| grep -F 'snapshot_has=1 kind=pi.extension entry_id=3'` | `entry_id=3`（与 EX-62 同一个；重连 snapshot 里也在） | T5 | T5 首次跑 · 2026-11-17 |
| EX-64 | `./scripts/judge.sh reload-merge-20 \| tee \| grep -F 'reloads=1 version=0.1.1 files=20'` | `reloads=1 version=0.1.1` | T5 | 待人拍 |
| EX-65 | `./scripts/judge.sh reload-two-exts \| tee \| grep -F 'reloads_a=1 reloads_b=1 inflight_b=line-3'` | `inflight_b=line-3` | T5 | 待人拍 |
| EX-66 | `./scripts/judge.sh boot-load-failure \| tee \| grep -F 'name=evil hash=8bc8 reason=哈希不符 others_ok=1 exit=0'` | `name=evil` + `hash=8bc8`（固定 fixtures 产物）+ `reason=哈希不符` + `others_ok=1` + `exit=0` | T5 | T5 首次跑 · 2026-11-17 |
| EX-67 | `./scripts/judge.sh symlink-escape \| tee \| grep -F 'skipped=leak.md diag=1 files=0'` | `skipped=leak.md` | T5 | 待人拍 |
| EX-68 | `./scripts/judge.sh truncation-deterministic \| tee \| grep -F 'files=50 truncated=1 last=f050.md runs_equal=1'` | `last=f050.md runs_equal=1` | T5 | 待人拍 |
| EX-69 | `./scripts/judge.sh uid-allocation \| tee \| grep -F 'passwd_60001=absent uids=60001,60002,60003'` | `passwd_60001=absent uids=60001,60002,60003` | T5 | 待人拍 |
| EX-70 | `./scripts/judge.sh cap-setuid \| tee \| grep -F 'caps=CAP_SETUID app_uid=0 ext_uid=60001'` | `caps=CAP_SETUID` + `app_uid=0`（本机以 root 跑；换成非 root 宿主则填实测的 uid）+ `ext_uid=60001` | T5 | T5 首次跑 · 2026-11-17 |
| EX-71 | `./scripts/judge.sh fd-exact-set \| tee \| grep -F 'fds=0,1,2,3,4'` | `fds=0,1,2,3,4`（多一个即红） | T5 | 待人拍 |
| EX-72 | 干净：`./scripts/judge.sh groups-empty \| tee \| grep -F 'hits=0'`（`hits=0`）；脏（`--dirty`）：`./scripts/judge.sh groups-empty --dirty \| tee \| grep -F 'hits=1 file=Groups: 27'` **必须命中** | `hits=0` / 脏：出现 `Groups: 27` 必红 | T5 | 待人拍 |
| EX-73 | `./scripts/judge.sh eacces \| tee \| grep -F 'errno=13 target_exists=0'` | `errno=13` | T5 | 待人拍 |
| EX-74 | `./scripts/judge.sh evil-carrier \| tee \| grep -F 'carriers=5 maps=EX-45,EX-58,EX-60,EX-67,EX-71'` | `maps=EX-45,EX-58,EX-60,EX-67,EX-71` | T5 | 待人拍 |
| EX-75 | `./scripts/judge.sh provenance \| tee \| grep -F 'builtFrom=621f4f5 buildMachine=asus hash=0056'` | `builtFrom=621f4f5`（`exts/provenance.json` 里写死的真实历史提交）+ `buildMachine=asus` + `hash=0056`（固定 fixtures 产物） | T5 | T5 首次跑 · 2026-11-17 |
| EX-76 | `./scripts/judge.sh disabled-nameplate-referenced \| tee \| grep -F 'error=会话选了 skill:tdd，提供它的扩展已被禁用 prompt_hits=0'` | `prompt_hits=0` + 具体错误原文 | T5 | 待人拍 |
| EX-77 | `./scripts/judge.sh atomic-swap-20x \| tee \| grep -F 'runs=20 half_installed=0 versions=0.1.0,0.1.1'` | `runs=20 half_installed=0` | T5 | 待人拍 |
| EX-78 | `./scripts/judge.sh canary-cross-extension \| tee \| grep -F 'canary=ZZ-9f3a-Y hits=0 ext=Y'` | `canary=ZZ-9f3a-Y hits=0` | T5 | 待人拍 |
| EX-79 | `./scripts/judge.sh canary-cross-request \| tee \| grep -F 'canary=ZZ-9f3a-B hits=0 round=2'` | `canary=ZZ-9f3a-B hits=0 round=2` | T5 | 待人拍 |
| EX-80 | `./scripts/judge.sh clock-invisible \| tee \| grep -F 'runs=5 delay=2 identical=1'` | `runs=5 delay=2 identical=1` | T5 | 待人拍 |
| EX-81 | 干净：`grep -nE 'echo .*(commit_seq\|entry_id\|hash\|version)=' scripts/judge.sh \| tee` 逐行空；脏（临时往 `judge.sh` 加一行 `echo "commit_seq=5074"`）：同一命令 **必须命中那一行** | 脏那次输出的行号（如 `scripts/judge.sh:44`） | T5 | 已签 · 2026-11-17 |

**统计（两段，不许合并）**

```
行为判据：69 条（EX-01…EX-80，去掉已合并的 30/33；含 EX-54 挂 T1）
工具判据： 1 条（EX-81，验 judge.sh 本身）
待实测哨兵：0 条（`--pending` 数得出来）
```

"**全表全 SKIP**"本身是一个信号（说明还没有任何实现）；混进一条真绿，这个信号就失真了 —— 而我们会拿它判断 T5 的进度。
**哨兵待实测**：0 条。15 条（EX-14 · 20 · 21 · 31 · 37 · 40 · 43 · 48 · 51 · 53 · 62 · 63 · 66 · 70 · 75）已在 **T5 首次跑（2026-11-17）** 把实测值写死进本表；判据里不再有 `X` / `<任意>` / `约等于`。
**已合并删除**：`EX-30`（并入 EX-29）· `EX-33`（并入 EX-32）。**编号留空不复用** —— 防止历史引用指到别处。
**全部 79 条都含机械断言**：散句断言 = **0**（每行都带 `grep -F` / `wc -l` / `diff`，且哨兵是具体字面量）。负向判据（EX-01 · 05 · 12 · 26 · 46 · 47 · 49 · 72 · 81）成对：干净必绿 + `--dirty` 必红。

## 附录 B · 不许做清单

**真相层 `crates/kernel/`**：`K1` 进程 · `K2` 网络 · `K3` 直接时钟（含注释） · `K4` 路由 · `K5` 产品词（baseline 只减不增） · `K6` 依赖 providers · `K7` 解释 skills/template 语义 · `K8` 文件锁
**能力层 `crates/platform/`**：`L1` 被真相层依赖（方向：providers，不是 nous-ai） · `L2` 拥有监听 · `L3` 碰数据库 · `L4` 无 IO 替身 · `L5` 自报为扩展 · `L6` 工具子进程绕开 `os_env`（全仓进程创建点恰好两个）
**应用层 `crates/app/`**：`A1` 第二个扩展 spawn 点 · `A2` 第二个监听 · `A3` 第二份真相（旁路日志/影子库） · `A4` 缓存段文本当真相 · `A5` 文件锁 · `A6` 诊断进提交（每轮一条，会把提交日志刷成一万条） · `A7` 整仓重载（一次只影响一个扩展） · `A8` 开机装载失败只打 stderr · `A9` 代读跟随符号链接且不校验 · `A10` 遍历顺序依赖目录返回顺序 / 截断不确定 · `A11` spawn 时不显式关 fd（靠默认继承） · `A12` 直接写目标目录（不原子 rename） · `A13` 静默删掉会话里失效的名牌（或往提示词里塞“不可用”）
**扩展**：`E1` 自己 open 路径 · `E2` 直连 provider · `E3` 直写库 · `E4` 看见别的会话 · `E5` 后台长驻定时器 · `E6` 自绘 UI · `E7` 清单自报超时/并发/内核版本 · `E8` 写终态五字面量（作为字段值） · `E9` 越界写 · `E10` fail-closed 段 · `E11` 申请第四种写形状 · `E12` 一轮内多次往返 · `E13` 名牌带默认值

`K*`/`L*`/`A*` 的断言归内核 spec 与 `seams-open.md`（S1–S11，已签）；`E*` 的断言归本文件附录 A（EX-01 起）。

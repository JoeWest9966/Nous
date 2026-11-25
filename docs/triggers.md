# 触发器 / Triggers

> [中文](#zh) · [English](#en)

---

<a id="zh"></a>
## 1 一句话

> **触发器 = 一份具名的「启动配方」（模板 + 运行参数）加一张「谁允许点火」的许可；
> 每次点火按 `(code, fire_at)` 生成一个幂等的启动事件，最终落成一次普通的 `submit_message`。**

三个词，别混：

| 词 | 是什么 | 不是 |
|---|---|---|
| **配方**（`Template` + 运行参数） | 点火之后跑的那一轮该怎么起：用哪个模板、哪个模型、在哪个工作目录 | 不是"提示词随便塞一段" |
| **点火许可**（`fires_allowed_by`） | 谁可以点它：`cron` / `ext:<扩展 code>` / `agent:<agent>` / `human` | 不是"谁能改它"（那是权限系统的事） |
| **点火事件**（`Fire`） | 一次具体的启动：`(trigger_code, fire_at, fired_by)` | 不是"一个定时任务" |
| **落点**（`target`） | 这一轮写进哪个会话：新开一个 / 写回指定的那个 | 不是全局策略，**每个触发器自己定** |
| **策略**（`policy`） | 上一轮还没完时再点火怎么办：跳过并记账 / 排队 | 同上，**每个触发器自己定** |

**规则用数据表达，不用代码表达。** `spec` 是一段有封闭键集合的 JSON（`schedule` / `on` / …），
**解释它的是点火者**（时钟、扩展、agent）。触发器里不许嵌可执行脚本 —— 那是 `ext` 的活，
两个机制都"能跑用户代码"等于两套沙箱、两套版本闸。

**每个触发器都是一个独立的商品**：自己的 `code`、自己的规则、自己的模板、自己的落点和策略。
所以你才能"了解它们各自的工作情况"——一个触发了多少次、跳过几次、失败几次，账都在它自己名下。

## 2 它不是什么

- **不是调度器。** 触发器**不知道时间**。定时只是「点火许可」里的一种条件实现（`cron`），
  由别人判断"到点了"再来点火。把 cron 塞进触发器本体，就等于以后加事件触发要拆一遍。
- **不是新的执行路径。** 点火最终就是一次 `submit_message`，所以幂等、审计、取消、限流、
  SSE 全部**继承**，一行都不用重写。它是第 4 个调用者，不是第 4 套机制。
- **不是 workflow / DAG 引擎。** 一次点火只起**一轮**。要"跑完 A 再跑 B"，用子任务
  （`status = waiting` + `JoinPolicy`），那是另一个机制，不许塞进触发器。
- **不是新表。** Nous 侧只要两张：`triggers`（配方 + 许可）、`fires`（每次点火的账）。
- **不是全局策略。** 落点、并发、重试、宽限期全部写在**触发器自己身上**。没有"Nous 全局默认点火行为"
  这种东西——有默认值，但没有一处全局开关能改掉所有触发器。
- **模板不能自己点火。** 模板是零件，触发器是成品。**"终极模板"不是触发器之外的东西，它就是触发器本身**
  （模板 + 规则 + id）。所以模板上永远不该出现 `enabled` / `next_fire_at` 这类字段。

## 3 一个真实场景

> **一句话示例**：一个 pi 风格的**子代理扩展**，在它自己的规则命中时（某版里写死的那个开关），
> 点燃 `nightly_digest` 触发器；触发器带着自己指定的模板起一轮，结果落成一次独立的会话。

> 我以后写一个扩展（类似 pi 的子代理扩展）：这个扩展基于某种规则，直接点燃我的某个触发器。
> 那个触发器本身已经指定了模板，而且它只能被某种情况触发 —— 也就是某版里写死的那个开关。

拆成技术动作为三步：

```rust
// 1 · 扩展判断条件成立（它自己的逻辑，Nous 不管）
// 2 · 点火：一次点火 = 一行账，唯一键挡住重复
fires.insert(Fire { trigger_code: "nightly_digest", fire_at: now_ms,
                    fired_by: Actor::agent("ext:pi-subagent"), state: Pending })?;
// 3 · 起一轮：内容来自模板，不是人现写的
durable.submit_message(SubmitRequest {
    request_id: format!("trigger:nightly_digest:{now_ms}"),   // 幂等锚
    conversation: target,                                     // 见 §7 的待定项
    prompt: template.render(&fire)?,                          // 配方决定内容
})?;
fires.mark_done(...)?;
```

**"只能被某种情况触发"就是 `fires_allowed_by`**：扩展不在这个列表里，点火直接被拒
（拒绝要有稳定 slug，跟 `refuse:` 一个路子：`fire-denied: <code> by <actor>`）。
这是权限系统的**第一个真实用例** —— 不是设计出来的，是触发器长的第一个需求。

## 4 设计思路（五条，每条都能反着做坏）

1. **幂等靠 `(code, fire_at)`，不靠锁。** `fire_at` 是"这次点火的身份"，不是"现在几点"。
   所以断电补跑、重放、重复调用全部安全——`fires` 的主键挡住第二次。
2. **点火许可是权限的第一块砖。** `fires_allowed_by` 里装的是 `Actor`（`#54` 的三值 kind +
   `trigger:` 前缀的 id）。这条一落地，"AI 主体拿受限钥匙"就有了第一个真实场景，不用等大设计。
3. **模板是引用，不是内联。** 触发器存 `template_ref`（指向 preset / document / skill），
   不存正文。否则改一次模板要改 N 个触发器，其中必有一个被忘掉。
4. **调度是"条件"的一种实现，不进本体。** 今天 `cron`，明天 `event`，后天 `ext:xxx` 自己判断——
   加新触发源 = 加一个会点火的调用者，触发器本身一行不改。
5. **不做编排。** 一次点火只起一轮。"跑完再跑"、"等三个都完"是子任务（`waiting`/`completing`）的活。
   把编排塞进触发器，就是给自己造第二套工作流引擎。
6. **一切规则写在自己身上。** 点火源（`fires_allowed_by`）、落点（`target`）、策略（`policy`）、
   模板（`template_ref`）全是触发器自己的列。这样"复制一个触发器 = 复制一件商品"，
   导出成一份文件、导入即建，不用再去别处找它的隐藏配置。
7. **零件不点火，成品才点火。** 模板可以被很多触发器引用，但它自身没有生命周期：
   没有 `enabled`、没有 `next_fire_at`、不进 `fires` 表。想让它"跑起来"，得有一个触发器指向它。

## 5 失败模式（先想清楚，别上线再发现）

| 现象 | 谁的问题 | 怎么处理 |
|---|---|---|
| 点了火，消息没入库 | 跨库写入断在中间 | `fires.state='pending'` 超期 → `orphaned` → 重放（幂等，安全）。**顺序必须是先写 `nous.db` 再调 durable** |
| 同一次点火跑了两次 | 没有唯一键 | 被 `(code, fire_at)` 挡住，`fires` 行数不增 |
| 跑了，但没人看 | 不是 bug | 合法的结果。落点决定它出现在哪（见 §7） |
| 模板被删/改名 | 引用悬空 | 点火失败要留账：`fires.state='orphaned'` + `note='template-missing'`，不许静默 |
| 触发太多把机器打满 | 没有限流 | 单触发器 `serial`：上一轮没结束就**跳过本次**并记账（`skipped`），不排队 |

## 6 判据（将来落到哪）

- `cargo test -p nous-store`：唯一键去重、`reclaim` 只捞该捞的、`code` 不可变 + 墓碑。
- `cargo test -p nous-triggers`：`fires_allowed_by` 拒绝未授权 actor（负向哨兵：把列表判断去掉 → 红）。
- `scripts/verify.sh`：Nous 侧库里 grep 不到 durable 的表名（域分离，双向）。
- **人工哨兵**：真扩展点火一次，看 `fires` 多一行 + 会话里多一轮（自动化点不了"真扩展"）。

## 7 边界与待定

| 关系 | 规矩 |
|---|---|
| `pi durable` | 触发器**调**它（`submit_message`），它**不知道**触发器存在 |
| `nous-store`（`#56`） | `triggers` / `fires` 住在 `nous.db`，独立迁移链 |
| `permissions` | `fires_allowed_by` 是它的第一个用例；完整 ACL 后做 |
| `NousDesk` | 触发器在 Desk 上是什么样（列表？灰入口？）**待定**：先只做后端，UI 等落点定了再说 |

**已定**（2026-11-18）：

1. **落点**：**不是全局策略，是触发器自己的 `target` 列** —— `{kind:"new"}` 新开一个会话，
   或 `{kind:"conversation", id}` 写回指定的那个。默认 `new`（商品卖出去是独立订单，
   不塞进已有订单）。
2. **并发策略**：**同样是触发器自己的 `policy` 列** —— 默认 `serial`（上一轮没完就跳过本次并记
   `skipped`）；要排队就自己声明。Nous 不提供"全局排队队列"。
3. **模板引用**：第一版**只支持 preset**（`{kind:"preset", ref}`）。document / skill 等 `#48` 的
   `documents` 定稿再加，否则引用路径会跟着改。

**仍待定**：Desk 上触发器长什么样（列表？灰入口？"系统发起"的会话怎么显示）—— 那是外观，
按规矩要先出 HTML demo，不在这里定。

## 8 开发准则（**由代码强制，不是靠文档**）

这一章是「写触发器的人必须遵守的东西」，也就是 Magento 2 那种「开发插件的准则」。
**准则不写在文档里靠自觉，它写在一个校验函数里**，创建时就拒绝，跑不进去。

```rust
Trigger::new(NewTrigger) -> Result<Trigger, TriggerError>   // 唯一入口，绕过它拿不到 Trigger
```

| # | 准则 | 违反时的错误 slug |
|---|---|---|
| 1 | `code` 匹配 `^[a-z][a-z0-9_]{2,63}$`，全局唯一 | `code-shape` / `code-taken` |
| 2 | `code` 创建后不可变；删除留墓碑，code 永不复用 | `code-immutable` / `code-tombstoned` |
| 3 | 必须有 `template_ref`，且指向存在的模板 | `template-missing` |
| 4 | `fires_allowed_by` **不得为空**（空 = 谁都不能点 = 死触发器） | `no-firing-source` |
| 5 | `target` / `policy` **落库时写实**，不许留 `null` 让读者猜默认值 | `explicit-policy-required` |
| 6 | `spec` 的键集合是**封闭**的：未知键**拒绝**（见下） | `unknown-spec-key` |
| 7 | `spec` 必须带 `spec_version`，将来迁移靠它 | `spec-version-missing` |
| 8 | `spec` 里不许出现"调另一个触发器"（编排禁止） | `orchestration-not-a-trigger` |
| 9 | 触发器行上不许有 `last_status` / `retry_at` / `failure_count`（那是 `fires` 的账） | `ledger-on-definition` |

**第 6 条要单独说，因为它看起来违反项目规矩。** 我们的规矩是「未知字段用 `Value` 收着原样透传、
不许 `deny_unknown_fields`」—— 那条针对的是**协议**（上游加字段我们不能编译不过）。
而 `spec` 是**我们自己的本地配置**，没有上游、没有前向兼容压力，未知键只可能是打错字
（`schedul` vs `schedule`）。本地配置必须 fail-fast，否则打错的键会被静默忽略，
表现成"触发器不工作但也不报错"。**判据要能红**：故意写 `scheduel` → 必须拒绝。

**准则怎么被钉死**（三条一起才算强制）：

1. `Trigger::new` 是唯一构造入口 —— 绕过它就构造不出 `Trigger`（类型层面封住）。
2. `crates/Nous/Store/tests/trigger_conformance.rs` 是一张**接受/拒绝表**：每条准则至少一条拒绝用例
   （跟上游 `storage-conformance.ts`、我们的 `ext-probe/cases_*.rs` 同一路子）。
3. 错误 slug 是**封闭集合**、唯一、且有单测覆盖 —— Desk 只许按 slug 分支、不许解析人话文案。

## 9 将来（不在第一版）

- **导出 / 导入**：触发器能导出成一份文件、导入即建。"商品"是可复制的；复制必须只靠这一份文件，
  这就反向要求 §4 的「一切规则写在自己身上」。
- 触发器类型清单（cron / event / agent-fired 各要什么字段）按第一条真实需求再加，不预先设计。

## 10 反悔点

- 如果发现开始给触发器加"条件语法"（`when: …` 表达式解析器），**停**：那是把调度器写进本体，
  应该由一个扩展去做判断。
- 如果发现 `triggers` 表开始长 `last_status` / `failure_count` / `retry_at` 这类列，**停**：
  那些是 `fires` 的账，不是配方的属性。
- 如果发现有人在触发器里写"跑完调另一个触发器"，**停**：那是编排，去子任务。
- 如果发现 `triggers` 之外还冒出一份"全局点火配置"（默认落点/默认并发/默认宽限期），**停**：
  那会让"复制一个触发器"不再等于"复制一件商品"。
- 如果发现模板上长了 `enabled` / `next_fire_at` / `last_fired_at`，**停**：模板是零件，没有生命周期。

---

<a id="en"></a>
## English

### 1 In one sentence

> **A trigger is a named "launch recipe" (template + runtime parameters) plus a permit for
> *who may fire it*. Each firing produces one idempotent launch event keyed by `(code, fire_at)`,
> and ultimately becomes nothing more than an ordinary `submit_message` call.**

### 2 What it is not

- **Not a scheduler.** Triggers do not know about time. Cron is merely one *firing condition*
  (`fires_allowed_by = ["cron", …]`). Putting cron inside the trigger means you will have to tear
  it apart the day an event source appears.
- **Not a new execution path.** Firing ends in `submit_message`, so idempotency, audit, cancellation,
  rate limiting and SSE are all inherited. It is a fourth *caller*, not a fourth mechanism.
- **Not a workflow/DAG engine.** One firing starts exactly one turn. "Run A then B" is a subtask
  concern (`waiting` + `JoinPolicy`).
- **Not a new table set.** Two tables on the Nous side: `triggers` (recipe + permit) and
  `fires` (the ledger of firings).
- **Not a global policy.** Placement (`target`), concurrency (`policy`), retries and grace periods
  live on the trigger row itself. Defaults exist; no global switch overrides all triggers.
- **Templates cannot fire.** A template is a part, a trigger is the product. The "ultimate template"
  *is* the trigger (template + rules + id) — so a template must never grow `enabled` / `next_fire_at`.

### 2b Rules are data, not code

`spec` is a JSON object with a closed key set (`schedule` / `on` / …). **The firing source interprets it**
(clock, extension, agent). A trigger must never embed executable code — that is what `ext` is for;
two mechanisms that both "run user code" means two sandboxes and two version gates.

### 3 A real scenario

> **In one sentence**: a pi-style *sub-agent extension* fires `nightly_digest` when its own rule
> hits (the switch committed in a specific version); the trigger starts one turn using the template
> it already carries, and the result lands as a standalone conversation.

An extension (a pi-style sub-agent extension) decides some condition holds and fires a trigger.
The trigger already carries a template, and it may only be fired by certain sources — a switch
committed in a specific version.

```rust
fires.insert(Fire { trigger_code: "nightly_digest", fire_at: now_ms,
                    fired_by: Actor::agent("ext:pi-subagent"), state: Pending })?;  // 1 ledger row
durable.submit_message(SubmitRequest {
    request_id: format!("trigger:nightly_digest:{now_ms}"),   // idempotency anchor
    conversation: target,
    prompt: template.render(&fire)?,
})?;
```

"A switch committed in a version" is `fires_allowed_by`. An actor that is not listed is refused
with a stable slug (`fire-denied: <code> by <actor>`), same discipline as `refuse:`.

### 4 Design principles

1. **Idempotency by key, not by lock** — `fire_at` is the *identity of this firing*, not "the clock".
2. **The firing permit is the first brick of permissions** — it takes an `Actor` (`#54`).
3. **Templates are referenced, never inlined** — one template edit must not require editing N triggers.
4. **Scheduling is one implementation of a condition** — new firing sources add callers, not trigger code.
5. **No orchestration** — one firing, one turn. Composition belongs to subtasks.
6. **Every rule lives on the trigger itself** — firing sources, `target`, `policy`, `template_ref`.
   Copying a trigger must copy a whole product, exportable as one file.
7. **Parts do not fire, products do** — a template may be referenced by many triggers but owns no
   lifecycle: no `enabled`, no `next_fire_at`, never a row in `fires`.

### 5 Failure modes

| Symptom | Cause | Handling |
|---|---|---|
| Fired but nothing landed | cross-database write interrupted | `pending` → `orphaned` after grace → replay (idempotent). **Write `nous.db` first, then durable.** |
| Same firing ran twice | missing unique key | `PRIMARY KEY (trigger_code, fire_at)` |
| Ran, nobody read it | not a bug | legitimate outcome; placement decided in §7 |
| Template deleted | dangling reference | `orphaned` + `note='template-missing'`, never silent |
| Too many firings | no throttle | per-trigger `serial`: skip this firing if the previous turn is still running, record `skipped` |

### 5b Decided (2026-11-18)

1. **Placement** is a per-trigger `target`: `{kind:"new"}` or `{kind:"conversation", id}`; default `new`.
2. **Concurrency** is a per-trigger `policy`: default `serial` (skip this firing while the previous
   turn runs, record `skipped`). No global queue.
3. **Template references** are `preset` only in v1; document/skill wait for `#48`.
   Still open: how triggers look in NousDesk (that is appearance → needs an HTML demo first).

### 6 Authoring contract — enforced by code, not by prose

```rust
Trigger::new(NewTrigger) -> Result<Trigger, TriggerError>   // the only constructor
```

| # | Rule | Error slug |
|---|---|---|
| 1 | `code` matches `^[a-z][a-z0-9_]{2,63}$`, globally unique | `code-shape` / `code-taken` |
| 2 | `code` immutable; deletes leave a tombstone, codes are never reused | `code-immutable` / `code-tombstoned` |
| 3 | `template_ref` required and resolvable | `template-missing` |
| 4 | `fires_allowed_by` must not be empty (empty = dead trigger) | `no-firing-source` |
| 5 | `target` / `policy` stored explicitly, never `null` | `explicit-policy-required` |
| 6 | `spec` keys are **closed**; unknown keys are rejected | `unknown-spec-key` |
| 7 | `spec_version` required | `spec-version-missing` |
| 8 | no "call another trigger" inside `spec` | `orchestration-not-a-trigger` |
| 9 | no `last_status` / `retry_at` / `failure_count` on the definition row | `ledger-on-definition` |

Rule 6 is a deliberate exception to "never `deny_unknown_fields`": that rule protects **protocol**
compatibility with the engine; `spec` is **our own local config**, where an unknown key can only be a
typo (`scheduel`) and must fail fast instead of being silently ignored.

Enforcement = three things: (1) `Trigger::new` is the only constructor; (2) a conformance table in
`tests/trigger_conformance.rs` with at least one rejection case per rule; (3) error slugs are a closed
set, unique, unit-tested — clients branch on slugs, never on prose.

### 7 Future (not v1)

Export/import: a trigger must be exportable as a single file and importable as-is — copying a product
must rely on that file alone. Firing-source types (cron / event / agent-fired) are added per first
real need, not designed upfront.

### 8 Reversal points

- Adding a condition *expression syntax* to triggers → stop; that belongs to an extension.
- `triggers` growing `last_status` / `failure_count` / `retry_at` → stop; those are `fires` facts.
- A trigger whose body calls another trigger → stop; that is orchestration.
- A "global firing config" appearing next to `triggers` → stop; copying a trigger would stop
  meaning copying a product.
- `enabled` / `next_fire_at` / `last_fired_at` on a template → stop; templates have no lifecycle.

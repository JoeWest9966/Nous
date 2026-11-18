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

## 2 它不是什么

- **不是调度器。** 触发器**不知道时间**。定时只是「点火许可」里的一种条件实现（`cron`），
  由别人判断"到点了"再来点火。把 cron 塞进触发器本体，就等于以后加事件触发要拆一遍。
- **不是新的执行路径。** 点火最终就是一次 `submit_message`，所以幂等、审计、取消、限流、
  SSE 全部**继承**，一行都不用重写。它是第 4 个调用者，不是第 4 套机制。
- **不是 workflow / DAG 引擎。** 一次点火只起**一轮**。要"跑完 A 再跑 B"，用子任务
  （`status = waiting` + `JoinPolicy`），那是另一个机制，不许塞进触发器。
- **不是新表。** Nous 侧只要两张：`triggers`（配方 + 许可）、`fires`（每次点火的账）。

## 3 一个真实场景

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

**仍待定（不定就不实现）**：

1. **点火落在哪个会话**：(a) 新会话（"张三的去处"，Desk 列表要能区分系统发起）/
   (b) 写回固定会话 / (c) 只落库不通知。**倾向 (a)**。
2. **`serial` 之外的策略**：要不要 `queue`（排队）？**倾向：不做**，排队等于自己造队列。
3. **模板引用到哪**：preset / document / skill 三种都支持，还是先只支持 preset？**倾向：先 preset**。

## 8 反悔点

- 如果发现开始给触发器加"条件语法"（`when: …` 表达式解析器），**停**：那是把调度器写进本体，
  应该由一个扩展去做判断。
- 如果发现 `triggers` 表开始长 `last_status` / `failure_count` / `retry_at` 这类列，**停**：
  那些是 `fires` 的账，不是配方的属性。
- 如果发现有人在触发器里写"跑完调另一个触发器"，**停**：那是编排，去子任务。

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

### 3 A real scenario

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

### 5 Failure modes

| Symptom | Cause | Handling |
|---|---|---|
| Fired but nothing landed | cross-database write interrupted | `pending` → `orphaned` after grace → replay (idempotent). **Write `nous.db` first, then durable.** |
| Same firing ran twice | missing unique key | `PRIMARY KEY (trigger_code, fire_at)` |
| Ran, nobody read it | not a bug | legitimate outcome; placement decided in §7 |
| Template deleted | dangling reference | `orphaned` + `note='template-missing'`, never silent |
| Too many firings | no throttle | per-trigger `serial`: skip this firing if the previous turn is still running, record `skipped` |

### 6 Reversal points

- Adding a condition *expression syntax* to triggers → stop; that belongs to an extension.
- `triggers` growing `last_status` / `failure_count` / `retry_at` → stop; those are `fires` facts.
- A trigger whose body calls another trigger → stop; that is orchestration.

# wire — 我们和 qrd3wh 之间的线

引擎是 `qrd3wh`（Nous 的 session 块）。这份抄的是**此刻**的协议，
字段是从引擎 `server.js` 和真实消费方 `bjes6b` 里读出来的，不是文档里的。

**这一份会漂。** 引擎加字段是常态。我们只依赖下面「我们依赖」列出来的字段，
其余字段一律 `serde_json::Value` 收着透传，不碰、不 `deny_unknown_fields`。

---

## 通用

- 会话 id 是 **数字**（不是字符串）。`GET /v1/sessions/:id/...` 里的 `:id` 是数字。
- 列表里 `busy` 是 `"queued" | "running" | <task state kind> | null`，不是布尔。
- 错误形状统一 `{ error: string }`，状态码跟着语义走（400 / 404 / 409 / 500…）。

## 我们用到的端点

### GET /v1/sessions

会话**树**（不是列表）。返回 `{ items: [...] }`，**新的在前**。

每条的形状：

```
{ id, name, parent: {id, at} | null, owner, model: "provider/modelId",
  thinkingLevel, thinkingLevels: string[], skills: string[], template: string|null,
  readOnly: bool, busy, last: {id, kind, text} | null }
```

我们依赖：`id` `name` `parent` `model` `thinkingLevel` `readOnly` `busy` `last`。

- `parent != null` = 这是一条 **fork 分支**。bjes6b 手机端把它全滤掉，只认主线。
  桌面端要不要显示分支是产品决定，别在客户端里猜 —— 真要做 fork 走 `POST /v1/sessions/:id/fork`。
- `last` 是最后一条 entry 的摘要（`{id, kind, text}`），用来画列表预览。它可能为 null。
- `skills` / `template` / `thinkingLevels` / `extensions` 是**生效值**，不是请求值 —— 拿来画勾，
  别自己猜。引擎改这些字段，列表预览不受影响，只有详情面板受影响。

### GET /v1/sessions/:id/entries

历史。返回 `{ items: [{id, kind, text, parent}], next }`，**新的在前**，客户端要翻正。

- `kind` 有 `pi.user` / `pi.assistant` / `pi.system` / `pi.tool-result` 等。桌面端至少处理
  `pi.user`（右气泡）和 `pi.assistant`（左气泡），其余降级显示，别崩。
- `parent` 是 entry 的父指针，和会话树的 `parent` 不是一回事。
- `next` 是分页游标，`?limit=` 上限 1000。

### POST /v1/sessions/:id/messages

body `{ content, requestId?, waitMs?, whenBusy? }`。

- `requestId` 是幂等锚：重连重发同一句拿到同一次提交，不会答两遍。**桌面端一定要传**。
- 不传 `waitMs` 时立即回 `202 { submissionId, status: "queued" }`，真正的流走 SSE。
  桌面端就按这条来：发消息 → 拿 202 → 靠 SSE 收内容。别用 `waitMs` 等同步。

### GET /v1/sessions/:id/events —— SSE

连上先推一帧 `snapshot`，然后是增量事件。**这是桌面的心跳，也是最容易写坏的一层。**

事件类型（`event.type`）：

| type | 我们依赖什么 | 备注 |
|---|---|---|
| `snapshot` | `generation?.message` | 重连第一帧。有正在生成的消息就续上，没有就只当「连上了」 |
| `message_start` | `message.role === "assistant"` | user/system 也发，只有 assistant 该开口一个气泡 |
| `message_update` | `changes[]` | 见下 |
| `message_end` | `entry.kind === "pi.assistant"` 时收尾 | 助手气泡结束，把攒的字定稿 |
| `entry_appended` | `entry.kind` | 别的标签页 / 客户端加的东西；`pi.user` `pi.tool-result` 才补 |
| `submission` | `record.status === "unanswered"` 时弹错误 | `record.reason` / `record.detail` 是「没答上来」的原因 |
| `task_failed` | `message` | 任务挂了，弹一句 |
| `tool_execution_start` | `toolCallId` `toolName` | 开一个工具卡片 |
| `tool_execution_update` | `toolCallId` `output` | `output.append` / `output.set` 追加 / 覆盖 |

`message_update` 的 `changes[]` —— **全集来自 pi-durable 的 `MessageChange`**
（`node_modules/@earendil-works/pi-durable/dist/harness/events.d.ts`），不是猜的：

```
{ type: "text_start" | "thinking_start" | "toolcall_start", contentIndex, block }
{ type: "text_delta" | "thinking_delta", contentIndex, delta }
{ type: "toolcall_delta", contentIndex, path, delta }
{ type: "block", contentIndex, block }     → 整块替换，该块后续变化都在这一批里
{ type: "message", message }               → 整条替换，以它为准
```

**只在 `text_delta` 上动字符串是不够的**：块的开头走 `text_start`（`block.text` 已有全文），
跳过它就等于把第一段吞掉。`thinking_*` 是思考不是正文，别往气泡里塞。

其他事件的全集（同一个 d.ts 里的 `AgentEvent`）还有 `run_start` / `run_end` / `turn_start` /
`turn_end` / `inbox_update` / `agent_changed` / `usage_changed` / `auto_retry_start` /
`auto_retry_end` / `deferred_poll` / `tool_execution_end` / `compaction_start` / `compaction_end`。
我们不认识的走 `Other`（`#[serde(other)]`），加事件不该让我们编译失败。

**合帧铁律**：`text_delta` 只改内存里的 `Map<contentIndex, String>`，16ms 定时器读一次再
`cx.notify()`。一个 token 一次 notify 会在 `Entity<T>` 不匹配报错之前，先把 CPU 烧穿。

### POST /v1/sessions/:id/abort

返回 `{ status: "aborted" }`。当前这轮生成停下。

### GET /v1/device

`{ self, origin, node, startedAt, model, faux, data, catalog, available }`。

我们依赖：`model`（默认模型）+ `faux`（假 provider 标志，本地验流式时用）。About 面板显示引擎版本用 `node`。

### GET /v1/selection · POST /v1/selection

`{ id }` 或 `{ id: null }` —— 「现在盯着哪个对话」。桌面端三栏同屏时，左栏高亮当前会话就用它，
**别为左栏开 N 条 SSE**。SSE 只连当前会话，其余靠 `GET /v1/sessions` 低频轮询。

### GET /v1/presets/:section

模板（通讯录）那一页的数据。`section` 写死 `Session-Template`。

返回 `{ section, title, repo, presets: [...], archived, categories, ... }`。

每条 preset：

```
{ name, summary, description, category, spec, specError, tracked, dir, resolved }
```

我们依赖：`name` `summary` `specError` `tracked` `resolved`。

- `specError` → 这条自己写坏了，红。
- `resolved.*.state === "invalid"` → 引用失效，现在开不了。
- `tracked === false` → 盘上有、仓里没。
- **能否启动的判据全是引擎算好的**，客户端不自己拼规则。加了规则就等于可以跟引擎吵起来。
- 能开的行：`POST /v1/sessions { preset: "Session-Template/<name>", name }`，`name` 带时间。

---

## 引擎改了会怎么坏（按我们依赖的字段）

| 引擎改动 | 我们会怎样 |
|---|---|
| 删 `busy` / 换成布尔 | 列表忙闲角标消失或全红 |
| 删 `last` / 换形状 | 列表预览空了 |
| `parent` 语义变 | fork 过滤逻辑失效 |
| SSE 事件改名 / `changes[]` 换结构 | 气泡不更新，流式死掉，症状是「能发出去但一个字不出来」 |
| `requestId` 幂等被去掉 | 重发会答两遍 |
| `/v1/selection` 语义变 | 三栏高亮错位 |

所以：启动时打 `GET /v1/device` 记引擎版本，About 里显示，出问题先对版本。

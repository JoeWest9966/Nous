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
  readOnly: bool, busy, updatedAt: number | null, last: {id, kind, text} | null }
```

我们依赖：`id` `name` `parent` `model` `thinkingLevel` `readOnly` `busy` `updatedAt` `last`。

- `updatedAt` 是这条会话**最后一次落库**的时刻，epoch **毫秒**，可能是 `null`。
  `null` 不是 bug：引擎只记「自己看着它写的那些」，重启前的老会话就是空的 ——
  显示相对年龄（5m / 3h / 2d）就用它，**别用 `last.id` 假装时间**，也别拿文件 mtime 冒充
  （拷贝一份盘会让每条会话都变成「刚刚」）。

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
| `snapshot` | `generation?.message`；`entries[].model[].timestamp` | 重连第一帧。有正在生成的消息就续上，没有就只当「连上了」。**`entries` 是唯一的时间源**：`/v1/entries` 不给时间（引擎的 `activity.json` 只按会话记一个时刻），按天分组目录读这里 |
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

### GET /v1/unread

每条会话的未读数，**平铺的 map**，一个会话一个键（**没有 `items` 包一层**）：

```
{ "14": { unread: 3, readAt: 1794212000000 }, "2": { unread: 0, readAt: null }, ... }
```

我们依赖：`unread` `readAt`。

- `unread` 是引擎按「EntryId 水位之后还有几条」数好的，**客户端不自己数**
  （条数会被 fork / compaction 弄错，水位不会）。
- `readAt` 是上次标已读的时刻（epoch ms），可能为 `null` —— 那是引擎种的水位，
  `null` 不是「读到过」，只是一个还没被 `POST /v1/read/:id` 推进过的水位。
- 覆盖**所有**会话；没有水位的会话给 `{ unread: 0, readAt: null }`。
- 跟 `GET /v1/sessions` **一起拉**（同一帧并发），别一会拉一个。

### POST /v1/read/:id

把这条会话标成读到最新，body 空。回 `{ id, readAt, unread: 0 }`。

- 不存在的会话 → 404；只有 `POST`，别的动词 → 405。
- 打开会话时打它；水位只进不退（引擎那边钉住了）。
- **别自己推水位**，也别把「已读」写本地就完事 —— 引擎才是水位的事实源，重装客户端不该丢未读。

### POST /v1/sessions/:id/fork

body `{ entryId?, name? }`。`entryId` 省略 = **从最后一条分叉**。回 201 `{ id, at }`，
`id` 是新会话，`at` 是在父会话的哪条 entry 上分的叉。

- 分出来的会话在列表里带 `parent: {id, at}`。
- 名字不给就沿用引擎的默认名（「对话 44」这种），要区分就自己带 `name`。

### GET /v1/presets/:section

模板（通讯录）那一页的数据。**段（section）不是只有一个**：同一个私仓按段放，
`Session-Template`（会话模板）和 `Ultimate-Template`（终极模板）各自一套端点、一套数据、
**一套类别词表**（`categories.json` 住在段目录里）。

返回 `{ section, title, repo, presets: [...], archived, categories, ... }`。

每条 preset：

```
{ name, summary, description, category, spec, specError, tracked, dir, resolved }
```

我们依赖：`name` `summary` `category` `spec` `specError` `tracked` `resolved`
\+ 顶层的 `categories` `repo`。

- `categories` 是**这一段的全部类别路径**，引擎已排好序（父在子前、逐段 `localeCompare('zh-CN')`），
  里面**包含零计数的类别**（词表里先建好、还没挂东西的）—— 客户端不许滤掉。
- `category` 是字符串数组，`/` 表示层级：`写作/周报` 自动蕴含父级 `写作`。一条可以挂多个。
- `repo` 是**这一段的**仓状态 `{ head, dirty, ahead, behind, error }`；`dirty` 按段算，
  `ahead`/`behind` 是整仓的。`head` 可能是 `null`（仓读不了）—— 别拿它当字符串硬切。
- `specError` → 这条自己写坏了，红。
- `resolved.*.state === "invalid"` → 引用失效，现在开不了。
- `tracked === false` → 盘上有、仓里没。
- **能否启动的判据全是引擎算好的**，客户端不自己拼规则。加了规则就等于可以跟引擎吵起来。
  （唯一客户端自己算的是**类别计数**：引擎不给，算法是「含后代 + 同一条预设按路径去重」。
  那是派生量，不是第二份事实源。）
- 能开的行：`POST /v1/sessions { preset: "Session-Template/<name>", name }`，`name` 带时间。

#### 终极模板那一段是另一种东西（`spec` 里长这样）

```
{ v, name, summary, description, category, trigger, params, session, enabled }
```

- `trigger` 是触发器名字，`params` 是它的参数（对象：字符串 / 数字 / 布尔）。
- `session` 指向 `Session-Template` 段里的一条会话模板。
- **这一段现在只读**：`enabled` 怎么切、`trigger` 用哪个端点启用，协议都还没定。
  桌面端只显示（`⚡ trigger` / `→ session`），点它什么也不发生。要动它先定端点。

---

### GET /v1/models

`{ default: {provider, modelId} | null, items: [...] }`。`items[]` 每条：

```
{ provider, modelId, name, thinkingLevels: string[], contextWindow, maxTokens }
```

- `default` 是**盘上那只眼当前指着的**模型，不是我们的默认值。
- `thinkingLevels` 从少想到多（`off` … `max`），**每个模型不一样**。换模型必须按新模型的表
  重新夹档位（`off` 一定要在表里，没有就退成 `off`）。
- 242 个模型是常态 —— 模型那一列必须能滚，别想着一次画完。

### GET /v1/skills

`{ root, categories: string[], repo, skills: [...] }`。技能那条：

```
{ name, path, description, dir, file, hidden, category: string[], tracked, load: { state } }
```

- **只列 `load.state == "ok" && !hidden`**。`hidden` 是 `disable-model-invocation`，
  勾上也不会动；`state != ok` 的加载不出来。
- `categories` 只是词表，和技能身上挂的 `category` 取并集才是栏里该有的那些。

### GET /v1/prompts

`{ default: string | null, categories: string[], prompts: [...] }`。模板那条至少有
`{ name, description?, category?, load: { state } }`。

- 和技能不是一回事：**模板是单值**（每轮都带的那份规则），技能是集合。所以列表里
  模板用单选框、技能用复选框 —— 形状本身在说能选几个。
- 仓库 `settings.json` 的默认值只用来**预勾**，选不选仍然是人的事。

### GET /v1/machines

`{ at, machines: [{ name, user, port, ips: [{addr,label}], addr, note, tags }] }`。

- 没有 `machine` 字段 = **本机**。名字要和创建请求里的 `machine` 对得上。

### GET /v1/fs/dirs?path=

`{ path, dirs: [{ name, path }] }`。**列的是 qrd3wh 那台机器上的目录。**

- 不带 `path` = 引擎给的起点（board 的 map 根）；`path: ""` 是起点，不是「没有路径」。
- 浏览发生在引擎进程里：引擎以后跑在远程机器上，这一页一行都不用改。

### POST /v1/sessions —— 建会话的完整 body

```jsonc
{
  "name": "…",                        // 可省，省了引擎给「对话 N」
  "model": {"provider":"…","modelId":"…"},
  "thinkingLevel": "off|minimal|low|medium|high|xhigh|max",
  "skills": ["code-review", "…"],     // 可省 = 不挂
  "template": "审查一段改动" | null,   // **总是发**：null 是「明确不要」
  "extensions": ["…"],                // 可省 = 全开（空数组才是「一个都不要」）
  "cwd": "/opt/Nous/content/new1/map",
  "machine": "j1900"                  // 缺省 = 本机
}
```

- 这几个键是 **camelCase**（`thinkingLevel` / `cwd` / `machine`）。写成 snake_case
  不会报错，只会**悄悄不生效** —— `crates/nous-engine/tests/parse.rs` 里有条测试钉着它。
- `template` 省掉 ≠ 不要：省掉服务端会用仓库默认值，于是「我明明点了无模板又给我塞回来」。
- 201 回来带 `missing` / `missingExtensions` / `missingTemplates`：引擎那边找不到的东西，
  客户端要如实说，别当成功。
- 建完再 `POST /v1/selection { id }` 把盘上那只眼切过去（切不过去不该把会话判死）。

## 引擎改了会怎么坏（按我们依赖的字段）

| 引擎改动 | 我们会怎样 |
|---|---|
| 删 `busy` / 换成布尔 | 列表忙闲角标消失或全红 |
| 删 `last` / 换形状 | 列表预览空了 |
| `parent` 语义变 | fork 过滤逻辑失效 |
| 删 `updatedAt` / 单位换成秒 | 列表时间全空，或者每条都显示成 1970 |
| `categories` 里把零计数的类别滤掉 | 通讯录里「建了但还没挂东西」的类别消失 |
| `category` 的 `/` 改成另一种分隔符 | 层级树断成一层，父级筛选丢失整枝 |
| `repo` 整块没了 / `head` 变必填 | 段脚注显示「仓状态未知」，最坏是解析失败 |
| 终极模板的 `trigger` / `session` 改名 | 那一段的 chip 没了（只是少显示，不报错） |
| `/v1/unread` 改回 `{items:[]}` / 改成数组 | 未读角标全灭（形状对不上） |
| `unread` 改成客户端自己数 | 水位退化成条数，fork 之后未读数永久错位 |
| `POST /v1/read/:id` 语义变（不再推到最新） | 未读点了不消，或者一次点掉不该消的 |
| SSE 事件改名 / `changes[]` 换结构 | 气泡不更新，流式死掉，症状是「能发出去但一个字不出来」 |
| `requestId` 幂等被去掉 | 重发会答两遍 |
| `/v1/selection` 语义变 | 三栏高亮错位 |

所以：启动时打 `GET /v1/device` 记引擎版本，About 里显示，出问题先对版本。

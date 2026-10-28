# 多会话的块：一棵树，不是一张列表

`templates/conversations/` —— 拷进一个块就是「会开很多对话的 app」。活样本在 `new1 / qrd3wh`。

一个磁盘一个 `data/agent.sqlite`，里面 N 个 conversation。

## 为什么是树

pi-durable 里会话本来就是树，不是列表：`ConversationRecord.parent = { conversationId, at }`，
`at` 是「从哪一条长出来的」。所以树不是装饰，是把已有的结构画出来。

于是两种「新增」语义完全不同，界面上必须分开：

| 动作 | 在树上 | 底层 |
|---|---|---|
| **新对话** | 顶层再长一根，跟现有的**平级** | `harness.createConversation({ ownership: { kind: "ownerless" } })` |
| **从这条分叉** | 该节点**下面**长子枝 | `conversation.fork(entryId, { ownership: { kind: "ownerless" } })` |

分叉只挂在具体的一条上 —— 从哪一句长出来的，得指得清。混成一个按钮，用户永远搞不清自己在哪。

## 视觉：抄 pele 的树，别抄 mermaid

参考的是 [pele.run/examples/treeview](https://pele.run/examples/treeview)。它是 Mermaid 的一个图类型
（`treeView-beta`），**静态图渲染器**，不是能点的组件。接它会当场撞墙。

但它那套视觉语言整套抄下来正好：

| pele | 我们 |
|---|---|
| `├──` `└──` 引导线 | 层级靠线，不靠缩进 —— 深了也不会飘 |
| 目录 vs 文件两色 | 会话（容器）vs 分叉出来的子枝 |
| `## description` 右侧淡色 | 上一次说了什么 / 从哪分叉 |
| `:::highlight` | **正在跑的那一支**（绿点 + 加粗），不加动画 |

## 接口

```
GET  /                            那棵树
GET  /v1/sessions                 会话树：id/name/parent/busy/model/readOnly/last
POST /v1/sessions                 {name} → 新对话
POST /v1/sessions/:id/rename      {name}
POST /v1/sessions/:id/fork        {entryId} 或 {}（空 = 从最后一条分叉）
POST /v1/sessions/:id/config      {readOnly}
GET  /v1/sessions/:id/entries     历史（新的在前，已滤掉 pi.system）
POST /v1/sessions/:id/messages    {content, requestId?, whenBusy?, waitMs?}
GET  /v1/sessions/:id/events      SSE
POST /v1/sessions/:id/abort
```

## 三个踩过的坑，记下来

**一、`defineDoc` 的 `checkpointWhen` 是必填的。** 少了就是 `change.definition.checkpointWhen is not a function`
—— 而且报在创建会话的 commit 里，看起来完全不像文档的错。这张表小、改得也不勤，`() => true` 就行：
每次给一份完整 base，别攒 delta。

**二、`entry.model` 里的 `content` 可能是字符串。** 用户那条是 `"你好"`，assistant 那条是一串 block。
`contentText()` 只吃后者 —— 混着来会**静悄悄变成空字符串**，界面上用户说的话全没了。要判类型：

```js
entry.model.map((m) => (typeof m.content === "string" ? m.content : contentText(m.content))).join("")
```

**三、只读要的是工具对象，不是名字。** `configure({ modelTools: { remove } })` 内部是 `item.name` 取名字，
传 `["write","edit","bash"]` 会报 `Value contains a non-JSON undefined` —— 因为取出来是 `undefined`。

```js
import { createBashTool, createEditTool, createWriteTool } from "@earendil-works/pi-durable/tools"
const WRITE_TOOLS = [createWriteTool(), createEditTool(), createBashTool()]
```

只读之后模型拿到的是 `[error] Tool bash is not available`：**不是拦住，是根本不递给它。**

## 会话名存在哪儿

`ConversationRecord` **没有 name 字段**。名字存一个 session 级文档：

```js
const Sessions = defineDoc({ kind: "qrd3wh.sessions", version: 1, scope: "session",
  initial: () => ({ names: {} }), checkpointWhen: () => true })

Harness.open(storage, {
  conversationCreated: async (tx, conversation) => {
    const doc = await tx.doc(Sessions)
    doc.names[String(conversation.id)] = `对话 ${conversation.id}`
  },
})
```

**不进 transcript**（污染上下文）、**不进 `tree.json`**（那是语义唯一真相，名字是这盘的状态）。

## 忙闲是派生量

`harness.inspect(ctx)` 一次拿全（tasks + submissions），别按会话一个个问。页面上 3 秒轮询一次就够 ——
不值得为「谁在跑」再开一条 SSE。

## 接真模型：models.json → provider

`models-json.js` 把 pi 那份 `~/.pi/agent/models.json` 装进 pi-ai（`PI_MODELS` 可改路径）。
块启动时先装它，再决定默认模型：

```
AGENT_MODEL=provider/modelId       ← 显式指定，最强
data/default-model.json            ← 上次在页面上选的
models.json 里第一个有密钥的 provider
内置 provider 里环境有凭据的
都没有 → faux（回声，能跑通链路）
```

默认模型是**这块的状态**，落 `data/default-model.json`，跟着文件夹一起被拷走。

| 接口 | 作用 |
|---|---|
| `GET /v1/models` | 只报「这台机器真能打的」：models.json 里配了的 + 环境里有凭据的（35 个，不是内置目录的 1634 个） |
| `PUT /v1/default` | `{provider, modelId}`，写进 `data/default-model.json` |
| `POST /v1/sessions` | 建会话时可选 `model`，不选就用默认 |

页面上就是右上角那个下拉框：换了它，下一个新对话就用那个模型。

## 记录

实测（`qrd3wh`，faux provider）：

| 试了什么 | 结果 |
|---|---|
| 建两个对话 | id 2 / 9（`mintId` 每次 commit 吃 id，跳号是正常的） |
| 分叉 | `POST /v1/sessions/2/fork {entryId:19}` → 会话 20，`parent {id:2, at:19}`，历史照抄 |
| 幂等 | 同一个 `requestId` 再发一次 → 同一个 `submissionId`，没有答第二遍 |
| 只读 | `!ls` → `[error] Tool bash is not available` |
| SSE | `snapshot → run_start → turn_start → message_* → turn_end → run_end` |
| 经宿主 | `http://<host>:4790/p/new1/qrd3wh/` 全通 |
| 真模型 | `volc-ark/deepseek-v4-flash` → 答「收到」；带 bash 工具跑出 `pi-ok` |
| 真模型 | `suanli/z-ai/glm-5.3-flash` → 同上（带 thinking） |

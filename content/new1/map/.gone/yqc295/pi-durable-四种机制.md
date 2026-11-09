# pi-durable 的四种机制

`defineTool` · `defineExtension` · `hook` · `section`

这四样东西合起来决定一件事：**模型这次请求能看见什么、能做什么、出事之后怎么办。**

| 机制 | 一句话 | 加进去的东西 |
|---|---|---|
| `defineTool` | 造一个能力 | 模型可以调的一个函数 |
| `defineExtension` | 把一个包的能力打包命名 | tools + sections + hooks + wraps + tasks |
| `hook` | 不改工具本体，在调用前后插一手 | 拦截、改参数、改结果 |
| `section` | 往系统提示词里塞文本 | 每次请求现算的一段话 |

四样都只是「声明」，真正生效要有第五样：**registry**（装进去）+ **选择**（这个会话开哪些）。

---

## 〇、先看全景

```
defineTool  ─┐
section     ─┼→ defineExtension(name)  ─→ registry.install()  ─→ 会话选择（按扩展名）
hook        ─┤                                                        │
wrapTool    ─┘                                                        ↓
                                                       每轮请求：Agent 解析
                                                       ├─ tools     → 递给模型
                                                       ├─ sections  → 拼系统提示词
                                                       └─ hooks     → 包在任务外面
```

三句话记住：

- **工具不是裸的**，它永远属于某个扩展。
- **扩展是开关的单位**，不是工具。你想关的其实是「开不开这套能力」。
- **hook 和 section 不产出工具**，所以它们在工具列表里看不见，但它们同样有力。

---

## 一、defineTool：一个能力

```js
import { defineTool } from "@earendil-works/pi-durable"
import { Type } from "@earendil-works/pi-ai"

const readBoard = defineTool({
  name: "read_board",
  description: "读一个盘面的树：每个组和块的 id、名字、值。",
  parameters: Type.Object({
    board: Type.String({ description: "盘面 id，比如 new1" }),
    depth: Type.Optional(Type.Number({ description: "最多钻几层，默认 3" })),
  }),
  replay: "safe",          // 崩了重开可以再跑一遍
  execute: async (args, api) => {
    const tree = await fetchBoard(args.board)      // 你自己写
    api.output(`读到 ${tree.length} 个节点\n`)      // 边跑边给前端看
    return { output: render(tree) }                // 给模型的最终文本
  },
})
```

### 字段全表

| 字段 | 默认 | 作用 |
|---|---|---|
| `name` / `description` / `parameters` | — | 模型看到的一切。`parameters` 是 TypeBox（`Type.Object`），**`execute` 的 `args` 类型由它推出来** |
| `replay` | **`"unsafe"`** | 见下面「最该想清楚的那个字段」 |
| `executionMode` | 跟设置走 | 同一轮里能不能并行 |
| `callers` | 模型 + 工具 | `["model"]` = 只给模型调（比如 codemode）；`["tools"]` = 只给别的工具调，模型看不见 |
| `deferred` | — | 先不给模型，等 `tool_search` 之类把它加载出来（工具上百个时用） |
| `outputLimits` | — | `{maxBytes, maxLines, retain:"head"\|"tail"}`，输出截断策略 |
| `prepareArguments` | — | 校验前修模型常见的烂参数（比如该给数组给了 JSON 字符串）。必须纯函数 |
| `structuredOutputSchema` | — | 声明「给程序看的返回值」的形状，配合 `api.executeTool()` 用 |
| `execute(args, api, ctx)` | — | 干活 |

### `execute` 手里的 `api`

不是只有参数 —— 这个 `api` 是**这次调用的上下文**，很厚：

| 用它干什么 |
|---|
| `api.output(chunk)` | 流式输出，最后会变成结果（除非你 return 了 `output`） |
| `api.details(value, ctx)` | 给界面看的结构化细节（不是给模型的） |
| `api.diagnostic({severity, message})` | 给模型一句关于这次调用的备注（截断了、写到哪个文件了） |
| `api.memo(name, ctx)` | **跨重启的记忆**。同一个工具重跑，拿到的是上次记下的值 |
| `api.commit(tx => …, ctx)` | 在会话事务里写东西（文档、条目、任务） |
| `api.executeTool(name, args, ctx)` | 调另一个工具，拿它的结果（嵌套调用，不进 transcript） |
| `api.env` | 执行环境（文件系统、shell） |
| `api.models` | 模型客户端 —— 工具里可以自己叫一次模型 |

### 返回什么

```js
{
  output,            // 给模型的文本（不写就用 api.output() 攒下来的）
  structuredOutput,  // 给程序的（声明了 structuredOutputSchema 时必填）
  isError,           // 标成错误结果，模型会看到
  details,           // 给界面的
  usage,             // 这次调用自己花的 token（工具里叫了模型时）
  control: {
    addTools,        // 往这次会话里加工具（tool_search 就靠这个）
    terminate,       // 这一轮所有结果都 true 就不再请求模型，直接结束
    handoff,         // 交接上下文（换一个上下文继续）
  },
}
```

### 最该想清楚的那个字段：`replay`

这是这套东西里唯一「想错了会出人命」的地方。

| 值 | 进程崩在 `execute` 中间，重开之后 |
|---|---|
| `"safe"` | **会再跑一遍**。适合读文件、查数据、跑只读命令 |
| `"unsafe"`（默认） | **绝不重跑**。模型收到一条 `interrupted` 错误结果，附带崩溃前已经提交的那部分输出 |

所以：

- `read` / `ls` / 查库 → `"safe"`
- 发邮件、转账、叫人来修、`rm -rf` → **`"unsafe"`**（默认就对了，别手贱加 safe）
- 写文件 → 通常 `"unsafe"`（重跑会把中间态覆盖一次）

durable-pi 里那个 `ping_on_call_engineer`（叫人）特意写了 `unsafe`，注释就一句：*a crash returns an interrupted result instead of paging again, and it works once per agent*。这是产品判断，不是技术细节。

配合 `api.memo()` 还能做出「幂等副作用」：

```js
execute: async (args, api, ctx) => {
  const done = await api.memo("already-sent", ctx)   // 重跑时能拿到上次的值
  if (done) return { output: "已经发过了，不重复发", details: done }
  const receipt = await sendEmail(args)
  return { output: "发了", details: await api.memo("already-sent", receipt, ctx) }
}
```

---

## 二、defineExtension：能力包

```js
import { defineExtension } from "@earendil-works/pi-durable"

const BoardTools = defineExtension({
  name: "board-tools",
  tools: [readBoard, renameNode, openDoc],
  sections: [boardGuide],        // 往提示词里塞说明
  hooks: [guardBash],            // 插一手
  wraps: [timedBash],            // 修饰别的工具
})
```

五个字段，全都可以省：

| 字段 | 内容 |
|---|---|
| `name` | 开关就是按它开关的。**改名字等于换了个人**，存的选择全丢 |
| `tools` | 这一包提供的工具 |
| `sections` | 这一包贡献的系统提示词段落 |
| `hooks` | 这一包挂的钩子 |
| `wraps` | 修饰**别人**的工具/段落（`wrapTool` / `wrapSection`） |
| `tasks` | 自定义的可恢复任务（一般不用碰） |

两条规则要记：

- **同名工具，后选的赢。** 「后装/后选中」的扩展里同名的工具会替换掉先前的那个。
- **`wraps` 永远修饰赢的那个。** 所以「给 bash 加计时」这件事，不改 bash 也能做到。

pi-durable **自己只带了这一个扩展**：

```js
import { CodingTools } from "@earendil-works/pi-durable/tools"
// read · write · edit · bash（+ powershell 版）
```

别的都得你自己写。

---

## 三、hook：不改本体插一手

```js
import { defineExtension, hook, ToolTask } from "@earendil-works/pi-durable"

const guard = defineExtension({
  name: "bash-guard",
  hooks: [
    hook(ToolTask, {
      // 在意图提交之前。返回 {block: "..."} 就拦住，模型收到这句话当结果
      beforeTool: async (call, api, ctx) => {
        if (call.name !== "bash") return
        const command = String(call.arguments.command ?? "")
        if (/\brm\s+-rf\s+\//.test(command)) return { block: "这条命令不给跑。" }
        // 也可以返回 {arguments: {...}} 改写参数
      },
      // 在结果提交之前，可以整个换掉
      afterTool: async (call, result, api, ctx) => {
        if (call.name === "bash") return { ...result, output: `[已审] ${result.output}` }
      },
    }),
  ],
})
```

`hook(任务, 处理器)` 的第一个参数是**任务类型**，决定你挂在哪一步：

| 任务 | 能挂什么 | 典型用途 |
|---|---|---|
| `ToolTask` | `beforeTool` / `afterTool` | 拦截危险命令、审计、脱敏、（结果）改写 |
| `GenerationTask` | `beforeRequest` / `afterResponse` / `onYield` / `afterTools` | 改请求、看响应、决定要不要继续跑一轮 |
| `CompactionTask` | `beforeCompact` | 换一套摘要指令 |
| 你自己的任务 | 自己定义 | — |

三个要点：

- **`beforeTool` 拿到的是「意图」，结果提交前** —— 这时候拦住，等于没发生。
- **钩子里的 `api.memo()` 是持久化的**。durable-pi 的 Jev 审核判过一次就记下来，重启不会再问第二遍（问一次 300ms，一轮对话里七八个 bash 就白扔两秒）。
- **钩子要克制**。Jev 那个判不出来时**放行**（fail open）：审核服务挂了不该让整个 agent 停摆。反过来，安全钩子若 fail closed，一次超时就是全线瘫痪。

还有一个容易忽略的：**钩子抛异常 = 拦住**。所以钩子里要 try/catch，别把网络抖动变成「工具突然不能用了」。

---

## 四、section：系统提示词

```js
import { defineExtension, section } from "@earendil-works/pi-durable"

const guide = section(
  "board-guide",                       // key 就是外面包的标签
  async (input, ctx) => {
    const enabled = await input.read.snapshot(PromptChecklist, input.conversationId, ctx)
    if (!enabled?.includes("board")) return undefined   // 返回 undefined = 这段不出现
    return "这个盘面里每个块都是一个可执行的东西。改名前先看清 id。"
  },
  { tag: false },                      // 默认 true：外面包一层 <board-guide>…</board-guide>
)

const prompts = defineExtension({ name: "board-prompts", sections: [guide] })
```

`render(input, ctx)` 的 `input` 是**这次请求的现场**：

| 字段 | 是什么 |
|---|---|
| `input.agent` | 这次解析出来的 agent，`input.agent.tools` 就是模型这次真拿到的工具 |
| `input.env` | 执行环境（cwd、文件系统） |
| `input.read` | 已经提交的文档，`.snapshot(Token, …)` 直接读 |
| `input.shown` | 前面已经生效的段落，避免重复说 |

所以「动态提示词」不是玄学：**读一个 doc，按开关拼一段话**。用户勾了哪几条，下一轮请求的提示词就是哪几条。

section 的纪律：**别把工具的说明书塞进 section**。工具的 `description` 已经会进上下文了，重复一遍是白烧 token，还会打架。

---

## 五、拼起来：一个能跑的最小例子

```js
import { BACKGROUND_CONTEXT as ctx } from "@earendil-works/chord/context"
import { createModels } from "@earendil-works/pi-ai/models"
import { Type } from "@earendil-works/pi-ai"
import {
  Harness, MemoryStorage, createRegistry,
  defineTool, defineExtension, section, hook, ToolTask,
} from "@earendil-works/pi-durable"
import { CodingTools } from "@earendil-works/pi-durable/tools"
import { NodeExecutionEnv } from "@earendil-works/pi-durable/env/node"

// 1. 工具
const say = defineTool({
  name: "say",
  description: "把一句话记到日志里",
  parameters: Type.Object({ text: Type.String() }),
  replay: "safe",
  execute: async (args, api) => {
    api.output(`记下了：${args.text}\n`)
    return {}
  },
})

// 2. 段落 + 钩子 + 打包
const style = section("style", async () => "回答要短。")
const noRm = defineExtension({
  name: "no-rm",
  hooks: [hook(ToolTask, {
    beforeTool: async (call) =>
      call.name === "bash" && /rm -rf/.test(String(call.arguments.command ?? ""))
        ? { block: "不给删。" } : undefined,
  })],
})
const mine = defineExtension({ name: "mine", tools: [say], sections: [style] })

// 3. 装进 registry
const registry = createRegistry()
registry.install(CodingTools)
registry.install(noRm)
registry.install(mine)

// 4. 开一个 Harness，选定开哪几个
const models = createModels()
models.setProvider(/* … */)
const harness = await Harness.open(
  new MemoryStorage(),
  {
    models,
    registry,
    env: ({ cwd }) => new NodeExecutionEnv({ cwd: cwd ?? process.cwd() }),
    settings: { extensions: [CodingTools, noRm, mine] },   // 默认全开
  },
  ctx,
)
const root = await harness.root(ctx, { agent: { model: { provider, modelId } } })
```

**注意第 4 步和第 3 步是两件事**：`install` 是「这一进程里存在这些能力」，`extensions` 是「这个会话开哪几个」。同一个 registry，不同会话可以开不同的组合。

---

## 六、三个层面的开关（决定了「只读」怎么做）

| 开关 | 粒度 | 长什么样 | 存在哪儿 |
|---|---|---|---|
| `settings.extensions` | 默认开哪些扩展 | `[CodingTools, mine]` | Harness 设置 |
| `configure({ extensions })` | **这个会话**开哪些 | `{add:[ext], remove:[ext]}`，传**扩展对象** | `pi.agent` 条目里（存名字） |
| `configure({ tools })` | 这个会话砍掉哪些工具 | `{remove:[工具对象]}` | 同上 |
| `configure({ modelTools })` | 模型**这次请求**看得见哪些 | `{add:[名字], remove:[名字]}` | 同上 |

「只读」就是最后一个：

```js
await conversation.configure({ modelTools: { remove: ["write", "edit", "bash"] } }, ctx)
```

模型下一次请求里就没这三个了 —— 不是拦住，是**根本没递过去**，它会自己说「我没有这个工具」。

`modelTools.remove` 要的是**工具对象**不是名字（内部走 `item.name`），传字符串会报
`Value contains a non-JSON undefined`。这个坑我踩过。

---

## 七、五个坑，都是我们自己踩的

1. **`replay` 默认是 `unsafe`。** 想要「崩了自动重跑」，得显式写 `safe`。忘了写不是报错，是行为更保守 —— 这方向对，但要清楚。
2. **工具里读会话数据，要在第一次写之前。** `tx` 里「先写后读」会抛 `ReadAfterWrite`。文档不受影响，表（entries/tasks/conversations）受。
3. **`defineDoc` 的 `checkpointWhen` 必填。** 少了就是 `change.definition.checkpointWhen is not a function`，而且报在「创建会话」的 commit 里，看着完全不像文档的错。
4. **`modelTools.remove` / `tools.remove` 要工具对象。** 见上。
5. **钩子抛错 = 拦住。** 复杂逻辑必须自己 try/catch，否则一次 500 就是「工具突然消失」。

---

## 八、对我们这个项目意味着什么

**一、要做「工具管理」，管的单位是扩展。** 因为开关的意义在扩展上：开不开「危险命令审核」是个决定，单关掉 `bash` 不是。工具列表是**结果**，不是配置项。

**二、每个工具最该露出来的字段是 `replay`。** 它是产品判断（「崩了能不能再干一遍」），不是技术参数。界面上应该跟 durable-pi 一样，两个徽章直接写「崩了会重跑」/「崩了不重跑」。

**三、我们要写的东西，pi 一个都没提供。** 它只给了 `read/write/edit/bash` 四个手。真正的工作量在业务工具：
- 块的：读树、改名、开文档、看邻居（`/__nous/ctx`）
- 盘面的：某块在跑什么、谁的 agent 活着
- 这些都得自己 `defineTool`，每个二三十行。

**四、先做哪个？** 我的顺序：
1. 三个只读工具（读树 / 读文档 / 看邻居）—— 全是 `replay: "safe"`，零风险，跑通了 `defineTool` 这条路。
2. 一个 `section`：告诉模型「这个盘面是什么、块是什么」。
3. 一个 `hook`：给 `bash` 加一道拦截 —— 抄 Jev 的克制（判不出来就放行）。
4. 最后才做扩展开关的界面。

做完 1 之后，`defineTool`/`section`/`hook` 三样的手感就有了，第四样（`wraps`）等真需要改别人的工具时再说。

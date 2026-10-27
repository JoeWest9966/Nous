# import 清单：三件套能拿什么

`@earendil-works/pi-durable` / `chord` / `pi-ai` 三个包一共一百多个导出名。这份文档只回答一个问题：**我这行 `import` 该写什么路径，拿到的那个东西是干嘛的。**

版本现状（写于 1.4.0）：

| 包 | 版本 | 用途 |
|---|---|---|
| `@earendil-works/pi-durable` | `1.4.0` | agent loop + tools + 持久化文档 |
| `@earendil-works/chord` | `1.4.0` | 底座：IPC、facet、sqlite 适配 |
| `@earendil-works/pi-ai` | `1.4.0` | 模型供应商、typebox、鉴权 |

三个都是**精确钉死**的（不是 `^`）。`pi-durable` 自己的 `package.json` 就精确依赖 `pi-ai@1.4.0`，顶层写 `^` 迟早分叉出两份 `pi-ai`，症状是「同一个类却不相等」。三个一起升、一起锁死。

> 别急着写代码：这三个包目前是**装上但没人 import** 的状态。`src/` 和 `server/` 里一行都没有。

---

## 三条硬规则

### 一、只有 `server/*.ts` 能 import

`src/` 里一行都不许。

| 写在哪 | 后果 |
|---|---|
| `server/*.ts` | 只在 Node 里跑，几十兆 sdk 躺在服务端，随便 |
| `src/*.tsx` | 进客户端 bundle，用户手机上下一坨跑不了的 node 代码 |

`pi-ai` 拖着 `openai` + `@anthropic-ai/sdk` + `@aws-sdk/client-bedrock-runtime` + photon wasm。往 `src/` 里一 import，就是几十兆死重。

`vite.config.ts` 里的 external 已经接好了保险丝：

```ts
// vite.config.ts:120
external: (id) => id.startsWith("@earendil-works/"),
```

就算手滑，是在 build 期报「模块没找到」，而不是悄悄把整个 AWS SDK 塞进 bundle 让人慢慢发现。

### 二、持久化状态写 `NOUS_DATA`

chord 的 sqlite 也好、agent 的会话记录也好，落块的 `data/` 底下，**不许落仓库根**。理由跟块一样：`cp -r map/<id> ~/<id>` 走了，状态也跟着走。

### 三、`node >= 22.19.0`

`engines` 已声明。`node:sqlite` 在 20.x 上不存在，别指望它会以看得懂的方式报错。

---

## `@earendil-works/pi-durable` —— 这个项目真正要用的

### 主入口 `pi-durable`

| 拿到什么 | 什么时候用 |
|---|---|
| `createSession(storage, opts?)` | **开一个 agent 会话**。第一个参数是 storage |
| `Harness`、`DEFAULT_HARNESS_IDENTITY` | 完整的 agent 运行时（会话 + 工具 + 中断 + 重试） |
| `createRegistry` | 注册表，给工具/扩展用的命名空间 |
| `defineTool`、`defineExtension`、`hook`、`section` | 自己定义工具和扩展 |
| `watchEvents` | 订阅事件流（agent 输出、工具调用、快照） |
| `AgentDoc`、`InboxDoc`、`UsageDoc`、`LiveDoc`、`ProviderDoc` | 响应式状态文档，客户端能订阅的那种 |
| `defineDoc`、`defineDocFamily`、`defineEntry` | 自己定义持久化文档/条目的形状 |
| `defineTask`、`CompactionTask`、`GenerationTask`、`ToolTask` | 可恢复的长时间任务 |
| `configure`、`DEFAULT_COMPACTION_POLICY`、`DEFAULT_RETRY_POLICY`、`DEFAULT_PROGRESS_POLICY`、`DEFAULT_RESTART_LIMIT` | 调参 |
| `ConversationBusy`、`ReadAfterWrite`、`SessionFailed`、`StorageRequestError` | 你这是要 catch 的错误类型 |
| `MemoryStorage` | 内存后端（也在 `storage/memory` 有一条单独的路径） |
| `ROOT_CONVERSATION_ID` | 根会话的固定 id |
| `DURABLE_TELEMETRY_SCHEMA` | 遥测表结构 |

还有一大坨纯类型（`Session`、`Storage`、`Task`、`TaskState`、`ConversationRecord`、`DocumentId`、`PromptInput`、`ModelRef`、`Extension`、`HookApi`…），`import type` 用，不产生运行时开销。

### 存储后端 —— 五选一

| import 路径 | 后端 | 环境 |
|---|---|---|
| `pi-durable/storage/sqlite/node` | `openNodeSqliteStorage(path, opts?)` | **Node。落盘用这个** |
| `pi-durable/storage/jsonl/node` | JSONL 追加日志 | Node，人类可读 |
| `pi-durable/storage/memory` | `MemoryStorage` | 都行，不落盘，测试/演示 |
| `pi-durable/storage/indexeddb` | IndexedDB | **浏览器专供** |
| `pi-durable/storage/sqlite/cloudflare` | Durable Objects SQLite | Cloudflare Workers |

`openNodeSqliteStorage` 会顺手 `mkdir -p` 父目录（见 chord 那边的实现），所以 `data/` 不存在也没关系 —— 它自己长出来。WAL 模式、`busyTimeoutMs` 默认 5000ms，都能通过第二个参数覆盖。

### 环境与工具

| import 路径 | 拿到什么 |
|---|---|
| `pi-durable/env` + `pi-durable/env/node` | `ExecutionEnv` 接口 / `NodeExecutionEnv` 实现（真 fs、真 shell、文件监听） |
| `pi-durable/tools` | `createCodingTools()`（read+write+edit+bash 四件套）、`createReadTool`、`createWriteTool`、`createEditTool`、`createBashTool`、`createPowerShellTool` |
| `pi-durable/images/node` | 图片处理（缩图、转格式），`createReadTool` 的 `images` 选项吃它 |
| `pi-durable/codemode` | 生成代码代替工具调用（工具太多时的另一条路） |
| `pi-durable/tool-search` | `createToolSearchTool()`，工具上百个时先搜再调 |
| `pi-durable/testing` | 一致性测试套件：`createStorageConformance`、`createEnvConformance`、storage benchmark |

`createCodingTools()` 什么都不自动装 —— 它只是返回一个 `Extension`，你得自己塞给 session。

---

## `@earendil-works/chord` —— 被 durable 吃掉的底座

日常你只碰一个路径，其余 pi-durable 已经包好了。

| import 路径 | 拿到什么 |
|---|---|
| `chord/sqlite/node` | `openNodeSqliteDatabase(path)`、`NodeSqliteDatabase` —— `openNodeSqliteStorage` 底下就是它 |
| `chord` | `createHost`、`defineFacet`、`bind`/`local`/`remote`/`method`、`hostFamily`、`defineRegistry` |
| `chord` | `ipcLink`、`webSocketLink`、`memoryLinkPair`、`acceptWebSockets` —— 跨进程/跨端通信 |
| `chord` | `replicatedState`、`ChordError`、`forbidden`、`invalid`、`copyJson` |
| `chord/node` | `createHostManager`、`forkDriver`、`processLock`、`nodeLoader` —— 真·子进程宿主 |
| `chord/delta`、`chord/context`、`chord/output` | 增量/上下文/输出流（durable 内部用） |
| `chord/telemetry-sqlite` | 遥测落 sqlite |

---

## `@earendil-works/pi-ai` —— 模型供应商

| import 路径 | 拿到什么 |
|---|---|
| `pi-ai` | `Type`（typebox，定义工具 schema 用的）、`createModels`、`createProvider`、`hasApi`、`calculateCost`、`clampThinkingLevel`、`uuidv7` |
| `pi-ai/models` | `createModels` / `Provider` / `Models` / `MutableModels` 类型 |
| `pi-ai/providers/all` | `getBuiltinModel(provider, modelId)` + 全部内置 provider 目录（anthropic、openai、google、bedrock…） |
| `pi-ai/providers/faux` | **假 provider** —— 不打网络就能测整个 agent loop，写测试时救命 |
| `pi-ai/oauth` | 登录流程（device code、浏览器回调） |
| `pi-ai/compat` | 兼容层类型 |
| `pi-ai/utils/*` | `retry`、`overflow`、`transcript`、`validation`、`text`、`event-stream`、`typebox-helpers` |

`pi-ai` 也 re-export 了 typebox 的 `Type` 和 `Static` —— 所以你不需要单独装 typebox。

---

## 最短能跑起来的样子

```ts
// server/agent.ts
import { createSession } from "@earendil-works/pi-durable"
import { createCodingTools } from "@earendil-works/pi-durable/tools"
import { NodeExecutionEnv } from "@earendil-works/pi-durable/env/node"
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node"

// NOUS_DATA 指向本块的 data/；openNodeSqliteStorage 会自己 mkdir -p
const storage = await openNodeSqliteStorage(`${process.env.NOUS_DATA}/agent.sqlite`)
const session = createSession(storage)

const env = new NodeExecutionEnv({ cwd: process.env.NOUS_DATA! })
const tools = createCodingTools()   // read / write / edit / bash
```

---

## 怎么自己查

别靠这份文档更新 —— 装上之后自己翻，永远准确：

```bash
# 这个包有哪些子路径能 import
node -p "Object.keys(require('./node_modules/@earendil-works/pi-durable/package.json').exports)"

# 某个子路径导出了什么名字
grep -E "^export" node_modules/@earendil-works/pi-durable/dist/tools/index.d.ts

# 有没有分叉出第二份 pi-ai（应该是 1 个）
find node_modules -name "pi-ai" -type d
```

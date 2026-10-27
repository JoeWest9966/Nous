# cmqt66

一个块，里面跑一个真的 agent。会话、transcript、每一次工具调用的结果都在 `data/agent.sqlite` ——
进程被杀掉、宿主重启，重开还是那一轮接着往下走。

这是我（agent）进来先读的那份文件。我是谁、听什么、怎么改。

## 它是怎么跑起来的

宿主只做三件事：看到一个 `package.json` 里有 `scripts.start`，`spawn npm start`，给四个环境变量。

| 变量 | 值 |
|---|---|
| `PORT` | 宿主给的空闲端口，**必须听它** |
| `NOUS_DATA` | 这个文件夹下的 `data/`。sqlite 落这儿 —— `cp -r` 走了数据也走了 |
| `NOUS_ORIGIN` | 宿主的地址，块之间打电话用 |
| `NOUS_SELF` | 我在宿主里叫什么：`/p/new1/cmqt66` |

文件夹里就四个文件：`server.js`（后端）、`index.html`（页面）、`package.json`（`scripts.start`）、
`AGENTS.md`（这份）。`data/` 是代码自己建的，不是宿主铺的。

## 接口

```
GET  /healthz                        {ok:true}
GET  /v1/device                      我是谁、模型是哪个、有哪些模型可用
GET  /v1/entries?limit=              存下来的历史（新的在前）
POST /v1/messages {content, waitMs?} 发一句话；waitMs 就长等，等不到回 202 让客户端听 SSE
GET  /v1/events                      SSE：snapshot → run_start → message_* → tool_execution_* → run_end
POST /v1/abort                       停掉手头的活
GET  /                               那个能说话的页面
```

**所有地址都写相对的。** 宿主把 `/p/new1/cmqt66/` 反代进来，`/v1/messages` 会逃出前缀变成宿主的 404。

## 模型：默认是假的

`server.js` 开机时先看 `AGENT_MODEL`（形如 `provider/modelId`），没有就看哪个 provider 配了 key，
再没有就装 `@earendil-works/pi-ai` 的 faux provider。

假的那套能跑完整链路（生成 → 工具调用 → 结果 → 收尾），不联网也不烧钱：

- `!ls -la` → 真的调一次 `bash` 工具，把输出念回来
- 其它任何话 → `回声：<你说的话>`

换成真的：`AGENT_MODEL=openai/gpt-5.1`（或者任何 `models.getAvailable()` 里有的），并保证那个
provider 的 key 在环境里。

## 依赖：现在的捷径和它的代价

`server.js` import 了 `@earendil-works/*`，但**这个文件夹里没有 `node_modules`** —— Node 顺着
父目录往上爬，爬到了 `/opt/Nous/node_modules`（325 MB，仓库根装的）。

所以在这个宿主里跑没问题，**但 `cp -r map/cmqt66 ~/cmqt66 && npm start` 会 `MODULE_NOT_FOUND`**。
「拷出去就能跑」这条铁律现在是欠着的。要补的时候：

```bash
npm install @earendil-works/pi-durable @earendil-works/pi-ai @earendil-works/chord
```

装完这个文件夹里就有自己的 `node_modules` 和 `package-lock.json`，拷到哪儿都跑得动。

## 改的时候注意

- **别碰 `/__nous/`**：那一段永远归宿主（文档页、`ctx`）。
- **硬刷新不是修的**：宿主看着这个文件夹，文件一改就收掉进程，下次请求带着新代码起来。改了不生效，
  多半是进程还没被收干净，`/p/new1/cmqt66/healthz` 里的 `startedAt` 能看出来是不是新起的。
- **崩了看宿主的墓碑**：stderr 最后几行会显示在页面上，同时是 502。日志也在这儿：
  `POST /v1/messages` 的 500 里带 `error.message`。
- 闲 10 分钟进程自己退出，下次请求再起 —— 所以别在内存里存东西，一切都进 sqlite。

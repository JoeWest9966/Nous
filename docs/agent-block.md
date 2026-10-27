# 块里的 agent：最小后端

`templates/agent-block/` 是一个能拷进任何块的 agent 后端。四个文件、零依赖安装（见末尾的欠账），
跑起来就是一个真能干活的东西：一个会话、`read`/`write`/`edit`/`bash` 四件套、SSE 推流。

已经在 `new1 / new1.1`（id `cmqt66`）里跑通了，那份是活样本。

## 拷进一个块

```bash
cp templates/agent-block/{server.js,index.html,package.json,AGENTS.md} \
   content/<board>/map/<nodeId>/
```

然后刷新 `/p/<board>/<nodeId>/`。宿主看到 `package.json` 里有 `scripts.start` 就会 `spawn npm start`，
给四个环境变量，剩下的交给它自己。

`data/` 不用建 —— `openNodeSqliteStorage()` 第一次落盘时 `mkdir -p` 自己长出来。

## 它长什么样

```
GET  /healthz                        {ok:true}
GET  /v1/device                      我是谁、模型是哪个、有哪些模型可用
GET  /v1/entries?limit=              存下来的历史（新的在前）
POST /v1/messages {content, waitMs?} 发一句话
GET  /v1/events                      SSE
POST /v1/abort                       停掉手头的活
GET  /                               那个能说话的页面
```

`POST /v1/messages` 两种脾气：

| 请求 | 回复 |
|---|---|
| `{content}` | `202 {submissionId, status:"queued"}` —— 立刻返回，去听 SSE |
| `{content, waitMs:15000}` | `200 {status:"done", answer:{entryId, text}}`；15 秒还没等到就 `202 {status:"running"}` |

**等不到不代表没做。** 这只是「等待」超时，活还在后头跑；同一个 submission 的结果之后会出现在
`/v1/entries` 里，SSE 也一直在推。

## SSE 的形状

先来一条 `snapshot`（现状：所有 entry + 在飞的生成 + 工具槽位），之后是一批一批的增量：

```
snapshot → run_start → turn_start → message_start
         → message_update {changes:[{type:"text_delta", delta}]}
         → tool_execution_start / tool_execution_update / tool_execution_end
         → message_end → turn_end → run_end
```

晚连上的客户端靠 `snapshot` 补齐，不会看到空屏；跟丢超过 100 批就再收一条 `snapshot`，不是断流。

## 模型

开机顺序：`AGENT_MODEL`（`provider/modelId`）→ 哪个 provider 配了 key → faux provider。

假的那套能跑完整链路（生成 → 工具调用 → 结果 → 收尾），不联网也不烧钱：

```
POST /v1/messages {"content":"!ls -la","waitMs":20000}
→ 真的调一次 bash，把输出念回来
```

实测（块在 `content/new1/map/cmqt66/`）：

```json
{"status":"done","answer":{"text":"工具跑完了：\ntotal 36\n-rw-r--r-- 1 root root 10247 server.js\n..."}}
```

换成真模型：`AGENT_MODEL=openai/gpt-5.1`，并保证那个 provider 的 key 在环境里。
`/v1/device` 的 `available` 会列出当前哪些能打。

## 页面上所有地址都是相对的

宿主把 `/p/<board>/<id>/` 反代进来。写 `/v1/messages` 就是逃出前缀，打到宿主自己的 404 上。
页面的 base 是这么算的：

```js
const base = location.pathname.endsWith("/") ? location.pathname : location.pathname + "/";
fetch(base + "v1/messages", ...)
```

`/p/<board>/<id>` 缺尾斜杠时相对路径会错位，所以先补齐。服务端那边也认带不带前缀（`stripPrefix`），
独立跑 `node server.js` 和经宿主反代是同一份代码。

## 欠账：依赖还没进块

`server.js` import 了 `@earendil-works/*`，但块里**没有 `node_modules`** —— Node 顺着父目录爬到了
仓库根那份 325 MB 的。

所以「这个宿主里跑」没问题，「`cp -r map/<id> ~/<id> && npm start`」会 `MODULE_NOT_FOUND`。
`AGENTS.md` 里那条铁律还没兑现。要补：

```bash
cd content/<board>/map/<nodeId>
npm install @earendil-works/pi-durable @earendil-works/pi-ai @earendil-works/chord
```

装完块里就有自己的 `node_modules` 和 `package-lock.json`，拷到哪儿都跑得动。这是有意留到最后的：
先在真环境里把链路跑通，再付这 325 MB 的代价。

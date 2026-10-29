# k5411g · Nous Warehouse 模型编目

这一页只做一件事：**把 Nous Warehouse 的模型编目摆出来，能在上面自查、看密钥、删一条。**

它不读本机、不比对、不做同步。上一版是「编目 × 本机」的对账页，本机那半边已经整个删掉了 ——
别再悄悄加回来。要回来，得人明确说「我要看本机差在哪」。

## 唯一的数据来源

`j1900:/opt/Data-001/models.json`，REST 面 `GET /api/model`。

- `NW_BASE` 默认 `https://j1900.tail8d0018.ts.net:8443`（tailscale serve，tailnet only，免 token）。
- `NW_TOKEN` 只在绕 `panel-gate` 的 `:8080` 上才需要（`Authorization: Bearer`）。
- 这一页**没有任何写文件的代码** —— 唯一的写是通过编目自己的 REST（recheck / delete）。

## 一条条目长什么样（ls 投影）

```
{ name, baseUrl, api, apiKeyRef, models: [id…], usedBy: [配方名…] }
```

四个必须记住的：

- **`apiKeyRef` 是引用或 `<redacted:…>`，永远不是真身。** 列表里出现 `<redacted…>` 不是 bug，
  那是 #220 起的纪律：`ls` 永不回显字面量密钥。真身只有一个出口 `GET /api/model/{名}/secret`。
- **`models` 只有 id。** `contextWindow` / `name` / 价格留在编目文件里，API 不给。
  所以页面上的模型 chips 能复制、不能编辑，也不该假装知道上下文长度。
- **`usedBy` 是现算的**（`nw ls` 扫 `recipes/` 回填），不是编目里存的字段。它只说明「现在谁指着它」。
- **名字是唯一键。** 同名同 baseUrl = 覆盖写；同名不同 baseUrl = 硬拒；换 baseUrl 只能删了再登记。

## 三个动作，各自的边界

| 动作 | 走哪 | 会改什么 | 风险 |
|---|---|---|---|
| 自查 | `POST /api/model/recheck` | 什么都不改，只比上游 `/models` | 无（202 作业，面板 2 分钟上限，超时那次其实可能已经落账） |
| 密钥 | `GET /api/model/{名}/secret` | 不改，取一次明文就丢 | 剪贴板里会留一份；页面不缓存 |
| 删除 | `DELETE /api/model/{名}` | 改 `models.json` + 打一个 git commit | **不可逆，且没有「谁在用」的保护** |

删除的三道闸（都在这一页里，不在编目里）：

1. `usedBy` 非空 → 先警告「这些配方会指向一个不存在的供应商」；
2. `prompt` 要求**手打名字**；
3. 服务端再比一次名字，对不上直接拒（UI 挡手滑，真拦人的是这条）。

## envelope 的两种「失败」

`nw` 是先落账、再 `git push`。push 挂了会把整份 envelope 变成 `ok:false`，
但 `data.written` 已经是 `true`，本地 commit 也打了。整天这台机器到 GitHub 的 TLS 常断，
所以「报错但其实写进去了」是常态。

`unwrap()` 因此把 `written === true` 的失败标成 `partial`，页面显示「已写，但没推到 origin」。
**别把这个分支删掉** —— 删了人会以为没写、反复重跑，每次多一条 commit。

## 部署注意（踩过的）

- **`nw-panel.service` 必须给 `HOME`。** 原来没有这一行，所有写操作都报
  「`HOME 未定义:无法定位安装根(数据仓写锁住在 <NW_HOME>/state/locks/)`」—— 读没事、写全挂。
  已补 `Environment=HOME=/root`。
- **register 的 token 闸不展开 `$ENV`。** `internal/modelcatalog/register.go` 把 stdin 收到的串
  原样塞进 `Authorization` 去扫上游，所以推一个引用进去必然 401、零字节落盘。
  编目里 `apiKey: "$STEPFUN_API_KEY"` 那条是 #115 时代**手写**进文件的遗产。
- **面板对单次操作有 2 分钟上限。** 上游慢（`api.suanli.cn` 就慢）会看到
  「nw 没在 2m0s 内跑完」—— 那一刻 CLI 被杀了，但**写可能已经发生**。重试前先看一眼 `/api/model`。

## 接口

```
GET    /                       这一页
GET    /healthz                { ok, ledger }
GET    /v1/state               { at, ledger:{ok,base,entries[]}, summary:{entries,models}, error? }
GET    /v1/secret/:name        { ok, name, key, shape }       唯一明文出口
POST   /v1/recheck  {name?}    { ok, scope, drifts[], summary }  不带 name = 全量
POST   /v1/delete   {name}     { ok, name, action, remaining, partial, warnings[] }
```

# k5411g · Nous Warehouse 模型编目

这一页只做一件事：**把 Nous Warehouse 的模型编目摆出来，能在上面新增、自查、看密钥、删一条。**

它不读本机、不比对、不做同步。上一版是「编目 × 本机」的对账页，本机那半边已经整个删掉了 ——
别再悄悄加回来。要回来，得人明确说「我要看本机差在哪」。

## 唯一的数据来源

`j1900:/opt/Data-001/models.json`，REST 面 `GET /api/model`。

- `NW_BASE` 默认 `https://j1900.tail8d0018.ts.net:8443`（tailscale serve，tailnet only，免 token）。
- `NW_TOKEN` 只在绕 `panel-gate` 的 `:8080` 上才需要（`Authorization: Bearer`）。
- 这一页**没有任何写文件的代码** —— 唯一的写都是通过编目自己的 REST（register / recheck / delete）。

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

## 四个动作，各自的边界

| 动作 | 走哪 | 会改什么 | 风险 |
|---|---|---|---|
| 新增 | `POST /api/model`（`/v1/register` 转一手） | 扫上游 `/models` 全量，写 `models.json` + 打一个 git commit | 过闸才写；同名同 baseUrl = 覆盖写 |
| 自查 | `POST /api/model/recheck` | 什么都不改，只比上游 `/models` | 无（202 作业，面板 2 分钟上限，超时那次其实可能已经落账） |
| 密钥 | `GET /api/model/{名}/secret` | 不改，取一次明文就丢 | 剪贴板里会留一份；页面不缓存 |
| 删除 | `DELETE /api/model/{名}` | 改 `models.json` + 打一个 git commit | **不可逆，且没有「谁在用」的保护** |

## 登记（`+ 新增条目`）

`registerDialog()` → `POST /v1/register {baseUrl, api, key, name?}` → 面板 `POST /api/model`。

- **`/v1/register` 是薄壳，不是第二套闸。** 它只挡「注定白跑一趟上游 + 打一次 commit」的四种明显错：
  baseUrl 不是 `http(s)://host…`、方言不在值域、名字带空白/斜杠、密钥空。**不要在这里复刻那五道闸的语义**
  —— 复刻的那份迟早跟 `nw` 漂，而真正决定「写不写」的是 `nw`。
- **闸簿原样回给前端。** 面板失败时 `ok:false`、`data.gates[]` 里那道是 `fail`、后面的全是 `skip`；
  所以 `/v1/register` **一律回 200**（连 `ok:false` 也回 200），前端不用状态码判生死，照 `gates` 摆。
  这个例外是故意的：把闸簿丢在半路，人就只知道「失败了」。
- **弹窗不关，进度和闸簿就在原地长。** 登记要扫全量 + commit + push，实测能到 2 分钟；
  关掉再飘个结果条 = 让人凭记忆对账。表单跑起来后变灰不能改（请求已经发出去了），
  「关掉」（跑着时是「跑着呢…」且禁用）/Esc/点遮罩也都按不住它。
- **密钥只在这一次请求里。** 提交前先从输入框读出、立刻 `value = ""`，然后只进 body；
  页面不存、不回显、不进 localStorage。`autocomplete="new-password"` 是防浏览器的密码管理器抢着填。
- **超时（「面板放弃的那一次」）不算没写。** 面板 2 分钟一刀切，`nw` 可能已经落账了，所以失败分支会额外说一句
  「先刷新看一眼再决定要不要重登」。照着「没写」重跑一遍 = 白多一条 commit。

### 删除的三道闸（都在这一页里，不在编目里）

1. `usedBy` 非空 → 弹窗里先摆「这些配方会指向一个不存在的供应商」；
2. 弹窗自己画（`confirmDelete()`），列清「改哪个文件、会打 commit、不可逆」，点「确认删除」就执行 ——
   **不要求手打名字**（2026-10-29 家长裁定：确认键要一击即中）；
3. 服务端再核一次 `name` 非空，空的直接拒（`/v1/delete` 的「没给名字」）。

弹窗的规矩（改这块前先看 `confirmDelete()`）：Esc / 点遮罩 / 点「取消」都算取消；焦点默认落
「取消」；Tab 只在两个键之间转；点「确认删除」立即收起弹窗，进度交回卡片上的按钮。
**弹窗必须自己画** —— `window.confirm` / `prompt` 会阻塞整个线程、样式不可控，也不给列后果的地方。

## envelope 的两种「失败」

`nw` 是先落账、再 `git push`。push 挂了会把整份 envelope 变成 `ok:false`，
但 `data.written` 已经是 `true`，本地 commit 也打了。整天这台机器到 GitHub 的 TLS 常断，
所以「报错但其实写进去了」是常态。

`unwrap()` 因此把 `written === true` 的失败标成 `partial`，页面显示「已写，但没推到 origin」。
**别把这个分支删掉** —— 删了人会以为没写、反复重跑，每次多一条 commit。

**删除慢是正常的，慢到 110 秒。** 实测一次真删：`git push` 卡到 GnuTLS 超时才回来，
所以 `busy()` 自己会走秒（「删… 12s」），过了 8 秒在卡片里补一句「最长 2 分钟」。
干杵一个不动的「删…」人会以为卡死、反复点 —— 那不是耐心问题，是页面没说实话。

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

# extend（vbe4pt）· pi 扩展管理器 + Nous 扩展的只读视图

这个组是**所有 pi extensions 的管理器** —— 不是某一个扩展的家。盘点盘上散落的扩展，
收进块内库，再装卸到各处。每个子块 = 一个具体的扩展（以后的事）。

**两栏，两套东西，别混成一栏：**上面是 pi 扩展（磁盘文件，改名启停，真相在安装位），
下面是 Nous 扩展（进程内对象，`registry.install()` 装进去就生效，不在盘上）。
给下面那栏套用「改名 /reload」那套开关会得到能点但不起作用的东西。

下面那栏的数据**问 qrd3wh 那个进程要**（`GET /v1/extensions`），不扫盘 ——
仓库里写着什么 ≠ 这一刻装着什么。块之间只走 HTTP，地址只从宿主的 `__nous/ctx` 拿。

## 它管什么、不管什么

- **真相在安装位。** pi 只认盘上的文件（`~/.pi/agent/extensions/` 和各项目的 `.pi/extensions/`）。
  这里的 `data/lib/` 是库/备份，`data/registry.json` 是台账。库和盘不一致时，以盘为准，
  「收回库」就是把盘上的拷回来覆盖。
- **没有运行时热切换。** 启用/停用 = 改安装位上的文件名（`x.ts` ↔ `x.disabled.ts`，
  目录形态是 `index.ts` ↔ `index.disabled.ts`）。改完要在 pi 里 `/reload` 才生效。
- **扫描是白名单制。** 范围 = 全局目录 + `registry.json` 的 `watch` 数组里点过头的目录。
  不扫家目录，`watch` 里的路径必须是绝对路径。

## API

```
GET  /api/inventory                  → { rows[], watch[], globalDir }   对账视图（库 ∪ 盘）
POST /api/adopt    {dir,name}        → item        收进库（盘 → lib/<id>/）
POST /api/install  {id,dir}          → {installed} 装出去（lib → 目标目录，自动建）
POST /api/pull     {id,dir}          → {pulled}    收回库（盘覆盖 lib）
POST /api/toggle   {dir,name,enable} → {ok}        改名启用/停用
POST /api/watch    {dir[,remove]}    → {watch[]}   加/移白名单目录
POST /api/remove   {id}              → {}          删库条目（不碰安装位）
POST /api/save-note{id,notes}        → {ok}        备注
GET  /api/file/<id>/<name>           → {text}      读库内源文件
POST /api/create   {name,template,scope} → item  从模板开新档（templates.js：guard/tool/command/blank）
POST /api/write    {id,file,text}    → {savedAt}  覆写库内文件；保存前自动快照到 data/revs/<id>/（最多 20 版）
GET  /api/revs?id=<id>               → {revs[]}    快照列表（带全文，只读回看用）
POST /api/lint     {text}            → {checks[]}  出厂自查：静态四条（default export / API 调用 / Type 导入 / 花括号），不跑代码

模板住在 templates.js —— 加新模板就是加一个 `label/desc/note/code(name)` 条目。
```

## 数据

```
data/registry.json   台账：{ v, watch[], items[] }，items = { id, name, scope, origin, files, installDirs, notes }
data/lib/<id>/       库内源文件。id 是入库时发的随机键，不改名不重用
```

`cp -r` 这个块走 = 走了整个扩展库。安装位上的文件不跟着走（它们属于各自的机器/项目）。

## 已知盘上的扩展（第一次扫描时的现状）

- `herdr-agent-state` —— 全局（`~/.pi/agent/extensions/`），herdr 装的集成，别动
- `worktree-guard` —— 项目（`/opt/nw-136/.pi/extensions/`），主 worktree 写闸

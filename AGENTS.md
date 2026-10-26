# Nous

## 块和组的字段

**块（leaf）和组（section）是同一个类型 `Node`，没有类型字段。** 判据是 `children` 在不在：
`isSection(n) = Array.isArray(n.children)`（`src/game.ts:20`）。

### 存进 `tree.json` 的（就这四个）

| 字段 | 类型 | 块 | 组 | 说明 |
|---|---|---|---|---|
| `id` | `string` | 必有 | 必有 | 唯一引用键。**6 位随机，如 `7kq2md`**：字母表 32 个（去掉 `i l o u`），`freshId()` 发，`newId()` 在整棵树里查重、撞了就重掷。**新建那一刻定型，改名、换爹、撤销都不动它**。这个字段搬过两次家（`id` → `tree` → `id`，值一次没变），老盘两个名字都认：`adoptIds()`（`store.ts`）载入时接住，下一次落子按新名写回 |
| `name` | `string` | 必有 | 必有 | 人看的那层皮。改名不动 `id`（`renameNode` 只改 `name`） |
| `value` | `number?` | 必有 | **不许有** | 面积就是它。组的值由子节点相加得出，写进去就是两份事实源 |
| `children` | `Node[]?` | **不许有** | 必有（可为空数组） | 唯一的结构字段，同时兼任「是不是组」的判据 |

就是这么四个。没有 `type`、没有 `parent`、没有 `order`、没有 `x`/`y`/`w`/`h`、没有 `collapsed`、没有时间戳 —— 也没有 `tree`：**路径是算出来的,不存盘**(见下表)。

### 派生、不存盘的三样

| 东西 | 怎么算 | 在哪儿 |
|---|---|---|
| `valueOf(n)` | 块返回 `value`，组返回子节点之和 | `game.ts:17` |
| `tree` | 名字路径 `N/src/App.tsx`，同级重名按遍历顺序加 `-2`。改名、搬爹它就变,每次从树现算，**不进 json** | `treeMap()` `game.ts:120` |
| `pathTo` / `childIds` | 钻到某节点的组路径（一串组 id）/ 某组底下的顺序 | `game.ts` |

布局（谁在哪、多大）也是派生的：`layout()` + `squarify()` 从值算出来，丢了重算一次就有。
存它等于把衍生物当事实源。

### 盘上长什么样

```
content/<board-id>/
├── tree.json   { v, name, version, updatedAt, root }   ← 语义唯一真相，丢了就是真丢
├── log.json    { v: 1, moves[] }                        ← 最近 200 手，丢了只是没历史
│                                    move 里指节点的字段也叫 `tree`（老记录是 `id`，只当历史看）
└── map/<nodeId>/
    ├── agent.md          ← 这一块的正文。用户的，程序只在建文件夹时写一次，之后永不覆盖
    ├── README.md         ← `map/` 的索引（id ↔ tree ↔ 名字），每次保存整份重写，纯派生
    └── .gone/<nodeId>/   ← 树里已经没有的节点，连文件夹挪到这儿，**不删**
```

`map/` 的文件夹名是 **id**，不是名字：改名、换爹、撤销都不碰文件系统。对账在 `server/map.ts`，
保存后跑一次，幂等。

### 块自己的页面

```
/p/<board>/<nodeId>/          ← 真实路径（不是 hash）。冷启动、粘贴、后退都认它
GET /api/boards/<board>/nodes/<node>/doc   → { text, etag, stub }
```

四条不变的：

- **正文不进 `tree.json`。** 它有自己的文件、自己的 `etag`（内容哈希），**不参与 board 的 `version`** ——
  写一篇文章不该把树顶成 409，落一手棋也不该让笔记冲突。
- **树里没有的节点不给看。** 文件夹可以躺在 `.gone/` 里，旧书签不能顺着 id 把它读活。
- **文件不在就给 stub，但不落盘。** 看一眼不该长出一个文件。
- **页面盖在盘面上，不卸载盘面。** 卸载会丢掉树的展开状态、滚动位置和撤销栈；
  被盖住的区域用 `inert`，键盘别穿透下去。

## Agent skills

### Issue tracker

Issues are tracked in GitHub Issues (via `gh`). See `docs/agents/issue-tracker.md`.

### Triage labels

Default five canonical triage labels. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `GLOSSARY.md` + `docs/adr/`. See `docs/agents/domain.md`.

# Nous

## 来历（读代码前先看这条）

Nous 起步时把 [Treemap](https://github.com/JoeWest9966/Treemap) 整个搬过来当底座，基线是
`Treemap@8e208e3`。**这不是 fork，是 vendoring**：不合并上游，需要的东西手动挑过来。
理由和边界见 `docs/adr/0001-vendor-treemap-as-substrate.md`。

所以在 `src/game.ts`、`src/Tree.tsx`、`src/store.ts` 里读到「一盘棋」「块」「组」「这盘棋」
这些词，不是 Nous 的领域语言 —— 那是遗产。Nous 自己的词进 `GLOSSARY.md`。

## 块和组的字段

**块（leaf）和组（section）是同一个类型 `Node`，没有类型字段。** 判据是 `children` 在不在：
`isSection(n) = Array.isArray(n.children)`（`src/game.ts:20`）。

### 存进 `tree.json` 的（就这四个）

| 字段 | 类型 | 块 | 组 | 说明 |
|---|---|---|---|---|
| `tree` | `string` | 必有 | 必有 | 唯一引用键。**现在是 `n1` `n2` `n3`… 全局递增**，由 `seedTrees()` 在载入时对齐，刷新/换盘后新值会重号 —— 这是已知待改项。**老的盘上这个字段叫 `id`**，`adoptTrees()`（`store.ts`）在载入时接住，下一次落子就按新名写回 |
| `name` | `string` | 必有 | 必有 | 人看的那层皮。改名不动 `tree`（`renameNode` 只改 `name`） |
| `value` | `number?` | 必有 | **不许有** | 面积就是它。组的值由子节点相加得出，写进去就是两份事实源 |
| `children` | `Node[]?` | **不许有** | 必有（可为空数组） | 唯一的结构字段，同时兼任「是不是组」的判据 |

就是这么四个。没有 `type`、没有 `id`（叫 `tree`）、没有 `parent`、没有 `order`、没有 `x`/`y`/`w`/`h`、没有 `collapsed`、没有时间戳。

### 派生、不存盘的三样

| 东西 | 怎么算 | 在哪儿 |
|---|---|---|
| `valueOf(n)` | 块返回 `value`，组返回子节点之和 | `game.ts:17` |
| `handle` | 名字路径 `N/src/App.tsx`，同级重名按遍历顺序加 `-2`。每次从树现算，**不进 json** | `handleMap()` `game.ts:104` |
| `pathTo` / `childTrees` | 钻到某节点的组路径 / 某组底下的顺序 | `game.ts` |

布局（谁在哪、多大）也是派生的：`layout()` + `squarify()` 从值算出来，丢了重算一次就有。
存它等于把衍生物当事实源。

### 盘上长什么样

```
content/<board-id>/
├── tree.json   { v, name, version, updatedAt, root }   ← 语义唯一真相，丢了就是真丢
└── log.json    { v: 1, moves[] }                        ← 最近 200 手，丢了只是没历史
                                    move 里指节点的字段也叫 `tree`（老记录是 `id`，只当历史看）
```

## Agent skills

### Issue tracker

Issues are tracked in GitHub Issues (via `gh`). See `docs/agents/issue-tracker.md`.

### Triage labels

Default five canonical triage labels. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `GLOSSARY.md` + `docs/adr/`. See `docs/agents/domain.md`.

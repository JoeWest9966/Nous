# 票 1 · 移动：模型

> 这一票**只碰模型**，不碰任何 UI。前端是下一票的事。

## 要做什么

给树加一个能力：把一个块或一个组，挪到另一个组的指定位置。

```ts
// src/game.ts
export function moveNode(root: Node, id: string, to: string, before?: string): Node
```

- `id`：被移动的节点
- `to`：目标父节点（必须是组）
- `before`：插到 `to` 的哪个子节点前面；**省略 = 追加到末尾**

「换爹」和「在同级里排序」是同一件事，都走这一个函数 —— 所以别写两个。

## 必须先读

`AGENTS.md` 的「树模型」那一节。三条铁律，这一票全靠它们：

1. **`children` 数组的顺序就是位置。不许加 `order` 字段。** 加了就有第二份顺序事实，早晚对不上。
2. **`id` 永不改变。** 移动不动 id、不动 `name`、不动 `value`。
3. **`tree`（路径）是算出来的。** 移动之后它自己就变了（`treeMap()` 现算），你什么都不用做。

## 语义细则

| 情况 | 行为 |
|---|---|
| 目标不存在 / 不是组（是块） | 返回原树，不动 |
| `to` 是被移动节点本身，或它的后代 | 返回原树 —— **别造出环** |
| `before` 给了但不在 `to` 的 children 里 | 返回原树 |
| `before` 就是被移动的节点自己 | 返回原树（等于没动） |
| 移动后位置没变（同一父节点、同一位置） | 返回原树 —— `play()` 的早退靠"引用没变"判断，返回新对象会白记一手 |
| **源父节点因此变空** | **保留空组。不裁剪。** `removeNode` 里有"空组自动收掉"的逻辑，别复用、别照搬 —— 一次移动不该顺手删掉一个组，那是两个决定 |

同一父节点内排序的实现顺子：**先摘再算下标**。摘掉之后 `indexOf(before)` 会左移一格，先算下标就会差一位。

## 落子日志

`src/store.ts:10` 的 `Move` union 加一支：

```ts
| { t: "move"; id: string; to: string; before?: string }
```

只记事实，不记结果。撤销靠 `App.tsx` 的 past 栈（快照），不靠重放 —— 所以这里不需要"反向操作"。

## 不许做的事

- 不碰 `demo/`、不碰 `server/`、不碰 `map/`。磁盘上**零动作**：文件夹按 `id` 命名，移动不会让任何文件夹需要 rename。`map/README.md` 下次保存时自己就刷新了。
- 不加 `order`、不加 `parent` 字段。
- 不写 UI。App 里的入口是下一票（票 3），这一票**不用**让它可见。
- 不写 ADR、不加文档章节（AGENTS.md 最后统一补一次）。

## 怎么验

`src/game.ts` 是纯函数、不依赖 React，可以直接被 node 跑（Node 22 type-stripping）：

```bash
cd /opt/Nous && node --experimental-strip-types /tmp/check-move.mjs
```

脚本放 `/tmp`，别进仓库。要覆盖的断言：

1. 换爹：`src/App.tsx` 移到根下 → 位置对、`takenIds()` 前后**完全相同的集合**
2. 同级排序：末尾那块挪到第一位 → children 顺序对，其余不动
3. `before` 省略 → 追加到末尾
4. 移进自己的子树 → 原样返回（引用相等）
5. 移进一个块 → 原样返回
6. `before` 是块自己 → 原样返回
7. 源组被移空 → **空组还在**（`children: []`，没有被裁掉）
8. 移动后 `treeMap()` 里那块的名字路径变了 —— 顺带确认派生值真的跟着走了

```bash
npx tsc --noEmit -p .   # 必须干干净净
```

## 这台机器上两张票同时在跑

所以规矩：

- **不要 `npm run build`，不要 `systemctl restart nous-panel`** —— 那是票 2 的活，两边一起跑会互相踩 `dist/`。
- 提交时**点名路径**，绝不用 `git add -A`（会把另一张票的半成品一起提交）：
  ```bash
  git add src/game.ts src/store.ts
  ```
- push 前先 `git pull --rebase`，另一张票可能先推了。
- 提交信息用中文，说清**为什么**，有实测数字就写数字。

## 完成标准

- [ ] `moveNode` 落地，8 条断言全过
- [ ] `Move` union 多一支
- [ ] `tsc` 干净
- [ ] 一次提交 + 推上去

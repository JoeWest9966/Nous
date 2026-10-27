/**
 * 锁：一块（或一个组）「别删我」。
 *
 * 三条设计上的选择，都写在类型里：
 *
 * 1. **锁是宿主的状态，不是块的内容。** 所以它不住在 `map/<id>/`（那是 AI 和用户的地盘，
 *    一个 `rm` 就没了），而是住在 `content/<board>/.nous/locks.json`。`syncMap` 永远不碰它。
 * 2. **块和组都能锁，但锁的永远是「子树」。** 组是容器，`restoreLocked` 接回时本来就是整枝 clone，
 *    所以「锁组」天然等于「锁里面的东西」—— 想只锁组节点本身是做不到的，硬做出来只会让
 *    「从锁住的组里删一块」和「删掉整个组」给出两种结果。既然如此就把语义说白：
 *    锁住 A，A 底下任何节点都不许消失。`locks.json` 里只存人亲手按下的 id，
 *    继承那一层是**派生**的（`lockMap` 在 `src/game.ts` 里），不存第二份。
 * 3. **锁由服务端强制。** 客户端发来的树里少了锁定的东西，这里把它接回最近的存活祖先。
 *    前端隐藏删除按钮只是防手滑；**防机器靠这一层** —— 所有树写入都必经 `save()`。
 *
 * 做不到的事也说清：能改宿主状态文件的人（root、或者……未来的 AI，等它有了隔离再说）
 * 照样能解锁。这里给的是「没有顺手删掉这回事」和「绕过会留痕」，不是物理防线。
 */
import { appendFile, mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { randomBytes } from "node:crypto";
import path from "node:path";
import type { Node } from "../src/game.ts";

/** 宿主的状态目录。放在 board 里面，但**不属于**树，也不属于任何一个块的文件夹。 */
export const NOUS = ".nous";
const LOCKS = "locks.json";
const AUDIT = "audit.log";

const locksFile = (boardDir: string): string => path.join(boardDir, NOUS, LOCKS);

type LockFile = { v: 1; ids: string[] };

/**
 * 读锁。**不因为文件不存在就报错** —— 绝大多数盘一把锁都没有。
 * 里面认不出来的东西一律当没有：一个坏文件不该让整盘打不开。
 */
export async function readLocks(boardDir: string): Promise<string[]> {
  try {
    const parsed = JSON.parse(await readFile(locksFile(boardDir), "utf8")) as LockFile;
    return Array.isArray(parsed.ids) ? parsed.ids.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

export async function writeLocks(boardDir: string, ids: string[]): Promise<void> {
  const file = locksFile(boardDir);
  await mkdir(path.dirname(file), { recursive: true });
  const body: LockFile = { v: 1, ids: [...new Set(ids)].sort() };
  const tmp = `${file}.${randomBytes(4).toString("hex")}.tmp`;
  await writeFile(tmp, `${JSON.stringify(body, null, 2)}\n`, "utf8");
  await rename(tmp, file);
}

/** 留痕：谁在什么时候试图让一个锁定的块消失。绕过的人改得掉它，但那已经是明确的越界。 */
export async function audit(boardDir: string, line: string): Promise<void> {
  const file = path.join(boardDir, NOUS, AUDIT);
  await mkdir(path.dirname(file), { recursive: true });
  await appendFile(file, `${new Date().toISOString()}\t${line}\n`, "utf8").catch(() => undefined);
}

const walk = (node: Node, out: Node[] = []): Node[] => {
  out.push(node);
  node.children?.forEach((c) => walk(c, out));
  return out;
};

/** 从根到某个 id 的一串节点（含两端）。找不到就是 undefined。 */
const chain = (node: Node, id: string): Node[] | undefined => {
  if (node.id === id) return [node];
  for (const kid of node.children ?? []) {
    const rest = chain(kid, id);
    if (rest) return [node, ...rest];
  }
  return undefined;
};

export type RestoreResult = { root: Node; restored: string[] };

/**
 * 这棵子树里剔掉新树里**还活着**的节点。
 *
 * 只有一种局面用得上：里面一块被搬出去了，然后整枝被删。整枝接回来的时候，
 * 那块已经在新树里活着 —— 再跟着 clone 一份出去，同一个 id 就有了两个节点，
 * `map/` 文件夹、`treeMap`、React key 当场打架。
 */
const prune = (node: Node, live: ReadonlySet<string>): Node | undefined => {
  if (live.has(node.id)) return undefined;
  // 叶子：浅拷贝就够，不跟旧树共享引用。
  if (!node.children) return { ...node };
  const kids = node.children
    .map((k) => prune(k, live))
    .filter((k): k is Node => k !== undefined);
  return { ...node, children: kids };
};

/**
 * 锁是**子树**：人按下的是一个 id，扛事的是它底下整棵树。
 * 所以进 `restoreLocked` 之前先把 `locks.json` 里那串 id 摊开 —— 没有这一步，
 * 「锁住组」就只防住了「整个组被删」，而从组里抽走一块就静悄悄放过去了。
 */
const guard = (prev: Node, locked: ReadonlySet<string>): Set<string> => {
  const out = new Set<string>();
  for (const id of locked) {
    const from = chain(prev, id);
    // 盘上已经没有这把锁了：空头锁，什么也保护不了，但也不用报错。
    if (!from) continue;
    walk(from[from.length - 1]!).forEach((n) => out.add(n.id));
  }
  return out;
};

/**
 * 把客户端弄丢的锁定玩意接回来。锁是子树，所以接回一枝就接回了它底下的全部。
 *
 * 「最近的存活祖先」：沿它**原来**的位置往上找，第一个还在新树里的祖先就收留它；
 * 一路上一个都没活（比如整枝被删、连根都换了），就挂到根上。
 * 挂末尾、按原顺序 —— 不重排，免得每次删东西位置都跳。
 *
 * 两个坑，都是「同一个 id 出现两次」这个病：
 *
 *   1. **先浅后深。** 锁了组 G 又锁了里面的块 B，两个一起没了 —— 必须先接 G，
 *      B 跟着 G 的整枝一起回家；反过来会先把 B 挂到外面，再接 G，于是两个 B。
 *   2. **每接一枝就重算「谁还活着」。** `here` 开头算一次就用到尾，等于假装什么都没变。
 */
export function restoreLocked(next: Node, prev: Node, locked: ReadonlySet<string>): RestoreResult {
  if (!locked.size) return { root: next, restored: [] };
  const guarded = guard(prev, locked);

  const root = next;
  const restored: string[] = [];
  const here = new Set(walk(root).map((n) => n.id));

  const orphans: Node[] = [];
  for (const id of guarded) {
    if (here.has(id)) continue;
    const from = chain(prev, id);
    // 盘上也没有：这把锁是空头，什么也接不回来。
    if (!from) continue;
    orphans.push(from[from.length - 1]!);
  }
  if (!orphans.length) return { root, restored: [] };

  // 浅的先接：组先回家，它底下的锁就一并解决了。
  const depth = (n: Node): number => chain(prev, n.id)?.length ?? 0;
  orphans.sort((a, b) => depth(a) - depth(b));

  for (const node of orphans) {
    // 上一层已经把它连枝接回来了 —— 再搬一次就是第二个它。
    if (here.has(node.id)) continue;
    const clone = prune(node, here);
    if (!clone) continue;

    const old = chain(prev, node.id)!;
    let host: Node = root;
    for (const ancestor of [...old.slice(0, -1)].reverse()) {
      const hit = walk(root).find((n) => n.id === ancestor.id);
      if (hit) {
        host = hit;
        break;
      }
    }
    host.children = [...(host.children ?? []), clone];
    // 收留它的如果原本是个叶子（组被拆空了），它现在有孩子了，`value` 就没意义了。
    delete host.value;
    restored.push(node.id);
    walk(clone).forEach((n) => here.add(n.id));
  }
  return { root, restored };
}

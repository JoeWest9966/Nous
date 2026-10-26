/**
 * 锁：一个块「我做成了 app，别删我」。
 *
 * 三条设计上的选择，都写在类型里：
 *
 * 1. **锁是宿主的状态，不是块的内容。** 所以它不住在 `map/<id>/`（那是 AI 和用户的地盘，
 *    一个 `rm` 就没了），而是住在 `content/<board>/.nous/locks.json`。`syncMap` 永远不碰它。
 * 2. **只有块能锁，组不能锁。** 组里没有内容可保护，锁组等于间接锁子节点 ——
 *    那是同一件事的第二种写法，会带来一堆没有好答案的组合问题。
 * 3. **锁由服务端强制。** 客户端发来的树里少了锁定的块，这里把它接回最近的存活祖先。
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
 * 把客户端弄丢的锁定块接回来。
 *
 * 「最近的存活祖先」：沿它**原来**的位置往上找，第一个还在新树里的祖先就收留它；
 * 一路上一个都没活（比如整枝被删、连根都换了），就挂到根上。
 * 挂末尾、按原顺序 —— 不重排，免得每次删东西位置都跳。
 */
export function restoreLocked(next: Node, prev: Node, locked: ReadonlySet<string>): RestoreResult {
  if (!locked.size) return { root: next, restored: [] };
  const here = new Set(walk(next).map((n) => n.id));
  const orphaned: Node[] = [];
  for (const id of locked) {
    if (here.has(id)) continue;
    const from = chain(prev, id);
    // 盘上也没有：这把锁是空头，什么也接不回来。
    if (!from) continue;
    orphaned.push(from[from.length - 1]!);
  }
  if (!orphaned.length) return { root: next, restored: [] };

  const root = next;
  const restored: string[] = [];
  for (const node of orphaned) {
    const old = chain(prev, node.id)!;
    let host: Node = root;
    for (const ancestor of [...old.slice(0, -1)].reverse()) {
      const hit = walk(root).find((n) => n.id === ancestor.id);
      if (hit) {
        host = hit;
        break;
      }
    }
    // 深拷贝：这棵子树要整枝搬回来，不能和旧树共享引用。
    const clone = structuredClone(node);
    host.children = [...(host.children ?? []), clone];
    // 收留它的如果原本是个叶子（组被拆空了），它现在有孩子了，`value` 就没意义了。
    delete host.value;
    restored.push(node.id);
  }
  return { root, restored };
}

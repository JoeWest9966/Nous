/**
 * `map/` —— 树里的每个节点在磁盘上有一个属于自己的文件夹。
 *
 * ```
 * map/
 * ├── README.md          索引：id ↔ tree ↔ 名字，每次保存重写（纯派生，删了没关系）
 * ├── .gone/<id>/        树里已经没有的节点，连文件夹带内容挪到这儿，**不删**
 * └── <id>/              节点的地盘。**刚建出来是空的**：一没文档，二没代码，
 *                        你写第一个字的时候才长出第一份文档，那份从此归你，永不覆盖
 * ```
 *
 * 文件夹名就是 `id`（随机六位，出生即定型）。所以改名、搬爹、撤销都不碰文件系统 ——
 * 没有 rename，没有半路失败，没有两个名字指同一个东西。撤销回来的节点 id 没变，
 * 文件夹自动就又是它的，`.gone/` 里的内容原样搬回来。
 *
 * 唯一堵不住的：人拿 `mv` 手动改了文件夹名，里面的 AGENTS.md 就和节点失联了。
 * 对账时会当成「缺」重建一个空的，旧内容留在改过名的目录里没人认领。认了吧。
 */
import { mkdir, readdir, rename, writeFile } from "node:fs/promises";
import path from "node:path";
import { randomBytes } from "node:crypto";
import { isSection, treeMap, type Node } from "../src/game.ts";

const MAP = "map";
const GONE = ".gone";
const INDEX = "README.md";

/** id 直接当目录名用，所以这里必须同时挡路径穿越和跨平台怪字符。 */
const SAFE_ID = /^[a-z0-9][a-z0-9-]{0,63}$/;

/** 节点 id 同时是文件夹名，规则和 board id 一样。读写文档之前必须过这一道。 */
export const isNodeId = (id: string): boolean => SAFE_ID.test(id);

const ALPHABET = "0123456789abcdefghjkmnpqrstvwxyz";

/** 跟 `src/game.ts` 里 `freshId()` 同一个规格：32 字母表、6 位、无模偏。 */
export const freshId = (): string => {
  const bytes = randomBytes(6);
  let out = "";
  for (const b of bytes) out += ALPHABET[b & 31];
  return out;
};

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/**
 * 客户端递上来的是一坨 json，不能当它一定是 `Node`。
 * 只收 id 合法的节点 —— 一个坏 id 就能把 `map/` 写到盘外面去。
 *
 * 字段搬过两次家（`id` → `tree` → `id`），两个名字都认：老盘不升级也能长出 map/。
 */
export const adoptNode = (v: unknown): Node | undefined => {
  if (!isRecord(v)) return undefined;
  const raw = typeof v.id === "string" ? v.id : v.tree;
  if (typeof raw !== "string" || !SAFE_ID.test(raw)) return undefined;
  const name = typeof v.name === "string" ? v.name : raw;
  if (Array.isArray(v.children)) {
    const kids = v.children.map(adoptNode).filter((k): k is Node => k !== undefined);
    return { id: raw, name, children: kids };
  }
  return { id: raw, name, value: typeof v.value === "number" ? v.value : 0 };
};

/** 树里还有没有这个 id。写文件之前必查：`.gone/` 里躺着的不能被一个旧页面写活。 */
export const hasNode = (node: Node, id: string): boolean =>
  node.id === id || (node.children?.some((c) => hasNode(c, id)) ?? false);

const exists = async (p: string): Promise<boolean> =>
  (await readdir(path.dirname(p)).catch(() => [] as string[])).includes(path.basename(p));

/** 索引是纯派生的，每次保存整份重写。**它是唯一同时写着 id 和 tree 的地方。** */
const renderIndex = (root: Node | undefined, nodes: Node[]): string => {
  const trees = root ? treeMap(root) : new Map<string, string>();
  const cell = (s: string): string => s.replace(/\|/g, "\\|");
  const rows = nodes.map((n) => {
    const kind = isSection(n) ? "组" : "块";
    return `| \`${n.id}\` | \`${cell(trees.get(n.id) ?? "?")}\` | ${cell(n.name)} | ${kind} |`;
  });
  return [
    "# map",
    "",
    "一个节点一个文件夹，文件夹名 = 节点的 id。**id 永不改变** —— 改名、搬爹、撤销都不动它。",
    "`AGENTS.md` 是你的地盘（也是 agent 进来时自动读的那份）。它是你写第一个字那天才出现的 —— 宿主不铺，之后也永不覆盖。",
    "",
    "树里已经没有的节点，文件夹会挪进 `.gone/`，不删。哪天撤销把它带回来，同样的 id 会把它接回家。",
    "",
    "| id | tree | 名字 | 类型 |",
    "|---|---|---|---|",
    ...rows,
    "",
  ].join("\n");
};

export type SyncResult = { created: number; restored: number; buried: number };

/**
 * 把 `map/` 校准成树的样子。**幂等**：跑一百次和跑一次结果一样。
 *
 * 不抛错 —— 调用方此时 `tree.json` 已经落盘了，让一个笔记目录把保存打成 500 是荒谬的。
 */
export async function syncMap(boardDir: string, rawRoot: unknown): Promise<SyncResult> {
  const out: SyncResult = { created: 0, restored: 0, buried: 0 };
  const root = adoptNode(rawRoot);
  // 认不出这棵树就一个字节都别动：一棵认不出来的树会被当成空树，
  // 空树会把整个 map/ 埋进 .gone/ —— 一个坏请求不该有这种威力。
  if (!root) return out;

  const nodes: Node[] = [];
  const walk = (n: Node): void => {
    nodes.push(n);
    n.children?.forEach(walk);
  };
  walk(root);

  const dir = path.join(boardDir, MAP);
  const graveyard = path.join(dir, GONE);
  await mkdir(dir, { recursive: true });

  const listing = async (d: string): Promise<Set<string>> => {
    const entries = await readdir(d, { withFileTypes: true }).catch(() => []);
    return new Set(entries.filter((e) => e.isDirectory()).map((e) => e.name));
  };

  const present = await listing(dir);

  for (const n of nodes) {
    const live = path.join(dir, n.id);
    if (present.has(n.id)) {
      // 已经有了就一个字节都不动 —— 包括用户删掉的 AGENTS.md。
      continue;
    }
    const buried = path.join(graveyard, n.id);
    if (await exists(buried)) {
      // 撤销把节点带回来了：连文件夹带里面的东西一起接回家。
      await rename(buried, live);
      out.restored++;
      continue;
    }
    // **只建文件夹，一个字节都不写。** 没有 AGENTS.md，没有脚手架，没有 README。
    // 新建的东西一出生就是空的，这是它能说真话的唯一前提。
    // 文档也不预生成：列表里没有它，`doc()` 也不会现编一份。
    await mkdir(live, { recursive: true });
    out.created++;
  }

  const wanted = new Set(nodes.map((n) => n.id));
  for (const name of present) {
    if (wanted.has(name) || name.startsWith(".")) continue;
    // 坟场里已经有同名的（撤销又删、或者 git 把删掉的文件夹复活了）：**换个名字埋**，
    // 绝不跳过。跳过就等于这个文件夹永远留在 map/ 里，和树永久对不上。
    await mkdir(graveyard, { recursive: true });
    let to = path.join(graveyard, name);
    for (let i = 2; await exists(to); i++) to = path.join(graveyard, `${name}-${i}`);
    await rename(path.join(dir, name), to);
    out.buried++;
  }

  await writeFile(path.join(dir, INDEX), renderIndex(root, nodes), "utf8");
  return out;
}

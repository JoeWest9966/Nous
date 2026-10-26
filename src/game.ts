/**
 * 一盘棋：一棵节点树。**没有规则** —— 盘面不会自己变，只因为你加 / 删而动。
 *
 * 两种节点，只有两种（中文只说这两个词：**块** 和 **组**）：
 * - **块**（代码里叫 leaf）—— 有值，面积就是它的值
 * - **组**（代码里叫 section）—— 没有值，值是子节点之和，负责把自己那片地方圈起来
 */
export type Node = {
  id: string;
  name: string;
  /** 只有块有值。组靠子节点算。 */
  value?: number;
  /** 只有组有 children。 */
  children?: Node[];
};

export const valueOf = (n: Node): number =>
  n.children ? n.children.reduce((s, c) => s + valueOf(c), 0) : (n.value ?? 0);

export const isSection = (n: Node): boolean => Array.isArray(n.children);

/**
 * id 的字母表：32 个，去掉 `i l o u` —— 念得出来、抄得对（`o` 一走，`0` 就没歧义了）。
 */
const ALPHABET = "0123456789abcdefghjkmnpqrstvwxyz";

/** 一个新节点的 id。6 位 ≈ 10.7 亿种。256 能被 32 整除，所以 `b & 31` 是均匀的。 */
export const freshId = (): string => {
  const bytes = crypto.getRandomValues(new Uint8Array(6));
  let out = "";
  for (const b of bytes) out += ALPHABET[b & 31];
  return out;
};

/** 整棵树已经用掉的 id。 */
export const takenIds = (root: Node): Set<string> => {
  const out = new Set<string>();
  const walk = (n: Node): void => {
    out.add(n.id);
    n.children?.forEach(walk);
  };
  walk(root);
  return out;
};

/**
 * 发一个这张盘上没人用过的 id：撞了就重掷。
 * 6 位在 1000 个节点下才 0.05% 会撞 —— 但「才」不是「不会」，重掷一次永远是 0。
 */
const newId = (taken: Set<string>): string => {
  let t = freshId();
  while (taken.has(t)) t = freshId();
  return t;
};

/** 某个组底下子节点的 id，按顺序来。落子日志要记「新的是哪个」，就靠它。 */
export const childIds = (root: Node, parent: string): string[] =>
  findNode(root, parent)?.children?.map((c) => c.id) ?? [];

const escapeRe = (s: string): string => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/**
 * 新子块的名字。取「已用过的最大序号 + 1」，不是 `children.length + 1` ——
 * 后者在删掉中间一块之后会重号：删掉 `src.3`，再加一块还是叫 `src.3`。
 * 重名会让同一个名字路径指向两块，所以这里必须堵住。
 */
const nextChildName = (parent: Node): string => {
  const re = new RegExp(`^${escapeRe(parent.name)}\\.(\\d+)$`);
  let max = 0;
  for (const c of parent.children ?? []) {
    const hit = re.exec(c.name);
    if (hit) max = Math.max(max, Number(hit[1]));
  }
  return `${parent.name}.${max + 1}`;
};

export const makeLeaf = (name: string, value: number, id = freshId()): Node => ({
  id,
  name,
  value,
});

export const makeSection = (name: string, children: Node[], id = freshId()): Node => ({
  id,
  name,
  children,
});

/** 开局：pele playground 那棵 treeView-beta（iconpack 示例），值取大致的文件大小。 */
export function initialState(): Node {
  return makeSection("N", [
    makeSection("src", [makeLeaf("App.tsx", 24), makeLeaf("utils.ts", 6)]),
    makeLeaf("Dockerfile", 2),
    makeLeaf("notes.txt", 1),
    makeLeaf("README.md", 9),
  ]);
}

const NAMES = [
  "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
  "iota", "kappa", "lambda", "mu", "nu", "xi", "omicron", "pi",
];

export const findNode = (root: Node, id: string): Node | undefined => {
  if (root.id === id) return root;
  for (const c of root.children ?? []) {
    const hit = findNode(c, id);
    if (hit) return hit;
  }
  return undefined;
};

/**
 * 全树的 **tree**：每个节点在树里的地址（`N/src/App.tsx`），同名按出现顺序加 `-2`。
 *
 * tree 不是 id。改名、搬家它都会变 —— 这是允许的：**id 负责永不失效，tree 负责念得出来**。
 * 所以它不进 tree.json、不进 log.json，每次从树现算；重名编号跟遍历顺序有关，
 * 必须一次算全树，一个个算会算出行数不一致的结果。
 */
export function treeMap(root: Node): Map<string, string> {
  const out = new Map<string, string>();
  const used = new Map<string, number>();
  const walk = (node: Node, prefix: string): void => {
    const base = prefix ? `${prefix}/${node.name}` : node.name;
    const nth = used.get(base) ?? 0;
    used.set(base, nth + 1);
    const tree = nth === 0 ? base : `${base}-${nth + 1}`;
    out.set(node.id, tree);
    // 子节点挂在这一支 resolved 过的 tree 下面 —— 爹带 -2 的话，儿子也跟着带。
    node.children?.forEach((kid) => walk(kid, tree));
  };
  walk(root, "");
  return out;
}

/** 钻到某个节点需要的那条路径。`path` 里只放组，根不算 —— 空数组就是「在根上」。 */
export function pathTo(root: Node, id: string): string[] {
  const rec = (node: Node, acc: string[], isRoot: boolean): string[] | null => {
    if (node.id === id) return isSection(node) && !isRoot ? [...acc, node.id] : acc;
    const next = isSection(node) && !isRoot ? [...acc, node.id] : acc;
    for (const kid of node.children ?? []) {
      const hit = rec(kid, next, false);
      if (hit) return hit;
    }
    return null;
  };
  return rec(root, [], true) ?? [];
}

function replaceNode(root: Node, id: string, next: Node): Node {
  if (root.id === id) return next;
  if (!root.children) return root;
  return { ...root, children: root.children.map((c) => replaceNode(c, id, next)) };
}

/** 往某个组里塞一块（值 1）。 */
export function addLeaf(root: Node, section: string): Node {
  const node = findNode(root, section);
  if (!node || !node.children) return root;
  return replaceNode(root, section, {
    ...node,
    children: [...node.children, makeLeaf(nextChildName(node), 1, newId(takenIds(root)))],
  });
}

/** 往某个组里再加一个组（默认只有一块，值 1）。名字按希腊字母往后排。 */
export function addSection(root: Node, parent: string): Node {
  const node = findNode(root, parent);
  if (!node || !node.children) return root;
  const name =
    NAMES.find((n) => !node.children!.some((c) => c.name === n)) ?? `s${node.children.length + 1}`;
  // 新组和它底下那块要两个 id，一次把两个都发掉，别让它们互相撞。
  const taken = takenIds(root);
  const section = newId(taken);
  taken.add(section);
  const leaf = newId(taken);
  return replaceNode(root, parent, {
    ...node,
    children: [...node.children, makeSection(name, [makeLeaf(`${name}.1`, 1, leaf)], section)],
  });
}

/**
 * 删掉一个节点（连同它的子树），并把因此空掉的组一路收掉。
 * 根不删 —— 空盘也还叫 N，只是底下什么都没有。
 */
export function removeNode(root: Node, id: string): Node {
  if (root.id === id) return root;

  const prune = (n: Node): Node => {
    if (!n.children) return n;
    const kids = n.children
      .filter((c) => c.id !== id)
      .map(prune)
      .filter((c) => !isSection(c) || c.children!.length > 0);
    return { ...n, children: kids };
  };

  return prune(root);
}

/**
 * 改名。id 不动 —— 路径、焦点、撤销栈全靠 id，名字只是一层皮。
 * 空名、纯空格、没改，都当没下这一手（返回原树，`play` 的早退会兜住）。
 */
export function renameNode(root: Node, id: string, name: string): Node {
  const node = findNode(root, id);
  const trimmed = name.trim();
  if (!node || !trimmed || node.name === trimmed) return root;
  return replaceNode(root, id, { ...node, name: trimmed });
}

export type Box = { x: number; y: number; w: number; h: number };

/** 越小越接近正方形。squarified 的核心判据。 */
function worst(row: number[], side: number): number {
  const sum = row.reduce((a, b) => a + b, 0);
  const max = Math.max(...row);
  const min = Math.min(...row);
  return Math.max((side * side * max) / (sum * sum), (sum * sum) / (side * side * min));
}

/**
 * squarified treemap：沿着短边一行一行地铺，每加一块都算一次「变得更方还是更扁」。
 * 换来的是块接近正方形 —— 面积一样，但看起来像砖墙，不像竖条。
 */
export function squarify(values: number[], into: Box): Box[] {
  const total = values.reduce((a, b) => a + b, 0) || 1;
  const scaled = values.map((v) => (v / total) * into.w * into.h);
  const out: Box[] = [];
  const free: Box = { ...into };
  let row: number[] = [];

  const flush = () => {
    if (!row.length) return;
    const sum = row.reduce((a, b) => a + b, 0);
    if (free.w >= free.h) {
      const w = sum / free.h;
      let y = free.y;
      for (const v of row) {
        const h = v / w;
        out.push({ x: free.x, y, w, h });
        y += h;
      }
      free.x += w;
      free.w -= w;
    } else {
      const h = sum / free.w;
      let x = free.x;
      for (const v of row) {
        const w = v / h;
        out.push({ x, y: free.y, w, h });
        x += w;
      }
      free.y += h;
      free.h -= h;
    }
    row = [];
  };

  for (const v of scaled) {
    const side = Math.min(free.w, free.h);
    if (!row.length || worst([...row, v], side) <= worst(row, side)) row.push(v);
    else {
      flush();
      row.push(v);
    }
  }
  flush();
  return out;
}

/** 一个组（section）的子节点在这个矩形里怎么摆。 */
export function layout(node: Node, box: Box, depth = 0): { node: Node; box: Box; depth: number }[] {
  const kids = node.children;
  if (!kids?.length) return [];
  return squarify(kids.map(valueOf), box).map((b, i) => ({ node: kids[i]!, box: b, depth: depth + 1 }));
}

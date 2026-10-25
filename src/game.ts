/**
 * 一盘棋：节点树 + 三条规则。
 *
 * 两种节点，只有两种：
 * - **leaf**（叶子块）—— 有值，能点，面积就是它的值
 * - **section**（分区）—— 没有值，值是子节点之和，负责把一块地方圈起来
 *
 * 三条规则：
 * - 点 leaf：值翻倍
 * - 同一 section 里两个**叶子**值相等：自动合并成一块
 * - 值涨到 `EXPAND_AT`：这个 leaf 升格成 section，自己裂成 `PER_EXPANSION` 个叶子
 */
export type Node = {
  id: string;
  name: string;
  /** 只有 leaf 有值。section 靠子节点算。 */
  value?: number;
  /** 只有 section 有 children。 */
  children?: Node[];
};

export const EXPAND_AT = 8;
export const PER_EXPANSION = 4;

export const valueOf = (n: Node): number =>
  n.children ? n.children.reduce((s, c) => s + valueOf(c), 0) : (n.value ?? 0);

export const isSection = (n: Node): boolean => Array.isArray(n.children);

let seq = 0;
const nextId = (): string => `n${++seq}`;

export const makeLeaf = (name: string, value: number): Node => ({
  id: nextId(),
  name,
  value,
});

export const makeSection = (name: string, children: Node[]): Node => ({
  id: nextId(),
  name,
  children,
});

/** 开局：N 底下**一个 section + 一个 leaf** —— 两种节点各一个，一眼看清。 */
export function initialState(): Node {
  return makeSection("N", [
    makeSection("alpha", [makeLeaf("alpha.1", 1), makeLeaf("alpha.2", 1)]),
    makeLeaf("beta", 1),
  ]);
}

const NAMES = [
  "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
  "iota", "kappa", "lambda", "mu", "nu", "xi", "omicron", "pi",
];

function findParent(root: Node, id: string): Node | undefined {
  if (!root.children) return undefined;
  if (root.children.some((c) => c.id === id)) return root;
  for (const c of root.children) {
    const hit = findParent(c, id);
    if (hit) return hit;
  }
  return undefined;
}

export const findNode = (root: Node, id: string): Node | undefined => {
  if (root.id === id) return root;
  for (const c of root.children ?? []) {
    const hit = findNode(c, id);
    if (hit) return hit;
  }
  return undefined;
};

/**
 * 同值合并：把第一对相等的**叶子**并成一块，合出来的可能又相等，继续。
 * 只合并叶子 —— section 和 leaf 撞上同一个值不该互相吃掉，那是两种东西。
 */
function mergeEqual(parent: Node): Node {
  const kids = [...(parent.children ?? [])];
  for (;;) {
    const i = kids.findIndex((a, i2) =>
      !isSection(a) &&
      kids.some((b, j) => j > i2 && !isSection(b) && valueOf(a) === valueOf(b)),
    );
    if (i < 0) break;
    const j = kids.findIndex(
      (b, k) => k > i && !isSection(b) && valueOf(b) === valueOf(kids[i]!),
    );
    if (j < 0) break;
    const merged: Node = {
      id: kids[j]!.id,
      name: kids[i]!.name,
      value: valueOf(kids[i]!) + valueOf(kids[j]!),
    };
    kids.splice(j, 1);
    kids.splice(i, 1, merged);
  }
  return { ...parent, children: kids };
}

/** 够大就升格：叶子裂成一个 section，里面 `PER_EXPANSION` 个叶子各拿 1/4。 */
function expand(node: Node): Node {
  if (isSection(node) || valueOf(node) < EXPAND_AT) return node;
  const each = valueOf(node) / PER_EXPANSION;
  return {
    id: node.id,
    name: node.name,
    children: Array.from({ length: PER_EXPANSION }, (_, i) =>
      makeLeaf(`${node.name}.${i + 1}`, each),
    ),
  };
}

/**
 * 点一下：值翻倍 → 同组合并 → 升格。
 * 只处理被点的那一个 parent，且升格后不再往下合并 —— 否则「裂开→并回去→再裂开」会死循环。
 */
export function bump(root: Node, id: string): Node {
  const node = findNode(root, id);
  const parent = findParent(root, id);
  if (!node || !parent || isSection(node)) return root;

  const doubled: Node = { ...node, value: valueOf(node) * 2 };
  const withDoubled: Node = {
    ...parent,
    children: parent.children!.map((c) => (c.id === id ? doubled : c)),
  };
  const merged = mergeEqual(withDoubled);
  return replaceNode(root, parent.id, {
    ...merged,
    children: merged.children!.map(expand),
  });
}

function replaceNode(root: Node, id: string, next: Node): Node {
  if (root.id === id) return next;
  if (!root.children) return root;
  return { ...root, children: root.children.map((c) => replaceNode(c, id, next)) };
}

/** 往某个 section 里塞一个叶子（值 1）。 */
export function addLeaf(root: Node, sectionId: string): Node {
  const section = findNode(root, sectionId);
  if (!section || !section.children) return root;
  const n = section.children.length + 1;
  return replaceNode(root, sectionId, {
    ...section,
    children: [...section.children, makeLeaf(`${section.name}.${n}`, 1)],
  });
}

/** 往某个 section 里再加一个 section（两个值为 1 的叶子）。名字按希腊字母往后排。 */
export function addSection(root: Node, parentId: string): Node {
  const parent = findNode(root, parentId);
  if (!parent || !parent.children) return root;
  const name =
    NAMES.find((n) => !parent.children!.some((c) => c.name === n)) ?? `s${parent.children.length + 1}`;
  return replaceNode(root, parentId, {
    ...parent,
    children: [...parent.children, makeSection(name, [makeLeaf(`${name}.1`, 1), makeLeaf(`${name}.2`, 1)])],
  });
}

/**
 * 删掉一个节点（连同它的子树），并把因此空掉的分区一路收掉。
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

/** 一个节点（section）的子节点在这个矩形里怎么摆。 */
export function layout(node: Node, box: Box, depth = 0): { node: Node; box: Box; depth: number }[] {
  const kids = node.children;
  if (!kids?.length) return [];
  return squarify(kids.map(valueOf), box).map((b, i) => ({ node: kids[i]!, box: b, depth: depth + 1 }));
}

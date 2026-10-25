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

let seq = 0;
const nextId = (): string => `n${++seq}`;

/**
 * 从载入的树上重新对齐计数器 —— 换项目的时候必须调。
 * 不调的话新 id 会从 0 重新数，撞上文件里已有的 id：React key 打架、
 * `findNode` 找错人、log 里的 id 全部指错。
 */
export function seedIds(root: Node): void {
  const walk = (n: Node): void => {
    const hit = /^n(\d+)$/.exec(n.id);
    if (hit) seq = Math.max(seq, Number(hit[1]));
    n.children?.forEach(walk);
  };
  seq = 0;
  walk(root);
}

/** 某个组底下子节点的 id，按顺序来。落子日志要记「新的是哪个」，就靠它。 */
export const childIds = (root: Node, parentId: string): string[] =>
  findNode(root, parentId)?.children?.map((c) => c.id) ?? [];

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

function replaceNode(root: Node, id: string, next: Node): Node {
  if (root.id === id) return next;
  if (!root.children) return root;
  return { ...root, children: root.children.map((c) => replaceNode(c, id, next)) };
}

/** 往某个组里塞一块（值 1）。 */
export function addLeaf(root: Node, sectionId: string): Node {
  const section = findNode(root, sectionId);
  if (!section || !section.children) return root;
  const n = section.children.length + 1;
  return replaceNode(root, sectionId, {
    ...section,
    children: [...section.children, makeLeaf(`${section.name}.${n}`, 1)],
  });
}

/** 往某个组里再加一个组（默认只有一块，值 1）。名字按希腊字母往后排。 */
export function addSection(root: Node, parentId: string): Node {
  const parent = findNode(root, parentId);
  if (!parent || !parent.children) return root;
  const name =
    NAMES.find((n) => !parent.children!.some((c) => c.name === n)) ?? `s${parent.children.length + 1}`;
  return replaceNode(root, parentId, {
    ...parent,
    children: [...parent.children, makeSection(name, [makeLeaf(`${name}.1`, 1)])],
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

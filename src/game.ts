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
 * 锁是**子树**：锁住一个组，组里的东西也不许被删。
 *
 * 所以「谁被锁了」不能只看 `locks.json` —— 那里面只有人亲手按下的那几个 id，
 * 真正生效的是它连同底下整棵树。回一张 `节点 id → 握着这把锁的最近祖先 id`：
 * 值等于自己就是亲手锁的，不等就是继承来的。
 *
 * 这张表是**派生**的，不进树、不进盘 —— 真相永远是那串 id。UI 要它只有一个用途：
 * 说清楚「为什么这块删不掉」，不然人只看到按钮没了。
 */
export function lockMap(root: Node, locks: ReadonlySet<string>): Map<string, string> {
  const out = new Map<string, string>();
  if (!locks.size) return out;
  const walk = (node: Node, above: readonly string[]): void => {
    const chain = [node.id, ...above];
    const owner = chain.find((id) => locks.has(id));
    if (owner !== undefined) out.set(node.id, owner);
    node.children?.forEach((c) => walk(c, chain));
  };
  walk(root, []);
  return out;
}

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
 * 块 → 组。**「转」在这套模型里就是让它长出 `children`** —— 没有类型字段，
 * 所以 id 不动、名字不动、文件夹不动、文档不动、锁不动、地址不动。
 *
 * 原值交给唯一那个孩子：面积守恒。不这么做的话，一块 24 的块变成组就塌成一粒 0，
 * 看着像被偷了。孩子名字走 `nextChildName`（`<name>.1`），和「往组里加一块」同一套。
 *
 * 根不转（它是门面，必须是容器），已经是组的不转。不成立就返回原树本身 ——
 * `play()` 靠「引用没变」早退，返回新对象会白记一手。
 */
export function toSection(root: Node, id: string): Node {
  const node = findNode(root, id);
  if (!node || isSection(node) || node.id === root.id) return root;
  const bare: Node = { id: node.id, name: node.name, children: [] };
  const kid = makeLeaf(nextChildName(bare), node.value ?? 1, newId(takenIds(root)));
  return replaceNode(root, id, { id: node.id, name: node.name, children: [kid] });
}

/**
 * 组 → 块：把这一枝收成一块，值 = 底下之和（面积守恒，和 `toSection` 正好互逆）。
 *
 * **这是全盘唯一会「让东西离开盘面」的操作。** id 不动（文件夹、文档、锁、地址都还在），
 * 但孩子连同它们的 `map/<id>/` 会从树里掉出去 —— 对账时整枝塞进 `.gone/`，不删。
 * 所以撤销栈在本次会话里能把它们原样接回来（同 id → 文件夹从坟场回家），
 * **刷新之后就没有回头路了**：调用方必须先说清这一点（菜单那条确认就是为这个）。
 *
 * 根不转，块不转，空组不转。
 */
export function toLeaf(root: Node, id: string): Node {
  const node = findNode(root, id);
  if (!node || !isSection(node) || node.id === root.id || !node.children!.length) return root;
  return replaceNode(root, id, { id: node.id, name: node.name, value: valueOf(node) });
}

/** 一棵子树上所有节点的 id（含自己）。组转块时要把「谁被埋了」记进日志。 */
export function subtreeIds(node: Node): string[] {
  const out: string[] = [];
  const walk = (n: Node): void => {
    out.push(n.id);
    n.children?.forEach(walk);
  };
  walk(node);
  return out;
}

/** id 顺序一模一样？用来判「这一手其实没动」。 */
const sameIds = (a: string[], b: string[]): boolean =>
  a.length === b.length && a.every((v, i) => v === b[i]);

/**
 * 把 id 从树里摘掉，**不收空组** —— 一次移动不该顺手删掉一个组，那是两个决定。
 * 裁剪是 `removeNode` 的事，别照搬过来。根摘不动。
 */
function detach(root: Node, id: string): Node {
  if (root.id === id || !root.children) return root;
  return { ...root, children: root.children.filter((c) => c.id !== id).map((c) => detach(c, id)) };
}

/**
 * 把 node 插进 to，插在 before 前面；没有 before 就是追加到末尾。
 * `at < 0` 兜一手：摘完之后的树上找不到 before 只能追加，不能喂给 splice(-1)
 * —— 那会把节点插到倒数第一位，一种很难看的静默错。
 */
function insertBefore(root: Node, to: string, node: Node, before?: string): Node {
  if (!root.children) return root;
  if (root.id === to) {
    const kids = root.children;
    const found = before === undefined ? -1 : kids.findIndex((c) => c.id === before);
    const at = found < 0 ? kids.length : found;
    return { ...root, children: [...kids.slice(0, at), node, ...kids.slice(at)] };
  }
  return { ...root, children: root.children.map((c) => insertBefore(c, to, node, before)) };
}

/**
 * 这一手能不能下。`moveNode` 的判据抽出来给选择器用 —— 目标列表要提前把不成立的落点
 * 划掉，不能让人点了才发现什么都没发生。两边必须是同一份判据，不然早晚分家。
 */
export function canMove(root: Node, id: string, to: string, before?: string): boolean {
  if (id === to) return false;
  const node = findNode(root, id);
  const target = findNode(root, to);
  if (!node || !target || !isSection(target)) return false;
  // `to` 在被搬的东西自己的子树里 → 会造环。根也在这条里被挡住（根不搬）。
  if (findNode(node, to)) return false;
  // `before` 不在 to 的直接子节点里，或者就是被搬的这块自己 → 当没下这一手。
  // 后者不是边角：被判移动的那一行本身也是落点（灰在那儿），点它/回车就该什么也不发生。
  if (before !== undefined && (before === id || !target.children!.some((c) => c.id === before)))
    return false;
  return true;
}

/**
 * 把一个块或一个组挪到另一个组的指定位置 —— 「换爹」和「同级排序」是同一件事，
 * 所以只有这一个函数。`before` 省略 = 追加到末尾。
 *
 * 三件不许动的东西：`id`、`name`、`value`。`tree`（路径）是算出来的，它自己会跟着变。
 * 什么都不成立时返回**原树本身**：`play()` 靠「引用没变」早退，返回新对象会白记一手。
 */
export function moveNode(root: Node, id: string, to: string, before?: string): Node {
  if (!canMove(root, id, to, before)) return root;
  const node = findNode(root, id)!;

  // 先摘再插 —— 先算下标会差一位，所以下标一定是在摘完之后的树上现算。
  const next = insertBefore(detach(root, id), to, node, before);
  return sameIds(childIds(root, to), childIds(next, to)) ? root : next;
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

/**
 * 一格的底线尺寸。低于它，「进入 ›」和「•••」就挤没了 —— 这是列数的真正约束。
 * 140 宽：名字 + 进入按钮；84 高：内缩 2px 后还剩 80，组的标题带才够得着 24px 那一档
 * （`hat()` 要求 h > 76）—— 24 以下「N 块」就隐了。
 */
export const MIN_CELL_W = 140;
export const MIN_CELL_H = 84;

/** 一行最多几个 —— 天花板，不是目标。地板是 MIN_CELL_W，两个一起决定列数。 */
export const maxCols = (width: number): number => (width < 640 ? 2 : width < 1100 ? 4 : 8);

/** 这一层铺成网格是几列、几行。列数只由宽度算，所以内层格子窄，列数自动就少 —— 不用特判。 */
export function grid(count: number, width: number): { cols: number; rows: number } {
  const cap = Math.min(Math.floor(width / MIN_CELL_W) || 1, maxCols(width));
  const rows = Math.max(1, Math.ceil(count / cap));
  // 再摊一次：4+2 这种“首行满、末行只剩两个”的岔子，压成 3+3。摊完列数只会更少，不会破地板。
  const cols = Math.max(1, Math.ceil(count / rows));
  return { cols, rows };
}

/**
 * 这一层铺成网格要占多高：装得下就是给它的高，装不下就往下长。
 * 只有最外层该用它 —— 内层长出去会被自己的格子裁掉，那是 bug 不是滚动。
 */
export function gridHeight(count: number, box: Box): number {
  return Math.max(box.h, grid(count, box.w).rows * MIN_CELL_H);
}

/**
 * 一个组（section）的子节点在这个矩形里怎么摆：等大的格子，最后一行平分剩下的宽度。
 *
 * 面积不再等于值 —— 列数一管，面积就归网格了。值改由两样东西说：块中间那个数字，
 * 和底部那条 `share`（同组内按最大值归一化）比例条。
 */
export function layout(
  node: Node,
  box: Box,
  depth = 0,
): { node: Node; box: Box; depth: number; share: number }[] {
  const kids = node.children;
  if (!kids?.length) return [];
  const { cols, rows } = grid(kids.length, box.w);
  const cellH = box.h / rows;
  const values = kids.map(valueOf);
  const max = Math.max(...values) || 1;
  return kids.map((node, i) => {
    const row = Math.floor(i / cols);
    const left = kids.length - row * cols;
    // 末行要么补满整行（左边不留沟），要么就按原宽度摆着 ——
    // 但只剩两三个时一律不拉：那会把一块拉成别人的两倍宽，看起来像“它比旁边大”。
    const cellW = box.w / (row === rows - 1 && left > cols / 2 ? left : cols);
    return {
      node,
      box: { x: box.x + (i - row * cols) * cellW, y: box.y + row * cellH, w: cellW, h: cellH },
      depth: depth + 1,
      share: values[i]! / max,
    };
  });
}

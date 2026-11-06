// 面板的纯计算部分 —— 不碰 DOM、不 fetch、不看时间，所以能裸跑 `node --test`。
//
// 判据只在一处：spec §7。卡片状态按优先级取第一个命中的（failed > unpublished > ok），
// 归档不在网格里；facet 计数对**全量**算（导航不是统计）；类别是路径，父子关系现拆。

export const CATEGORY_SEP = "/";
/** 「未分类」是算出来的桶，不是仓库里的一条类别 —— 别把这个字符串写进任何文件。 */
export const UNCATEGORIZED = "__none__";
export const ARCHIVED = "__archived__";
export const STATE_PREFIX = "__state__:";

export const STATE_LABEL = {
  ok: "正常",
  unpublished: "未发布",
  failed: "装载失败",
  archived: "已归档",
  upstream: "上游",
};

/** 网格里的先后：能用的先看，待发布的其次，坏了的再其次；上游沉底；归档根本不进网格。 */
const STATE_RANK = { ok: 0, unpublished: 1, failed: 2, upstream: 3 };

export const stateFilterOf = (value) =>
  typeof value === "string" && value.startsWith(STATE_PREFIX) ? value.slice(STATE_PREFIX.length) : null;
export const stateFilter = (state) => STATE_PREFIX + state;

/** 类别规范化：去首尾空白、连续 / 压成一个、丢空段。服务端还管长度和字符，这里只求同一套形状。 */
export function normalizeCategory(value) {
  return String(value ?? "")
    .split(CATEGORY_SEP)
    .map((segment) => segment.trim())
    .filter(Boolean)
    .join(CATEGORY_SEP);
}

export function ancestorsOf(path) {
  const parts = normalizeCategory(path).split(CATEGORY_SEP).filter(Boolean);
  return parts.slice(0, -1).map((_, index) => parts.slice(0, index + 1).join(CATEGORY_SEP));
}

export const isUnder = (path, root) => path === root || path.startsWith(root + CATEGORY_SEP);

export function depthOf(path) {
  const parts = normalizeCategory(path).split(CATEGORY_SEP).filter(Boolean);
  return Math.max(0, parts.length - 1);
}

export const leafOf = (path) => normalizeCategory(path).split(CATEGORY_SEP).filter(Boolean).at(-1) ?? "";

/** 树里同一层的顺序：按段比（zh 排序），前缀短的在前。 */
export function compareCategories(a, b) {
  const left = normalizeCategory(a).split(CATEGORY_SEP);
  const right = normalizeCategory(b).split(CATEGORY_SEP);
  for (let i = 0; i < Math.min(left.length, right.length); i++) {
    const cmp = left[i].localeCompare(right[i], "zh-CN");
    if (cmp) return cmp;
  }
  return left.length - right.length;
}

/** 一条技能挂了哪些类别（规范化 + 去重）。`[]` = 未分类，不写"未分类"三个字进文件。 */
export function categoriesOf(skill) {
  const list = Array.isArray(skill?.category) ? skill.category : [];
  const out = [];
  for (const raw of list) {
    const path = normalizeCategory(raw);
    if (path && !out.includes(path)) out.push(path);
  }
  return out;
}

/** 三种状态，只取第一个命中的。技能没有「装 / 卸」，所以别处也长不出第四种。 */
export function cardState(skill) {
  if (skill?.load?.state === "failed") return "failed";
  if (skill?.tracked === false) return "unpublished";
  return "ok";
}

/**
 * 上游的一条 → 网格里的一张卡。
 *
 * `kind` 是**唯一**分得清「我们自己的」和「别人仓里的」的东西：两边会有同名
 * （上游撞上来我们管不了），所以卡片上认身份不能靠名字。
 */
export function upstreamCard(entry) {
  return { ...entry, kind: "upstream", state: "upstream" };
}

/**
 * facet 计数：对**全量**技能算，父级 = 自身 + 整枝，同一条技能挂了父子只算一次。
 * `vocabulary` 里的路径先落 0 —— 一个刚建、还没挂到任何技能上的类别不能建了看不见。
 */
export function facetCounts(skills, vocabulary = []) {
  const counts = new Map();
  const seed = (path) => {
    for (const parent of ancestorsOf(path)) if (!counts.has(parent)) counts.set(parent, 0);
    if (!counts.has(path)) counts.set(path, 0);
  };
  for (const raw of vocabulary) {
    const path = normalizeCategory(raw);
    if (path) seed(path);
  }
  for (const skill of skills) {
    const touched = new Set();
    for (const tag of categoriesOf(skill)) {
      for (const parent of ancestorsOf(tag)) touched.add(parent);
      touched.add(tag);
    }
    for (const path of touched) counts.set(path, (counts.get(path) ?? 0) + 1);
  }
  return counts;
}

/** 类别词表 = `categories.json` 的词表 ∪ 每条技能自己的 category ∪ 上游那些注记，补上祖先，排好序。 */
export function categoryVocabulary(data, extra = []) {
  const set = new Set();
  const add = (raw) => {
    const path = normalizeCategory(raw);
    if (!path) return;
    for (const parent of ancestorsOf(path)) set.add(parent);
    set.add(path);
  };
  for (const raw of Array.isArray(data?.categories) ? data.categories : []) add(raw);
  for (const skill of Array.isArray(data?.skills) ? data.skills : []) {
    for (const tag of categoriesOf(skill)) add(tag);
  }
  for (const card of Array.isArray(extra) ? extra : []) {
    for (const tag of categoriesOf(card)) add(tag);
  }
  return [...set].sort(compareCategories);
}

// 同名不算并列：本地的在前（它是我们自己的），上游的在后 —— 顺序不能跟着数据来。
const compareCards = (a, b) =>
  (STATE_RANK[a.state] ?? 9) - (STATE_RANK[b.state] ?? 9) ||
  a.name.localeCompare(b.name) ||
  Number(a.kind === "upstream") - Number(b.kind === "upstream") ||
  String(a.path ?? "").localeCompare(String(b.path ?? ""));

/**
 * `GET /v1/skills` 的响应（+ `GET /v1/skills/upstream` 的 entries）→ 页面要的那一份。
 *
 * 上游的条目**并进同一张网格**：它们在「全部」和「未分类」里跟我们自己的技能并排。
 * 区别只有一个 —— `kind`，它决定颜色和菜单（见 `app.js` 的 `productCard`）。
 * 传入上游用的是**原样的 entries**，不先转卡：调用方手里那份还要给上游那一屏用。
 */
export function buildView(data, upstreamEntries = []) {
  const skills = Array.isArray(data?.skills) ? data.skills : [];
  const upstream = (Array.isArray(upstreamEntries) ? upstreamEntries : []).map(upstreamCard);
  const all = [...skills, ...upstream];
  const vocabulary = categoryVocabulary(data, upstream);
  const counts = facetCounts(all, Array.isArray(data?.categories) ? data.categories : []);
  const cards = [...skills.map((skill) => ({ ...skill, state: cardState(skill) })), ...upstream].sort(compareCards);
  const archived = (Array.isArray(data?.archived) ? data.archived : [])
    .map((name) => ({ name, state: "archived" }))
    .sort((a, b) => a.name.localeCompare(b.name));
  const uncategorized = all.filter((skill) => !categoriesOf(skill).length).length;
  const rows = vocabulary.map((path) => ({
    path,
    label: leafOf(path),
    depth: depthOf(path),
    count: counts.get(path) ?? 0,
    parent: vocabulary.some((other) => other !== path && isUnder(other, path)),
  }));
  return {
    cards,
    archived,
    counts,
    uncategorized,
    categoryNames: vocabulary,
    rows,
    skillCount: cards.length,
    localCount: skills.length,
    upstreamCount: upstream.length,
    total: cards.length + archived.length,
  };
}

/** 筛选：未分类桶 / 状态 / 类别整枝，再叠一层搜索。计数不参与，所以筛完计数不变。 */
export function filterCards(cards, { filter = "", query = "" } = {}) {
  let out = Array.isArray(cards) ? cards : [];
  if (filter === UNCATEGORIZED) {
    out = out.filter((card) => !categoriesOf(card).length);
  } else if (filter) {
    const state = stateFilterOf(filter);
    if (state) out = out.filter((card) => card.state === state);
    else out = out.filter((card) => categoriesOf(card).some((tag) => isUnder(tag, filter)));
  }
  const needle = String(query ?? "").trim().toLowerCase();
  if (needle) {
    out = out.filter((card) =>
      // `summary` 只有上游那些卡上有；`source` **不进搜索** —— 一条订阅底下几十条
      // 全带着同一个名字，搜「matt」会把整个上游都捞出来，那不是筛，那是噪声。
      [card.name, card.description, card.summary, ...categoriesOf(card)]
        .join(" ")
        .toLowerCase()
        .includes(needle),
    );
  }
  return out;
}

/** 删 / 改名这个类别会动到几条技能 —— 整枝算，不是只算直接挂着它的那几条。 */
export function categoryImpact(skills, path) {
  const target = normalizeCategory(path);
  if (!target) return 0;
  return (Array.isArray(skills) ? skills : []).filter((skill) =>
    categoriesOf(skill).some((tag) => isUnder(tag, target)),
  ).length;
}

/** 这一枝下面有几个子类别（确认框里那句「连同 N 个子类别」）。 */
export function childCount(names, path) {
  const target = normalizeCategory(path);
  return (Array.isArray(names) ? names : []).filter((name) => name !== target && isUnder(name, target)).length;
}

// ── 正文抽屉里的文件树 ──────────────────────────────────────────────────────
//
// 文件列表是扁的（`docs/a.md`），抽屉里要长成一棵树。`/` 是唯一的分隔符 ——
// 目录**不**单独存一行，它由「有文件住在这儿」推出来：文件夹里没文件，它本来就不该在。
// 所以这里没有「空目录」这种状态：建文件夹就是建一个带 `.gitkeep` 的目录（服务端的事）。

/** 同一层：文件夹在前、文件在后，各自按名字排（zh-CN）。 */
function sortTree(node) {
  node.children.sort((a, b) => {
    if (a.type !== b.type) return a.type === "dir" ? -1 : 1;
    return a.name.localeCompare(b.name, "zh-CN");
  });
  for (const child of node.children) if (child.type === "dir") sortTree(child);
}

/**
 * 扁平文件列表 → 树。返回一个虚拟根（`path: ""`），孩子才是要渲染的行。
 *
 * @param {Array<{path: string, size?: number}>} files
 * @returns {{type: "dir", path: string, name: string, depth: number, children: Array<object>}}
 */
export function fileTree(files) {
  const root = { type: "dir", path: "", name: "", depth: -1, children: [] };
  const dirs = new Map([["", root]]);
  for (const file of Array.isArray(files) ? files : []) {
    const path = String(file?.path ?? "").trim().replace(/^\/+/, "");
    const parts = path.split("/").filter(Boolean);
    if (!parts.length) continue;
    let parent = root;
    for (let i = 0; i < parts.length - 1; i++) {
      const dirPath = parts.slice(0, i + 1).join("/");
      let node = dirs.get(dirPath);
      if (!node) {
        node = { type: "dir", path: dirPath, name: parts[i], depth: i, children: [] };
        dirs.set(dirPath, node);
        parent.children.push(node);
      }
      parent = node;
    }
    parent.children.push({
      type: "file",
      path,
      name: parts.at(-1),
      depth: parts.length - 1,
      size: Number(file?.size) || 0,
    });
  }
  sortTree(root);
  return root;
}

/** 树 → 当前该显示的行（折叠起来的文件夹不往里走）。`isOpen(path)` 由调用方给。 */
export function flattenTree(node, isOpen = () => true) {
  const out = [];
  const walk = (parent) => {
    for (const child of parent.children) {
      out.push(child);
      if (child.type === "dir" && isOpen(child.path)) walk(child);
    }
  };
  walk(node);
  return out;
}

/** 树里所有文件夹的 path（打开抽屉时默认全展开）。 */
export function dirPaths(node) {
  const out = [];
  const walk = (parent) => {
    for (const child of parent.children) {
      if (child.type !== "dir") continue;
      out.push(child.path);
      walk(child);
    }
  };
  walk(node);
  return out;
}

// ── 上游 ────────────────────────────────────────────────────────────────────
//
// 上游那条技能的四种关系由**服务端**算（`local` 字段，判据是 `nous.json.upstream`
// 注脚里的 tree sha 跟镜像里那个 tree sha 比）。前端只摆，不重算 —— 两边各算一次
// 就会对不上，然后没人知道该信谁（I8）。

/** 排序：需要动手的在前（未取用 → 上游有更新 → 名字被占），已取用的沉底。 */
const UPSTREAM_RANK = { none: 0, changed: 1, taken: 2, same: 3 };

export const UPSTREAM_LABEL = {
  none: "未取用",
  changed: "上游有更新",
  taken: "名字被占",
  same: "已取用",
};

/** 上游清单的摘要：左栏那个数字说的是「有多少条值得你看一眼」。 */
export function upstreamSummary(entries) {
  const out = { none: 0, changed: 0, taken: 0, same: 0, todo: 0, total: 0 };
  for (const entry of Array.isArray(entries) ? entries : []) {
    const state = UPSTREAM_LABEL[entry?.local] ? entry.local : "none";
    out[state] += 1;
    out.total += 1;
  }
  // `same` 不算待办：取用过了、上游也没动，那一条现在只是「还在上游住着」。
  out.todo = out.none + out.changed + out.taken;
  return out;
}

export function sortUpstream(entries) {
  return [...(Array.isArray(entries) ? entries : [])].sort((a, b) => {
    const rank = (UPSTREAM_RANK[a?.local] ?? 0) - (UPSTREAM_RANK[b?.local] ?? 0);
    if (rank) return rank;
    return String(a?.path ?? "").localeCompare(String(b?.path ?? ""), "zh-CN");
  });
}

/** 取用时默认叫什么：名字被占就退一格，别让人一开始就撞墙。 */
export function suggestedAdoptName(entry) {
  const name = String(entry?.name ?? "").trim();
  if (!name) return "";
  return entry?.local === "taken" ? `${name}-upstream` : name;
}

/**
 * 从地址推一个订阅名：`https://github.com/mattpocock/skills.git` → `matt`。
 *
 * 为什么用**倒数第二段**而不是最后一段：仓库名谁都有（`skills`、`notes`），
 * 名字撞了要比对半天；`宿主/人/仓` 这个形状里，人是唯一的那一段。
 * 推不对就自己改 —— 这只是默认值。
 */
export function sourceNameFromUrl(url) {
  const raw = String(url ?? "").trim().replace(/[/:]+$/, "").replace(/\.git$/i, "");
  if (!raw) return "";
  const parts = raw.split(/[/:]/).map((part) => part.trim()).filter(Boolean);
  // `git@github.com:matt/skills` → ["git@github.com", "matt", "skills"]；`/srv/skills` → ["srv","skills"]
  // 本机路径的倒数第二段常常是 srv / opt / home 这种废话，那就退到仓名。
  const boring = new Set(["srv", "opt", "home", "var", "tmp", "root", "users", "mnt", "data", "git", "repos", "src"]);
  const parent = parts.length >= 2 ? parts[parts.length - 2] : "";
  const slug = parent && !boring.has(parent.toLowerCase()) ? parent : parts.at(-1) ?? "";
  return slug
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^[-._]+|[-._]+$/g, "")
    .slice(0, 32);
}

// ── 上游的树 ────────────────────────────────────────────────────────────────
//
// 上游不该是「一列条目」。别人的仓跟我们一样会长成多级目录：`plugins/x/skills/y`，
// 而且往下一层一层钻才是人看仓库的方式。所以把清单折成**跟本地同一副骨头**：
// 目录是组，带 SKILL.md 的那一层是叶子。
//
// 为什么在客户端算而不是服务端算：本地那棵树（`buildView`）也是这儿算的。
// 两处算法长在一起，改口径时不会只改一边 —— 服务端只管「git 里有什么」。

/**
 * 清单 → 树。返回根节点（`path: ""`），`children` 是一级。
 *
 * 每个节点：`{ name, path, kind: "dir"|"skill", children, n, todo, entry? }`
 * - `n`：这一层往下有多少条技能（目录卡片上那个数）
 * - `todo`：其中值得看一眼的（未取用 / 有更新 / 名字被占）
 *
 * 边角：一个目录自己就是技能、底下还套着子目录（`x/SKILL.md` + `x/y/SKILL.md`）。
 * 这种节点 `kind` 留 `"dir"`，单独的 `entry` 挂在它身上 —— 卡片上给「本层也是技能」。
 */
export function upstreamTree(entries) {
  const root = { name: "", path: "", kind: "dir", children: [] };
  // 一个路径只有一个节点 —— 技能和目录共用这张表，撞上就地升级成目录，
  // 否则「x 是技能、x/y 也是技能」会长出两个 x。
  const nodes = new Map([["", root]]);
  const parentPath = (full) => (full.includes("/") ? full.slice(0, full.lastIndexOf("/")) : "");
  const dir = (path) => {
    if (nodes.has(path)) {
      // 先来过一条同路径的技能，现在它底下又冒出一个子目录：就地升级成目录，
      // 它自己的那一条留在 `entry` 上（面板会写成「本层也是技能」）。
      const found = nodes.get(path);
      if (found.kind === "skill") found.kind = "dir";
      return found;
    }
    const parent = dir(parentPath(path));
    const node = { name: path.split("/").at(-1), path, kind: "dir", children: [] };
    parent.children.push(node);
    nodes.set(path, node);
    return node;
  };

  for (const entry of Array.isArray(entries) ? entries : []) {
    const full = String(entry?.path ?? entry?.name ?? "").replace(/^\/+|\/+$/g, "");
    if (!full) continue;
    const existing = nodes.get(full);
    if (existing) {
      // 已经在树上了（另一种形态）：把这条技能挂上去，别丢掉。
      if (existing.kind === "skill") existing.kind = "dir";
      existing.entry = entry;
      continue;
    }
    const node = { name: full.split("/").at(-1), path: full, kind: "skill", children: [], entry };
    dir(parentPath(full)).children.push(node);
    nodes.set(full, node);
  }

  const roll = (node) => {
    if (node.kind === "skill") {
      node.n = 1;
      node.todo = node.entry?.local === "same" ? 0 : 1;
      return node;
    }
    // 目录在前、技能在后，各自按名字。跟文件管理器一个顺序。
    node.children.sort(
      (a, b) => (a.kind === b.kind ? 0 : a.kind === "dir" ? -1 : 1) || a.name.localeCompare(b.name, "zh-CN"),
    );
    node.n = 0;
    node.todo = 0;
    for (const child of node.children) {
      roll(child);
      node.n += child.n;
      node.todo += child.todo;
    }
    if (node.entry) {
      node.n += 1;
      node.todo += node.entry.local === "same" ? 0 : 1;
    }
    return node;
  };
  roll(root);
  return root;
}

/** 树里按路径找节点；找不到给根（`path` 是脏的、上游变了，都不该炸成白屏）。 */
export function upstreamAt(tree, path) {
  const want = String(path ?? "").replace(/^\/+|\/+$/g, "");
  if (!tree || !want) return tree ?? null;
  // 路径是绝对的（`skills/a/b`），所以往下钻时把同一个 `want` 传下去。
  for (const child of tree.children ?? []) {
    if (child.path === want) return child;
    if (want.startsWith(`${child.path}/`)) return upstreamAt(child, want) ?? tree;
  }
  return tree;
}

/** 面包屑：`skills/a/b` → 可从根点回去的一串。 */
export function upstreamCrumb(path) {
  const parts = String(path ?? "").split("/").filter(Boolean);
  return parts.map((name, index) => ({ name, path: parts.slice(0, index + 1).join("/") }));
}

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
};

/** 网格里的先后：能用的先看，待发布的其次，坏了的再其次；归档根本不进网格。 */
const STATE_RANK = { ok: 0, unpublished: 1, failed: 2 };

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

/** 类别词表 = `categories.json` 的词表 ∪ 每条技能自己的 category，补上祖先，排好序。 */
export function categoryVocabulary(data) {
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
  return [...set].sort(compareCategories);
}

const compareCards = (a, b) =>
  (STATE_RANK[a.state] ?? 9) - (STATE_RANK[b.state] ?? 9) || a.name.localeCompare(b.name);

/**
 * `GET /v1/skills` 的响应 → 页面要的那一份。
 * `archived` 只给名字（恢复不需要别的），但也要进 total —— 它是仓库里的东西。
 */
export function buildView(data) {
  const skills = Array.isArray(data?.skills) ? data.skills : [];
  const vocabulary = categoryVocabulary(data);
  // 变体**不另开卡**：它在本体详情里是一行（`variants[本名]`），跟扩展面板同一个分组法。
  // 名册里它是一条独立条目（会话按 `本名@id` 存），但网格上再长一张卡就成了「两条技能」。
  const main = skills.filter((skill) => !skill.variant);
  const counts = facetCounts(main, Array.isArray(data?.categories) ? data.categories : []);
  const cards = main
    .map((skill) => ({ ...skill, state: cardState(skill) }))
    .sort(compareCards);
  const archived = (Array.isArray(data?.archived) ? data.archived : [])
    .map((name) => ({ name, state: "archived" }))
    .sort((a, b) => a.name.localeCompare(b.name));
  const uncategorized = main.filter((skill) => !categoriesOf(skill).length).length;
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
      [card.name, card.description, ...categoriesOf(card)].join(" ").toLowerCase().includes(needle),
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
// **一个订阅 = 一条「技能」**（整个仓）。所以这一层没有「清单折成树」这回事：
// 目录只活在抽屉里那棵用 `fileTree` 算出来的树上。
//
// 每条上游技能跟我们本地是什么关系（`local`）由**服务端**算（判据是
// `nous.json.upstream` 注脚里的 tree sha 跟镜像里那个 tree sha 比），前端只摆，
// 不重算 —— 两边各算一次就会对不上，然后没人知道该信谁（I8）。

export const UPSTREAM_LABEL = {
  none: "未取用",
  changed: "上游有更新",
  taken: "名字被占",
  same: "已取用",
};

/** 上游清单的摘要：卡上那个数字说的是「有多少条值得你看一眼」。 */
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

/** 取用时默认叫什么：名字被占就退一格，别让人一开始就撞墙。 */
export function suggestedAdoptName(entry) {
  const name = String(entry?.name ?? "").trim();
  if (!name) return "";
  return entry?.local === "taken" ? `${name}-upstream` : name;
}

/**
 * 订阅名 = **仓库名**。`https://github.com/mattpocock/skills.git` → `skills`。
 *
 * 早先这里取的是倒数第二段（宿主/人），结果订阅叫 `matt` —— 一个人的名字，
 * 不是那个仓。地址栏里 `#upstream/matt` 看着像谁的私聊，不像一个仓库。
 * 现在就跟仓走：**你订阅的是仓，名字就是仓的名字**。
 *
 * 撞名（两个仓都叫 `skills`）才需要人插手，那时自己加前缀（`mattpocock-skills`）。
 */
export function repoNameFromUrl(url) {
  const raw = String(url ?? "").trim().replace(/[/:]+$/, "").replace(/\.git$/i, "");
  if (!raw) return "";
  const last = raw.split(/[/:]/).map((part) => part.trim()).filter(Boolean).at(-1) ?? "";
  return last
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^[-._]+|[-._]+$/g, "")
    .slice(0, 32);
}

/**
 * 版本 = 短的 commit sha + 记下它的那天：`4ae2317 · 2026-11-06`。
 *
 * 行里那串 40 位 sha 谁也读不出来，人只需要「左边这 7 位是不是同一个」。
 * 时间只到天 —— 版本是按天说的，秒级精度在这儿只会挤掉别的字。
 */
export function versionText(version) {
  // 上游自己打的 tag 才是「版本号」：`v1.3.2`。
  const tag = String(version?.tag ?? "").trim();
  if (tag) return tag;
  // 提交在 tag 之后（上游天天提交、tag 一周一次）→ `v1.3.2+`，念作「v1.3.2 之后的版本」。
  const behind = String(version?.behind ?? "").trim();
  if (behind) return `${behind}+`;
  // 一个 tag 都没有的仓，才退回机器码 —— 那时候确实没有别的东西可显示。
  const commit = String(version?.commit ?? "").trim();
  if (!commit) return "";
  const short = commit.slice(0, 7);
  const at = String(version?.at ?? "").trim();
  if (!at) return short;
  const day = new Date(at);
  if (Number.isNaN(day.getTime())) return short;
  return `${short} · ${day.toISOString().slice(0, 10)}`;
}

/** 浮层里那行：`v1.3.2+ · 2026-11-06` —— 版本号 + 认下它的那天。 */
export function versionLine(version) {
  const text = versionText(version);
  if (!text) return "";
  const at = String(version?.at ?? "").trim();
  const day = at ? new Date(at) : null;
  if (!day || Number.isNaN(day.getTime())) return text;
  return `${text} · ${day.toISOString().slice(0, 10)}`;
}

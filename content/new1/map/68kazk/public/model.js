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
  const counts = facetCounts(skills, Array.isArray(data?.categories) ? data.categories : []);
  const cards = skills
    .map((skill) => ({ ...skill, state: cardState(skill) }))
    .sort(compareCards);
  const archived = (Array.isArray(data?.archived) ? data.archived : [])
    .map((name) => ({ name, state: "archived" }))
    .sort((a, b) => a.name.localeCompare(b.name));
  const uncategorized = skills.filter((skill) => !categoriesOf(skill).length).length;
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

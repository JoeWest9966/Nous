// model.js —— 这一页的**全部判断**，一个 DOM 都不碰，所以能裸跑 `node --test`。
//
// app.js 只负责「把这里算出来的东西画出来」。凡是「这条算什么状态」「chip 上写什么」
// 「左栏那棵树怎么摆」的规矩，都在这个文件里，一处。
//
// 三条跟 3699b9 一脉相承的纪律：
//
//  1. **形状未定就不假装。** `ultimate.json` 的键表还在提案阶段，所以这一版没有编辑器、
//     没有新建、没有归档 —— 只读。写的那一半等形状定下来再长，别先长出来再改。
//  2. **红卡分两种。** `failed` = 这条自己写坏了（修这里）；`invalid` = 它引用的东西没了
//     （修那边）。糊成一种，人就不知道该去哪儿动手。
//  3. **「没选」有三种长相。** 一条 ultimate 里 `prompt` 缺席 = 跟随默认、`null` = 明确不要、
//     字符串 = 用它。chip 上写「默认」和「无」两句话，不是都写「—」。

/** 提案中的键表。改这里 = 改契约，`sections.json` 的 checker 得跟着一起改。 */
export const ULTIMATE_KEYS = ["v", "name", "summary", "category", "description", "presets", "prompt", "readOnly"];

const NAME_RE = /^[a-z0-9][a-z0-9._-]*$/;

/** 名字：目录名 = `name` = 会话里的键（`Ultimate-Template/<名>`）。 */
export const nameLooksValid = (value) => {
  const name = String(value ?? "").trim();
  return name.length > 0 && name.length <= 64 && NAME_RE.test(name);
};

// ── 引用 ────────────────────────────────────────────────────────────────────

/** `Session-Template/deep-dive` → `{ section: "Session-Template", name: "deep-dive" }`。 */
export const splitRef = (ref) => {
  const text = String(ref ?? "").trim();
  const at = text.indexOf("/");
  if (at <= 0 || at === text.length - 1) return { section: null, name: text };
  return { section: text.slice(0, at), name: text.slice(at + 1) };
};

/**
 * 一条引用现在算不算数。`known` 是「那一段里现有哪些名字」——真数据从 qrd3wh 的
 * `resolved` 来；**这一页不自己判断引用存在性**，只翻译结果。
 */
export const refState = (ref, known) => {
  if (!known) return "unknown";
  return known.has(String(ref ?? "")) ? "ok" : "missing";
};

/** 一条 ultimate 引用的全部东西（顺序有意义：叠放顺序就是执行顺序）。 */
export const refsOf = (ultimate, known = {}) => {
  const presets = Array.isArray(ultimate?.presets) ? ultimate.presets : [];
  const refs = presets.map((ref) => ({
    kind: "preset",
    ref: String(ref),
    state: refState(ref, known.presets),
  }));
  const prompt = ultimate?.prompt;
  if (prompt !== undefined) {
    refs.push({
      kind: "prompt",
      ref: prompt === null ? null : String(prompt),
      state: prompt === null ? "none" : refState(prompt, known.prompts),
    });
  }
  return refs;
};

// ── 卡状态 ──────────────────────────────────────────────────────────────────

/**
 * `failed > invalid > unpublished > ok`。
 *
 * 顺序不是随手排的：写坏了的这条**连解析都做不了**，报引用失效是骗人（引用可能都是好的）；
 * 而未发布是「盘上有、HEAD 没有」，那是最轻的 —— 它跑得起来，只是别人 pull 不到。
 */
export const cardState = ({ specError = null, refs = [], tracked = true } = {}) => {
  if (specError) return "failed";
  if (refs.some((ref) => ref.state === "missing")) return "invalid";
  if (!tracked) return "unpublished";
  return "ok";
};

/** 能不能「启动」。这一段现在**一律不能** —— 展开还没接（形状未定），先老实说。 */
export const canLaunch = () => false;

/** 启动按钮为什么是灰的。一句话，页面直接显示。 */
export const launchBlockedReason = () =>
  "这一段还没接展开：ultimate.json 的形状定下来、引擎学会把它展开成一次会话之后，这个按钮才亮";

// ── chip ────────────────────────────────────────────────────────────────────

/**
 * 卡片上那一行引用 chip。三条：
 *   预设 —— 缺席（一个不叠）/ `[]`（明确不叠）/ `N 条`
 *   提示词 —— 缺席 = 跟随仓库默认（灰）/ `null` = 明确不要（正常色）/ 名字 / 红
 *   只读 —— 只在 true 时出现（false 是常态，不用占一行）
 */
export const chipRows = (ultimate, known = {}) => {
  const refs = refsOf(ultimate, known);
  const chips = [];
  const presetRefs = refs.filter((ref) => ref.kind === "preset");
  const missingPresets = presetRefs.filter((ref) => ref.state === "missing");

  if (!Array.isArray(ultimate?.presets)) {
    chips.push({ kind: "presets", state: "default", text: "预设 默认", title: "这一条没有 presets 键" });
  } else if (presetRefs.length === 0) {
    chips.push({ kind: "presets", state: "none", text: "预设 无", title: "一条预设都不叠" });
  } else if (missingPresets.length) {
    chips.push({
      kind: "presets",
      state: "bad",
      text: `预设 ${presetRefs.length} 条 · ${missingPresets.length} 条不在`,
      title: missingPresets.map((ref) => `缺：${ref.ref}`).join("\n"),
    });
  } else {
    chips.push({
      kind: "presets",
      state: "ok",
      text: `预设 ${presetRefs.length} 条`,
      title: presetRefs.map((ref, index) => `${index + 1}. ${ref.ref}`).join("\n"),
    });
  }

  const promptChip = refs.find((ref) => ref.kind === "prompt");
  if (!promptChip) {
    chips.push({ kind: "prompt", state: "default", text: "提示词 默认", title: "没写 prompt：跟随仓库默认模板" });
  } else if (promptChip.state === "none") {
    chips.push({ kind: "prompt", state: "none", text: "提示词 无", title: "prompt: null —— 明确不要模板" });
  } else if (promptChip.state === "missing") {
    chips.push({ kind: "prompt", state: "bad", text: "提示词 不在", title: `Nous-prompts 里没有：${promptChip.ref}` });
  } else {
    // 三个分支都得带「提示词」这三个字：只写一个裸名字，人得先猜这是模板还是预设。
    chips.push({ kind: "prompt", state: "ok", text: `提示词 ${promptChip.ref}`, title: `提示词模板：${promptChip.ref}` });
  }

  if (ultimate?.readOnly === true) {
    chips.push({ kind: "readOnly", state: "ok", text: "只读", title: "写工具被摘掉了，不只是提示词里说一句" });
  }

  return chips;
};

// ── 类别 facet ──────────────────────────────────────────────────────────────

export const ancestorsOf = (category) =>
  String(category ?? "")
    .split("/")
    .filter(Boolean)
    .map((_, index, all) => all.slice(0, index + 1).join("/"));

/**
 * 路径排序：父在子前、同级按段比。**跟 `repo-tools.js` 的 `compareCategories` 同一套**
 * （`localeCompare(…, "zh-CN")`）—— 不然左栏的次序会跟引擎给的次序不一样，同一个类别
 * 在两页上排不同的位置。
 */
export const sortCategories = (list) =>
  [...new Set(list.filter(Boolean))].sort((a, b) => {
    const left = a.split("/");
    const right = b.split("/");
    for (let i = 0; i < Math.min(left.length, right.length); i += 1) {
      const cmp = left[i].localeCompare(right[i], "zh-CN");
      if (cmp) return cmp;
    }
    return left.length - right.length;
  });

export const isUnder = (category, root) => category === root || category.startsWith(`${root}/`);

/**
 * 左栏那棵树。**计数含后代**，一条 ultimate 挂 `工程/审查` 时 `工程` 也算一条。
 * 零计数的类别留着但灰掉（词表里有它、这会儿没人用）。
 */
export const facetTree = (categories, rows) => {
  const counts = new Map();
  for (const row of rows) {
    for (const category of row.category ?? []) {
      for (const step of ancestorsOf(category)) counts.set(step, (counts.get(step) ?? 0) + 1);
    }
  }
  const names = sortCategories([...categories, ...counts.keys()]);
  return names.map((name) => ({
    name,
    depth: name.split("/").length - 1,
    label: name.split("/").pop(),
    count: counts.get(name) ?? 0,
  }));
};

// ── 筛选 ────────────────────────────────────────────────────────────────────

/** 非零才出现；图例本身就是筛选，不另写说明。`all` 永远在（它不是「有 N 条」那一类）。 */
export const VIEWS = [
  { id: "all", label: "全部" },
  { id: "uncategorized", label: "未分类" },
  { id: "invalid", label: "引用失效", tone: "bad" },
  { id: "failed", label: "写坏了", tone: "bad" },
  { id: "unpublished", label: "未发布", tone: "warn" },
  { id: "archived", label: "已归档", tone: "gone" },
];

export const viewCounts = (rows, { archived = 0 } = {}) => {
  const counts = { all: rows.length, archived };
  counts.uncategorized = rows.filter((row) => !(row.category ?? []).length).length;
  counts.invalid = rows.filter((row) => row.state === "invalid").length;
  counts.failed = rows.filter((row) => row.state === "failed").length;
  counts.unpublished = rows.filter((row) => row.state === "unpublished").length;
  return counts;
};

export const visibleViews = (counts) =>
  VIEWS.filter((view) => view.id === "all" || view.id === "archived" || (counts[view.id] ?? 0) > 0);

/**
 * 搜 + 筛。`category` 为 null = 不过滤；`view` 为 `all` 时**不藏坏账** ——
 * 藏着的坏账等于没有账。
 */
export const filterPresets = (rows, { view = "all", category = null, query = "" } = {}) => {
  const needle = String(query ?? "").trim().toLowerCase();
  return rows.filter((row) => {
    if (view === "uncategorized" && (row.category ?? []).length) return false;
    if (view === "invalid" && row.state !== "invalid") return false;
    if (view === "failed" && row.state !== "failed") return false;
    if (view === "unpublished" && row.state !== "unpublished") return false;
    if (view === "archived") return false; // 归档是另一份数据，不进这张列表
    if (category && !(row.category ?? []).some((entry) => isUnder(entry, category))) return false;
    if (!needle) return true;
    const haystack = [row.name, row.summary, ...(row.category ?? [])].join(" ").toLowerCase();
    return haystack.includes(needle);
  });
};

/** 空的时候说清「是这一档空了」还是「整段空了」—— 两句话，别合成一句。 */
export const emptyHint = ({ total, view, category, query }) => {
  if (!total) {
    return {
      title: "这一段还是空的",
      body: "Ultimate-Template/ 里还没有一条。ultimate.json 的形状定下来之后，第一条从这里开始。",
    };
  }
  const why = query ? `搜「${query}」` : category && view !== "all" ? `${category} · ${view}` : category || "这一档";
  return { title: `${why}里没有`, body: "换个筛选，或者点「全部」看这一段的全部。" };
};

/** 详情里「叠放顺序」那一节：每条引用带上它现在的状态，缺的直接点名。 */
export const stackOf = (ultimate, known = {}) =>
  refsOf(ultimate, known)
    .filter((ref) => ref.kind === "preset")
    .map((ref, index) => ({ ...ref, order: index + 1, ...splitRef(ref.ref) }));

/** 字母砖的色相：名字 hash，同一条永远同一个颜色。 */
export const tileStyle = (name) => {
  let hash = 0;
  for (const ch of String(name ?? "")) hash = (hash * 31 + ch.codePointAt(0)) % 360;
  return `background:hsl(${hash} 42% 34%)`;
};

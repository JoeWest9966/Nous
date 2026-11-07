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

/**
 * 键表。**已经定下来了**（跟 `Nous-Template/index.mjs` 和 `qrd3wh/presets.js` 同一份契约）：
 * 一条 = 触发器 + 触发器的参数 + 一条会话模板。
 */
export const ULTIMATE_KEYS = [
  "v",
  "name",
  "summary",
  "category",
  "description",
  "trigger",
  "params",
  "session",
  "enabled",
];

/**
 * 参数类型 → 页面画什么控件。**这是「类型 → 控件」的映射，不是「触发器 → 表单」的映射。**
 *
 * 关键的一条：**这里没有触发器的名字。** 页面上永远不写 `if (名字 === "single-shot")` ——
 * 那样每加一个触发器就要改一次页面，而且两份代码迟早对不上。加类型是「说清楚了才加」的决定，
 * 给某个触发器开后门不是。
 */
export const FIELD_OF = {
  epoch: { control: "datetime", label: "日期时间" },
  number: { control: "number", label: "数字" },
  string: { control: "text", label: "文本" },
  text: { control: "textarea", label: "长文本" },
  boolean: { control: "checkbox", label: "开关" },
  select: { control: "select", label: "下拉" },
};

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

/**
 * 一条 ultimate 引用的两样东西：触发器（什么时候起）、会话模板（起什么）。
 *
 * 状态由**引擎**给（`resolved`）：这一页不自己判断引用在不在 —— 它一判断就有两份规则。
 * 这里只把引擎的结果翻译成能画的东西。
 */
export const refsOf = (ultimate, known = {}) => {
  const refs = [];
  const trigger = ultimate?.trigger;
  if (trigger !== undefined && trigger !== null) {
    refs.push({
      kind: "trigger",
      ref: String(trigger),
      state: known.triggers ? (known.triggers.has(String(trigger)) ? "ok" : "missing") : "unknown",
    });
  }
  const session = ultimate?.session;
  if (session !== undefined && session !== null) {
    refs.push({
      kind: "session",
      ref: String(session),
      state: known.sessions ? (known.sessions.has(String(session)) ? "ok" : "missing") : "unknown",
    });
  }
  return refs;
};

/** 参数按触发器的声明排一遍（没声明的排最后）。页面上照这个顺序画。 */
export const paramsOf = (ultimate, schema = []) => {
  const given = ultimate?.params ?? {};
  const known = new Set(Object.keys(given));
  const rows = schema.map((param) => ({
    name: param.name,
    label: param.label ?? param.name,
    type: param.type,
    required: param.required === true,
    help: param.help ?? "",
    options: Array.isArray(param.options) ? param.options : [],
    value: given[param.name],
    state: given[param.name] === undefined ? (param.required ? "missing" : "default") : "ok",
  }));
  for (const key of known) {
    if (schema.some((param) => param.name === key)) continue;
    // 触发器改了参数名，这条还留着旧的 —— 单独一行标出来，别混在正常的参数里。
    rows.push({
      name: key,
      label: key,
      type: "string",
      required: false,
      help: "触发器里没有这个参数了",
      options: [],
      value: given[key],
      state: "unknown",
    });
  }
  return rows;
};

/**
 * 表单模型：照触发器的参数声明，列出这一页要画哪些字段。
 *
 * **页面照着它画，不照着触发器的名字画。** 新加一个触发器 = 它自己声明参数，
 * 页面一个字节都不用改。
 */
export const formFields = (trigger) => {
  const schema = Array.isArray(trigger?.params) ? trigger.params : [];
  return schema.map((param) => {
    const field = FIELD_OF[param.type] ?? FIELD_OF.string;
    return {
      name: param.name,
      label: param.label ?? param.name,
      type: param.type,
      control: field.control,
      typeLabel: field.label,
      required: param.required === true,
      help: param.help ?? "",
      options: Array.isArray(param.options) ? param.options : [],
      default: param.default,
    };
  });
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
  const params = ultimate?.params ?? {};

  const triggerChip = refs.find((ref) => ref.kind === "trigger");
  if (!triggerChip) {
    chips.push({ kind: "trigger", state: "default", text: "触发器 未选", title: "这一条还没挂触发器" });
  } else if (triggerChip.state === "missing") {
    chips.push({
      kind: "trigger",
      state: "bad",
      text: `触发器 不在`,
      title: `Nous-Trigger 里没有：${triggerChip.ref}`,
    });
  } else {
    chips.push({
      kind: "trigger",
      state: "ok",
      text: `触发器 ${triggerChip.ref}`,
      title: `什么时候起：${triggerChip.ref}`,
    });
  }

  const count = Object.keys(params).length;
  chips.push({
    kind: "params",
    state: count ? "ok" : "default",
    text: count ? `参数 ${count} 个` : "参数 无",
    title: count
      ? Object.entries(params)
          .map(([key, value]) => `${key} = ${String(value)}`)
          .join("\n")
      : "没填参数",
  });

  const sessionChip = refs.find((ref) => ref.kind === "session");
  if (!sessionChip) {
    chips.push({ kind: "session", state: "default", text: "会话 未选", title: "这一条还没钉会话模板" });
  } else if (sessionChip.state === "missing") {
    chips.push({
      kind: "session",
      state: "bad",
      text: "会话 不在",
      title: `Session-Template 里没有：${sessionChip.ref}`,
    });
  } else {
    chips.push({
      kind: "session",
      state: "ok",
      text: `会话 ${sessionChip.ref}`,
      title: `起什么对话：${sessionChip.ref}`,
    });
  }

  if (ultimate?.enabled === false) {
    chips.push({ kind: "enabled", state: "none", text: "已停", title: "enabled: false —— 这条现在不触发" });
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

/** 详情里「引用」那一节：触发器 + 会话模板，各自的状态，缺的直接点名。 */
export const stackOf = (ultimate, known = {}) =>
  refsOf(ultimate, known).map((ref, index) => ({ ...ref, order: index + 1, ...splitRef(ref.ref) }));

// ── 新建 / 编辑：表单的判断 ─────────────────────────────────────────────────

/**
 * 一条终极模板现在**能不能存**。返回问题清单，空 = 能存。
 *
 * 这里只做**页面能自己判断的事**：必填有没有填、名字合不合格、类型对不对。
 * 引用在不在（触发器还在吗、会话模板还在吗）**归引擎** —— 页面说不准，
 * 硬说就成了第二份规则。存下去之后引擎会再判一遍，那时才是准的。
 */
export const draftProblems = (draft, { triggers = new Map(), sessions = new Set() } = {}) => {
  const problems = [];
  const name = String(draft?.name ?? "").trim();
  if (!name) problems.push("缺名字");
  else if (!nameLooksValid(name)) problems.push("名字不合格：小写字母 / 数字开头，只留 . _ -");
  if (!String(draft?.summary ?? "").trim()) problems.push("缺一句话说明");

  const trigger = String(draft?.trigger ?? "").trim();
  if (!trigger) problems.push("还没选触发器");
  else if (triggers.size && !triggers.has(trigger)) problems.push(`触发器「${trigger}」不在 Nous-Trigger 里`);

  const session = String(draft?.session ?? "").trim();
  if (!session) problems.push("还没选会话模板");
  else if (sessions.size && !sessions.has(session)) problems.push(`会话模板「${session}」不在 Session-Template 里`);

  // 参数：照那个触发器自己的声明逐个看。
  const schema = triggers.get(trigger)?.params ?? [];
  const given = draft?.params ?? {};
  for (const param of schema) {
    const value = given[param.name];
    if (value === undefined || value === "") {
      if (param.required) problems.push(`触发器要参数「${param.name}」，还没填`);
      continue;
    }
    const problem = valueProblem(param, value);
    if (problem) problems.push(`参数「${param.name}」${problem}`);
  }
  return problems;
};

/** 一个参数值对这个声明合不合格。返回一句话（空 = 合格）。跟引擎那份同一套规矩。 */
export const valueProblem = (param, value) => {
  switch (param.type) {
    case "epoch":
      if (typeof value !== "number" || !Number.isInteger(value) || value < 0) return "得是 UTC 秒（正整数）";
      return "";
    case "number":
      if (typeof value !== "number" || !Number.isFinite(value)) return "得是数字";
      return "";
    case "boolean":
      return typeof value === "boolean" ? "" : "得是 true / false";
    case "select":
      if (typeof value !== "string") return "得是字符串";
      if (Array.isArray(param.options) && !param.options.includes(value)) return `只能是 ${param.options.join(" / ")}`;
      return "";
    case "text":
    case "string":
    default:
      return typeof value === "string" ? "" : "得是字符串";
  }
};

/** UTC 秒 → 本地时间（`datetime-local` 输入框要的格式：`YYYY-MM-DDTHH:mm`）。 */
export const epochToLocalInput = (epoch) => {
  if (typeof epoch !== "number" || !Number.isFinite(epoch)) return "";
  const date = new Date(epoch * 1000);
  const pad = (n) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
};

/** `datetime-local` 的值 → UTC 秒。**按本地时区解释** —— 人挑的是本地时间。 */
export const localInputToEpoch = (value) => {
  const text = String(value ?? "").trim();
  if (!text) return null;
  const parsed = new Date(text);
  const time = parsed.getTime();
  if (!Number.isFinite(time)) return null;
  return Math.floor(time / 1000);
};

/** 字母砖的色相：名字 hash，同一条永远同一个颜色。 */
export const tileStyle = (name) => {
  let hash = 0;
  for (const ch of String(name ?? "")) hash = (hash * 31 + ch.codePointAt(0)) % 360;
  return `background:hsl(${hash} 42% 34%)`;
};

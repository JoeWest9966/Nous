// model.js —— 3699b9 的纯计算层：卡片状态、facet 树、筛选、chip 文案、会话默认名。
//
// 这个文件里没有 DOM、没有 fetch、没有 localStorage —— 所以能裸跑 `node --test`。
// 所有「这条预设现在算什么状态」的判断都在这里，而且**只有一份**：
// 服务端给的 `resolved` 是它的输入，不是它的替代品。
//
// 一条规矩贯穿全篇：**面板上写的 = 实际跑的。** chip 上的模型 / 档位 / 模板一律读
// `resolved`（服务端算的），不读 `spec`（我们发上去的那份）。这条破了，这一页就没意义了。

/** 类别路径的分隔符。父子关系不存盘，按 `/` 现拆。 */
export const CATEGORY_SEP = "/";

/** 卡片状态的优先级：坏的（自己写坏了）> 引用失效 > 未发布 > 正常。 */
export const CARD_STATES = ["failed", "invalid", "unpublished", "ok"];

/**
 * 这张卡现在是什么状态。
 *
 * `failed` 和 `invalid` 必须分开，而且卡上要写清是哪一种：
 *   failed    = 这条自己写坏了（JSON / 形状），去修**这里**；
 *   invalid   = 引用的东西没了（技能被卸、模板被删），去修**那边**。
 * 混成一格，人就分不清该动哪个文件。
 */
export function cardState(preset) {
  if (!preset) return "failed";
  if (preset.specError) return "failed";
  const resolved = preset.resolved;
  if (!resolved) return "failed";
  const bad =
    resolved.model?.state === "invalid" ||
    resolved.thinking?.state === "invalid" ||
    resolved.template?.state === "invalid" ||
    (resolved.skills ?? []).some((item) => item.state === "invalid") ||
    (resolved.extensions ?? []).some((item) => item.state === "invalid");
  if (bad) return "invalid";
  if (preset.tracked === false) return "unpublished";
  return "ok";
}

/** 能不能直接开对话。`invalid` / `failed` 都挡 —— 开了也是个跑不对的会话。 */
export const canLaunch = (preset) => cardState(preset) === "ok" || cardState(preset) === "unpublished";

// ── 类别树 ──────────────────────────────────────────────────────────────────

const segmentsOf = (value) =>
  String(value ?? "")
    .split(CATEGORY_SEP)
    .map((segment) => segment.trim())
    .filter(Boolean);

export const lastSegment = (value) => segmentsOf(value).at(-1) ?? "";

export const depthOf = (value) => segmentsOf(value).length;

/** `a` 是不是 `root` 或它的后代。筛选「父级筛出整枝」靠它。 */
export const isUnder = (value, root) =>
  value === root || String(value ?? "").startsWith(String(root ?? "") + CATEGORY_SEP);

/**
 * 扁平类别表 → 缩进树（带计数）。
 *
 * 计数含后代、按预设去重：一条预设同时挂了 `工程` 和 `工程/审查`，`工程` 只算它一次。
 * 零计数的类别保留（词表里有、还没挂东西）—— 建了看不见比灰着更糟。
 *
 * @param {string[]} categories 词表 ∪ 所有预设挂着的
 * @param {Array<{name:string, category:string[]}>} presets
 */
export function facetTree(categories, presets) {
  const flat = new Set();
  for (const category of categories ?? []) for (const segment of ancestorsOf(category)) flat.add(segment);

  const counts = new Map();
  for (const preset of presets ?? []) {
    const seen = new Set();
    for (const category of preset.category ?? []) {
      for (const step of ancestorsOf(category)) {
        if (seen.has(step)) continue;
        seen.add(step);
        counts.set(step, (counts.get(step) ?? 0) + 1);
      }
    }
  }

  return [...flat]
    .sort(compareCategories)
    .map((path) => ({
      path,
      label: lastSegment(path),
      depth: depthOf(path) - 1,
      count: counts.get(path) ?? 0,
      children: [...flat].filter((other) => other !== path && other.startsWith(path + CATEGORY_SEP)),
    }));
}

/** 一条类别路径的祖先链：`写作/周报` → [`写作`, `写作/周报`]。 */
export function ancestorsOf(category) {
  const segments = segmentsOf(category);
  return segments.map((_, index) => segments.slice(0, index + 1).join(CATEGORY_SEP));
}

/** 排序：先按段数，再按字典序 —— 同一支的子级永远贴在父级下面。 */
const compareCategories = (a, b) => {
  const left = segmentsOf(a);
  const right = segmentsOf(b);
  const limit = Math.min(left.length, right.length);
  for (let index = 0; index < limit; index += 1) {
    if (left[index] === right[index]) continue;
    return left[index].localeCompare(right[index], "zh-CN");
  }
  return left.length - right.length;
};

export const sortCategories = (list) => [...new Set((list ?? []).filter(Boolean))].sort(compareCategories);

// ── 筛选 ────────────────────────────────────────────────────────────────────

/** 顶栏那组「系统视图」。右栏的类别是同一条轴上的另一格，一击只有一个亮。 */
export const VIEWS = [
  { key: "all", label: "全部" },
  { key: "uncategorized", label: "未分类" },
  { key: "unpublished", label: "未发布" },
  { key: "invalid", label: "引用失效" },
  { key: "failed", label: "写坏了" },
  { key: "archived", label: "已归档" },
];

/** 一个视图现在有多少条。零计数的视图**不出现** —— 图例就是筛选，空的是噪音。 */
export function viewCounts(presets, archived = []) {
  const counts = { all: presets.length };
  counts.uncategorized = presets.filter((preset) => (preset.category ?? []).length === 0).length;
  counts.unpublished = presets.filter((preset) => cardState(preset) === "unpublished").length;
  counts.invalid = presets.filter((preset) => cardState(preset) === "invalid").length;
  counts.failed = presets.filter((preset) => cardState(preset) === "failed").length;
  counts.archived = archived.length;
  return counts;
}

/** 只留「真该出现」的视图：全部永远在，别的零计数就不出现。 */
export function visibleViews(counts) {
  return VIEWS.filter((view) => view.key === "all" || (counts[view.key] ?? 0) > 0);
}

const haystack = (preset) =>
  [preset.name, preset.summary, preset.description, ...(preset.category ?? [])].join(" ").toLowerCase();

/**
 * 筛出这一屏该画哪些卡。
 *
 * `category` 和 `view` 是**同一条轴**（「给我看哪些」），一击只有一个亮：
 * 点右栏类别 → view 落回 null；点顶栏 → category 落回 null。两套叠着亮，
 * 等于让人在脑子里做交集。
 *
 * @param {object[]} presets
 * @param {{view?: string|null, category?: string|null, query?: string}} options
 */
export function filterPresets(presets, { view = null, category = null, query = "" } = {}) {
  const needle = String(query ?? "").trim().toLowerCase();
  let out = presets ?? [];
  if (category) out = out.filter((preset) => (preset.category ?? []).some((item) => isUnder(item, category)));
  else if (view && view !== "all") {
    if (view === "uncategorized") out = out.filter((preset) => (preset.category ?? []).length === 0);
    else out = out.filter((preset) => cardState(preset) === view);
  } else if (view === "all" || view === null) {
    // 「全部」不排除任何东西：写坏的和引用失效的也得能看见 —— 藏着的坏账等于没有账。
  }
  if (needle) out = out.filter((preset) => haystack(preset).includes(needle));
  return out;
}

/**
 * 空状态按「哪一层挡住的」给出口。
 * 在错的类别里搜一个存在的预设，读起来就跟它不存在一样 —— 所以要说清是哪一层的锅。
 */
export function emptyHint({ view, category, query, presets, archived }) {
  if (archived?.length) return { text: "归档区里这一屏是空的", action: { kind: "back", label: "回列表" } };
  if (query && (presets ?? []).length) {
    return { text: `没有匹配「${query}」的预设`, action: { kind: "clear-search", label: "清空搜索" } };
  }
  if (category) {
    const elsewhere = (presets ?? []).filter((preset) => (preset.category ?? []).some((item) => isUnder(item, category)));
    return {
      text: elsewhere.length ? `「${category}」下没有预设` : `「${category}」下没有预设`,
      action: { kind: "clear-category", label: "全部类别" },
    };
  }
  if (view && view !== "all") return { text: "这一档里没有预设", action: { kind: "view-all", label: "看全部" } };
  return { text: "私仓里还没有预设", action: null };
}

// ── 卡片上的引用 chip ───────────────────────────────────────────────────────

/**
 * 一张卡上那排 chip。全部读 `resolved`（服务端算的），**一个都不自己推**。
 *
 * 三态在这里必须有三种不同的长相：
 *   `None`  = template: null（明确不要）—— 正常字色，写「无」
 *   `默认`  = 键缺席 —— 灰字，写「默认」
 *   `invalid` —— 红字 + 引用原文
 *
 * @returns {Array<{key:string, label:string, text:string, state:"ok"|"auto"|"invalid"|"none"|"default"}>}
 */
export function chipRows(preset) {
  const resolved = preset?.resolved;
  if (!resolved) return [];

  const rows = [];

  const model = resolved.model ?? { state: "auto", value: null };
  rows.push(
    model.state === "invalid"
      ? { key: "model", label: "模型", text: model.ref ?? "?", state: "invalid" }
      : model.state === "auto"
        ? { key: "model", label: "模型", text: model.value ? `自动 · ${model.value}` : "自动", state: "auto" }
        : { key: "model", label: "模型", text: model.value ?? "?", state: "ok" },
  );

  const thinking = resolved.thinking ?? {};
  if (thinking.ref) {
    rows.push(
      thinking.state === "invalid"
        ? { key: "thinking", label: "思考", text: thinking.ref, state: "invalid" }
        : { key: "thinking", label: "思考", text: thinking.ref, state: "ok" },
    );
  } else if (!resolved.absence?.thinkingLevel) {
    rows.push({ key: "thinking", label: "思考", text: "off", state: "ok" });
  }

  const skills = resolved.skills ?? [];
  const badSkills = skills.filter((item) => item.state === "invalid");
  if (badSkills.length) {
    rows.push({ key: "skills", label: "技能", text: badSkills.map((item) => item.ref).join("、"), state: "invalid" });
  } else if (resolved.absence?.skills) {
    rows.push({ key: "skills", label: "技能", text: "默认", state: "default" });
  } else if (!skills.length) {
    rows.push({ key: "skills", label: "技能", text: "都不要", state: "none" });
  } else {
    rows.push({ key: "skills", label: "技能", text: `${skills.length} 条`, state: "ok" });
  }

  const extensions = resolved.extensions ?? [];
  const badExtensions = extensions.filter((item) => item.state === "invalid");
  if (badExtensions.length) {
    rows.push({
      key: "extensions",
      label: "扩展",
      text: badExtensions.map((item) => item.ref).join("、"),
      state: "invalid",
    });
  } else if (resolved.absence?.extensions) {
    rows.push({ key: "extensions", label: "扩展", text: "默认", state: "default" });
  } else if (!extensions.length) {
    rows.push({ key: "extensions", label: "扩展", text: "都不要", state: "none" });
  } else {
    rows.push({ key: "extensions", label: "扩展", text: `${extensions.length} 个`, state: "ok" });
  }

  const template = resolved.template ?? { mode: "default" };
  if (template.mode === "invalid" || template.state === "invalid") {
    rows.push({ key: "template", label: "模板", text: template.ref ?? "?", state: "invalid" });
  } else if (template.mode === "default") {
    rows.push({ key: "template", label: "模板", text: "默认", state: "default" });
  } else if (template.mode === "none") {
    rows.push({ key: "template", label: "模板", text: "无", state: "none" });
  } else {
    rows.push({ key: "template", label: "模板", text: template.value ?? template.ref, state: "ok" });
  }

  const cwd = resolved.cwd ?? { mode: "default" };
  if (cwd.mode === "picked") {
    rows.push({
      key: "cwd",
      label: "路径",
      text: cwd.value === "/" ? "/" : (lastSegment(cwd.value ?? "") || cwd.value),
      state: "ok",
    });
  } else if (cwd.mode === "none") {
    rows.push({ key: "cwd", label: "路径", text: "无", state: "none" });
  } else {
    rows.push({ key: "cwd", label: "路径", text: "默认", state: "default" });
  }

  if (resolved.readOnly) rows.push({ key: "readOnly", label: "只读", text: "是", state: "none" });

  return rows;
}

/** 启动确认框里那几行「会跳过什么」。有才出，空的就别摆一行假话。 */
export function launchWarnings(preset) {
  const resolved = preset?.resolved ?? {};
  const missingSkills = (resolved.skills ?? []).filter((item) => item.state === "invalid").map((item) => item.ref);
  const missingExtensions = (resolved.extensions ?? [])
    .filter((item) => item.state === "invalid")
    .map((item) => item.ref);
  const missingTemplate = resolved.template?.state === "invalid" ? resolved.template.ref : null;
  const notes = [];
  if (resolved.model?.state === "auto") notes.push("模型跟随设备默认");
  if (resolved.template?.mode === "default") notes.push("模板跟随仓库默认");
  if (resolved.cwd?.mode === "default") notes.push("工作目录用块缺省");
  if (resolved.absence?.thinkingLevel && resolved.model?.state === "ok") notes.push("档位用引擎缺省 off");
  return { missingSkills, missingExtensions, missingTemplate, notes };
}

/** 新建时那个默认会话名：`<预设名> 11-06 14:32`。可以改，服务端还会再截一次。 */
export function defaultSessionName(presetName, now = new Date()) {
  const pad = (value) => String(value).padStart(2, "0");
  const stamp = `${pad(now.getMonth() + 1)}-${pad(now.getDate())} ${pad(now.getHours())}:${pad(now.getMinutes())}`;
  return `${presetName} ${stamp}`.slice(0, 120);
}

// ── 类别菜单的影响范围 ──────────────────────────────────────────────────────

/** 删 / 改一个类别之前，先说清它带着多少东西走。 */
export function categoryImpact(presets, path) {
  const affected = (presets ?? []).filter((preset) => (preset.category ?? []).some((item) => isUnder(item, path)));
  const children = new Set();
  for (const preset of presets ?? []) {
    for (const category of preset.category ?? []) {
      for (const step of ancestorsOf(category)) {
        if (step !== path && isUnder(step, path)) children.add(step);
      }
    }
  }
  return { presets: affected.length, children: children.size };
}

/** 名字就是 API：面板只做**提示**，判定在服务端。这里给的是「能不能点提交」的乐观值。 */
export function nameLooksValid(name) {
  return /^[a-z0-9][a-z0-9._-]*$/.test(String(name ?? "")) && String(name).length <= 64;
}

// ── 表单草稿 ↔ preset.json ──────────────────────────────────────────────────
//
// 表单是**三态的显示层**：`default` / `none` / `picked` 必须各自有对应的控件状态。
// 所以这一对函数只做「摊开」和「复原」，不猜默认值：
//
//   draftFromText("...") → 表单能显示的那份（坏 JSON 给 `broken: true`）
//   specFromDraft(draft) → 写回磁盘的那份（缺席 / null / 有值，一个都不许塌）

/** preset.json 的原文 → 表单草稿。 */
export function draftFromText(text) {
  let spec;
  try {
    spec = JSON.parse(text);
  } catch {
    spec = null;
  }
  const ok = spec && typeof spec === "object" && !Array.isArray(spec);
  const base = ok ? spec : {};
  const skillsMode = base.skills === undefined ? "default" : (base.skills ?? []).length ? "picked" : "none";
  const extMode = base.extensions === undefined ? "default" : (base.extensions ?? []).length ? "picked" : "none";
  // `template: "auto"` 是「跟随默认」的别名（hb7k3j 的 picker 用的），表单里归到 default。
  const templateMode =
    !("template" in base) || base.template === "auto" ? "default" : base.template === null ? "none" : "picked";
  // cwd 三态跟 template 一条纪律：键不写 = 跟随默认，null = 显式不要，字符串 = 钉死路径。
  const cwdMode = !("cwd" in base) ? "default" : base.cwd === null ? "none" : "picked";
  return {
    broken: !ok,
    summary: base.summary ?? "",
    description: base.description ?? "",
    category: Array.isArray(base.category) ? [...base.category] : [],
    model: base.model ?? null,
    thinkingLevel: base.thinkingLevel ?? "",
    skillsMode,
    skills: [...(base.skills ?? [])],
    extMode,
    extensions: [...(base.extensions ?? [])],
    templateMode,
    template: typeof base.template === "string" && base.template !== "auto" ? base.template : "",
    cwdMode,
    cwd: typeof base.cwd === "string" ? base.cwd : "",
    readOnly: base.readOnly === true,
  };
}

/**
 * 表单草稿 → preset.json 的对象。**三态在这里复原**：
 *   技能「跟随默认」→ 不写这个键；「都不要」→ `[]`；「选几个」→ 那个数组
 *   模板「跟随默认」→ 不写；「无」→ `null`；「选一条」→ 字符串
 * `readOnly: false` 不写：缺席和 false 在引擎那边是同一件事，别给文件添噪音。
 */
export function specFromDraft(draft, name) {
  const spec = {
    v: 1,
    name,
    summary: String(draft.summary ?? "").trim(),
    category: sortCategories(draft.category ?? []),
  };
  if (draft.description?.trim()) spec.description = draft.description;
  if (draft.model?.provider && draft.model?.modelId) {
    spec.model = { provider: draft.model.provider, modelId: draft.model.modelId };
  }
  if (draft.thinkingLevel) spec.thinkingLevel = draft.thinkingLevel;
  if (draft.skillsMode === "picked") spec.skills = [...draft.skills];
  else if (draft.skillsMode === "none") spec.skills = [];
  if (draft.extMode === "picked") spec.extensions = [...draft.extensions];
  else if (draft.extMode === "none") spec.extensions = [];
  if (draft.templateMode === "picked" && draft.template) spec.template = draft.template;
  else if (draft.templateMode === "none") spec.template = null;
  if (draft.cwdMode === "picked" && draft.cwd) spec.cwd = draft.cwd;
  else if (draft.cwdMode === "none") spec.cwd = null;
  if (draft.readOnly) spec.readOnly = true;
  return spec;
}

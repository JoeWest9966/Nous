// app.js —— 画。所有判断都在 model.js 里，这里只把结果摆到 DOM 上。
//
// 这一版是**只读**的：没有新建、没有编辑、没有归档、启动按钮是灰的。不是偷懒 ——
// `ultimate.json` 的形状还没定（`sections.json` 里那段的 `checker` 是 `null`，引擎回 503），
// 形状没定就先长出编辑器的结果，是写完就扔。
//
// **没有任何示例数据。** 这一段现在就是空的，页面就画成空的 —— 顶上一条黄条说清为什么。
// 拿假数据把页面填满，等于让人以为仓里有东西：那是这一页最不该犯的错。

import { createApi, SectionNotReady } from "./api.js";
import {
  cardState,
  chipRows,
  draftProblems,
  emptyHint,
  epochToLocalInput,
  facetTree,
  filterPresets,
  formFields,
  localInputToEpoch,
  paramsOf,
  refsOf,
  sortCategories,
  stackOf,
  tileStyle,
  viewCounts,
  visibleViews,
} from "./model.js";

const $ = (id) => document.getElementById(id);
const esc = (value) =>
  String(value ?? "").replace(/[&<>"']/g, (ch) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[ch]);

const el = (tag, className, text) => {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
};

const api = createApi({ self: location.pathname });

const state = {
  rows: [],
  categories: [],
  archived: [],
  repo: null,
  notice: null,
  view: "all",
  category: null,
  query: "",
  selected: null,
  // 引用存在性由引擎给（`resolved`），这一页只翻译。
  known: { presets: null, prompts: null },
  /** 触发器表（引擎的投影）：name → { name, summary, params }。表单照它画。 */
  triggers: new Map(),
  /** 会话模板的名字（另一个段的名册）。 */
  sessions: new Set(),
  /** 正在编辑的草稿：null = 没在编辑。 */
  draft: null,
};

// ── 数据 ────────────────────────────────────────────────────────────────────

/** 接口来的行：`spec` 是盘上那份 `ultimate.json` 原文，`resolved` / `specError` 由引擎算。 */
const rowOfApi = (row) => {
  const spec = row.spec ?? {};
  const base = {
    name: row.name,
    summary: row.summary ?? spec.summary ?? "",
    category: row.category ?? spec.category ?? [],
    description: spec.description ?? null,
    trigger: spec.trigger ?? null,
    params: spec.params ?? {},
    session: spec.session ?? null,
    enabled: spec.enabled !== false,
    tracked: row.tracked !== false,
    specError: row.specError ?? null,
  };
  // 引擎的 `resolved` 说了算（它知道触发器表和会话模板名册）：这一页不自己判断引用在不在。
  const resolved = row.resolved ?? null;
  const refs = refsOf(base, {
    triggers: new Set(resolved?.trigger?.state === "ok" ? [String(base.trigger)] : []),
    sessions: new Set(resolved?.session?.state === "ok" ? [String(base.session)] : []),
  });
  const state_ = cardState({
    specError: base.specError,
    refs: resolved
      ? [
          { state: resolved.trigger?.state === "ok" ? "ok" : "missing", ref: base.trigger },
          { state: resolved.session?.state === "ok" ? "ok" : "missing", ref: base.session },
        ]
      : refs,
    tracked: base.tracked,
  });
  return { ...base, refs, resolved, state: state_ };
};

/** 这一段还装不了东西：**空着**画，顶上那条黄条是唯一的交代。 */
const loadUnavailable = (notice) => {
  state.notice = notice;
  state.rows = [];
  state.categories = [];
  state.archived = [];
  state.known = { presets: null, prompts: null };
  state.repo = null;
};

const load = async () => {
  state.notice = null;
  try {
    // 先问宿主「qrd3wh 在哪」，再问数据。顺序反了拿到的是「还没连上」——一句跟真实原因
    // 无关的话（这一段其实是 503，不是没连上）。
    if (!api.base) await api.connect();
    const data = await api.overview();
    state.repo = data.repo ?? {};
    state.categories = data.categories ?? [];
    state.archived = data.archived ?? [];

    // 触发器表 + 会话模板名册：**都要了才画得出手单**。
    // 问不到就空着 —— 页面要说清「没有触发器可选」，而不是画一个空的表单让人猜。
    const [triggers, sessions] = await Promise.all([
      api.triggers().catch(() => ({ triggers: [] })),
      api.overviewOf("Session-Template").catch(() => ({ presets: [] })),
    ]);
    state.triggers = new Map((triggers.triggers ?? []).map((item) => [item.name, item]));
    state.sessions = new Set((sessions.presets ?? []).map((item) => item.name));

    state.rows = (data.presets ?? []).map(rowOfApi);
  } catch (error) {
    // 503 = 这一段还没装好（形状未定）。这不是错误，是「还没长出来」—— 页面空着，
    // 黄条把引擎那句话原样摆出来。别的错误（没连上 / 块被删了）也照实报，一样空着。
    if (error instanceof SectionNotReady) loadUnavailable(error.message);
    else loadUnavailable(`问不到这一段：${error.message}`);
  }
  render();
};

// ── 顶栏 ────────────────────────────────────────────────────────────────────

const renderTop = () => {
  const remote = state.repo?.remote;
  $("crumb").textContent = `Nous-Template / Ultimate-Template`;
  $("repo").textContent = [
    state.repo?.head ? `HEAD ${String(state.repo.head).slice(0, 7)}` : null,
    remote ?? (state.repo ? "没有远端（只在本地）" : null),
  ]
    .filter(Boolean)
    .join(" · ") || "—";
  $("repo").dataset.drift = state.repo?.error ? "1" : "0";
  if (state.repo?.error) $("repo").textContent = state.repo.error;

  const sync = !state.repo
    ? { text: "这一段还装不了东西", dot: "wait" }
    : state.repo?.dirty
      ? { text: "有未提交改动", dot: "wait" }
      : (state.repo?.ahead ?? 0) > 0
        ? { text: `${state.repo.ahead} 个提交没推`, dot: "wait" }
        : (state.repo?.behind ?? 0) > 0
          ? { text: `远端多 ${state.repo.behind} 个`, dot: "wait" }
          : { text: "已同步", dot: "ok" };
  $("sync-text").textContent = sync.text;
  $("sync-dot").dataset.state = sync.dot;

  const banner = $("banner");
  if (state.notice) {
    banner.hidden = false;
    banner.textContent = state.notice;
  } else {
    banner.hidden = true;
    banner.textContent = "";
  }
};

// ── 左栏 ────────────────────────────────────────────────────────────────────

const renderRail = () => {
  const facets = $("facets");
  facets.innerHTML = "";
  const rows = filterPresets(state.rows, { view: "all" });
  const tree = facetTree(state.categories, rows);

  const all = el("button", "facet");
  all.type = "button";
  all.dataset.zero = "0";
  all.setAttribute("aria-current", String(state.category === null));
  all.append(el("span", "facet-label", "全部"), el("span", "facet-count", String(rows.length)));
  all.onclick = () => selectCategory(null);
  facets.append(all);

  for (const node of tree) {
    const button = el("button", "facet");
    button.type = "button";
    button.style.paddingLeft = `${8 + node.depth * 12}px`;
    button.dataset.zero = node.count === 0 ? "1" : "0";
    button.setAttribute("aria-current", String(state.category === node.name));
    button.append(el("span", "facet-label", node.label), el("span", "facet-count", String(node.count)));
    button.onclick = () => selectCategory(node.name);
    facets.append(button);
  }

  $("cat-count").textContent = String(tree.length);
  $("rail-foot").textContent = "类别、索引、回收站都在这一段文件夹里，跟 Session-Template 那段不共享。";
};

const selectCategory = (name) => {
  state.category = state.category === name ? null : name;
  render();
  if ($("rail").dataset.open === "1") {
    $("rail").dataset.open = "0";
    $("rail-scrim").hidden = true;
    $("rail-toggle").setAttribute("aria-expanded", "false");
  }
};

// ── 筛选 ────────────────────────────────────────────────────────────────────

const renderFilters = () => {
  const counts = viewCounts(state.rows, { archived: state.archived.length });
  const box = $("filters");
  box.innerHTML = "";
  for (const view of visibleViews(counts)) {
    const button = el("button", "filter");
    button.type = "button";
    button.dataset.tone = view.tone ?? "";
    button.setAttribute("aria-current", String(state.view === view.id));
    if (view.tone === "bad" || view.tone === "warn" || view.tone === "gone") button.append(el("span", "dot"));
    button.append(el("span", "", view.label), el("span", "n", String(counts[view.id] ?? 0)));
    button.onclick = () => {
      state.view = view.id;
      render();
    };
    box.append(button);
  }
};

// ── 卡片 ────────────────────────────────────────────────────────────────────

const badgesOf = (row) => {
  const badges = [];
  if (row.state === "invalid") badges.push(["引用失效", "bad"]);
  if (row.state === "failed") badges.push(["写坏了", "bad"]);
  if (row.state === "unpublished") badges.push(["未发布", "warn"]);
  if (row.readOnly === true) badges.push(["只读", "ok"]);
  return badges;
};

const cardFor = (row) => {
  const card = el("article", "card");
  card.tabIndex = 0;
  card.dataset.state = row.state;
  card.dataset.name = row.name;
  card.setAttribute("role", "button");
  card.setAttribute("aria-label", `打开 ${row.name}`);

  const top = el("div", "card-top");
  const tile = el("div", "tile", String(row.name ?? "?").slice(0, 1).toUpperCase());
  tile.style.cssText = tileStyle(row.name);
  const titles = el("div", "card-titles");
  titles.append(el("h3", "card-name", row.name), el("p", "card-summary", row.summary || "（没有摘要）"));
  top.append(tile, titles);
  card.append(top);

  if ((row.category ?? []).length) {
    const badges = el("div", "badges");
    for (const category of row.category) badges.append(el("span", "badge", category));
    card.append(badges);
  }

  const chips = el("div", "chips");
  for (const chip of chipRows(row, state.known)) {
    const node = el("span", "chip");
    node.dataset.state = chip.state === "bad" ? "invalid" : chip.state;
    node.title = chip.title ?? "";
    node.textContent = chip.text;
    chips.append(node);
  }
  card.append(chips);

  // 红卡要写清「修哪边」：写坏了修这条，引用没了去那边。
  const note =
    row.state === "failed" ? row.specError : row.state === "invalid" ? invalidNote(row) : null;
  if (note) card.append(el("p", "card-note", note));

  const foot = el("div", "card-foot");
  const run = el("button", "launch", "▶ 现在跑");
  run.type = "button";
  run.title = "跳过触发器判断，立刻起一条会话 —— 测这条组合本身对不对";
  run.onclick = (event) => {
    event.stopPropagation();
    launchCombo(row, run);
  };
  const edit = el("button", "launch", "改");
  edit.type = "button";
  edit.onclick = (event) => {
    event.stopPropagation();
    openEditor(row);
  };
  const hint = el("span", "card-hint", row.state === "unpublished" ? "只在本地" : "");
  foot.append(run, edit, hint);
  card.append(foot);

  const open = () => openDetail(row);
  card.onclick = open;
  card.onkeydown = (event) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      open();
    }
  };
  return card;
};

/** 「引用的东西不在了」要说清是哪一条 —— 说「引用失效」等于让人自己去数。 */
const invalidNote = (row) => {
  const missing = (row.refs ?? []).filter((ref) => ref.state === "missing");
  if (!missing.length) return "有一条引用不在了";
  return `引用的东西不在了：${missing.map((ref) => ref.ref ?? "（空）").join("、")}`;
};

const renderGrid = () => {
  const grid = $("grid");
  grid.innerHTML = "";
  const rows = filterPresets(state.rows, { view: state.view, category: state.category, query: state.query });
  $("result-count").textContent = `${rows.length} / ${state.rows.length} 条`;

  const subhead = $("subhead");
  if (state.category) {
    subhead.hidden = false;
    subhead.textContent = `类别 ${state.category}（含子类别）`;
  } else {
    subhead.hidden = true;
  }

  const empty = $("empty");
  if (!rows.length) {
    empty.hidden = false;
    const hint = emptyHint({ total: state.rows.length, view: state.view, category: state.category, query: state.query });
    empty.innerHTML = "";
    empty.append(el("p", "", esc(hint.title)), el("p", "dim small", esc(hint.body)));
    return;
  }
  empty.hidden = true;
  for (const row of rows) grid.append(cardFor(row));
};

// ── 详情 ────────────────────────────────────────────────────────────────────

const section = (title) => {
  const box = el("div", "sec");
  box.append(el("h4", "", title));
  return box;
};

const renderDetail = (row) => {
  const tile = $("dt-tile");
  tile.textContent = String(row.name ?? "?").slice(0, 1).toUpperCase();
  tile.style.cssText = tileStyle(row.name);
  $("dt-name").textContent = row.name;
  $("dt-meta").textContent = `Nous-Template / Ultimate-Template / ${row.name}`;

  const badges = $("dt-badges");
  badges.innerHTML = "";
  for (const [text, tone] of badgesOf(row)) {
    const node = el("span", "badge", text);
    node.dataset.tone = tone;
    badges.append(node);
  }

  const body = $("dt-body");
  body.innerHTML = "";

  const summary = section("摘要");
  summary.append(el("p", "prose", row.summary || "（没有摘要）"));
  body.append(summary);

  if (row.description) {
    const about = section("说明");
    about.append(el("p", "prose", row.description));
    body.append(about);
  }

  if (row.specError) {
    const broken = section("哪里写坏了");
    broken.append(el("p", "redline", row.specError));
    body.append(broken);
  }

  // 引用：触发器（什么时候起）+ 会话模板（起什么）。
  const stack = section("引用");
  const list = el("div", "ref-list");
  const items = stackOf(row, state.known);
  if (!items.length) list.append(el("p", "dim small", "还没挂触发器、也没钉会话模板。"));
  for (const item of items) {
    const line = el("div", "ref");
    line.dataset.state = item.state === "missing" ? "invalid" : "";
    line.append(el("span", "mono dim", `${item.order}.`), el("span", "", item.ref));
    if (item.state === "missing") line.append(el("span", "state", "不在"));
    list.append(line);
  }
  stack.append(list);
  body.append(stack);

  // 参数：照那个触发器自己的声明列出来（顺序也照它）。
  const params = section("参数");
  const schema = state.triggers.get(row.trigger)?.params ?? [];
  const rows = paramsOf(row, schema);
  if (!rows.length) {
    params.append(el("p", "prose dim", "这个触发器不要参数。"));
  } else {
    const table = el("dl", "kv");
    for (const param of rows) {
      table.append(el("dt", "", param.name));
      const value = param.value === undefined ? "（没填）" : String(param.value);
      table.append(el("dd", "", param.state === "unknown" ? `${value} · 触发器里没这个参数了` : value));
    }
    params.append(table);
  }
  body.append(params);

  // 跑过的会话：这条终极模板 fire 出来的会话（含手点「现在跑」），新在前。
  const fired = section("跑过的会话");
  fired.append(el("p", "dim small", "正在加载…"));
  body.append(fired);
  api
    .getRaw("v1/origins")
    .then(({ items = [] }) => {
      const mine = items.filter((it) => it.ultimate === row.name);
      fired.innerHTML = "";
      if (!mine.length) {
        fired.append(el("p", "dim small", "还没 fire 过任何会话。"));
        return;
      }
      const list = el("div", "ref-list");
      for (const it of mine) {
        const line = el("div", "ref");
        line.append(el("span", "mono dim", `#${it.id} `), el("span", "", it.name));
        const when = it.at ? new Date(it.at).toLocaleString() : "时间未知";
        const tag = el("span", "state", it.forced ? "手动" : "自动");
        tag.title = `${it.trigger ?? "?"} · ${when}`;
        line.append(tag);
        list.append(line);
      }
      fired.append(list);
    })
    .catch(() => {
      fired.innerHTML = "";
      fired.append(el("p", "dim small", "跑过的会话读不到。"));
    });

  const where = section("仓库");
  const kv = el("dl", "kv");
  const pairs = [
    ["段", "Ultimate-Template"],
    ["目录", `Nous-Template/Ultimate-Template/${row.name}/`],
    ["状态", row.tracked === false ? "未发布（只在本地）" : "已发布"],
  ];
  for (const [key, value] of pairs) kv.append(el("dt", "", key), el("dd", "", value));
  where.append(kv);
  body.append(where);

  const foot = $("dt-foot");
  foot.innerHTML = "";
  const run = el("button", "btn btn-go", "▶ 现在跑");
  run.type = "button";
  run.title = "跳过触发器判断，立刻起一条会话 —— 测这条组合本身对不对";
  run.onclick = () => launchCombo(row, run);
  const edit = el("button", "btn btn-primary", "改");
  edit.type = "button";
  edit.onclick = () => openEditor(row);
  const close = el("button", "btn", "关闭");
  close.onclick = closeDetail;
  foot.append(close, run, edit);
};

const openDetail = (row) => {
  state.selected = row.name;
  renderDetail(row);
  $("detail").hidden = false;
  $("dt-scrim").hidden = false;
  $("detail").focus?.();
};

const closeDetail = () => {
  state.selected = null;
  $("detail").hidden = true;
  $("dt-scrim").hidden = true;
};


// ── 新建 / 编辑 ─────────────────────────────────────────────────────────────
//
// 一条终极模板 = **触发器 + 触发器的参数 + 一条会话模板**。
// 三步都在这一个弹层里：选触发器 → 照它的声明填参数 → 选会话模板。
//
// **参数表单是照触发器的声明画出来的**，不是照触发器的名字：这一页里没有
// `if (名字 === "single-shot")` 那种代码。新加一个触发器 = 它自己声明参数，
// 这一页一个字节都不用改。

const draftDefaults = () => ({
  name: "",
  summary: "",
  trigger: "",
  params: {},
  session: "",
  category: [],
  enabled: true,
});

/** 打开编辑器。`row` 为 null = 新建。 */
const openEditor = (row = null) => {
  state.draft = row
    ? {
        name: row.name,
        summary: row.summary ?? "",
        trigger: row.trigger ?? "",
        params: { ...(row.params ?? {}) },
        session: row.session ?? "",
        category: [...(row.category ?? [])],
        enabled: row.enabled !== false,
      }
    : draftDefaults();
  state.editing = row?.name ?? null;

  $("edit-title").textContent = row ? `改 ${row.name}` : "新建终极模板";
  $("edit-name").value = state.draft.name;
  $("edit-summary").value = state.draft.summary;
  $("edit-category").value = (state.draft.category ?? []).join(", ");
  $("edit-enabled").checked = state.draft.enabled !== false;
  $("edit-err").hidden = true;

  // 类别候选：这一段的词表 + 用过的。
  const cats = [...new Set([...(state.categories ?? []), ...state.rows.flatMap((entry) => entry.category ?? [])])];
  $("edit-cats").innerHTML = "";
  for (const name of cats) {
    const option = el("option");
    option.value = name;
    $("edit-cats").append(option);
  }

  renderTriggerPicker();
  renderSessionPicker();
  renderParamFields();
  refreshDraft();

  $("edit-scrim").hidden = false;
  $("edit").hidden = false;
  $("edit-name").focus?.();
};

const closeEditor = () => {
  state.draft = null;
  state.editing = null;
  $("edit-scrim").hidden = true;
  $("edit").hidden = true;
};

/** 触发器：一排可点的条。**这一页不按名字写死任何一个** —— 列表来自引擎。 */
const renderTriggerPicker = () => {
  const box = $("edit-triggers");
  box.innerHTML = "";
  const triggers = [...state.triggers.values()];
  if (!triggers.length) {
    box.append(el("span", "picker-empty", "一个触发器都没有 —— 去触发器页建一个。"));
    return;
  }
  for (const trigger of triggers) {
    const button = el("button", "pick");
    button.type = "button";
    button.setAttribute("aria-pressed", String(state.draft?.trigger === trigger.name));
    button.append(el("span", "", trigger.name));
    const params = (trigger.params ?? []).length;
    button.append(el("span", "pick-sub", params ? `要 ${params} 个参数` : trigger.summary || "不要参数"));
    button.onclick = () => {
      state.draft.trigger = trigger.name;
      // 换触发器 = 换参数表。旧参数留着只会变成「触发器里没这个参数了」。
      state.draft.params = {};
      renderTriggerPicker();
      renderParamFields();
      refreshDraft();
    };
    box.append(button);
  }
};

/** 会话模板：同一段（Session-Template）的名册。 */
const renderSessionPicker = () => {
  const box = $("edit-sessions");
  box.innerHTML = "";
  if (!state.sessions.size) {
    box.append(el("span", "picker-empty", "没有会话模板 —— 去会话模板页建一个。"));
    return;
  }
  for (const name of state.sessions) {
    const button = el("button", "pick");
    button.type = "button";
    button.setAttribute("aria-pressed", String(state.draft?.session === name));
    button.append(el("span", "", name));
    button.onclick = () => {
      state.draft.session = name;
      renderSessionPicker();
      refreshDraft();
    };
    box.append(button);
  }
};

/** 参数表单：照触发器的声明画。**控件只看类型。** */
const renderParamFields = () => {
  const wrap = $("edit-params-wrap");
  const box = $("edit-params");
  box.innerHTML = "";
  const trigger = state.triggers.get(state.draft?.trigger);
  const fields = formFields(trigger);
  wrap.hidden = fields.length === 0;
  for (const field of fields) {
    const label = el("label", "field");
    label.append(el("span", "field-k", `${field.label}${field.required ? " *" : ""}`));
    const current = state.draft.params[field.name];

    let input;
    if (field.control === "datetime") {
      // epoch 在盘上是 UTC 秒；页面上人挑的是**本地时间** —— 挑的时候就得说清是哪一种。
      input = el("input");
      input.type = "datetime-local";
      input.value = epochToLocalInput(current);
      input.oninput = () => {
        const epoch = localInputToEpoch(input.value);
        if (epoch === null) delete state.draft.params[field.name];
        else state.draft.params[field.name] = epoch;
        refreshDraft();
      };
    } else if (field.control === "number") {
      input = el("input");
      input.type = "number";
      input.value = current ?? "";
      input.oninput = () => {
        const value = Number(input.value);
        if (input.value === "" || !Number.isFinite(value)) delete state.draft.params[field.name];
        else state.draft.params[field.name] = value;
        refreshDraft();
      };
    } else if (field.control === "checkbox") {
      input = el("input");
      input.type = "checkbox";
      input.checked = current === true;
      input.onchange = () => {
        state.draft.params[field.name] = input.checked;
        refreshDraft();
      };
      const row = el("label", "check");
      row.append(input, el("span", "", field.label));
      label.innerHTML = "";
      label.append(row);
    } else if (field.control === "select") {
      input = el("select");
      for (const option of field.options) {
        const node = el("option");
        node.value = option;
        node.textContent = option;
        input.append(node);
      }
      input.value = current ?? field.options[0] ?? "";
      input.onchange = () => {
        state.draft.params[field.name] = input.value;
        refreshDraft();
      };
    } else if (field.control === "textarea") {
      input = el("textarea");
      input.rows = 3;
      input.value = current ?? "";
      input.oninput = () => {
        state.draft.params[field.name] = input.value;
        refreshDraft();
      };
    } else {
      input = el("input");
      input.type = "text";
      input.value = current ?? "";
      input.oninput = () => {
        state.draft.params[field.name] = input.value;
        refreshDraft();
      };
    }
    if (field.control !== "checkbox") label.append(input);
    if (field.help) label.append(el("span", "field-help", field.help));
    box.append(label);
  }
};

/** 表单状态 → 能不能存 + 还差什么。 */
const refreshDraft = () => {
  if (!state.draft) return;
  state.draft.name = $("edit-name").value.trim();
  state.draft.summary = $("edit-summary").value.trim();
  state.draft.enabled = $("edit-enabled").checked;
  state.draft.category = $("edit-category")
    .value.split(/[,，]/)
    .map((value) => value.trim())
    .filter(Boolean);

  const problems = draftProblems(state.draft, { triggers: state.triggers, sessions: state.sessions });
  $("edit-save").disabled = problems.length > 0;
  const err = $("edit-err");
  if (problems.length) {
    err.hidden = false;
    err.textContent = problems.join("；");
  } else {
    err.hidden = true;
    err.textContent = "";
  }
  const hint = $("edit-hint");
  hint.textContent = state.draft.trigger
    ? `触发器「${state.draft.trigger}」的参数按它自己的声明填 —— 换触发器就换表单。`
    : "先选一个触发器：它决定这条什么时候起。";
};

/**
 * 发布这一段：校验 + 重扫索引 + commit + push。**走引擎** —— 这一页不跑 git。
 * 发布的是**这一段**（Ultimate-Template/），另一段的东西一个字节都不带。
 */
const publishSection = async () => {
  const button = $("publish");
  button.disabled = true;
  button.textContent = "发布中…";
  try {
    const result = await api.post("v1/presets/publish", { message: "ultimate: 更新终极模板" });
    const warnings = (result.warnings ?? []).join("；");
    toast(warnings ? `已发布，但：${warnings}` : "已发布");
    await load();
  } catch (error) {
    toast(`发布失败：${error.message}`);
  } finally {
    button.disabled = false;
    button.textContent = "发布";
  }
};

/**
 * 手动跑一次：**跳过触发器判断**，直接起一条会话。
 * 这是「fire → 起会话」这条路的测试 / 手动入口 —— 测的是参数、会话模板、展开对不对，
 * 不是触发器「该不该 fire」（那是触发器自己的单测）。
 */
const launchCombo = async (row, button = null) => {
  if (button) button.disabled = true;
  try {
    const result = await api.post("v1/presets/launch", { name: row.name });
    toast(`已开一条会话：${result.name ?? result.id}`);
  } catch (error) {
    toast(`没开起来：${error.message}`);
  } finally {
    if (button) button.disabled = false;
  }
};

/** 存。形状和引用都让引擎判 —— 这一页只拦明显不行的。 */
const submitEditor = async (event) => {
  event.preventDefault();
  if (!state.draft) return;
  const body = {
    name: state.draft.name,
    summary: state.draft.summary,
    category: state.draft.category,
    trigger: state.draft.trigger,
    params: state.draft.params,
    session: state.draft.session,
    enabled: state.draft.enabled,
    spec: {
      v: 1,
      name: state.draft.name,
      summary: state.draft.summary,
      category: state.draft.category,
      trigger: state.draft.trigger,
      params: state.draft.params,
      session: state.draft.session,
      enabled: state.draft.enabled,
    },
  };
  $("edit-save").disabled = true;
  try {
    if (state.editing) {
      // 改一条 = 整份写回去（清单文件是配置，不是散文：整份覆盖比打补丁诚实）。
      await api.post("v1/presets/file", {
        name: state.editing,
        path: "ultimate.json",
        text: `${JSON.stringify(body.spec, null, 2)}\n`,
      });
      toast(`已改 ${body.name} · 还没发布`);
    } else {
      await api.post("v1/presets/create", body);
      toast(`已建 ${body.name} · 未发布`);
    }
    closeEditor();
    await load();
  } catch (error) {
    $("edit-err").hidden = false;
    $("edit-err").textContent = error.message;
  } finally {
    refreshDraft();
  }
};

// ── 总装 ────────────────────────────────────────────────────────────────────

const render = () => {
  renderTop();
  renderRail();
  renderFilters();
  renderGrid();
  if (state.selected) {
    const row = state.rows.find((entry) => entry.name === state.selected);
    if (row) renderDetail(row);
    else closeDetail();
  }
};

const toast = (text) => {
  const node = el("div", "toast", text);
  $("toasts").append(node);
  setTimeout(() => node.remove(), 4000);
};

const bind = () => {
  $("refresh").onclick = () => {
    load().catch((error) => toast(error.message));
  };
  $("new").onclick = () => openEditor(null);
  $("publish").onclick = () => publishSection();
  $("edit-cancel").onclick = closeEditor;
  $("edit-scrim").onclick = closeEditor;
  $("edit").addEventListener("submit", submitEditor);
  for (const id of ["edit-name", "edit-summary", "edit-category"]) {
    $(id).addEventListener("input", refreshDraft);
  }
  $("edit-enabled").addEventListener("change", refreshDraft);
  $("sync").onclick = () => {
    load().catch((error) => toast(error.message));
  };
  $("search").oninput = () => {
    state.query = $("search").value;
    render();
  };
  $("dt-close").onclick = closeDetail;
  $("dt-scrim").onclick = closeDetail;

  // 窄屏的左栏是个抽屉：没有这个按钮，手机上的类别筛选就是一个拉不开的抽屉。
  const rail = $("rail");
  const setRail = (open) => {
    rail.dataset.open = open ? "1" : "0";
    $("rail-scrim").hidden = !open;
    $("rail-toggle").setAttribute("aria-expanded", String(open));
  };
  $("rail-toggle").onclick = () => setRail(rail.dataset.open !== "1");
  $("rail-scrim").onclick = () => setRail(false);
  // Esc 只收一层：这一版只有详情那一层。
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    if (!$("edit").hidden) {
      event.preventDefault();
      closeEditor();
      return;
    }
    if (!$("detail").hidden) {
      event.preventDefault();
      closeDetail();
    }
  });
};

bind();
load().catch((error) => toast(error.message));

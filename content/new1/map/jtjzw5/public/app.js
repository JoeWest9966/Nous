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
  canLaunch,
  cardState,
  chipRows,
  emptyHint,
  facetTree,
  filterPresets,
  launchBlockedReason,
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
  known: { presets: null, prompts: null },
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
    presets: spec.presets,
    prompt: spec.prompt,
    readOnly: spec.readOnly,
    tracked: row.tracked !== false,
    specError: row.specError ?? null,
  };
  const refs = refsOf(base, state.known);
  return { ...base, refs, state: cardState({ specError: base.specError, refs, tracked: base.tracked }) };
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
    state.known = { presets: null, prompts: null };
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
  const launch = el("button", "launch", "▶ 启动");
  launch.type = "button";
  launch.disabled = !canLaunch();
  launch.title = launchBlockedReason();
  const hint = el("span", "card-hint", row.state === "unpublished" ? "只在本地" : "");
  foot.append(launch, hint);
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

  // 叠放顺序：有顺序，所以是列表不是 chip。
  const stack = section("叠放顺序（先叠的在前）");
  const list = el("div", "ref-list");
  const items = stackOf(row, state.known);
  if (!items.length) list.append(el("p", "dim small", "一条预设都不叠 —— 这条只是一层壳。"));
  for (const item of items) {
    const line = el("div", "ref");
    line.dataset.state = item.state === "missing" ? "invalid" : "";
    line.append(el("span", "mono dim", `${item.order}.`), el("span", "", item.ref));
    if (item.state === "missing") line.append(el("span", "state", "不在"));
    list.append(line);
  }
  stack.append(list);
  body.append(stack);

  // 提示词：三态在三行字里说清，不用一个「—」糊过去。
  const prompt = section("提示词");
  if (row.prompt === undefined) prompt.append(el("p", "prose dim", "没写 —— 跟随 Nous-prompts 的仓库默认"));
  else if (row.prompt === null) prompt.append(el("p", "prose", "null —— 明确不要模板（跟「没说」不是一件事）"));
  else prompt.append(el("p", "prose", String(row.prompt)));
  body.append(prompt);

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
  const launch = el("button", canLaunch() ? "btn btn-go" : "btn", "▶ 启动");
  launch.disabled = !canLaunch();
  launch.title = launchBlockedReason();
  const close = el("button", "btn", "关闭");
  close.onclick = closeDetail;
  foot.append(close, launch);
  if (!canLaunch()) foot.append(el("span", "dim small", "形状未定，展开还没接"));
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
    if (!$("detail").hidden) {
      event.preventDefault();
      closeDetail();
    }
  });
};

bind();
load().catch((error) => toast(error.message));

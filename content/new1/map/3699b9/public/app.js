// app.js —— 3699b9 的 DOM 那一半：渲染 / 菜单 / 手势 / 浮层 / 编辑器 / 启动。
//
// 分工：
//   model.js  算「这条是什么状态、这一屏该画哪些」（纯函数，有测试）
//   api.js    只跟 qrd3wh 说话（纯数据，有测试）
//   app.js    这一份：只做 DOM 和事件
//
// 三条不许踩（前两条抄自 6ysg01，第三条是本页独有的）：
//   1. **不复刻服务端的闸。** 名字 / 摘要 / 引用的判定全在 qrd3wh，这一页只播报它那一行原文。
//   2. **改完要重扫。** 写端点自己会 load；命令行改了盘就点「重扫」。
//   3. **面板上写的 = 实际跑的。** chip 上的模型 / 档位 / 模板一律读服务端算的 `resolved`。
//
// Esc 的层级（一次一层，别写死某一个）：
//   菜单 → 启动确认 → 新建 → 确认框 → 编辑抽屉 → 详情 → 列表。

import { buildDiff } from "./diff.js";
import { createBoard } from "./board.js";
import {
  ConflictError,
  CONFLICT_MESSAGE,
  createApi,
  createValidateDebounce,
  launchPreset,
  siblingBase,
} from "./api.js";
import {
  canLaunch,
  cardState,
  categoryImpact,
  chipRows,
  defaultSessionName,
  draftFromText,
  emptyHint,
  facetTree,
  filterPresets,
  isUnder,
  launchWarnings,
  sortCategories,
  specFromDraft,
  viewCounts,
  visibleViews,
} from "./model.js";

// ── 小工具 ─────────────────────────────────────────────────────────────────

const $ = (id) => document.getElementById(id);
const esc = (value) =>
  String(value ?? "").replace(/[&<>"']/g, (ch) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[ch]);

/** 建元素的小壳：`el("button", "btn", "文字")`。省掉一堆 createElement 赋值。 */
function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** 名字 → 色相。同一个名字永远同一个颜色（跟 68kazk / 6ysg01 / vbe4pt 同一个算法）。 */
function hueOf(name) {
  let hash = 0;
  for (const ch of String(name ?? "")) hash = (hash * 31 + ch.codePointAt(0)) % 360;
  return hash;
}
const tileStyle = (name) => `background:hsl(${hueOf(name)} 42% 46%)`;
const monoOf = (name) => (String(name ?? "").trim()[0] ?? "?").toUpperCase();

function toast(text, tone = "") {
  const node = el("div", "toast", text);
  if (tone) node.dataset.tone = tone;
  $("toasts").append(node);
  setTimeout(() => node.remove(), tone === "bad" ? 6200 : 3600);
}

/** 剪贴板：`navigator.clipboard` 在 http 下不存在（这页挂在 tailnet 上，不是 secure context）。 */
async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const area = el("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.append(area);
    area.select();
    const ok = document.execCommand?.("copy") ?? false;
    area.remove();
    return ok;
  }
}

// ── 状态 ───────────────────────────────────────────────────────────────────

const state = {
  presets: [],
  archived: [],
  categories: [],
  repo: {},
  /** 顶栏那组筛选。`null` = 没选（等于「全部」）。 */
  view: "all",
  /** 右栏类别。跟 `view` 是同一条轴：一击只有一个亮。 */
  category: null,
  query: "",
  selected: null, // 详情里那条的 name
  editing: null, // 编辑器里那条的 name
  kb: { row: 0, col: 0, on: false },
  railOpen: false,
};

let api = null;
let models = []; // `/v1/models` 的 items，编辑器里的模型选择用
let deviceModel = null; // 设备默认模型（`/v1/devices` 的 model）
let validateDebounce = null;

// ── 数据 ───────────────────────────────────────────────────────────────────

async function boot() {
  api = createApi({ self: location.pathname });
  try {
    await api.connect();
  } catch (error) {
    renderFatal(error);
    return;
  }
  validateDebounce = createValidateDebounce({
    validate: (payload) => api.post("v1/presets/validate", payload),
  });
  await Promise.all([refresh(), refreshDevice()]);
}

async function refreshDevice() {
  try {
    const device = await api.get("v1/device");
    deviceModel = device?.model ?? null;
  } catch {
    deviceModel = null;
  }
  try {
    const list = await api.get("v1/models");
    models = list?.items ?? [];
  } catch {
    models = [];
  }
}

async function refresh() {
  try {
    const data = await api.get("v1/presets");
    state.presets = data.presets ?? [];
    state.archived = data.archived ?? [];
    state.categories = data.categories ?? [];
    state.repo = data.repo ?? {};
    // 选中的那条被删了 / 归档了：详情得自己关掉，别留一张幽灵。
    if (state.selected && !state.presets.some((preset) => preset.name === state.selected)) closeDetail();
    render();
  } catch (error) {
    renderFatal(error);
  }
}

const findPreset = (name) => state.presets.find((preset) => preset.name === name) ?? null;

// ── 顶栏 / 左栏 / 筛选 ─────────────────────────────────────────────────────

function render() {
  renderTop();
  renderRail();
  renderFilters();
  renderGrid();
  if (state.selected) renderDetail(state.selected);
}

function renderFatal(error) {
  const message = String(error?.message ?? error);
  $("crumb").textContent = "没连上";
  $("repo").textContent = message;
  $("repo").dataset.drift = "1";
  $("sync-text").textContent = "没连上";
  $("sync-dot").dataset.state = "bad";
  $("grid").innerHTML = "";
  const box = $("empty");
  box.hidden = false;
  box.innerHTML = `<p class="redline">${esc(message)}</p>
    <p class="dim small">这一页只做视图：预设的新建 / 编辑 / 发布 / 启动全在会话引擎（qrd3wh）那儿。它不在，就什么都问不了。</p>`;
}

function renderTop() {
  const counts = viewCounts(state.presets, state.archived);
  $("crumb").innerHTML = `new1<span class="sep">/</span>3699b9<span class="sep">·</span>${counts.all} 条预设${
    state.archived.length ? ` · 归档 ${state.archived.length}` : ""
  }`;

  const repo = state.repo ?? {};
  const bits = [];
  if (repo.head) bits.push(`HEAD ${String(repo.head).slice(0, 7)}`);
  if (repo.remote) bits.push(String(repo.remote).replace(/\/\/[^@/]+@/, "//"));
  else bits.push("没有远端（只在本地）");
  $("repo").textContent = bits.join(" · ");
  $("repo").dataset.drift = repo.error ? "1" : "";
  if (repo.error) $("repo").textContent = repo.error;

  // 同步 chip：脏 / 领先 / 落后 / 失败，四个不同的说法。
  let text = "已同步";
  let tone = "ok";
  if (repo.error) {
    text = "仓库读不了";
    tone = "bad";
  } else if (repo.dirty) {
    text = "有未提交改动";
    tone = "wait";
  }
  if (repo.ahead) {
    text = `${repo.ahead} 个提交没推`;
    tone = "wait";
  }
  if (repo.behind) {
    text = `远端多 ${repo.behind} 个`;
    tone = "wait";
  }
  $("sync-text").textContent = text;
  $("sync-dot").dataset.state = tone;
  $("sync").dataset.state = tone;
}

function renderRail() {
  const tree = facetTree(state.categories, state.presets);
  $("cat-count").textContent = tree.length ? String(tree.length) : "—";
  const box = $("facets");
  box.innerHTML = "";
  box.append(facetButton({ path: null, label: "全部", count: state.presets.length, depth: 0 }));
  for (const node of tree) box.append(facetButton(node));
  $("rail-foot").textContent = state.repo?.remote
    ? `仓在 ${String(state.repo.remote).replace(/\/\/[^@/]+@/, "//")}`
    : "这个仓没有远端 —— 本地也可以（专属私仓的 origin 归你自己配）";
}

function facetButton({ path, label, count, depth }) {
  const wrap = el("div");
  wrap.style.display = "flex";
  wrap.style.alignItems = "center";
  const button = el("button", "facet");
  button.type = "button";
  button.style.paddingLeft = `${8 + depth * 12}px`;
  button.setAttribute("aria-current", String(state.category === path));
  if ((count ?? 0) === 0 && path) button.dataset.zero = "1";
  button.innerHTML = `<span class="facet-label">${esc(label)}</span><span class="facet-count">${count ?? 0}</span>`;
  button.onclick = () => {
    // 右栏和顶栏是同一条轴：点类别，顶栏落回「全部」。
    state.category = path;
    state.view = "all";
    render();
  };
  wrap.append(button);
  if (path) {
    const more = el("button", "facet-more", "⋯");
    more.type = "button";
    more.title = "类别菜单";
    more.setAttribute("aria-haspopup", "menu");
    more.onclick = (event) => {
      event.stopPropagation();
      const rect = more.getBoundingClientRect();
      openCategoryMenu(rect.left, rect.bottom + 4, path);
    };
    attachMenuGesture(button, (event) => openCategoryMenu(event.clientX, event.clientY, path));
    wrap.append(more);
  }
  return wrap;
}

function renderFilters() {
  const counts = viewCounts(state.presets, state.archived);
  const box = $("filters");
  box.innerHTML = "";
  for (const view of visibleViews(counts)) {
    const button = el("button", "filter");
    button.type = "button";
    button.dataset.view = view.key;
    button.setAttribute("aria-current", String(state.view === view.key && !state.category));
    const tone = view.key === "unpublished" ? "warn" : view.key === "invalid" || view.key === "failed" ? "bad" : view.key === "archived" ? "gone" : "";
    button.innerHTML = `${tone ? `<span class="dot" data-state="${tone === "gone" ? "" : tone}"></span>` : ""}${esc(
      view.label,
    )}<span class="n">${counts[view.key] ?? 0}</span>`;
    // 「已归档」只在归档区非空时出现；点它就把主区换成回收站。
    button.onclick = () => {
      state.view = view.key;
      state.category = null;
      render();
    };
    box.append(button);
  }
}

// ── 卡片网格 ───────────────────────────────────────────────────────────────

function renderGrid() {
  const archivedView = state.view === "archived";
  $("archived").hidden = !archivedView;
  $("grid").hidden = archivedView;

  if (archivedView) {
    // 先把网格清空：归档区不画卡，但旧卡留在 DOM 里就会「看起来还在列表里」。
    $("grid").innerHTML = "";
    renderArchived();
    $("empty").hidden = true;
    $("subhead").hidden = true;
    $("result-count").textContent = `${state.archived.length} 条已归档`;
    return;
  }

  const rows = filterPresets(state.presets, { view: state.view, category: state.category, query: state.query });
  $("result-count").textContent = `${rows.length} / ${state.presets.length} 条`;
  $("subhead").hidden = !(state.category || state.query);
  if (state.category || state.query) {
    const bits = [];
    if (state.category) {
      const children = facetTree(state.categories, state.presets).find((node) => node.path === state.category)?.children ?? [];
      bits.push(children.length ? `${state.category} · 含 ${children.length} 个子级` : state.category);
    }
    if (state.query) bits.push(`搜「${state.query}」`);
    $("subhead").textContent = bits.join(" · ");
  }

  const grid = $("grid");
  grid.innerHTML = "";
  for (const [index, preset] of rows.entries()) {
    grid.append(cardFor(preset, index));
  }

  paintKb();

  const empty = $("empty");
  if (rows.length) {
    empty.hidden = true;
    return;
  }
  empty.hidden = false;
  const hint = emptyHint({
    view: state.view,
    category: state.category,
    query: state.query,
    presets: state.presets,
    archived: [],
  });
  empty.innerHTML = `<p>${esc(hint.text)}</p>`;
  const actions = el("div", "empty-actions");
  if (hint.action) {
    const button = el("button", "btn", hint.action.label);
    button.type = "button";
    button.onclick = () => {
      if (hint.action.kind === "clear-search") {
        state.query = "";
        $("search").value = "";
      }
      if (hint.action.kind === "clear-category") state.category = null;
      if (hint.action.kind === "view-all") state.view = "all";
      render();
    };
    actions.append(button);
  }
  // 一条都没有时给**真能走的**链接，不是「暂未开放」四个字。
  if (!state.presets.length) {
    const manual = siblingBase(api.ctx, "hb7k3j");
    const prompts = siblingBase(api.ctx, "6ysg01");
    if (manual) actions.append(linkButton(`${manual}`, "去手动建一条（hb7k3j）", false));
    if (prompts) actions.append(linkButton(`${prompts}`, "去写一条模板（6ysg01）", false));
  }
  empty.append(actions);
  paintKb();
}

function linkButton(href, label, blank = true) {
  const anchor = el("a", "btn", label);
  anchor.href = href;
  if (blank) {
    anchor.target = "_blank";
    anchor.rel = "noopener";
  }
  return anchor;
}

function cardFor(preset, index) {
  const status = cardState(preset);
  const card = el("article", "card");
  card.tabIndex = 0;
  card.dataset.state = status;
  card.dataset.name = preset.name;
  card.dataset.index = String(index);

  const top = el("div", "card-top");
  const tile = el("div", "tile", monoOf(preset.name));
  tile.style.cssText = tileStyle(preset.name);
  const titles = el("div", "card-titles");
  const name = el("h3", "card-name", preset.name);
  const summary = el("p", "card-summary", preset.summary || "（没有摘要）");
  summary.title = preset.summary ?? "";
  titles.append(name, summary);

  const more = el("button", "card-more", "⋯");
  more.type = "button";
  more.title = "菜单（右键 / 长按也行）";
  more.setAttribute("aria-haspopup", "menu");
  more.onclick = (event) => {
    event.stopPropagation();
    const rect = more.getBoundingClientRect();
    openCardMenu(rect.left, rect.bottom + 4, preset);
  };
  top.append(tile, titles, more);

  const badges = el("div", "badges");
  for (const badge of badgesFor(preset, status)) badges.append(badge);

  const chips = el("div", "chips");
  for (const row of chipRows(preset)) {
    const chip = el("span", "chip");
    chip.dataset.state = row.state;
    chip.innerHTML = `<span class="k">${esc(row.label)}</span>${esc(row.text)}`;
    chip.title = `${row.label}：${row.text}`;
    chips.append(chip);
  }

  const foot = el("div", "card-foot");
  const launch = el("button", "launch", "▶ 启动");
  launch.type = "button";
  launch.disabled = !canLaunch(preset);
  launch.title = launch.disabled ? "引用失效了，先修好再开（点卡看是哪一条）" : "开一条新对话（新标签页）";
  launch.onclick = (event) => {
    event.stopPropagation();
    openLaunch(preset);
  };
  foot.append(launch);
  const hint = el("span", "card-hint", status === "unpublished" ? "未发布" : "");
  foot.append(hint);

  card.append(top, badges, chips);
  const note = noteFor(preset, status);
  if (note) card.append(el("p", "card-note", note));
  card.append(foot);

  card.onclick = () => openDetail(preset.name);
  card.onkeydown = (event) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      openDetail(preset.name);
    }
  };
  attachMenuGesture(card, (event) => openCardMenu(event.clientX, event.clientY, preset));
  return card;
}

function badgesFor(preset, status) {
  const out = [];
  const badge = (text, tone) => {
    const node = el("span", "badge", text);
    if (tone) node.dataset.tone = tone;
    return node;
  };
  if (status === "failed") out.push(badge("写坏了", "bad"));
  else if (status === "invalid") out.push(badge("引用失效", "bad"));
  else if (status === "unpublished") out.push(badge("未发布", "warn"));
  else out.push(badge("已发布", "ok"));
  for (const category of preset.category ?? []) out.push(badge(category));
  return out;
}

function noteFor(preset, status) {
  if (status === "failed") return preset.specError ?? "preset.json 读不出来";
  if (status !== "invalid") return null;
  const resolved = preset.resolved ?? {};
  const bits = [];
  if (resolved.model?.state === "invalid") bits.push(`模型 ${resolved.model.ref}`);
  if (resolved.thinking?.state === "invalid") bits.push(`档位 ${resolved.thinking.ref}`);
  if (resolved.template?.state === "invalid") bits.push(`模板 ${resolved.template.ref}`);
  for (const row of resolved.skills ?? []) if (row.state === "invalid") bits.push(`技能 ${row.ref}`);
  for (const row of resolved.extensions ?? []) if (row.state === "invalid") bits.push(`扩展 ${row.ref}`);
  return `引用的东西不在了：${bits.join("、")}`;
}

function renderArchived() {
  const box = $("archived");
  box.innerHTML = "";
  if (!state.archived.length) {
    box.append(el("p", "dim", "归档区是空的。"));
    return;
  }
  for (const entry of state.archived) {
    const row = el("div", "arch-row");
    const tile = el("div", "tile", monoOf(entry.name));
    tile.style.cssText = tileStyle(entry.name);
    const titles = el("div", "card-titles");
    titles.append(el("h3", "card-name", entry.name));
    titles.append(el("p", "card-summary", entry.summary || "（没有摘要）"));
    const actions = el("div", "arch-actions");
    const restore = el("button", "btn btn-small", "恢复");
    restore.type = "button";
    restore.onclick = async () => {
      await runAction(() => api.post("v1/presets/restore", { name: entry.name }), `已恢复 ${entry.name}`);
      // 恢复完回列表：留在回收站里看一行「已恢复」的空白，等于让人怀疑它到底回来没有。
      state.view = "all";
      render();
    };
    const purge = el("button", "btn btn-small btn-bad", "彻底删除");
    purge.type = "button";
    purge.onclick = () => openDanger({ kind: "purge", name: entry.name });
    actions.append(restore, purge);
    row.append(tile, titles, actions);
    box.append(row);
  }
}

// ── 键盘焦点（焦点是「格」，不是项） ───────────────────────────────────────

function paintKb() {
  const cards = [...$("grid").querySelectorAll(".card")];
  for (const card of cards) {
    card.removeAttribute("data-kb");
    card.style.outline = "";
    card.style.outlineOffset = "";
  }
  if (!state.kb.on || !cards.length) return;
  const at = Math.min(state.kb.row * columnsOf() + state.kb.col, cards.length - 1);
  const card = cards[Math.max(0, at)];
  if (!card) return;
  card.dataset.kb = "1";
  card.style.outline = "2px solid var(--ink)";
  card.style.outlineOffset = "-2px";
  card.scrollIntoView({ block: "nearest" });
}

function columnsOf() {
  const grid = $("grid");
  if (!grid.children.length) return 1;
  const top = grid.children[0].getBoundingClientRect().top;
  let count = 0;
  for (const child of grid.children) {
    if (Math.abs(child.getBoundingClientRect().top - top) > 2) break;
    count += 1;
  }
  return Math.max(1, count);
}

// ── 详情 ───────────────────────────────────────────────────────────────────

function openDetail(name) {
  state.selected = name;
  $("detail").hidden = false;
  $("dt-scrim").hidden = false;
  document.querySelector(".layout").inert = true;
  $("detail").inert = false;
  $("detail").focus();
  renderDetail(name);
}

function closeDetail() {
  state.selected = null;
  $("detail").hidden = true;
  $("dt-scrim").hidden = true;
  document.querySelector(".layout").inert = false;
}

function renderDetail(name) {
  const preset = findPreset(name);
  if (!preset) return closeDetail();
  const status = cardState(preset);
  const resolved = preset.resolved ?? {};

  $("dt-mono").textContent = monoOf(preset.name);
  $("dt-tile").style.cssText = tileStyle(preset.name);
  $("dt-name").textContent = preset.name;
  const badges = $("dt-badges");
  badges.innerHTML = "";
  for (const badge of badgesFor(preset, status)) badges.append(badge);
  $("dt-meta").textContent = [preset.tracked ? `HEAD 里有` : "未发布（只在盘上）", preset.dir]
    .filter(Boolean)
    .join(" · ");

  const body = $("dt-body");
  body.innerHTML = "";

  // 写坏了 / 引用失效：最上面一条红字，说清去动哪儿。
  if (status === "failed") {
    const sec = el("div", "sec");
    sec.append(el("p", "redline", preset.specError ?? "preset.json 读不出来"));
    sec.append(el("p", "dim small", "这条**自己**写坏了 —— 去「编辑」里改 preset.json。"));
    body.append(sec);
  }
  if (status === "invalid") {
    const sec = el("div", "sec");
    sec.append(el("p", "redline", noteFor(preset, status)));
    sec.append(el("p", "dim small", "引用的东西没了 —— 去那边修好，或者回来把这条引用改掉。"));
    body.append(sec);
  }

  body.append(sectionSummary(preset));
  const description = sectionDescription(preset);
  if (description) body.append(description);
  body.append(sectionCategories(preset));
  body.append(sectionRefs(preset, resolved));
  body.append(sectionRepo(preset));

  const foot = $("dt-foot");
  foot.innerHTML = "";
  const edit = el("button", "btn btn-small", "编辑");
  edit.type = "button";
  edit.onclick = () => openEditor(preset.name);
  const launch = el("button", "btn btn-small btn-go", "▶ 启动");
  launch.type = "button";
  launch.disabled = !canLaunch(preset);
  launch.onclick = () => openLaunch(preset);
  const archive = el("button", "btn btn-small", preset.tracked ? "归档" : "删掉");
  archive.type = "button";
  archive.onclick = () => openDanger({ kind: "archive", name: preset.name });
  foot.append(archive, edit, launch);
}

function sectionSummary(preset) {
  const sec = el("div", "sec");
  sec.append(el("h4", null, "一句话"));
  sec.append(el("p", null, preset.summary || "（没有摘要）"));
  return sec;
}

function sectionDescription(preset) {
  const text = preset.description;
  if (!text) return null; // 没有就不摆「暂无说明」那一行
  const sec = el("div", "sec");
  sec.append(el("h4", null, "说明"));
  const prose = el("div", "prose");
  prose.innerHTML = miniMarkdown(text);
  sec.append(prose);
  return sec;
}

/** 极小 markdown：标题 / 列表 / 行内 code / 粗体。够读 PRESET.md 这种说明书了。 */
function miniMarkdown(text) {
  const lines = String(text).split("\n");
  const out = [];
  let inList = false;
  const inline = (value) =>
    esc(value)
      .replace(/`([^`]+)`/g, "<code>$1</code>")
      .replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>");
  for (const line of lines) {
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      if (inList) out.push("</ul>");
      inList = false;
      out.push(`<h3>${inline(heading[2])}</h3>`);
      continue;
    }
    const item = /^\s*[-*]\s+(.*)$/.exec(line);
    if (item) {
      if (!inList) out.push("<ul>");
      inList = true;
      out.push(`<li>${inline(item[1])}</li>`);
      continue;
    }
    if (inList) out.push("</ul>");
    inList = false;
    if (!line.trim()) continue;
    out.push(`<p>${inline(line)}</p>`);
  }
  if (inList) out.push("</ul>");
  return out.join("");
}

function sectionCategories(preset) {
  const sec = el("div", "sec");
  sec.append(el("h4", null, "类别（这里能改）"));
  const picker = el("div", "cat-picker");
  const mine = new Set(preset.category ?? []);
  const options = sortCategories([...(state.categories ?? []), ...mine]);
  for (const category of options) {
    const chip = el("button", "cat-chip", category);
    chip.type = "button";
    chip.setAttribute("aria-pressed", String(mine.has(category)));
    chip.onclick = async () => {
      const next = new Set(mine);
      if (next.has(category)) next.delete(category);
      else next.add(category);
      await saveCategories(preset.name, [...next]);
    };
    picker.append(chip);
  }
  const add = el("button", "cat-chip add", "＋ 新类别");
  add.type = "button";
  add.onclick = async () => {
    const value = prompt("类别（可以写路径，如 写作/周报）");
    if (!value) return;
    await saveCategories(preset.name, [...new Set([...mine, value.trim()])].filter(Boolean));
  };
  picker.append(add);
  sec.append(picker);
  return sec;
}

async function saveCategories(name, category) {
  try {
    await api.post("v1/presets/category", { name, category });
    await refresh();
    toast("类别已保存", "ok");
  } catch (error) {
    toast(String(error?.message ?? error), "bad");
  }
}

function sectionRefs(preset, resolved) {
  const sec = el("div", "sec");
  sec.append(el("h4", null, "引用一览"));
  const list = el("div", "ref-list");

  const row = (label, state, text, href) => {
    const line = el("div", "ref");
    if (state) line.dataset.state = state;
    const left = el("span", "k dim", label);
    const value = el("span", null, text);
    line.append(left, value);
    if (href) {
      const anchor = el("a", "state", "去那边 ›");
      anchor.href = href;
      anchor.target = "_blank";
      anchor.rel = "noopener";
      line.append(anchor);
    } else {
      line.append(el("span", "state", state === "invalid" ? "失效" : ""));
    }
    list.append(line);
  };

  // 模型 → 模型块（k5411g）
  const modelsHref = siblingBase(api.ctx, "k5411g");
  const modelRow = resolved.model ?? {};
  row(
    "模型",
    modelRow.state === "invalid" ? "invalid" : "",
    modelRow.state === "auto" ? `自动 · ${modelRow.value ?? "设备默认"}` : modelRow.value ?? "?",
    modelRow.state === "invalid" ? modelsHref : null,
  );
  if (modelRow.thinkingLevels?.length) {
    row("可选档位", "", modelRow.thinkingLevels.join(" / "));
  }
  row("思考", resolved.thinking?.state === "invalid" ? "invalid" : "", resolved.thinking?.ref ?? "off（引擎缺省）");

  const skillsHref = siblingBase(api.ctx, "68kazk");
  if (resolved.absence?.skills) row("技能", "", "跟随默认（全开）");
  else if (!(resolved.skills ?? []).length) row("技能", "", "一个都不要");
  else for (const item of resolved.skills) row("技能", item.state, item.ref, item.state === "invalid" ? skillsHref : null);

  const extensionsHref = siblingBase(api.ctx, "vbe4pt");
  if (resolved.absence?.extensions) row("扩展", "", "跟随默认");
  else if (!(resolved.extensions ?? []).length) row("扩展", "", "一个都不要");
  else
    for (const item of resolved.extensions) {
      row("扩展", item.state, item.ref, item.state === "invalid" ? extensionsHref : null);
    }

  const promptsHref = siblingBase(api.ctx, "6ysg01");
  const templateRow = resolved.template ?? {};
  if (templateRow.mode === "default") row("模板", "", "跟随仓库默认");
  else if (templateRow.mode === "none") row("模板", "", "无（明确不要）");
  else row("模板", templateRow.state, templateRow.value ?? templateRow.ref, templateRow.state === "invalid" ? promptsHref : null);

  row("只读", "", resolved.readOnly ? "是" : "否");
  sec.append(list);
  return sec;
}

function sectionRepo(preset) {
  const sec = el("div", "sec");
  sec.append(el("h4", null, "仓库"));
  const dl = el("dl", "kv");
  const add = (key, value) => {
    dl.append(el("dt", null, key));
    dl.append(el("dd", null, value));
  };
  add("状态", preset.tracked ? "已发布（HEAD 里有这个目录）" : "未发布（HEAD 里没有）");
  add("目录", preset.dir ?? "—");
  const copy = el("button", "btn btn-small", "复制目录");
  copy.type = "button";
  copy.onclick = async () => {
    await copyText(preset.dir ?? "");
    toast("目录已复制", "ok");
  };
  const wrap = el("div");
  wrap.append(copy);
  dl.append(el("dt", null, ""));
  dl.append(wrap);
  sec.append(dl);
  return sec;
}

// ── 编辑器 ─────────────────────────────────────────────────────────────────

const editor = {
  name: null,
  files: [],
  path: "preset.json",
  text: "",
  base: null, // etag
  baseline: "", // HEAD 里那一版（diff 用）
  mode: "form",
  draft: null,
  dirty: false,
};

async function openEditor(name) {
  const preset = findPreset(name);
  if (!preset) return;
  if (preset.specError) {
    // 写坏了也能进来改 —— 那正是要进这儿的时候。
  }
  editor.name = name;
  editor.path = "preset.json";
  editor.mode = "form";
  editor.dirty = false;
  $("editor").hidden = false;
  $("ed-scrim").hidden = false;
  $("detail").inert = true;
  $("editor").focus?.();
  $("ed-name").textContent = name;
  $("ed-meta").textContent = "preset.json";
  setEdState("载入…");
  try {
    const file = await api.get(`v1/presets/file?name=${encodeURIComponent(name)}&path=preset.json`);
    editor.text = file.text;
    editor.base = file.etag;
    editor.baseline = await baselineOf(name, "preset.json");
    editor.draft = draftFromText(file.text);
    renderEditor();
  } catch (error) {
    setEdState("打不开");
    showEdMessage(String(error?.message ?? error), "bad");
  }
}

/** HEAD 里那一版（未发布的文件 `before` 是 null，等于整篇都是新增）。 */
async function baselineOf(name, rel) {
  try {
    const diff = await api.get(`v1/presets/pending/diff?path=${encodeURIComponent(`${name}/${rel}`)}`);
    return diff.before ?? "";
  } catch {
    return "";
  }
}

function closeEditor() {
  editor.name = null;
  editor.draft = null;
  editor.dirty = false;
  validateDebounce?.cancel();
  $("editor").hidden = true;
  $("ed-scrim").hidden = true;
  if (state.selected) $("detail").inert = false;
  showEdMessage("");
}

function renderEditor() {
  $("ed-tab-form").classList.toggle("on", editor.mode === "form");
  $("ed-tab-json").classList.toggle("on", editor.mode === "json");
  $("ed-tab-form").setAttribute("aria-selected", String(editor.mode === "form"));
  $("ed-tab-json").setAttribute("aria-selected", String(editor.mode === "json"));
  $("ed-form").hidden = editor.mode !== "form";
  $("ed-code").hidden = editor.mode !== "json";
  if (editor.mode === "form") renderForm();
  else renderJsonTab();
  setEdState(editor.dirty ? "未保存" : editor.base ? "已保存 · 与盘上一致" : "");
}

function setEdState(text, tone = "") {
  const node = $("ed-state");
  node.textContent = text;
  node.dataset.tone = tone;
}

function showEdMessage(text, tone = "") {
  const box = $("ed-msg");
  box.hidden = !text;
  box.textContent = text;
  box.dataset.tone = tone;
}

// ── 盘面 ───────────────────────────────────────────────────────────────────
//
// 这一屏是**盘面**，不是表单：四个域各一颗节点，点开各自的弹窗，中间那颗椭圆落子。
// 结构住在 board.js（版式照 hb7k3j），这里只负责三件事：喂数据、收改动、落盘。
//   · 数据只有一份 —— `editor.draft`。board.js 里不存副本，两张皮必分叉。
//   · 改一次 ⇒ markDirty()（顺带把 JSON 文本同步过去，两个 tab 编的是同一份东西）。
//   · 「启动」先落盘再开对话：拿没存过的东西开会话，等于让盘上的和屏幕上的不一样。

let board = null;

function ensureBoard() {
  if (board) return board;
  board = createBoard({
    root: $("board"),
    api,
    getDraft: () => editor.draft,
    getModels: () => models,
    getDeviceModel: () => deviceModel,
    getName: () => editor.name,
    isDirty: () => editor.dirty,
    onChange: () => markDirty(),
    onSave: () => saveEditor({ publish: false }),
    onLaunch: () => launchFromEditor(),
    onState: (text) => showEdMessage(text, "bad"),
  });
  void board.load();
  return board;
}

function renderForm() {
  const draft = editor.draft;
  if (!draft) return;
  if (draft.broken) {
    showEdMessage("preset.json 不是合法 JSON —— 去 JSON 那一栏修好，盘面才有东西可改。", "bad");
    return;
  }
  ensureBoard().render();
}

/** 启动：先落盘（有改动才写），再拿盘上那一版开会话。落盘失败就不启动。 */
async function launchFromEditor() {
  const name = editor.name;
  if (!name) return;
  const flash = $("b-flash");
  flash.textContent = "";
  flash.dataset.tone = "";
  if (editor.dirty) {
    await saveEditor({ publish: false });
    if (editor.dirty) {
      flash.textContent = "没落盘，先修上面那条";
      flash.dataset.tone = "bad";
      return;
    }
  }
  const preset = findPreset(name);
  if (!preset) {
    flash.textContent = "盘上找不到这条";
    flash.dataset.tone = "bad";
    return;
  }
  flash.textContent = "落盘了 · 在开对话";
  openLaunch(preset);
}

function markDirty() {
  editor.dirty = true;
  setEdState("未保存", "warn");
  // 盘面自己也要跟着变：框上的字、中央那行状态、只读那颗的开关，都读的是同一份草稿。
  board?.render();
  // 表单改了就把 JSON 文本同步过去 —— 两个 tab 编的是同一份东西。
  if (editor.mode === "form") syncTextFromForm();
}

function syncTextFromForm() {
  if (!editor.draft || editor.draft.broken) return;
  editor.text = JSON.stringify(specFromDraft(editor.draft, editor.name), null, 2) + "\n";
  const area = $("ed-text");
  if (area) area.value = editor.text;
}

function revalidate() {
  if (!validateDebounce) return;
  const draft = editor.draft;
  if (!draft || draft.broken) return;
  const spec = specFromDraft(draft, editor.name);
  validateDebounce.schedule(
    { spec },
    {
      onResult: (result) => {
        if (!result) return;
        if (result.errors?.length) showEdMessage(result.errors.join("；"), "bad");
        else if (result.warnings?.length) showEdMessage(result.warnings.join("；"), "warn");
        else showEdMessage("");
      },
      onError: (error) => showEdMessage(String(error?.message ?? error), "bad"),
    },
  );
}

// ── JSON tab ───────────────────────────────────────────────────────────────

function renderJsonTab() {
  const area = $("ed-text");
  area.disabled = false;
  area.value = editor.text;
  area.oninput = () => {
    editor.text = area.value;
    editor.dirty = true;
    setEdState("未保存", "warn");
    drawGutter();
    const draft = draftFromText(area.value);
    if (!draft.broken) {
      editor.draft = draft;
      revalidate();
    } else {
      showEdMessage("不是合法 JSON（服务端也会这么答）", "bad");
    }
  };
  area.onkeydown = (event) => {
    if (event.key === "Tab") {
      event.preventDefault();
      const { selectionStart: start, selectionEnd: end } = area;
      area.value = `${area.value.slice(0, start)}  ${area.value.slice(end)}`;
      area.selectionStart = area.selectionEnd = start + 2;
      area.oninput();
    }
  };
  drawGutter();
}

function drawGutter() {
  const area = $("ed-text");
  const text = String(area.value);
  const lineCount = text.split("\n").length;
  const diff = buildDiff(editor.baseline ?? "", text);
  const gutter = $("ed-gutter");
  const rows = [];
  for (let index = 1; index <= lineCount; index += 1) {
    const gap = diff.gaps.get(index);
    const mark = diff.marks.get(index);
    const flag = gap ? `▾${gap}` : mark === "add" || mark === "mod" ? "▏" : " ";
    const color = gap ? "var(--bad)" : mark ? "var(--warn)" : "var(--dim)";
    rows.push(`<span style="color:${color}">${flag}</span>${index}`);
  }
  gutter.innerHTML = rows.join("\n");
  // 行高必须两侧一致（20px）：靠 scrollTop 同步，差 1px 就会越滚越偏。
  area.onscroll = () => {
    gutter.scrollTop = area.scrollTop;
  };
}

async function saveEditor({ publish = false } = {}) {
  if (!editor.name) return;
  if (editor.mode === "form") syncTextFromForm();
  const name = editor.name;
  try {
    setEdState("保存中…");
    // 保存 = 只落盘（带 etag，撞了就 409）。发布 = 另一条路（闸 + 重扫 + commit + push）。
    const saved = await api.put("v1/presets/file", {
      name,
      path: "preset.json",
      text: editor.text,
      base: editor.base ?? undefined,
    });
    editor.base = saved.etag;
    editor.dirty = false;
    editor.baseline = await baselineOf(name, "preset.json");
    setEdState("已保存 · 未提交");
    showEdMessage("已保存到磁盘（还没提交）", "ok");
    board?.render();
    if (publish) await publishFromEditor(name);
    await refresh();
    board?.render();
  } catch (error) {
    if (error instanceof ConflictError || error?.conflict) {
      setEdState("冲突", "bad");
      showEdMessage(CONFLICT_MESSAGE, "bad");
      return;
    }
    setEdState("保存失败", "bad");
    showEdMessage(String(error?.message ?? error), "bad");
  }
}

async function publishFromEditor(name) {
  try {
    setEdState("发布中…");
    const result = await api.post("v1/presets/publish", {});
    const file = await api.get(`v1/presets/file?name=${encodeURIComponent(name)}&path=preset.json`);
    editor.base = file.etag;
    editor.baseline = await baselineOf(name, "preset.json");
    const line = result.pushed === false ? result.error ?? "推送失败" : result.pushed ? "已提交并推送" : "已提交（没有远端）";
    setEdState("已发布", "ok");
    showEdMessage(line, result.pushed === false ? "bad" : "ok");
    for (const warning of result.warnings ?? []) toast(warning, "bad");
    await refresh();
  } catch (error) {
    setEdState("发布失败", "bad");
    showEdMessage(String(error?.message ?? error), "bad");
  }
}

// ── 启动 ───────────────────────────────────────────────────────────────────

let launchTarget = null;

function openLaunch(preset) {
  if (!canLaunch(preset)) return;
  launchTarget = preset;
  const warnings = launchWarnings(preset);
  $("launch-title").textContent = `开一条新对话 · ${preset.name}`;
  $("launch-hint").textContent = preset.summary || "";
  $("launch-name").value = defaultSessionName(preset.name);
  $("launch-err").hidden = true;
  $("launch-ok").disabled = false;
  $("launch-ok").textContent = "开对话";

  const body = $("launch-body");
  body.innerHTML = "";
  const dl = el("dl", "kv");
  const add = (key, value) => {
    dl.append(el("dt", null, key));
    dl.append(el("dd", null, value));
  };
  const resolved = preset.resolved ?? {};
  add("预设", preset.name);
  add("模型", resolved.model?.state === "auto" ? `自动 · ${resolved.model.value ?? "设备默认"}` : resolved.model?.value ?? "?");
  add("思考", resolved.thinking?.ref ?? "off（缺省）");
  add("技能", resolved.absence?.skills ? "跟随默认" : (resolved.skills ?? []).length ? (resolved.skills ?? []).map((row) => row.ref).join("、") : "一个都不要");
  add("扩展", resolved.absence?.extensions ? "跟随默认" : (resolved.extensions ?? []).length ? (resolved.extensions ?? []).map((row) => row.ref).join("、") : "一个都不要");
  add(
    "模板",
    resolved.template?.mode === "default" ? "跟随仓库默认" : resolved.template?.mode === "none" ? "无" : resolved.template?.value ?? "?",
  );
  add("只读", resolved.readOnly ? "是" : "否");
  body.append(dl);

  const missing = [
    ...(warnings.missingSkills.length ? [`技能：${warnings.missingSkills.join("、")}`] : []),
    ...(warnings.missingExtensions.length ? [`扩展：${warnings.missingExtensions.join("、")}`] : []),
    ...(warnings.missingTemplate ? [`模板：${warnings.missingTemplate}`] : []),
  ];
  if (missing.length) body.append(el("p", "warn-line", `这些没装，会被跳过 —— ${missing.join("；")}`));
  if (warnings.notes.length) body.append(el("p", "note-line", warnings.notes.join(" · ")));

  $("launch").hidden = false;
  $("launch-scrim").hidden = false;
  $("launch-name").focus();
  $("launch-name").select();
}

function closeLaunch() {
  launchTarget = null;
  $("launch").hidden = true;
  $("launch-scrim").hidden = true;
}

async function confirmLaunch() {
  if (!launchTarget) return;
  const preset = launchTarget;
  const button = $("launch-ok");
  button.disabled = true;
  button.textContent = "开…";
  try {
    const created = await launchPreset(api, {
      name: preset.name,
      sessionName: $("launch-name").value.trim(),
      open: (url) => window.open(url, "_blank", "noopener"),
    });
    const skipped = [
      ...(created.missing?.length ? [`技能 ${created.missing.join("、")}`] : []),
      ...(created.missingExtensions?.length ? [`扩展 ${created.missingExtensions.join("、")}`] : []),
      ...(created.missingTemplates?.length ? [`模板 ${created.missingTemplates.join("、")}`] : []),
    ];
    // 认服务端回来的**生效值**：它夹过一遍，面板该跟它走。
    const line = `已开「${created.name ?? preset.name}」· ${created.model ?? "?"} · 模板 ${created.template ?? "无"}`;
    toast(skipped.length ? `${line}（跳过：${skipped.join("；")}）` : line, skipped.length ? "bad" : "ok");
    closeLaunch();
  } catch (error) {
    $("launch-err").hidden = false;
    $("launch-err").textContent = String(error?.message ?? error);
    button.disabled = false;
    button.textContent = "开对话";
  }
}

// ── 新建 ───────────────────────────────────────────────────────────────────

function openCreate() {
  $("create").reset();
  $("create-err").hidden = true;
  $("create").hidden = false;
  $("create-scrim").hidden = false;
  $("create-name").focus();
}

function closeCreate() {
  $("create").hidden = true;
  $("create-scrim").hidden = true;
}

async function submitCreate(event) {
  event.preventDefault();
  const name = $("create-name").value.trim();
  const summary = $("create-summary").value.trim();
  const category = $("create-cat")
    .value.split(",")
    .map((item) => item.trim())
    .filter(Boolean);
  const from = $("create-from").value;
  const button = $("create-ok");
  button.disabled = true;
  try {
    await api.post("v1/presets/create", { name, summary, category, from });
    closeCreate();
    await refresh();
    toast(`建好了 ${name}（未发布）`, "ok");
    openEditor(name);
  } catch (error) {
    $("create-err").hidden = false;
    $("create-err").textContent = String(error?.message ?? error);
  } finally {
    button.disabled = false;
  }
}

// ── 确认框（手输名字） ─────────────────────────────────────────────────────

let dangerTask = null;

/**
 * `kind` 决定文案和动作：
 *   archive —— 移进 .archive/，能恢复（未发布的其实等于删掉，因为它从没进过仓库）
 *   purge   —— 真删（只有这一条会真删）
 * 两者都要**手输名字**才亮按钮。归档/删名字是那两件事里唯一不可预期的部分。
 */
function openDanger({ kind, name }) {
  dangerTask = { kind, name };
  const archived = kind === "purge";
  $("danger-title").textContent = archived ? "彻底删除" : "归档";
  $("danger-hint").textContent = archived
    ? `把 .archive/${name}/ 整棵删掉。git 历史还留着，但盘上再也找不回来了。`
    : `搬进 .archive/${name}/。能从「已归档」里恢复；它会从列表里消失，会话里再选它就没了。`;
  $("danger-name").textContent = name;
  $("danger-word").textContent = "预设名";
  $("danger-input").value = "";
  $("danger-ok").disabled = true;
  $("danger-err").hidden = true;
  $("danger").hidden = false;
  $("danger-scrim").hidden = false;
  $("danger-input").focus();
}

function closeDanger() {
  dangerTask = null;
  $("danger").hidden = true;
  $("danger-scrim").hidden = true;
}

async function confirmDanger() {
  if (!dangerTask) return;
  const { kind, name } = dangerTask;
  const button = $("danger-ok");
  button.disabled = true;
  try {
    const result = await api.post(kind === "purge" ? "v1/presets/purge" : "v1/presets/archive", { name });
    closeDanger();
    if (state.selected === name) closeDetail();
    if (editor.name === name) closeEditor();
    await refresh();
    toast(result?.pushed === false ? `做完但没推成：${result.error ?? ""}` : `已${kind === "purge" ? "彻底删除" : "归档"} ${name}`, result?.pushed === false ? "bad" : "ok");
  } catch (error) {
    $("danger-err").hidden = false;
    $("danger-err").textContent = String(error?.message ?? error);
    button.disabled = false;
  }
}

async function runAction(fn, okText) {
  try {
    const result = await fn();
    await refresh();
    toast(result?.pushed === false ? `${okText}，但没推成：${result.error ?? ""}` : okText, result?.pushed === false ? "bad" : "ok");
  } catch (error) {
    toast(String(error?.message ?? error), "bad");
  }
}

// ── 菜单 ───────────────────────────────────────────────────────────────────

const menu = { node: null, close: null };

function mountMenu(x, y, items, header) {
  closeMenu();
  const node = $("ctxmenu");
  node.innerHTML = "";
  if (header) node.append(el("div", "menu-head", header));
  for (const item of items) {
    if (item === "-") {
      node.append(el("div", "menu-sep"));
      continue;
    }
    const button = el("button", "menu-item");
    button.type = "button";
    button.setAttribute("role", "menuitem");
    const label = el("span", null, item.label);
    button.append(label);
    if (item.tail) button.append(el("span", "tail", item.tail));
    if (item.tone) button.dataset.tone = item.tone;
    if (item.disabled) button.disabled = true;
    button.onclick = () => {
      if (item.disabled) return;
      closeMenu();
      item.run();
    };
    node.append(button);
  }
  node.hidden = false;
  // 视口内夹紧：菜单贴着手指弹出来，跑到屏幕外等于点了没反应。
  const rect = node.getBoundingClientRect();
  node.style.left = `${Math.min(x, window.innerWidth - rect.width - 8)}px`;
  node.style.top = `${Math.min(y, window.innerHeight - rect.height - 8)}px`;
  menu.node = node;

  const onDown = (event) => {
    if (node.contains(event.target)) return;
    closeMenu();
  };
  const onKey = (event) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      closeMenu();
    }
  };
  const onScroll = () => closeMenu();
  setTimeout(() => {
    document.addEventListener("pointerdown", onDown, true);
    document.addEventListener("keydown", onKey, true);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onScroll);
  }, 0);
  menu.close = () => {
    document.removeEventListener("pointerdown", onDown, true);
    document.removeEventListener("keydown", onKey, true);
    window.removeEventListener("scroll", onScroll, true);
    window.removeEventListener("resize", onScroll);
  };
}

function closeMenu() {
  menu.close?.();
  menu.close = null;
  menu.node = null;
  $("ctxmenu").hidden = true;
}

function openCardMenu(x, y, preset) {
  const status = cardState(preset);
  const items = [
    { label: "开对话", tail: "新标签页", disabled: !canLaunch(preset), run: () => openLaunch(preset) },
    { label: "看详情", tail: "", run: () => openDetail(preset.name) },
    { label: "编辑 preset.json", tail: "改引用", run: () => openEditor(preset.name) },
    "-",
    {
      label: "发布",
      tail: preset.tracked ? "只发这条" : "首次进仓库",
      run: () => runAction(() => api.post("v1/presets/publish", {}), `已发布 ${preset.name}`),
    },
    {
      label: preset.tracked ? "归档" : "删掉",
      tail: preset.tracked ? "可恢复" : "没进过仓库",
      tone: "bad",
      run: () => openDanger({ kind: "archive", name: preset.name }),
    },
  ];
  if (status === "failed") items.unshift({ label: "重读磁盘", tail: "先看看改对了没", run: () => runAction(() => api.post("v1/presets/reload", {}), "已重扫") });
  mountMenu(x, y, items, preset.name);
}

function openCategoryMenu(x, y, path) {
  const impact = categoryImpact(state.presets, path);
  mountMenu(
    x,
    y,
    [
      {
        label: "新建子类别",
        tail: `${path}/…`,
        run: () => startFacetInput({ mode: "add", prefix: `${path}/` }),
      },
      { label: "编辑名称（整枝搬家）", tail: `${impact.presets} 条`, run: () => startFacetInput({ mode: "rename", path }) },
      "-",
      {
        label: "删除类别（整枝）",
        tail: impact.children ? `含 ${impact.children} 个子级` : "空类别",
        tone: "bad",
        run: () => removeCategory(path),
      },
    ],
    path,
  );
}

/**
 * 左栏那个原地输入框，两种用法：
 *   `startFacetInput({ mode: "add", prefix: "写作/" })` —— 建子类别，前缀预填
 *   `startFacetInput({ mode: "rename", path: "写作/周报" })` —— 改名字（填**全路径**，改前缀 = 搬家）
 *
 * 落盘只有两个动作（`op:add` / `op:rename`），判定全在服务端。这里只负责「回车提交 / Esc 反悔 /
 * 失焦也提交」和「改完把筛选跟着搬过去」。
 */
function startFacetInput({ mode, path = "", prefix = "" } = {}) {
  const box = $("rail-new");
  box.innerHTML = "";
  const input = el("input", "facet-input");
  input.value = mode === "rename" ? path : prefix;
  input.placeholder = mode === "rename" ? "全路径，如 写作/周报" : "新类别（可写路径）";
  let done = false;
  const commit = async (save) => {
    if (done) return;
    done = true;
    const value = input.value.trim();
    if (!save || !value) return renderRail();
    try {
      if (mode === "rename") {
        if (value === path) return renderRail();
        await api.post("v1/presets/categories", { op: "rename", name: path, to: value });
        // 改的是前缀 = 搬家：正在筛这一支的话，筛选跟着搬过去。
        if (state.category && isUnder(state.category, path)) {
          state.category = value + state.category.slice(path.length);
        }
      } else {
        await api.post("v1/presets/categories", { op: "add", name: value });
      }
      await refresh();
    } catch (error) {
      toast(String(error?.message ?? error), "bad");
      renderRail();
    }
  };
  input.onkeydown = (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      void commit(true);
    }
    if (event.key === "Escape") {
      event.stopPropagation();
      void commit(false);
    }
  };
  input.onblur = () => void commit(true);
  box.append(input);
  // 先挂进文档再 focus —— 行还在内存里时聚焦会落空（踩过）。
  input.focus();
  input.select();
}

async function removeCategory(path) {
  const impact = categoryImpact(state.presets, path);
  const label = impact.children ? `删「${path}」连同 ${impact.children} 个子类别` : `删「${path}」`;
  if (!window.confirm(`${label}？\n${impact.presets} 条预设会失去这个类别（预设本身不动）。`)) return;
  try {
    await api.post("v1/presets/categories", { op: "remove", name: path });
    if (state.category && isUnder(state.category, path)) state.category = null;
    await refresh();
    toast(`已删除类别 ${path}`, "ok");
  } catch (error) {
    toast(String(error?.message ?? error), "bad");
  }
}

/** 长按 520ms 出菜单；挪动超过 10px 当滚动；松手补的那一下 click 必须吞掉。 */
function attachMenuGesture(node, open) {
  let timer = null;
  let start = null;
  let swallow = false;
  const clear = () => {
    if (timer) clearTimeout(timer);
    timer = null;
    start = null;
  };
  node.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    open(event);
  });
  node.addEventListener("pointerdown", (event) => {
    if (event.pointerType === "mouse") return;
    start = { x: event.clientX, y: event.clientY };
    swallow = false;
    timer = setTimeout(() => {
      swallow = true;
      open(event);
    }, 520);
  });
  node.addEventListener("pointermove", (event) => {
    if (!start || !timer) return;
    if (Math.abs(event.clientX - start.x) > 10 || Math.abs(event.clientY - start.y) > 10) clear();
  });
  node.addEventListener("pointerup", (event) => {
    clear();
    if (swallow) {
      // 松手浏览器会补一个 `click` —— 菜单就在手指底下，不吞掉就是「松手即误触」。
      // `preventDefault()` 拦不住 click，只能自己拦一次 capture。
      event.preventDefault();
      swallow = false;
      const swallowClick = (clickEvent) => {
        clickEvent.stopPropagation();
        clickEvent.preventDefault();
      };
      node.addEventListener("click", swallowClick, { capture: true, once: true });
      setTimeout(() => node.removeEventListener("click", swallowClick, { capture: true }), 0);
    }
  });
  node.addEventListener("pointercancel", clear);
}

// ── Esc 的层级 ────────────────────────────────────────────────────────────

function escLayer() {
  if (menu.node) return closeMenu();
  if (!$("launch").hidden) return closeLaunch();
  if (!$("create").hidden) return closeCreate();
  if (!$("danger").hidden) return closeDanger();
  if (!$("editor").hidden) return closeEditor();
  if (!$("detail").hidden) return closeDetail();
  if (state.category) {
    state.category = null;
    return render();
  }
  if (state.query) {
    state.query = "";
    $("search").value = "";
    return render();
  }
  if (state.view !== "all") {
    state.view = "all";
    return render();
  }
  closeMenu();
}

// ── 事件绑定 ───────────────────────────────────────────────────────────────

function bind() {
  $("refresh").onclick = () => void refresh();
  $("sync").onclick = () => void refresh();
  $("new-preset").onclick = () => openCreate();
  $("new-cat").onclick = () => startFacetInput({ mode: "add" });
  $("create").onsubmit = (event) => void submitCreate(event);
  $("create-cancel").onclick = () => closeCreate();
  $("create-scrim").onclick = () => closeCreate();
  $("launch-cancel").onclick = () => closeLaunch();
  $("launch-scrim").onclick = () => closeLaunch();
  $("launch-ok").onclick = () => void confirmLaunch();
  $("danger-cancel").onclick = () => closeDanger();
  $("danger-scrim").onclick = () => closeDanger();
  $("danger-ok").onclick = () => void confirmDanger();
  $("danger-input").oninput = () => {
    $("danger-ok").disabled = $("danger-input").value.trim() !== dangerTask?.name;
  };
  $("dt-close").onclick = () => closeDetail();
  $("dt-scrim").onclick = () => closeDetail();
  $("ed-close").onclick = () => closeEditor();
  $("ed-scrim").onclick = () => closeEditor();
  $("ed-publish").onclick = () => void saveEditor({ publish: true });
  $("ed-tab-form").onclick = () => {
    // 表单 ← JSON：只在 JSON 能解析时回填（非法 JSON 上回填等于猜）。
    if (editor.mode === "json") {
      const draft = draftFromText(editor.text);
      if (draft.broken) {
        showEdMessage("JSON 不是合法的，先修好再回表单（表单不猜你想写什么）", "bad");
        return;
      }
      editor.draft = draft;
    }
    editor.mode = "form";
    renderEditor();
  };
  $("ed-tab-json").onclick = () => {
    if (editor.mode === "form") syncTextFromForm();
    editor.mode = "json";
    renderEditor();
  };

  // 搜索：输入即筛，但不动 `state.view` / `state.category`（筛选是视图，不是数据）。
  let searchTimer = null;
  $("search").oninput = () => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      state.query = $("search").value;
      render();
    }, 90);
  };

  document.addEventListener("keydown", (event) => {
    // ⌘/Ctrl+S 保存；Alt+D 切表单 / JSON。
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
      if (!$("editor").hidden) {
        event.preventDefault();
        void saveEditor({ publish: false });
      }
      return;
    }
    if (event.altKey && event.key.toLowerCase() === "d") {
      if (!$("editor").hidden) {
        event.preventDefault();
        (editor.mode === "form" ? $("ed-tab-json") : $("ed-tab-form")).click();
      }
      return;
    }
    if (event.key === "Escape") {
      const open = ["#launch", "#create", "#danger", "#editor", "#detail"].some((id) => !$(id.slice(1)).hidden) || menu.node;
      if (open || state.category || state.query || state.view !== "all") {
        event.preventDefault();
        escLayer();
      }
      return;
    }
    // 网格里的键盘导航：焦点是「格」，不是项。
    if ($("detail").hidden && $("editor").hidden && !menu.node && document.activeElement?.tagName !== "INPUT") {
      const columns = columnsOf();
      if (event.key === "ArrowDown") {
        event.preventDefault();
        state.kb = { on: true, row: state.kb.row + 1, col: state.kb.col };
        paintKb();
      } else if (event.key === "ArrowUp") {
        event.preventDefault();
        state.kb = { on: true, row: Math.max(0, state.kb.row - 1), col: state.kb.col };
        paintKb();
      } else if (event.key === "ArrowRight") {
        event.preventDefault();
        state.kb = { on: true, row: state.kb.row, col: Math.min(columns - 1, state.kb.col + 1) };
        paintKb();
      } else if (event.key === "ArrowLeft") {
        event.preventDefault();
        state.kb = { on: true, row: state.kb.row, col: Math.max(0, state.kb.col - 1) };
        paintKb();
      } else if (event.key === "Enter" && state.kb.on) {
        const card = $("grid").querySelector(".card[data-kb]");
        if (card) {
          event.preventDefault();
          openDetail(card.dataset.name);
        }
      } else if (event.key === "n" && !event.metaKey && !event.ctrlKey) {
        event.preventDefault();
        openCreate();
      } else if (event.key === "/") {
        event.preventDefault();
        $("search").focus();
      }
    }
  });

  window.addEventListener("resize", () => paintKb());
}

bind();
void boot();


// skills · Nous 自持技能的清单面板。数据全部来自 qrd3wh 的 `GET /v1/skills`（spec §6.2）。
//
// 块之间只走 HTTP，地址只从宿主的 `__nous/ctx` 来 —— 这里不拼任何绝对路径。
// 类别 / 正文 / 发布 / 归档全在 qrd3wh 那一头：这一页只发 POST，不读仓库、不跑 git（I4）。
//
// 取数和写回全在 api.js（`createApi` / ctx 解析 / 类别 debounce / 发布后重读），
// 纯计算在 model.js，DOM 在这儿。三层里只有这一层碰 document，所以
// `node --test` 拿假 fetch 测的是 api.js —— UI 归合并后的 code-review 看（spec §9）。

import { buildDiff } from "./diff.js";
import {
  ARCHIVED,
  STATE_LABEL,
  UNCATEGORIZED,
  buildView,
  categoriesOf,
  categoryImpact,
  childCount,
  compareCategories,
  depthOf,
  filterCards,
  isUnder,
  leafOf,
  normalizeCategory,
  stateFilter,
  stateFilterOf,
} from "./model.js";
import { CONFLICT_MESSAGE, createApi, createCategoryDebounce, publishWithRefresh } from "./api.js";

const SELF = location.pathname.endsWith("/") ? location.pathname : location.pathname + "/";
const $ = (id) => document.getElementById(id);
const $sel = (selector) => document.querySelector(selector);

const el = (tag, className, text) => {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
};

const short = (sha) => (sha ? sha.slice(0, 7) : "—");

let toastTimer;
function toast(message, kind = "") {
  const node = $("toast");
  node.textContent = message;
  node.dataset.kind = kind;
  node.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (node.hidden = true), 3600);
}

/**
 * 「提交了但没推成」那一行说什么。
 *
 * 原因在服务端的 `error` 里（`pushed: false` + `error`，见 spec §6.3 第 6 条）；
 * `warnings` 是另一类东西（「未发布的不跟着走」「已提交但装载失败」）。早先只读
 * warnings，于是断网的时候页面上写着「未知原因」—— 真正的理由就在响应体里躺着。
 */
const pushFailure = (result) =>
  result?.error || (Array.isArray(result?.warnings) ? result.warnings.filter(Boolean).join("；") : "") || "未知原因";

// 商品砖：字母砖的色相由名字决定，同一条技能永远同一个颜色。
const hueOf = (name) => {
  let hash = 0;
  for (let i = 0; i < name.length; i++) hash = (hash * 31 + name.charCodeAt(i)) % 360;
  return hash;
};
const initialsOf = (name) => {
  const clean = String(name).replace(/[^a-z0-9]+/gi, " ").trim();
  if (!clean) return "?";
  const parts = clean.split(/\s+/);
  if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
  return clean.slice(0, 2).toUpperCase();
};

// ── 状态 ───────────────────────────────────────────────────────────────────

const state = {
  data: null, // GET /v1/skills 的原样
  view: null, // buildView(data) 的结果（唯一的派生层）
  filter: "", // "" = 全部；UNCATEGORIZED；ARCHIVED；"__state__:<state>"；其它 = 类别路径
  query: "",
  creating: false, // 左栏「新建类别」的输入框开着
  createPrefix: "", // 「新建子类别」预填的 `工具/`
  renaming: null, // 正在原地改名的类别
  editing: null, // 正在输入类别的技能名（只住在详情里）
  detail: null, // 详情视图里是哪条技能；null = 列表
  detailBody: null, // 详情里那份 SKILL.md：{name, loading, text, error}
  editor: null, // 正文抽屉
  publishing: false,
};

const skillNow = (name) => (state.data?.skills ?? []).find((item) => item.name === name) ?? null;
const categoryNames = () => state.view?.categoryNames ?? [];
const catCount = (path) => state.view?.counts.get(path) ?? 0;

/** 数据变了就重算派生层 —— 页面各处都读 state.view，别各算一遍。 */
function refreshView() {
  state.view = buildView(state.data);
  // 筛着的东西没了就落回「全部」：别让人盯着一面空墙以为是仓库空了。
  const value = state.filter;
  if (!value) return;
  if (value === UNCATEGORIZED && !state.view.uncategorized) state.filter = "";
  else if (value === ARCHIVED && !state.view.archived.length) state.filter = "";
  else if (stateFilterOf(value) && !state.view.cards.some((card) => card.state === stateFilterOf(value))) state.filter = "";
  else if (!stateFilterOf(value) && value !== UNCATEGORIZED && value !== ARCHIVED && !state.view.categoryNames.includes(value)) {
    state.filter = "";
  }
}

// ── 取数 ───────────────────────────────────────────────────────────────────

let transport = null;

/** 地址只从宿主的 `__nous/ctx` 里找 qrd3wh，别的一律不拼（I5）。 */
async function connect() {
  const api = createApi({ self: SELF });
  await api.connect();
  transport = api;
}

// 类别改动 8 秒攒一次再落盘（票 7）：连点三个 chip 只写一次。
const categoryWrites = createCategoryDebounce({
  post: (path, body) => transport.post(path, body),
  onSent: () => void load({ quiet: true }),
  onError: (error) => {
    toast(`改类别失败：${error.message}`, "bad");
    // 失败的乐观更新得撤回 —— 重读一次，服务端说是什么就是什么。
    void load({ quiet: true });
  },
});

async function load({ quiet = false } = {}) {
  const dot = $sel("#refresh .dot");
  if (!quiet) dot.dataset.state = "wait";
  try {
    if (!transport) await connect();
    state.data = await transport.get("v1/skills");
    refreshView();
    render();
    dot.dataset.state = "ok";
  } catch (error) {
    dot.dataset.state = "bad";
    $("count").textContent = "拿不到";
    $("result-count").textContent = "";
    $("grid").textContent = "";
    const box = el("div", "empty", String(error?.message ?? error));
    const retry = el("button", "btn btn-small", "重试");
    retry.type = "button";
    retry.onclick = () => void load();
    box.append(el("br"), retry);
    $("grid").append(box);
    if (!quiet) toast(String(error?.message ?? error), "bad");
  }
}

// ── 类别（写回仓库，qrd3wh 落盘并提交） ────────────────────────────────────
//
// 类别不在这一页存：真相是每个 `nous.json` 的 category + `categories.json` 的词表，
// 两份取并集（spec §3）。所以这一页只有一块 8 秒的写入队列（categoryWrites）：
// 先乐观地画出来，攒够 8 秒发一次；离开详情页 / 发布前立刻 flush。

function saveCategories(name, next) {
  const skill = skillNow(name);
  if (!skill || !transport) return;
  const wanted = [...new Set(next.map(normalizeCategory).filter(Boolean))].sort(compareCategories);
  skill.category = wanted;
  refreshView();
  render();
  categoryWrites.schedule(name, wanted);
}

function addTag(name, value) {
  const skill = skillNow(name);
  if (!skill) return;
  const tag = normalizeCategory(value);
  if (!tag) return;
  const set = new Set(categoriesOf(skill));
  if (set.has(tag)) return;
  saveCategories(name, [...set, tag]);
}

function removeTag(name, value) {
  const skill = skillNow(name);
  if (!skill) return;
  saveCategories(
    name,
    categoriesOf(skill).filter((tag) => tag !== value),
  );
}

async function addCategory(value) {
  const name = normalizeCategory(value);
  if (!name || !transport) return;
  try {
    await transport.post("v1/skills/categories", { op: "add", name });
    await load({ quiet: true });
  } catch (error) {
    toast(`新建类别失败：${error.message}`, "bad");
  }
}

async function renameCategory(name, next) {
  const to = normalizeCategory(next);
  if (!to || !transport) return;
  try {
    await transport.post("v1/skills/categories", { op: "rename", name, to });
    // 筛选跟着走：正在筛「A/x」时把 A 改成 B，筛的应该变成「B/x」。
    if (state.filter === name) state.filter = to;
    else if (state.filter.startsWith(name + "/")) state.filter = to + state.filter.slice(name.length);
    await load({ quiet: true });
    toast(`类别已改名为 ${to}`);
  } catch (error) {
    toast(`改名失败：${error.message}`, "bad");
  }
}

async function removeCategory(name) {
  if (!transport) return;
  const names = categoryNames();
  const subs = childCount(names, name);
  const count = categoryImpact(state.data?.skills ?? [], name);
  const scope = `类别「${name}」${subs ? `连同 ${subs} 个子类别` : ""}`;
  const message = count ? `删除${scope}？会从 ${count} 条技能上摘掉。` : `删除${scope}？（空类别）`;
  if (!confirm(message)) return;
  try {
    await transport.post("v1/skills/categories", { op: "remove", name });
    if (isUnder(state.filter, name)) state.filter = "";
    await load({ quiet: true });
    toast(`已删除类别 ${name}`);
  } catch (error) {
    toast(`删除类别失败：${error.message}`, "bad");
  }
}

// ── 右键 / 长按菜单 ────────────────────────────────────────────────────────
//
// 左栏类别行和技能卡共用这一个菜单件：可见的 `⋯` 是主入口，桌面右键 / 手机长按 /
// 键盘 ContextMenu 只是快捷方式（只藏右键里，第一次进来的人永远找不到）。

const LONG_PRESS_MS = 520;
const LONG_PRESS_SLOP = 10;

let menuOpen = false;
let menuTeardown = null;
let swallowTimer = null;

function closeMenu() {
  if (!menuOpen) return;
  menuOpen = false;
  const menu = $("ctxmenu");
  menu.hidden = true;
  menu.textContent = "";
  if (menuTeardown) {
    menuTeardown();
    menuTeardown = null;
  }
}

// 菜单件的公共骨架：贴边夹紧、点外面 / Esc / 滚动 / 改窗口大小都关。
function mountMenu(menu, x, y, focusTarget) {
  // 先贴左上角量一遍，再摆到点击处 —— 右键点在屏幕边上不能被切掉。
  menu.style.left = "0px";
  menu.style.top = "0px";
  const rect = menu.getBoundingClientRect();
  const pad = 8;
  const left = Math.max(pad, Math.min(x, innerWidth - rect.width - pad));
  const top = Math.max(pad, Math.min(y, innerHeight - rect.height - pad));
  menu.style.left = `${Math.round(left)}px`;
  menu.style.top = `${Math.round(top)}px`;

  focusTarget?.focus({ preventScroll: true });

  // 下一拍才挂「点外面关掉」：这一下 pointerdown 还在派发，挂早了会被自己关掉。
  let armed = false;
  const arm = setTimeout(() => (armed = true), 0);
  const onPointerDown = (event) => {
    if (armed && !menu.contains(event.target)) closeMenu();
  };
  const onKey = (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      closeMenu();
    }
  };
  const onViewport = () => closeMenu();

  addEventListener("pointerdown", onPointerDown, true);
  addEventListener("keydown", onKey);
  addEventListener("scroll", onViewport, true);
  addEventListener("resize", onViewport);
  menuTeardown = () => {
    clearTimeout(arm);
    removeEventListener("pointerdown", onPointerDown, true);
    removeEventListener("keydown", onKey);
    removeEventListener("scroll", onViewport, true);
    removeEventListener("resize", onViewport);
  };
}

function menuHead(menu, title, badge) {
  const head = el("div", "ctx-head");
  head.append(el("span", "ctx-head-name", title));
  if (badge) head.append(el("span", "ctx-badge", badge));
  menu.append(head);
}

function menuItem(menu, label, hint, run, { danger = false, disabled = false, title = "" } = {}) {
  const button = el("button", "ctx-item" + (danger ? " ctx-danger" : ""));
  button.type = "button";
  button.setAttribute("role", "menuitem");
  button.append(el("span", "ctx-item-label", label));
  if (hint) button.append(el("span", "ctx-item-hint", hint));
  button.disabled = disabled;
  if (title) button.title = title;
  button.onclick = () => {
    if (disabled) return;
    closeMenu();
    run?.();
  };
  menu.append(button);
  return button;
}

function openCategoryMenu(x, y, path) {
  closeMenu();
  const menu = $("ctxmenu");
  menu.textContent = "";
  menu.hidden = false;
  menuOpen = true;

  menuHead(menu, path, `${catCount(path)} 条`);

  const createSub = menuItem(menu, "新建子类别", "在它下面", () => {
    state.creating = true;
    state.createPrefix = path + "/";
    state.renaming = null;
    render();
  });

  menuItem(menu, "编辑类别名称", "填全路径", () => {
    state.renaming = path;
    state.creating = false;
    render();
  });

  menu.append(el("div", "ctx-sep"));
  menuItem(menu, "删除类别", catCount(path) ? `${categoryImpact(state.data?.skills ?? [], path)} 条技能` : "空类别", () => {
    void removeCategory(path);
  }, { danger: true });

  mountMenu(menu, x, y, createSub);
}

function openCardMenu(x, y, skill) {
  closeMenu();
  const menu = $("ctxmenu");
  menu.textContent = "";
  menu.hidden = false;
  menuOpen = true;

  const stateName = state.view?.cards.find((card) => card.name === skill.name)?.state ?? "ok";
  menuHead(menu, skill.name, STATE_LABEL[stateName] ?? stateName);

  const first = menuItem(menu, "查看详情", "类别 · 正文", () => openDetail(skill.name));
  menuItem(menu, "看正文", "读 / 改 SKILL.md", () => void openEditor(skill));
  menuItem(menu, "类别", "在详情里勾", () => openDetail(skill.name, { editTags: true }));

  menu.append(el("div", "ctx-sep"));
  // 「未发布」也走归档：盘上删掉的东西一律进 `.archive/`（I10），只有「彻底删除」才真删。
  // 所以这里没有第二条更狠的路，菜单里也就不写「删掉」那种话。
  menuItem(menu, "归档", "可从归档恢复", () => void archiveSkill(skill.name), { danger: true });

  mountMenu(menu, x, y, first);
}

// 长按（手机）/ 右键（桌面）都走同一个回调：卡片和左栏类别行用的是同一套手势。
function attachMenuGesture(node, open) {
  let timer = null;
  let startX = 0;
  let startY = 0;

  const cancel = () => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  };

  node.addEventListener("pointerdown", (event) => {
    if (event.pointerType === "mouse") return; // 桌面走 contextmenu
    if (event.button !== 0) return;
    cancel();
    startX = event.clientX;
    startY = event.clientY;
    timer = setTimeout(() => {
      timer = null;
      open(startX, startY);
      swallowNextClick();
    }, LONG_PRESS_MS);
  });

  // 手指一挪就当滚动，别在滑动列表时蹦出菜单。
  node.addEventListener("pointermove", (event) => {
    if (timer === null) return;
    if (Math.abs(event.clientX - startX) > LONG_PRESS_SLOP || Math.abs(event.clientY - startY) > LONG_PRESS_SLOP) {
      cancel();
    }
  });

  for (const type of ["pointerup", "pointercancel", "pointerleave"]) node.addEventListener(type, cancel);
}

// 长按松手时浏览器还会补一个 click；落在刚弹出的菜单上就是误触。吞掉这一下。
function swallowNextClick() {
  const kill = (event) => {
    event.preventDefault();
    event.stopPropagation();
    drop();
  };
  const drop = () => {
    removeEventListener("click", kill, true);
    clearTimeout(swallowTimer);
  };
  addEventListener("click", kill, true);
  clearTimeout(swallowTimer);
  swallowTimer = setTimeout(drop, 1200);
}

// ── 类别 chip（卡上只读 / 详情里可改） ────────────────────────────────────
//
// 同一份 `category[]` 只开一个写入路径：详情。卡上是只读 chip，菜单里「类别」只是跳过去。

function tagRow(skill, { readOnly = false } = {}) {
  const wrap = el("div", "tags " + (readOnly ? "product-tags" : "detail-tags"));

  for (const tag of categoriesOf(skill)) {
    const chip = el("span", "tag");
    const nameEl = el("span", "tag-name");
    const cut = tag.lastIndexOf("/");
    if (cut > 0) {
      nameEl.append(el("span", "tag-prefix", tag.slice(0, cut + 1)));
      nameEl.append(tag.slice(cut + 1));
    } else {
      nameEl.append(tag);
    }
    chip.append(nameEl);
    if (readOnly) chip.classList.add("tag-ro");
    else {
      const remove = el("button", "tag-x", "×");
      remove.type = "button";
      remove.title = `从 ${skill.name} 摘掉「${tag}」`;
      remove.onclick = () => removeTag(skill.name, tag);
      chip.append(remove);
    }
    wrap.append(chip);
  }

  if (readOnly) return wrap;

  if (state.editing === skill.name) {
    const input = el("input", "tag-input");
    input.type = "text";
    input.placeholder = "类别，回车";
    input.setAttribute("list", "tag-suggestions");
    let done = false;
    const commit = () => {
      if (done) return;
      done = true;
      const value = input.value;
      state.editing = null;
      render();
      if (value.trim()) addTag(skill.name, value);
    };
    input.onkeydown = (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        commit();
      } else if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        done = true;
        state.editing = null;
        render();
      }
    };
    input.onblur = commit;
    wrap.append(input);
  } else {
    const add = el("button", "tag-add", "+ 类别");
    add.type = "button";
    add.onclick = () => {
      state.editing = skill.name;
      render();
    };
    wrap.append(add);
  }

  return wrap;
}

// ── 技能卡 ─────────────────────────────────────────────────────────────────
//
// 卡上只放身份：字母砖 + 名字 + 说明 + 类别 chip + 右上 `⋯`。
// **状态角标一个都不许搬走** —— 扫一眼看谁坏了、谁还没发布，是这页存在的理由（spec §7.1）。

function productCard(card) {
  const skill = card;
  const failed = card.state === "failed";
  const unpublished = card.state === "unpublished";
  const node = el("article", "product" + (failed ? " load-failed" : unpublished ? " unpublished" : ""));

  node.tabIndex = 0;
  node.setAttribute("role", "button");
  node.setAttribute("aria-label", `${skill.name} · ${STATE_LABEL[card.state] ?? card.state}`);
  node.dataset.name = skill.name;
  const openMenuAt = (mx, my) => openCardMenu(mx, my, skill);

  node.onclick = () => openDetail(skill.name);
  node.oncontextmenu = (event) => {
    event.preventDefault();
    openMenuAt(event.clientX, event.clientY);
  };
  node.onkeydown = (event) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      openDetail(skill.name);
    } else if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      event.preventDefault();
      const rect = node.getBoundingClientRect();
      openMenuAt(rect.left + 8, rect.bottom - 6);
    }
  };
  attachMenuGesture(node, openMenuAt);

  const visual = el("div", "visual");
  visual.style.setProperty("--hue", String(hueOf(skill.name)));
  visual.append(el("span", "visual-mono", initialsOf(skill.name)));
  if (failed) visual.append(el("span", "badge-float fail", "装载失败"));
  else if (unpublished) visual.append(el("span", "badge-float unpub", "未发布"));
  node.append(visual);

  const body = el("div", "product-body");
  const title = el("div", "product-title");
  title.append(el("span", "product-name", skill.name));
  if (skill.hidden) {
    const flag = el("span", "product-hidden", "不进提示词");
    flag.title = "disable-model-invocation：模型看不到它，只有人手动读";
    title.append(flag);
  }

  // 卡上唯一可见的菜单入口：桌面 hover 出现、触屏常驻。
  const menuBtn = el("button", "product-menu", "⋯");
  menuBtn.type = "button";
  menuBtn.title = `${skill.name} 的菜单：正文 / 类别 / 归档`;
  menuBtn.setAttribute("aria-label", `${skill.name} 的菜单`);
  menuBtn.onclick = (event) => {
    event.stopPropagation();
    const rect = menuBtn.getBoundingClientRect();
    openMenuAt(rect.left, rect.bottom + 4);
  };
  menuBtn.onkeydown = (event) => {
    if (event.key === "Enter" || event.key === " ") event.stopPropagation();
  };
  title.append(menuBtn);
  body.append(title);

  if (skill.description) body.append(el("p", "product-sum", skill.description));

  // 装载失败的错误摘要在详情里有全文；卡上留两行，因为「坏了」和「为什么坏」不是一个问题。
  // 摘要直接摆出来，不放 title：手机没有 hover，「为什么坏了」永远点不出来。
  if (failed && skill.load?.error) body.append(el("p", "product-err", skill.load.error));

  const tags = tagRow(skill, { readOnly: true });
  if (tags.childNodes.length) body.append(tags);
  node.append(body);
  return node;
}

// ── 筛选栏 + 左栏 facet ────────────────────────────────────────────────────

function renderFilters() {
  const box = $("filters");
  box.textContent = "";
  const view = state.view;
  const archived = view.archived;

  const chip = (label, value, count, { tone = "", title = "", dot = "" } = {}) => {
    const button = el("button", "filter-chip" + (state.filter === value ? " on" : "") + (tone ? ` ${tone}` : ""));
    button.type = "button";
    if (dot) {
      const mark = el("span", "filter-dot");
      mark.dataset.tone = dot;
      button.append(mark);
    }
    button.append(el("span", "filter-label", label));
    button.append(el("span", "filter-n", String(count)));
    if (title) button.title = title;
    button.onclick = () => {
      state.filter = state.filter === value ? "" : value;
      leaveDetail(); // 换筛选就是回列表
      render();
    };
    return button;
  };

  box.append(chip("全部", "", view.skillCount));

  // 选中的是自己建的类别时，把「父级含子级」摆在结果旁边，而不是写进左栏说明书。
  const filter = state.filter;
  if (filter && filter !== UNCATEGORIZED && filter !== ARCHIVED && !stateFilterOf(filter)) {
    const kids = childCount(view.categoryNames, filter);
    const scope = el("button", "filter-chip on");
    scope.type = "button";
    scope.append(el("span", "filter-label", kids ? `${leafOf(filter)} · 含 ${kids} 个子级` : leafOf(filter)));
    scope.append(el("span", "filter-n", "×"));
    scope.title = `只看 ${filter}${kids ? "（含子级）" : ""} —— 点一下取消`;
    scope.onclick = () => {
      state.filter = "";
      leaveDetail();
      render();
    };
    box.append(scope);
  }

  const uncategorized = view.uncategorized;
  if (uncategorized) box.append(chip("未分类", UNCATEGORIZED, uncategorized, { title: "一个类别都没挂的技能" }));

  // 状态图例就是筛选：非零才出现，色点只是辅助，文字才是本体。
  const byState = new Map();
  for (const card of state.view.cards) byState.set(card.state, (byState.get(card.state) ?? 0) + 1);
  const views = [
    { key: "unpublished", label: STATE_LABEL.unpublished, tone: "warn", title: "磁盘上有、HEAD 里没有 —— 别人 pull 不到" },
    { key: "failed", label: STATE_LABEL.failed, tone: "bad", title: "装载失败：frontmatter 或目录名不对，进不了名册" },
  ].filter((item) => byState.get(item.key));
  if (views.length) {
    box.append(el("span", "filter-sep"));
    for (const item of views) {
      box.append(chip(item.label, stateFilter(item.key), byState.get(item.key), { dot: item.tone, title: item.title }));
    }
  }

  if (archived.length) {
    box.append(
      chip("已归档", ARCHIVED, archived.length, {
        tone: "archived",
        title: "从仓库归档的技能：可以恢复，也可以彻底删除",
      }),
    );
  }
}

function renderFacets() {
  const view = state.view;
  const facets = $("facets");
  facets.textContent = "";

  const row = (path) => {
    const item = el(
      "div",
      "facet" + (state.filter === path ? " on" : "") + (view.rows.find((entry) => entry.path === path)?.parent ? " parent" : ""),
    );

    // 改名就在原地改：一行输入框，回车落盘，Esc 反悔。填的是全路径，所以改前缀 = 搬家。
    if (state.renaming === path) {
      const input = el("input", "facet-input");
      input.type = "text";
      input.value = path;
      input.setAttribute("aria-label", `重命名类别 ${path}`);
      input.style.marginLeft = `${Math.min(depthOf(path), 3) * 11}px`;
      let done = false;
      const finish = (keep) => {
        if (done) return;
        done = true;
        const next = input.value.trim();
        state.renaming = null;
        render();
        if (keep && next && next !== path) void renameCategory(path, next);
      };
      input.onkeydown = (event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          finish(true);
        } else if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          finish(false);
        }
      };
      input.onblur = () => finish(true);
      item.append(input);
      return item;
    }

    const main = el("button", "facet-main");
    main.type = "button";
    main.append(el("span", "facet-label", leafOf(path)));
    main.append(el("span", "facet-n", String(catCount(path))));
    // 栏里只显示末段，缩进已经说明它在谁底下；全路径放 title。
    main.title = path;
    main.style.paddingLeft = `${8 + Math.min(depthOf(path), 3) * 11}px`;
    main.onclick = () => {
      state.filter = state.filter === path ? "" : path;
      leaveDetail();
      render();
    };
    item.append(main);

    item.dataset.category = path;

    // 可见的菜单入口：桌面 hover 出现，触屏常驻。右键 / 长按只是快捷方式。
    const menuBtn = el("button", "facet-menu", "⋯");
    menuBtn.type = "button";
    menuBtn.title = `${path} 的菜单：新建子类别 / 改名 / 删除`;
    menuBtn.setAttribute("aria-label", `${path} 的类别菜单`);
    menuBtn.onclick = (event) => {
      event.stopPropagation();
      const rect = menuBtn.getBoundingClientRect();
      openCategoryMenu(rect.left, rect.bottom + 4, path);
    };
    item.append(menuBtn);

    item.oncontextmenu = (event) => {
      event.preventDefault();
      openCategoryMenu(event.clientX, event.clientY, path);
    };
    main.onkeydown = (event) => {
      if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
        event.preventDefault();
        const rect = main.getBoundingClientRect();
        openCategoryMenu(rect.left + 8, rect.bottom - 4, path);
      }
    };
    attachMenuGesture(main, (mx, my) => openCategoryMenu(mx, my, path));
    return item;
  };

  for (const path of view.categoryNames) facets.append(row(path));

  // 整串挂上去之后再聚焦：行在 append 之前是「失联」的，focus 会落空（vbe4pt 踩过）。
  const renaming = facets.querySelector(".facet-input");
  if (renaming) {
    renaming.focus();
    renaming.select();
  }

  $("cat-count").textContent = String(view.categoryNames.length);
}

function renderRailNew() {
  const host = $("rail-new");
  host.textContent = "";
  if (state.creating) {
    const input = el("input", "rail-input");
    input.type = "text";
    input.placeholder = "新类别名，如 工具/搜索";
    input.value = state.createPrefix;
    let done = false;
    const commit = () => {
      if (done) return;
      done = true;
      const value = input.value;
      state.creating = false;
      state.createPrefix = "";
      render();
      if (value.trim()) void addCategory(value);
    };
    input.onkeydown = (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        commit();
      } else if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        done = true;
        state.creating = false;
        state.createPrefix = "";
        render();
      }
    };
    input.onblur = commit;

    // 说明只在「正在输入」这一刻出现：/ 是层级，回车落盘，写回仓库。
    const hint = el("p", "rail-hint");
    const updateHint = () => {
      const parts = normalizeCategory(input.value).split("/").filter(Boolean);
      hint.textContent = parts.length > 1 ? `${parts.join(" › ")} · 回车建` : "回车建 · 会写回仓库（qrd3wh 落盘）";
    };
    input.oninput = updateHint;
    updateHint();
    host.append(input, hint);
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  } else {
    const add = el("button", "rail-add", "+ 新建类别");
    add.type = "button";
    add.onclick = () => {
      state.creating = true;
      state.createPrefix = "";
      state.renaming = null;
      render();
    };
    host.append(add);
  }
}

function renderSuggestions() {
  const list = $("tag-suggestions");
  list.textContent = "";
  for (const name of state.view.categoryNames) {
    const option = el("option");
    option.value = name;
    list.append(option);
  }
}

// 归档区：仓库里 `.archive/*`。这些不在网格里（spec §7.1），是备份。
function renderArchivedGrid() {
  const grid = $("grid");
  grid.textContent = "";
  const items = state.view.archived;
  const list = el("div", "arch-list");
  for (const item of items) {
    const row = el("div", "arch-row");
    const main = el("div", "arch-main");
    const head = el("div", "arch-head");
    head.append(el("span", "arch-name", item.name));
    head.append(el("span", "arch-when dim", ".archive/ 里躺着"));
    main.append(head);
    main.append(el("p", "arch-sum", "仓库里还留着这一份，恢复就是搬回仓库根目录。"));
    row.append(main);

    const acts = el("div", "arch-acts");
    const back = el("button", "btn btn-small", "恢复");
    back.type = "button";
    back.title = `把 .archive/${item.name} 搬回仓库根目录，并 commit + push`;
    back.onclick = () => void restoreArchived(item.name);
    acts.append(back);
    const gone = el("button", "btn btn-small btn-bad", "彻底删除");
    gone.type = "button";
    gone.title = `连备份一起删掉：git rm -r .archive/${item.name}`;
    gone.onclick = () => void purgeArchived(item.name);
    acts.append(gone);
    row.append(acts);
    list.append(row);
  }
  grid.append(list);
  $("result-count").textContent = `${items.length} 条已归档`;
}

// ── 详情：点卡进来（主区换页，不是浮层） ──────────────────────────────────
//
// 里面三块：类别 / 正文 / 仓库。vbe4pt 那一页的另外几栏这里不存在 ——
// 技能没有「装 / 卸」，也没有多个可装载名（spec §7.1、§11），搬过来就是能点但没用的按钮。

function detailSection(title, hint) {
  const node = el("section", "detail-sec");
  const head = el("div", "detail-sec-head");
  head.append(el("h3", "detail-sec-title", title));
  if (hint) head.append(el("span", "detail-sec-hint", hint));
  node.append(head);
  const body = el("div", "detail-sec-body");
  node.append(body);
  return { node, body };
}

function detailRow(label, value, title = "") {
  const row = el("div", "detail-row");
  row.append(el("span", "detail-row-k", label));
  const text = el("span", "detail-row-v", value);
  if (title) text.title = title;
  row.append(text);
  return row;
}

function alertLine(tone, label, text) {
  const line = el("p", `detail-alert ${tone}`);
  line.append(el("span", "detail-alert-k", label));
  line.append(el("span", "detail-alert-v", text));
  return line;
}

function leaveDetail() {
  state.detail = null;
  state.detailBody = null;
  state.editing = null;
  // 离开详情就把攒着的类别发掉：别让人以为勾上了，其实还压在 8 秒计时器里。
  if (categoryWrites.hasPending()) void categoryWrites.flush();
}

function openDetail(name, { editTags = false } = {}) {
  state.detail = name;
  // 「类别」那一项是跳到详情里的勾选区，顺手把输入框打开 —— 菜单不做第二个编辑面。
  state.editing = editTags ? name : null;
  render();
  $sel(".results")?.scrollTo({ top: 0, behavior: "smooth" });
}

function closeDetail() {
  if (!state.detail) return;
  leaveDetail();
  render();
  $sel(".results")?.scrollTo({ top: 0, behavior: "smooth" });
}

function renderDetail() {
  const host = $("detail");
  const skill = skillNow(state.detail);
  // 详情开着的时候这条被归档 / 删走了：退回列表，别留一面空墙。
  if (!skill) {
    leaveDetail();
    host.hidden = true;
    $("grid").hidden = false;
    renderGrid();
    return;
  }

  host.textContent = "";
  const cats = categoriesOf(skill);
  // 顶栏那一格在列表里是计数，在详情里就是这条的类别 —— 别写一句「技能详情」凑数。
  $("result-count").textContent = cats.length ? cats.join(" · ") : "未分类";

  const card = state.view.cards.find((item) => item.name === skill.name);
  const stateName = card?.state ?? "ok";
  const failed = stateName === "failed";
  const unpublished = stateName === "unpublished";

  const back = el("button", "detail-back", "‹ 返回");
  back.type = "button";
  back.title = "回到列表（Esc）";
  back.onclick = closeDetail;
  host.append(back);

  // ── 头：色砖 + 名字 + 状态 + 说明 + 入口 ─────────────────────────────
  const head = el("header", "detail-head");
  const tile = el("div", "detail-tile");
  tile.style.setProperty("--hue", String(hueOf(skill.name)));
  tile.append(el("span", "detail-mono", initialsOf(skill.name)));
  head.append(tile);

  const id = el("div", "detail-id");
  const nameRow = el("div", "detail-name-row");
  nameRow.append(el("h2", "detail-name", skill.name));
  const badge = el("span", "detail-badge", STATE_LABEL[stateName] ?? stateName);
  badge.dataset.tone = failed ? "fail" : unpublished ? "unpub" : "ok";
  nameRow.append(badge);
  if (skill.hidden) {
    const hidden = el("span", "detail-badge", "不进提示词");
    hidden.dataset.tone = "gone";
    hidden.title = "disable-model-invocation：模型看不到它";
    nameRow.append(hidden);
  }
  id.append(nameRow);
  id.append(el("p", "detail-sum", skill.description || "没写说明。"));

  const acts = el("div", "detail-acts");
  const body = el("button", "btn btn-small", "正文 ›");
  body.type = "button";
  body.title = `看 / 改 ${skill.name}/SKILL.md`;
  body.onclick = () => void openEditor(skill);
  acts.append(body);
  id.append(acts);
  head.append(id);
  host.append(head);

  // ── 真话说在最前面：装载失败 ──────────────────────────────────────────
  if (failed && skill.load?.error) {
    const line = alertLine("bad", "装载失败", skill.load.error);
    line.title = "";
    host.append(line);
    // 「重试装载」只做一件事：`POST v1/skills/reload` —— 让 qrd3wh 把仓库重扫一遍。
    // 它不修文件、不撤改动、不 commit。所以按钮旁边必须写清「先改回去，再点这里」：
    // 点一百次也不会让一个写坏的 frontmatter 自己变好。
    host.append(
      el("p", "detail-sum", "这个按钮只是让 qrd3wh 重扫仓库 —— 它不会替你修文件。先把 SKILL.md 改回来，再点这里。"),
    );
    const retry = el("button", "btn btn-small", "重试装载（重扫仓库）");
    retry.type = "button";
    retry.title = "POST v1/skills/reload：只重扫，不改文件、不提交";
    retry.onclick = () => void reloadSkill(skill.name);
    host.append(retry);
  }

  const grid = el("div", "detail-grid");

  const cat = detailSection("类别", "写回仓库 · qrd3wh 落盘");
  cat.body.append(tagRow(skill));
  grid.append(cat.node);

  // 正文：说明是 frontmatter 的 description（给模型看），SKILL.md 的正文才是人写的那一段。
  const text = detailSection("正文", "SKILL.md");
  if (skill.description) text.body.append(detailRow("说明", skill.description));
  const pre = el("pre", "detail-body-text");
  pre.id = "detail-body";
  text.body.append(pre);
  const edit = el("button", "btn btn-small", "改正文 ›");
  edit.type = "button";
  edit.onclick = () => void openEditor(skill, { path: "SKILL.md" });
  text.body.append(edit);
  grid.append(text.node);

  const where = detailSection("仓库", "HEAD 里有没有");
  where.body.append(detailRow("发布", skill.tracked === false ? "未发布 · HEAD 里没有" : "已进 HEAD"));
  where.body.append(detailRow("目录", skill.dir ?? skill.name));
  where.body.append(detailRow("文件", skill.file ?? `${skill.name}/SKILL.md`, skill.file ?? ""));
  grid.append(where.node);

  host.append(grid);

  // 输入框必须等整串挂进文档之后再 focus —— 行还在内存里时聚焦会落空（左栏踩过）。
  const input = host.querySelector(".tag-input");
  if (input) {
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  }

  ensureDetailBody(skill);
}

/** 详情里的正文按需读：不进这一页就别发请求。 */
function ensureDetailBody(skill) {
  if (state.detailBody?.name === skill.name) return paintDetailBody();
  state.detailBody = { name: skill.name, loading: true, text: "", error: "" };
  paintDetailBody();
  transport
    .get(`v1/skills/file?name=${encodeURIComponent(skill.name)}&path=SKILL.md`)
    .then((file) => {
      if (state.detailBody?.name !== skill.name) return;
      state.detailBody = { name: skill.name, loading: false, text: file.text, error: "" };
      paintDetailBody();
    })
    .catch((error) => {
      if (state.detailBody?.name !== skill.name) return;
      state.detailBody = { name: skill.name, loading: false, text: "", error: String(error?.message ?? error) };
      paintDetailBody();
    });
}

function paintDetailBody() {
  const pre = $("detail-body");
  const body = state.detailBody;
  if (!pre || !body) return;
  pre.textContent = body.loading ? "读 SKILL.md…" : body.error ? `读不到：${body.error}` : body.text;
}

function renderGrid() {
  if (state.filter === ARCHIVED) return renderArchivedGrid();
  const view = state.view;
  const shown = filterCards(view.cards, { filter: state.filter, query: state.query });

  const grid = $("grid");
  grid.textContent = "";
  for (const card of shown) grid.append(productCard(card));
  if (!shown.length) {
    const why = !state.data
      ? "读取中…"
      : state.data?.repo?.error
        ? `仓库读不了：${state.data.repo.error}（这不是「一条技能都没有」—— 是清单还没读到）`
        : state.query.trim()
        ? "没有匹配的技能。"
        : state.filter === UNCATEGORIZED
          ? "没有未分类的技能。"
          : stateFilterOf(state.filter)
            ? `没有「${STATE_LABEL[stateFilterOf(state.filter)] ?? stateFilterOf(state.filter)}」的技能。`
            : state.filter
              ? `没有标「${state.filter}」的技能。`
              : "仓库里一条技能都没有 —— 点右上「＋ 新建技能」开一条。";
    grid.append(el("div", "empty", why));
  }

  $("result-count").textContent =
    shown.length === view.skillCount
      ? `${view.skillCount} 条技能`
      : `${shown.length} / ${view.skillCount} 条技能`;
}

// ── 渲染入口 ───────────────────────────────────────────────────────────────

function renderRepo() {
  const repo = state.data?.repo ?? {};
  const error = String(repo.error ?? "");
  $("repo-path").textContent = state.data?.root ?? "—";
  // repo.error 非空 = git 读不了。光把红字塞进角落的 hint 里没人看得见，
  // 所以顶栏也立一个 chip：这一页现在是「读不到清单」，不是「一条技能都没有」。
  repoChip.hidden = !error;
  repoChip.textContent = error ? "仓库读不了" : "";
  repoChip.title = "";
  const dirty = Boolean(repo.dirty);
  const ahead = repo.ahead ?? 0;
  const behind = repo.behind ?? 0;
  const bits = [];
  if (repo.error) bits.push(`仓库读不了：${repo.error}`);
  else {
    bits.push(`HEAD ${short(repo.head)}`);
    if (dirty) bits.push("有未提交改动");
    if (ahead) bits.push(`${ahead} 个提交没推`);
    if (behind) bits.push(`远端多 ${behind} 个`);
    if (!dirty && !ahead && !behind) bits.push("已同步");
  }
  $("repo").textContent = bits.join(" · ");
  $("repo").dataset.drift = repo.error || dirty ? "1" : "0";
  $("repo").title = repo.error
    ? "git 命令失败了 —— 面板还能显示上一次能读到的清单，但发布 / 归档会失败"
    : [repo.remote ?? "", repo.head ? `HEAD ${repo.head}` : ""].filter(Boolean).join("\n");
}

function render() {
  if (!state.data) return;
  closeMenu();

  renderRepo();
  renderFilters();
  renderFacets();
  renderRailNew();
  renderSuggestions();

  // 详情占主区：网格让位，而不是被挤窄。
  $("grid").hidden = Boolean(state.detail);
  $("detail").hidden = !state.detail;
  if (state.detail) renderDetail();
  else renderGrid();

  $("count").textContent = `${state.view.skillCount} 条技能${state.view.archived.length ? ` · ${state.view.archived.length} 条归档` : ""}`;
}

/** 重扫仓库：修好的 SKILL.md 得重新进名册，别让人刷新整页才能看到。 */
async function reloadSkill(name) {
  if (!transport) return;
  try {
    const result = await transport.post("v1/skills/reload", {});
    await load({ quiet: true });
    const card = state.view?.cards.find((item) => item.name === name);
    if (card?.state === "failed") toast(`${name} 还是装载失败：${card.load?.error ?? "看卡上的摘要"}`, "bad");
    else if (result.error) toast(`重扫有问题：${result.error}`, "bad");
    else toast(`${name} 已重新装载`);
  } catch (error) {
    toast(`重新装载失败：${error.message}`, "bad");
  }
}

// ── 归档 / 恢复 / 彻底删除（会改仓库） ────────────────────────────────────

// 破坏性且会推上远端：要求手输技能名。
function confirmName({ title, hint, name, label = "确定", onConfirm }) {
  const modal = $("danger");
  const input = $("danger-input");
  const ok = $("danger-ok");
  $("danger-title").textContent = title;
  $("danger-hint").textContent = hint;
  $("danger-name").textContent = name;
  $("danger-ok").textContent = label;
  $("danger-err").hidden = true;
  input.value = "";
  ok.disabled = true;
  modal.hidden = false;
  $("danger-scrim").hidden = false;
  setTimeout(() => input.focus(), 30);

  const cleanup = () => {
    modal.hidden = true;
    $("danger-scrim").hidden = true;
    input.oninput = null;
    ok.onclick = null;
    $("danger-cancel").onclick = null;
    $("danger-scrim").onclick = null;
    document.removeEventListener("keydown", onKey, true);
  };
  const go = async () => {
    if (ok.disabled) return;
    cleanup();
    await onConfirm();
  };
  const onKey = (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      cleanup();
    } else if (event.key === "Enter" && !ok.disabled) {
      event.preventDefault();
      void go();
    }
  };
  input.oninput = () => {
    ok.disabled = input.value.trim() !== name;
  };
  ok.onclick = () => void go();
  $("danger-cancel").onclick = cleanup;
  $("danger-scrim").onclick = cleanup;
  document.addEventListener("keydown", onKey, true);
}

async function archiveSkill(name) {
  confirmName({
    // 文案说真话（I10）：**未发布的也进 .archive/**。它没发布过不代表可以悄悄 rm ——
    // 盘上删掉的东西只有「彻底删除」才真没，别的都得留个能捡回来的地方。
    // （早先这里对未发布的分支写「删掉 / 没有历史可找」，那是旧行为留下的谎。）
    title: `把 ${name} 归档？`,
    hint: "未发布的技能也会移进 `.archive/` —— 只有「彻底删除」才真删（I10）。",
    name,
    label: "归档并推送",
    onConfirm: async () => {
      try {
        const result = await transport.post("v1/skills/archive", { name });
        if (state.detail === name) leaveDetail();
        if (result.pushed === false) toast(`已在本地提交，但推送失败：${pushFailure(result)}`, "bad");
        else toast(`已归档 ${name} · 右上「已归档」里可以恢复`);
        await load({ quiet: true });
      } catch (error) {
        toast(`归档失败：${error.message}`, "bad");
      }
    },
  });
}

async function restoreArchived(name) {
  try {
    const result = await transport.post("v1/skills/restore", { name });
    if (result.pushed === false) toast(`已恢复，但推送失败：${pushFailure(result)}`, "bad");
    else toast(`${name} 已从归档恢复（未提交前的状态就是未发布）`);
    await load({ quiet: true });
  } catch (error) {
    toast(`恢复失败：${error.message}`, "bad");
  }
}

async function purgeArchived(name) {
  confirmName({
    title: `彻底删除 ${name}？`,
    hint: "连仓库里的备份一起删掉，并提交 + 推送。这一步之后只能靠 git 历史找回来。",
    name,
    label: "彻底删除",
    onConfirm: async () => {
      try {
        const result = await transport.post("v1/skills/purge", { name });
        if (result.pushed === false) toast(`已提交，但推送失败：${pushFailure(result)}`, "bad");
        else toast(`${name} 已彻底删除`);
        await load({ quiet: true });
      } catch (error) {
        toast(`彻底删除失败：${error.message}`, "bad");
      }
    },
  });
}

// ── 正文抽屉 ───────────────────────────────────────────────────────────────
//
// 保存和发布是两个动作，别合并：
//   · 保存 = `PUT /v1/skills/file`：立刻落盘，**不 commit** —— 敲到一半可以停。
//   · 发布 = `POST /v1/skills/publish`：内容闸 + 重扫 + commit + push。
// 合起来的话，半截的 SKILL.md 会被推上去；而 `description` 是空的就等于这条技能不存在。

const bytes = (n) => (n < 1024 ? `${n} B` : `${(n / 1024).toFixed(1)} KB`);
const editorDirty = () => Boolean(state.editor) && $("ed-text").value !== state.editor.text;

async function openEditor(skill, { path = null } = {}) {
  closeMenu();
  state.editor = {
    name: skill.name,
    files: [],
    filesLoading: true,
    path: null,
    text: "",
    base: null,
    pending: new Set(),
    saving: false,
    error: "",
    conflict: false, // 409：别人先改了 —— 只提示「重新加载」，不猜内容
    notice: "",
    // ── 差异视图 ──────────────────────────────────────────────────────────
    mode: "edit", // edit | diff
    baseText: null, // HEAD 里那一版；null = 这个文件还没进仓库
    baseMissing: true,
    diff: null,
    expanded: new Set(),
    lineCount: 1,
    diffTimer: null,
  };
  state.publishing = false;
  $("ed-text").value = "";
  $("ed-text").disabled = true;
  $("ed-path").textContent = "—";
  $("ed-scrim").hidden = false;
  $("editor").hidden = false;
  $("ed-name").textContent = skill.name;
  $("ed-meta").textContent = `SKILL.md · ${skill.tracked === false ? "未发布" : "已进 HEAD"}`;
  setEditorMode("edit");
  renderEditorFiles();
  renderEditorState();

  try {
    const list = await transport.get(`v1/skills/files?name=${encodeURIComponent(skill.name)}`);
    state.editor.files = list.files ?? [];
    state.editor.filesLoading = false;
    renderEditorFiles();
    // 先开 SKILL.md（正文最常改），指定了就跟指定的走。
    const first =
      (path && state.editor.files.find((file) => file.path === path)) ??
      state.editor.files.find((file) => file.path === "SKILL.md") ??
      state.editor.files[0];
    if (first) await openEditorFile(first.path, { force: true });
    else $("ed-path").textContent = "这个目录里没有可编辑的文件";
  } catch (error) {
    state.editor.filesLoading = false;
    state.editor.error = `读文件列表失败：${error.message}`;
    renderEditorFiles();
    renderEditorState();
  }
}

function closeEditor({ force = false } = {}) {
  if (!state.editor) return;
  if (!force && editorDirty() && !confirm("这个文件有没保存的改动，关掉就丢了。继续？")) return;
  state.editor = null;
  $("editor").hidden = true;
  $("ed-scrim").hidden = true;
}

function renderEditorFiles() {
  const ed = state.editor;
  if (!ed) return;
  const nav = $("ed-files");
  nav.textContent = "";
  if (ed.filesLoading) {
    nav.append(el("div", "ed-files-empty", "读文件列表…"));
    return;
  }
  if (!ed.files.length) {
    nav.append(el("div", "ed-files-empty", "这个目录里没有文件"));
    return;
  }
  for (const file of ed.files) {
    const btn = el("button", "ed-file");
    btn.type = "button";
    btn.dataset.kind = file.path.endsWith(".md") ? "markdown" : "other";
    btn.setAttribute("aria-current", String(file.path === ed.path));
    btn.append(el("span", "ed-file-name", file.path));
    btn.append(el("span", "ed-file-size", bytes(file.size)));
    btn.title = `${file.path} · ${bytes(file.size)}`;
    btn.onclick = () => void openEditorFile(file.path);
    nav.append(btn);
  }
}

function renderEditorState() {
  const ed = state.editor;
  if (!ed) return;
  const chip = $("ed-state");
  let tone = "ok";
  let text = "已落盘";
  if (ed.error) {
    tone = "bad";
    text = "出错";
  } else if (ed.saving) {
    tone = "pending";
    text = "保存中…";
  } else if (state.publishing) {
    tone = "pending";
    text = "发布中…";
  } else if (editorDirty()) {
    tone = "dirty";
    text = "未保存";
  } else if (ed.pending?.size) {
    tone = "pending";
    text = `已保存 · 待发布 ${ed.pending.size}`;
  }
  chip.dataset.tone = tone;
  chip.textContent = text;

  const busy = ed.saving || state.publishing || !ed.path;
  $("ed-save").disabled = busy;
  $("ed-publish").disabled = busy;

  const msg = $("ed-msg");
  if (ed.error) {
    msg.hidden = false;
    msg.dataset.tone = "bad";
    msg.textContent = ed.error;
    // 409 不是「坏了」，是「你手里这份旧了」：给一个就地重读，别让人关抽屉再摸回来。
    if (ed.conflict) {
      const again = el("button", "ed-btn", "重新加载");
      again.type = "button";
      again.title = "重读磁盘上的这一版，base 跟着更新";
      again.style.marginTop = "8px";
      again.onclick = () => void reloadEditorFile();
      msg.append(again);
    }
  } else if (ed.notice) {
    msg.hidden = false;
    msg.dataset.tone = "ok";
    msg.textContent = ed.notice;
  } else {
    msg.hidden = true;
    msg.textContent = "";
  }
}

async function openEditorFile(path, { force = false } = {}) {
  const ed = state.editor;
  if (!ed) return;
  if (!force && editorDirty() && !confirm(`「${ed.path}」有没保存的改动，切过去会丢掉。继续？`)) return;
  try {
    const file = await transport.get(
      `v1/skills/file?name=${encodeURIComponent(ed.name)}&path=${encodeURIComponent(path)}`,
    );
    ed.path = file.path;
    ed.text = file.text;
    ed.base = file.etag;
    ed.expanded.clear();
    ed.error = "";
    ed.conflict = false;
    ed.notice = "";
    const area = $("ed-text");
    area.disabled = false;
    area.value = file.text;
    $("ed-path").textContent = `${ed.name}/${file.path}`;
    ed.lineCount = file.text.split("\n").length;
    // 差异基线：对 HEAD。未跟踪的文件 before 是 null（整份算新增）。
    ed.baseText = null;
    ed.baseMissing = true;
    recomputeDiff();
    renderEditorFiles();
    renderEditorState();
    await loadBaseline(ed, file.path);
  } catch (error) {
    ed.error = `读文件失败：${error.message}`;
    renderEditorState();
  }
}

/** `GET /v1/skills/pending/diff?path=<name>/<相对路径>` 的 `before` 就是 HEAD 里那一版。 */
async function loadBaseline(ed, path) {
  try {
    const data = await transport.get(`v1/skills/pending/diff?path=${encodeURIComponent(`${ed.name}/${path}`)}`);
    if (state.editor !== ed || ed.path !== path) return;
    ed.baseText = data.before ?? null;
    ed.baseMissing = data.before == null;
    recomputeDiff();
    if (ed.mode === "diff") renderDiffView();
  } catch {
    // 读不到基线只影响差异视图，不该挡住编辑。
    if (state.editor === ed && ed.path === path) {
      ed.baseText = null;
      ed.baseMissing = true;
      recomputeDiff();
    }
  }
}

async function saveEditorFile({ quiet = false } = {}) {
  const ed = state.editor;
  if (!ed?.path || ed.saving) return false;
  const text = $("ed-text").value;
  ed.saving = true;
  ed.error = "";
  ed.notice = "";
  renderEditorState();
  try {
    const saved = await transport.put("v1/skills/file", {
      name: ed.name,
      path: ed.path,
      text,
      // base 是「我改的是哪一版」：别人先改了，服务端 409，别把对方的字盖掉。
      base: ed.base,
    });
    ed.text = text;
    ed.base = saved.etag;
    ed.conflict = false;
    ed.pending.add(ed.path);
    if (!quiet) ed.notice = `已保存到磁盘：${ed.name}/${ed.path}（还没提交）`;
    return true;
  } catch (error) {
    // 409 走自己的分支：不是「保存坏了」，是我手里这份 base 过期了 —— 重读再改。
    ed.conflict = Boolean(error?.conflict);
    ed.error = ed.conflict
      ? `保存失败：${ed.path} ${CONFLICT_MESSAGE} —— 磁盘上那一版比手里的新，重读之后再改，别把别人的字盖掉。`
      : `保存失败：${error.message}`;
    return false;
  } finally {
    ed.saving = false;
    renderEditorState();
  }
}

/** 409 之后的重读：拿磁盘那一版 + 新 base，放弃手里这份（用户是主动点的）。 */
async function reloadEditorFile() {
  const ed = state.editor;
  if (!ed?.path) return;
  ed.conflict = false;
  ed.error = "";
  await openEditorFile(ed.path, { force: true });
  if (state.editor !== ed) return;
  ed.notice = "已重读磁盘上的版本 —— base 跟着更新了，接着改就行";
  renderEditorState();
}

async function publishEditor() {
  const ed = state.editor;
  if (!ed || state.publishing) return;
  if (editorDirty() && !(await saveEditorFile({ quiet: true }))) return;
  // 类别还压在 8 秒队列里就先发掉：否则这一次 commit 会漏掉它，发布完才脏。
  if (categoryWrites.hasPending()) await categoryWrites.flush();
  state.publishing = true;
  ed.error = "";
  ed.notice = "";
  renderEditorState();
  try {
    // 不传 message：说明由服务端按**真正改了的文件**生。客户端只知道现在开着哪个文件。
    // 发布成功 = HEAD 变了：publishWithRefresh 会重读这个文件，base 换成新 etag，
    // 基线（pending/diff）也跟着重取 —— 否则差异视图还在跟旧 HEAD 比，永远不归零。
    const typed = $("ed-text").value;
    const { result, file } = await publishWithRefresh(transport, { name: ed.name, path: ed.path });
    ed.pending.clear();
    if (file && ed.path) {
      ed.text = file.text;
      ed.base = file.etag;
      ed.lineCount = file.text.split("\n").length;
      if ($("ed-text").value === typed) $("ed-text").value = file.text;
      await loadBaseline(ed, ed.path);
    }
    const warnings = Array.isArray(result.warnings) ? result.warnings.filter(Boolean) : [];
    if (result.pushed === false) {
      // 提交在本地、远端没动 —— 最坏的一种「假成功」，必须红着说出来。
      ed.error = `本地已提交，但推送失败：\n${pushFailure(result)}\n点「发布」再试一次。`;
    } else if (result.commit) {
      ed.notice = `已发布 · ${short(result.commit.hash)} · ${result.commit.subject}${
        warnings.length ? `\n${warnings.join("\n")}` : ""
      }`;
    } else {
      ed.notice = warnings.length ? warnings.join("\n") : "没有待提交的改动。";
    }
    await load({ quiet: true });
  } catch (error) {
    ed.error = `发布失败：${error.message}`;
  } finally {
    state.publishing = false;
    renderEditorState();
  }
}

// ── 抽屉：行号槽 / 差异视图 / 改动概览 ─────────────────────────────────────
//
// 一份 diff（ed.diff）两个视图共用：编辑态拿它画左侧槽的变更标记和右侧概览条，
// 差异态拿它渲染整页。打字时 160ms 防抖重算 —— 槽上的颜色跟着手走，才有编辑器的感觉。

const ED_LINE = () => parseFloat(getComputedStyle($("ed-text")).lineHeight) || 20;

function recomputeDiff() {
  const ed = state.editor;
  if (!ed?.path) return;
  const text = $("ed-text").value;
  ed.lineCount = text.split("\n").length;
  ed.diff = buildDiff(ed.baseText ?? "", text, { context: 3 });
  renderGutter();
  renderDelta();
  renderOverview();
  if (ed.mode === "diff") renderDiffView();
}

function scheduleDiff() {
  const ed = state.editor;
  if (!ed) return;
  clearTimeout(ed.diffTimer);
  ed.diffTimer = setTimeout(recomputeDiff, 160);
}

function renderDelta() {
  const ed = state.editor;
  const chip = $("ed-delta");
  if (!ed?.diff) {
    chip.hidden = true;
    return;
  }
  const { adds, dels } = ed.diff.stats;
  chip.hidden = adds === 0 && dels === 0;
  $("ed-plus").textContent = `+${adds}`;
  $("ed-minus").textContent = `−${dels}`;
}

/** 左侧行号槽。行高必须和 textarea 完全一致，否则越滚越偏。 */
function renderGutter() {
  const ed = state.editor;
  const gutter = $("ed-gutter");
  const area = $("ed-text");
  if (!ed || !gutter) return;
  const total = Math.max(1, ed.lineCount);
  gutter.style.width = `${Math.max(2, String(total).length) * 7.5 + 26}px`;
  const frag = document.createDocumentFragment();
  for (let n = 1; n <= total; n++) {
    const mark = ed.diff?.marks?.get(n);
    const row = el("div", "gut" + (mark ? ` ${mark}` : ""));
    const gap = ed.diff?.gaps?.get(n);
    if (gap) {
      row.dataset.gap = String(gap);
      row.title = `这上面删掉了 ${gap} 行`;
    }
    row.append(el("span", "gut-n", String(n)));
    frag.append(row);
  }
  const tail = ed.diff?.gaps?.get(total + 1);
  if (tail) {
    const row = el("div", "gut gut-tail");
    row.dataset.gap = String(tail);
    row.title = `文件末尾删掉了 ${tail} 行`;
    frag.append(row);
  }
  gutter.textContent = "";
  gutter.append(frag);
  gutter.scrollTop = area.scrollTop;
}

function syncGutter() {
  const gutter = $("ed-gutter");
  const area = $("ed-text");
  if (gutter && area) gutter.scrollTop = area.scrollTop;
}

function foldRowEl(row) {
  const button = el("button", "dl-fold");
  button.type = "button";
  button.append(el("span", "dl-fold-icon", "⋯"));
  button.append(el("span", "", `展开 ${row.count} 行未改动`));
  button.onclick = () => {
    state.editor.expanded.add(`${row.from}-${row.to}`);
    renderDiffView();
  };
  return button;
}

/** 一行 → 元素数组：修改行拆成「旧行 + 新行」两行，词级碎片各自高亮 —— 这才读得懂。 */
function diffRowEls(row) {
  const line = (kind, oldNo, newNo, content) => {
    const node = el("div", `dl ${kind}`);
    if (newNo != null) node.dataset.new = String(newNo);
    const num = (value) => {
      const cell = el("span", "dl-n");
      cell.textContent = value == null ? "" : String(value);
      return cell;
    };
    node.append(num(oldNo));
    node.append(num(newNo));
    const code = el("span", "dl-code");
    if (typeof content === "string") code.textContent = content;
    else for (const seg of content) code.append(seg);
    if (!code.childNodes.length) code.append(" ");
    node.append(code);
    return node;
  };

  if (row.kind === "mod" && row.segs) {
    const side = (wanted) => {
      const parts = [];
      for (const seg of row.segs) {
        if (seg.type === "same") parts.push(seg.text);
        else if (seg.type === wanted) parts.push(el("span", `seg-${wanted}`, seg.text));
      }
      return parts;
    };
    return [line("mod-del", row.old, null, side("del")), line("mod-add", null, row.new, side("add"))];
  }
  return [line(row.kind, row.old, row.new, row.text ?? "")];
}

function renderDiffView() {
  const ed = state.editor;
  const box = $("ed-diff");
  if (!ed?.diff || !box) return;
  box.textContent = "";
  if (ed.diff.identical) {
    box.append(
      el(
        "p",
        "ed-diff-empty",
        ed.baseMissing ? "这个文件还没进仓库（HEAD 里没有），发布之后才有基线。" : "与 HEAD 一致 —— 这个文件没有改动。",
      ),
    );
    return;
  }
  const meta = el("div", "ed-diff-head");
  meta.append(el("span", "dh-path", ed.path));
  meta.append(el("span", "dh-note", ed.diff.coarse ? "文件较大，只做行级对比" : "与 HEAD 比"));
  meta.append(el("span", "dh-stat", `+${ed.diff.stats.adds} −${ed.diff.stats.dels}`));
  box.append(meta);

  const all = ed.diff.allRows ?? [];
  for (const row of ed.diff.rows) {
    if (row.kind !== "fold") {
      for (const node of diffRowEls(row)) box.append(node);
      continue;
    }
    const key = `${row.from}-${row.to}`;
    if (ed.expanded.has(key)) for (const inner of all.slice(row.from, row.to)) for (const node of diffRowEls(inner)) box.append(node);
    else box.append(foldRowEl(row));
  }
}

/** 右侧那根细条：改动在文件里的位置。点哪儿跳哪儿。 */
function renderOverview() {
  const ed = state.editor;
  const map = $("ed-map");
  if (!map) return;
  const all = ed?.diff?.allRows ?? [];
  if (!all.length || ed.diff.identical) {
    map.hidden = true;
    map.textContent = "";
    return;
  }
  map.hidden = false;
  map.textContent = "";
  const total = Math.max(1, all.length);
  for (let i = 0; i < all.length; ) {
    const kind = all[i].kind;
    if (kind === "ctx") {
      i++;
      continue;
    }
    let j = i;
    while (j < all.length && all[j].kind === kind) j++;
    const flag = el("div", `map-block ${kind}`);
    flag.style.top = `${(i / total) * 100}%`;
    flag.style.height = `${Math.max(1.6, ((j - i) / total) * 100)}%`;
    map.append(flag);
    i = j;
  }
}

function setEditorMode(mode) {
  const ed = state.editor;
  if (!ed) return;
  ed.mode = mode;
  const diff = mode === "diff";
  $("ed-code").hidden = diff;
  $("ed-diff").hidden = !diff;
  $("ed-tab-edit").classList.toggle("on", !diff);
  $("ed-tab-diff").classList.toggle("on", diff);
  $("ed-tab-edit").setAttribute("aria-selected", String(!diff));
  $("ed-tab-diff").setAttribute("aria-selected", String(diff));
  if (diff) {
    recomputeDiff();
    renderDiffView();
    $("ed-diff").focus();
  } else {
    renderGutter();
    $("ed-text").focus();
  }
}

/** Alt+↑/↓：跳到上一处 / 下一处改动。两个视图都给。 */
function gotoChange(dir) {
  const ed = state.editor;
  if (!ed?.diff) return;
  const lines = [...ed.diff.marks.keys()].sort((a, b) => a - b);
  if (!lines.length) return;

  if (ed.mode === "diff") {
    const box = $("ed-diff");
    const nodes = [...box.querySelectorAll(".dl.add, .dl.mod-add")];
    if (!nodes.length) return;
    const middle = box.scrollTop + box.clientHeight / 2;
    const after = nodes.find((node) => node.offsetTop > middle);
    const before = [...nodes].reverse().find((node) => node.offsetTop < middle);
    const target = dir > 0 ? after ?? nodes[0] : before ?? nodes[nodes.length - 1];
    target.scrollIntoView({ block: "center", behavior: "smooth" });
    return;
  }

  const area = $("ed-text");
  const caret = area.value.slice(0, area.selectionStart).split("\n").length;
  const line = dir > 0 ? lines.find((n) => n > caret) ?? lines[0] : [...lines].reverse().find((n) => n < caret) ?? lines[lines.length - 1];
  const textLines = area.value.split("\n");
  const start = textLines.slice(0, line - 1).join("\n").length + (line > 1 ? 1 : 0);
  area.focus();
  area.setSelectionRange(start, start + (textLines[line - 1]?.length ?? 0));
  area.scrollTop = Math.max(0, (line - 1) * ED_LINE() - area.clientHeight / 2);
  syncGutter();
}

// ── 新建技能 ──────────────────────────────────────────────────────────────
//
// 只问三样：名字、一句 description、类别（spec §6.5）。
//
// **这里不复刻语义闸（I8）。** 名字的字符集、撞已有目录、撞 `.archive/<名>`、撞仓库自己的
// 保留名（`index.json` 这类）全部由 qrd3wh 的 `POST v1/skills/create` 一处判定；这一页
// 只播报结果 —— 服务端的 400 原文直接进 `#create-err`（`api.js` 原样往上抛）。
// 本地多一套规则就是第二份真相：改了一边忘了另一边，两边就会说不同的话。
//
// 建完**不发布**：新技能的第一个正确状态是「未发布」（tracked: false）。

function validateCreate() {
  const name = $("create-name").value.trim();
  const description = $("create-description").value.trim();
  const root = state.data?.root ?? "技能仓库";

  // 空名字是唯一拦下来的东西：没名字连「建什么」都说不清。
  if (!name) return { ok: false, tone: "bad", message: `先起个名字。会建在 ${root}/&lt;名字&gt;/` };
  // 空说明只提醒，**不拦**：拦不拦是服务端的事（它会回 400 并把原因写在 #create-err 里）。
  if (!description) {
    return {
      ok: true,
      tone: "warn",
      message: "还差一句说明 —— 模型靠它决定用不用这条技能。空着提交的话服务端会拦。",
    };
  }
  return { ok: true, tone: "ok", message: `会建在 ${root}/<b>${name}</b>/` };
}

function renderCreate() {
  const verdict = validateCreate();
  const note = $("create-name-note");
  note.className = `create-note ${verdict.tone}`;
  note.innerHTML = verdict.message;
  $("create-ok").disabled = !verdict.ok || state.creating;
  $("create-where").textContent = "SKILL.md + nous.json";
  $("create-name").classList.toggle("bad", verdict.tone === "bad" && $("create-name").value.trim() !== "");
}

function openCreate() {
  closeMenu();
  state.creating = false;
  $("create").reset();
  $("create-err").hidden = true;
  // 当前正筛着某个类别，新建时就默认挂上去 —— 少点一次。
  const filter = state.filter && !state.filter.startsWith("__") ? state.filter : "";
  $("create-cat").value = filter;
  $("create-cats").replaceChildren(
    ...state.view.categoryNames.map((name) => {
      const option = el("option");
      option.value = name;
      return option;
    }),
  );
  $("create-scrim").hidden = false;
  $("create").hidden = false;
  renderCreate();
  $("create-name").focus();
}

function closeCreate() {
  $("create-scrim").hidden = true;
  $("create").hidden = true;
  state.creating = false;
}

async function submitCreate(event) {
  event.preventDefault();
  const verdict = validateCreate();
  if (!verdict.ok) return renderCreate();
  const name = $("create-name").value.trim();
  const description = $("create-description").value.trim();
  const category = normalizeCategory($("create-cat").value);
  state.creating = true;
  renderCreate();
  $("create-err").hidden = true;
  $("create-ok").textContent = "建…";
  try {
    await transport.post("v1/skills/create", {
      name,
      description,
      category: category ? [category] : [],
    });
    closeCreate();
    await load({ quiet: true });
    openDetail(name);
    toast(`已建 ${name} · 未发布（只有这台有）`);
  } catch (error) {
    $("create-err").textContent = String(error?.message ?? error);
    $("create-err").hidden = false;
  } finally {
    state.creating = false;
    $("create-ok").textContent = "建";
    if (!$("create").hidden) renderCreate();
  }
}

// ── 绑定 ───────────────────────────────────────────────────────────────────

// 顶栏那个「仓库读不了」的红 chip：只在 repo.error 时出现，常驻 DOM、靠 hidden 开关。
const repoChip = el("span", "chip-status");
repoChip.id = "repo-error";
repoChip.setAttribute("role", "status");
repoChip.hidden = true;
$("repo").before(repoChip);

$("search").oninput = (event) => {
  state.query = event.target.value;
  state.detail = null;
  state.detailBody = null;
  state.editing = null;
  $("grid").hidden = false;
  $("detail").hidden = true;
  renderGrid();
};
$("refresh").onclick = () => void load();
$("new-skill").onclick = () => openCreate();

// 命令行里 `git pull` / 手改 SKILL.md 之后，内存名册和盘会分叉 —— 因为 `render()` 只查内存。
// 这不是 bug 是设计（每轮提示词都不许碰盘），代价就是需要一个显式的重扫按钮。
$("rescan").onclick = async () => {
  try {
    await transport.post("v1/skills/reload", {});
    await load({ quiet: true });
    toast(`已重扫：${state.view?.skillCount ?? 0} 条`);
  } catch (error) {
    toast(`重扫失败：${error.message}`, "bad");
  }
};
$("create-cancel").onclick = () => closeCreate();
$("create-scrim").onclick = () => closeCreate();
$("create").addEventListener("submit", submitCreate);
for (const id of ["create-name", "create-description", "create-cat"]) {
  const node = $(id);
  node.addEventListener("input", renderCreate);
  node.addEventListener("change", renderCreate);
}
// 回车一格一格往下走：名字 → 说明 → 类别 → 建。别在第一格就提交。
for (const [id, next] of [
  ["create-name", "create-description"],
  ["create-description", "create-cat"],
]) {
  $(id).addEventListener("keydown", (event) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    $(next).focus();
  });
}

// 抽屉的交互：Esc 关、Ctrl/⌘+S 保存、Tab 缩进两格（不跳出编辑框）。
$("ed-close").onclick = () => closeEditor();
$("ed-scrim").onclick = () => closeEditor();
$("ed-save").onclick = () => void saveEditorFile();
$("ed-publish").onclick = () => void publishEditor();
$("ed-text").addEventListener("input", renderEditorState);
$("ed-text").addEventListener("input", scheduleDiff);
$("ed-text").addEventListener("scroll", syncGutter);
$("ed-tab-edit").onclick = () => setEditorMode("edit");
$("ed-tab-diff").onclick = () => setEditorMode("diff");
$("ed-map").onclick = (event) => {
  const ed = state.editor;
  if (!ed?.diff) return;
  const rect = $("ed-map").getBoundingClientRect();
  const at = Math.min(1, Math.max(0, (event.clientY - rect.top) / rect.height));
  const box = ed.mode === "diff" ? $("ed-diff") : $("ed-text");
  box.scrollTop = at * (box.scrollHeight - box.clientHeight);
  syncGutter();
};
$("ed-text").addEventListener("keydown", (event) => {
  if (event.key === "Tab" && !event.metaKey && !event.ctrlKey && !event.altKey) {
    event.preventDefault();
    const area = event.target;
    const { selectionStart: start, selectionEnd: end } = area;
    area.value = area.value.slice(0, start) + "  " + area.value.slice(end);
    area.selectionStart = area.selectionEnd = start + 2;
    renderEditorState();
    return;
  }
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
    event.preventDefault();
    void saveEditorFile();
  }
});

// Esc 的层级：菜单 → 新建弹层 → 抽屉 → 详情 → 列表。
// 菜单排最前，因为它是最后一次交互；不这么排的话，Esc 关菜单会顺手把背后的详情也关了。
addEventListener("keydown", (event) => {
  if (event.key !== "Escape") {
    if (state.editor && event.altKey && event.key.toLowerCase() === "d") {
      event.preventDefault();
      setEditorMode(state.editor.mode === "diff" ? "edit" : "diff");
    } else if (state.editor && event.altKey && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
      event.preventDefault();
      gotoChange(event.key === "ArrowDown" ? 1 : -1);
    }
    return;
  }
  if (menuOpen) return closeMenu();
  if (!$("danger").hidden) return;
  if (!$("create").hidden) return closeCreate();
  if (state.editor) {
    if (state.editor.mode === "diff") setEditorMode("edit");
    else closeEditor();
    return;
  }
  // 详情是主区、不是浮层：Esc 退回列表。
  if (state.detail) closeDetail();
});

// 有没保存的字就别让浏览器静默关掉这一页。
addEventListener("beforeunload", (event) => {
  // 攒着的类别能抢救一下就抢救（不等结果：这一页已经在走了）。
  if (categoryWrites.hasPending()) void categoryWrites.flush();
  if (!editorDirty()) return;
  event.preventDefault();
  event.returnValue = "";
});

// 切到后台 / 藏起来的时候也把攒着的类别发掉 —— 手机上切走再回来常常就是「没了」。
addEventListener("visibilitychange", () => {
  if (document.visibilityState === "hidden" && categoryWrites.hasPending()) void categoryWrites.flush();
});

load();

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
  dirPaths,
  fileTree,
  filterCards,
  flattenTree,
  isUnder,
  leafOf,
  normalizeCategory,
  stateFilter,
  stateFilterOf,
  repoNameFromUrl,
  suggestedAdoptName,
  upstreamSummary,
  versionLine,
  versionText,
} from "./model.js";
import { CONFLICT_MESSAGE, createApi, createCategoryDebounce, publishWithRefresh } from "./api.js";

/**
 * 这一块在宿主里的根（`/p/new1/68kazk/`）。**不是** `location.pathname` ——
 * 一屏一个真路径之后，站在 `/upstream` 上时页面路径已经不等于块的根了，
 * 拿它拼 `__nous/ctx` 会去问 `/upstream/__nous/ctx`（宿主会说不认识）。
 *
 * 两个来源：宿主注入的 `window.__nous_self`、或者 `<base href>`（同一个数）。都没有
 * （裸跑、测试）才退回「按当前路径算」，此时路径本来就是根。
 */
const SELF = (() => {
  const injected = typeof window === "undefined" ? null : window.__nous_self;
  const fromBase =
    typeof document === "undefined" ? null : document.querySelector("base")?.getAttribute("href") ?? null;
  const fallback = location.pathname.endsWith("/") ? location.pathname : location.pathname + "/";
  const path = String(injected || fromBase || fallback);
  return path.endsWith("/") ? path : `${path}/`;
})();
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
  duplicating: null, // 复制弹层开着的话，这里是源技能的名字
  duplicatingVariant: null, // 从变体那棵树拷（「独立成技能 ›」）时是变体 id
  variant: null, // 变体弹层：{skill, item}（item 为空 = 新增）
  createPrefix: "", // 「新建子类别」预填的 `工具/`
  renaming: null, // 正在原地改名的类别
  editing: null, // 正在输入类别的技能名（只住在详情里）
  detail: null, // 详情视图里是哪条技能；null = 列表
  detailBody: null, // 详情里那份 SKILL.md：{name, loading, text, error}
  editor: null, // 正文抽屉
  publishing: false,
  pending: null, // GET v1/skills/pending 的原样（未同步的改动单元）
  pendingOpen: false, // 未同步改动那扇浮层开着
  pendingMessage: "", // 发布时那条 commit message
  upstream: null, // GET v1/skills/upstream 的原样（订阅 + 清单 + 每个 source 的报错）
  upstreamOpen: false, // 主区顶上来的是上游这块视图（不是筛出来的列表）
  upstreamSource: null, // 在看哪个订阅

  upstreamDetail: null, // 哪一条上游技能开着浮层
  upstreamBusy: null, // 正在取用 / 刷新 / 添加（按钮上的转圈）
  addingSource: false, // 添加上游那个表单开着
};

const skillNow = (name) => (state.data?.skills ?? []).find((item) => item.name === name) ?? null;

/**
 * `code-review@strict` → `{family: "code-review", id: "strict"}`；本体就是 `{family, id: null}`。
 *
 * 变体在名册里的键是 `本名@id`（会话里存的就是它），但**接口按两个参数走**：
 * `name=本名&variant=id`。跟扩展那边一笔不差 —— `repoDir()` 认的是目录名，`@` 那一半只是「哪棵树」。
 */
const VARIANT_SEP = "@";
const splitSkillKey = (name) => {
  const text = String(name ?? "");
  const at = text.indexOf(VARIANT_SEP);
  return at <= 0 ? { family: text, id: null } : { family: text.slice(0, at), id: text.slice(at + 1) };
};

/** 带变体的请求多一个参数：`variant=strict`。没有变体就原样返回（别多送一个空串）。 */
const withVariant = (query, id) => (id ? `${query}&variant=${encodeURIComponent(id)}` : query);
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
    // 顶栏那颗 sync chip 的数要它；不挡住首屏，回来再画一次。
    void loadPending();
  } catch (error) {
    dot.dataset.state = "bad";
    state.pending = null;
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

  // 变体：同一条技能的另一个可装载名字。本体才有这一项 —— 变体自己再开变体没有意义
  // （要那样就该「独立成一个新技能」）。
  menuItem(menu, "变体…", "另一条分支，另一个名字", () => openVariantDialog(skill, null));

  menu.append(el("div", "ctx-sep"));
  // 复制 = 整目录拷成新的一条（`POST v1/skills/duplicate`）：正文、附件、类别、summary
  // 全跟着走，名字换成新的，出来是**未发布**。历史不带 —— 旧提交留在原来那条里。
  menuItem(menu, "复制成新技能", "换名字，历史不带", () => openDuplicate(skill.name));

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
  if (skill.variant) {
    const flag = el("span", "product-hidden", "变体");
    flag.title = `${skill.variant.family} 的变体「${skill.variant.id}」：正文在 skill/${skill.variant.family}-${skill.variant.id} 上`;
    title.append(flag);
  }

  // 卡上唯一可见的菜单入口：桌面 hover 出现、触屏常驻。
  const menuBtn = el("button", "product-menu", "⋯");
  menuBtn.type = "button";
  menuBtn.title = `${skill.name} 的菜单：正文 / 类别 / 复制 / 归档`;
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
      state.upstreamOpen = false; // 换筛选就是回列表（上游那块视图让位）
      leaveDetail();
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

  // 「上游」跟着这一排走，但它不是筛选：点了是把上游那块**视图**顶上来。
  // 一条订阅都没有时也要在 —— 否则第一次加订阅没有入口。
  {
    const up = upstreamSummary(state.upstream?.entries ?? []);
    // 是 `<a href>`，不是 `<button>`：它指向的是这一块里的**另一屏**，得能右键复制、
    // 能 cmd-click 开新标签。普通左键才在同一个标签里换屏（不重载）。
    const upChip = el("a", "filter-chip" + (state.upstreamOpen ? " on" : ""));
    upChip.href = `${SELF}upstream`;
    upChip.append(el("span", "filter-label", "上游"));
    const n = el("span", "filter-n", state.upstream ? (up.todo ? String(up.todo) : "—") : "…");
    if (up.todo) n.dataset.tone = "warn";
    upChip.append(n);
    upChip.title = "别的 git 仓库里的技能：看清单、取用过来（不会自动同步）";
    upChip.onclick = (event) => {
      // 带修饰键 / 中键 = 人想开新标签，别拦。
      if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0) return;
      event.preventDefault();
      if (state.upstreamOpen) closeUpstream();
      else openUpstream();
    };
    box.append(upChip);
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
  if (state.upstreamOpen) return renderUpstreamFacets();
  $("rail-title").textContent = "类别";
  $("rail-new").hidden = false;
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
  // 半扇浮层：列表不动（滚动位置、筛选、搜索词都留着），滚的是抽屉自己。
  $("dt-body").scrollTop = 0;
  // 带「类别」进来时焦点要落在输入框上（renderDetail 已经给了），别被抽屉抢走。
  if (!editTags) $("detail").focus({ preventScroll: true });
}

function closeDetail({ restoreFocus = true } = {}) {
  if (!state.detail) return;
  const name = state.detail;
  leaveDetail();
  render();
  if (!restoreFocus) return;
  // 焦点还回那张卡：卡片是重画过的，所以按名字重新找一遍，不存元素。
  const card = [...document.querySelectorAll(".product")].find((node) => node.dataset.name === name);
  card?.focus({ preventScroll: true });
}

function renderDetail() {
  const host = $("dt-body");
  const skill = skillNow(state.detail);
  // 详情开着的时候这条被归档 / 删走了：把浮层收掉，别留一面空墙。
  if (!skill) {
    leaveDetail();
    $("detail").hidden = true;
    $("dt-scrim").hidden = true;
    renderGrid();
    setBehindInert();
    return;
  }

  host.textContent = "";
  const cats = categoriesOf(skill);

  const card = state.view.cards.find((item) => item.name === skill.name);
  const stateName = card?.state ?? "ok";
  const failed = stateName === "failed";
  const unpublished = stateName === "unpublished";

  // ── 抽屉头：色砖 + 名字 + 状态 + 类别 ────────────────────────────────
  $("dt-name").textContent = skill.name;
  $("dt-meta").textContent = cats.length ? cats.join(" · ") : "未分类";
  $("dt-tile").style.setProperty("--hue", String(hueOf(skill.name)));
  $("dt-mono").textContent = initialsOf(skill.name);
  const badges = $("dt-badges");
  badges.textContent = "";
  const badge = el("span", "detail-badge", STATE_LABEL[stateName] ?? stateName);
  badge.dataset.tone = failed ? "fail" : unpublished ? "unpub" : "ok";
  badges.append(badge);
  if (skill.hidden) {
    const hidden = el("span", "detail-badge", "不进提示词");
    hidden.dataset.tone = "gone";
    hidden.title = "disable-model-invocation：模型看不到它";
    badges.append(hidden);
  }

  // ── 说明 + 入口 ───────────────────────────────────────────────────────
  host.append(el("p", "detail-sum", skill.description || "没写说明。"));

  const acts = el("div", "detail-acts");
  const body = el("button", "btn btn-small", "正文 ›");
  body.type = "button";
  body.title = `看 / 改 ${skill.name}/SKILL.md`;
  body.onclick = () => void openEditor(skill);
  acts.append(body);
  // 复制也开在这里：卡上那个 `⋯` 是快捷入口，详情里得有一个看得见的。
  const dup = el("button", "btn btn-small", "复制成新技能");
  dup.type = "button";
  dup.title = `整目录拷成另一条技能（POST v1/skills/duplicate）—— 历史不带，出来是未发布`;
  dup.onclick = () => openDuplicate(skill.name);
  acts.append(dup);
  host.append(acts);

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

  // 变体：本体详情里的一行一行（跟扩展面板同一个分组法）。变体的卡不另开，
  // 但点它自己的那一行照样能开它的抽屉 —— 正文、差异、发布全在它那棵树上。
  const family = splitSkillKey(skill.name).family;
  const variants = state.data?.variants?.[family] ?? [];
  const varSection = detailSection("变体", variants.length ? `${variants.length} 个可装载名字` : "另一个可装载名字");
  varSection.body.append(variantStrip(skill, variants));
  grid.append(varSection.node);

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
  // 变体：正文在它自己那条分支上，报的是那条分支的名字 —— 说成「已进 HEAD」会让人以为在主干上。
  if (skill.variant) {
    where.body.append(
      detailRow(
        "变体",
        `${skill.variant.family}@${skill.variant.id}`,
        `分支 skill/${skill.variant.family}-${skill.variant.id} · 树在仓的隔壁`,
      ),
    );
  }
  where.body.append(detailRow("发布", skill.tracked === false ? "未发布 · HEAD 里没有" : "已进 HEAD"));
  where.body.append(detailRow("路径", skill.path ?? skill.name));
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

/**
 * 浮层开着的时候，背后那一层不接焦点、也不接点击（Tab 不会跑到遮罩后面去）。
 * 只留这一处开关，因为两层会套在一起：详情在下面、正文抽屉在上面 ——
 * 关掉上面那层，还得看下面那层在不在。
 */
function setBehindInert() {
  const layout = document.querySelector(".layout");
  if (layout) layout.inert =
    Boolean(state.detail || state.upstreamDetail || state.pendingOpen) || !$("editor").hidden;
}

function render() {
  if (!state.data) return;
  closeMenu();

  renderRepo();
  renderSync();
  renderFilters();
  renderFacets();
  renderRailNew();
  renderSuggestions();

  // 上游占主区：网格让位，而不是被挤窄。
  // 详情是浮层：网格不隐身 —— 它就在遮罩下面，关掉抽屉原样还在。
  $("grid").hidden = Boolean(state.upstreamOpen);
  // 工具栏跟着换班：上游那一屏没有技能可建、也没东西可搜。
  $("toolbar-grid").hidden = Boolean(state.upstreamOpen);
  $("toolbar-up").hidden = !state.upstreamOpen;
  $("upstream").hidden = !state.upstreamOpen;
  const sheet = Boolean(state.detail || state.upstreamDetail || state.pendingOpen);
  $("detail").hidden = !sheet;
  $("dt-scrim").hidden = !sheet;
  if (state.upstreamOpen) renderUpstreamPanel();
  else {
    renderGrid();
    // 详情重画在网格之后：两边读同一份 view，但 result-count 归网格写。
    if (state.detail) renderDetail();
  }
  // 「未同步的改动」也住在这扇浮层里。
  if (state.pendingOpen) renderPending();
  // 上游那条技能也住在同一扇浮层里 —— 上面那条分支不管浮层，这里统一画。
  if (state.upstreamDetail && !renderUpstreamDetail()) {
    state.upstreamDetail = null;
    $("detail").hidden = true;
    $("dt-scrim").hidden = true;
  }

  $("count").textContent = `${state.view.skillCount} 条技能${state.view.archived.length ? ` · ${state.view.archived.length} 条归档` : ""}`;
  setBehindInert();
}

// ── 上游 ────────────────────────────────────────────────────────────────────
//
// 上游就是另一个 git remote。这块是一块**视图**，不是筛选 —— 顶部那排 chip 里的
// 「上游」把它顶上来：列的不是我们仓库里的东西，而是「别人仓库里有什么、要不要拿过来」。
//
// **它跟本地那个网格长同一个样子**：别人的仓也是多级目录（`plugins/x/skills/y`），
// 所以这里是「左栏目录树 + 网格 + 半扇浮层」，不是一列条目配一排按钮。
// 一个订阅 = 一张卡：那不是「39 张卡」，是**一捆技能**（那个仓里带 SKILL.md 的全部）。
// 点开是浮层，看正文是抽屉（那一捆的文件树）—— 仓里跟技能无关的东西不进来。
//
// 三个动作，边界很清楚：
//   1. 看清单 / 钻目录 —— 读本地镜像，**打开页面不联网**；
//   2. 取用 —— POST adopt，落进我们仓里，仍然是未发布；
//   3. 刷新 / 添加上游 —— 刷新是唯一联网动作；添加只是往 `sources.json` 里写一行（未发布）。
//
// **没有自动同步。** 上游动了只标「上游有更新」，跟不跟是人的事。

async function loadUpstream({ quiet = true } = {}) {
  try {
    if (!transport) await connect();
    state.upstream = await transport.get("v1/skills/upstream");
  } catch (error) {
    state.upstream = { sources: [], entries: [], errors: [], error: String(error?.message ?? error) };
    if (!quiet) toast(`读不到上游清单：${error.message}`, "bad");
  }
  // 顶部那颗 chip、左栏那棵树、主区那些卡片都是这份数据算的，所以整页重画一次（很便宜）。
  if (state.data) render();
}

const upstreamEntriesOf = (sourceName) =>
  (state.upstream?.entries ?? []).filter((entry) => entry.source === sourceName);

const upstreamSourceNow = () =>
  (state.upstream?.sources ?? []).find((item) => item.name === state.upstreamSource) ?? null;

// ── 地址 ────────────────────────────────────────────────────────────────────
//
// 真路径，不是 hash。这一块的正身就是**技能清单**，所以正身住 `/`：
//
//   /            技能（这块用来做的事）
//   /upstream    上游（独立功能分支，自己的一个链接）
//   /upstream/<订阅>  顺带选中那条订阅
//   /pending     未同步的改动
//
// **地址只到订阅**：一个订阅 = 一条「技能」（一捆），所以地址里没有「哪一条」。
// 目录更不进地址 —— 进了就是「钻进去换一页」，那是文件管理器，不是这一页的规矩。
//
// 服务端认「没扩展名的路径 = 路由」（见 server.js），所以这些链接刷新、贴给人、
// cmd-click 开新标签都对得上。

/** 路径 → 这一屏。多出来的段只当没看见（昨天贴出去的深链接不该变成白屏）。 */
const routeOf = (pathname = location.pathname) => {
  const rel = pathname.startsWith(SELF) ? pathname.slice(SELF.length) : pathname.replace(/^\/+/, "");
  const [head = "", ...rest] = rel.split("/").filter(Boolean);
  if (head === "upstream") return { view: "upstream", source: rest[0] ?? null };
  if (head === "pending") return { view: "pending" };
  return { view: "skills" };
};

/** 这一屏 → 路径（相对块根）。正身就是空串 —— `/` 不是 `/skills`。 */
const pathOf = ({ view = "skills", source = null } = {}) => {
  if (view === "upstream") return `upstream${source ? `/${encodeURIComponent(source)}` : ""}`;
  if (view === "pending") return "pending";
  return "";
};

/** 换地址。`replace` = 这一下不该多一条历史（首屏纠正老链接时用）。 */
const writePath = (route, { replace = false } = {}) => {
  const url = SELF + pathOf(route);
  if (url === location.pathname) return;
  history[replace ? "replaceState" : "pushState"](null, "", url);
};

/**
 * 老链接照样认：`#upstream/skills`、`#pending` 换成真路径，页面不重载。
 * 地址是可分享的，昨天贴出去的不能变成白屏。
 */
const migrateHash = () => {
  const raw = location.hash.replace(/^#/, "");
  if (!raw) return;
  const [head, ...rest] = raw.split("/").filter(Boolean);
  if (head !== "upstream" && head !== "pending") return;
  history.replaceState(
    null,
    "",
    SELF + pathOf({ view: head, source: head === "upstream" ? rest[0] ?? null : null }),
  );
};

/** 地址变了（后退 / 前进 / 直接开的那一条）→ 把那屏画出来。只画，不写地址。 */
function applyRoute() {
  const route = routeOf();
  if (route.view === "upstream") {
    openUpstream(route.source ?? undefined, { silent: true });
    // 订阅名不存在（比如改名之前的 `/upstream/matt`）→ 落到第一个，但**地址也跟着换过去**
    // （replace，不留历史）：不然地址说 matt、屏幕是别的，复制出去还是坏的。
    writePath({ view: "upstream", source: state.upstreamSource }, { replace: true });
    return;
  }
  if (state.upstreamOpen) closeUpstream({ silent: true });
  if (route.view === "pending") {
    if (!state.pendingOpen) openPending({ silent: true });
    return;
  }
  if (state.pendingOpen) closePending({ silent: true });
  // 认不出来的路径就当技能清单，顺手把地址摆正 —— 别让人对着一个死路径刷新。
  writePath({ view: "skills" }, { replace: true });
}

addEventListener("popstate", () => applyRoute());

function openUpstream(name = undefined, { silent = false } = {}) {
  leaveDetail();
  state.upstreamDetail = null;
  state.upstreamOpen = true;
  // 没说看哪个就落到第一个订阅；一个都没有就停在「添加上游」那张空状态上。
  // 说了一个不存在的（比如改名之前的 `#upstream/matt`）也落到第一个 ——
  // 地址换了是订阅名换了，不是让人对着空屏发呆。
  const names = (state.upstream?.sources ?? []).map((item) => item.name);
  const known = name && names.includes(name) ? name : null;
  state.upstreamSource = known ?? state.upstreamSource ?? names[0] ?? null;
  if (!names.includes(state.upstreamSource)) state.upstreamSource = names[0] ?? null;
  if (!silent) writePath({ view: "upstream", source: state.upstreamSource });
  render();
  $("results")?.scrollTo({ top: 0 });
}

function closeUpstream({ silent = false } = {}) {
  if (!state.upstreamOpen) return;
  state.upstreamOpen = false;
  state.upstreamDetail = null;
  if (!silent) writePath({ view: "skills" });
  render();
  $("results")?.scrollTo({ top: 0 });
}

function switchSource(name) {
  state.upstreamSource = name;
  state.upstreamDetail = null;
  writePath({ view: "upstream", source: name });
  render();
  $("results")?.scrollTo({ top: 0 });
}

// ── 会改仓库 / 会联网的三个动作 ──────────────────────────────────────────────

async function refreshSource(name) {
  if (state.upstreamBusy) return;
  state.upstreamBusy = `fetch:${name}`;
  render();
  try {
    const out = await transport.post("v1/skills/upstream/refresh", { name });
    const bad = (out.sources ?? []).find((item) => !item.ok);
    if (bad) throw new Error(bad.error);
    toast(`已刷新 ${name}`);
  } catch (error) {
    toast(`刷新 ${name} 失败：${error.message}`, "bad");
  } finally {
    state.upstreamBusy = null;
    await loadUpstream({ quiet: true });
  }
}

/** 问一句上游动没动（只读 refs，不拉内容）。结果服务端缓存起来，卡片靠它提示。 */
async function checkSource(name) {
  if (state.upstreamBusy) return;
  state.upstreamBusy = `check:${name}`;
  render();
  try {
    const out = await transport.post("v1/skills/upstream/check", { name });
    const row = (out.sources ?? [])[0];
    if (!row?.ok) throw new Error(row?.error ?? "检查失败");
    toast(row.newer ? `上游更新了：${versionText(row.version) || row.head.slice(0, 7)}` : "已经是最新的了");
  } catch (error) {
    toast(`检查更新失败：${error.message}`, "bad");
  } finally {
    state.upstreamBusy = null;
    await loadUpstream({ quiet: true });
  }
}

/** 同步 = 拉下来 + 认这一版。一个动作，因为是同一个决定：「跟上游走」。 */
async function syncSource(name) {
  if (state.upstreamBusy) return;
  state.upstreamBusy = `sync:${name}`;
  render();
  try {
    const out = await transport.post("v1/skills/upstream/sync", { source: name });
    toast(`已同步到 ${versionText(out.version) || String(out.head ?? "").slice(0, 7)}`);
  } catch (error) {
    toast(`同步失败：${error.message}`, "bad");
  } finally {
    state.upstreamBusy = null;
    await loadUpstream({ quiet: true });
  }
}

/**
 * 把「我们认的版本」钉到上游现在这一版。
 *
 * **只有这一个动作会改 `version`。** 刷新只采集，不动它 —— 不然「记录」自己就会跑，
 * 人也就没法说「我认的是这一版」。
 */
async function pinSourceVersion(source) {
  if (state.upstreamBusy) return;
  state.upstreamBusy = `pin:${source.name}`;
  render();
  try {
    const out = await transport.post("v1/skills/upstream/pin", { source: source.name });
    toast(`已记下 ${source.name} @ ${String(out.version?.commit ?? "").slice(0, 7)}`);
  } catch (error) {
    toast(`记录版本失败：${error.message}`, "bad");
  } finally {
    state.upstreamBusy = null;
    await loadUpstream({ quiet: true });
  }
}

/** 加一个上游 = 往 sources.json 里写一行，**不提交** —— 它跟技能一样是内容，走发布。 */
async function addSource({ url, prefix }) {
  const cleanUrl = String(url ?? "").trim();
  if (!cleanUrl) {
    toast("先填个地址", "bad");
    return;
  }
  const name = repoNameFromUrl(cleanUrl);
  if (!name) {
    toast("认不出这个地址 —— 要 https://… / git@host:… / 绝对路径", "bad");
    return;
  }
  const existing = state.upstream?.sources ?? [];
  if (existing.some((source) => source.name === name)) {
    toast(`已经订阅过一个叫 ${name} 的仓了 —— 两个仓同名才需要人插手，先订阅那个，再回来给它加前缀`, "bad");
    return;
  }
  state.upstreamBusy = "add";
  render();
  try {
    // 整份替换（POST /v1/skills/sources 的语义）：把现有的原样带回去，别把它们弄丢。
    const next = [
      ...existing.map(({ name: n, url: u, prefix: p, branch: b }) => ({
        name: n,
        url: u,
        ...(p ? { prefix: p } : {}),
        ...(b ? { branch: b } : {}),
      })),
      { name, url: cleanUrl, ...(prefix ? { prefix: String(prefix).trim() } : {}) },
    ];
    await transport.post("v1/skills/sources", { sources: next });
    await loadUpstream({ quiet: true });
    state.upstreamSource = name;
    state.addingSource = false;
    toast(`订阅了 ${name}（未发布）—— 顺手拉一下镜像`);
    await refreshSource(name);
  } catch (error) {
    toast(`加不上：${error.message}`, "bad");
  } finally {
    state.upstreamBusy = null;
    render();
  }
}

async function adoptEntry(entry, name) {
  if (state.upstreamBusy) return;
  const target = String(name ?? "").trim() || entry.name;
  state.upstreamBusy = `adopt:${entry.source}/${entry.path}`;
  render();
  try {
    await transport.post("v1/skills/upstream/adopt", { source: entry.source, path: entry.path, name: target });
    toast(`已取用 ${target} —— 现在是未发布，去正文改完再发布`);
    state.upstreamDetail = null;
    await load({ quiet: true });
    await loadUpstream({ quiet: true });
  } catch (error) {
    toast(`取用失败：${error.message}`, "bad");
  } finally {
    state.upstreamBusy = null;
    render();
  }
}

const upstreamBusyOn = (entry) => state.upstreamBusy === `adopt:${entry.source}/${entry.path}`;

/** 上游那张卡的菜单：看正文 / 刷新 / 复制路径。别人的仓，没有删除。 */
function openUpstreamMenu(x, y, source) {
  closeMenu();
  const menu = $("ctxmenu");
  menu.textContent = "";
  menu.hidden = false;
  menuOpen = true;

  const mine = upstreamSummary(upstreamEntriesOf(source.name));
  menuHead(menu, source.name, `上游 · ${mine.total} 条技能 · 只读`);
  const first = menuItem(menu, "看正文", "技能的文件树 · 只读", () => void openUpstreamEditor(source));
  menuItem(menu, "检查更新", "只问远端 refs，不下载", () => void checkSource(source.name));
  if (source.newer) {
    menuItem(menu, "更新到最新", `${versionText(source.remote?.version) ?? ""} · 拉下来 + 认这一版`, () => void syncSource(source.name));
  }
  menuItem(menu, "刷新", "git fetch（拉镜像，不认版本）", () => void refreshSource(source.name));
  if (source.drift) {
    menuItem(menu, "记录版本", `${source.head?.slice(0, 7) ?? ""} · 把现在这一版记下来`, () => void pinSourceVersion(source));
  }
  // **本机那个目录**，不是 GitHub 地址：粘出来能直接 `cd` 进去 `ls` / `cat` / grep。
  menuItem(menu, "复制路径", source.dir ?? "", () => {
    const where = source.dir ?? "";
    navigator.clipboard?.writeText(where).then(
      () => toast("本机路径已复制"),
      () => toast(where),
    );
  });
  mountMenu(menu, x, y, first);
}

/** 浮层里的正文：一个订阅那一层的事（仓库 / 路径 / 多少条 / 动作）。 */
/** `1.4 MB` / `812 KB` / `96 B` —— 盘上的数，别摆一串原始字节。 */
function bytesText(bytes) {
  const n = Number(bytes) || 0;
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

function upstreamDetailBody(source) {
  const wrap = el("div", "up-detail");
  const entries = upstreamEntriesOf(source.name);
  const mine = upstreamSummary(entries);
  // 名字就是仓库名（`skills`），地址另外一行 —— 这两样以前挤在一起，看着像个用户名。
  wrap.append(detailRow("仓库", source.name, source.url));
  if (source.prefix) wrap.append(detailRow("只看", `${source.prefix}/`, "仓里的这一段才算技能"));
  wrap.append(detailRow("技能", `${mine.total} 条${mine.todo ? ` · ${mine.todo} 条待看` : ""}`));
  // 两个版本并排摆：左边是**我们认的**（sources.json 里那份，进版本库），
  // 右边是**采集到的**（镜像现在停在哪）。不一样就是上游动了，这是唯一诚实的说法。
  const pinned = detailRow("版本", versionLine(source.version) || "还没记录",
    "我们认的那一版：记在 sources.json 里，进版本库。只有「记录这个版本」能改它。`+` 表示在最近一个 tag 之后（上游天天提交，tag 一周一次）。");
  pinned.querySelector(".detail-row-v")?.classList.add("mono");
  wrap.append(pinned);
  if (source.head) {
    const now = detailRow(
      "上游现在",
      versionText(source.headVersion) || source.head.slice(0, 7),
      "镜像现在停在哪一版（上次刷新拉到的）",
    );
    now.querySelector(".detail-row-v")?.classList.add("mono");
    if (source.drift) {
      now.querySelector(".detail-row-v")?.classList.add("up-drift");
      wrap.append(alertLine("warn", "上游往前走了", "采集到的版本跟记录的不一样。想跟新版就把「记录这个版本」按下去；不想动就放着 —— 它不会自己改。"));
    }
    wrap.append(now);
  }

  // 「上游最新」= 上次检查问到的远端。没有这项就是没检查过（我们不会偷偷联网）。
  // 顺序按「越靠上越该先看」：认的 → 镜像现在 → 上游现在（最新）。
  if (source.remote) {
    const latest = detailRow(
      "上游最新",
      versionText(source.remote.version) || source.remote.head.slice(0, 7),
      `上次检查：${new Date(source.remote.at).toLocaleString()}`,
    );
    latest.querySelector(".detail-row-v")?.classList.add("mono");
    if (source.newer) latest.querySelector(".detail-row-v")?.classList.add("up-drift");
    wrap.append(latest);
    if (source.newer) {
      wrap.append(
        alertLine(
          "warn",
          "上游有更新",
          "远端已经往前走了。按「更新到最新」= 拉下来 + 认这一版，一步做完；只想看看就按「刷新」，镜像会更新但认的版本不动。",
        ),
      );
    }
  }
  // 「本机到底存没存、存在哪、存了多大」是个人会问的问题，所以三句都直接答。
  // 路径那一行是可点的：点一下复制（就是「复制路径」给的那个东西）。
  if (source.mirror === "ready") {
    const where = detailRow("本机路径", source.dir ?? "", "只装技能，没有别人的 README / .github —— 点一下复制");
    where.classList.add("detail-path");
    where.tabIndex = 0;
    where.title = "点一下复制这个路径";
    where.onclick = () => {
      navigator.clipboard?.writeText(source.dir ?? "").then(
        () => toast("本机路径已复制"),
        () => toast(source.dir ?? ""),
      );
    };
    wrap.append(where);
    wrap.append(detailRow("占用", bytesText(source.bytes), "删了下次 fetch 从源头重建，不是第二份真相"));
  }
  if (source.mirror === "missing") {
    wrap.append(alertLine("warn", "还没镜像", "点「刷新（fetch）」把它拉下来。第一次要联网。"));
  }

  wrap.append(
    el("p", "detail-sum", "取用不在这儿：进正文，找到那条的 SKILL.md，行尾有「取用」—— 一个仓里是一条一条拿的。"),
  );

  const acts = el("div", "detail-sec-acts");
  const read = el("button", "btn btn-small", "看正文 ›");
  read.type = "button";
  read.title = "技能的文件树：带 SKILL.md 的目录 + 目录里的文件（只读）";
  read.onclick = () => void openUpstreamEditor(source);
  acts.append(read);

  if (source.newer) {
    const update = el(
      "button",
      "btn btn-small",
      state.upstreamBusy === `sync:${source.name}` ? "同步中…" : "更新到最新",
    );
    update.type = "button";
    update.title = "拉下来 + 把「认的版本」推到最新（写进 sources.json，走发布）";
    update.disabled = Boolean(state.upstreamBusy);
    update.onclick = () => void syncSource(source.name);
    acts.append(update);
  } else {
    const check = el(
      "button",
      "btn btn-small",
      state.upstreamBusy === `check:${source.name}` ? "检查中…" : "检查更新",
    );
    check.type = "button";
    check.title = "问一句远端，不下载任何东西";
    check.disabled = Boolean(state.upstreamBusy);
    check.onclick = () => void checkSource(source.name);
    acts.append(check);
  }

  const sync = el("button", "btn btn-small", state.upstreamBusy === `fetch:${source.name}` ? "刷新中…" : "刷新（fetch）");
  sync.type = "button";
  sync.disabled = Boolean(state.upstreamBusy);
  sync.onclick = () => void refreshSource(source.name);
  acts.append(sync);

  // 只在「采集到的」跟「认的」不一样时才摆出来 —— 平时按下去是空动作。
  if (source.drift) {
    const pin = el("button", "btn btn-small", "记录这个版本");
    pin.type = "button";
    pin.title = "把上游现在这一版记成我们认的版本（写进 sources.json，走发布）";
    pin.disabled = Boolean(state.upstreamBusy);
    pin.onclick = () => void pinSourceVersion(source);
    acts.append(pin);
  }
  wrap.append(acts);
  return wrap;
}

function closeUpstreamDetail() {
  if (!state.upstreamDetail) return;
  const name = state.upstreamDetail;
  state.upstreamDetail = null;
  // 哪一条的浮层**不进地址**（地址只到订阅），所以这里一个字都不用写。
  render();
  // 焦点还回那张卡（卡片是重画过的，所以按订阅名重新找一遍）。
  const card = [...document.querySelectorAll("#upstream .product")].find(
    (node) => node.dataset.source === name,
  );
  card?.focus({ preventScroll: true });
}

/** 浮层只有一个开关：正在看哪条就关哪条。 */
function closeSheet() {
  if (state.pendingOpen) return closePending();
  if (state.upstreamDetail) return closeUpstreamDetail();
  closeDetail();
}

function renderUpstreamDetail() {
  const entry = (state.upstream?.sources ?? []).find((item) => item.name === state.upstreamDetail) ?? null;
  if (!entry) {
    state.upstreamDetail = null;
    return false;
  }
  const mine = upstreamSummary(upstreamEntriesOf(entry.name));
  $("dt-mono").textContent = initialsOf(entry.name);
  $("dt-tile").style?.setProperty?.("--hue", String(hueOf(entry.name)));
  $("dt-name").textContent = entry.name;
  const badges = $("dt-badges");
  badges.textContent = "";
  // 上游那一层的徽章说的是「这是别人家的仓」，不是某一条的取用状态（那在抽屉里按文件标）。
  const chip = el("span", "detail-badge", "别人的仓");
  chip.dataset.tone = entry.mirror === "missing" ? "unpub" : "ok";
  chip.title = "上游按「一个仓 = 一条技能」算：看正文打开的是那一捆技能（不是整仓的每个字节）";
  badges.append(chip);
  $("dt-meta").textContent = `${mine.total} 条技能 · ${entry.url}`;
  const body = $("dt-body");
  body.textContent = "";
  body.append(upstreamDetailBody(entry));
  return true;
}

// ── 添加上游 ────────────────────────────────────────────────────────────────

/** 添加上游：地址 + 可选 prefix。名字从地址推，推不对就换个地址重来。 */
function sourceForm() {
  const form = el("div", "source-form");
  const url = el("input");
  url.type = "text";
  url.placeholder = "上游地址：https://github.com/谁的/哪个仓（git 认得就行）";
  const prefix = el("input");
  prefix.type = "text";
  prefix.placeholder = "只看哪一段（可选，如 skills）";
  const go = el("button", "btn btn-small", state.upstreamBusy === "add" ? "添加中…" : "添加");
  go.type = "button";
  go.disabled = state.upstreamBusy === "add";
  go.onclick = () => void addSource({ url: url.value, prefix: prefix.value });
  const cancel = el("button", "btn btn-small", "算了");
  cancel.type = "button";
  cancel.onclick = () => {
    state.addingSource = false;
    render();
  };
  for (const node of [url, prefix, go, cancel]) form.append(node);
  url.onkeydown = (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      void addSource({ url: url.value, prefix: prefix.value });
    }
  };
  return form;
}

// ── 左栏：上游的目录树 ─────────────────────────────────────────────────────
//
// 跟本地类别栏同构：一行一个目录，缩进说明深浅，数字是「这一层往下有多少条技能」。
// 只在主区是上游的时候出现 —— 左栏永远只说当前主区那一件事。

/**
 * 左栏：**订阅列表**。一个订阅 = 一条「技能」（一捆），所以这里不摆目录 ——
 * 目录住在抽屉那棵树里（`看正文`）。
 */
function renderUpstreamFacets() {
  const host = $("facets");
  host.textContent = "";
  const sources = state.upstream?.sources ?? [];

  for (const source of sources) {
    const on = source.name === state.upstreamSource;
    const item = el("div", "facet" + (on ? " on" : ""));
    const main = el("button", "facet-main");
    main.type = "button";
    main.append(el("span", "facet-label", source.name));
    const mine = upstreamSummary(upstreamEntriesOf(source.name));
    const n = el("span", "facet-n", source.mirror === "missing" ? "未镜像" : String(mine.total));
    if (source.mirror === "missing" || mine.todo) n.dataset.tone = "warn";
    main.append(n);
    main.title = `${source.url}${source.prefix ? ` · 只看 ${source.prefix}/` : ""}`;
    main.onclick = () => switchSource(source.name);
    item.append(main);
    host.append(item);
  }

  if (!sources.length) host.append(el("p", "facet-empty dim small", "还没订阅任何一个上游。"));
  $("rail-title").textContent = "订阅";
  // 「+ 新建类别」在上游这一屏没有意义：那改的是我们自己的词表。
  $("rail-new").hidden = true;
  $("cat-count").textContent = String(sources.length);
}

function renderUpstreamPanel() {
  const host = $("upstream");
  const sources = state.upstream?.sources ?? [];
  const source = upstreamSourceNow();
  const entries = upstreamEntriesOf(state.upstreamSource);
  const mine = upstreamSummary(entries);

  host.textContent = "";
  $("result-count").textContent = source
    ? `${mine.total} 条技能${mine.todo ? ` · ${mine.todo} 条待看` : ""}`
    : "还没订阅";

  // 动作摆在工具栏那一格（跟「新建技能 / 重扫」同一个位置）—— 换的是这一屏能做什么。
  const bar = $("toolbar-up");
  bar.textContent = "";
  const add = el("button", "btn btn-small", state.addingSource ? "填地址…" : "+ 添加上游");
  add.type = "button";
  add.title = "往 sources.json 里加一行（未发布，跟技能一样走发布）";
  add.onclick = () => {
    state.addingSource = !state.addingSource;
    render();
  };
  bar.append(add);
  if (source) {
    const sync = el(
      "button",
      "btn btn-small",
      state.upstreamBusy === `fetch:${source.name}` ? "刷新中…" : "刷新（fetch）",
    );
    sync.type = "button";
    sync.disabled = Boolean(state.upstreamBusy);
    sync.title = "git fetch 这个上游 —— 打开这一页不会联网，所以更新要手动拉";
    sync.onclick = () => void refreshSource(source.name);
    bar.append(sync);
  }

  if (state.addingSource || !sources.length) host.append(sourceForm());

  if (!source) {
    host.append(el("p", "detail-sum", "别的 git 仓库里的技能：订阅一个仓，它带 SKILL.md 的那几层就都在这儿，觉得有用就一条一条取用。"));
    return;
  }

  // 一个订阅 = 一张卡。卡上写的是那一捆技能的事。
  const grid = el("div", "grid");
  grid.append(upstreamRepoCard(source));
  host.append(grid);
}

/**
 * 上游那张卡：**一个订阅一张**。它代表那个仓里的一捆技能 —— 点开是半扇浮层，
 * 浮层里 `看正文 ›` 是那只抽屉，左栏是**那一捆的文件树**（技能目录，不是整仓）。
 *
 * 上游按「一个仓 = 一条技能」理解：所以这里没有一条一条的卡片，
 * 也没有目录卡（目录只在抽屉那棵树里）。
 */
function upstreamRepoCard(source) {
  const entries = upstreamEntriesOf(source.name);
  const mine = upstreamSummary(entries);
  const card = el("article", "product up-card up-repo");
  card.tabIndex = 0;
  card.setAttribute("role", "button");
  card.setAttribute(
    "aria-label",
    `${source.name} · 上游 · ${mine.total} 条技能${source.version?.commit ? ` · 版本 ${versionText(source.version)}` : ""}`,
  );
  card.dataset.source = source.name;
  card.title = source.url;

  const openMenuAt = (mx, my) => openUpstreamMenu(mx, my, source);
  const open = () => {
    state.upstreamDetail = source.name;
    render();
    $("dt-body").scrollTop = 0;
    $("detail").focus({ preventScroll: true });
  };
  card.onclick = open;
  card.oncontextmenu = (event) => {
    event.preventDefault();
    openMenuAt(event.clientX, event.clientY);
  };
  card.onkeydown = (event) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      open();
    } else if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      event.preventDefault();
      const rect = card.getBoundingClientRect();
      openMenuAt(rect.left + 8, rect.bottom - 6);
    }
  };
  attachMenuGesture(card, openMenuAt);

  const visual = el("div", "visual");
  visual.append(el("span", "visual-mono", initialsOf(source.name)));
  visual.append(el("span", "badge-float dir", `${mine.total} 条`));
  // 「上游更新了」是卡片上最该先看见的一件事 —— 左上角一个点 + 三个字。
  if (source.newer) {
    const fresh = el("span", "badge-float up-new", "上游有更新");
    fresh.title = `远端已经是 ${versionText(source.remote?.version) || source.remote?.head?.slice(0, 7)}`;
    visual.append(fresh);
  }
  card.append(visual);

  const body = el("div", "product-body");
  const title = el("div", "product-title");
  title.append(el("span", "product-name", source.name));
  const kind = el("span", "product-hidden", "上游");
  kind.title = "别的 git 仓库：它带 SKILL.md 的那几层就是技能，一条一条取用";
  title.append(kind);

  const menuBtn = el("button", "product-menu", "⋯");
  menuBtn.type = "button";
  menuBtn.title = `${source.name} 的菜单：看正文 / 刷新 / 地址`;
  menuBtn.setAttribute("aria-label", `${source.name} 的菜单`);
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
  // 底部那行放**版本**：URL 在浮层里、`title` 上都有，卡片这一格更该回答
  // 「我订阅的是哪一版」。漂移时后面挂个箭头指上游现在停在哪。
  const sum = el("p", "product-sum mono");
  if (source.version?.commit) {
    sum.append(el("span", "", versionText(source.version)));
    // 右箭头指「上游现在是哪一版」：有检查结果就用它，没有就看镜像漂到哪了。
    const ahead = source.newer
      ? versionText(source.remote?.version) || source.remote?.head?.slice(0, 7)
      : source.drift
        ? versionText(source.headVersion) || source.head?.slice(0, 7)
        : "";
    if (ahead) {
      const drift = el("span", "up-drift", ` → ${ahead}`);
      drift.title = source.newer
        ? "上游已经有更新了 —— 打开卡片，按「更新到最新」"
        : "镜像里的版本跟认的不一样 —— 浮层里可以记录新版本";
      sum.append(drift);
    }
  } else {
    sum.textContent = source.mirror === "ready" ? "还没记录版本" : "还没镜像";
  }
  body.append(sum);
  card.append(body);
  return card;
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

// ── 未同步的改动（唯一的发布入口） ──────────────────────────────────────────
//
// 顶栏那颗 sync chip 是这一页的账：仓库读不了 / 远端有新提交 / 装载失败 /
// 未发布 / 没推的提交 —— 一档一档往下掉，只说最要紧的那一条。点开是同一份事实的清单：
// 哪些单元脏了、每个文件增删几行、一条 message，一个按钮。
//
// **发布只有一个动作**：内容闸 → 重扫 index.json → commit → push（`POST /v1/skills/publish`）。
// 归档 / 彻底删除不跟发布走 —— 那是另外两个显式动作，各有各的弹窗。

const PENDING_KIND = { skill: "技能", categories: "类别清单", archive: "归档区", other: "其它文件" };

async function loadPending() {
  const repo = state.data?.repo ?? {};
  // 干净的时候不必去算（那是三条 git 命令）；脏了才问值不值当摆清单。
  if (!repo.dirty && !(repo.ahead > 0)) {
    state.pending = { units: [], head: repo.head, dirty: false, ahead: repo.ahead ?? 0, behind: repo.behind ?? 0, remote: repo.remote };
    renderSync();
    return;
  }
  try {
    state.pending = await transport.get("v1/skills/pending");
  } catch (error) {
    state.pending = { units: [], error: String(error?.message ?? error) };
  }
  renderSync();
  if (state.pendingOpen) renderPending();
}

/** 一档一档往下掉，只说最要紧的那一条 —— 跟 vbe4pt 那颗 chip 同一套优先级。 */
function syncState() {
  const repo = state.data?.repo ?? {};
  const units = state.pending?.units ?? [];
  const failed = (state.view?.cards ?? []).filter((card) => card.state === "failed").length;
  if (repo.error) return { tone: "bad", text: "仓库读不了", title: repo.error };
  if ((repo.behind ?? 0) > 0) {
    return { tone: "bad", text: "远端有新提交", title: `origin 比本地多 ${repo.behind} 个提交 —— 先拉，否则推送会被挡` };
  }
  if (failed) {
    return { tone: "bad", text: `${failed} 条装载失败`, title: "装不上就进不了提示词：打开那条技能看卡上的原因" };
  }
  if (repo.dirty) {
    return {
      tone: "wait",
      text: state.pending ? `${units.length} 处未发布` : "有未发布改动",
      title: "点开看清单：发布 = 内容闸 + commit + push",
    };
  }
  if ((repo.ahead ?? 0) > 0) return { tone: "wait", text: `${repo.ahead} 个提交没推`, title: "本地有提交还没推上去" };
  return { tone: "ok", text: "已同步", title: "工作区干净，远端一致" };
}

function renderSync() {
  const chip = $("sync");
  if (!chip) return;
  const { tone, text, title } = syncState();
  chip.hidden = !state.data;
  chip.dataset.state = tone;
  const dot = $sel("#sync .dot");
  if (dot) dot.dataset.state = tone;
  const label = chip.querySelector(".sync-label");
  if (label) label.textContent = text;
  chip.title = title;
}

function openPending({ silent = false } = {}) {
  if (!state.data) return;
  state.pendingOpen = true;
  // `/pending` 也能直接落到这扇浮层上（贴给别人、或者测试里省一次点击）。
  if (!silent) writePath({ view: "pending" });
  state.pendingMessage = "";
  render();
  $("dt-body").scrollTop = 0;
  $("detail").focus({ preventScroll: true });
  if (!state.pending) void loadPending();
}

function closePending({ silent = false } = {}) {
  if (!state.pendingOpen) return;
  state.pendingOpen = false;
  if (!silent) writePath({ view: "skills" });
  render();
}

async function publishAll() {
  if (state.publishing) return;
  state.publishing = true;
  renderPending();
  try {
    const out = await transport.post("v1/skills/publish", { message: state.pendingMessage });
    const hash = String(out.commit?.hash ?? "").slice(0, 7);
    if (out.pushed) toast(`已发布 ${hash} —— 推上去了`);
    else if (out.commit) toast(`已提交 ${hash}，但没推上去${out.error ? `：${out.error}` : ""}`, "bad");
    else toast("没有要提交的");
    for (const warning of out.warnings ?? []) toast(warning, "bad");
    state.pendingMessage = "";
    state.pendingOpen = false;
    await load({ quiet: true });
    await loadUpstream({ quiet: true });
  } catch (error) {
    toast(`发布失败：${error.message}`, "bad");
  } finally {
    state.publishing = false;
    render();
  }
}

function renderPending() {
  const repo = state.data?.repo ?? {};
  const pending = state.pending;
  const units = pending?.units ?? [];
  const { tone, text } = syncState();

  $("dt-mono").textContent = "≠";
  $("dt-name").textContent = "未同步的改动";
  const badges = $("dt-badges");
  badges.textContent = "";
  const chip = el("span", "detail-badge", text);
  chip.dataset.tone = tone === "wait" ? "unpub" : tone === "ok" ? "ok" : "bad";
  badges.append(chip);
  $("dt-meta").textContent = repo.remote ?? "（没有远端）";

  const body = $("dt-body");
  body.textContent = "";

  if (pending?.error) body.append(alertLine("bad", "算不出来", pending.error));
  const repoRows = el("div", "up-detail");
  repoRows.append(detailRow("远端", repo.remote || "没有配 origin"));
  repoRows.append(detailRow("HEAD", String(repo.head ?? "—").slice(0, 7) || "—"));
  if (repo.ahead) repoRows.append(detailRow("没推的提交", String(repo.ahead)));
  if (repo.behind) repoRows.append(detailRow("远端多出来", String(repo.behind)));
  body.append(repoRows);

  if (!units.length) {
    body.append(el("p", "detail-sum", repo.dirty ? "读不到改动清单 —— 面板重启一下试试。" : "工作区干净，没有要发布的。"));
    return;
  }

  for (const unit of units) {
    const kind = PENDING_KIND[unit.kind] ?? "改动";
    const title = unit.name ? `${kind} · ${unit.name}` : kind;
    const { node, body: rows } = detailSection(title, unit.tracked === false ? "HEAD 里没有" : "");
    for (const file of unit.files ?? []) {
      const delta = [file.added ? `+${file.added}` : "", file.removed ? `−${file.removed}` : ""].filter(Boolean).join(" ");
      rows.append(detailRow(file.path, [file.status, delta].filter(Boolean).join(" · ")));
    }
    if (!(unit.files ?? []).length) rows.append(el("p", "detail-sum", "（没有文件级信息）"));
    body.append(node);
  }

  // 发布：一个输入框 + 一个按钮。没有「勾选一半」—— 技能仓不该有半截提交。
  const box = el("div", "up-detail");
  const message = el("input", "rail-input");
  message.type = "text";
  message.placeholder = "这条提交叫什么（空着就写「skill: 更新技能仓库」）";
  message.value = state.pendingMessage;
  message.oninput = () => {
    state.pendingMessage = message.value;
  };
  box.append(message);
  const acts = el("div", "detail-sec-acts");
  const go = el("button", "btn btn-small primary", state.publishing ? "发布中…" : "发布");
  go.type = "button";
  go.disabled = state.publishing;
  go.title = "内容闸 → 重扫 index.json → commit → push";
  go.onclick = () => void publishAll();
  acts.append(go);
  box.append(acts);
  box.append(el("p", "detail-sum", "发布推的是工作区里改过的东西；归档 / 彻底删除各有各的弹窗，不跟这一下走。"));
  body.append(box);
}

// ── 归档 / 恢复 / 彻底删除（会改仓库） ────────────────────────────────────

// 破坏性且会推上远端：要求手输技能名。
function confirmName({ title, hint, name, label = "确定", fieldLabel = "手输技能名", onConfirm }) {
  const modal = $("danger");
  const input = $("danger-input");
  const ok = $("danger-ok");
  $("danger-title").textContent = title;
  $("danger-field").textContent = fieldLabel;
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
  const key = splitSkillKey(skill.name);
  state.editor = {
    // `name` 永远是**本名**（接口按它找目录）；变体只多一个 `variant` 参数。
    // 显示用的整串在 `display` 里 —— 页面上人要看的是 `code-review@strict`。
    name: key.family,
    variant: key.id,
    display: skill.name,
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
    // 文件树里展开了哪些文件夹（打开抽屉时默认全展开）。
    dirs: new Set(),
  };
  state.publishing = false;
  $("ed-text").value = "";
  $("ed-text").disabled = true;
  $("ed-path").textContent = "—";
  $("ed-scrim").hidden = false;
  $("editor").hidden = false;
  setBehindInert();
  $("ed-name").textContent = skill.name;
  $("ed-meta").textContent = `${skill.path && skill.path !== skill.name ? `${skill.path} · ` : ""}SKILL.md · ${
    skill.tracked === false ? "未发布" : "已进 HEAD"
  }`;
  closeNewEntry();
  setEditorMode("edit");
  renderEditorFiles();
  renderEditorState();

  try {
    const list = await transport.get(withVariant(`v1/skills/files?name=${encodeURIComponent(key.family)}`, key.id));
    state.editor.files = list.files ?? [];
    state.editor.filesLoading = false;
    // 默认全展开：一进来就看见树，不用一层层点。
    state.editor.dirs = new Set(dirPaths(fileTree(state.editor.files)));
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

/**
 * 别人的仓里那一条，用**同一扇抽屉**打开 —— 左栏同样是那棵文件树，能钻 `references/`。
 *
 * 不一样的只有一件事：**只读**。没有保存 / 新建 / 删除 / 发布，也没有差异视图
 * （没有 HEAD 可比：那是别人的仓）。内容从镜像里读（`git cat-file`），
 * **不摊进我们的仓** —— 看正文不等于取用。
 */
async function openUpstreamEditor(source, { path = null } = {}) {
  closeMenu();
  state.editor = {
    kind: "upstream",
    readOnly: true,
    source: source.name,
    name: source.name,
    // 树里的路径相对**技能根**（`engineering/tdd/SKILL.md`），调接口时要加回 prefix。
    prefix: source.prefix ?? "",
    dir: source.dir ?? "",
    files: [],
    filesLoading: true,
    path: null,
    text: "",
    error: "",
    notice: "",
    mode: "edit",
    copyPick: null,
    expanded: new Set(),
    lineCount: 1,
    dirs: new Set(),
  };
  state.publishing = false;
  $("ed-text").value = "";
  $("ed-text").disabled = true;
  $("ed-text").readOnly = true;
  $("ed-path").textContent = "—";
  $("ed-scrim").hidden = false;
  $("editor").hidden = false;
  $("editor").dataset.readonly = "1";
  setBehindInert();
  $("ed-name").textContent = source.name;
  // 标题下面那行写**本机路径**：看正文时人想知道的是「这些字从哪来」，
  // 而 GitHub 地址点不进去、grep 不了。
  $("ed-meta").textContent = `${source.dir || source.url} · 只读`;
  closeNewEntry();
  setEditorMode("edit");
  renderEditorFiles();
  renderEditorState();

  try {
    const tree = await transport.get(`v1/skills/upstream/tree?source=${encodeURIComponent(source.name)}`);
    state.editor.files = tree.files ?? [];
    state.editor.filesLoading = false;
    // 树里只有技能（服务端已经裁过）。头两层全展开，剩下谁想看谁点。
    state.editor.dirs = new Set(dirPaths(fileTree(state.editor.files)).filter((dir) => dir.split("/").length <= 2));
    renderEditorFiles();
    const first = path
      ? state.editor.files.find((file) => file.path === path || file.path.endsWith(`/${path}`))
      : null;
    if (first) await openEditorFile(first.path, { force: true });
    else $("ed-path").textContent = "左边挑一个文件看";
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
  closeNewEntry();
  delete $("editor").dataset.readonly;
  $("ed-text").readOnly = false;
  $("editor").hidden = true;
  $("ed-scrim").hidden = true;
  setBehindInert();
}

/**
 * 左栏的文件树。列表是扁的（`docs/a.md`），这里长成树 —— 纯计算全在 model.js
 * 的 `fileTree` / `flattenTree`，这一层只管画和点。
 *
 * 文件夹**不是存出来的**：有文件住在这儿它才在。所以「新建文件夹」在服务端落成
 * 一个带 `.gitkeep` 的目录（空目录 git 不跟踪），写进第一个真文件时那份占位会自己没。
 */
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
  const tree = fileTree(ed.files);
  for (const node of flattenTree(tree, (path) => ed.dirs.has(path))) {
    nav.append(node.type === "dir" ? editorDirRow(node) : editorFileRow(node));
    if (state.editor.copyPick?.path === node.path) nav.append(copyPickRow(node));
  }
}

/** 一行的壳：缩进靠 `--depth`。行尾那个「⋯」和右键 / 长按开的是同一个菜单。 */
function editorRowShell(node) {
  const row = el("div", "ed-row");
  row.style.setProperty("--depth", String(node.depth));
  row.dataset.path = node.path;
  return row;
}

/** 行尾那个「⋯」：菜单的可见入口（hover / 焦点时才亮，触屏常驻）。 */
function editorRowMore(node) {
  const btn = el("button", "ed-file-more", "⋯");
  btn.type = "button";
  btn.title = `${node.path}${node.type === "dir" ? "/" : ""} 的操作`;
  btn.setAttribute("aria-label", `打开 ${node.path} 的操作菜单`);
  return btn;
}

/**
 * 行菜单：打开 / 展开 / 复制路径 / 删除。
 *
 * 删除从「行尾那个 ×」挪到这里 —— 一是 × 太窄，手指点不中、鼠标也容易误触；
 * 二是这一行能做的事本来就不止删除，挤成一个小叉子只是把删除冒充成唯一动作。
 * 行尾仍留一个「⋯」当可见入口：只藏在右键里的动作，第一次进来的人永远找不到。
 */
function openEditorEntryMenu(x, y, node) {
  const ed = state.editor;
  if (!ed) return;
  closeMenu();
  const menu = $("ctxmenu");
  menu.textContent = "";
  menu.hidden = false;
  menuOpen = true;

  const isDir = node.type === "dir";
  menuHead(menu, node.name, isDir ? `${node.children.length} 项` : bytes(node.size));

  let first;
  if (isDir) {
    const open = ed.dirs.has(node.path);
    first = menuItem(menu, open ? "收起" : "展开", `${node.path}/`, () => {
      if (open) ed.dirs.delete(node.path);
      else ed.dirs.add(node.path);
      renderEditorFiles();
    });
  } else {
    first = menuItem(menu, "打开", "在右边改", () => void openEditorFile(node.path));
  }

  menuItem(menu, "复制路径", node.path, () => {
    navigator.clipboard?.writeText(node.path).then(
      () => toast("路径已复制"),
      () => toast(node.path),
    );
  });

  // 别人的仓（只读）：菜单只剩看和复制的部分，删除那一格干脆不出现。
  if (!ed.readOnly) {
    menu.append(el("div", "ctx-sep"));
    menuItem(
      menu,
      isDir ? "删除文件夹" : "删除文件",
      isDir ? `连同 ${node.children.length} 项` : "只落盘，没发布前都在 git 里",
      () => void deleteEditorEntry(node),
      { danger: true },
    );
  }

  mountMenu(menu, x, y, first);
}

/** 右键 / 长按 / 行尾「⋯」三张嘴，喂同一个菜单。 */
function attachEditorRowMenu(row, node, trigger) {
  const openMenuAt = (mx, my) => openEditorEntryMenu(mx, my, node);

  row.oncontextmenu = (event) => {
    event.preventDefault();
    openMenuAt(event.clientX, event.clientY);
  };
  attachMenuGesture(row, openMenuAt);

  if (!trigger) return;
  trigger.setAttribute("aria-haspopup", "menu");
  trigger.onclick = (event) => {
    event.stopPropagation();
    const rect = trigger.getBoundingClientRect();
    openMenuAt(rect.left, rect.bottom + 4);
  };
}

/**
 * 上游那棵树里的一条技能 = 一个装着 `SKILL.md` 的目录。
 * 整仓都摆进抽屉之后，**只有取用这件事还按「一条」算**，所以它挂在 `SKILL.md` 那一行。
 */
function upstreamEntryOfFile(node) {
  const ed = state.editor;
  if (ed?.kind !== "upstream" || node.type !== "file" || node.name !== "SKILL.md") return null;
  // 树里的路径相对**技能根**（`engineering/tdd/SKILL.md`），而 `entry.path` 是仓内
  // 完整路径（`skills/engineering/tdd`）—— 差的那一层就是 prefix，得加回去才比得上。
  const home = node.path.slice(0, -"/SKILL.md".length);
  if (!home) return null;
  const want = ed.prefix ? `${ed.prefix}/${home}` : home;
  return (state.upstream?.entries ?? []).find(
    (entry) => entry.source === ed.source && entry.path === want,
  ) ?? null;
}

/** 上游里的一条技能 = 一个装着 `SKILL.md` 的目录。整条抄只认这种目录。 */
function isUpstreamSkillDir(node) {
  const ed = state.editor;
  if (ed?.kind !== "upstream" || node.type !== "dir") return false;
  return (ed.files ?? []).some((file) => file.path === `${node.path}/SKILL.md`);
}

/** 抽屉里那条「抄」标 —— 骨子里跟「取用」一样，也是一个 role=button 的 span（不许 button 套 button）。 */
function copyMark(node, label, title, onFire) {
  const mark = el("span", "ed-file-mark", label);
  mark.setAttribute("role", "button");
  mark.tabIndex = 0;
  mark.dataset.tone = "copy";
  mark.title = title;
  const off = Boolean(state.upstreamBusy);
  mark.setAttribute("aria-disabled", String(off));
  const fire = (event) => {
    event.stopPropagation();
    event.preventDefault();
    if (off) return;
    onFire();
  };
  mark.onclick = fire;
  mark.onkeydown = (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    fire(event);
  };
  return mark;
}

/** 把上游的一条技能整抄过来。重名由服务端接（`tdd-2`、`tdd-3`…），我们只管说结果。 */
async function copyUpstreamSkill(home) {
  if (state.upstreamBusy) return;
  const ed = state.editor;
  state.upstreamBusy = `copy:${home}`;
  renderEditorFiles();
  try {
    const out = await transport.post("v1/skills/upstream/copy", { source: ed.source, path: home });
    if (out.name !== out.original) toast(`抄好了：${out.name}（原来那条叫 ${out.original}，名字被占了）`);
    else toast(`抄好了：${out.name}`);
    await load({ quiet: true });
    await loadUpstream({ quiet: true });
  } catch (error) {
    toast(`抄失败：${error.message}`, "bad");
  } finally {
    state.upstreamBusy = null;
    renderEditorFiles();
  }
}

/** 抄单个文件要先说「落到我哪个技能里」—— 内联一行，不弹模态。 */
function openCopyPick(node) {
  const ed = state.editor;
  if (!ed) return;
  // 变体（`tdd@strict`）也住在仓里，但名字里有 `@`，不是能往里抄的目录 —— 过滤掉。
  const mine = (state.data?.skills ?? []).map((skill) => skill.name).filter((name) => !name.includes("@"));
  if (!mine.length) {
    toast("你还没有自己的技能 —— 先建一条，再往里抄文件", "bad");
    return;
  }
  ed.copyPick = { path: node.path, value: mine[0] };
  renderEditorFiles();
}

async function confirmCopyPick(node) {
  const ed = state.editor;
  const pick = ed?.copyPick;
  if (!pick || !pick.value) return;
  const home = node.path.includes("/") ? node.path.slice(0, node.path.lastIndexOf("/")) : "";
  const file = node.name;
  state.upstreamBusy = `copy:${node.path}`;
  try {
    const out = await transport.post("v1/skills/upstream/copy", {
      source: ed.source,
      path: ed.prefix ? `${ed.prefix}/${home}` : home,
      file,
      to: pick.value,
    });
    toast(`抄好了：${out.path}`);
    ed.copyPick = null;
    await load({ quiet: true });
  } catch (error) {
    toast(`抄失败：${error.message}`, "bad");
  } finally {
    state.upstreamBusy = null;
    renderEditorFiles();
  }
}

/** 那行内联选择器：抄到哪个技能里。 */
function copyPickRow(node) {
  const ed = state.editor;
  const wrap = el("div", "ed-copy-pick");
  wrap.append(el("span", "ed-copy-pick-label", "抄到"));
  const select = el("select", "ed-copy-pick-select");
  for (const skill of state.data?.skills ?? []) {
    if (skill.name.includes("@")) continue;
    const option = el("option", "", skill.name);
    option.value = skill.name;
    select.append(option);
  }
  select.value = ed.copyPick.value;
  select.onchange = () => {
    ed.copyPick.value = select.value;
  };
  wrap.append(select);
  const go = el("button", "btn btn-small", "抄过来");
  go.type = "button";
  go.onclick = () => void confirmCopyPick(node);
  const no = el("button", "btn btn-small btn-ghost", "算了");
  no.type = "button";
  no.onclick = () => {
    ed.copyPick = null;
    renderEditorFiles();
  };
  wrap.append(go, no);
  return wrap;
}

function editorFileMark(node) {
  const entry = upstreamEntryOfFile(node);
  if (!entry) return null;
  const busy = upstreamBusyOn(entry);
  const taken = entry.local === "same" || entry.local === "changed";
  // **不能用 `<button>`**：这一行的行体本身就是个 button，HTML 不许 button 套 button ——
  // 浏览器会把内层的拆到外面，行体的 onclick 跟着哑掉（点了没反应，也不报错）。
  // 所以这里是个带 role 的 span，键盘照样能到。
  const mark = el("span", "ed-file-mark", busy ? "取用中…" : taken ? "已取用" : "取用");
  mark.setAttribute("role", "button");
  mark.tabIndex = 0;
  const off = busy || taken || Boolean(state.upstreamBusy);
  mark.setAttribute("aria-disabled", String(off));
  mark.dataset.tone = taken ? "missing" : "diff";
  mark.title = taken
    ? "本地已经有这一条了"
    : "拷进我们仓里（不提交）—— 取用之后它是我们自己的一条技能";
  const fire = (event) => {
    event.stopPropagation();
    event.preventDefault();
    if (off) return;
    void adoptEntry(entry, entry.local === "taken" ? suggestedAdoptName(entry) : undefined);
  };
  mark.onclick = fire;
  mark.onkeydown = (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    fire(event);
  };
  return mark;
}

function editorFileRow(node) {
  const ed = state.editor;
  const row = editorRowShell(node);
  const main = el("button", "ed-file");
  main.type = "button";
  main.dataset.kind = node.path.endsWith(".md") ? "markdown" : "other";
  main.setAttribute("aria-current", String(node.path === ed.path));
  main.append(el("span", "ed-file-caret"));
  main.append(el("span", "ed-file-name", node.name));
  const mark = editorFileMark(node);
  if (mark) main.append(mark);
  if (ed.kind === "upstream") {
    main.append(
      copyMark(node, "抄", "把这个文件抄进我自己的某一条技能里", () => openCopyPick(node)),
    );
  }
  main.append(el("span", "ed-file-size", bytes(node.size)));
  main.title = `${node.path} · ${bytes(node.size)}`;
  main.onclick = () => void openEditorFile(node.path);
  // 只读也长「⋯」（打开 / 复制路径还要用），只是菜单里没有删除那一格。
  const more = editorRowMore(node);
  row.append(main, more);
  attachEditorRowMenu(row, node, more);
  return row;
}

function editorDirRow(node) {
  const ed = state.editor;
  const open = ed.dirs.has(node.path);
  const row = editorRowShell(node);
  const main = el("button", "ed-file");
  main.type = "button";
  main.dataset.kind = "dir";
  main.setAttribute("aria-expanded", String(open));
  main.append(el("span", "ed-file-caret", open ? "▾" : "▸"));
  main.append(el("span", "ed-file-name", node.name));
  // 整条抄只对「装着 SKILL.md 的目录」开放 —— 别的目录抄下来是一堆没人认领的散文件。
  if (isUpstreamSkillDir(node)) {
    main.append(
      copyMark(node, "抄", "整条抄进我们仓里（重名自动叫 -2、-3）", () => {
        const home = ed.prefix ? `${ed.prefix}/${node.path}` : node.path;
        void copyUpstreamSkill(home);
      }),
    );
  }
  main.append(el("span", "ed-file-size", String(node.children.length)));
  main.title = `${node.path}/ · 点一下${open ? "收起" : "展开"}`;
  main.onclick = () => {
    if (open) ed.dirs.delete(node.path);
    else ed.dirs.add(node.path);
    renderEditorFiles();
  };
  const more = editorRowMore(node);
  row.append(main, more);
  attachEditorRowMenu(row, node, more);
  return row;
}

/** 重新问一遍文件列表（新建 / 删除之后）。新出现的父目录顺手展开，不然建完看不见。 */
async function reloadEditorFiles() {
  const ed = state.editor;
  if (!ed) return;
  try {
    const list = await transport.get(withVariant(`v1/skills/files?name=${encodeURIComponent(ed.name)}`, ed.variant));
    ed.files = list.files ?? [];
    ed.filesLoading = false;
    for (const path of dirPaths(fileTree(ed.files))) ed.dirs.add(path);
    renderEditorFiles();
    renderEditorState();
  } catch (error) {
    ed.error = `读文件列表失败：${error.message}`;
    renderEditorState();
  }
}

// ── 新建 / 删除 ───────────────────────────────────────────────────────────

/**
 * 打开「新建」那一格。`kind` = file | dir。
 * 路径里带 `/` 就是顺手建文件夹 —— 所以「新建文件夹」本质上只是「建一个带 .gitkeep 的目录」。
 */
function openNewEntry(kind) {
  const ed = state.editor;
  if (!ed) return;
  state.newKind = kind;
  const form = $("ed-new");
  form.hidden = false;
  $("ed-new-kind").textContent = kind === "dir" ? "文件夹" : "文件";
  const input = $("ed-new-path");
  input.value = "";
  input.placeholder = kind === "dir" ? "references" : "docs/guide.md";
  const hint = $("ed-new-hint");
  hint.hidden = false;
  hint.dataset.tone = "";
  hint.textContent =
    kind === "dir"
      ? "空文件夹里会铺一份 .gitkeep 撑着 —— 空目录 git 不跟踪，发布那一刻它就没了。"
      : "路径里带 / 就等于顺手建文件夹。建出来是一份空文件：写点什么，再发布。";
  input.focus();
}

function closeNewEntry() {
  const form = $("ed-new");
  if (form) form.hidden = true;
  const hint = $("ed-new-hint");
  if (hint) hint.hidden = true;
  state.newKind = null;
}

async function submitNewEntry(event) {
  event.preventDefault();
  const ed = state.editor;
  if (!ed) return;
  const kind = state.newKind;
  const hint = $("ed-new-hint");
  const fail = (message) => {
    hint.hidden = false;
    hint.dataset.tone = "bad";
    hint.textContent = message;
  };
  const raw = $("ed-new-path").value.trim().replace(/^\/+|\/+$/g, "");
  if (!raw) return fail("先写个路径。");
  if (raw.split("/").some((part) => !part || part === "." || part === "..")) {
    return fail("路径里不能有空段、`.` 或 `..`。");
  }
  // 已存在的文件不能让 PUT 拿空串把它盖掉 —— 这是「新建」，不是「清空」。
  if (kind !== "dir" && ed.files.some((file) => file.path === raw)) {
    return fail(`已经有 ${raw} 了 —— 在左边点它去改。`);
  }
  $("ed-new-ok").disabled = true;
  try {
    if (kind === "dir") {
      await transport.post("v1/skills/mkdir", { name: ed.name, path: raw, variant: ed.variant });
      await reloadEditorFiles();
      closeNewEntry();
      toast(`建了文件夹 ${raw}/`);
      return;
    }
    await transport.put("v1/skills/file", { name: ed.name, path: raw, text: "", variant: ed.variant });
    await reloadEditorFiles();
    closeNewEntry();
    if (editorDirty() && !confirm(`「${ed.path}」有没保存的改动，切过去会丢掉。继续？`)) return;
    await openEditorFile(raw, { force: true });
    toast(`建了 ${raw} —— 空文件，写点什么再发布`);
  } catch (error) {
    fail(String(error?.message ?? error));
  } finally {
    $("ed-new-ok").disabled = false;
  }
}

/** 删文件 / 删文件夹。都要手输路径确认 —— 跟归档、彻底删除同一套骨头。 */
async function deleteEditorEntry(node) {
  const ed = state.editor;
  if (!ed) return;
  const isDir = node.type === "dir";
  const query =
    withVariant(`name=${encodeURIComponent(ed.name)}&path=${encodeURIComponent(node.path)}`, ed.variant) +
    (isDir ? "&recursive=1" : "");
  confirmName({
    title: isDir ? `删掉文件夹 ${node.path}/？` : `删掉 ${node.path}？`,
    hint: isDir
      ? `底下 ${node.children.length} 个条目跟着走。只落盘、不提交 —— 发布之前都在 git 历史里捡得回来。`
      : "只落盘、不提交 —— 发布之前都在 git 历史里捡得回来。",
    name: node.path,
    fieldLabel: isDir ? "手输路径" : "手输文件名",
    label: isDir ? "删除整棵" : "删除",
    onConfirm: async () => {
      try {
        await transport.del(`v1/skills/file?${query}`);
        if (ed.path === node.path || (isDir && String(ed.path ?? "").startsWith(`${node.path}/`))) {
          ed.path = null;
          ed.text = "";
          ed.base = null;
          $("ed-text").value = "";
          $("ed-text").disabled = true;
          $("ed-path").textContent = "—";
        }
        await reloadEditorFiles();
        toast(`删了 ${node.path} —— 没发布之前都还在 git 里`);
      } catch (error) {
        toast(`删除失败：${error.message}`, "bad");
      }
    },
  });
}

function renderEditorState() {
  const ed = state.editor;
  if (!ed) return;
  const chip = $("ed-state");
  let tone = "ok";
  let text = ed.readOnly ? "只读 · 别人的仓" : "已落盘";
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

  const busy = ed.readOnly || ed.saving || state.publishing || !ed.path;
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
  closeNewEntry();
  if (ed.readOnly) return openUpstreamFile(path);
  if (!force && editorDirty() && !confirm(`「${ed.path}」有没保存的改动，切过去会丢掉。继续？`)) return;
  try {
    const file = await transport.get(
      withVariant(`v1/skills/file?name=${encodeURIComponent(ed.name)}&path=${encodeURIComponent(path)}`, ed.variant),
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
    $("ed-path").textContent = `${ed.display}/${file.path}`;
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

/** 只读那一支：从镜像里读一份，没有任何「保存」可言。 */
async function openUpstreamFile(path) {
  const ed = state.editor;
  if (!ed) return;
  try {
    const want = ed.prefix ? `${ed.prefix}/${path}` : path;
    const file = await transport.get(
      `v1/skills/upstream/file?source=${encodeURIComponent(ed.source)}&file=${encodeURIComponent(want)}`,
    );
    if (state.editor !== ed) return;
    ed.path = path;
    ed.text = file.text;
    ed.error = "";
    ed.notice = "";
    ed.diff = null;
    ed.lineCount = String(file.text).split("\n").length;
    const area = $("ed-text");
    area.readOnly = true;
    area.disabled = false;
    area.value = file.text;
    // 相对技能根（`engineering/tdd/SKILL.md`）—— 跟左栏那棵树里的路径一模一样。
    $("ed-path").textContent = path;
    renderGutter();
    renderDelta();
    renderEditorFiles();
    renderEditorState();
  } catch (error) {
    ed.error = `读文件失败：${error.message}`;
    renderEditorState();
  }
}

/** `GET /v1/skills/pending/diff?path=<name>/<相对路径>` 的 `before` 就是 HEAD 里那一版。 */
async function loadBaseline(ed, path) {
  try {
    // 基线取的是**那棵树**（变体）的 HEAD，所以除了 path 还得说清是哪一棵。
    const data = await transport.get(
      withVariant(`v1/skills/pending/diff?path=${encodeURIComponent(`${ed.name}/${path}`)}`, ed.variant),
    );
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
      variant: ed.variant,
      path: ed.path,
      text,
      // base 是「我改的是哪一版」：别人先改了，服务端 409，别把对方的字盖掉。
      base: ed.base,
    });
    ed.text = text;
    ed.base = saved.etag;
    ed.conflict = false;
    ed.pending.add(ed.path);
    if (!quiet) ed.notice = `已保存到磁盘：${ed.display}/${ed.path}（还没提交）`;
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
    const { result, file } = await publishWithRefresh(transport, {
      name: ed.name,
      path: ed.path,
      variant: ed.variant,
    });
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
  // 还没点开任何文件（整仓那棵树刚打开时）：别画一个孤零零的「1」。
  if (!ed.path) {
    ed.lineCount = 0;
    gutter.textContent = "";
    return;
  }
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
  // 上游那条没有差异视图：没有 HEAD 可比（它不在我们的仓里）。
  ed.mode = ed.readOnly ? "edit" : mode;
  mode = ed.mode;
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
  const group = normalizeCategory($("create-group").value);
  const root = state.data?.root ?? "技能仓库";
  const where = group ? `${group}/<b>${name || "&lt;名字&gt;"}</b>/` : `<b>${name || "&lt;名字&gt;"}</b>/`;
  const verb = state.duplicating ? "复制到" : "会建在";

  // 空名字是唯一拦下来的东西：没名字连「建什么 / 复制成什么」都说不清。
  if (!name) return { ok: false, tone: "bad", message: `先起个名字。${verb} ${root}/${where}` };
  // 空说明只提醒，**不拦**：拦不拦是服务端的事（它会回 400 并把原因写在 #create-err 里）。
  // 复制时说明是照搬的，不问。
  if (!description && !state.duplicating) {
    return {
      ok: true,
      tone: "warn",
      message: "还差一句说明 —— 模型靠它决定用不用这条技能。空着提交的话服务端会拦。",
    };
  }
  return { ok: true, tone: "ok", message: `${verb} ${root}/${where}` };
}

function renderCreate() {
  const verdict = validateCreate();
  const note = $("create-name-note");
  note.className = `create-note ${verdict.tone}`;
  note.innerHTML = verdict.message;
  $("create-ok").disabled = !verdict.ok || state.creating;
  $("create-where").textContent = state.duplicating ? `复制自 ${state.duplicating}` : "SKILL.md + nous.json";
  $("create-ok").textContent = state.duplicating ? "复制" : "建";
  $("create-name").classList.toggle("bad", verdict.tone === "bad" && $("create-name").value.trim() !== "");
}

/** 同名的“下一个名字”：`foo-copy` → `foo-copy-2`。只为少一次 409，服务端仍然说了算。 */
function freeCopyName(from) {
  const taken = new Set((state.data?.skills ?? []).map((skill) => skill.name));
  let candidate = `${from}-copy`;
  for (let i = 2; taken.has(candidate); i++) candidate = `${from}-copy-${i}`;
  return candidate;
}

/**
 * 打开「复制成新技能」。
 *
 * 复用新建那个弹层：除了名字和分组，其余一律照搬源那条 —— 所以说明那一格直接收起来，
 * 类别预填源那条的。跟新建共一套校验、一套报错位置（服务端的 400 原文照样进 #create-err）。
 */
function openDuplicate(from, { variantId = null } = {}) {
  closeMenu();
  const skill = skillNow(from);
  if (!skill) return toast(`找不到 ${from}`, "bad");
  openCreate({
    duplicating: from,
    variantId,
    // `独立成技能 ›` 的默认名字跟扩展那边一样：`本名-变体id`。
    name: variantId ? freeCopyName(`${from}-${variantId}`) : freeCopyName(from),
    category: categoriesOf(skill)[0] ?? "",
  });
}

function openCreate({ duplicating = null, variantId = null, name = "", category = null } = {}) {
  closeMenu();
  state.creating = false;
  state.duplicating = duplicating;
  state.duplicatingVariant = variantId;
  $("create").reset();
  $("create-err").hidden = true;
  // 当前正筛着某个类别，新建时就默认挂上去 —— 少点一次。
  const filter = state.filter && !state.filter.startsWith("__") ? state.filter : "";
  $("create-cat").value = category ?? filter;
  $("create-cats").replaceChildren(
    ...state.view.categoryNames.map((name) => {
      const option = el("option");
      option.value = name;
      return option;
    }),
  );
  $("create-groups").replaceChildren(
    ...(state.data?.groups ?? []).map((name) => {
      const option = el("option");
      option.value = name;
      return option;
    }),
  );
  // 复制不问说明（照搬）、也不给分组（留在原地最安全；要搬就改分组那一格）。
  $("create-title").textContent = duplicating ? `复制 ${duplicating}` : "新建技能";
  $("create-desc-wrap").hidden = Boolean(duplicating);
  $("create-hint").innerHTML = duplicating && variantId
    ? `拷的是 <span class="mono">${duplicating}@${variantId}</span> <b>那棵树现在的样子</b>（含还没提交的改动）：<br>
       出来是一条<b>新技能</b>，不跟那个变体沾亲。变体本身不动 —— 解绑要另外点。`
    : duplicating
    ? `整目录拷一份：正文、附件、类别、说明都跟着走，名字换成新的。复制出来是<b>未发布</b>，满意了再发布。`
    : `一个目录 + 一份 <code>SKILL.md</code> + 一份 <code>nous.json</code>。建完是<b>未发布</b>：
        只在你这台、远端上看不到，满意了再发布。`;
  $("create-name").value = name;
  // 分组预填源那条住的抽屉（`deprecated/demo` → `deprecated`），默认留在原地。
  if (duplicating) {
    const home = skillNow(duplicating)?.path ?? "";
    const cut = home.lastIndexOf("/");
    $("create-group").value = cut > 0 ? home.slice(0, cut) : "";
  }
  $("create-scrim").hidden = false;
  $("create").hidden = false;
  renderCreate();
  $("create-name").focus();
  $("create-name").select();
}

function closeCreate() {
  $("create-scrim").hidden = true;
  $("create").hidden = true;
  state.creating = false;
  state.duplicating = null;
  state.duplicatingVariant = null;
}

async function submitCreate(event) {
  event.preventDefault();
  const verdict = validateCreate();
  if (!verdict.ok) return renderCreate();
  const name = $("create-name").value.trim();
  const description = $("create-description").value.trim();
  const category = normalizeCategory($("create-cat").value);
  const group = normalizeCategory($("create-group").value);
  const from = state.duplicating;
  state.creating = true;
  renderCreate();
  $("create-err").hidden = true;
  $("create-ok").textContent = from ? "复制中…" : "建…";
  try {
    if (from) {
      await transport.post("v1/skills/duplicate", {
        from,
        name,
        category: category ? [category] : [],
        group,
        variant: state.duplicatingVariant,
      });
      closeCreate();
      await load({ quiet: true });
      openDetail(name);
      toast(`已复制成 ${name} · 未发布（历史没跟着走）`);
      return;
    }
    await transport.post("v1/skills/create", {
      name,
      description,
      category: category ? [category] : [],
      group,
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
    $("create-ok").textContent = state.duplicating ? "复制" : "建";
    if (!$("create").hidden) renderCreate();
  }
}

// ── 变体：一条技能的多个可装载名字 ───────────────────────────────────────────
//
// 跟扩展面板（vbe4pt）同一副骨头：名册里变体是**另一个注册名**（`tdd@strict`），
// 界面上它是本体**详情里的一行** —— 这一层只有四个动作，跟那边一一对应：
// 绑定（开一棵 tree）/ 改正文 › / 独立成技能 › / 解绑。
//
// 一条铁律：`name` 永远是**本名**，变体 id 单独送（`variant=`）。拼成 `本名@id` 再让服务端
// 拆开，等于让路径那一层去理解会话键 —— 扩展那边踩过，别再来一次。

const VARIANT_ID_RE = /^[a-z0-9][a-z0-9._-]*$/;

/** 详情里的那一行：一条变体一个 chip，圆点就是装载状态，`+` 是新增。 */
function variantStrip(skill, variants) {
  const wrap = el("div", "variants");
  wrap.append(el("span", "variants-label", "变体"));
  for (const item of variants) {
    const row = el("button", "variant");
    row.type = "button";
    row.dataset.tone = item.load?.state === "failed" ? "bad" : item.installed ? "ok" : "off";
    row.append(el("span", "variant-id", item.id));
    row.append(el("span", "variant-src", item.branch ? "分支" : "目录"));
    row.title = `${item.key}\n${item.dir ?? ""}${item.branch ? `\n分支 ${item.branch}` : ""}`;
    row.onclick = () => openVariantDialog(skill, item);
    wrap.append(row);
  }
  const add = el("button", "variant-add", "+");
  add.type = "button";
  add.title = `给 ${splitSkillKey(skill.name).family} 加一个变体（另一个可装载名字）`;
  add.onclick = () => openVariantDialog(skill, null);
  wrap.append(add);
  return wrap;
}

/** 路径和分支名都是**算**出来的，跟着当前技能和变体 id 走。别写死示例：对不上比空着更糟。 */
function variantTargets() {
  const skill = state.variant?.skill;
  const family = skill ? splitSkillKey(skill.name).family : "技能";
  const id = $("variant-id").value.trim() || "变体";
  const repo = state.data?.root ?? "/opt/Nous-skills";
  const parent = repo.replace(/\/+$/, "").replace(/\/[^/]+$/, "");
  return { family, id, branch: `skill/${family}-${id}`, worktree: `${parent}/nous-skill-${family}-${id}` };
}

function renderVariantHints() {
  const t = variantTargets();
  $("variant-where").textContent = `树 ${t.worktree} · 分支 ${t.branch}`;
  $("variant-where").title = `${t.worktree}\n分支 ${t.branch}`;
}

/**
 * `item` 为空 = 新增（要那两格输入）；有 item = 看/管这一条（字段收起来，
 * 摆出它现在在哪棵树、哪条分支、名册里装没装上）。
 */
function openVariantDialog(skill, item) {
  closeMenu();
  state.variant = { skill, item };
  $("variant").reset();
  $("variant-err").hidden = true;
  $("variant-title").textContent = item ? `${splitSkillKey(skill.name).family}@${item.id}` : `给 ${splitSkillKey(skill.name).family} 加变体`;
  $("variant-fields").hidden = Boolean(item);
  $("variant-code").hidden = true;
  $("variant-promote").hidden = true;
  $("variant-where").textContent = "";
  $("variant-info").hidden = !item;
  $("variant-remove").hidden = !item;
  $("variant-ok").hidden = Boolean(item);
  if (item) {
    // 只读的一段说明：变体这东西的真相在 git 里，面板上别再开第二个编辑面。
    $("variant-info").textContent = [
      "本机绑定：正文在那棵 checkout 上，只在这台存在；分支 push 上去，别的机器绑一下就有。",
      item.branch ? `分支：${item.branch}` : null,
      item.dir ? `树：${item.dir}` : null,
      item.load?.state === "failed" ? `装载失败：${item.load.error}` : item.installed ? "名册里：已装载" : "名册里：没装上",
    ]
      .filter(Boolean)
      .join("\n");
    $("variant-code").hidden = false;
    $("variant-promote").hidden = false;
    $("variant-remove").textContent = item.branch ? "拆树并解绑" : "解绑";
  } else {
    renderVariantHints();
    setTimeout(() => $("variant-id").focus(), 0);
  }
  $("variant-scrim").hidden = false;
  $("variant").hidden = false;
}

function closeVariantDialog() {
  $("variant-scrim").hidden = true;
  $("variant").hidden = true;
  state.variant = null;
}

async function submitVariant(event) {
  event.preventDefault();
  const ctx = state.variant;
  if (!ctx?.skill) return;
  const err = $("variant-err");
  err.hidden = true;
  const id = $("variant-id").value.trim();
  if (!VARIANT_ID_RE.test(id) || id.includes(VARIANT_SEP)) {
    err.textContent = "变体 id 只能用字母、数字和 . _ -（不带 @）";
    err.hidden = false;
    return;
  }
  const family = splitSkillKey(ctx.skill.name).family;
  const ok = $("variant-ok");
  ok.disabled = true;
  ok.textContent = "开树…";
  try {
    const result = await transport.post("v1/skills/variant", {
      op: "bind",
      name: family,
      id,
      branch: $("variant-branch").value.trim(),
    });
    closeVariantDialog();
    await load({ quiet: true });
    const key = `${family}${VARIANT_SEP}${id}`;
    const failed = (result.skills ?? []).find((skill) => skill.name === key)?.load?.state === "failed";
    toast(failed ? `${key} 绑上了，但装载失败（卡上是红的）` : `已加变体 ${key}`, failed ? "bad" : "");
  } catch (error) {
    err.textContent = String(error?.message ?? error);
    err.hidden = false;
  } finally {
    ok.disabled = false;
    ok.textContent = "绑定";
  }
}

/**
 * 解绑 / 拆树。有分支 = 连树带分支一起拆（要手输确认，跟归档同一套骨头）；
 * 没分支（老式的「登记型」变体）只摘记录。
 */
async function removeVariant() {
  const ctx = state.variant;
  const item = ctx?.item;
  if (!item) return;
  const family = splitSkillKey(ctx.skill.name).family;
  const err = $("variant-err");
  err.hidden = true;

  const prune = Boolean(item.branch);
  const run = async () => {
    const btn = $("variant-remove");
    btn.disabled = true;
    try {
      const result = await transport.post("v1/skills/variant", {
        op: prune ? "prune" : "unbind",
        name: family,
        id: item.id,
      });
      closeVariantDialog();
      await load({ quiet: true });
      if (!prune) return toast(`已解绑 ${item.key} —— 树和分支都留着`);
      const w = result.worktree ?? {};
      if (w.remoteError) {
        toast(`树和本地分支已删，但 GitHub 那条没删掉：\n${w.remoteError}`, "bad");
      } else if (w.branchDeleted) {
        toast(`已拆树并删掉分支 ${w.branch}${w.remoteDeleted ? "（本地 + GitHub）" : "（本地；GitHub 上本来没有）"}`);
      } else {
        toast(`已拆树并解绑 ${item.key}（分支 ${w.branch} 没删掉，去命令行看看）`, "bad");
      }
    } catch (error) {
      err.textContent = String(error?.message ?? error);
      err.hidden = false;
    } finally {
      btn.disabled = false;
    }
  };

  // 分支上有没并进 main 的提交 = 真会丢东西 → 手输分支名再删。
  // 空壳 / 已合并的分支没什么可惜的，直接拆（跟扩展面板同一条规矩）。
  if (item.branch && item.unmerged > 0) {
    // 先关弹层再开确认框：两层叠着时，确认框的按钮点不到（被上层的 form 吞掉）。
    closeVariantDialog();
    const remoteNote = item.remoteBranch ? "，GitHub 上那条也一起删" : "";
    confirmName({
      title: `拆树并解绑 ${item.key}？`,
      hint: `分支 ${item.branch} 上有 ${item.unmerged} 个提交没并进 main${remoteNote}。删了这些代码就只能从 reflog 里捞。`,
      name: item.branch,
      fieldLabel: "手输分支名",
      label: "删掉",
      onConfirm: run,
    });
    return;
  }
  await run();
}

/** `改正文 ›`：打开的是**那棵树**的文件树 —— 读写、差异、发布全跟着走。 */
function openVariantCode() {
  const ctx = state.variant;
  if (!ctx?.item) return;
  const key = `${splitSkillKey(ctx.skill.name).family}${VARIANT_SEP}${ctx.item.id}`;
  const skill = skillNow(key);
  if (!skill) return toast(`名册里没有 ${key} —— 先重扫一下`, "bad");
  closeVariantDialog();
  void openEditor(skill);
}

/** `独立成技能 ›`：把那棵树的正文拷成**一条新技能**（历史不带），变体本身不动。 */
function promoteVariant() {
  const ctx = state.variant;
  if (!ctx?.item) return;
  const family = splitSkillKey(ctx.skill.name).family;
  closeVariantDialog();
  openDuplicate(family, { variantId: ctx.item.id });
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
  // 打字搜东西 = 回列表：浮层和它读过的正文一起收掉（不然搜完背后还压着一层）。
  state.detail = null;
  state.detailBody = null;
  state.editing = null;
  $("grid").hidden = false;
  $("detail").hidden = true;
  $("dt-scrim").hidden = true;
  renderGrid();
};
$("refresh").onclick = () => void load();

// 牌子 = 回这一块的正身（`/`）。站在 `/upstream` 上时它是「回去」最自然的那一下，
// 所以它是个真链接（`href="./"`），只是普通左键不重载页面。
$("home").onclick = (event) => {
  if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0) return;
  event.preventDefault();
  if (state.upstreamOpen) return closeUpstream();
  closeSheet();
};
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
$("variant-close").onclick = () => closeVariantDialog();
$("variant-scrim").onclick = () => closeVariantDialog();
$("variant").addEventListener("submit", submitVariant);
$("variant-remove").onclick = () => void removeVariant();
$("variant-code").onclick = () => openVariantCode();
$("variant-promote").onclick = () => promoteVariant();
// 树和分支名是算出来的：id 一变就得重算，不然那一行会挂着上一次的示例。
$("variant-id").addEventListener("input", renderVariantHints);
$("create-scrim").onclick = () => closeCreate();
$("create").addEventListener("submit", submitCreate);
for (const id of ["create-name", "create-description", "create-cat", "create-group"]) {
  const node = $(id);
  node.addEventListener("input", renderCreate);
  node.addEventListener("change", renderCreate);
}
// 回车一格一格往下走：名字 → 说明 → 类别 → 建。别在第一格就提交。
for (const [id, next] of [
  ["create-name", "create-description"],
  ["create-description", "create-cat"],
  ["create-cat", "create-group"],
]) {
  $(id).addEventListener("keydown", (event) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    const target = $(next);
    // 复制时「说明」那一格是收起来的：跳进一个看不见的输入框等于把回车吃掉。
    if (target.closest("[hidden]")) return $("create").requestSubmit();
    target.focus();
  });
}

// 抽屉的交互：Esc 关、Ctrl/⌘+S 保存、Tab 缩进两格（不跳出编辑框）。
$("ed-close").onclick = () => closeEditor();
$("ed-scrim").onclick = () => closeEditor();

// 详情浮层：遮罩和 × 都只是「关掉」一个动作。
$("sync").onclick = () => (state.pendingOpen ? closePending() : openPending());
$("dt-scrim").onclick = () => closeSheet();
$("dt-close").onclick = () => closeSheet();
$("ed-add-file").onclick = () => openNewEntry("file");
$("ed-add-dir").onclick = () => openNewEntry("dir");
$("ed-new-cancel").onclick = () => closeNewEntry();
$("ed-new").addEventListener("submit", submitNewEntry);
$("ed-new-path").addEventListener("keydown", (event) => {
  // Esc 在这一格自己收掉，别再往上传 —— 一次 Esc 关两层是最烦的那种。
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    closeNewEntry();
  }
});
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
  if (!$("ed-new").hidden) {
    closeNewEntry();
    return;
  }
  // 变体弹层是从详情/卡片上开的，比新建层更靠外：Esc 先关它。
  if (!$("variant").hidden) return closeVariantDialog();
  if (!$("create").hidden) return closeCreate();
  if (state.editor) {
    if (state.editor.mode === "diff") setEditorMode("edit");
    else closeEditor();
    return;
  }
  // 上游是主区换页，详情是浮层：Esc 一层一层往回剥（浮层先走）。
  if (state.upstreamDetail) return closeUpstreamDetail();
  if (state.detail) return closeDetail();
  if (state.upstreamOpen) return closeUpstream();
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

const firstPaint = load();

// 上游清单跟着一起读（服务端只读本地镜像，所以这一条不联网、也不拖慢首屏）。
// 地址直接落在哪一屏就开哪一屏（贴给别人、刷新、cmd-click 都靠这个）。
// `/upstream` 要等上游清单读回来才有东西可画，所以挂在它的 then 后面。
void loadUpstream({ quiet: true }).then(() => {
  migrateHash(); // `#upstream/...` → `/upstream/...`，只有这一下会改地址（replace，不留历史）
  applyRoute();
});

// `/pending` 要等技能清单回来才有东西可画（`openPending` 空手就返回），所以再来一趟：
// 谁慢都不影响「地址说哪一屏就哪一屏」。
void firstPaint.then(() => {
  if (routeOf().view === "pending") applyRoute();
});

// n8qkkj · Nous 触发器清单。数据全来自这一块自己的 `v1/triggers`（server.js）。
//
// 触发器 = 一个独立的 Rust 项目。Nous 在这件事里只干「管项目」：列出来、改源码、
// 编过再提交。所以这一页没有「装载」「运行」这些状态 —— 那是会话引擎在起新对话时的事。
//
// 三条线：
//   · 保存 ≠ 发布。保存只落盘；发布才 cargo check + commit + push。
//   · 类别写回仓库（每个 nous.json 的 category + categories.json 词表），8 秒 debounce 推。
//   · 名字是身份（目录名 = 清单 name = 二进制名），所以页面上不提供改名。

import { buildDiff } from "./diff.js";

const SELF = location.pathname.endsWith("/") ? location.pathname : location.pathname + "/";
const API = `${SELF}v1/triggers`;
const $ = (id) => document.getElementById(id);
const $sel = (sel) => document.querySelector(sel);

async function readJson(res) {
  const body = await res.json().catch(() => null);
  if (!res.ok) {
    const error = new Error(body?.error ?? `HTTP ${res.status}`);
    error.status = res.status;
    throw error;
  }
  return body ?? {};
}

const get = (url) => fetch(url).then(readJson);
const post = (url, payload, opts = {}) =>
  fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(payload ?? {}),
    ...opts,
  }).then(readJson);

let toastTimer;
function toast(msg, kind = "") {
  const node = $("toast");
  node.textContent = msg;
  node.dataset.kind = kind;
  node.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (node.hidden = true), 3600);
}

const el = (tag, className, text) => {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
};

const short = (sha) => (sha ? String(sha).slice(0, 7) : "?");
const categoriesOf = (trigger) => (Array.isArray(trigger?.category) ? trigger.category.filter(Boolean) : []);

// 类别是路径：`日常/复盘`。父子关系不存，靠 `/` 推 —— 跟服务端同一套规矩。
const CAT_SEP = "/";
const ancestorsOf = (name) => {
  const parts = name.split(CAT_SEP);
  return parts.slice(0, -1).map((_, index) => parts.slice(0, index + 1).join(CAT_SEP));
};
const isUnder = (name, root) => name === root || name.startsWith(root + CAT_SEP);
const depthOf = (name) => name.split(CAT_SEP).length - 1;
const leafOf = (name) => name.split(CAT_SEP).at(-1);
const compareCats = (a, b) => {
  const left = a.split(CAT_SEP);
  const right = b.split(CAT_SEP);
  const common = Math.min(left.length, right.length);
  for (let i = 0; i < common; i++) {
    const cmp = left[i].localeCompare(right[i], "zh-CN");
    if (cmp) return cmp;
  }
  return left.length - right.length;
};

// 色砖：色相由名字决定，同一个触发器永远同一个颜色。
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
  data: null,
  ctx: null,
  filter: "", // "" = 全部；"__none__" = 未分类；其它 = 类别名
  query: "",
  creating: false, // 左栏「新建类别」的输入框开着
  createPrefix: "", // 「新建子类别」预填的 `工具/`
  building: false, // 新建触发器弹窗正在提交
  renaming: null, // 正在原地改名的类别
  editing: null, // 正在输入类别的触发器名（只住在详情里）
  detail: null, // 详情里是哪个触发器；null = 列表
  dirty: 0, // 页面自己记的「有类别写入待推」
  ahead: 0,
  behind: 0,
  sync: "idle", // idle | pending | pushing | bad
  syncError: "",
  editor: null,
};
const ARCHIVED = "__archived__";
const NONE = "__none__";
const SYNC_DEBOUNCE_MS = 8000;
let syncTimer = null;

const oursNow = () => state.data?.catalog ?? [];
const findTrigger = (name) => oursNow().find((item) => item.name === name);

/**
 * 状态只有一种优先级，卡片和筛选条共用这一份：
 * 清单坏了 > 未发布（HEAD 里没有）> 有改动（改了没发布）> 已发布。
 */
const statusOf = (trigger) => {
  if (trigger.error) return "broken";
  if (trigger.tracked === false) return "unpublished";
  if (trigger.dirty) return "dirty";
  return "ok";
};
const statusValueOf = (trigger) => `__${statusOf(trigger)}__`;
const STATUS_VIEWS = [
  { key: "ok", value: "__ok__", label: "已发布", tone: "ok", title: "进了 HEAD，也没有没提交的改动" },
  { key: "dirty", value: "__dirty__", label: "有改动", tone: "bad", title: "改过还没发布" },
  { key: "unpublished", value: "__unpublished__", label: "未发布", tone: "warn", title: "磁盘上有，HEAD 里没有" },
  { key: "broken", value: "__broken__", label: "清单坏了", tone: "gone", title: "nous.json 缺字段，或者跟目录名对不上" },
];
const isStatusFilter = (value) => STATUS_VIEWS.some((view) => view.value === value);

// ── 同步 ───────────────────────────────────────────────────────────────────
//
// 只自动推「词表 / 清单」这几种文件（server 端 syncMeta 只 add 这三类）。
// 代码不在这条路上 —— 改过 .rs 的卡片会自己变成「有改动」，得人去抽屉里点发布。

function renderSync() {
  const chip = $("sync");
  const dot = $sel("#sync .dot");
  const label = chip.querySelector(".sync-label");
  const cat = oursNow();
  const unpublished = cat.filter((item) => item.tracked === false && !item.error).length;
  const dirtyCode = cat.filter((item) => item.dirty && item.tracked !== false).length;

  let tone = "ok";
  let text = "已同步";
  let title = "词表 / 清单改完 8 秒内自动提交并推送；代码得在抽屉里点发布";

  if (state.sync === "pushing") {
    tone = "wait";
    text = "推送中…";
  } else if (state.sync === "bad") {
    tone = "bad";
    text = "同步失败";
    title = state.syncError || "点我重试";
  } else if (state.behind > 0) {
    tone = "bad";
    text = "远端有新提交";
    title = "origin 比本地新 —— 先拉一下再改，否则 push 会被拒";
  } else if (state.sync === "pending" || state.dirty > 0) {
    tone = "wait";
    text = "待同步";
    title = "类别改动还没推上去 —— 点我立刻推（只推词表和清单，不碰代码）";
  } else if (unpublished > 0) {
    tone = "wait";
    text = `${unpublished} 个未发布`;
    title = "磁盘上有、仓库里没有 —— 换台机器看不到它们；点卡片进抽屉发布";
  } else if (dirtyCode > 0) {
    tone = "wait";
    text = `${dirtyCode} 个有改动`;
    title = "代码改了没发布 —— 在抽屉里点「发布」（先过 cargo check）";
  } else if (state.ahead > 0) {
    tone = "wait";
    text = `${state.ahead} 个提交没推`;
    title = "本地提交了，还没 push —— 点我推";
  }

  chip.hidden = !state.data;
  chip.dataset.state = tone;
  dot.dataset.state = tone;
  label.textContent = text;
  chip.title = title;
}

async function flushSync() {
  if (syncTimer) {
    clearTimeout(syncTimer);
    syncTimer = null;
  }
  if (!state.data) return;
  state.dirty = 0;
  state.sync = "pushing";
  renderSync();
  try {
    const result = await post(`${API}/sync`, {}, { keepalive: true });
    state.ahead = result.ahead ?? 0;
    state.behind = result.behind ?? 0;
    if (result.pushed === false) {
      state.sync = "bad";
      state.syncError = result.pushError || "git push 失败";
      toast(`推送失败：${state.syncError}`, "bad");
    } else {
      state.sync = "idle";
      state.syncError = "";
      if (result.committed) toast("词表 / 清单已提交并推送");
    }
  } catch (error) {
    state.sync = "bad";
    state.syncError = String(error?.message ?? error);
    toast(`同步失败：${state.syncError}`, "bad");
  }
  renderSync();
  void load({ quiet: true });
}

function scheduleSync() {
  state.dirty++;
  state.sync = "pending";
  renderSync();
  if (syncTimer) clearTimeout(syncTimer);
  syncTimer = setTimeout(() => void flushSync(), SYNC_DEBOUNCE_MS);
}

function flushOnHide() {
  if (!syncTimer && !state.dirty) return;
  if (syncTimer) clearTimeout(syncTimer);
  syncTimer = null;
  state.dirty = 0;
  void fetch(`${API}/sync`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: "{}",
    keepalive: true,
  }).catch(() => {});
}

addEventListener("pagehide", flushOnHide);
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "hidden") flushOnHide();
});

// ── 类别：查询 ─────────────────────────────────────────────────────────────

function allCategoryNames() {
  const set = new Set();
  const add = (value) => {
    const name = String(value ?? "").trim();
    if (!name) return;
    for (const parent of ancestorsOf(name)) set.add(parent);
    set.add(name);
  };
  for (const name of Array.isArray(state.data?.categories) ? state.data.categories : []) add(name);
  for (const trigger of oursNow()) for (const tag of categoriesOf(trigger)) add(tag);
  return [...set].sort(compareCats);
}

function categoryCounts() {
  const counts = new Map();
  let none = 0;
  for (const trigger of oursNow()) {
    const tags = categoriesOf(trigger);
    if (!tags.length) none++;
    // 父级计数含后代；同一个触发器同时挂父和子只算一次。
    const applied = new Set();
    for (const tag of tags) {
      for (const parent of ancestorsOf(tag)) applied.add(parent);
      applied.add(tag);
    }
    for (const name of applied) counts.set(name, (counts.get(name) ?? 0) + 1);
  }
  return { counts, none };
}
const countOf = (name) => categoryCounts().counts.get(name) ?? 0;

// ── 类别：写回仓库 ─────────────────────────────────────────────────────────

async function saveCategories(name, categories) {
  const trigger = findTrigger(name);
  if (!trigger) return;
  const before = categoriesOf(trigger).slice();
  const next = [...new Set(categories)].sort();
  trigger.category = next;
  render();
  try {
    const result = await post(`${API}/category`, { name, category: next });
    if (Array.isArray(result.catalog)) state.data.catalog = result.catalog;
    if (Array.isArray(result.categories)) state.data.categories = result.categories;
    render();
    scheduleSync();
  } catch (error) {
    toast(`改类别失败：${error.message}`, "bad");
    trigger.category = before;
    render();
    void load({ quiet: true });
  }
}

function addTag(name, value) {
  const tag = String(value ?? "").trim();
  if (!tag) return;
  const trigger = findTrigger(name);
  if (!trigger) return;
  const set = new Set(categoriesOf(trigger));
  if (set.has(tag)) return;
  set.add(tag);
  void saveCategories(name, [...set]);
}

function removeTag(name, value) {
  const trigger = findTrigger(name);
  if (!trigger) return;
  void saveCategories(
    name,
    categoriesOf(trigger).filter((tag) => tag !== value),
  );
}

async function addCategory(value) {
  const name = String(value ?? "").trim();
  if (!name) return;
  try {
    const result = await post(`${API}/categories`, { op: "add", name });
    if (Array.isArray(result.categories)) state.data.categories = result.categories;
    render();
    scheduleSync();
  } catch (error) {
    toast(`新建类别失败：${error.message}`, "bad");
  }
}

async function renameCategory(name, next) {
  const value = String(next ?? "").trim();
  if (!value || value === name) return;
  try {
    const result = await post(`${API}/categories`, { op: "rename", name, to: value });
    if (Array.isArray(result.categories)) state.data.categories = result.categories;
    if (Array.isArray(result.catalog)) state.data.catalog = result.catalog;
    if (state.filter === name) state.filter = value;
    render();
    scheduleSync();
    toast(`「${name}」改名为「${value}」`);
  } catch (error) {
    toast(`改名失败：${error.message}`, "bad");
  }
}

async function removeCategory(name) {
  const names = allCategoryNames();
  const subs = names.filter((candidate) => candidate !== name && isUnder(candidate, name)).length;
  const count = countOf(name);
  const scope = `类别「${name}」${subs ? `连同 ${subs} 个子类别` : ""}`;
  const message = count ? `删除${scope}？会从 ${count} 个触发器上摘掉。` : `删除${scope}？（空类别）`;
  if (!confirm(message)) return;
  try {
    const result = await post(`${API}/categories`, { op: "remove", name });
    if (Array.isArray(result.categories)) state.data.categories = result.categories;
    if (Array.isArray(result.catalog)) state.data.catalog = result.catalog;
    if (state.filter === name) state.filter = "";
    render();
    scheduleSync();
  } catch (error) {
    toast(`删除类别失败：${error.message}`, "bad");
  }
}

// ── 右键 / 长按菜单 ────────────────────────────────────────────────────────
//
// 行尾的 ⋯ 是主入口，右键 / 手机长按 / 键盘 ContextMenu 是快捷方式。
// 菜单是个空壳 div，条目现拼 —— 加一项就多 push 一条，别处不用动。

const LONG_PRESS_MS = 520;
const LONG_PRESS_SLOP = 10;
let menuOpen = false;
let menuTeardown = null;

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

function mountMenu(menu, x, y, focusTarget) {
  menu.hidden = false;
  menuOpen = true;
  // 先挂上去量尺寸，再夹进视口 —— 否则贴底的那一行菜单会跑到屏幕外面。
  const rect = menu.getBoundingClientRect();
  const left = Math.max(8, Math.min(x, innerWidth - rect.width - 8));
  const top = Math.max(8, Math.min(y, innerHeight - rect.height - 8));
  menu.style.left = `${left}px`;
  menu.style.top = `${top}px`;
  focusTarget?.focus?.();

  const onDown = (event) => {
    if (menu.contains(event.target)) return;
    closeMenu();
  };
  const onKey = (event) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      closeMenu();
    }
  };
  setTimeout(() => document.addEventListener("mousedown", onDown, true), 0);
  document.addEventListener("keydown", onKey, true);
  menuTeardown = () => {
    document.removeEventListener("mousedown", onDown, true);
    document.removeEventListener("keydown", onKey, true);
  };
}

function menuItem(menu, { label, hint, danger = false, disabled = false, onPick }) {
  const item = el("button", "ctx-item" + (danger ? " ctx-danger" : ""));
  item.type = "button";
  item.setAttribute("role", "menuitem");
  item.disabled = disabled;
  item.append(el("span", "ctx-item-label", label));
  if (hint) item.append(el("span", "ctx-item-hint", hint));
  item.onclick = () => {
    closeMenu();
    onPick();
  };
  menu.append(item);
  return item;
}

function openCategoryMenu(x, y, name) {
  closeMenu();
  const menu = $("ctxmenu");
  menu.textContent = "";

  menuItem(menu, {
    label: "新建子类别",
    hint: `${name}/…`,
    onPick: () => {
      state.creating = true;
      state.createPrefix = `${name}/`;
      render();
      requestAnimationFrame(() => $(".rail-input")?.focus());
    },
  });
  menuItem(menu, {
    label: "改名",
    hint: `${countOf(name)} 个`,
    onPick: () => {
      state.renaming = name;
      render();
      requestAnimationFrame(() => {
        const input = $(".facet-input");
        input?.focus();
        input?.setSelectionRange(input.value.length, input.value.length);
      });
    },
  });
  menu.append(el("div", "ctx-sep"));
  menuItem(menu, {
    label: "删除类别",
    hint: "从所有触发器上摘掉",
    danger: true,
    onPick: () => void removeCategory(name),
  });
  mountMenu(menu, x, y, menu.querySelector(".ctx-item"));
}

function openCardMenu(x, y, trigger) {
  closeMenu();
  const menu = $("ctxmenu");
  menu.textContent = "";
  const status = statusOf(trigger);

  menuItem(menu, { label: "看代码", hint: "src/main.rs", onPick: () => void openEditor(trigger) });
  menuItem(menu, {
    label: "改类别",
    hint: categoriesOf(trigger).length ? `${categoriesOf(trigger).length} 个` : "未分类",
    onPick: () => openDetail(trigger.name, { editTags: true }),
  });
  menuItem(menu, { label: "复制信息", hint: "给 AI 一条 cat", onPick: () => void copyInfo(trigger) });
  menu.append(el("div", "ctx-sep"));
  menuItem(menu, {
    label: status === "unpublished" ? "删掉" : "归档",
    hint: status === "unpublished" ? "没发布过，删了就没有" : "移到 .archive/ 并推送",
    danger: true,
    onPick: () => void archiveTrigger(trigger.name, { unpublished: status === "unpublished" }),
  });
  mountMenu(menu, x, y, menu.querySelector(".ctx-item"));
}

function attachMenuGesture(node, open) {
  let timer = null;
  let start = null;
  node.addEventListener("pointerdown", (event) => {
    if (event.pointerType === "mouse") return;
    start = { x: event.clientX, y: event.clientY };
    timer = setTimeout(() => {
      timer = null;
      open(start.x, start.y);
    }, LONG_PRESS_MS);
  });
  const cancel = () => {
    if (timer) clearTimeout(timer);
    timer = null;
  };
  node.addEventListener("pointerup", cancel);
  node.addEventListener("pointercancel", cancel);
  node.addEventListener("pointermove", (event) => {
    if (!timer || !start) return;
    if (Math.abs(event.clientX - start.x) > LONG_PRESS_SLOP || Math.abs(event.clientY - start.y) > LONG_PRESS_SLOP) cancel();
  });
  node.addEventListener("contextmenu", (event) => {
    if (timer === null && start) {
      // 长按已经出过菜单了：别让系统菜单再叠一层。
      event.preventDefault();
    }
  });
}

/** 给 AI 的一条 cat：它要能拿着这段话直接开工，不用再问仓库在哪。 */
async function copyInfo(trigger) {
  const repo = state.data?.repo?.path ?? "/opt/Nous-Trigger";
  const text = [
    `仓库 ${repo}，我要改触发器「${trigger.name}」（v${trigger.version}）。`,
    `先读 ${repo}/README.md 和 index.mjs，再读 ${trigger.name}/nous.json、${trigger.name}/Cargo.toml、${trigger.name}/src/main.rs。`,
    `用三句话讲清它现在干什么、你觉得哪里值得改；我点头你再动代码。`,
    `改完跑 cargo check，再跑 node index.mjs，然后 git commit + git push（只提交 ${trigger.name}/ 和 index.json）。`,
    `别碰别的触发器。`,
  ].join("\n");
  try {
    await navigator.clipboard.writeText(text);
    toast("已复制：把这段贴给 agent");
  } catch {
    toast("复制失败 —— 浏览器不给权限", "bad");
  }
}

// ── 卡片上的类别 ───────────────────────────────────────────────────────────

function tagRow(trigger, { readOnly = false, className = "product-tags" } = {}) {
  const row = el("div", `tags ${className}`);
  const cats = categoriesOf(trigger);

  for (const tag of cats) {
    const chip = el("span", "tag");
    const parts = tag.split(CAT_SEP);
    if (parts.length > 1) {
      chip.append(el("span", "tag-prefix", `${parts.slice(0, -1).join(CAT_SEP)}/`));
    }
    chip.append(el("span", "tag-name", parts.at(-1)));
    chip.title = tag;
    if (!readOnly) {
      const x = el("button", "tag-x", "×");
      x.type = "button";
      x.title = `从 ${trigger.name} 上摘掉「${tag}」`;
      x.onclick = (event) => {
        event.stopPropagation();
        removeTag(trigger.name, tag);
      };
      chip.append(x);
    }
    row.append(chip);
  }

  if (readOnly) {
    if (!cats.length) row.append(el("span", "dim small", "未分类"));
    row.classList.add("tag-ro");
    return row;
  }

  if (state.editing === trigger.name) {
    const input = el("input", "tag-input");
    input.type = "text";
    input.placeholder = "工具/网络";
    input.setAttribute("list", "tag-suggestions");
    const commit = () => {
      const value = input.value.trim();
      state.editing = null;
      if (value) addTag(trigger.name, value);
      else render();
    };
    input.onkeydown = (event) => {
      event.stopPropagation();
      if (event.key === "Enter") {
        event.preventDefault();
        commit();
      } else if (event.key === "Escape") {
        event.preventDefault();
        state.editing = null;
        render();
      }
    };
    input.onblur = () => {
      if (state.editing === trigger.name) commit();
    };
    // 挂进文档之后再 focus：还在内存里时聚焦会落空。
    requestAnimationFrame(() => input.focus());
    row.append(input);
  } else {
    const add = el("button", "tag-add", "+ 类别");
    add.type = "button";
    add.title = "给这个触发器加一个类别";
    add.onclick = (event) => {
      event.stopPropagation();
      state.editing = trigger.name;
      render();
    };
    row.append(add);
  }
  return row;
}

// ── 商品卡 ─────────────────────────────────────────────────────────────────

function productCard(trigger) {
  const status = statusOf(trigger);
  const card = el("article", `product ${status === "ok" ? "" : status}`.trim());
  card.tabIndex = 0;
  card.setAttribute("role", "button");
  card.dataset.name = trigger.name;

  const openMenuAt = (x, y) => openCardMenu(x, y, trigger);
  card.onclick = (event) => {
    if (event.target.closest("button")) return;
    openDetail(trigger.name);
  };
  card.oncontextmenu = (event) => {
    event.preventDefault();
    openMenuAt(event.clientX, event.clientY);
  };
  card.onkeydown = (event) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      openDetail(trigger.name);
    } else if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      event.preventDefault();
      const rect = card.getBoundingClientRect();
      openMenuAt(rect.left + 8, rect.bottom - 6);
    }
  };
  attachMenuGesture(card, openMenuAt);

  const visual = el("div", "visual");
  visual.style.setProperty("--hue", String(hueOf(trigger.name)));
  visual.append(el("span", "visual-mono", initialsOf(trigger.name)));
  if (status === "broken") visual.append(el("span", "badge-float fail", "清单坏了"));
  else if (status === "unpublished") visual.append(el("span", "badge-float unpub", "未发布"));
  else if (status === "dirty") visual.append(el("span", "badge-float bad", "有改动"));
  card.append(visual);

  const body = el("div", "product-body");
  const title = el("div", "product-title");
  title.append(el("span", "product-name", trigger.name));
  title.append(el("span", "product-ver", `v${trigger.version}`));
  const menu = el("button", "product-menu", "⋯");
  menu.type = "button";
  menu.title = "菜单（右键 / 长按同款）";
  menu.onclick = (event) => {
    event.stopPropagation();
    const rect = menu.getBoundingClientRect();
    openMenuAt(rect.left, rect.bottom + 4);
  };
  title.append(menu);
  body.append(title);
  body.append(el("p", "product-sum", trigger.summary || "没写说明。"));
  if (trigger.error) body.append(el("p", "product-err", trigger.error));
  card.append(body);

  card.append(tagRow(trigger, { readOnly: true }));

  const foot = el("div", "product-foot");
  const binary = el("span", "pmeta");
  binary.append(el("span", "pmeta-k", "产物"));
  binary.append(el("span", "pmeta-v mono", trigger.binary));
  foot.append(binary);
  const rust = el("span", "pmeta");
  rust.append(el("span", "pmeta-k", "Rust"));
  rust.append(el("span", "pmeta-v", trigger.tracked === false ? "未进 HEAD" : "已进 HEAD"));
  foot.append(rust);
  const params = Array.isArray(trigger.params) ? trigger.params : [];
  if (params.length) {
    const need = el("span", "pmeta");
    need.append(el("span", "pmeta-k", "参数"));
    need.append(el("span", "pmeta-v", String(params.length)));
    foot.append(need);
  }
  if (trigger.dirty) {
    const changed = el("span", "pmeta");
    changed.append(el("span", "pmeta-k", "改动"));
    changed.append(el("span", "pmeta-v", `${trigger.dirtyFiles} 个文件`));
    foot.append(changed);
  }
  card.append(foot);
  return card;
}

// ── 详情 ───────────────────────────────────────────────────────────────────

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

function openDetail(name, { editTags = false } = {}) {
  state.detail = name;
  state.editing = editTags ? name : null;
  render();
  $("dt-body").scrollTop = 0;
  if (!editTags) $("detail").focus({ preventScroll: true });
}

function closeDetail({ restoreFocus = true } = {}) {
  if (!state.detail) return;
  const name = state.detail;
  state.detail = null;
  state.editing = null;
  render();
  if (!restoreFocus) return;
  const card = [...document.querySelectorAll(".product")].find((node) => node.dataset.name === name);
  card?.focus({ preventScroll: true });
}

function renderDetail() {
  const host = $("dt-body");
  const trigger = findTrigger(state.detail);
  if (!trigger) {
    state.detail = null;
    state.editing = null;
    $("detail").hidden = true;
    $("dt-scrim").hidden = true;
    renderGrid();
    setBehindInert();
    return;
  }

  host.textContent = "";
  const cats = categoriesOf(trigger);
  const status = statusOf(trigger);
  const view = STATUS_VIEWS.find((item) => item.key === status);

  $("dt-name").textContent = trigger.name;
  $("dt-meta").textContent = `v${trigger.version} · ${cats.length ? cats.join(" · ") : "未分类"}`;
  $("dt-tile").style.setProperty("--hue", String(hueOf(trigger.name)));
  $("dt-mono").textContent = initialsOf(trigger.name);

  const badges = $("dt-badges");
  badges.textContent = "";
  const badge = el("span", "detail-badge", view?.label ?? "已发布");
  badge.dataset.tone = status === "broken" ? "fail" : status === "unpublished" ? "unpub" : status === "dirty" ? "bad" : "ok";
  badges.append(badge);

  host.append(el("p", "detail-sum", trigger.summary || "没写说明。"));

  const acts = el("div", "detail-acts");
  const code = el("button", "btn btn-small", "代码 ›");
  code.type = "button";
  code.title = `看 / 改 ${trigger.name} 的源码`;
  code.onclick = () => void openEditor(trigger);
  acts.append(code);
  if (status === "broken") {
    const fix = el("button", "btn btn-small", "去修清单");
    fix.type = "button";
    fix.title = "打开 nous.json —— 缺字段或 name 跟目录名对不上";
    fix.onclick = () => void openEditor(trigger, { path: "nous.json" });
    acts.append(fix);
  }
  host.append(acts);

  if (trigger.error) host.append(alertLine("bad", "清单不对", trigger.error));

  const grid = el("div", "detail-grid");

  const cat = detailSection("类别", "写回仓库 · 8 秒内同步");
  cat.body.append(tagRow(trigger, { className: "detail-tags" }));
  grid.append(cat.node);

  const list = detailSection("清单", "nous.json 写的");
  list.body.append(detailRow("版本", trigger.version));
  list.body.append(detailRow("产物名", trigger.binary, "cargo 编出来叫什么"));
  list.body.append(detailRow("目录", trigger.name, "目录名 = 清单 name = 身份"));
  list.body.append(detailRow("语言", "Rust · 独立项目"));
  grid.append(list.node);

  const params = Array.isArray(trigger.params) ? trigger.params : [];
  if (params.length) {
    const sec = detailSection("参数", `${params.length} 个 · 装配时填`);
    for (const param of params) {
      const shape = param.type === "select" ? `单选：${(param.options ?? []).join(" / ")}` : param.type;
      const row = detailRow(
        param.name,
        `${param.label ?? shape}${param.required ? " · 必填" : ""}`,
        param.help ?? `${param.name}（${shape}）`,
      );
      sec.body.append(row);
    }
    grid.append(sec.node);
  }

  const where = detailSection("仓库", "HEAD 里有没有");
  where.body.append(detailRow("发布", trigger.tracked === false ? "未发布 · HEAD 里没有" : "已进 HEAD"));
  where.body.append(detailRow("改动", trigger.dirty ? `${trigger.dirtyFiles} 个文件没提交` : "没有"));
  grid.append(where.node);

  host.append(grid);

  const input = host.querySelector(".tag-input");
  if (input) input.focus();
}

// ── 归档 ───────────────────────────────────────────────────────────────────

function renderArchivedGrid() {
  const grid = $("grid");
  grid.textContent = "";
  const rows = state.data?.archived ?? [];
  if (!rows.length) {
    grid.append(el("div", "empty", "归档里是空的。归档过的触发器会躺在这里，随时能搬回来。"));
    $("result-count").textContent = "0 个已归档";
    return;
  }
  const list = el("div", "arch-list");
  for (const row of rows) {
    const item = el("div", "arch-row");
    const main = el("div", "arch-main");
    const head = el("div", "arch-head");
    head.append(el("span", "arch-name", row.name));
    head.append(el("span", "arch-ver", `v${row.version ?? "?"}`));
    const when = el("span", "arch-when dim small", row.at ? new Date(row.at).toLocaleDateString("zh-CN") : "");
    head.append(when);
    main.append(head);
    main.append(el("p", "arch-sum", row.summary || "（没写说明）"));
    item.append(main);

    const acts = el("div", "arch-acts");
    const back = el("button", "btn btn-small", "恢复");
    back.type = "button";
    back.onclick = () => void restoreArchived(row.name);
    acts.append(back);
    const burn = el("button", "btn btn-small btn-bad", "彻底删除");
    burn.type = "button";
    burn.onclick = () => purgeArchived(row.name);
    acts.append(burn);
    item.append(acts);
    list.append(item);
  }
  grid.append(list);
  $("result-count").textContent = `${rows.length} 个已归档`;
}

async function restoreArchived(name) {
  try {
    const result = await post(`${API}/restore`, { name });
    if (result.pushed === false) toast(`已恢复，但推送失败：${result.pushError || "未知原因"}`, "bad");
    else toast(`${name} 已从归档搬回来`);
    await load({ quiet: true });
  } catch (error) {
    toast(`恢复失败：${error.message}`, "bad");
  }
}

function purgeArchived(name) {
  confirmName({
    title: `彻底删除 ${name}？`,
    hint: "连仓库里的 .archive/ 备份一起删掉，并提交 + 推送。之后只能靠 git 历史找回来。",
    name,
    label: "彻底删除",
    onConfirm: async () => {
      try {
        const result = await post(`${API}/purge`, { name });
        if (result.pushed === false) toast(`已提交，但推送失败：${result.pushError || "未知原因"}`, "bad");
        else toast(`${name} 已彻底删除`);
        await load({ quiet: true });
      } catch (error) {
        toast(`彻底删除失败：${error.message}`, "bad");
      }
    },
  });
}

// ── 破坏性动作的确认层 ─────────────────────────────────────────────────────

/** 会推上 GitHub 的动作要求手输名字。签名不对，按钮就一直灰着。 */
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

function archiveTrigger(name, { unpublished = false } = {}) {
  confirmName({
    // 没发布过的没有历史可留：归档它等于「先推上去再归档」，所以后端直接删掉。
    title: unpublished ? `删掉 ${name}？` : `从仓库里归档 ${name}？`,
    hint: unpublished
      ? "它还没发布过，仓库里从来就没有它 —— 没有备份、也没有历史可找。这一步就是把这台磁盘上的目录删掉。"
      : "整个项目目录会移到仓库的 .archive/ 并提交 + 推送。GitHub 上不再有它，但备份还在仓库里，随时能恢复。",
    name,
    label: unpublished ? "删掉" : "归档并推送",
    onConfirm: async () => {
      try {
        const result = await post(`${API}/archive`, { name });
        if (result.dropped) {
          toast(`已删掉 ${name}（没发布过，仓库里本来就没有）`);
          await load({ quiet: true });
          return;
        }
        if (result.pushed === false) toast(`已在本地提交，但推送失败：${result.pushError || "未知原因"}`, "bad");
        else toast(`已归档 ${name} · 左栏「已归档」里可以恢复`);
        await load({ quiet: true });
      } catch (error) {
        toast(`归档失败：${error.message}`, "bad");
      }
    },
  });
}

// ── 左栏 ───────────────────────────────────────────────────────────────────

function facetRow({ label, count, value, on, parent = false, menu = false, tone = "", depth = 0 }) {
  const row = el("div", `facet${on ? " on" : ""}${parent ? " parent" : ""}`);
  const main = el("button", "facet-main");
  main.type = "button";
  // 缩进用 padding 不用空格：`.facet-label` 是 nowrap，空白会被压成一个。
  if (depth) main.style.paddingLeft = `${8 + depth * 13}px`;
  if (value !== undefined) main.dataset.value = value;
  const text = el("span", "facet-label");
  if (tone) {
    const dot = el("span", "filter-dot");
    dot.dataset.tone = tone;
    text.append(dot);
  }
  text.append(document.createTextNode(label));
  main.append(text);
  if (count !== undefined) main.append(el("span", "facet-n", String(count)));
  main.onclick = () => {
    state.filter = value ?? "";
    state.detail = null;
    render();
  };
  row.append(main);

  if (menu) {
    const more = el("button", "facet-menu", "⋯");
    more.type = "button";
    more.title = "菜单（右键 / 长按同款）";
    more.onclick = (event) => {
      event.stopPropagation();
      const rect = more.getBoundingClientRect();
      openCategoryMenu(rect.left, rect.bottom + 4, value);
    };
    row.append(more);
    row.oncontextmenu = (event) => {
      event.preventDefault();
      openCategoryMenu(event.clientX, event.clientY, value);
    };
    attachMenuGesture(row, (x, y) => openCategoryMenu(x, y, value));
  }
  return row;
}

function renderFacets() {
  const host = $("facets");
  host.textContent = "";
  const { counts, none } = categoryCounts();
  const total = oursNow().length;
  const names = allCategoryNames();

  // 左栏只放「按类别找」，状态留给顶栏那条筛选条 —— 同一件事摆两处，
  // 人就分不清「我现在被哪一层筛着」。
  host.append(facetRow({ label: "全部", count: total, value: "", on: state.filter === "" }));
  host.append(facetRow({ label: "未分类", count: none, value: NONE, on: state.filter === NONE }));
  host.append(el("div", "ctx-sep"));

  for (const name of names) {
    if (state.renaming === name) {
      const row = el("div", "facet");
      const input = el("input", "facet-input");
      input.type = "text";
      input.value = name;
      const commit = () => {
        const next = input.value.trim();
        state.renaming = null;
        render();
        if (next && next !== name) void renameCategory(name, next);
      };
      input.onkeydown = (event) => {
        event.stopPropagation();
        if (event.key === "Enter") {
          event.preventDefault();
          commit();
        } else if (event.key === "Escape") {
          event.preventDefault();
          state.renaming = null;
          render();
        }
      };
      input.onblur = () => {
        if (state.renaming === name) commit();
      };
      row.append(input);
      host.append(row);
      requestAnimationFrame(() => input.focus());
      continue;
    }
    host.append(
      facetRow({
        label: leafOf(name),
        count: counts.get(name) ?? 0,
        value: name,
        depth: depthOf(name),
        on: state.filter === name,
        parent: depthOf(name) === 0 && names.some((other) => other !== name && isUnder(other, name)),
        menu: true,
      }),
    );
  }

  const archived = state.data?.archived?.length ?? 0;
  host.append(el("div", "ctx-sep"));
  host.append(
    facetRow({ label: "已归档", count: archived, value: ARCHIVED, on: state.filter === ARCHIVED }),
  );
  $("cat-count").textContent = String(names.length);
}

function renderRailNew() {
  const host = $("rail-new");
  host.textContent = "";
  if (!state.creating) {
    const add = el("button", "rail-add", "＋ 新建类别");
    add.type = "button";
    add.onclick = () => {
      state.creating = true;
      state.createPrefix = "";
      render();
      requestAnimationFrame(() => $(".rail-input")?.focus());
    };
    host.append(add);
    return;
  }
  const input = el("input", "rail-input");
  input.type = "text";
  input.value = state.createPrefix;
  input.placeholder = "里 / 里/外";
  const done = () => {
    const value = input.value.trim();
    state.creating = false;
    state.createPrefix = "";
    render();
    if (value) void addCategory(value);
  };
  input.onkeydown = (event) => {
    event.stopPropagation();
    if (event.key === "Enter") {
      event.preventDefault();
      done();
    } else if (event.key === "Escape") {
      event.preventDefault();
      state.creating = false;
      state.createPrefix = "";
      render();
    }
  };
  input.onblur = () => {
    if (state.creating) done();
  };
  host.append(input);
  requestAnimationFrame(() => {
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  });
}

function renderSuggestions() {
  const list = $("tag-suggestions");
  list.textContent = "";
  for (const name of allCategoryNames()) {
    const option = el("option");
    option.value = name;
    list.append(option);
  }
}

function renderFilters() {
  const host = $("filters");
  host.textContent = "";
  for (const view of STATUS_VIEWS) {
    const count = oursNow().filter((trigger) => statusValueOf(trigger) === view.value).length;
    const chip = el("button", "filter-chip");
    chip.type = "button";
    const dot = el("span", "filter-dot");
    dot.dataset.tone = view.tone;
    chip.append(dot);
    chip.append(el("span", "filter-label", view.label));
    chip.append(el("span", "filter-n", String(count)));
    chip.title = view.title;
    chip.setAttribute("aria-pressed", String(state.filter === view.value));
    if (state.filter === view.value) chip.classList.add("on");
    chip.onclick = () => {
      state.filter = state.filter === view.value ? "" : view.value;
      render();
    };
    host.append(chip);
  }
}

// ── 网格 ───────────────────────────────────────────────────────────────────

function renderGrid() {
  if (state.filter === ARCHIVED) return renderArchivedGrid();
  const all = oursNow();
  const query = state.query.trim().toLowerCase();
  let shown = all;
  if (state.filter === NONE) shown = shown.filter((trigger) => !categoriesOf(trigger).length);
  else if (isStatusFilter(state.filter)) shown = shown.filter((trigger) => statusValueOf(trigger) === state.filter);
  else if (state.filter) shown = shown.filter((trigger) => categoriesOf(trigger).some((tag) => isUnder(tag, state.filter)));

  if (query) {
    shown = shown.filter((trigger) =>
      [trigger.name, trigger.summary, trigger.binary, ...categoriesOf(trigger)].join(" ").toLowerCase().includes(query),
    );
  }

  const grid = $("grid");
  grid.textContent = "";
  for (const trigger of shown) grid.append(productCard(trigger));
  if (!shown.length) {
    const view = STATUS_VIEWS.find((item) => item.value === state.filter);
    const inCat = state.filter && !state.filter.startsWith("__") ? `在「${state.filter}」里` : "";
    const why = !state.data
      ? "读取中…"
      : query
        ? `没有匹配的触发器${inCat}。`
        : view && inCat
          ? `「${state.filter}」里没有「${view.label}」的触发器 —— 再点一下那个状态就取消了。`
          : view
            ? `没有「${view.label}」的触发器。`
            : state.filter === NONE
              ? "没有未分类的触发器。"
              : state.filter
                ? `没有标「${state.filter}」的触发器。`
                : "仓库里一个触发器都没有 —— 点右上「＋ 新建触发器」。";
    grid.append(el("div", "empty", why));
  }

  $("result-count").textContent =
    shown.length === all.length ? `${all.length} 个触发器` : `${shown.length} / ${all.length} 个触发器`;
}

/** 浮层开着的时候，背后那一层不接焦点、也不接点击。 */
function setBehindInert() {
  const layout = document.querySelector(".layout");
  if (layout) layout.inert = Boolean(state.detail) || !$("editor").hidden;
}

function render() {
  const data = state.data;
  if (!data) return;
  closeMenu();

  const repo = data.repo ?? {};
  $("repo-path").textContent = repo.path ?? "—";
  $("repo").textContent = `${repo.branch ?? "?"} · HEAD ${short(repo.head)}`;
  $("repo").title = `触发器仓库：${repo.path ?? "?"}`;

  renderFilters();
  renderFacets();
  renderRailNew();
  renderSuggestions();
  $("grid").hidden = false;
  $("detail").hidden = !state.detail;
  $("dt-scrim").hidden = !state.detail;
  renderGrid();
  if (state.detail) renderDetail();
  $("count").textContent = `${oursNow().length} 个触发器`;
  setBehindInert();
  renderSync();
}

// ── 取数 ───────────────────────────────────────────────────────────────────

async function load({ quiet = false } = {}) {
  const dot = $sel("#refresh .dot");
  if (!quiet) dot.dataset.state = "wait";
  try {
    // 宿主 ctx 只用来拿「隔壁会话引擎在哪」—— 这一页的数据是自己的，不靠它。
    state.ctx = await get(`${SELF}__nous/ctx`).catch(() => null);
    const data = await get(`${API}`);
    state.data = data;
    const repo = data.repo ?? {};
    state.ahead = repo.ahead ?? 0;
    state.behind = repo.behind ?? 0;
    if (!syncTimer) state.dirty = 0;
    if (state.sync !== "pushing") {
      state.sync = "idle";
      state.syncError = "";
    }
    render();
    dot.dataset.state = "ok";
  } catch (error) {
    dot.dataset.state = "bad";
    $("count").textContent = "拿不到";
    $("result-count").textContent = "";
    $("grid").textContent = "";
    $("grid").append(el("div", "empty", String(error?.message ?? error)));
    if (!quiet) toast(String(error?.message ?? error), "bad");
  }
}

// ── 代码抽屉 ───────────────────────────────────────────────────────────────
//
// 保存和发布是两个动作，别合并：
//   · 保存 = PUT file：立刻落盘，不提交 —— 敲到一半可以停。
//   · 发布 = POST publish：cargo check + index.mjs + commit + push。
// 合起来的话，编不过的代码会被推上去，下一个人 pull 到的就是坏东西。

const bytes = (n) => (n < 1024 ? `${n} B` : `${(n / 1024).toFixed(1)} KB`);
const editorDirty = () => Boolean(state.editor) && $("ed-text").value !== state.editor.text;

async function openEditor(trigger, { path: wanted = null } = {}) {
  closeMenu();
  state.editor = {
    name: trigger.name,
    version: trigger.version,
    files: [],
    filesLoading: true,
    path: null,
    kind: null,
    text: "",
    base: null,
    pending: new Set(),
    saving: false,
    publishing: false,
    error: "",
    notice: "",
    mode: "edit",
    baseText: null,
    baseMissing: false,
    diff: null,
    expanded: new Set(),
    lineCount: 1,
    diffTimer: null,
  };
  $("ed-text").value = "";
  $("ed-text").disabled = true;
  $("ed-path").textContent = "—";
  $("ed-scrim").hidden = false;
  $("editor").hidden = false;
  setBehindInert();
  $("ed-name").textContent = trigger.name;
  $("ed-meta").textContent = `v${trigger.version} · Rust 项目`;
  renderEditorFiles();
  renderEditorState();

  try {
    const list = await get(`${API}/files?name=${encodeURIComponent(trigger.name)}`);
    state.editor.files = list.files ?? [];
    state.editor.filesLoading = false;
    renderEditorFiles();
    // 先开指定的那个，否则 nous.json（最常改），再否则第一个 .rs，最后兜底第一个。
    const first =
      (wanted && state.editor.files.find((file) => file.path === wanted)) ??
      state.editor.files.find((file) => file.path === "nous.json") ??
      state.editor.files.find((file) => file.path.endsWith(".rs")) ??
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
  setBehindInert();
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
    nav.append(el("div", "ed-files-empty", "没有 .rs / .toml / .json / .md 文件"));
    return;
  }
  for (const file of ed.files) {
    const btn = el("button", "ed-file");
    btn.type = "button";
    btn.dataset.kind = file.kind;
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
  let text = "已同步";
  if (ed.error) {
    tone = "bad";
    text = "出错";
  } else if (ed.saving) {
    tone = "pending";
    text = "保存中…";
  } else if (ed.publishing) {
    tone = "pending";
    text = "编译 / 发布中…";
  } else if (editorDirty()) {
    tone = "dirty";
    text = "未保存";
  } else if (ed.pending?.size) {
    tone = "pending";
    text = `已保存 · 待发布 ${ed.pending.size}`;
  }
  chip.dataset.tone = tone;
  chip.textContent = text;

  const busy = ed.saving || ed.publishing || !ed.path;
  $("ed-save").disabled = busy;
  $("ed-publish").disabled = busy;
  $("ed-dev").disabled = busy;

  const msg = $("ed-msg");
  if (ed.error) {
    msg.hidden = false;
    msg.dataset.tone = "bad";
    msg.textContent = ed.error;
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
    const file = await get(`${API}/file?name=${encodeURIComponent(ed.name)}&path=${encodeURIComponent(path)}`);
    ed.path = file.path;
    ed.kind = file.kind;
    ed.text = file.text;
    ed.base = file.etag;
    ed.baseText = file.base ?? null;
    ed.baseMissing = file.base == null;
    ed.expanded.clear();
    ed.error = "";
    ed.notice = "";
    const area = $("ed-text");
    area.disabled = false;
    area.value = file.text;
    $("ed-path").textContent = `${ed.name}/${file.path}`;
    ed.lineCount = file.text.split("\n").length;
    recomputeDiff();
    renderEditorFiles();
    renderEditorState();
  } catch (error) {
    ed.error = `读文件失败：${error.message}`;
    renderEditorState();
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
    const saved = await fetch(`${API}/file`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      // base = 「我改的是哪一版」：别人先改了，服务端 409，别把对方的字盖掉。
      body: JSON.stringify({ name: ed.name, path: ed.path, text, base: ed.base }),
    }).then(readJson);
    ed.text = text;
    ed.base = saved.etag;
    ed.pending.add(ed.path);
    if (!quiet) ed.notice = `已保存到磁盘：${ed.name}/${ed.path}（还没提交）`;
    // 顶栏得跟着变：仓库脏了就不是「已同步」，否则页面在撒谎。
    void load({ quiet: true });
    return true;
  } catch (error) {
    ed.error = `保存失败：${error.message}`;
    return false;
  } finally {
    ed.saving = false;
    renderEditorState();
  }
}

async function publishEditor() {
  const ed = state.editor;
  if (!ed || ed.publishing) return;
  if (editorDirty() && !(await saveEditorFile({ quiet: true }))) return;
  ed.publishing = true;
  ed.error = "";
  ed.notice = "跑 cargo check…";
  renderEditorState();
  try {
    const result = await post(`${API}/publish`, { name: ed.name });
    ed.pending.clear();
    if (ed.path) await openEditorFile(ed.path, { force: true }).catch(() => {});
    if (result.pushed === false) {
      ed.error = `本地已提交，但推送失败：\n${result.pushError || "未知原因"}\n点「发布」再试一次。`;
    } else if (result.committed) {
      ed.notice = `已发布 · ${short(result.head)}${result.pushed ? ` · 已推到 ${result.branch ?? "origin"}` : ""}`;
    } else {
      ed.notice = "工作区没有待提交的改动。";
    }
    await load({ quiet: true });
  } catch (error) {
    ed.error = `发布失败：${error.message}`;
  } finally {
    ed.publishing = false;
    renderEditorState();
  }
}

/** 开一个会话，让 agent 去改这个触发器 —— 比在 textarea 里敲 Rust 划算。 */
async function openDevSession() {
  const ed = state.editor;
  if (!ed) return;
  if (editorDirty() && !(await saveEditorFile({ quiet: true }))) return;
  const engine =
    state.ctx?.blocks?.find((block) => block.id === "qrd3wh") ??
    state.ctx?.blocks?.find((block) => block.kind === "group" && block.name === "session");
  if (!engine?.url) {
    ed.error = "这一盘里没有 qrd3wh（会话引擎）—— 起不了会话";
    renderEditorState();
    return;
  }
  const base = engine.url.endsWith("/") ? engine.url : `${engine.url}/`;
  ed.error = "";
  ed.notice = "";
  $("ed-dev").disabled = true;
  try {
    const device = await get(`${base}v1/device`);
    const created = await post(`${base}v1/sessions`, { name: `开发 ${ed.name}`, model: device.model });
    const repo = state.data?.repo?.path ?? "/opt/Nous-Trigger";
    const prompt = [
      `仓库在 ${repo}，我要改触发器「${ed.name}」。`,
      `先读 ${repo}/README.md 和 index.mjs，再读 ${ed.name}/nous.json、${ed.name}/Cargo.toml、${ed.name}/src/main.rs。`,
      `用三句话讲清它现在做什么、你觉得哪里值得改；我确认之后你再动代码。`,
      `改完跑 cargo check，再跑 node index.mjs，然后 git commit + git push（只提交 ${ed.name}/ 和 index.json）。`,
      `别碰别的触发器。`,
    ].join("\n");
    await post(`${base}v1/sessions/${created.id}/messages`, { content: prompt });
    await post(`${base}v1/selection`, { id: created.id });
    window.open(base, "_blank", "noopener");
    ed.notice = `已开一个会话（#${created.id}）—— 新标签页里继续说；它改完 push 回仓库，这页刷新就看到了。`;
  } catch (error) {
    ed.error = `开会话失败：${error.message}`;
  } finally {
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

/** 一行 → 元素数组：修改行拆成「旧行 + 新行」两行，词级碎片各自高亮。 */
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
    if (ed.expanded.has(key)) {
      for (const inner of all.slice(row.from, row.to)) for (const node of diffRowEls(inner)) box.append(node);
    } else box.append(foldRowEl(row));
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
    const target = dir > 0 ? (after ?? nodes[0]) : (before ?? nodes[nodes.length - 1]);
    target.scrollIntoView({ block: "center", behavior: "smooth" });
    return;
  }

  const area = $("ed-text");
  const caret = area.value.slice(0, area.selectionStart).split("\n").length;
  const line =
    dir > 0
      ? (lines.find((n) => n > caret) ?? lines[0])
      : ([...lines].reverse().find((n) => n < caret) ?? lines[lines.length - 1]);
  const textLines = area.value.split("\n");
  const start = textLines.slice(0, line - 1).join("\n").length + (line > 1 ? 1 : 0);
  area.focus();
  area.setSelectionRange(start, start + (textLines[line - 1]?.length ?? 0));
  area.scrollTop = Math.max(0, (line - 1) * ED_LINE() - area.clientHeight / 2);
  syncGutter();
}

// ── 新建触发器 ─────────────────────────────────────────────────────────────
//
// 只问三样：名字、一句话说明、类别。其余全给默认（0.1.0 / Cargo.toml / 一个能编过的 main.rs）。
// 名字是唯一不可逆的决定 —— 它是目录名、清单 name、二进制名三合一。

const NAME_RE = /^[a-z0-9][a-z0-9._-]*$/i;

function validateCreate() {
  const name = $("create-name").value.trim();
  const summary = $("create-summary").value.trim();
  const set = (tone, msg) => ({ ok: tone === "ok", tone, msg });
  const where = state.data?.repo?.path ?? "触发器仓库";

  if (!name) return set("bad", `先起个名字（小写字母开头最好）。会建在 ${where}/&lt;名字&gt;/`);
  if (!NAME_RE.test(name)) return set("bad", "只能字母数字和 . _ -，而且得字母数字开头");
  if ((state.data?.catalog ?? []).some((trigger) => trigger.name === name)) return set("bad", `已经有一个叫 ${name} 的了`);
  if ((state.data?.archived ?? []).some((item) => item.name === name)) {
    return set("bad", `.archive/${name} 还留着 —— 去「已归档」里点恢复，别建第二个`);
  }
  if (!summary) return set("warn", "名字可用。还差一句说明 —— 卡片上就靠它认人");
  return set("ok", `会建在 ${where}/<b>${name}</b>/`);
}

function renderCreate() {
  const verdict = validateCreate();
  const note = $("create-name-note");
  note.className = `create-note ${verdict.tone}`;
  note.innerHTML = verdict.msg;
  $("create-ok").disabled = !verdict.ok || state.building;
  $("create-where").textContent = "Cargo.toml + src/main.rs + nous.json · v0.1.0";
  $("create-name").classList.toggle("bad", verdict.tone === "bad" && $("create-name").value.trim() !== "");
}

function openCreate() {
  closeMenu();
  state.building = false;
  $("create").reset();
  $("create-err").hidden = true;
  const filter = state.filter && !state.filter.startsWith("__") ? state.filter : "";
  $("create-cat").value = filter;
  const cats = state.data?.categories ?? [];
  $("create-cats").replaceChildren(
    ...cats.map((name) => {
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
  state.building = false;
}

async function submitCreate(event) {
  event.preventDefault();
  const verdict = validateCreate();
  if (!verdict.ok) return renderCreate();
  const name = $("create-name").value.trim();
  const summary = $("create-summary").value.trim();
  const category = $("create-cat").value.trim();
  state.building = true;
  renderCreate();
  $("create-err").hidden = true;
  $("create-ok").textContent = "建…";
  try {
    const result = await post(`${API}/create`, { name, summary, category: category ? [category] : [] });
    closeCreate();
    await load({ quiet: true });
    const trigger = findTrigger(name) ?? result.trigger;
    toast(`已建 ${name} · 未发布（只有这台有）`);
    if (state.editor) closeEditor({ force: true });
    if (trigger) await openEditor(trigger);
  } catch (error) {
    $("create-err").textContent = String(error?.message ?? error);
    $("create-err").hidden = false;
  } finally {
    state.building = false;
    $("create-ok").textContent = "建";
    if (!$("create").hidden) renderCreate();
  }
}

// ── 绑定 ───────────────────────────────────────────────────────────────────

$("search").oninput = (event) => {
  state.query = event.target.value;
  state.detail = null;
  state.editing = null;
  $("grid").hidden = false;
  $("detail").hidden = true;
  $("dt-scrim").hidden = true;
  renderGrid();
};
$("refresh").onclick = () => void load();
$("sync").onclick = () => void flushSync();
$("new-trigger").onclick = () => openCreate();
$("create-cancel").onclick = () => closeCreate();
$("create-scrim").onclick = () => closeCreate();
$("create").addEventListener("submit", submitCreate);
for (const id of ["create-name", "create-summary", "create-cat"]) {
  $(id).addEventListener("input", renderCreate);
}
$("create-name").addEventListener("keydown", (event) => {
  if (event.key !== "Enter") return;
  event.preventDefault();
  $("create-summary").focus();
});
$("create-summary").addEventListener("keydown", (event) => {
  if (event.key !== "Enter") return;
  event.preventDefault();
  $("create-cat").focus();
});

$("dt-scrim").onclick = () => closeDetail();
$("dt-close").onclick = () => closeDetail();
$("ed-close").onclick = () => closeEditor();
$("ed-scrim").onclick = () => closeEditor();
$("ed-save").onclick = () => void saveEditorFile();
$("ed-publish").onclick = () => void publishEditor();
$("ed-dev").onclick = () => void openDevSession();
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
    area.value = area.value.slice(0, start) + "    " + area.value.slice(end);
    area.selectionStart = area.selectionEnd = start + 4;
    renderEditorState();
    return;
  }
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
    event.preventDefault();
    void saveEditorFile();
  }
});

addEventListener("keydown", (event) => {
  if (event.key === "Escape" && menuOpen) {
    closeMenu();
    return;
  }
  if (event.key === "Escape" && !$("create").hidden) {
    closeCreate();
    return;
  }
  if (!state.editor) {
    if (event.key === "Escape" && state.detail) closeDetail();
    return;
  }
  if (event.altKey && event.key.toLowerCase() === "d") {
    event.preventDefault();
    setEditorMode(state.editor.mode === "diff" ? "edit" : "diff");
  } else if (event.altKey && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
    event.preventDefault();
    gotoChange(event.key === "ArrowDown" ? 1 : -1);
  } else if (event.key === "Escape") {
    if (state.editor.mode === "diff") setEditorMode("edit");
    else closeEditor();
  }
});

addEventListener("beforeunload", (event) => {
  if (!editorDirty()) return;
  event.preventDefault();
  event.returnValue = "";
});

load();

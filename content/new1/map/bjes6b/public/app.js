// 微信的外壳：列表、会话、气泡、输入条。
//
// 全部状态都活在服务端（qrd3wh 的 sqlite）。这里只有三样东西是本地的：
//   - 这一屏正在渲染的 DOM
//   - 一条正在吐的助手消息（blocks）
//   - 上一帧之后攒下的改动（dirty，按 rAF 合帧）
//
// 所以刷新、切走、被杀进程，都不丢东西。SSE 重连第一帧就是 snapshot。

const API = new URL("api/", document.baseURI);
const $ = (sel, root = document) => root.querySelector(sel);

const listEl = $("#sessions");
const messagesEl = $("#messages");
const inputEl = $("#input");
const toastEl = $("#toast");
const templatesEl = $("#templates");
const indexEl = $("#tpl-index");
const queryEl = $("#tpl-q");

/* ── 小工具 ─────────────────────────────────────────── */

let toastTimer;
const toast = (text) => {
  toastEl.textContent = text;
  toastEl.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (toastEl.hidden = true), 1400);
};

const api = async (path, init) => {
  const res = await fetch(new URL(path, API), init);
  const text = await res.text();
  if (!res.ok) throw new Error(`${res.status} ${text.slice(0, 200)}`);
  return text ? JSON.parse(text) : null;
};

const el = (tag, className, text) => {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
};

const esc = (s) => s.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);

/* markdown 只做到「能读」：标题、列表、行内码、粗体、围栏。不做链接、不做表格、不做嵌套。 */
const inline = (t) =>
  t
    .replace(/`([^`]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*\n]+)\*\*/g, "<strong>$1</strong>")
    .replace(/\[([^\]\n]+)\]\([^)\s]*\)/g, "$1");

const md = (src) => {
  const lines = esc(src).split("\n");
  const out = [];
  let para = [];
  let list = null;

  const flushPara = () => {
    if (para.length) out.push(`<p>${inline(para.join("<br>"))}</p>`);
    para = [];
  };
  const flushList = () => {
    if (list) out.push(`<${list.tag}>${list.items.map((i) => `<li>${inline(i)}</li>`).join("")}</${list.tag}>`);
    list = null;
  };
  const flush = () => {
    flushPara();
    flushList();
  };

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];

    if (/^\s*```/.test(line)) {
      flush();
      const body = [];
      for (i += 1; i < lines.length && !/^\s*```/.test(lines[i]); i += 1) body.push(lines[i]);
      out.push(`<pre><code>${body.join("\n")}</code></pre>`);
      continue;
    }

    if (!line.trim()) {
      flush();
      continue;
    }

    const heading = line.match(/^(#{1,3})\s+(.*)$/);
    if (heading) {
      flush();
      const level = Math.min(heading[1].length + 1, 4);
      out.push(`<h${level}>${inline(heading[2])}</h${level}>`);
      continue;
    }

    const bullet = line.match(/^\s*[-*]\s+(.*)$/);
    const ordered = line.match(/^\s*\d+[.)]\s+(.*)$/);
    if (bullet || ordered) {
      flushPara();
      const tag = bullet ? "ul" : "ol";
      if (!list || list.tag !== tag) {
        flushList();
        list = { tag, items: [] };
      }
      list.items.push((bullet ?? ordered)[1]);
      continue;
    }

    flushList();
    para.push(line);
  }
  flush();
  return out.join("");
};

/** 一条 entry 的正文。pi.system 没正文，工具结果正文很长 —— 都交给调用方筛。 */
const textOf = (entry) => {
  const parts = [];
  for (const message of entry?.model ?? []) {
    const content = message?.content;
    if (typeof content === "string") parts.push(content);
    else if (Array.isArray(content)) {
      for (const block of content) if (block?.type === "text" && block.text) parts.push(block.text);
    }
  }
  return parts.join("\n");
};

/* ── 列表 ───────────────────────────────────────────── */

const preview = (text) =>
  (text ?? "")
    .replace(/\s+/g, " ")
    .replace(/[#*`>|]/g, "")
    .trim();

const loadSessions = async () => {
  let data;
  try {
    data = await api("sessions");
  } catch (error) {
    listEl.replaceChildren(el("div", "empty", `连不上引擎：${error.message}`));
    return;
  }

  if (!data.items.length) {
    listEl.replaceChildren(el("div", "empty", "还没有会话"));
    return;
  }

  // 乐观重排要用同一份置顶序 —— 服务端给的顺序就是真相。
  rowOrder = data.items.filter((s) => s.pinned).map((s) => s.id);

  const frag = document.createDocumentFragment();
  for (const session of data.items) {
    const row = el("button", "row");
    row.type = "button";
    row.dataset.id = session.id;
    row.dataset.last = session.last?.id ?? 0;
    row.dataset.pinned = session.pinned ? "1" : "0";
    row.append(el("span", "avatar"));

    const main = el("div", "row-main");
    const name = el("div", "row-name");
    if (session.pinned) name.append(el("span", "pin", "📌"));
    name.append(el("span", null, session.name || `会话 ${session.id}`));
    if (session.busy) name.append(el("span", "dot"));
    main.append(name);
    main.append(el("div", "row-last", preview(session.last?.text) || "还没有消息"));
    row.append(main);

    row.addEventListener("click", () => {
      location.hash = `#/c/${session.id}`;
    });
    hold(row, session.id);
    frag.append(row);
  }
  listEl.replaceChildren(frag);
};

/* ── 长按 / 右键 → 底部动作面板 ─────────────────────────── */

const HOLD_MS = 500;      // 票上的手感：按住 0.5 秒才出面板
const MOVE_SLOP = 6;      // 手指挪超过 6px = 想滚动，不算长按

let holdFired = false;
/** 当前列表的置顶序（id 数组）。loadSessions 时从服务端拿，乐观更新时自己改。 */
let rowOrder = [];
/** 菜单开着时保持变暗的那一行。菜单一关就恢复。 */
let heldRow = null;

/**
 * 微信那套节奏（H 方案，票上的手感参数）：
 *
 *   按下 → 0.5s 内不动，列表还能照常滚动（长按没"生效"）；
 *   到阈值 → 震一下 + 遮罩变暗 + 底部面板从下面滑出（180ms，CSS 的 transition）；
 *   滑动超过 6px → 取消（不出面板）。
 *
 * 电脑右键：当场开同一个面板（同一份菜单内容，只有一份实现）。
 */
const hold = (row, id) => {
  let startX = 0;
  let startY = 0;
  let active = false; // 这次按压还在长按流程里
  let timer = 0;

  const disarm = () => {
    active = false;
    clearTimeout(timer);
  };

  row.addEventListener("pointerdown", (event) => {
    // 右键不在这里处理：走 document 上的 `contextmenu`（跟宿主 NodeMenu 同一套）。
    // 放在 pointerdown 里弹，浏览器原生菜单照样出来——多浏览器实测都这样。
    if (event.pointerType === "mouse") return; // 左键归 click

    startX = event.clientX;
    startY = event.clientY;
    active = true;
    holdFired = false;
    clearTimeout(timer);
    timer = setTimeout(() => {
      if (!active) return;
      active = false;
      holdFired = true;
      navigator.vibrate?.(8);
      openRowMenu(row, id);
    }, HOLD_MS);
  });

  row.addEventListener("pointermove", (event) => {
    if (!active || event.pointerType === "mouse") return;
    const dx = event.clientX - startX;
    const dy = event.clientY - startY;
    if (dx * dx + dy * dy > MOVE_SLOP * MOVE_SLOP) disarm();
  });

  row.addEventListener("pointerup", () => {
    if (!active) return;
    disarm();
  });

  row.addEventListener("pointercancel", disarm);
  row.addEventListener("touchcancel", disarm);

  // 长按过的那次抬手不许当 click（会进会话）。捕获阶段拦，真正的 click 监听根本看不到。
  row.addEventListener("click", (event) => {
    if (holdFired) {
      event.preventDefault();
      event.stopImmediatePropagation();
      holdFired = false;
    }
  }, true);
};

const setHeldRow = (row) => {
  if (heldRow === row) return;
  heldRow?.classList.remove("holding");
  heldRow = row;
  heldRow?.classList.add("holding");
};

/** 面板里那一项。pointer 驱动的语义跟老浮层菜单的 menuItem 一模一样：
 *  按住高亮、滑出项外取消、松手才执行 —— 别改成 click。 */
const sheetItem = (label, { danger = false, cap = false, run } = {}) => {
  const item = el("button", `sheet-item${danger ? " danger" : ""}`, label);
  item.type = "button";
  if (cap) {
    item.classList.add("sheet-cap");
    item.disabled = true; // 灰字，只是告诉你按的是哪条，不可点
    return item;
  }
  let active = false;
  let touchHandled = false;

  const inside = (event) => {
    const r = item.getBoundingClientRect();
    return (
      event.clientX >= r.left &&
      event.clientX <= r.right &&
      event.clientY >= r.top &&
      event.clientY <= r.bottom
    );
  };

  item.addEventListener("pointerdown", (event) => {
    if (event.pointerType === "mouse") return; // 鼠标交给 click
    active = true;
    item.classList.add("pressed");
    event.preventDefault();
  });

  item.addEventListener("pointermove", (event) => {
    if (!active) return;
    item.classList.toggle("pressed", inside(event));
  });

  item.addEventListener("pointerup", (event) => {
    if (!active) return;
    active = false;
    item.classList.remove("pressed");
    const hit = inside(event);
    if (hit) run();
    // 合成 click 还在路上：sheetItem 必须留在 DOM 里当它的目标，否则 click 会
    // 穿透到底下的会话行，直接跳进会话。下一帧再关面板。
    touchHandled = true;
    requestAnimationFrame(closeRowMenu);
    event.preventDefault();
  });

  item.addEventListener("pointercancel", () => {
    if (!active) return;
    active = false;
    item.classList.remove("pressed");
    // 滑出面板后被系统手势接管（浏览器发 cancel 而不是 up）：跟滑出松手一样，只关不执行。
    closeRowMenu();
  });

  item.addEventListener("click", (event) => {
    event.preventDefault();
    event.stopPropagation();
    if (touchHandled) {
      touchHandled = false;
      return;
    }
    closeRowMenu();
    run();
  });
  return item;
};

/**
 * 菜单内容只有一份：长按开它，右键也开它。要加动作只改这里。
 * 「彻底删除」永远不在这里 —— 它只住在归档库那一页。
 */
const openRowMenu = (row, id) => {
  closeRowMenu();
  setHeldRow(row);
  // pinned 读**当前** dataset —— 乐观更新后 row 的状态已经变了，闭包里的旧值会撒谎。
  const pinned = row.dataset.pinned === "1";
  const name = row.querySelector(".row-name span:last-child")?.textContent?.trim() || `会话 ${id}`;

  const group = el("div", "sheet-group");
  group.append(
    sheetItem(name, { cap: true }),
    sheetItem(pinned ? "取消置顶" : "置顶该聊天", { run: () => togglePin(row, id) }),
    sheetItem("归档该聊天…", { run: () => archiveSession(row, id) }),
    sheetItem("撕原件…", { danger: true, run: () => confirmPurge({ id: Number(id), name }, () => purgeSession(row, id)) }),
  );
  const cancelGroup = el("div", "sheet-group");
  cancelGroup.append(sheetItem("取消", { run: () => {} }));

  const sheet = el("div", "sheet");
  sheet.append(group, cancelGroup);
  const mask = el("div", "sheet-mask");
  mask.addEventListener("pointerdown", closeRowMenu);
  mask.addEventListener("click", closeRowMenu);

  document.body.append(mask, sheet);
  // 下一帧再拉 on：transition 要先有一个 translateY(110%) 的起点才有「滑出」。
  requestAnimationFrame(() => {
    mask.classList.add("on");
    sheet.classList.add("on");
  });
  // 捕获阶段监听：手指碰到面板以外的任何地方（包括别的行）就关。
  document.addEventListener("pointerdown", onMenuOutside, true);
};

/** Esc（有键盘时）关面板 —— 桌面右键开的那份也归它管。 */
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && document.querySelector(".sheet")) closeRowMenu();
});

/**
 * 右键菜单照抄宿主 NodeMenu 的做法：在 `contextmenu` 里 `preventDefault` + 弹面板。
 * 浏览器原生菜单的抑制只认 contextmenu 这一个事件 —— 放在 pointerdown 里弹、
 * 再另挂一个 contextmenu 去拦，是两套逻辑，原生菜单照样出来（多浏览器实测都这样）。
 * 别处右键（空白、输入框）还给浏览器。归档库的行也归它管。
 */
document.addEventListener("contextmenu", (event) => {
  const row = event.target.closest("#sessions .row, #archive-list .arch-row");
  if (!row) return;
  event.preventDefault();
  openRowMenu(row, row.dataset.id);
});

/** 乐观更新只动了 rowOrder，列表 DOM 的顺序得按同一套规则重排。 */
const resortRows = () => {
  const rows = [...listEl.querySelectorAll(".row")];
  rows.sort((a, b) => {
    const ia = Number(a.dataset.id);
    const ib = Number(b.dataset.id);
    const pa = rowOrder.indexOf(ia);
    const pb = rowOrder.indexOf(ib);
    if (pa !== -1 || pb !== -1) {
      if (pa !== -1 && pb !== -1) return pa - pb;
      return pa !== -1 ? -1 : 1;
    }
    return Number(b.dataset.last ?? 0) - Number(a.dataset.last ?? 0);
  });
  for (const row of rows) listEl.append(row);
};

const togglePin = async (row, id) => {
  const wasPinned = row.dataset.pinned === "1";
  const want = !wasPinned;

  // 乐观：先动自己的序和 DOM，服务端失败再全数回来。
  if (want) rowOrder.unshift(id);
  else {
    const at = rowOrder.indexOf(id);
    if (at !== -1) rowOrder.splice(at, 1);
  }
  row.dataset.pinned = want ? "1" : "0";
  const label = row.querySelector(".pin");
  if (want && !label) row.querySelector(".row-name").prepend(el("span", "pin", "📌"));
  else if (!want && label) label.remove();
  resortRows();

  try {
    const out = await api(`sessions/${id}/pin`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ pinned: want }),
    });
    if (!out.changed) loadSessions(); // 服务端说没变化 —— 状态分叉了，重拉兜底
  } catch (error) {
    if (want) {
      const at = rowOrder.indexOf(id);
      if (at !== -1) rowOrder.splice(at, 1);
    } else rowOrder.unshift(id);
    row.dataset.pinned = wasPinned ? "1" : "0";
    resortRows();
    toast(`没改成：${error.message.slice(0, 60)}`);
  }
};

/**
 * 菜单项完全由 pointer 事件驱动，不靠 click：
 * 手指按住 → 高亮；滑出去 → 高亮消失；松手时还在项内才执行，否则只关菜单。
 * 这就是微信「按住菜单项滑到外面再松手 = 取消」。
 */
const closeRowMenu = () => {
  const sheet = document.querySelector(".sheet");
  if (sheet) {
    // 180ms 滑回去再拆 DOM —— 直接 remove 就没有「收回」动画了。
    sheet.classList.remove("on");
    document.querySelector(".sheet-mask")?.classList.remove("on");
    setTimeout(() => {
      sheet.remove();
      document.querySelector(".sheet-mask")?.remove();
    }, 200);
  }
  setHeldRow(null);
  document.removeEventListener("pointerdown", onMenuOutside, true);
};

const onMenuOutside = (event) => {
  if (!event.target.closest(".sheet") && !event.target.closest(".confirm")) closeRowMenu();
};

// 菜单开着时，列表一滚就关（滚之前的那一下 pointerdown 其实已经关了，这是兜底）。
listEl.addEventListener("scroll", closeRowMenu, { passive: true });

/* ── 归档 / 撕原件：动作只有一层壳，硬条件全在引擎那侧 ───────────── */

/** 归档：不问（可逆，随时可捞回）。行从列表消失靠重拉 —— 引擎默认藏归档。 */
const archiveSession = async (row, id) => {
  try {
    await api(`sessions/${id}/archive`, { method: "POST" });
    if (String(thread.id) === String(id)) {
      // 归档的正是当前打开的会话 → 退回列表页。
      location.hash = "#/";
    }
    toast("已归档 · 可在归档库捞回");
    loadSessions();
  } catch (error) {
    // 引擎那句原话（忙 / 有活分支），一个字不改。
    toast(String(error.message).slice(0, 90));
  }
};

/** 撕原件：先红面板再确认一层。失败把引擎原文 toast 出来，不改写。 */
const purgeSession = async (id) => {
  try {
    await api(`sessions/${id}/purge`, { method: "POST" });
    toast("已撕掉原件 · 归档库那份复印件也一起销毁了");
    loadSessions();
    loadArchive();
  } catch (error) {
    toast(String(error.message).slice(0, 90));
  }
};

/**
 * 撕原件的二次确认（票上的文案，一个字不改）。
 * onConfirm 由调用方给：主列表里是「撕完回列表重拉」，归档库里是「撕完刷新归档库」。
 */
const confirmPurge = ({ id, name }, onConfirm) => {
  const wrap = el("div", "confirm");
  const box = el("div", "confirm-box");
  box.append(
    el("h3", null, `撕原件：${name}`),
    el("p", null, "真删，归档库里那份复印件也一起销毁，不能撤销。"),
  );
  const foot = el("div", "confirm-foot");
  const cancel = el("button", null, "取消");
  cancel.type = "button";
  const ok = el("button", "danger", "撕掉原件");
  ok.type = "button";
  const close = () => wrap.remove();
  cancel.addEventListener("click", close);
  ok.addEventListener("click", () => {
    close();
    onConfirm?.();
  });
  foot.append(cancel, ok);
  box.append(foot);
  wrap.append(box);
  // 点遮罩不算确认 —— 这种面板上，只有那颗红按钮能放行。
  wrap.addEventListener("click", (event) => {
    if (event.target === wrap) close();
  });
  wrap.addEventListener("keydown", (event) => {
    if (event.key === "Escape") close();
  });
  document.body.append(wrap);
  ok.focus();
  return wrap;
};

/* ── 会话 ───────────────────────────────────────────── */

const thread = {
  id: null,
  es: null,
  /** 正在吐的那条：{ node, body, blocks: Map<contentIndex, string> } */
  live: null,
  /** 已经画上去的 entry id，用来防重（重连、乐观上屏都会撞）。 */
  seen: new Set(),
  /** 自己乐观上屏、但还没拿到 entry id 的那几句。引擎回放回来时要认得出是自己。 */
  pending: [],
};

const bubbleNode = (kind) => {
  const row = el("div", `msg ${kind === "me" ? "me" : "you"}`);
  row.append(el("span", "avatar"));
  const bubble = el("div", "bubble");
  row.append(bubble);
  return { row, bubble };
};

const nearBottom = () =>
  messagesEl.scrollHeight - messagesEl.scrollTop - messagesEl.clientHeight < 140;

const scrollDown = () => {
  messagesEl.scrollTop = messagesEl.scrollHeight;
};

const append = (node) => {
  const stuck = nearBottom();
  messagesEl.append(node);
  if (stuck) scrollDown();
};

/** 连着来的工具结果合成一张卡。一次 turn 里常常挂着十几条，平铺出来就是刷屏。 */
const toolGroup = () => messagesEl.lastElementChild?.classList.contains("tool")
  ? messagesEl.lastElementChild
  : null;

const appendTool = (text) => {
  const body = text.length > 4000 ? `${text.slice(0, 4000)}\n…` : text;
  const open = toolGroup();

  if (open) {
    open.dataset.count = String(Number(open.dataset.count ?? 1) + 1);
    open.querySelector("summary").textContent = `工具结果 × ${open.dataset.count}`;
    open.querySelector("pre").textContent += `\n\n${body}`;
    if (nearBottom()) scrollDown();
    return open;
  }

  const card = el("details", "tool");
  card.dataset.count = "1";
  card.append(el("summary", null, "工具结果"));
  card.append(el("pre", null, body));
  append(card);
  return card;
};

const appendEntry = (entry) => {
  if (entry.id != null) {
    if (thread.seen.has(entry.id)) return;
    thread.seen.add(entry.id);
  }

  const text = entry.text ?? textOf(entry);

  if (entry.kind === "pi.user") {
    const { row, bubble } = bubbleNode("me");
    bubble.textContent = text;
    append(row);
    return;
  }

  if (entry.kind === "pi.assistant") {
    if (!text.trim()) return;
    const { row, bubble } = bubbleNode("you");
    bubble.innerHTML = md(text);
    append(row);
    return;
  }

  if (entry.kind === "pi.tool-result") {
    if (!text.trim()) return;
    appendTool(text);
  }
};

const loadHistory = async () => {
  const data = await api(`sessions/${thread.id}/entries?limit=200`);
  messagesEl.replaceChildren();
  thread.seen.clear();
  thread.live = null;
  for (const entry of data.items) appendEntry(entry);
  scrollDown();
};

/** 一条正在吐的助手消息。blocks 按 contentIndex 拼回去 —— 模型可能先说话再调工具再说话。 */
const liveText = () => {
  if (!thread.live) return "";
  return [...thread.live.blocks.entries()]
    .sort((a, b) => a[0] - b[0])
    .map((entry) => entry[1])
    .join("");
};

let dirty = false;
const paint = () => {
  if (dirty) return;
  dirty = true;
  requestAnimationFrame(() => {
    dirty = false;
    if (!thread.live) return;
    thread.live.body.innerHTML = md(liveText());
    if (nearBottom()) scrollDown();
  });
};

const startLive = (existing) => {
  if (!thread.live) {
    const { row, bubble } = bubbleNode("you");
    messagesEl.append(row);
    thread.live = { node: row, body: bubble, blocks: new Map() };
  }
  if (typeof existing === "string") thread.live.blocks.set(0, existing);
  paint();
};

const endLive = (entry) => {
  if (!thread.live) return;
  const text = textOf(entry) || liveText();
  const { body } = thread.live;
  body.innerHTML = md(text);
  thread.live = null;
  if (entry?.id != null) thread.seen.add(entry.id);
};

const handle = (event) => {
  switch (event.type) {
    case "snapshot": {
      const partial = event.generation?.message;
      if (partial) startLive(textOf({ model: partial.content ? [{ content: partial.content }] : [] }));
      break;
    }
    case "message_start": {
      // 引擎会给每一条消息都发 message_start：user、system、assistant。
      // 只有 assistant 该开口一个气泡，其余两条是历史，早就在屏幕上了。
      if (event.message?.role === "assistant") startLive();
      break;
    }
    case "message_update": {
      for (const change of event.changes ?? []) {
        if (change.type === "text_delta") {
          const index = change.contentIndex ?? 0;
          thread.live?.blocks.set(index, (thread.live.blocks.get(index) ?? "") + change.delta);
        } else if (change.type === "block" && change.block?.type === "text") {
          thread.live?.blocks.set(change.contentIndex ?? 0, change.block.text ?? "");
        } else if (change.type === "message") {
          // 结束了才给整条：以它为准。
          if (thread.live && change.message) thread.live.blocks.set(0, textOf({ model: [change.message] }));
        }
      }
      paint();
      break;
    }
    case "message_end": {
      if (event.entry?.kind === "pi.assistant") endLive(event.entry);
      break;
    }
    case "entry_appended": {
      // 助手消息走 message_end，这里只补别人（另一个标签页、另一个客户端）的东西。
      const entry = event.entry;
      if (entry?.kind !== "pi.user" && entry?.kind !== "pi.tool-result") break;
      const text = textOf(entry);
      const mine = thread.pending.indexOf(text);
      if (mine >= 0) {
        thread.pending.splice(mine, 1);
        break;
      }
      appendEntry({ id: entry.id, kind: entry.kind, text });
      break;
    }
    case "submission": {
      const record = event.record;
      if (record?.status === "unanswered") {
        const why = String(record.detail ?? record.reason ?? "没答上来");
        toast(`没答上来：${why.slice(0, 90)}`);
      }
      break;
    }
    case "task_failed": {
      toast(`任务挂了：${String(event.message ?? "").slice(0, 80)}`);
      break;
    }
    case "tool_execution_start": {
      if (thread.live) endLive(null); // 先把手上的字收进气泡
      const card = el("details", "tool");
      card.dataset.call = event.toolCallId;
      card.append(el("summary", null, event.toolName ?? "工具"));
      card.append(el("pre"));
      append(card);
      break;
    }
    case "tool_execution_update": {
      const card = messagesEl.querySelector(`.tool[data-call="${CSS.escape(event.toolCallId)}"] pre`);
      if (!card) break;
      const out = event.output;
      if (out && "append" in out) card.textContent += out.append ?? "";
      else if (out && "set" in out) card.textContent = out.set;
      if (card.textContent.length > 4000) card.textContent = `…${card.textContent.slice(-4000)}`;
      break;
    }
    default:
      break;
  }
};

const connect = () => {
  thread.es?.close();
  thread.es = new EventSource(new URL(`sessions/${thread.id}/events`, API));
  let reconnected = false;

  thread.es.onopen = () => {
    if (!reconnected) return;
    // 断开期间发生的事我一条都没看见。重连第一帧只有 snapshot，但历史在 entries 里，
    // 那就直接把历史重新拉一遍 —— 比解析 snapshot 里那坨 model 数组便宜。
    loadHistory().catch(() => {});
  };

  thread.es.onerror = () => {
    reconnected = true;
  };

  thread.es.onmessage = (message) => {
    try {
      handle(JSON.parse(message.data));
    } catch {
      // 一帧看不懂不该把整条流扔掉。
    }
  };
};

const openThread = async (id) => {
  thread.id = id;
  show("thread");
  $("#thread-title").textContent = "会话";
  messagesEl.replaceChildren(el("div", "empty", "加载中…"));

  try {
    const data = await api("sessions");
    const session = data.items.find((item) => String(item.id) === String(id));
    if (session) $("#thread-title").textContent = session.name || `会话 ${id}`;
  } catch {
    // 标题拿不到不影响看消息。
  }

  await loadHistory().catch((error) => {
    messagesEl.replaceChildren(el("div", "empty", `拉不到历史：${error.message}`));
  });
  connect();
  scrollDown();
};

const closeThread = () => {
  thread.es?.close();
  thread.es = null;
  thread.id = null;
  thread.live = null;
  thread.seen.clear();
  messagesEl.replaceChildren();
};

const route = () => {
  const match = location.hash.match(/^#\/c\/(\d+)/);
  if (match) {
    if (String(thread.id) !== match[1]) openThread(match[1]);
    return;
  }
  // 离开会话：先关流。画哪一屏交给下面 —— 回列表还是去模板/归档库，hash 说了算。
  if (thread.id != null) closeThread();

  if (/^#\/templates\/?$/.test(location.hash)) {
    show("templates");
    loadTemplates();
  } else if (/^#\/archive\/?$/.test(location.hash)) {
    show("archive");
    loadArchive();
  } else {
    show("list");
    loadSessions();
  }
};

/* ── 输入 ───────────────────────────────────────────── */

const resizeInput = () => {
  inputEl.style.height = "auto";
  inputEl.style.height = `${Math.min(inputEl.scrollHeight, window.innerHeight * 0.4)}px`;
};

const submit = async () => {
  const content = inputEl.value.trim();
  if (!content || thread.id == null) return;

  inputEl.value = "";
  resizeInput();

  const { row, bubble } = bubbleNode("me");
  bubble.textContent = content;
  append(row);
  thread.pending.push(content);

  try {
    await api(`sessions/${thread.id}/messages`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      // requestId 幂等：网络抖一下重发同一句，不会答两遍。
      body: JSON.stringify({ content, requestId: crypto.randomUUID() }),
    });
  } catch (error) {
    toast(`没发出去：${error.message.slice(0, 60)}`);
  }
};

/* ── 事件 ───────────────────────────────────────────── */

$("#composer").addEventListener("submit", (event) => {
  event.preventDefault();
  submit();
});

inputEl.addEventListener("input", resizeInput);

inputEl.addEventListener("keydown", (event) => {
  // 桌面回车发送，手机回车换行；中文输入法选词时的回车不能当发送。
  if (event.key === "Enter" && !event.shiftKey && !event.isComposing && matchMedia("(pointer: fine)").matches) {
    event.preventDefault();
    submit();
  }
});

$("#back").addEventListener("click", () => {
  if (history.length > 1) history.back();
  else location.hash = "#/";
});

document.addEventListener("click", (event) => {
  const todo = event.target.closest("[data-todo]");
  if (todo) toast(`${todo.dataset.todo}：还没做`);
});

window.addEventListener("hashchange", () => {
  closeRowMenu();
  route();
});

// 键盘顶起：iOS 上 100dvh 不跟着软键盘收缩，只有 visualViewport 说实话。
const vv = window.visualViewport;
const fit = () => {
  const height = vv?.height ?? window.innerHeight;
  document.documentElement.style.setProperty("--app-h", `${height}px`);
  if (thread.id != null && nearBottom()) scrollDown();
};
if (vv) {
  vv.addEventListener("resize", fit);
  vv.addEventListener("scroll", fit);
}
window.addEventListener("resize", fit);
fit();

/* ── 页面切换 ─────────────────────────────────── */

const PAGES = { list: "#page-list", templates: "#page-templates", thread: "#page-thread", archive: "#page-archive" };

/**
 * 一屏只有一页在。会话页藏 tabbar 靠 `body[data-view="thread"]` 那一条 CSS。
 *
 * 为什么不是「盖一层」而是「换页」：这三页互斥，而且后退键得能用（hash 是唯一的状态源）。
 */
const show = (view) => {
  document.body.dataset.view = view;
  for (const [name, sel] of Object.entries(PAGES)) {
    const node = $(sel);
    if (node) node.hidden = name !== view;
  }
  const tab = view === "templates" ? "templates" : "nous"; // 会话页也算第一个 tab
  for (const node of document.querySelectorAll(".tab")) {
    const on = node.dataset.tab === tab;
    node.classList.toggle("on", on);
    if (on) node.setAttribute("aria-current", "page");
    else node.removeAttribute("aria-current");
  }
  // 离开模板页就收掉搜索态：微信的搜索是另一个页，回来不该还停在搜索框上。
  if (view !== "templates") closeTemplateSearch();
};

/* ── Template：会话模板，形式是微信通讯录 ────────────
   数据不在这里长：引擎给什么就是什么。这一块只负责把它画成一行一行。 */

const tpl = {
  items: [],
  repo: null,
  archived: 0,
  q: "",
};

/**
 * 能不能开、为什么不能开。
 *
 * **闸门在引擎**：`POST /v1/sessions {preset}` 会自己挡下写坏了的和引用没了的，并把那句
 * 原话递回来。这里只是把红点提前画在脸上，不必按下去才知道。而且读的字段全是引擎算好的
 * （`specError` / `resolved.*.state` / `tracked`），这一块一条规则都没自己定。
 */
const tplState = (item) => {
  if (item.specError) return { state: "failed", why: String(item.specError) };
  const r = item.resolved;
  if (!r) return { state: "failed", why: "读不到引用" };

  const gone = [];
  if (r.model?.state === "invalid") gone.push(r.model.ref ?? "模型");
  if (r.thinking?.state === "invalid") gone.push(r.thinking.ref ?? "档位");
  if (r.template?.state === "invalid") gone.push(r.template.ref ?? "模板");
  for (const s of r.skills ?? []) if (s.state === "invalid") gone.push(s.ref ?? "技能");
  for (const e of r.extensions ?? []) if (e.state === "invalid") gone.push(e.ref ?? "扩展");
  if (gone.length) return { state: "invalid", why: gone.join("、") };

  if (item.tracked === false) return { state: "unpublished", why: "盘上有、仓里没" };
  return { state: "ok", why: "" };
};

const BADGE = { invalid: "引用失效", failed: "写坏了", unpublished: "未发布" };

/** 分组靠首字母（`#` 兑非英文）。将来真要按类别分组，改的是这一个函数加一个表。 */
const letterOf = (name) => {
  const c = String(name ?? "").trim().charAt(0).toUpperCase();
  return c >= "A" && c <= "Z" ? c : "#";
};

const hueOf = (text) => {
  let h = 0;
  for (let i = 0; i < text.length; i += 1) h = (h * 31 + text.charCodeAt(i)) % 360;
  return h;
};

const stamp = (now = new Date()) => {
  const pad = (n) => String(n).padStart(2, "0");
  return `${pad(now.getMonth() + 1)}-${pad(now.getDate())} ${pad(now.getHours())}:${pad(now.getMinutes())}`;
};

const loadTemplates = async () => {
  // 已经有东西在手就不闪「加载中」：在列表页之间来回跳不该看见白屏。
  if (!tpl.items.length) templatesEl.replaceChildren(el("div", "empty", "加载中…"));

  let data;
  try {
    data = await api("presets");
  } catch (error) {
    templatesEl.replaceChildren(el("div", "empty", `连不上模板库：${error.message}`));
    indexEl.replaceChildren();
    return;
  }

  tpl.items = Array.isArray(data.presets) ? data.presets : [];
  tpl.repo = data.repo ?? null;
  tpl.archived = Array.isArray(data.archived) ? data.archived.length : 0;
  renderTemplates();
};

const renderTemplates = () => {
  const q = tpl.q.trim().toLowerCase();
  const matches = (item) =>
    !q ||
    [item.name, item.summary, item.description, ...(item.category ?? [])]
      .filter(Boolean)
      .some((text) => String(text).toLowerCase().includes(q));

  const items = tpl.items.filter(matches).sort((a, b) => String(a.name).localeCompare(String(b.name)));

  if (!items.length) {
    templatesEl.replaceChildren(
      el("div", "empty", tpl.items.length ? `没有「${tpl.q.trim()}」` : "模板库里还没有东西"),
    );
    indexEl.replaceChildren();
    return;
  }

  const frag = document.createDocumentFragment();
  const letters = [];
  let current = null;
  for (const item of items) {
    const letter = letterOf(item.name);
    if (letter !== current) {
      current = letter;
      letters.push(letter);
      frag.append(el("div", "group", letter));
      frag.lastElementChild.dataset.letter = letter;
    }
    frag.append(templateRow(item));
  }

  const foot = el("div", "tpl-foot");
  if (tpl.repo?.error) {
    foot.classList.add("bad");
    foot.textContent = `模板库读不到：${tpl.repo.error}`; // 引擎那句原话，不重写
  } else {
    foot.textContent =
      `${items.length} / ${tpl.items.length} 条模板` + (tpl.archived ? ` · 已归档 ${tpl.archived}` : "");
  }
  frag.append(foot);

  templatesEl.replaceChildren(frag);
  // 标尺上只放真有这些字母的。点了没反应比少几颗更糟。
  indexEl.replaceChildren(
    ...letters.map((letter) => {
      const node = el("span", null, letter);
      node.dataset.letter = letter;
      return node;
    }),
  );
  delete indexEl.dataset.active;
};

const templateRow = (item) => {
  const { state, why } = tplState(item);
  const canOpen = state === "ok" || state === "unpublished";

  const row = el("button", `row tpl${canOpen ? "" : " off"}`);
  row.type = "button";
  row.dataset.name = item.name;
  if (!canOpen) row.title = `开不了：${why}`;

  const brick = el("span", "brick", letterOf(item.name));
  brick.style.background = `hsl(${hueOf(String(item.name))} 52% 55%)`;
  row.append(brick);

  const main = el("div", "row-main");
  const name = el("div", "row-name");
  name.append(el("span", null, item.name));
  if (state !== "ok") {
    name.append(el("span", `warn${state === "unpublished" ? " pre" : ""}`, BADGE[state]));
  }
  main.append(name);
  main.append(el("div", "row-last", preview(item.summary) || "没有摘要"));
  if (!canOpen) main.append(el("div", "row-why", why));
  row.append(main);

  row.addEventListener("click", () => launchTemplate(item, row));
  return row;
};

/** 防双击：一个名字只允许在飞一次。 */
const launching = new Set();

const launchTemplate = async (item, row) => {
  const { state, why } = tplState(item);
  if (state === "invalid" || state === "failed") {
    toast(`开不了：${why}`);
    return;
  }
  if (launching.has(item.name)) return;

  launching.add(item.name);
  row.disabled = true;
  try {
    const created = await api("sessions", {
      method: "POST",
      headers: { "content-type": "application/json" },
      // 预设打底；name 带时间——同一个模板开两条，列表里分得出来。
      body: JSON.stringify({ preset: item.name, name: `${item.name} ${stamp()}` }),
    });

    // 认引擎回的生效值。说了要却被丢掉的东西必须点名，静默降级比报错难查。
    const missing = [
      ...(created.missing ?? []).map((n) => `技能 ${n}`),
      ...(created.missingExtensions ?? []).map((n) => `扩展 ${n}`),
      ...(created.missingTemplates ?? []).map((n) => `模板 ${n}`),
    ];
    if (missing.length) toast(`没装上：${missing.join("、")}`.slice(0, 90));

    location.hash = `#/c/${created.id}`;
  } catch (error) {
    toast(`开不了：${String(error.message).slice(0, 70)}`);
  } finally {
    launching.delete(item.name);
    row.disabled = false;
  }
};

/* 右边的 A–Z。按住上滑就是标尺（跟微信一个手势），松手不弹东西。 */
const scrollToLetter = (letter) => {
  const group = templatesEl.querySelector(`.group[data-letter="${letter}"]`);
  if (!group) return;
  // 两下 offsetTop 同一个 offsetParent（#page-templates），相减就是它在滚动区里的位置。
  templatesEl.scrollTop = group.offsetTop - templatesEl.offsetTop;
};

const scrub = (event) => {
  const nodes = [...indexEl.children];
  if (!nodes.length) return;
  let best = nodes[0];
  let nearest = Infinity;
  for (const node of nodes) {
    const rect = node.getBoundingClientRect();
    const d = Math.abs(rect.top + rect.height / 2 - event.clientY);
    if (d < nearest) {
      nearest = d;
      best = node;
    }
  }
  const letter = best.dataset.letter;
  if (indexEl.dataset.active === letter) return;
  indexEl.dataset.active = letter;
  for (const node of nodes) node.classList.toggle("on", node === best);
  navigator.vibrate?.(5);
  scrollToLetter(letter);
};

indexEl.addEventListener("pointerdown", (event) => {
  event.preventDefault();
  indexEl.classList.add("hit");
  // 合成事件的 pointerId 可能根本不存在，抓不住就算了 —— 抓取只是锦上添花。
  try {
    indexEl.setPointerCapture?.(event.pointerId);
  } catch {
    /* 没抓住，下面的 pointermove 照样能滚 */
  }
  scrub(event);
});
indexEl.addEventListener("pointermove", (event) => {
  if (indexEl.classList.contains("hit")) scrub(event);
});
for (const name of ["pointerup", "pointercancel"]) {
  indexEl.addEventListener(name, () => {
    indexEl.classList.remove("hit");
    delete indexEl.dataset.active;
  });
}

/* 底栏：有 data-tab 的归路由，没 data-tab 的还是那三个「未开发」（归 data-todo 的 toast）。 */
$("#tabbar").addEventListener("click", (event) => {
  const tab = event.target.closest(".tab");
  if (!tab?.dataset.tab) return;
  const target = tab.dataset.tab === "templates" ? "#/templates" : "#/";
  if (location.hash === target) route(); // 已经在这一页：再点一次当刷新
  else location.hash = target;
});

/* 搜索不是常驻入口：常态只有右上角放大镜，点了才把导航栏换成搜索条 + 取消。 */
const closeTemplateSearch = () => {
  const nav = $("#tpl-nav");
  if (!nav?.classList.contains("searching")) return;
  nav.classList.remove("searching");
  $("#tpl-searchbar").hidden = true;
  queryEl.value = "";
  tpl.q = "";
  renderTemplates();
};

$("#tpl-open-search").addEventListener("click", () => {
  $("#tpl-nav").classList.add("searching");
  $("#tpl-searchbar").hidden = false;
  queryEl.focus();
});

$("#tpl-cancel-search").addEventListener("click", closeTemplateSearch);

queryEl.addEventListener("input", () => {
  tpl.q = queryEl.value ?? "";
  renderTemplates();
});

/* ── 归档库页：按 gen 分天，四态由引擎的 archived / live 推 ──────────
   四态的判断照抄 mcfpqm（现成的消费者），不自己编第二套：

     gone    墓碑 purged（撕过原件）→ 只能打捞
     here    归档过、原件还在 → 可以撕
     back    捞回来了、墓碑抹了（复印件照旧留着）→ 不用操作
     orphan  复印件在、活库没有、也没墓碑 → 盘被人动过，只让人看

   「彻底删除」只在这一页有 —— 主列表永远不出现。 */

const archiveListEl = $("#archive-list");

const stateOf = (item) => {
  if (item.archived?.purged) return "gone";
  if (item.live && item.archived) return "here";
  if (item.live) return "back";
  return "orphan";
};

const STATE_LABEL = { gone: "已销毁", here: "原件在", back: "已捞回", orphan: "无墓碑" };

const pad2 = (n) => String(n).padStart(2, "0");
const fmtGen = (gen) => (/^\d{8}$/.test(gen) ? `${gen.slice(4, 6)}月${gen.slice(6, 8)}日` : gen);

const archiveRow = (item, gen) => {
  const kind = stateOf(item);
  const name = item.name || `会话 ${item.id}`;

  const row = el("button", `row arch-row${kind === "gone" ? " gone" : ""}`);
  row.type = "button";
  row.dataset.id = item.id;
  row.dataset.gen = gen;

  const main = el("div", "row-main");
  const rowName = el("div", "row-name");
  rowName.append(el("span", null, name));
  main.append(rowName);
  main.append(el("div", "row-last", item.last?.text ? preview(item.last.text) : `归档于 ${fmtGen(gen)}`));
  row.append(main);

  const state = el("span", `arch-state ${kind}`, STATE_LABEL[kind]);
  state.dataset.state = kind;
  row.append(state);

  // 只给两种操作留口子：撕（原件还在时）和打捞（原件没了 / 无墓碑）。
  const acts = el("div", "arch-acts");
  if (kind === "here") {
    const purge = el("span", "arch-act purge", "撕");
    purge.addEventListener("click", (event) => {
      event.stopPropagation();
      confirmPurge({ id: item.id, name }, async () => {
        try {
          await api(`sessions/${item.id}/purge`, { method: "POST" });
          toast("已撕掉原件 · 归档库那份复印件也一起销毁了");
          loadArchive();
        } catch (error) {
          toast(String(error.message).slice(0, 90));
        }
      });
    });
    acts.append(purge);
  }
  if (kind === "gone" || kind === "orphan") {
    const restore = el("span", "arch-act restore", "打捞");
    restore.addEventListener("click", async (event) => {
      event.stopPropagation();
      try {
        await api(`archive/${item.id}/restore`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ gen }),
        });
        toast("已捞回 · 回主列表看看");
        loadArchive();
      } catch (error) {
        toast(String(error.message).slice(0, 90));
      }
    });
    acts.append(restore);
  }
  if (acts.children.length) row.append(acts);

  return row;
};

const loadArchive = async () => {
  let data;
  try {
    data = await api("archive");
  } catch (error) {
    archiveListEl.replaceChildren(el("div", "empty", `连不上引擎：${error.message}`));
    return;
  }

  const gens = (data.gens ?? []).filter((g) => g.items.length);
  if (!gens.length) {
    archiveListEl.replaceChildren(el("div", "empty", "归档库是空的。去会话列表长按一条，先归档。"));
    return;
  }

  const frag = document.createDocumentFragment();
  for (const group of gens) {
    const head = el("div", "arch-gen", fmtGen(group.gen));
    head.dataset.gen = group.gen;
    frag.append(head);
    // 引擎给的是新的在前？两边都不赌：按 id 稳定排序，重拉不闪。
    for (const item of group.items.slice().sort((a, b) => a.id - b.id)) {
      frag.append(archiveRow(item, group.gen));
    }
  }
  archiveListEl.replaceChildren(frag);
};

$("#open-archive").addEventListener("click", () => {
  location.hash = "#/archive";
});

$("#archive-refresh").addEventListener("click", loadArchive);

$("#archive-back").addEventListener("click", () => {
  if (history.length > 1) history.back();
  else location.hash = "#/";
});

// PWA：只在安全上下文注册。`http://IP:4790` 下注册必然失败，而且失败了没人会去看。
if ("serviceWorker" in navigator && window.isSecureContext) {
  navigator.serviceWorker.register("./sw.js").catch(() => {});
}

route();

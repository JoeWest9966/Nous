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

/* ── 长按 / 右键菜单 ─────────────────────────────────── */

const HOLD_MS = 400;      // 按到这么久才进入"选中"态
const MOVE_SLOP = 10;     // 手指挪超过这么多 = 想滚动，不算长按
const ARROW_W = 14;       // 小三角的宽
const ARROW_H = 7;        // 小三角的高
const MENU_GAP = 8;       // 菜单和手指之间的缝

let holdFired = false;
/** 当前列表的置顶序（id 数组）。loadSessions 时从服务端拿，乐观更新时自己改。 */
let rowOrder = [];
/** 菜单开着时保持变暗的那一行。菜单一关就恢复。 */
let heldRow = null;

/**
 * 微信那套节奏，一步不差：
 *
 *   按下 → 400ms 内不动，列表还能照常滚动（长按没"生效"）；
 *   到阈值 → 震一下 + 这一行变暗（holding），但菜单**还不出现**；
 *   松手 → 菜单从手指位置弹出来，带指向手指的小三角 + 弹簧。
 *   到阈值之后再滑开 > 10px → 取消（不弹）。
 *
 * 电脑右键：当场弹，锚点 = 鼠标位置。
 */
const hold = (row, id) => {
  let startX = 0;
  let startY = 0;
  let armed = false; // 到了阈值，行已经变暗
  let active = false; // 这次按压还在长按流程里
  let timer = 0;

  const disarm = () => {
    active = false;
    clearTimeout(timer);
    if (armed) {
      armed = false;
      row.classList.remove("holding");
    }
  };

  row.addEventListener("pointerdown", (event) => {
    // 右键不在这里处理：走 document 上的 `contextmenu`（跟宿主 NodeMenu 同一套）。
    // 放在 pointerdown 里弹，浏览器原生菜单照样出来——多浏览器实测都这样。
    if (event.pointerType === "mouse") return; // 左键归 click

    startX = event.clientX;
    startY = event.clientY;
    armed = false;
    active = true;
    holdFired = false;
    clearTimeout(timer);
    timer = setTimeout(() => {
      armed = true;
      row.classList.add("holding");
      navigator.vibrate?.(10);
    }, HOLD_MS);
  });

  row.addEventListener("pointermove", (event) => {
    if (!active || event.pointerType === "mouse") return;
    const dx = event.clientX - startX;
    const dy = event.clientY - startY;
    if (dx * dx + dy * dy > MOVE_SLOP * MOVE_SLOP) disarm();
  });

  row.addEventListener("pointerup", (event) => {
    if (event.pointerType === "mouse" || !active) return;
    active = false;
    clearTimeout(timer);
    if (armed) {
      armed = false;
      row.classList.remove("holding");
      holdFired = true;
      openRowMenu(row, id, event.clientX, event.clientY);
    }
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

const openRowMenu = (row, id, x, y) => {
  closeRowMenu();
  setHeldRow(row);
  // pinned 读**当前** dataset —— 乐观更新后 row 的状态已经变了，闭包里的旧值会撒谎。
  const pinned = row.dataset.pinned === "1";
  const menu = el("div", "rowmenu");
  menu.append(
    menuItem(pinned ? "取消置顶" : "置顶该聊天", () => togglePin(row, id)),
  );
  document.body.append(menu);
  place(menu, x, y);
  // 捕获阶段监听：手指碰到菜单以外的任何地方（包括别的行）就关。
  document.addEventListener("pointerdown", onMenuOutside, true);
};

/**
 * 右键菜单照抄宿主 NodeMenu 的做法：在 `contextmenu` 里 `preventDefault` + 弹菜单。
 * 浏览器原生菜单的抑制只认 contextmenu 这一个事件 —— 放在 pointerdown 里弹、
 * 再另挂一个 contextmenu 去拦，是两套逻辑，原生菜单照样出来（多浏览器实测都这样）。
 * 别处右键（空白、输入框）还给浏览器。
 */
document.addEventListener("contextmenu", (event) => {
  const row = event.target.closest("#sessions .row");
  if (!row) return;
  event.preventDefault();
  openRowMenu(row, row.dataset.id, event.clientX, event.clientY);
});

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
const menuItem = (label, run) => {
  const item = el("button", "rowmenu-item", label);
  item.type = "button";
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
    // 合成 click 还在路上：menuItem 必须留在 DOM 里当它的目标，否则 click 会
    // 穿透到底下的会话行，直接跳进会话。下一帧再关菜单。
    touchHandled = true;
    requestAnimationFrame(closeRowMenu);
    event.preventDefault();
  });

  item.addEventListener("pointercancel", () => {
    if (!active) return;
    active = false;
    item.classList.remove("pressed");
    // 滑出菜单后被系统手势接管（浏览器发 cancel 而不是 up）：跟滑出松手一样，只关不执行。
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

const closeRowMenu = () => {
  document.querySelector(".rowmenu")?.remove();
  setHeldRow(null);
  document.removeEventListener("pointerdown", onMenuOutside, true);
};

const onMenuOutside = (event) => {
  if (!event.target.closest(".rowmenu")) closeRowMenu();
};

/** 菜单锚定手指：默认在手指下方（箭头朝上），放不下翻到上方（箭头朝下）。 */
const place = (menu, x, y) => {
  requestAnimationFrame(() => {
    const vw = document.documentElement.clientWidth;
    const vh = document.documentElement.clientHeight;
    const mw = menu.offsetWidth;
    const mh = menu.offsetHeight;

    // 菜单水平居中于手指，出屏就贴边。
    let left = x - mw / 2;
    left = Math.max(8, Math.min(left, vw - mw - 8));

    // 箭头水平对齐手指，但别跑出菜单圆角外面。
    let arrowX = x - left - ARROW_W / 2;
    arrowX = Math.max(12, Math.min(arrowX, mw - 12 - ARROW_W));

    let below = true;
    let top = y + MENU_GAP;
    if (top + mh > vh - 8) {
      below = false;
      top = y - MENU_GAP - mh;
    }
    if (top < 8) top = 8;

    menu.style.top = `${top}px`;
    menu.style.left = `${left}px`;
    menu.dataset.arrow = below ? "up" : "down";
    menu.style.setProperty("--arrow-x", `${arrowX}px`);
    // 弹簧从箭头那个点往外长，而不是从菜单角落。
    menu.style.transformOrigin = `${arrowX + ARROW_W / 2}px ${below ? 0 : mh}px`;
  });
};

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

// 菜单开着时，列表一滚就关（滚之前的那一下 pointerdown 其实已经关了，这是兜底）。
listEl.addEventListener("scroll", closeRowMenu, { passive: true });

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
  // 离开会话：先关流。画哪一屏交给下面 —— 回列表还是去模板，hash 说了算。
  if (thread.id != null) closeThread();

  if (/^#\/templates\/?$/.test(location.hash)) {
    show("templates");
    loadTemplates();
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

const PAGES = { list: "#page-list", templates: "#page-templates", thread: "#page-thread" };

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

$("#tpl-search").addEventListener("submit", (event) => event.preventDefault());

queryEl.addEventListener("input", () => {
  tpl.q = queryEl.value ?? "";
  renderTemplates();
});

// PWA：只在安全上下文注册。`http://IP:4790` 下注册必然失败，而且失败了没人会去看。
if ("serviceWorker" in navigator && window.isSecureContext) {
  navigator.serviceWorker.register("./sw.js").catch(() => {});
}

route();

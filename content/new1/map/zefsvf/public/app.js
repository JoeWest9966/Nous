// tailnet 架构图。没有框架、没有构建。
//
// 两条自我约束：
// 1. 地址全用相对路径（宿主把这一页反代在 /p/<board>/<id>/ 下，绝对路径会逃出前缀）。
// 2. 图是结构，不是账本：一屏里只有方块和线；一台机器有几个 IP、key 什么时候到期、
//    走哪个中继 —— 点开那块才出现。所有信息摊在一页上，等于一页上什么都没说清。
//
// 视觉照 https://pele.run/examples/architecture 那套来：48px 图标方块 + 集群框 +
// 直边箭头，连线的两端吸附在方块边上（不是文字上）。

const here = location.pathname.endsWith("/") ? location.pathname : location.pathname.replace(/[^/]*$/, "");
const api = (p) => new URL(p, location.origin + here).toString();

const $ = (sel) => document.querySelector(sel);
const mapEl = $("#map");
const wrapEl = $("#map-wrap");
const stageHint = $("#hint");
const insEl = $("#inspector");
const insBody = $("#ins-body");
const shellEl = $("#shell");
const backdropEl = $("#backdrop");
const bannerEl = $("#banner");
const toastEl = $("#toast");

// ── 小工具 ────────────────────────────────────────────────────────────────

const esc = (s) =>
  String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

/** 「3h 前」「76d 前」—— 精确到秒的时间戳在这一页上没有信息量。 */
const ago = (iso) => {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (!Number.isFinite(t)) return "";
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 45) return "刚刚";
  if (s < 3600) return `${Math.round(s / 60)} 分钟前`;
  if (s < 86400) return `${Math.round(s / 3600)} 小时前`;
  const d = Math.round(s / 86400);
  return d < 60 ? `${d} 天前` : `${Math.round(d / 30)} 个月前`;
};

const inDays = (iso) => {
  const t = Date.parse(iso ?? "");
  return Number.isFinite(t) ? (t - Date.now()) / 86400_000 : null;
};

const bytes = (n) => {
  if (!n) return "0";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 && i > 0 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
};

const wideChar = (ch) => /[\u1100-\u115f\u2e80-\ua4cf\uac00-\ud7a3\uf900-\ufaff\uff00-\uff60]/.test(ch);
const textW = (s, size) => {
  let w = 0;
  for (const ch of s) w += wideChar(ch) ? size : size * 0.58;
  return w;
};
const fit = (s, max, size) => {
  const str = String(s ?? "");
  if (textW(str, size) <= max) return str;
  let out = "";
  for (const ch of str) {
    if (textW(out + ch, size) > max - size * 0.7) break;
    out += ch;
  }
  return `${out}…`;
};

let toastTimer = 0;
const toast = (msg) => {
  toastEl.textContent = msg;
  toastEl.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    toastEl.hidden = true;
  }, 1500);
};

const copy = async (text) => {
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // 非 https / 老浏览器：退回到 execCommand，能用就行。
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.append(ta);
    ta.select();
    document.execCommand("copy");
    ta.remove();
  }
  toast(`已复制 ${text}`);
};

// ── 图标（24×24 描边，跟 pele 那套一个路子） ──────────────────────────────

const ICONS = {
  server: '<rect x="2" y="3" width="20" height="7" rx="2"/><rect x="2" y="14" width="20" height="7" rx="2"/><path d="M6 6.5h.01M6 17.5h.01"/>',
  laptop: '<path d="M4 5h16v10H4z"/><path d="M2 19h20"/>',
  phone: '<rect x="7" y="2" width="10" height="20" rx="2.4"/><path d="M11 18h2"/>',
  cloud: '<path d="M17.5 19H9a6.5 6.5 0 1 1 6.2-8.5h2.3a4.25 4.25 0 1 1 0 8.5Z"/>',
  terminal: '<path d="m4 16 5-5-5-5"/><path d="M11 18h8"/>',
  wifi: '<path d="M2.5 9a15 15 0 0 1 19 0"/><path d="M6 12.6a10 10 0 0 1 12 0"/><path d="M9.5 16.2a5 5 0 0 1 5 0"/><path d="M12 19.5h.01"/>',
  power: '<path d="M12 3v9"/><path d="M6.4 7.2a8 8 0 1 0 11.2 0"/>',
  box: '<path d="m12 2.5 8.5 4.6v9.8L12 21.5 3.5 16.9V7.1z"/><path d="m3.5 7.1 8.5 4.6 8.5-4.6"/><path d="M12 11.7v9.8"/>',
  book: '<path d="M4 4.5h11a3 3 0 0 1 3 3v12H7a3 3 0 0 1-3-3z"/><path d="M8 8.5h6M8 12h6"/>',
  link: '<path d="M9.5 14.5 14.5 9.5"/><path d="M7 12 5 14a3.5 3.5 0 0 0 5 5l2-2"/><path d="M17 12l2-2a3.5 3.5 0 0 0-5-5l-2 2"/>',
};

const ICON_ATTRS = 'fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"';

const iconSvg = (name, { x = 0, y = 0, size = 24, cls = "ic" } = {}) =>
  `<svg class="${cls}" x="${x}" y="${y}" width="${size}" height="${size}" viewBox="0 0 24 24" ${ICON_ATTRS}>${ICONS[name] ?? ICONS.server}</svg>`;

const iconHtml = (name, size = 18) =>
  `<svg width="${size}" height="${size}" viewBox="0 0 24 24" ${ICON_ATTRS}>${ICONS[name] ?? ICONS.server}</svg>`;

// ── 状态 ──────────────────────────────────────────────────────────────────

const state = {
  snap: null,
  reg: null, // 服务台账（data/registry.json，人写的）
  sel: null, // "node:<ip>" | "svc:<name>" | "reg:<id>"
  tiles: new Map(), // 上一帧的方块，给详情面板取数据用
  pings: new Map(), // ip -> { ok, ms, direct, region, addr, error }
  tcp: new Map(), // "ip:port" -> { ok, ms, error }
  regProbe: new Map(), // "node:port" -> { ok, ms, error }
  busy: new Set(), // 正在测的 key
  q: "",
  at: 0,
  loading: false,
  autoPinged: false,
};

const KEY = (kind, id) => `${kind}:${id}`;
const nodeKey = (n) => KEY("node", n.ip4 || n.id);
const osIcon = (os) => (/android|ios/i.test(os ?? "") ? "phone" : /windows|mac|darwin/i.test(os ?? "") ? "laptop" : "server");
const svcPort = (s) => String(s.ports?.[0] ?? 443);

// ── 场景：谁在图里、在哪、连给谁 ────────────────────────────────────────────

const CELL_W = 124;
const TILE = 48;
const LABEL_H = 36; // 名字 + IP 两行
const ROW_H = 100;
const BOX_PAD_X = 14;
const BOX_PAD_TOP = 16;
const BOX_PAD_BOTTOM = 18;
const CAP_H = 26;
const BAND_GAP = 46;
const COL_GAP = 28;
const PAD = 4;

function tiles() {
  const map = new Map();
  const put = (t) => {
    map.set(t.key, t);
    return t;
  };

  for (const n of state.snap?.nodes ?? []) {
    put({
      key: nodeKey(n),
      kind: "node",
      icon: n.self ? "terminal" : osIcon(n.os),
      label: n.host || n.dns || "?",
      sub: n.ip4 || n.ip6 || "—",
      tone: n.self ? "me" : n.online ? "on" : "off",
      online: n.online,
      data: n,
      hay: [n.host, n.dns, n.ip4, n.ip6, n.os, n.owner, ...(n.tags ?? [])].join(" ").toLowerCase(),
    });
  }

  for (const s of state.snap?.services ?? []) {
    put({
      key: KEY("svc", s.name),
      kind: "svc",
      icon: "cloud",
      label: s.name,
      sub: s.ports?.length ? s.ports.map((p) => `:${p}`).join(" ") : "svc",
      tone: "svc",
      data: s,
      hay: [s.name, s.dns, s.ip4, s.ip6].join(" ").toLowerCase(),
    });
  }

  for (const e of state.reg?.entries ?? []) {
    const host = (state.snap?.nodes ?? []).find((n) => n.ip4 === e.node || n.ip6 === e.node);
    put({
      key: KEY("reg", e.id || e.name),
      kind: "reg",
      icon: "box",
      label: e.name,
      sub: `${host?.host ?? e.node}:${e.port}`,
      tone: "reg",
      data: e,
      hay: [e.name, e.kind, e.node, e.url, e.auth, ...(e.facts ?? []).map((f) => `${f.k} ${f.v}`)]
        .join(" ")
        .toLowerCase(),
    });
  }

  return map;
}

/** 一个方框：标题在外面（框里全是方块，线可以随便从顶上进来）。 */
function box(id, caption, icon, items, cols) {
  const list = items.filter(Boolean);
  const c = Math.max(1, Math.min(cols, Math.max(1, list.length)));
  const rows = Math.max(1, Math.ceil(list.length / c));
  return {
    id,
    caption,
    icon,
    items: list,
    cols: c,
    w: c * CELL_W + BOX_PAD_X * 2,
    h: (rows - 1) * ROW_H + TILE + LABEL_H + BOX_PAD_TOP + BOX_PAD_BOTTOM,
    x: 0,
    y: 0,
  };
}

/**
 * 列数：先看这一列放得下几格，再把行数摊平。
 * 4 块塞进 3 列会变成 3+1（第二行孤零零一个），所以 4 块就是 2×2。
 */
const fitCols = (n, avail) => {
  const count = Math.max(1, n || 1);
  const max = Math.max(1, Math.floor((avail - BOX_PAD_X * 2) / CELL_W));
  const rows = Math.max(1, Math.ceil(count / max));
  return Math.max(1, Math.min(count, max, Math.ceil(count / rows)));
};

function scene(width) {
  const W = Math.max(300, width);
  const tilesMap = tiles();
  const nodes = state.snap?.nodes ?? [];
  const self = nodes.find((n) => n.self) ?? null;
  const others = nodes.filter((n) => !n.self);
  const online = others.filter((n) => n.online);
  const offline = others.filter((n) => !n.online);
  const svcs = state.snap?.services ?? [];
  const regs = state.reg?.entries ?? [];

  const T = (n) => tilesMap.get(nodeKey(n));
  const selfBox = self ? box("self", "这台机器", "terminal", [T(self)], 1) : null;
  const onBox = box("online", `在线 · ${online.length}`, "wifi", online.map(T), 4);
  const offBox = offline.length ? box("offline", `离线 · ${offline.length}`, "power", offline.map(T), 4) : null;
  const svcBox = svcs.length ? box("svc", `服务 VIP · ${svcs.length}`, "cloud", svcs.map((s) => tilesMap.get(KEY("svc", s.name))), 3) : null;
  const regBox = regs.length
    ? box("reg", `台账 · ${regs.length}`, "book", regs.map((e) => tilesMap.get(KEY("reg", e.id || e.name))), 2)
    : null;

  const usable = W - PAD * 2;
  const wide = usable >= 680 && onBox.items.length > 0;

  const boxes = [];
  let totalH = 0;

  const place = (b, x, y) => {
    b.x = x;
    b.y = y;
    b.items.forEach((t, i) => {
      const r = Math.floor(i / b.cols);
      const c = i % b.cols;
      t.x = x + BOX_PAD_X + c * CELL_W;
      t.y = y + BOX_PAD_TOP + r * ROW_H;
      t.cx = t.x + CELL_W / 2;
      t.cy = t.y + TILE / 2;
      t.boxX = t.x + (CELL_W - TILE) / 2;
      t.col = c;
      t.row = r;
      t.cluster = b;
      t.tile = true;
    });
    boxes.push(b);
  };

  if (wide) {
    const leftAvail = Math.floor(usable * 0.6);
    const rightAvail = usable - leftAvail - COL_GAP;

    onBox.cols = fitCols(online.length, leftAvail);
    onBox.w = onBox.cols * CELL_W + BOX_PAD_X * 2;
    onBox.h = (Math.max(1, Math.ceil(online.length / onBox.cols)) - 1) * ROW_H + TILE + LABEL_H + BOX_PAD_TOP + BOX_PAD_BOTTOM;

    if (offBox) {
      offBox.cols = fitCols(offline.length, leftAvail);
      offBox.w = offBox.cols * CELL_W + BOX_PAD_X * 2;
      offBox.h =
        (Math.max(1, Math.ceil(offline.length / offBox.cols)) - 1) * ROW_H + TILE + LABEL_H + BOX_PAD_TOP + BOX_PAD_BOTTOM;
    }

    if (regBox) {
      regBox.cols = fitCols(regs.length, rightAvail);
      regBox.w = regBox.cols * CELL_W + BOX_PAD_X * 2;
      regBox.h = (Math.max(1, Math.ceil(regs.length / regBox.cols)) - 1) * ROW_H + TILE + LABEL_H + BOX_PAD_TOP + BOX_PAD_BOTTOM;
    }
    if (svcBox) {
      svcBox.cols = fitCols(svcs.length, rightAvail);
      svcBox.w = svcBox.cols * CELL_W + BOX_PAD_X * 2;
      svcBox.h = (Math.max(1, Math.ceil(svcs.length / svcBox.cols)) - 1) * ROW_H + TILE + LABEL_H + BOX_PAD_TOP + BOX_PAD_BOTTOM;
    }

    const leftW = Math.max(onBox.w, offBox?.w ?? 0);
    const rightW = Math.max(svcBox?.w ?? 0, regBox?.w ?? 0);
    const rowW = leftW + (rightW ? COL_GAP + rightW : 0);

    if (rowW <= usable) {
      const startX = PAD + Math.floor((usable - rowW) / 2);
      let y = CAP_H + PAD;

      if (selfBox) {
        const sx = Math.max(PAD, startX + Math.floor((leftW - selfBox.w) / 2));
        place(selfBox, sx, y);
        y += selfBox.h + BAND_GAP;
      }

      // 每列内的方框居中：本机压在「在线」的正中，撑开的那一列（离线多一块）也不会把上面顶歪。
      place(onBox, startX + Math.floor((leftW - onBox.w) / 2), y);
      if (offBox) place(offBox, startX, y + onBox.h + CAP_H);
      // 右列两块都靠左：读起来是一摞，不是两个各自居中的东西。
      if (regBox) place(regBox, startX + leftW + COL_GAP, y);
      if (svcBox) place(svcBox, startX + leftW + COL_GAP, y + (regBox?.h ?? 0) + (regBox ? CAP_H : 0));

      const left = y + onBox.h + (offBox ? CAP_H + offBox.h : 0);
      const right = y + (regBox?.h ?? 0) + (regBox ? CAP_H : 0) + (svcBox?.h ?? 0);
      totalH = Math.max(left, right) + PAD;
    } else {
      return stackAll([selfBox, onBox, offBox, regBox, svcBox], usable, tilesMap, W);
    }
  } else {
    return stackAll([selfBox, onBox, offBox, regBox, svcBox], usable, tilesMap, W);
  }

  return finish(boxes, totalH, W, tilesMap, self, others, online);
}

function stackAll(list, usable, tilesMap, W) {
  const boxes = [];
  let y = PAD;
  let maxW = 0;

  for (const b of list) {
    if (!b) continue;
    b.cols = fitCols(b.items.length, usable);
    b.w = b.cols * CELL_W + BOX_PAD_X * 2;
    b.h = (Math.max(1, Math.ceil(b.items.length / b.cols)) - 1) * ROW_H + TILE + LABEL_H + BOX_PAD_TOP + BOX_PAD_BOTTOM;
    maxW = Math.max(maxW, b.w);
  }

  for (const b of list) {
    if (!b) continue;
    const x = PAD + Math.floor((usable - b.w) / 2);
    b.x = x;
    b.y = y;
    b.items.forEach((t, i) => {
      const r = Math.floor(i / b.cols);
      const c = i % b.cols;
      t.x = x + BOX_PAD_X + c * CELL_W;
      t.y = y + BOX_PAD_TOP + r * ROW_H;
      t.cx = t.x + CELL_W / 2;
      t.cy = t.y + TILE / 2;
      t.boxX = t.x + (CELL_W - TILE) / 2;
      t.col = c;
      t.row = r;
      t.cluster = b;
      t.tile = true;
    });
    boxes.push(b);
    y += CAP_H + b.h + BAND_GAP - 10;
  }

  const nodes = state.snap?.nodes ?? [];
  const self = nodes.find((n) => n.self) ?? null;
  const others = nodes.filter((n) => !n.self);
  const online = others.filter((n) => n.online);

  return finish(boxes, y - (BAND_GAP - 10) + PAD, W, tilesMap, self, others, online);
}

function finish(boxes, h, W, tilesMap, self, others, online) {
  const edges = [];
  if (self && tilesMap.get(nodeKey(self))) {
    for (const n of online) {
      const t = tilesMap.get(nodeKey(n));
      if (t?.tile) edges.push({ from: nodeKey(self), to: t.key, ip: n.ip4, kind: "link" });
    }
  }
  for (const e of state.reg?.entries ?? []) {
    const host = others.find((n) => n.ip4 === e.node || n.ip6 === e.node);
    const t = tilesMap.get(KEY("reg", e.id || e.name));
    if (host && t?.tile && tilesMap.get(nodeKey(host))?.tile) {
      edges.push({ from: nodeKey(host), to: t.key, kind: "host", port: e.port });
    }
  }

  return { w: Math.max(W, PAD * 2), h: Math.max(220, h), boxes, edges, tilesMap };
}

// ── 画 ────────────────────────────────────────────────────────────────────

const toneOf = (t) => {
  if (t.kind === "node") return t.data.self ? "me" : t.data.online ? "on" : "off";
  return t.kind;
};

/**
 * 直线会不会从别的方块（包括它下面的名字）身上碾过去。
 * 采点就够：线都很短，8px 一个点，比写线段求交好读。
 */
function crossesTile(a, b, x1, y1, x2, y2, tilesMap) {
  const steps = Math.max(2, Math.ceil(Math.hypot(x2 - x1, y2 - y1) / 8));
  for (let i = 1; i < steps; i++) {
    const t = i / steps;
    const x = x1 + (x2 - x1) * t;
    const y = y1 + (y2 - y1) * t;
    for (const o of tilesMap.values()) {
      if (!o.tile || o === a || o === b) continue;
      if (x > o.x + 2 && x < o.x + CELL_W - 2 && y > o.y + 2 && y < o.y + TILE + LABEL_H - 2) return true;
    }
  }
  return false;
}

/**
 * 一条线怎么走。方向按「谁在哪儿」选，撞上别的方块才改走直角：
 *  - 目标在右边一列：上翻 → 走两列之间的缝 → 落下 → 从左腰进。
 *  - 目标在下面、直线不撞人：直线下去，落在方块顶上。
 *  - 目标在下面、直线撞人：走两行之间的横向公共线，在目标格子左边的缝里落下，
 *    再横着进左腰。它因此永远不会穿过别的方块 —— 手机上 3 台机器折成两行时全靠这条。
 *  - 同一行：直线过去。
 */
function edgeGeom(a, b, tilesMap) {
  const aC = a.cluster;
  const bC = b.cluster;

  // 右边一列：上翻过柱
  if (aC && bC && bC.x >= aC.x + aC.w) {
    const lane = bC.x - 12;
    // 走在标题带上面：标题是「在线 · 3」，线不能从字上压过去。
    const top = aC.y - CAP_H - 8;
    const endX = b.boxX - 6;
    return {
      d: `M ${a.cx} ${a.y - 6} V ${top} H ${lane} V ${b.cy} H ${endX}`,
      arrow: { x: endX, y: b.cy, ang: 0 },
      label: { x: lane + 8, y: b.cy - 8, anchor: "start" },
    };
  }

  if (a.y + TILE + 8 < b.y) {
    const x1 = a.cx;
    const y1 = a.y + TILE + LABEL_H + 4;
    const x2 = b.cx;
    const y2 = b.y - 2;

    if (!crossesTile(a, b, x1, y1, x2, y2, tilesMap)) {
      const ang = Math.atan2(y2 - y1, x2 - x1);
      return {
        d: `M ${x1} ${y1} L ${x2} ${y2}`,
        arrow: { x: x2, y: y2, ang },
        // 标签挂在自己方块的正上方（一格一个，不会互相撞）。
        label: { x: x2, y: b.y - 7, anchor: "middle" },
      };
    }

    // 横向公共线走在自己这一列的下面：源方块的集群底边再往下 12px。
    // 落点走目标格子的左内边距（tile 在 +38 处，名字最宽 116 —— 两边都让开了）。
    const busY = aC ? aC.y + aC.h + 12 : (y1 + b.y) / 2;
    // 第一列从集群左边的沟里落（标题在框左上角，别压上去）；别的列走格子自己的左内边距。
    const lane = b.col === 0 && bC ? Math.min(b.x + 8, bC.x - 12) : b.x + 8;
    const endX = b.boxX - 6;
    return {
      d: `M ${x1} ${y1} V ${busY} H ${lane} V ${b.cy} H ${endX}`,
      arrow: { x: endX, y: b.cy, ang: 0 },
      label: { x: b.cx, y: b.y - 7, anchor: "middle" },
    };
  }

  const aRight = a.boxX + TILE;
  const toRight = b.boxX > aRight;
  const x1 = toRight ? aRight + 4 : a.boxX - 4;
  const x2 = toRight ? b.boxX - 6 : b.boxX + TILE + 6;
  const ang = toRight ? 0 : Math.PI;
  return {
    d: `M ${x1} ${a.cy} L ${x2} ${b.cy}`,
    arrow: { x: x2, y: b.cy, ang },
    label: { x: (x1 + x2) / 2, y: b.cy - 8, anchor: "middle" },
  };
}

function edgeState(ip) {
  const p = ip ? state.pings.get(ip) : null;
  if (!p) return { cls: "idle", label: "" };
  if (!p.ok) return { cls: "bad", label: "不通" };
  if (p.direct) return { cls: "direct", label: `${Math.round(p.ms)}ms 直连` };
  return { cls: "derp", label: `${Math.round(p.ms)}ms DERP` };
}

function renderMap() {
  const W = Math.max(300, wrapEl.clientWidth || 900);
  const sc = scene(W);
  state.tiles = sc.tilesMap;

  if (!sc.boxes.length) {
    mapEl.setAttribute("viewBox", "0 0 320 160");
    mapEl.setAttribute("height", "160");
    mapEl.setAttribute("width", "320");
    mapEl.innerHTML = `<text class="empty" x="10" y="30">正在读 tailscaled…</text>`;
    return sc;
  }

  const parts = [];

  // 集群底板先画：线走在上面，方块盖在最上面。
  for (const b of sc.boxes) {
    parts.push(
      `<g class="cluster" data-box="${esc(b.id)}">
        <rect class="cbox" x="${b.x}" y="${b.y}" width="${b.w}" height="${b.h}" rx="14"/>
        ${iconSvg(b.icon, { x: b.x + 2, y: b.y - CAP_H + 4, size: 13, cls: "cap-ic" })}
        <text class="cap" x="${b.x + 20}" y="${b.y - 10}">${esc(b.caption)}</text>
      </g>`,
    );
  }

  // 连线
  const labels = [];
  for (const e of sc.edges) {
    const a = sc.tilesMap.get(e.from);
    const b = sc.tilesMap.get(e.to);
    if (!a?.tile || !b?.tile) continue;

    // 台账那条线只在宽屏画（台账在右边一列，一条干净的直角线）。
    // 窄屏是竖着摞的，画一条横穿好几层的线不如让方块自己说「j1900:8443」。
    if (e.kind === "host" && !(a.cluster && b.cluster && b.cluster.x >= a.cluster.x + a.cluster.w)) continue;

    const g = edgeGeom(a, b, sc.tilesMap);
    if (!g) continue;
    const st = edgeState(e.ip);
    const dim = state.q && (!a.hay.includes(state.q) || !b.hay.includes(state.q));

    const { x, y, ang } = g.arrow;
    const back = 4; // 别让箭头把线尾盖住
    const tipX = x - back * Math.cos(ang);
    const tipY = y - back * Math.sin(ang);
    const s = 7;
    const arrow = [
      [x, y],
      [x - s * Math.cos(ang - 0.42), y - s * Math.sin(ang - 0.42)],
      [x - s * Math.cos(ang + 0.42), y - s * Math.sin(ang + 0.42)],
    ]
      .map((p) => `${p[0].toFixed(1)},${p[1].toFixed(1)}`)
      .join(" ");

    parts.push(
      `<g class="edge ${st.cls}${e.kind === "host" ? " host" : ""}${dim ? " dim" : ""}" data-edge="${esc(e.from)}>${esc(e.to)}">
        <path d="${g.d}" fill="none"/>
        <polygon class="arw" points="${arrow}"/>
      </g>`,
    );

    if (st.label) {
      labels.push(
        `<text class="elbl ${st.cls}" x="${g.label.x.toFixed(1)}" y="${g.label.y.toFixed(1)}" text-anchor="${g.label.anchor}">${esc(st.label)}</text>`,
      );
    }
  }

  // 方块
  for (const t of sc.tilesMap.values()) {
    if (!t.tile) continue;
    const dim = state.q && !t.hay.includes(state.q);
    const sel = state.sel === t.key;
    const lbl = fit(t.label, CELL_W - 8, 12);
    const sub = fit(t.sub, CELL_W - 8, 10);
    parts.push(
      `<g class="tile ${toneOf(t)}${dim ? " dim" : ""}${sel ? " sel" : ""}" data-sel="${esc(t.key)}" tabindex="0" role="button"
          aria-label="${esc(`${t.label} ${t.sub}`)}">
        <rect class="hit" x="${t.x}" y="${t.y}" width="${CELL_W}" height="${TILE + LABEL_H}" rx="12"/>
        <rect class="box" x="${t.boxX}" y="${t.y}" width="${TILE}" height="${TILE}" rx="13"/>
        ${iconSvg(t.icon, { x: t.boxX + 12, y: t.y + 12, size: 24 })}
        <circle class="dot" cx="${t.boxX + TILE - 9}" cy="${t.y + 9}" r="3.2"/>
        <text class="lbl" x="${t.cx}" y="${t.y + TILE + 17}">${esc(lbl)}</text>
        <text class="sub" x="${t.cx}" y="${t.y + TILE + 31}">${esc(sub)}</text>
      </g>`,
    );
  }

  parts.push(`<g class="elabels">${labels.join("")}</g>`);

  mapEl.setAttribute("viewBox", `0 0 ${sc.w} ${sc.h}`);
  mapEl.setAttribute("width", String(sc.w));
  mapEl.setAttribute("height", String(sc.h));
  mapEl.innerHTML = parts.join("");

  return sc;
}

// ── 顶栏 / 横幅 / 角标 ────────────────────────────────────────────────────

function renderHeader(snap) {
  const { counts, tailnet, daemon } = snap;
  $("#count").innerHTML = `<b>${counts.online}</b> / ${counts.total} 在线`;

  const bits = [tailnet?.name, tailnet?.suffix ? `MagicDNS ${tailnet.suffix}` : "", daemon?.version ? `v${daemon.version.split("-")[0]}` : ""];
  const net = bits.filter(Boolean).join(" · ");
  $("#net").textContent = net;
  $("#net").title = bits.filter(Boolean).join("\n");

  const warn = [];
  if (!snap.ok) {
    warn.push({
      bad: true,
      text: `拿不到 tailscaled 的实时状态：${snap.error ?? "未知原因"}`,
      detail: snap.at ? `下面是 ${ago(snap.at)} 的快照` : "也没有历史快照可以看",
    });
  }
  if (daemon?.backend && daemon.backend !== "Running") {
    warn.push({ bad: true, text: `tailscaled 状态：${daemon.backend}`, detail: "不是 Running，图上的数据可能一直在变旧" });
  }
  if (daemon?.health?.length) warn.push({ text: `tailscaled 自检：${daemon.health.join("；")}`, detail: "" });

  bannerEl.hidden = warn.length === 0;
  bannerEl.className = `banner${warn.some((w) => w.bad) ? " bad" : ""}`;
  bannerEl.innerHTML = warn
    .map((w) => `<div><span>${esc(w.text)}</span>${w.detail ? `<span class="detail"> · ${esc(w.detail)}</span>` : ""}</div>`)
    .join("");
}

const tickStamp = () => {
  const el = $("#stamp");
  if (!state.at) {
    el.textContent = "";
    return;
  }
  const s = Math.max(0, Math.round((Date.now() - state.at) / 1000));
  el.textContent = s < 2 ? "刚刚" : `${s}s 前`;
};

// ── 详情面板 ──────────────────────────────────────────────────────────────
//
// 这一页的规矩：图上只有「谁、在哪、连给谁」。端口、token、重启命令、坑 ——
// 点开才出现，一次只出现一块。

const chip = (text, cls = "") => `<span class="chip ${cls}">${esc(text)}</span>`;
const kv = (k, v, cls = "") => `<div class="kv-row"><span class="k">${esc(k)}</span><span class="v ${cls}">${v}</span></div>`;
const ipBtn = (v) => `<button class="ip" data-copy="${esc(v)}" data-k="copy:${esc(v)}" title="复制 ${esc(v)}">${esc(v)}</button>`;

const pingRes = (ip, key) => {
  if (state.busy.has(key)) return `<span class="res">测…</span>`;
  const p = state.pings.get(ip);
  if (!p) return `<span class="res dim">还没测</span>`;
  if (!p.ok) return `<span class="res bad" title="${esc(p.error)}">${esc(p.error)}</span>`;
  return p.direct
    ? `<span class="res ok">${Math.round(p.ms)}ms · 直连 <code>${esc(p.addr)}</code></span>`
    : `<span class="res warn">${Math.round(p.ms)}ms · 经 DERP <code>${esc(p.region)}</code></span>`;
};

function peerBody(n) {
  const key = nodeKey(n);
  const d = inDays(n.expires);
  const stateText = n.self
    ? "这台机器（tailscaled 就在它上面）"
    : n.online
      ? n.curAddr
        ? `直连 ${n.curAddr}`
        : `经中继 ${n.relay || "?"}`
      : `离线 · ${ago(n.lastSeen) || "没有记录"}`;

  const attached = (state.reg?.entries ?? []).filter((e) => e.node === n.ip4 || e.node === n.ip6);

  return `
    <div class="ins-sec">
      <div class="chips">
        ${n.self ? chip("本机", "accent") : ""}
        ${chip(n.online ? "在线" : "离线", n.online ? "ok" : "dim")}
        ${n.exitNode ? chip("exit 出口") : ""}
        ${n.exitNodeOption && !n.exitNode ? chip("可当 exit", "dim") : ""}
        ${(n.tags ?? []).map((t) => chip(t, "dim")).join("")}
        ${d !== null && d < 30 ? chip(d < 1 ? "key 今天过期" : `key ${Math.floor(d)}d 后过期`, d < 7 ? "bad" : "warn") : ""}
      </div>
      <div class="kv">
        ${n.ip4 ? kv("IPv4", ipBtn(n.ip4)) : ""}
        ${n.ip6 ? kv("IPv6", ipBtn(n.ip6), "small") : ""}
        ${kv("MagicDNS", `<span class="mono">${esc(n.dns || "—")}</span>`)}
        ${kv("系统", esc(n.os || "—"))}
        ${kv("状态", esc(stateText))}
        ${n.online && (n.rx || n.tx) ? kv("收发", `↓${bytes(n.rx)} ↑${bytes(n.tx)}`) : ""}
        ${n.owner ? kv("owner", `<span class="mono">${esc(n.owner)}</span>`) : ""}
      </div>
    </div>

    <div class="ins-sec">
      <h3>链路</h3>
      <div class="act-row">
        <button class="btn" data-ping="${esc(n.ip4 || n.id)}" data-k="ping:${esc(key)}"
          ${n.self || !n.online ? "disabled" : ""}>
          ${n.self ? "本机不用 ping" : n.online ? (state.pings.get(n.ip4) ? "再测" : "ping 一下") : "离线，测不了"}
        </button>
        ${pingRes(n.ip4, key)}
      </div>
      <p class="note">ping 说的是「此刻从这台机器到那一台」：能直连就走直连，走不通才落 DERP。结果缓存 30 秒。</p>
    </div>

    ${
      attached.length
        ? `<div class="ins-sec"><h3>这台机器上的服务</h3><div class="link-list">
            ${attached
              .map(
                (e) =>
                  `<button class="link-row" data-sel="${esc(KEY("reg", e.id || e.name))}"><span>${esc(e.name)}</span><span class="mono">:${esc(e.port)}</span></button>`,
              )
              .join("")}
          </div></div>`
        : ""
    }
  `;
}

function svcBody(s) {
  const key = `${s.ip4 || s.ip6}:${svcPort(s)}`;
  const busy = state.busy.has(`tcp:${key}`);
  const r = state.tcp.get(key);
  const res = busy
    ? `<span class="res">测…</span>`
    : !r
      ? `<span class="res dim">还没测</span>`
      : r.ok
        ? `<span class="res ok">端口开着 · ${r.ms}ms</span>`
        : `<span class="res bad">${esc(r.error ?? "不通")}</span>`;

  return `
    <div class="ins-sec">
      <div class="chips">
        ${chip("svc VIP")}
        ${(s.ports ?? []).map((p) => chip(`:${p}`, "dim")).join("")}
      </div>
      <div class="kv">
        ${s.ip4 ? kv("IPv4", ipBtn(s.ip4)) : ""}
        ${s.ip6 ? kv("IPv6", ipBtn(s.ip6), "small") : ""}
        ${kv("MagicDNS", `<span class="mono">${esc(s.dns || "—")}</span>`)}
      </div>
    </div>
    <div class="ins-sec">
      <h3>探活</h3>
      <div class="act-row">
        <button class="btn" data-tcp="${esc(s.ip4 || s.ip6)}" data-port="${esc(svcPort(s))}" data-k="tcp:${esc(key)}">
          探 ${svcPort(s)} 端口
        </button>
        ${res}
      </div>
      <p class="note">svc 不是机器：它没有 OS、没有「在线」，只有一个 VIP 和广告出来的端口。
        所以这里探的是 TCP，不是 <code>tailscale ping</code> —— 后者对它只会超时。</p>
    </div>`;
}

function regBody(e) {
  const key = `${e.node}:${e.port}`;
  const busy = state.busy.has(`reg:${key}`);
  const r = state.regProbe.get(key);
  const res = busy
    ? `<span class="res">测…</span>`
    : !r
      ? `<span class="res dim">还没测</span>`
      : r.ok
        ? `<span class="res ok">端口开着 · ${r.ms}ms</span>`
        : `<span class="res bad">${esc(r.error ?? "不通")}</span>`;

  const facts = (e.facts ?? []).map((f) => `<span class="k">${esc(f.k)}</span><span class="v">${esc(f.v)}</span>`).join("");
  const caveats = (e.caveats ?? []).map((c) => `<li>${esc(c)}</li>`).join("");

  const doc = !e.doc
    ? ""
    : e.docExists
      ? `<button class="btn skill" data-copy="${esc(e.doc)}" data-k="doc:${esc(key)}" title="复制路径 · 粘给 agent，它会去读这份说明">复制 doc 路径</button>`
      : `<span class="reg-missing" title="${esc(e.doc)}">说明缺失</span>`;

  return `
    <div class="ins-sec">
      <div class="chips">
        ${e.kind ? chip(e.kind) : ""}
        ${chip(`${e.node}:${e.port}`, "dim mono")}
      </div>
      <div class="kv">
        ${e.url ? kv("地址", `<a class="mono url" href="${esc(e.url)}" target="_blank" rel="noreferrer">${esc(e.url)}</a>`) : ""}
        ${e.scheme ? kv("TLS", esc(e.scheme)) : ""}
        ${e.auth ? kv("认证", esc(e.auth)) : ""}
      </div>
      <div class="act-row">
        ${e.url ? `<button class="btn" data-copy="${esc(e.url)}" data-k="url:${esc(key)}">复制地址</button>` : ""}
        <button class="btn" data-probe-node="${esc(e.node)}" data-probe-port="${esc(e.port)}" data-k="probe:${esc(key)}">探活</button>
        ${doc}
        ${res}
      </div>
    </div>
    ${facts ? `<div class="ins-sec"><h3>记得的事</h3><div class="facts">${facts}</div></div>` : ""}
    ${caveats ? `<div class="ins-sec"><h3>踩过的坑</h3><ul class="caveats">${caveats}</ul></div>` : ""}
  `;
}

function renderInspector() {
  const t = state.sel ? state.tiles.get(state.sel) : null;

  if (!t) {
    state.sel = null;
    insEl.hidden = true;
    shellEl.classList.remove("has-ins");
    backdropEl.hidden = true;
    stageHint.textContent = state.snap ? "点一个方块看细节" : "";
    return;
  }

  insEl.hidden = false;
  shellEl.classList.add("has-ins");
  const narrow = matchMedia("(max-width: 900px)").matches;
  backdropEl.hidden = !narrow;
  stageHint.textContent = "线是关系 · 点 ping 才量链路";

  $("#ins-icon").innerHTML = iconHtml(t.icon, 18);

  const title = t.kind === "node" ? t.data.host || t.data.dns : t.data.name;
  const sub = t.kind === "node" ? t.data.dns || "" : t.kind === "svc" ? "tailscale 广告的 VIP" : `台账 · ${t.data.node}:${t.data.port}`;
  $("#ins-name").textContent = title;
  $("#ins-sub").textContent = sub;

  insBody.innerHTML =
    `<div class="ins-kind ${t.kind}">${t.kind === "node" ? "设备" : t.kind === "svc" ? "服务 VIP" : "服务台账"}</div>` +
    (t.kind === "node" ? peerBody(t.data) : t.kind === "svc" ? svcBody(t.data) : regBody(t.data));

  insBody.scrollTop = 0;
}

// ── 一次画齐 ──────────────────────────────────────────────────────────────

function renderAll() {
  if (!state.snap) return;

  const act = document.activeElement;
  const focusK = act?.closest?.("[data-k]")?.dataset?.k ?? "";

  renderHeader(state.snap);
  renderMap();
  renderInspector();

  const regCount = state.reg?.entries?.length ?? 0;
  const svcCount = state.snap.services?.length ?? 0;
  $("#foot-counts").textContent = `${state.snap.counts.online} 在线 / ${state.snap.counts.total} 台 · ${svcCount} 个 svc · ${regCount} 条台账`;
  tickStamp();

  if (focusK) document.querySelector(`[data-k="${CSS.escape(focusK)}"]`)?.focus?.();
}

// ── 数据 ──────────────────────────────────────────────────────────────────

const refresh = async () => {
  if (state.loading) return;
  state.loading = true;
  $("#refresh").classList.add("spin");
  try {
    const res = await fetch(api("api/status"), { cache: "no-store" });
    const snap = await res.json();
    state.snap = snap;
    if (snap.ok) state.at = Date.now();
    // 有 ping / 探活正在跑就只更新顶栏：重画会把手指底下那个按钮抽掉（鼠标抬起时就点空了）。
    if (state.busy.size === 0) renderAll();
    else renderHeader(snap);
  } catch (e) {
    bannerEl.hidden = false;
    bannerEl.className = "banner bad";
    bannerEl.innerHTML = `<div>连不上这个块的进程：${esc(e?.message ?? e)}</div>`;
  } finally {
    state.loading = false;
    $("#refresh").classList.remove("spin");
  }
};

const pingOne = async (key) => {
  if (state.busy.has(key)) return;
  state.busy.add(key);
  renderAll();
  try {
    const res = await fetch(api(`api/ping?ip=${encodeURIComponent(key)}`), { cache: "no-store" });
    const out = await res.json();
    state.pings.set(key, out);
    // 服务端认的是 ip4；按钮上是别的键时也把结果挂到 ip4 上，画线时才找得着。
    if (out.ip && out.ip !== key) state.pings.set(out.ip, out);
  } catch (e) {
    state.pings.set(key, { ok: false, error: e?.message ?? String(e) });
  } finally {
    state.busy.delete(key);
    renderAll();
  }
};

const probe = async (ip, port) => {
  const key = `${ip}:${port}`;
  const busyKey = `tcp:${key}`;
  if (state.busy.has(busyKey)) return;
  state.busy.add(busyKey);
  renderAll();
  try {
    const res = await fetch(api(`api/tcp?ip=${encodeURIComponent(ip)}&port=${encodeURIComponent(port)}`), {
      cache: "no-store",
    });
    state.tcp.set(key, await res.json());
  } catch (e) {
    state.tcp.set(key, { ok: false, error: e?.message ?? String(e) });
  } finally {
    state.busy.delete(busyKey);
    renderAll();
  }
};

const probeReg = async (node, port) => {
  const key = `${node}:${port}`;
  const busyKey = `reg:${key}`;
  if (state.busy.has(busyKey)) return;
  state.busy.add(busyKey);
  renderAll();
  try {
    const res = await fetch(api(`api/probe?node=${encodeURIComponent(node)}&port=${encodeURIComponent(port)}`), {
      cache: "no-store",
    });
    state.regProbe.set(key, await res.json());
  } catch (e) {
    state.regProbe.set(key, { ok: false, error: e?.message ?? String(e) });
  } finally {
    state.busy.delete(busyKey);
    renderAll();
  }
};

// 台账只拿一次：它是个磁盘上的文件，不是每秒变一次的网络状况。
// 拿完探一遍活（串行，一条 3 秒封顶），省得你手动点。
const loadRegistry = async () => {
  try {
    const res = await fetch(api("api/registry"), { cache: "no-store" });
    state.reg = await res.json();
  } catch (e) {
    state.reg = { ok: false, entries: [], error: e?.message ?? String(e) };
  }
  renderAll();
  for (const e of state.reg?.entries ?? []) await probeReg(e.node, e.port);
};

const pingAll = async () => {
  const btn = $("#ping-all");
  const targets = (state.snap?.nodes ?? []).filter((n) => n.online && !n.self && n.ip4).map((n) => n.ip4);
  if (!targets.length) return toast("没有在线的机器可测");
  btn.disabled = true;
  try {
    for (const ip of targets) await pingOne(ip);
  } finally {
    btn.disabled = false;
  }
  const ok = targets.filter((ip) => state.pings.get(ip)?.ok).length;
  toast(`${ok}/${targets.length} 台有回音`);
};

/** 第一次拿到快照时量一遍链路：图上那些线才有数字。之后按需 —— 不每 5 秒烧一次网络。 */
const autoPing = async () => {
  if (state.autoPinged) return;
  state.autoPinged = true;
  const targets = (state.snap?.nodes ?? []).filter((n) => n.online && !n.self && n.ip4).map((n) => n.ip4);
  for (const ip of targets) await pingOne(ip);
};

// ── 事件 ──────────────────────────────────────────────────────────────────

const select = (key) => {
  state.sel = state.sel === key ? null : key;
  renderMap();
  renderInspector();
};

document.addEventListener("click", (e) => {
  const copyBtn = e.target.closest("[data-copy]");
  if (copyBtn) {
    copy(copyBtn.dataset.copy);
    copyBtn.classList.add("copied");
    setTimeout(() => copyBtn.classList.remove("copied"), 900);
    return;
  }

  const selEl = e.target.closest("[data-sel]");
  if (selEl) {
    select(selEl.dataset.sel);
    return;
  }

  const pingBtn = e.target.closest("[data-ping]");
  if (pingBtn && !pingBtn.disabled) {
    e.preventDefault();
    pingOne(pingBtn.dataset.ping);
    return;
  }

  const tcpBtn = e.target.closest("[data-tcp]");
  if (tcpBtn && !tcpBtn.disabled) {
    e.preventDefault();
    probe(tcpBtn.dataset.tcp, tcpBtn.dataset.port);
    return;
  }

  const regBtn = e.target.closest("[data-probe-node]");
  if (regBtn && !regBtn.disabled) {
    e.preventDefault();
    probeReg(regBtn.dataset.probeNode, Number(regBtn.dataset.probePort));
    return;
  }

  if (e.target === backdropEl || e.target.closest("#ins-close")) {
    state.sel = null;
    renderMap();
    renderInspector();
  }
});

// 方块是 role=button：键盘也得能开。
document.addEventListener("keydown", (e) => {
  const tile = e.target.closest?.(".tile");
  if (tile && (e.key === "Enter" || e.key === " ")) {
    e.preventDefault();
    select(tile.dataset.sel);
    return;
  }

  const typing = /^(INPUT|TEXTAREA)$/.test(document.activeElement?.tagName ?? "");
  if (e.key === "/" && !typing) {
    e.preventDefault();
    $("#q").focus();
    $("#q").select();
  } else if (e.key === "Escape") {
    if (typing) {
      $("#q").value = "";
      state.q = "";
      $("#q").blur();
    } else if (state.sel) {
      state.sel = null;
      renderMap();
      renderInspector();
      return;
    }
    renderMap();
  } else if ((e.key === "r" || e.key === "R") && !typing) {
    refresh();
  }
});

$("#q").addEventListener("input", (e) => {
  state.q = e.target.value.trim().toLowerCase();
  renderMap(); // 只重画图，不动详情面板：输入的焦点在 input 上，本来就不会被抽走
});

$("#refresh").addEventListener("click", () => refresh());
$("#ping-all").addEventListener("click", () => pingAll());

// 窗口一变，格子就重排：图按容器宽度 1:1 画，不做缩放（缩了字就变成 6px）。
let lastW = 0;
new ResizeObserver(() => {
  const w = Math.max(300, wrapEl.clientWidth || 0);
  if (Math.abs(w - lastW) < 6) return;
  lastW = w;
  renderMap();
}).observe(wrapEl);

// 页面看不见就别轮询：块闲着 10 分钟会被宿主收掉，回来时再起也行。
let timer = 0;
const startTimer = () => {
  clearInterval(timer);
  timer = setInterval(() => {
    if (document.visibilityState === "visible") refresh();
  }, 5000);
};
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") refresh();
});

setInterval(tickStamp, 1000);

refresh().then(() => autoPing());
loadRegistry();
startTimer();

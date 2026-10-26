/* 左栏「移动」的十个方案 —— 选题板，不是产品。
 *
 * 十个格子、一棵树、一道题：**把 App.tsx 挪到 docs/ADR 前面**。
 * 每格的左栏都是真的：children 里摘出来、再插进别处，id 一个字符都不变。
 * 不引模型、不落盘、没有撤销栈 —— 这一页只比「怎么动」，不比「动了什么」。
 *
 * 这一页只挪**块**。挪组会把验收题搅乱（组一动，ADR 的路径就变了），十个方案没一个需要它。
 */

import "../../src/tokens/tokens.css";

/* ── 那棵树 ──────────────────────────────────────────────────────────────────
   十个格子共用这一份种子。每格 seed() 一次，各挪各的，互不影响。
   id 是短号 —— demo 里不需要六位乱码，但「移动只换位置、id 一字不变」这条规矩照旧。 */
const seed = () => ({
  id: "n1",
  name: "N",
  children: [
    {
      id: "n2",
      name: "src",
      children: [
        { id: "n3", name: "App.tsx" },
        { id: "n4", name: "utils.ts" },
        { id: "n5", name: "tokens", children: [{ id: "n6", name: "theme.ts" }] },
      ],
    },
    {
      id: "n7",
      name: "docs",
      children: [{ id: "n8", name: "ADR", children: [] }, { id: "n9", name: "notes.md" }],
    },
    { id: "n10", name: "server.ts" },
  ],
});

const esc = (s) =>
  String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

const isGroup = (n) => Array.isArray(n.children);

/** 前序展开：一行一个节点，带爹和它在爹里的下标。 */
const entries = (root) => {
  const out = [];
  const walk = (node, depth, parent, index) => {
    out.push({ node, depth, parent, index });
    if (isGroup(node)) node.children.forEach((child, i) => walk(child, depth + 1, node, i));
  };
  walk(root, 0, null, 0);
  return out;
};

const entryOf = (root, id) => entries(root).find((e) => e.node.id === id) ?? null;

/** 子树里有没有这个 id（含自己）。拿来挡住「把爹挪进儿子肚子里」。 */
const holds = (node, id) => node.id === id || (isGroup(node) && node.children.some((c) => holds(c, id)));

/** 名字路径：`src/App.tsx`。和真模型一样，路径是算出来的，不存。 */
const treePath = (root, id) => {
  const go = (node, trail) => {
    if (node.id === id) return trail;
    if (!isGroup(node)) return null;
    for (const child of node.children) {
      const hit = go(child, [...trail, child.name]);
      if (hit) return hit;
    }
    return null;
  };
  const path = go(root, []);
  return path ? path.join("/") || root.name : id;
};

/* 落点就两种：插到某个节点前面，或者放进某个组末尾。 */
const beforePos = (root, id) => {
  const e = entryOf(root, id);
  return e?.parent ? { parentId: e.parent.id, index: e.index } : null;
};
const afterPos = (root, id) => {
  const e = entryOf(root, id);
  return e?.parent ? { parentId: e.parent.id, index: e.index + 1 } : null;
};
const intoPos = (parentId) => ({ parentId, index: Number.MAX_SAFE_INTEGER });

/** 摘出来、插进去。同爹时先修下标，不然「往后挪一位」会差一格。 */
const applyMove = (root, id, parentId, index) => {
  const src = entryOf(root, id);
  const dest = entryOf(root, parentId)?.node;
  if (!src?.parent || !dest || !isGroup(dest)) return false;
  if (holds(src.node, dest.id)) return false;

  const sameParent = src.parent === dest;
  let to = Number.isFinite(index) ? index : dest.children.length;
  if (sameParent && to > src.index) to -= 1;

  src.parent.children.splice(src.index, 1);
  dest.children.splice(Math.max(0, Math.min(to, dest.children.length)), 0, src.node);
  return true;
};

/** 「落到 docs 第 1 位」—— 落完自己报数，人能对着左栏数。 */
const landing = (root, id) => {
  const e = entryOf(root, id);
  if (!e?.parent) return treePath(root, id);
  return `${treePath(root, e.parent.id)} 第 ${e.index + 1} 位`;
};

/** 路径补全用的模糊命中：`dadr` 打中 `docs/ADR`。命中越靠前、越连续分越高。 */
const fuzzy = (text, query) => {
  const q = query.toLowerCase().replace(/\s+/g, "");
  if (!q) return 1;
  const t = text.toLowerCase();
  let qi = 0;
  let score = 0;
  let run = 0;
  for (let i = 0; i < t.length && qi < q.length; i++) {
    if (t[i] === q[qi]) {
      qi += 1;
      run += 1;
      score += 2 + run + (i === 0 || t[i - 1] === "/" ? 3 : 0);
    } else run = 0;
  }
  return qi === q.length ? score : 0;
};

/* ── 卡片引擎 ───────────────────────────────────────────────────────────────
   十个方案共用：一棵树、一套落点规则、一条 Esc、一条状态行。
   每个方案只声明「怎么拎起来」和「怎么算落点」，其余全在这里。 */

const CARDS = [];

/* 默认落点规则：组=放进去（三段的中间段也是它），块=插到前面。
   zone 只在 03 里真有三种: top / mid / bottom；其余传 "row"。 */
const defaultResolve = (c, id, zone) => {
  const src = c.dragging ?? c.pick;
  const node = c.nodeOf(id);
  if (!src || !node || node.id === src) return null;
  if (holds(c.nodeOf(src), id)) return null;
  // 三段里，块行的中段是死区：一块没有「里面」可以放
  if (zone === "mid" && !isGroup(node)) return null;
  if (isGroup(node) && (zone === "mid" || zone === "row")) return intoPos(id);
  return zone === "bottom" ? afterPos(c.tree, id) : beforePos(c.tree, id);
};

class Card {
  constructor(cfg) {
    this.cfg = cfg;
    this.tree = seed();
    this.pick = null; // 拎起来的块
    this.dragging = null; // 正在被指针拖的块
    this.fresh = null; // 刚落子那行，闪一下
    this.note = "";
    this.bad = false;
    this.sel = null; // 06 / 08：选中的源
    this.query = "";
    this.cursor = 0; // 07：面板光标
    this.step = 1; // 07：1 选源 / 2 选目标
    this.palette = false; // 07
    this.cursorId = null; // 09：方向键游标
    this.armedId = null; // 09：落点
    this.armedMode = "before";
    this.pathDraft = null; // 08

    // 11–20 自己的状态。都挂在卡片上，互不打扰。
    this.menu = null; // 11 浮起的两层菜单
    this.menuAt = null; // 11
    this.lift = null; // 11 跟着手指的替身
    this.swallowUntil = 0; // 11 松手后吞掉那一下 click
    this.ring = null; // 12 { src, path }
    this.link = null; // 13 { src }
    this.wireMove = null; // 13
    this.snip = null; // 14 剪贴板
    this.wheel = null; // 15 { src, idx }
    this.nl = ""; // 16 那句话
    this.drill = null; // 17 { src, path }
    this.cf = null; // 18 { src, idx }
    this.scrub = null; // 19 { id, origin }
    this.ed = ""; // 20 输入
    this.edSrc = null; // 20 第一拍的行号

    const el = document.createElement("section");
    el.className = `card ${cfg.id}`;
    el.id = cfg.id;
    el.dataset.id = cfg.id;
    el.innerHTML = `
      <header class="card-head">
        <span class="num">${cfg.id.slice(1).padStart(2, "0")}</span>
        <div class="head">
          <h2>${esc(cfg.name)}</h2>
          <p>${esc(cfg.blurb)}</p>
          <div class="specs">
            ${cfg.specs.map(([k, v]) => `<span>${k} <b>${esc(v)}</b></span>`).join("")}
          </div>
        </div>
      </header>
      <div class="plate">
        <div class="bar" data-role="bar"></div>
        <div class="panel">
          <div class="panel-head"><span>结构</span><span>id</span></div>
          <div class="body">
            <div class="list" role="tree" tabindex="-1" aria-label="${esc(cfg.name)}的左栏"></div>
            <div class="aux"></div>
          </div>
        </div>
        <p class="status" aria-live="polite"></p>
      </div>`;

    this.el = el;
    this.bar = el.querySelector(".bar");
    this.body = el.querySelector(".body");
    this.listEl = el.querySelector(".list");
    this.aux = el.querySelector(".aux");
    this.statusEl = el.querySelector(".status");

    el.addEventListener("click", (ev) => this.onClick(ev));
    el.addEventListener("keydown", (ev) => this.onKeyDown(ev));
    el.addEventListener("input", (ev) => this.onInput(ev));
    el.addEventListener("change", (ev) => this.onChange(ev));
    this.listEl.addEventListener("focusin", (ev) => {
      if (!this.cfg.followFocus) return;
      const row = ev.target.closest?.(".row");
      if (!row) return;
      this.cursorId = row.dataset.id;
      this.listEl.querySelectorAll(".row.cur").forEach((r) => r.classList.remove("cur"));
      row.classList.add("cur");
    });

    el.addEventListener("wheel", (ev) => this.cfg.onWheel?.(this, ev), { passive: false });

    if (cfg.drag) installDrag(this);
    CARDS.push(this);
    this.render();
  }

  /* ── 读树 ── */
  nodeOf(id) {
    return entryOf(this.tree, id)?.node ?? null;
  }
  nameOf(id) {
    return this.nodeOf(id)?.name ?? id;
  }
  rowEl(id) {
    return this.listEl.querySelector(`.row[data-id="${id}"]`);
  }

  /* ── 画 ── */
  render() {
    const rows = this.cfg.rows ? this.cfg.rows(this) : entries(this.tree);
    this.listEl.innerHTML = rows.map((e, i) => this.rowHTML(e, i)).join("");
    this.bar.innerHTML = this.cfg.bar
      ? this.cfg.bar(this)
      : this.pick
        ? capsule(this)
        : "";
    this.aux.innerHTML = this.cfg.aux ? this.cfg.aux(this) : "";
    this.body.classList.toggle("show-aux", !!this.cfg.auxOn?.(this));
    this.el.classList.toggle("picking", !!this.pick);
    this.statusEl.textContent = this.note || this.cfg.hint?.(this) || "";
    this.statusEl.classList.toggle("bad", !!this.bad);
    this.cfg.paint?.(this);
    this.cfg.afterRender?.(this);
  }

  rowHTML(e, i = 0) {
    const n = e.node;
    const g = isGroup(n);
    const cls = ["row", g ? "group" : "leaf"];
    if (this.pick === n.id || this.dragging === n.id) cls.push("src");
    if (this.pick && this.pick !== n.id) cls.push("drop");
    if (this.fresh === n.id) cls.push("fresh");
    if (this.sel === n.id) cls.push("sel");
    if (this.cursorId === n.id) cls.push("cur");
    if (this.armedId === n.id) cls.push("armed", this.armedMode === "into" ? "into" : "before");
    const hint = this.pick && this.pick !== n.id ? this.cfg.dropHint?.(this, e) : null;

    return `<div class="${cls.join(" ")}" role="treeitem" aria-level="${e.depth + 1}"
      data-id="${n.id}" tabindex="0" style="--d:${e.depth}">
      ${this.cfg.numbering ? `<span class="idx">${i + 1}</span>` : ""}
      <span class="glyph" aria-hidden="true"></span>
      ${this.cfg.pre ? this.cfg.pre(this, e) : ""}
      <span class="nid">${n.id}</span>
      <span class="nm">${esc(n.name)}</span>
      ${hint ? `<span class="hint">${esc(hint)}</span>` : ""}
      ${this.cfg.tail ? this.cfg.tail(this, e) : ""}
    </div>`;
  }

  say(text, bad = false) {
    this.note = text;
    this.bad = bad;
    this.statusEl.textContent = text;
    this.statusEl.classList.toggle("bad", bad);
  }

  /* ── 事件 ── */
  onClick(ev) {
    const act = ev.target.closest?.("[data-act]");
    if (act && this.el.contains(act)) {
      const kind = act.dataset.act;
      if (this.cfg.act?.(this, kind, act.dataset.id, ev, act) === true) return;
      if (kind === "pick" && act.dataset.id) return this.beginPick(act.dataset.id);
      if (kind === "cancel") return this.cancel();
      if (kind === "cancel-sel") return this.clearSel();
      return;
    }

    const target = ev.target.closest?.("[data-target]");
    if (target && this.el.contains(target)) {
      return this.apply(target.dataset.target, target.dataset.mode || "before");
    }

    const row = ev.target.closest?.(".row");
    if (row && this.listEl.contains(row)) return this.onRow(row.dataset.id, ev);

    this.cfg.onClick?.(this, ev);
  }

  onKeyDown(ev) {
    if (this.cfg.onKey?.(this, ev) === true) return;
    const row = ev.target.closest?.(".row");
    if (row && this.listEl.contains(row) && (ev.key === "Enter" || ev.key === " ")) {
      ev.preventDefault();
      this.onRow(row.dataset.id, ev);
    }
  }

  onInput(ev) {
    const kind = ev.target.dataset?.act;
    if (kind === "query" || kind === "palette-query") {
      this.query = ev.target.value;
      this.cursor = 0;
      this.cfg.paint?.(this); // 只重画列表 —— 输入框和光标都不动
      return;
    }
    if (kind === "path") this.pathDraft = ev.target.value;
    this.cfg.onInput?.(this, ev);
  }

  onChange(ev) {
    const kind = ev.target.dataset?.act;
    if (kind === "dad") {
      const id = ev.target.dataset.id;
      this.cfg.act?.(this, "dad", id, ev, ev.target);
    }
  }

  onRow(id, ev) {
    if (this.pick) return this.drop(id);
    this.cfg.onRow?.(this, id, ev);
  }

  /* ── 落子 ── */
  beginPick(id) {
    const node = this.nodeOf(id);
    if (!node || isGroup(node)) return; // 这一页只挪块
    if (this.pick === id) return this.cancel();
    this.pick = id;
    this.fresh = null;
    this.armedId = null;
    this.say("");
    this.render();
    this.rowEl(id)?.focus();
  }

  cancel() {
    this.pick = null;
    this.armedId = null;
    this.say("已取消 —— 树没动，位置还是原来的");
    this.render();
  }

  clearSel() {
    this.sel = null;
    this.query = "";
    this.pathDraft = null;
    this.say("");
    this.render();
  }

  escape() {
    if (this.cfg.onEscape?.(this)) return true;
    if (this.pick) {
      this.cancel();
      return true;
    }
    if (this.sel) {
      this.clearSel();
      return true;
    }
    return false;
  }

  /** 拎起来之后点/敲某一行。 */
  drop(targetId) {
    const pos = (this.cfg.resolve ?? defaultResolve)(this, targetId, "row");
    this.applyPos(pos);
  }

  applyPos(pos, srcId = this.pick) {
    if (!srcId || !pos) return this.say("这个落点不行", true);
    if (!applyMove(this.tree, srcId, pos.parentId, pos.index)) return this.say("这个落点不行", true);
    this.landed(srcId);
  }

  /** 显式落点（两栏、命令面板、下拉用）：mode = "into" | "before"。 */
  apply(id, mode) {
    const src = this.pick ?? this.sel;
    if (!src) return;
    const pos = mode === "into" ? intoPos(id) : beforePos(this.tree, id);
    if (!pos) return this.say("这个落点不行 —— 根上没有「前面」", true);
    this.sel = null;
    this.applyPos(pos, src);
  }

  landed(id) {
    this.pick = null;
    this.dragging = null;
    this.armedId = null;
    this.fresh = id;
    const extra = this.cfg.afterDrop?.(this, id) ?? "";
    this.say(`${this.nameOf(id)} → ${landing(this.tree, id)} · id 还是 ${id}${extra ? ` ${extra}` : ""}`);
    this.render();
    this.rowEl(id)?.focus();
  }
}

const capsule = (c) => `
  <div class="capsule">
    <span class="pulse" aria-hidden="true"></span>
    <span class="capsule-txt">正在移动 <b>${esc(c.nameOf(c.pick))}</b>${
      c.cfg.pickTip ? `<span class="tip">${esc(c.cfg.pickTip(c))}</span>` : ""
    }</span>
    <button class="linkbtn" data-act="cancel">取消 <kbd>Esc</kbd></button>
  </div>`;

/* ── 拖拽：03 / 04 / 05 共用一台机器 ────────────────────────────────────────
   three  = 上中下三段（插入线 + 进组高亮）
   one    = 整行一个落点（块=插前面，组=进组）
   ghost  = 幽灵行 + 空槽，松手落光标那一行，其余什么都不提示 —— 那就是它要证明的代价 */
function installDrag(card) {
  const mode = card.cfg.drag;
  let drag = null;

  const clear = () => {
    if (!drag) return;
    drag.row.classList.remove("dragging", "slot");
    drag.line?.remove();
    drag.tag?.remove();
    drag.ghost?.remove();
    card.listEl.querySelectorAll(".hit, .into").forEach((r) => r.classList.remove("hit", "into"));
    document.body.classList.remove("is-dragging", "nope");
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    window.removeEventListener("pointercancel", onCancel);
    window.removeEventListener("keydown", onEsc, true);
    drag = null;
    card.dragging = null;
  };

  const onMove = (ev) => {
    if (!drag) return;
    const { clientX: x, clientY: y } = ev;
    const follow = drag.tag || drag.ghost;
    if (follow) follow.style.transform = `translate3d(${x + 12}px, ${y + 10}px, 0)`;

    const under = document.elementFromPoint(x, y)?.closest?.(".row");
    const row = under && card.listEl.contains(under) ? under : null;
    card.listEl.querySelectorAll(".hit, .into").forEach((r) => r.classList.remove("hit", "into"));
    drag.line?.classList.add("dead");
    drag.pos = null;

    if (row) {
      const id = row.dataset.id;
      const box = row.getBoundingClientRect();
      const t = (y - box.top) / box.height;
      const zone = mode === "three" ? (t < 1 / 3 ? "top" : t > 2 / 3 ? "bottom" : "mid") : "row";
      const pos = (card.cfg.resolve ?? defaultResolve)(card, id, zone);
      if (pos) {
        drag.pos = pos;
        if (mode === "three" && zone !== "mid") {
          drag.line.classList.remove("dead");
          const y2 = zone === "top" ? row.offsetTop : row.offsetTop + row.offsetHeight;
          drag.line.style.transform = `translate3d(0, ${y2 - 1}px, 0)`;
        } else if (mode === "three") {
          row.classList.add("into");
        }
        // 05 故意什么都不加：那一格要证明的就是「没有反馈时你敢不敢松手」
        if (mode === "one") row.classList.add("hit");
      }
    }
    document.body.classList.toggle("nope", !drag.pos);
  };

  const onUp = () => {
    if (!drag) return;
    const { id, pos } = drag;
    clear();
    if (!pos) return; // 没落在有效位置 = 什么都没发生
    if (applyMove(card.tree, id, pos.parentId, pos.index)) card.landed(id);
  };

  const onCancel = () => {
    if (!drag) return;
    clear();
    card.say("已取消 —— 树没动，位置还是原来的");
  };

  const onEsc = (ev) => {
    if (ev.key !== "Escape" || !drag) return;
    ev.stopPropagation();
    onCancel();
  };

  card.listEl.addEventListener("pointerdown", (ev) => {
    if (ev.pointerType === "touch" || ev.button !== 0) return;
    if (drag || card.pick) return;
    if (ev.target.closest?.("button, select, input, textarea, a")) return;
    const row = ev.target.closest?.(".row");
    if (!row || !card.listEl.contains(row)) return;
    const id = row.dataset.id;
    if (!id || isGroup(card.nodeOf(id))) return;

    ev.preventDefault();
    const box = row.getBoundingClientRect();
    drag = { id, row, pos: null };
    card.dragging = id;
    row.classList.add("dragging");
    document.body.classList.add("is-dragging");

    if (mode === "ghost") {
      const ghost = row.cloneNode(true);
      ghost.classList.add("ghost");
      ghost.style.width = `${box.width}px`;
      document.body.appendChild(ghost);
      drag.ghost = ghost;
      row.classList.add("slot");
    } else {
      const tag = document.createElement("div");
      tag.className = "dragtag";
      tag.innerHTML = `<span>${esc(card.nameOf(id))}</span><b>${id}</b>`;
      document.body.appendChild(tag);
      drag.tag = tag;
    }

    if (mode === "three") {
      const line = document.createElement("div");
      line.className = "line dead";
      card.listEl.appendChild(line);
      drag.line = line;
    }

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
    window.addEventListener("keydown", onEsc, true);
    onMove(ev);
  });
}

/* ── 07 的落点清单：每个节点都给「插到它前面」，组再多给一条「放进末尾」 ── */
const targetRows = (root, excludeId) => {
  const out = [];
  for (const e of entries(root)) {
    if (e.node.id === excludeId) continue;
    if (e.depth > 0) {
      out.push({
        id: e.node.id,
        mode: "before",
        label: `插到 ${treePath(root, e.node.id)} 前面`,
      });
    }
    if (isGroup(e.node) && e.depth > 0) {
      out.push({ id: e.node.id, mode: "into", label: `放进 ${treePath(root, e.node.id)} 末尾` });
    }
  }
  return out;
};

/* ── 08 的地址解析：`docs/ADR` = 进 docs、插到 ADR 前面 ───────────────────── */
const resolvePath = (root, text) => {
  const raw = text.trim();
  if (!raw) return null;
  const trailing = raw.endsWith("/");
  const segs = raw.split("/").map((s) => s.trim()).filter(Boolean);
  if (!segs.length) return null;

  let node = root;
  for (const seg of trailing ? segs : segs.slice(0, -1)) {
    if (!isGroup(node)) return null;
    const next = node.children.find((c) => c.name === seg);
    if (!next) return null;
    node = next;
  }
  if (!isGroup(node)) return null;

  // 结尾带斜杠 = 落到这个组末尾；不带 = 插到同名兄弟前面
  if (trailing) return { parentId: node.id, index: node.children.length };
  const at = node.children.findIndex((c) => c.name === segs[segs.length - 1]);
  return at < 0 ? null : { parentId: node.id, index: at };
};

const applyPath = (c) => {
  const box = c.el.querySelector('[data-act="path"]');
  const text = (box?.value ?? "").trim();
  const pos = resolvePath(c.tree, text);
  if (!pos) return c.say(`没有这条路径：${text || "(空)"}`, true);
  const src = c.sel;
  c.sel = null;
  c.pathDraft = null;
  c.applyPos(pos, src);
};

/* ── 11–20 的共用零件 ────────────────────────────────────────────────────────
   第一排（01–10）比的是「怎么落子」：点、拖、打字，都还在左栏这张表里。
   这一排换的是**手**：按住、画线、转环、翻卡片、滚着走 —— 有四格干脆不在树上落子了，
   树只负责被改完之后长成什么样。题还是那道题：把 App.tsx 挪到 docs/ADR 前面。 */

/** 落点 → 位置。每个节点前面一次，每个组末尾一次。 */
const posOf = (root, slot) => (slot.mode === "into" ? intoPos(slot.id) : beforePos(root, slot.id));

const relLabel = (root, id, rel) =>
  rel === "into"
    ? `放进 ${treePath(root, id)} 末尾`
    : rel === "after"
      ? `插到 ${treePath(root, id)} 后面`
      : `插到 ${treePath(root, id)} 前面`;

/** 前序里的「下一行 / 上一行」—— 19 的滚一格走一行。
    往下一行是组的时候进到它第一格，不然块永远进不了组。 */
const stepPos = (root, id, dir) => {
  const all = entries(root);
  const at = all.findIndex((e) => e.node.id === id);
  if (at < 0) return null;
  if (dir < 0) {
    const prev = all[at - 1];
    return prev ? beforePos(root, prev.node.id) : null;
  }
  const next = all[at + 1];
  if (!next) return null;
  const first = isGroup(next.node) ? next.node.children[0] : null;
  if (first) return beforePos(root, first.id);
  return afterPos(root, next.node.id);
};

/** 16 的关系词。顺序有讲究：先说「末尾」再说「里」，不然「放到 docs 里」会被前面吃掉。 */
const relOf = (s) =>
  /末尾|最后|尽头/.test(s)
    ? "into"
    : /前面|之前|上头|顶上/.test(s)
      ? "before"
      : /后面|之后|下头|底下/.test(s)
        ? "after"
        : /里面|里头|里边|放进|塞进|挪进|移到.*里/.test(s)
          ? "into"
          : null;

/** 16 的中文解析：句子里认这棵树上的名字，别的全当噪音。
    名字和全路径都认（「ADR」和「docs/ADR」），长的先匹配。 */
const parseNL = (root, text) => {
  const raw = String(text ?? "").trim();
  if (!raw) return { why: "先说一句话" };

  const hits = [];
  for (const e of entries(root)) {
    if (e.depth === 0) continue;
    const path = treePath(root, e.node.id);
    for (const needle of path === e.node.name ? [e.node.name] : [path, e.node.name]) {
      const at = raw.indexOf(needle);
      if (at >= 0) hits.push({ e, needle, at, end: at + needle.length });
    }
  }
  if (!hits.length) return { why: "这句话里没有一个名字是这棵树上的" };
  // 同一位置长的优先（docs/ADR 盖掉 ADR）
  hits.sort((a, b) => a.at - b.at || b.end - a.end);

  const srcHit = hits.find((h) => h.at >= 0 && !isGroup(h.e.node));
  if (!srcHit) return { why: "只找到组 —— 这一页只挪块" };

  const tgtHit = hits.find((h) => h.at >= srcHit.end && h.e.node.id !== srcHit.e.node.id);
  if (!tgtHit) return { why: "只找到要挪的，没找到落到哪儿" };

  const rel = relOf(raw.slice(tgtHit.at)) ?? "before";
  const tgtId = tgtHit.e.node.id;
  if (rel === "into" && !isGroup(tgtHit.e.node)) {
    return { why: `${treePath(root, tgtId)} 不是组，进不去` };
  }
  if (rel !== "into" && !tgtHit.e.parent) return { why: "根上没有前面后面" };

  return {
    src: srcHit.e.node.id,
    targetId: tgtId,
    rel,
    pos: rel === "into" ? intoPos(tgtId) : rel === "after" ? afterPos(root, tgtId) : beforePos(root, tgtId),
    says: relLabel(root, tgtId, rel),
  };
};

/** 20 的 ed 式指令：`3>8` / `3 8` / `3m8` —— 四个键。 */
const parseEd = (root, text) => {
  const m = String(text ?? "").trim().match(/^(\d{1,2})\s*(?:[>,]|m|\s)\s*(\d{1,2})$/);
  if (!m) return { why: "写成 3>8：左边是它现在第几行，右边是挪到第几行前面" };
  const all = entries(root);
  const a = Number(m[1]) - 1;
  const b = Number(m[2]) - 1;
  if (!all[a]) return { why: `没有第 ${m[1]} 行` };
  if (!all[b]) return { why: `没有第 ${m[2]} 行` };
  if (isGroup(all[a].node)) return { why: `第 ${m[1]} 行是组 —— 这一页只挪块` };
  if (a === b) return { why: "两头是同一行" };
  const pos = beforePos(root, all[b].node.id);
  if (!pos) return { why: "根上没有前面" };
  return { src: all[a].node.id, srcIdx: a + 1, tgtId: all[b].node.id, tgtIdx: b + 1, pos };
};

/* ── 11 长按浮起：两层菜单跟着手指走 ─────────────────────────────────────── */
const ctxItems = (c, level = c.menu?.level ?? 0) => {
  if (!c.menu) return [];
  if (level === 0) {
    return entries(c.tree)
      .filter((e) => isGroup(e.node))
      .map((e) => ({ kind: "into", id: e.node.id, label: `放进 ${treePath(c.tree, e.node.id)} 末尾` }))
      .concat([{ kind: "sub", label: "插到某一行前面 ▸" }]);
  }
  return [{ kind: "back", label: "← 返回" }].concat(
    entries(c.tree)
      .filter((e) => e.depth > 0 && e.node.id !== c.menu.src)
      .map((e) => ({ kind: "before", id: e.node.id, label: `插到 ${treePath(c.tree, e.node.id)} 前面` })),
  );
};

const ctxHTML = (c) => {
  const at = c.menuAt ?? { x: 16, y: 16 };
  const left = Math.max(4, Math.min(at.x, Math.max(4, c.body.clientWidth - 210)));
  const top = Math.max(4, Math.min(at.y, 46));
  const panel = (lvl) => `
    <div class="ctx-panel">
      <p class="ctx-cap">${lvl === 0 ? "放进哪个组末尾" : "插到哪一行前面"}</p>
      ${ctxItems(c, lvl)
        .map(
          (it, i) =>
            `<button class="mitem ${it.kind === "sub" || it.kind === "back" ? "sub" : ""}"
               role="menuitem" data-act="mitem" data-lvl="${lvl}" data-i="${i}">${esc(it.label)}</button>`,
        )
        .join("")}
    </div>`;
  return `
    <div class="ctx-scrim" data-act="ctx-close"></div>
    <div class="ctx" role="menu" aria-label="移到哪里" style="left:${left}px; top:${top}px">
      <div class="ctx-path">${esc(treePath(c.tree, c.menu.src))}</div>
      <div class="ctx-slide" style="--lvl:${c.menu.level}">${panel(0)}${panel(1)}</div>
    </div>`;
};

const closeCtx = (c, msg = "已取消 —— 树没动") => {
  c.menu = null;
  c.lift?.remove();
  c.lift = null;
  c.say(msg);
  c.render();
};

const ctxGo = (c, lvl, i) => {
  const item = ctxItems(c, lvl)[i];
  if (!item || !c.menu) return;
  if (item.kind === "back") {
    c.menu.level = 0;
    c.menu.hover = -1;
    c.say("回到第一层");
    c.render();
    c.el.querySelector(".mitem")?.focus();
    return;
  }
  if (item.kind === "sub") {
    c.menu.level = 1;
    c.menu.hover = -1;
    c.say("第二层：插到哪一行前面");
    c.render();
    c.el.querySelector(".mitem")?.focus();
    return;
  }
  const src = c.menu.src;
  const pos = posOf(c.tree, item);
  c.menu = null;
  c.lift?.remove();
  c.lift = null;
  c.applyPos(pos, src);
};

function openCtx(c, id, at) {
  if (c.swallowUntil && performance.now() < c.swallowUntil) return;
  const node = c.nodeOf(id);
  if (!node || isGroup(node)) return c.say("组不能挪 —— 这一页只挪块", true);
  if (c.menu?.src === id) return;

  const box = c.body.getBoundingClientRect();
  const rowBox = c.rowEl(id)?.getBoundingClientRect();
  c.menu = { src: id, level: 0, hover: -1 };
  c.menuAt = at
    ? { x: at.x - box.left, y: at.y - box.top }
    : rowBox
      ? { x: rowBox.left - box.left + 24, y: rowBox.top - box.top + 26 }
      : { x: 16, y: 16 };
  c.say(`浮着 ${c.nameOf(id)} —— 选一项，Esc 放回去`);
  c.render();

  if (!at) {
    c.el.querySelector(".mitem")?.focus();
    return;
  }

  c.lift?.remove();
  const lift = document.createElement("div");
  lift.className = "lift";
  lift.innerHTML = `<span>${esc(c.nameOf(id))}</span><b>${id}</b>`;
  document.body.appendChild(lift);
  c.lift = lift;
  const place = (x, y) => {
    lift.style.transform = `translate3d(${x + 14}px, ${y - 36}px, 0)`;
  };
  place(at.x, at.y);

  const move = (ev) => {
    place(ev.clientX, ev.clientY);
    const el = document.elementFromPoint(ev.clientX, ev.clientY)?.closest?.(".mitem");
    const items = [...c.el.querySelectorAll(".mitem")];
    const i = el && c.el.contains(el) ? items.indexOf(el) : -1;
    if (i !== (c.menu?.hover ?? -1)) {
      if (c.menu) c.menu.hover = i;
      items.forEach((n, k) => n.classList.toggle("hot", k === i));
    }
  };
  const up = (ev) => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    const el = document.elementFromPoint(ev.clientX, ev.clientY)?.closest?.(".mitem");
    if (el && c.el.contains(el)) {
      c.swallowUntil = performance.now() + 260;
      ctxGo(c, Number(el.dataset.lvl), Number(el.dataset.i));
    } else closeCtx(c, "松手落在空地上 —— 树没动");
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
}

function installHold(c) {
  if (c.holdReady) return;
  c.holdReady = true;
  let timer = null;
  const stop = () => {
    clearTimeout(timer);
    timer = null;
  };
  c.listEl.addEventListener("pointerdown", (ev) => {
    if (ev.button !== 0 || c.menu) return;
    if (ev.target.closest?.("button, input, select, a")) return;
    const row = ev.target.closest?.(".row");
    if (!row || !c.listEl.contains(row)) return;
    const id = row.dataset.id;
    if (isGroup(c.nodeOf(id))) return;
    stop();
    const { clientX, clientY } = ev;
    timer = setTimeout(() => {
      timer = null;
      openCtx(c, id, { x: clientX, y: clientY });
    }, 260);
  });
  ["pointerup", "pointercancel"].forEach((k) => c.el.addEventListener(k, stop));
}

/* ── 12 径向环：一圈按钮，靠角度分落点 ───────────────────────────────────── */
const ringItems = (c) => {
  const path = c.ring?.path ?? [];
  const cur = c.nodeOf(path[path.length - 1]);
  if (!cur) return [];
  const out = cur.children.map((n) => ({
    kind: "node",
    id: n.id,
    group: isGroup(n),
    label: n.name,
  }));
  out.push({ kind: "into", id: cur.id, label: "放进这层末尾" });
  if (path.length > 1) out.push({ kind: "up", id: cur.id, label: "← 上一层" });
  return out;
};

const ringHTML = (c) => {
  const items = ringItems(c);
  const cur = c.nodeOf(c.ring.path[c.ring.path.length - 1]);
  const n = items.length || 1;
  const wedges = items
    .map((it, i) => {
      const a = (i / n) * Math.PI * 2 - Math.PI / 2;
      const style = `left:calc(50% + ${Math.round(132 * Math.cos(a))}px);top:calc(50% + ${Math.round(88 * Math.sin(a))}px)`;
      if (it.kind === "node") {
        return `<div class="wedge ${it.group ? "is-g" : ""}" style="${style}">
          <button class="wl" data-act="ring-go" data-id="${it.id}"
            aria-label="插到 ${esc(it.label)} 前面"><span class="wn">${esc(it.label)}</span></button>
          ${it.group ? `<button class="wgo" data-act="ring-in" data-id="${it.id}" aria-label="进入 ${esc(it.label)}">进 ▸</button>` : ""}
        </div>`;
      }
      const act = it.kind === "up" ? "ring-up" : "ring-into";
      return `<button class="wedge spec ${it.kind}" style="${style}" data-act="${act}" data-id="${it.id}">${esc(it.label)}</button>`;
    })
    .join("");

  return `
    <div class="ring">
      <div class="ring-core">
        <b>${esc(treePath(c.tree, cur.id))}</b>
        <span>点名字 = 插到它前面</span>
      </div>
      ${wedges}
      <button class="linkbtn ring-x" data-act="ring-close">Esc</button>
    </div>`;
};

/* ── 13 连线：一根橡皮筋，行不动 ──────────────────────────────────────────── */
const drawWire = (c, cx, cy) => {
  const path = c.body.querySelector(".wires .wire");
  const src = c.link && c.listEl.querySelector(`.port[data-id="${c.link.src}"]`);
  if (!path || !src) return;
  const box = c.body.getBoundingClientRect();
  const s = src.getBoundingClientRect();
  const x1 = s.left + s.width / 2 - box.left;
  const y1 = s.top + s.height / 2 - box.top;
  const x2 = cx == null ? x1 + 90 : cx - box.left;
  const y2 = cy == null ? y1 + 46 : cy - box.top;
  const mx = (x1 + x2) / 2;
  path.setAttribute("d", `M ${x1} ${y1} C ${mx} ${y1} ${mx} ${y2} ${x2} ${y2}`);
};

const startWire = (c, id) => {
  c.link = { src: id };
  c.el.classList.add("wiring");
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("class", "wires");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(ns, "path");
  path.setAttribute("class", "wire");
  svg.appendChild(path);
  c.body.appendChild(svg);
  c.wireMove = (ev) => drawWire(c, ev.clientX, ev.clientY);
  c.el.addEventListener("pointermove", c.wireMove);
  c.say("拉到另一颗圆点上 —— 左端点插前面，组右边的点进末尾");
  drawWire(c, null, null);
};

const endWire = (c, msg) => {
  c.link = null;
  c.el.classList.remove("wiring");
  if (c.wireMove) c.el.removeEventListener("pointermove", c.wireMove);
  c.wireMove = null;
  c.body.querySelector(".wires")?.remove();
  if (msg) c.say(msg);
};

/* ── 15 / 18 共用的候选清单 ───────────────────────────────────────────────── */
const slotsOf = (c) => targetRows(c.tree, c.wheel?.src ?? c.cf?.src ?? null);

const wheelHTML = (c) => {
  const items = slotsOf(c);
  const idx = c.wheel.idx;
  return `
    <div class="wheel-wrap">
      <div class="wheel" role="listbox" aria-label="目的地" tabindex="-1">
        ${items
          .map((it, i) => {
            const d = i - idx;
            const far = Math.abs(d) > 3;
            const style = [
              `transform: translateY(calc(-50% + ${d * 34}px)) rotateX(${-d * 24}deg) scale(${Math.max(0.72, 1 - Math.abs(d) * 0.06).toFixed(3)})`,
              `opacity:${Math.max(0, 1 - Math.abs(d) * 0.26).toFixed(3)}`,
              far ? "visibility:hidden" : "",
            ]
              .filter(Boolean)
              .join(";");
            return `<button class="witem ${d === 0 ? "on" : ""}" data-act="wheel-go" data-i="${i}"
              style="${style}" role="option" aria-selected="${d === 0}">${esc(it.label)}</button>`;
          })
          .join("")}
      </div>
      <div class="wheel-guard" aria-hidden="true"></div>
      <button class="linkbtn wheel-x" data-act="wheel-close">Esc</button>
    </div>`;
};

const wheelStep = (c, dir) => {
  if (!c.wheel) return;
  const n = slotsOf(c).length;
  const next = Math.max(0, Math.min(n - 1, c.wheel.idx + dir));
  if (next === c.wheel.idx) return c.say("到头了 —— 再滚也没有了", true);
  c.wheel.idx = next;
  c.render();
};

const wheelDrop = (c, i) => {
  const src = c.wheel?.src;
  const it = slotsOf(c)[i];
  if (!src || !it) return;
  const pos = posOf(c.tree, it);
  c.wheel = null;
  c.applyPos(pos, src);
};

const cfHTML = (c) => {
  const items = slotsOf(c);
  const idx = c.cf.idx;
  return `
    <div class="cf">
      <div class="cf-stage">
        ${items
          .map((it, i) => {
            const d = i - idx;
            if (Math.abs(d) > 2) return "";
            const style = `transform: translateX(${d * 118}px) rotateY(${-d * 36}deg) translateZ(${-Math.abs(d) * 70}px) scale(${1 - Math.abs(d) * 0.06});opacity:${(1 - Math.abs(d) * 0.34).toFixed(3)}`;
            return `<button class="cfcard ${d === 0 ? "on" : ""}" data-act="cf-go" data-i="${i}" style="${style}">
              <span class="cfpath">${esc(it.label)}</span>
              <span class="cfmeta">${it.mode === "into" ? "进组末尾" : "插到前面"} · ${it.id}</span>
            </button>`;
          })
          .join("")}
      </div>
      <div class="cf-bar">
        <span class="cf-count">${idx + 1} / ${items.length}</span>
        <button class="linkbtn" data-act="cf-go" data-i="${idx}">落这一张</button>
        <button class="linkbtn" data-act="cf-close">Esc</button>
      </div>
    </div>`;
};

const cfDrop = (c, i) => {
  const src = c.cf?.src;
  const it = slotsOf(c)[i];
  if (!src || !it) return;
  const pos = posOf(c.tree, it);
  c.cf = null;
  c.applyPos(pos, src);
};

/* ── 17 面包屑下潜 ───────────────────────────────────────────────────────── */
const drillHTML = (c) => {
  const path = c.drill.path;
  const cur = c.nodeOf(path[path.length - 1]);
  const crumbs = path
    .map(
      (id, i) =>
        `<button class="crumb ${i === path.length - 1 ? "on" : ""}" data-act="drill-up" data-i="${i}">${esc(c.nodeOf(id).name)}</button>`,
    )
    .join(`<span class="csep" aria-hidden="true">/</span>`);
  const tiles = cur.children
    .map(
      (n) => `<div class="tile ${isGroup(n) ? "is-g" : ""}">
        <button class="tmain" data-act="drill-go" data-id="${n.id}">
          <span class="tname">${esc(n.name)}</span>
          <span class="tcap">插到它前面</span>
        </button>
        ${
          isGroup(n)
            ? n.children.length
              ? `<button class="tgo" data-act="drill-in" data-id="${n.id}">进 ▸</button>`
              : `<span class="tgo is-empty">空组</span>`
            : ""
        }
      </div>`,
    )
    .join("");
  return `
    <div class="drill">
      <div class="drill-head">
        <div class="crumbs">${crumbs}</div>
        <button class="linkbtn" data-act="drill-close">Esc</button>
      </div>
      <button class="drop-here" data-act="drill-into">放进 ${esc(cur.name)} 末尾</button>
      <div class="tgrid">${tiles || `<p class="empty">这一层是空的 —— 只能放进末尾</p>`}</div>
    </div>`;
};

/* ── 16 说话 ─────────────────────────────────────────────────────────────── */
const nlGo = (c) => {
  const p = parseNL(c.tree, c.nl);
  if (p.why) return c.say(`没听懂：${p.why}`, true);
  c.nl = "";
  c.applyPos(p.pos, p.src);
};

/* ── 19 滚着挪 ───────────────────────────────────────────────────────────── */
const startScrub = (c, id) => {
  const node = c.nodeOf(id);
  if (!node || isGroup(node)) return c.say("组不能挪 —— 这一页只挪块", true);
  if (c.scrub?.id === id) return;
  if (c.scrub) {
    const prev = c.scrub.id;
    c.scrub = null;
    c.landed(prev);
  }
  const e = entryOf(c.tree, id);
  c.scrub = { id, origin: { parentId: e.parent.id, index: e.index } };
  c.fresh = null;
  c.say(`攥住 ${c.nameOf(id)} —— 滚轮或 ⌥↑↓ 走一行，Enter 定住，Esc 退回原处`);
  c.render();
  c.rowEl(id)?.focus();
};

const scrubStep = (c, dir) => {
  const s = c.scrub;
  if (!s) return;
  const pos = stepPos(c.tree, s.id, dir);
  if (!pos) return c.say("到头了 —— 再滚也走不动", true);
  if (!applyMove(c.tree, s.id, pos.parentId, pos.index)) return;
  const e = entryOf(c.tree, s.id);
  c.say(`${c.nameOf(s.id)} · ${treePath(c.tree, e.parent.id)} 第 ${e.index + 1} 位 · Enter 定住 / Esc 退回原处`);
  c.render();
  c.rowEl(s.id)?.focus();
};

/* ── 20 序号指令 ─────────────────────────────────────────────────────────── */
const edParse = (c) => parseEd(c.tree, c.ed);

const edGo = (c) => {
  const p = edParse(c);
  if (p.why) return c.say(p.why, true);
  c.ed = "";
  c.edSrc = null;
  c.applyPos(p.pos, p.src);
};

/* ── 二十个方案 ───────────────────────────────────────────────────────────────
   每格只写给「拎起来」和「算落点」有关的那几行，别的都在上面的引擎里。 */
const VARIANTS = [
  {
    id: "v1",
    name: "拎起来",
    blurb:
      "行尾一颗「移动」→ 该行变灰、顶部浮出胶囊、其余每一行都变成落点：点组就放进组末尾，点块就插到它前面。赌的是 —— 一个状态 + 一种点击，已经够用了。",
    specs: [
      ["动作", "2 次点击"],
      ["状态", "1 个"],
      ["排序", "能"],
    ],
    tail: (c, e) =>
      isGroup(e.node)
        ? ""
        : `<button class="mv" data-act="pick" data-id="${e.node.id}"
             aria-label="移动 ${esc(e.node.name)}">移动</button>`,
    pickTip: () => "组=放进末尾 · 块=插到前面",
    dropHint: (c, e) => (isGroup(e.node) ? "放进末尾" : "插到前面"),
  },
  {
    id: "v2",
    name: "目标优先",
    blurb:
      "先点「移到…」，左栏只剩组，点一个就落进去。赌的是「省一步」—— 代价写在同一格里：它连 ADR 前面都放不进去，只能进 docs 末尾。",
    specs: [
      ["动作", "2 次点击"],
      ["落点", "只有组末尾"],
      ["排序", "不能"],
    ],
    tail: (c, e) =>
      isGroup(e.node)
        ? ""
        : `<button class="mv" data-act="pick" data-id="${e.node.id}"
             aria-label="把 ${esc(e.node.name)} 移到…">移到…</button>`,
    rows: (c) => (c.pick ? entries(c.tree).filter((e) => isGroup(e.node)) : entries(c.tree)),
    resolve: (c, id) => (isGroup(c.nodeOf(id)) ? intoPos(id) : null),
    pickTip: () => "只剩组 —— 挑一个",
    dropHint: (c, e) => (isGroup(e.node) ? "放进末尾" : "×"),
    afterDrop: () => "（02 只能进组末尾，排序做不到）",
  },
  {
    id: "v3",
    name: "拖 · 三段",
    blurb:
      "真拖拽，落点分上中下三段：上=排它前面，中=进这个组，下=排它后面，带插入线。赌的是热区能讲清楚。26px 的行三等分，一条只有 8.6px；块行的中段还是死区。",
    specs: [
      ["热区", "26px ÷ 3 = 8.6px"],
      ["块行中段", "死区"],
      ["排序", "能"],
    ],
    drag: "three",
    tail: (c, e) =>
      isGroup(e.node)
        ? ""
        : `<button class="grab" data-act="pick" data-id="${e.node.id}"
             aria-label="用键盘拎起 ${esc(e.node.name)}">拎</button>`,
    pickTip: () => "键盘：组=放进 · 块=插到前面",
  },
  {
    id: "v4",
    name: "拖 · 一段",
    blurb:
      "同一套拖拽，砍掉三段：拖到组上=进组，拖到块上=排它前面。赌的是 —— 少一半规则，比多出来的那点精确更值钱。和 03 摆在一起才知道三段值不值。",
    specs: [
      ["热区", "整行 26px"],
      ["规则", "2 条"],
      ["排序", "能"],
    ],
    drag: "one",
    tail: (c, e) =>
      isGroup(e.node)
        ? ""
        : `<button class="grab" data-act="pick" data-id="${e.node.id}"
             aria-label="用键盘拎起 ${esc(e.node.name)}">拎</button>`,
    pickTip: () => "键盘：组=放进 · 块=插到前面",
  },
  {
    id: "v5",
    name: "幽灵行",
    blurb:
      "拎起来后，那行变成一条跟着光标走的半透明幽灵，原位留一道空槽；松手落到光标指着的那一行。赌的是「不用瞄准」—— 代价是落点在哪一行只能靠猜，全程没有一条插入线。",
    specs: [
      ["反馈", "只有幽灵"],
      ["插入线", "没有"],
      ["排序", "能"],
    ],
    drag: "ghost",
    tail: (c, e) =>
      isGroup(e.node)
        ? ""
        : `<button class="grab" data-act="pick" data-id="${e.node.id}"
             aria-label="用键盘拎起 ${esc(e.node.name)}">拎</button>`,
    pickTip: () => "键盘：组=放进 · 块=插到前面",
  },
  {
    id: "v6",
    name: "两栏",
    blurb:
      "左栏点一个块当源，右半屏滑出压平的目标树：带路径、带搜索，点行=插到它前面，组上另有一颗「进末尾」。赌的是树一深，一屏放不下两层选择。",
    specs: [
      ["一屏", "2 层"],
      ["搜索", "有"],
      ["排序", "能"],
    ],
    onRow: (c, id) => {
      if (isGroup(c.nodeOf(id))) return;
      if (c.sel === id) return c.clearSel();
      c.sel = id;
      c.query = "";
      c.say(`源：${treePath(c.tree, id)} —— 右边挑落点`);
      c.render();
    },
    auxOn: (c) => !!c.sel,
    aux: (c) => `
      <div class="pane">
        <div class="pane-head">
          <span class="pane-title">目标</span>
          <input class="search" data-act="query" spellcheck="false"
                 placeholder="搜路径：doc / adr / dadr" value="${esc(c.query)}" />
        </div>
        <div class="pane-list"></div>
      </div>`,
    paint: (c) => {
      const box = c.el.querySelector(".pane-list");
      if (!box) return;
      if (!c.sel) return void (box.innerHTML = "");
      const q = c.query.trim();
      const rows = entries(c.tree)
        .filter((e) => e.node.id !== c.sel)
        .map((e) => ({ e, score: fuzzy(treePath(c.tree, e.node.id), q) }))
        .filter((r) => r.score > 0)
        .sort((a, b) => (q ? b.score - a.score : 0));
      box.innerHTML = rows.length
        ? rows
            .map(
              ({ e }) => `<div class="tline">
                 ${
                   e.depth === 0
                     ? `<span class="tp troot">${esc(treePath(c.tree, e.node.id))}</span>`
                     : `<button class="trow ${isGroup(e.node) ? "is-group" : ""}"
                          data-target="${e.node.id}" data-mode="before">
                          <span class="tp">${esc(treePath(c.tree, e.node.id))}</span>
                          <span class="tm">插到前面</span>
                        </button>`
                 }
                 ${
                   isGroup(e.node)
                     ? `<button class="tin" data-target="${e.node.id}" data-mode="into">进末尾</button>`
                     : ""
                 }
               </div>`,
            )
            .join("")
        : `<p class="empty">没有匹配的路径</p>`;
    },
    onEscape: (c) => {
      if (!c.sel) return false;
      c.clearSel();
      return true;
    },
  },
  {
    id: "v7",
    name: "命令面板",
    blurb:
      "⌘K 打开：先选源、再选目标，全程键盘，路径补全 —— 打 dadr 就能命中 docs/ADR。赌的是键盘党的上限；鼠标党不会去学，但每次用都是三步。",
    specs: [
      ["入口", "⌘K / Ctrl+K"],
      ["补全", "dadr → docs/ADR"],
      ["鼠标", "不用"],
    ],
    bar: () => `<button class="keycap" data-act="open-palette"><kbd>⌘K</kbd> 打开命令面板</button>`,
    auxOn: (c) => c.palette,
    aux: (c) => `
      <div class="palette" role="dialog" aria-label="命令面板">
        <div class="palette-top">
          <span class="step">${c.step === 1 ? "1 选源" : "2 选目标"}</span>
          <input class="palette-input" data-act="palette-query" spellcheck="false"
                 placeholder="${c.step === 1 ? "找要挪的块：app / notes" : "找落点：dadr = docs/ADR"}"
                 value="${esc(c.query)}" />
          <button class="linkbtn" data-act="palette-close">Esc</button>
        </div>
        <div class="palette-list" role="listbox"></div>
        <div class="palette-hint"></div>
      </div>`,
    paint: (c) => {
      const list = c.el.querySelector(".palette-list");
      if (!list) return;
      const rows = paletteRows(c);
      if (c.cursor > rows.length - 1) c.cursor = Math.max(0, rows.length - 1);
      list.innerHTML = rows.length
        ? rows
            .map(
              (r, i) => `<button class="prow ${i === c.cursor ? "cur" : ""}"
                 data-act="palette-go" data-id="${r.id}" data-mode="${r.mode}">
                 <span class="plabel">${esc(r.label)}</span>
                 <span class="pid">${r.id}</span>
               </button>`,
            )
            .join("")
        : `<p class="empty">没有匹配的路径</p>`;
      const hint = c.el.querySelector(".palette-hint");
      if (hint) {
        hint.textContent =
          c.step === 1
            ? `↑↓ 选 · Enter 定源 · Esc 关（共 ${rows.length} 项）`
            : `↑↓ 选 · Enter 落子 · Backspace 回上一步 · Esc 关（共 ${rows.length} 项）`;
      }
    },
    afterRender: (c) => {
      if (!c.palette) return;
      const box = c.el.querySelector(".palette-input");
      if (box && document.activeElement !== box) {
        box.focus();
        box.setSelectionRange(c.query.length, c.query.length);
      }
    },
    act: (c, kind, id, ev, el) => {
      if (kind === "open-palette") {
        openPalette(c);
        return true;
      }
      if (kind === "palette-close") {
        closePalette(c);
        return true;
      }
      if (kind === "palette-go") {
        // 点的第几项就落第几项 —— 别用键盘光标那个数
        paletteGo(c, [...c.el.querySelectorAll(".prow")].indexOf(el));
        return true;
      }
      return false;
    },
    onKey: (c, ev) => {
      if ((ev.metaKey || ev.ctrlKey) && ev.key.toLowerCase() === "k") {
        ev.preventDefault();
        if (c.palette) closePalette(c);
        else openPalette(c);
        return true;
      }
      if (!c.palette) return false;
      if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
        ev.preventDefault();
        const n = paletteRows(c).length;
        if (!n) return true;
        c.cursor = (c.cursor + (ev.key === "ArrowDown" ? 1 : -1) + n) % n;
        c.cfg.paint(c);
        return true;
      }
      if (ev.key === "Enter") {
        ev.preventDefault();
        paletteGo(c, c.cursor);
        return true;
      }
      if (ev.key === "Backspace" && !c.query && c.step === 2) {
        ev.preventDefault();
        c.step = 1;
        c.pick = null;
        c.cursor = 0;
        c.say("");
        c.render();
        return true;
      }
      return false;
    },
    onEscape: (c) => {
      if (!c.palette) return false;
      closePalette(c);
      return true;
    },
  },
  {
    id: "v8",
    name: "改地址",
    blurb:
      "选中节点，顶部露出它那条 tree 路径，直接编辑这段字符串再回车。赌的是「位置就是个地址」—— 爽和危险是同一件事：docs/ADR 落在 ADR 前面，docs/ 落在 docs 末尾，打错一个字就落不下去。",
    specs: [
      ["动作", "选中 + 改字"],
      ["打错", "报错，不猜"],
      ["排序", "能"],
    ],
    bar: (c) =>
      c.sel
        ? `<div class="pathbar">
             <span class="label">tree</span>
             <input class="path" data-act="path" spellcheck="false"
                    value="${esc(c.pathDraft ?? treePath(c.tree, c.sel))}" />
             <button class="linkbtn" data-act="apply-path">Enter</button>
             <button class="linkbtn" data-act="cancel-sel">Esc</button>
           </div>`
        : `<p class="barnote">点一行选中，然后改它的路径：docs/ADR 插到 ADR 前面，docs/ 落到 docs 末尾。</p>`,
    onRow: (c, id) => {
      if (isGroup(c.nodeOf(id))) return;
      if (c.sel === id) return c.clearSel();
      c.sel = id;
      c.pathDraft = null;
      c.say(`${c.nameOf(id)} 现在是 ${treePath(c.tree, id)} —— 改成 docs/ADR 试试`);
      c.render();
    },
    act: (c, kind) => {
      if (kind === "apply-path") {
        applyPath(c);
        return true;
      }
      return false;
    },
    onKey: (c, ev) => {
      if (ev.key === "Enter" && ev.target.dataset?.act === "path") {
        ev.preventDefault();
        applyPath(c);
        return true;
      }
      return false;
    },
    afterRender: (c) => {
      const box = c.el.querySelector('[data-act="path"]');
      if (box && document.activeElement !== box) {
        box.focus();
        box.select();
      }
    },
    onEscape: (c) => {
      if (!c.sel) return false;
      c.clearSel();
      return true;
    },
  },
  {
    id: "v9",
    name: "键盘游标",
    blurb:
      "不弹任何新 UI：拎起来后 ↑↓ 走行，→ 进组、← 出组，Enter 落。赌的是全键盘也能不迷路 —— 前提是左栏永远显示全树，一格都不许折叠。",
    specs: [
      ["新 UI", "0"],
      ["折叠", "不许"],
      ["排序", "能"],
    ],
    followFocus: true,
    bar: (c) =>
      `<div class="barnote">${
        c.pick
          ? `拎着 <b>${esc(c.nameOf(c.pick))}</b> · ↑↓ 走行 · → 进组 · ← 出组 · Enter 落 · Esc 取消
             <span class="target">落点：${
               c.armedMode === "into"
                 ? `放进 ${esc(treePath(c.tree, c.armedId))} 末尾`
                 : `插到 ${esc(treePath(c.tree, c.armedId))} 前面`
             }</span>`
          : `↑↓ 选行 · Enter 拎起来 · Esc 取消`
      }</div>`,
    onKey: (c, ev) => {
      const row = ev.target.closest?.(".row");
      if (!row || !c.listEl.contains(row)) return false;
      const all = entries(c.tree);
      const at = () => {
        const i = all.findIndex((e) => e.node.id === (c.cursorId ?? row.dataset.id));
        return i < 0 ? 0 : i;
      };
      const repaint = () => {
        c.render();
        c.rowEl(c.cursorId)?.focus();
      };

      if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
        ev.preventDefault();
        const next = Math.max(
          0,
          Math.min(all.length - 1, at() + (ev.key === "ArrowDown" ? 1 : -1)),
        );
        c.cursorId = all[next].node.id;
        if (c.pick) {
          c.armedId = c.cursorId;
          c.armedMode = "before";
        }
        repaint();
        return true;
      }

      if (!c.pick) {
        if (ev.key !== "Enter") return false;
        ev.preventDefault();
        const node = c.nodeOf(c.cursorId ?? row.dataset.id);
        if (!node || isGroup(node)) {
          c.say("组不能挪 —— 这一页只挪块", true);
          return true;
        }
        c.pick = node.id;
        c.fresh = null;
        c.say("");
        const next = all[Math.min(all.length - 1, all.findIndex((e) => e.node.id === node.id) + 1)];
        c.cursorId = next.node.id;
        c.armedId = next.node.id;
        c.armedMode = "before";
        repaint();
        return true;
      }

      if (ev.key === "ArrowRight") {
        ev.preventDefault();
        const node = c.nodeOf(c.cursorId);
        if (node && isGroup(node)) {
          c.armedId = node.id;
          c.armedMode = "into";
          repaint();
        } else c.say("这一行不是组 —— 进不去", true);
        return true;
      }
      if (ev.key === "ArrowLeft") {
        ev.preventDefault();
        c.armedMode = "before";
        repaint();
        return true;
      }
      if (ev.key === "Enter") {
        ev.preventDefault();
        if (!c.armedId || c.armedId === c.pick) {
          c.say("这个落点不行：那是它自己", true);
          return true;
        }
        const pos =
          c.armedMode === "into" ? intoPos(c.armedId) : beforePos(c.tree, c.armedId);
        c.applyPos(pos, c.pick);
        return true;
      }
      return false;
    },
  },
  {
    id: "v10",
    name: "改爹下拉",
    blurb:
      "行内一枚原生下拉，列出所有组，选一个就落到它的末尾。赌的是「最不可能点错」—— 它顺手把一件事说清楚了：排序是另一个需求，这个方案里做不了。",
    specs: [
      ["动作", "下拉 + 选"],
      ["控件", "原生 select"],
      ["排序", "不能"],
    ],
    tail: (c, e) => {
      if (isGroup(e.node)) return "";
      const opts = entries(c.tree)
        .filter((x) => isGroup(x.node))
        .map(
          (x) =>
            `<option value="${x.node.id}" ${x.node.id === e.parent?.id ? "selected" : ""}>${esc(
              treePath(c.tree, x.node.id),
            )}</option>`,
        )
        .join("");
      return `<select class="dad" data-act="dad" data-id="${e.node.id}"
                aria-label="把 ${esc(e.node.name)} 换到哪个组">${opts}</select>`;
    },
    act: (c, kind, id, ev, el) => {
      // 只认 change：点开下拉本身不该动任何东西
      if (kind !== "dad" || !id || ev.type !== "change") return false;
      const parentId = el.value;
      if (parentId === c.nodeOf(id)?.parent?.id) return true;
      if (!applyMove(c.tree, id, parentId, Number.MAX_SAFE_INTEGER)) {
        c.say("这个落点不行", true);
        return true;
      }
      c.landed(id);
      return true;
    },
    afterDrop: () => "（只换了爹，排序还是做不到）",
  },
  /* ══ 11–20：换的是手，不是题 ══════════════════════════════════════════════ */

  /* ── 11 长按浮起 ───────────────────────────────────────────────────────── */
  {
    id: "v11",
    name: "长按浮起",
    blurb:
      "按住一行 0.26 秒：它从树里分离出来跟着手指走，底下压成暗底，菜单在手指边冒出来 —— 第一层是「放进哪个组」，第二层才是「插到哪一行前面」。滑到哪项、松开就落在哪项。赌的是菜单比落点好认：树不参与落点，就没有瞄准这回事。",
    specs: [
      ["入口", "长按 260ms"],
      ["菜单", "两层"],
      ["树", "压成暗底"],
    ],
    bar: () =>
      `<p class="barnote">按住任意一个块 0.26 秒（或者按 <kbd>Enter</kbd>）—— 它浮起来，菜单跟着手指走。</p>`,
    onRow: (c, id) => openCtx(c, id),
    afterRender: (c) => installHold(c),
    auxOn: (c) => !!c.menu,
    aux: (c) => (c.menu ? ctxHTML(c) : ""),
    act: (c, kind, _id, _ev, el) => {
      if (kind === "mitem") {
        ctxGo(c, Number(el.dataset.lvl), Number(el.dataset.i));
        return true;
      }
      if (kind === "ctx-close") {
        closeCtx(c);
        return true;
      }
      return false;
    },
    onKey: (c, ev) => {
      if (!c.menu) return false;
      const items = [...c.el.querySelectorAll(".mitem")];
      if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
        ev.preventDefault();
        const at = items.indexOf(document.activeElement);
        const next = Math.max(0, Math.min(items.length - 1, at + (ev.key === "ArrowDown" ? 1 : -1)));
        c.menu.hover = next;
        items.forEach((n, k) => n.classList.toggle("hot", k === next));
        items[next]?.focus();
        return true;
      }
      if (ev.key === "Backspace" && c.menu.level === 1) {
        ev.preventDefault();
        ctxGo(c, 1, 0); // 「← 返回」那一项
        return true;
      }
      return false;
    },
    onEscape: (c) => {
      if (!c.menu) return false;
      closeCtx(c);
      return true;
    },
  },

  /* ── 12 径向环 ─────────────────────────────────────────────────────────── */
  {
    id: "v12",
    name: "径向环",
    blurb:
      "点一个块，一圈按钮在它上面铺开：这一层的每一格 + 一条「放进这一层末尾」。点名字 = 插到它前面；组上多一颗「进 ▸」，钻下去看它里面。赌的是环不受行数限制 —— 这一格终于把「插到组前面」和「放进组里面」分成了两个按钮。",
    specs: [
      ["形状", "16px 一颗"],
      ["两个意思", "分得开"],
      ["键盘", "1–9 / Backspace"],
    ],
    bar: (c) =>
      `<div class="barnote">${
        c.ring
          ? `环在 <b>${esc(treePath(c.tree, c.ring.path[c.ring.path.length - 1]))}</b> 这一层 · 点名字 = 插到它前面 · Esc 收起`
          : `点一个块 —— 环会绕着它铺开，一格一颗。`
      }</div>`,
    onRow: (c, id) => {
      if (isGroup(c.nodeOf(id))) return;
      c.ring = { src: id, path: [c.tree.id] };
      c.say("");
      c.render();
    },
    auxOn: (c) => !!c.ring,
    aux: (c) => (c.ring ? ringHTML(c) : ""),
    act: (c, kind, id) => {
      if (kind === "ring-go") {
        const src = c.ring?.src;
        c.ring = null;
        c.applyPos(beforePos(c.tree, id), src);
        return true;
      }
      if (kind === "ring-in") {
        c.ring.path.push(id);
        c.render();
        return true;
      }
      if (kind === "ring-into") {
        const src = c.ring?.src;
        c.ring = null;
        c.applyPos(intoPos(id), src);
        return true;
      }
      if (kind === "ring-up") {
        if (c.ring.path.length > 1) c.ring.path.pop();
        else c.ring = null;
        c.render();
        return true;
      }
      if (kind === "ring-close") {
        c.ring = null;
        c.say("已取消 —— 树没动");
        c.render();
        return true;
      }
      return false;
    },
    onKey: (c, ev) => {
      if (!c.ring) return false;
      if (ev.key === "Backspace") {
        ev.preventDefault();
        c.ring.path.length > 1 ? c.ring.path.pop() : (c.ring = null);
        c.render();
        return true;
      }
      if (/^[1-9]$/.test(ev.key)) {
        const items = ringItems(c);
        const item = items[Number(ev.key) - 1];
        if (!item) return false;
        ev.preventDefault();
        if (item.kind === "up") {
          c.ring.path.pop();
          c.render();
        } else if (item.kind === "into") {
          const src = c.ring.src;
          c.ring = null;
          c.applyPos(intoPos(item.id), src);
        } else if (item.group) {
          c.ring.path.push(item.id); // 组：数字键先进去看
          c.render();
        } else {
          const src = c.ring.src;
          c.ring = null;
          c.applyPos(beforePos(c.tree, item.id), src);
        }
        return true;
      }
      return false;
    },
    onEscape: (c) => {
      if (!c.ring) return false;
      c.ring = null;
      c.say("已取消 —— 树没动");
      c.render();
      return true;
    },
  },

  /* ── 13 连线 ───────────────────────────────────────────────────────────── */
  {
    id: "v13",
    name: "连线",
    blurb:
      "每一行左边一颗圆点，组右边多一颗：从一个圆点拉一条线到另一个圆点 —— 线是唯一的落点。赌的是「不搬东西」：这一行从头到尾没离开过它的位置，动的是那条线。代价也明摆着：圆点只有 12px，比行还小。",
    specs: [
      ["落点", "12px 的圆点"],
      ["源不动", "全程在原位"],
      ["键盘", "Tab 到圆点"],
    ],
    bar: () =>
      `<p class="barnote">从一个圆点拉到另一个圆点：左端点 = 插到它前面，组右边的点 = 放进组末尾。</p>`,
    pre: (c, e) =>
      e.depth === 0
        ? `<span class="port is-off" aria-hidden="true"></span>`
        : `<button class="port" data-act="wire" data-id="${e.node.id}" tabindex="0"
             aria-label="从 ${esc(e.node.name)} 拉一条线"></button>`,
    tail: (c, e) =>
      isGroup(e.node) && e.depth > 0
        ? `<button class="port inport" data-act="wire-in" data-id="${e.node.id}" tabindex="0"
             aria-label="把线拉进 ${esc(e.node.name)}"></button>`
        : "",
    act: (c, kind, id) => {
      if (kind === "wire") {
        if (!c.link) {
          if (isGroup(c.nodeOf(id))) return true;
          startWire(c, id);
          return true;
        }
        if (c.link.src === id) {
          endWire(c, "同一颗点 —— 松开了");
          return true;
        }
        const src = c.link.src;
        endWire(c);
        c.applyPos(beforePos(c.tree, id), src);
        return true;
      }
      if (kind === "wire-in") {
        if (!c.link) return true;
        const src = c.link.src;
        endWire(c);
        c.applyPos(intoPos(id), src);
        return true;
      }
      return false;
    },
    onClick: (c) => {
      if (c.link) endWire(c, "线断了 —— 树没动");
    },
    onEscape: (c) => {
      if (!c.link) return false;
      endWire(c, "线断了 —— 树没动");
      return true;
    },
  },

  /* ── 14 剪贴板 ─────────────────────────────────────────────────────────── */
  {
    id: "v14",
    name: "剪贴板",
    blurb:
      "剪下来，再粘上去。剪切之后整棵树**一个字都没动** —— 它只是进了剪贴板；每一行这时都多一颗粘贴键。赌的是「先拿走、再放」：动作是两拍的，中间那拍可以反悔，也可以先去看清楚。",
    specs: [
      ["拍数", "2 拍"],
      ["粘贴前", "树不动"],
      ["键盘", "⌘X / ⌘V"],
    ],
    bar: (c) =>
      c.snip
        ? `<div class="clip">
             <span class="clip-lab">剪贴板</span>
             <b>${esc(c.nameOf(c.snip))}</b><span class="clip-id">${c.snip}</span>
             <span class="clip-tip">粘到哪一行前面 · <kbd>⌘V</kbd></span>
             <button class="linkbtn" data-act="unsnip">放回 <kbd>Esc</kbd></button>
           </div>`
        : `<p class="barnote">行尾「剪切」（或 <kbd>⌘X</kbd>）—— 树在你按粘贴之前一个字都不动。</p>`,
    tail: (c, e) => {
      if (e.depth === 0) return "";
      if (!c.snip) {
        return isGroup(e.node)
          ? ""
          : `<button class="mv" data-act="cut" data-id="${e.node.id}">剪切</button>`;
      }
      if (c.snip === e.node.id) return `<button class="mv" data-act="unsnip">放回</button>`;
      return isGroup(e.node)
        ? `<button class="mv" data-act="paste" data-id="${e.node.id}" data-mode="before">粘到前面</button>
           <button class="mv" data-act="paste" data-id="${e.node.id}" data-mode="into">粘进末尾</button>`
        : `<button class="mv" data-act="paste" data-id="${e.node.id}" data-mode="before">粘到前面</button>`;
    },
    act: (c, kind, id, _ev, el) => {
      if (kind === "cut") {
        c.snip = id;
        c.sel = id;
        c.say(`剪下来了：${c.nameOf(id)} —— 树没动，等你粘`);
        c.render();
        return true;
      }
      if (kind === "unsnip") {
        c.snip = null;
        c.sel = null;
        c.say("放回去了 —— 树从头到尾没动过");
        c.render();
        return true;
      }
      if (kind === "paste") {
        const src = c.snip;
        if (!src) return true;
        const mode = el.dataset.mode;
        c.snip = null;
        c.sel = null;
        c.applyPos(posOf(c.tree, { id, mode }), src);
        return true;
      }
      return false;
    },
    onKey: (c, ev) => {
      const row = ev.target.closest?.(".row");
      const id = row && c.listEl.contains(row) ? row.dataset.id : null;
      const mod = ev.metaKey || ev.ctrlKey;
      if (mod && ev.key.toLowerCase() === "x") {
        ev.preventDefault();
        if (!id) {
          c.say("先把光标放到一行上", true);
          return true;
        }
        c.cfg.act(c, "cut", id);
        return true;
      }
      if (mod && ev.key.toLowerCase() === "v") {
        ev.preventDefault();
        if (!c.snip) {
          c.say("剪贴板是空的", true);
          return true;
        }
        if (!id) {
          c.say("先选一行当落点", true);
          return true;
        }
        const mode = isGroup(c.nodeOf(id)) ? "into" : "before";
        const src = c.snip;
        c.snip = null;
        c.sel = null;
        c.applyPos(posOf(c.tree, { id, mode }), src);
        return true;
      }
      return false;
    },
    onEscape: (c) => {
      if (!c.snip) return false;
      c.snip = null;
      c.sel = null;
      c.say("放回去了 —— 树从头到尾没动过");
      c.render();
      return true;
    },
  },

  /* ── 15 滚轮 ───────────────────────────────────────────────────────────── */
  {
    id: "v15",
    name: "滚轮",
    blurb:
      "点一个块当源，目的地变成一只转轮：滚一格换一个候选，只有正中间那一格会被落下 —— 上下两侧的按距离缩小、转身、变淡。赌的是「候选再多也不用扫」：眼睛只盯中间一格。",
    specs: [
      ["选择", "滚一格"],
      ["候选", "一次只看见 3 个"],
      ["确认", "中间那格"],
    ],
    bar: (c) =>
      c.wheel
        ? `<div class="barnote">滚到中间那一格 · <kbd>Enter</kbd> 落 · <kbd>Esc</kbd> 收起来</div>`
        : `<p class="barnote">先点一个块当源，然后滚轮转目的地。</p>`,
    onRow: (c, id) => {
      if (isGroup(c.nodeOf(id))) return;
      c.wheel = { src: id, idx: 0 };
      c.say(`源：${treePath(c.tree, id)} —— 转到中间那一格`);
      c.render();
    },
    auxOn: (c) => !!c.wheel,
    aux: (c) => (c.wheel ? wheelHTML(c) : ""),
    act: (c, kind, _id, _ev, el) => {
      if (kind === "wheel-go") {
        const i = Number(el.dataset.i);
        if (i === c.wheel.idx) wheelDrop(c, i);
        else {
          c.wheel.idx = i;
          c.render();
        }
        return true;
      }
      if (kind === "wheel-close") {
        c.wheel = null;
        c.say("收起来了 —— 树没动");
        c.render();
        return true;
      }
      return false;
    },
    onWheel: (c, ev) => {
      if (!c.wheel) return;
      ev.preventDefault();
      wheelStep(c, ev.deltaY > 0 ? 1 : -1);
    },
    onKey: (c, ev) => {
      if (!c.wheel) return false;
      if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
        ev.preventDefault();
        wheelStep(c, ev.key === "ArrowDown" ? 1 : -1);
        return true;
      }
      if (ev.key === "Enter") {
        ev.preventDefault();
        wheelDrop(c, c.wheel.idx);
        return true;
      }
      return false;
    },
    onEscape: (c) => {
      if (!c.wheel) return false;
      c.wheel = null;
      c.say("收起来了 —— 树没动");
      c.render();
      return true;
    },
  },

  /* ── 16 说话 ───────────────────────────────────────────────────────────── */
  {
    id: "v16",
    name: "说话",
    blurb:
      "不打路径、不点行：用中文说一句「把 App.tsx 挪到 ADR 前面」，它从这句话里认出源、落点和关系，下面那行字告诉你它听懂了什么。赌的是这一格最省动作，也最可能听错 —— 它只认这棵树上的名字，多一个字都算噪音。",
    specs: [
      ["动作", "打一句话"],
      ["树", "只用来显示结果"],
      ["听不懂", "明说，不猜"],
    ],
    bar: (c) => `
      <div class="saybox">
        <input class="say" data-act="nl" spellcheck="false" value="${esc(c.nl)}"
               placeholder="把 App.tsx 挪到 ADR 前面" aria-label="用一句话说要怎么挪" />
        <button class="linkbtn" data-act="nl-go">执行</button>
      </div>
      <button class="ex" data-act="nl-ex" data-text="把 App.tsx 挪到 ADR 前面">试试：把 App.tsx 挪到 ADR 前面</button>
      <button class="ex" data-act="nl-ex" data-text="notes.md 放进 docs 里">试试：notes.md 放进 docs 里</button>`,
    hint: (c) => {
      if (!c.nl.trim()) return "说一句就行。它只认这页面上的名字。";
      const p = parseNL(c.tree, c.nl);
      return p.why ? `没听懂：${p.why}` : `听成：${p.says}`;
    },
    act: (c, kind, _id, _ev, el) => {
      if (kind === "nl-go") {
        nlGo(c);
        return true;
      }
      if (kind === "nl-ex") {
        c.nl = el.dataset.text;
        nlGo(c);
        return true;
      }
      return false;
    },
    onInput(c, ev) {
      if (ev.target.dataset?.act !== "nl") return false;
      c.nl = ev.target.value;
      const p = parseNL(c.tree, c.nl);
      c.say(p.why ? `没听懂：${p.why}` : `听成：${p.says}`, !!p.why);
      return true;
    },
    onKey: (c, ev) => {
      if (ev.key === "Enter" && ev.target.dataset?.act === "nl") {
        ev.preventDefault();
        nlGo(c);
        return true;
      }
      return false;
    },
  },

  /* ── 17 面包屑下潜 ─────────────────────────────────────────────────────── */
  {
    id: "v17",
    name: "面包屑下潜",
    blurb:
      "选一个块，整块面板变成一层一层往下钻的格子：每层只看得到这一层的孩子，顶上是面包屑，回去就点面包屑。每一格两个意思都摆在明面上 —— 「插到它前面」，或者「进它里面」。赌的是深树里，钻比滚好受。",
    specs: [
      ["一屏", "1 层"],
      ["回去", "点面包屑"],
      ["每格", "2 个意思"],
    ],
    bar: (c) =>
      `<div class="barnote">${
        c.drill
          ? `源 <b>${esc(c.nameOf(c.drill.src))}</b> · 一层一层钻到落点那一层 · Esc 收起`
          : `点一个块当源 —— 面板会变成钻头。`
      }</div>`,
    onRow: (c, id) => {
      if (isGroup(c.nodeOf(id))) return;
      c.drill = { src: id, path: [c.tree.id] };
      c.say("");
      c.render();
    },
    auxOn: (c) => !!c.drill,
    aux: (c) => (c.drill ? drillHTML(c) : ""),
    act: (c, kind, id, _ev, el) => {
      if (kind === "drill-up") {
        c.drill.path = c.drill.path.slice(0, Number(el.dataset.i) + 1);
        c.render();
        return true;
      }
      if (kind === "drill-in") {
        c.drill.path.push(id);
        c.render();
        return true;
      }
      if (kind === "drill-go") {
        const src = c.drill.src;
        c.drill = null;
        c.applyPos(beforePos(c.tree, id), src);
        return true;
      }
      if (kind === "drill-into") {
        const src = c.drill.src;
        const level = c.drill.path[c.drill.path.length - 1];
        c.drill = null;
        c.applyPos(intoPos(level), src);
        return true;
      }
      if (kind === "drill-close") {
        c.drill = null;
        c.say("收起来了 —— 树没动");
        c.render();
        return true;
      }
      return false;
    },
    onKey: (c, ev) => {
      if (!c.drill) return false;
      if (ev.key === "Backspace") {
        ev.preventDefault();
        c.drill.path = c.drill.path.slice(0, -1);
        if (!c.drill.path.length) c.drill = null;
        c.render();
        return true;
      }
      return false;
    },
    onEscape: (c) => {
      if (!c.drill) return false;
      c.drill = null;
      c.say("收起来了 —— 树没动");
      c.render();
      return true;
    },
  },

  /* ── 18 封面流 ─────────────────────────────────────────────────────────── */
  {
    id: "v18",
    name: "封面流",
    blurb:
      "落点做成一张一张的卡，排成一条有透视的带子：中间的正面朝你，两边的转过身、退远、变暗。←→ 翻，Enter 落中间那张。赌的是「看一眼就够」—— 每条落点都写着完整路径，不用在树里找它在哪。",
    specs: [
      ["深度", "3D 透视"],
      ["一次看见", "5 张"],
      ["确认", "中间那张"],
    ],
    bar: (c) =>
      c.cf
        ? `<div class="barnote">←→ 翻卡片 · <kbd>Enter</kbd> 落中间那张 · <kbd>Esc</kbd> 收起来</div>`
        : `<p class="barnote">点一个块当源，落点会排成一条带子。</p>`,
    onRow: (c, id) => {
      if (isGroup(c.nodeOf(id))) return;
      c.cf = { src: id, idx: 0 };
      c.say("");
      c.render();
    },
    auxOn: (c) => !!c.cf,
    aux: (c) => (c.cf ? cfHTML(c) : ""),
    act: (c, kind, _id, _ev, el) => {
      if (kind === "cf-go") {
        const i = Number(el.dataset.i);
        if (i === c.cf.idx) cfDrop(c, i);
        else {
          c.cf.idx = i;
          c.render();
        }
        return true;
      }
      if (kind === "cf-close") {
        c.cf = null;
        c.say("收起来了 —— 树没动");
        c.render();
        return true;
      }
      return false;
    },
    onKey: (c, ev) => {
      if (!c.cf) return false;
      const n = slotsOf(c).length;
      if (ev.key === "ArrowRight" || ev.key === "ArrowLeft") {
        ev.preventDefault();
        c.cf.idx = (c.cf.idx + (ev.key === "ArrowRight" ? 1 : -1) + n) % n;
        c.render();
        return true;
      }
      if (ev.key === "Enter") {
        ev.preventDefault();
        cfDrop(c, c.cf.idx);
        return true;
      }
      return false;
    },
    onEscape: (c) => {
      if (!c.cf) return false;
      c.cf = null;
      c.say("收起来了 —— 树没动");
      c.render();
      return true;
    },
  },

  /* ── 19 滚着挪 ─────────────────────────────────────────────────────────── */
  {
    id: "v19",
    name: "滚着挪",
    blurb:
      "没有落点这回事：攥住一行，滚一格它就往下走一行 —— 下一行是组，它就直接进到组的头一格。树是真的在动，Esc 把它送回原处。赌的是连续比离散好使：眼睛跟着那一行走，根本不用挑落点。",
    specs: [
      ["落点", "没有"],
      ["树", "边滚边变"],
      ["Esc", "送回原处"],
    ],
    bar: (c) =>
      `<div class="barnote">${
        c.scrub
          ? `攥着 <b>${esc(c.nameOf(c.scrub.id))}</b> · 滚轮或 <kbd>⌥↑↓</kbd> 走一行 · <kbd>Enter</kbd> 定住 · <kbd>Esc</kbd> 退回原处`
          : `点一个块攥住 —— 然后滚。没有落点，只有一步一行。`
      }</div>`,
    tail: (c, e) => {
      if (isGroup(e.node)) return "";
      if (c.scrub?.id === e.node.id) {
        return `<span class="nudge">
          <button data-act="scrub-up" aria-label="往上一行">↑</button><button data-act="scrub-down" aria-label="往下一行">↓</button>
        </span>`;
      }
      return `<button class="grab" data-act="scrub" data-id="${e.node.id}"
        aria-label="攥住 ${esc(e.node.name)}">攥</button>`;
    },
    onRow: (c, id) => startScrub(c, id),
    act: (c, kind, _id, _ev, el) => {
      if (kind === "scrub") {
        startScrub(c, el.dataset.id);
        return true;
      }
      if (kind === "scrub-up" || kind === "scrub-down") {
        scrubStep(c, kind === "scrub-down" ? 1 : -1);
        return true;
      }
      return false;
    },
    onWheel: (c, ev) => {
      if (!c.scrub) return;
      ev.preventDefault();
      scrubStep(c, ev.deltaY > 0 ? 1 : -1);
    },
    onKey: (c, ev) => {
      if (!c.scrub) return false;
      if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
        ev.preventDefault();
        scrubStep(c, ev.key === "ArrowDown" ? 1 : -1);
        return true;
      }
      if (ev.key === "Enter") {
        ev.preventDefault();
        const id = c.scrub.id;
        c.scrub = null;
        c.landed(id);
        return true;
      }
      return false;
    },
    onEscape: (c) => {
      if (!c.scrub) return false;
      const { id, origin } = c.scrub;
      c.scrub = null;
      applyMove(c.tree, id, origin.parentId, origin.index);
      c.say(`${c.nameOf(id)} 退回原处 —— 树回到你动手之前`);
      c.render();
      return true;
    },
  },

  /* ── 20 序号指令 ───────────────────────────────────────────────────────── */
  {
    id: "v20",
    name: "序号指令",
    blurb:
      "行号印在左边。敲 3>8 回车 —— 把第 3 行挪到第 8 行前面，跟 ed 里的 m 命令一模一样；也可以点两行凑出这两个数。赌的是熟手只要四个键：不要搜索、不要落点、不要确认。",
    specs: [
      ["动作", "4 个键"],
      ["鼠标", "不用"],
      ["落点", "写成数字"],
    ],
    numbering: true,
    bar: (c) => `
      <div class="edbar">
        <span class="ed-prompt" aria-hidden="true">:</span>
        <input class="ed-in" data-act="ed" spellcheck="false" value="${esc(c.ed)}"
               placeholder="3>8" aria-label="按行号挪：3>8 表示第 3 行挪到第 8 行前面" />
        <button class="linkbtn" data-act="ed-go">执行</button>
      </div>`,
    hint: (c) => {
      const p = edParse(c);
      if (!c.ed.trim()) return "3>8 = 把第 3 行挪到第 8 行前面 · 也可以直接点两行";
      return p.why ? p.why : `会这么落：${c.nameOf(p.src)} → 第 ${p.tgtIdx} 行（${c.nameOf(p.tgtId)}）前面`;
    },
    onRow: (c, id) => {
      const i = entries(c.tree).findIndex((e) => e.node.id === id) + 1;
      if (c.edSrc && c.edSrc !== i) {
        c.ed = `${c.edSrc}>${i}`;
        c.edSrc = null;
        edGo(c);
        return;
      }
      if (isGroup(c.nodeOf(id))) return c.say("第一个数是组 —— 这一页只挪块", true);
      c.edSrc = i;
      c.ed = `${i}>`;
      c.say(`第 ${i} 行 ${c.nameOf(id)} —— 再点一行当落点，或者把数字敲完`);
      c.render();
      c.el.querySelector('[data-act="ed"]')?.focus();
    },
    onInput: (c, ev) => {
      if (ev.target.dataset?.act !== "ed") return false;
      c.ed = ev.target.value;
      const p = edParse(c);
      if (!c.ed.trim()) return c.say(""), true;
      c.say(p.why ? p.why : `会这么落：${c.nameOf(p.src)} → 第 ${p.tgtIdx} 行（${c.nameOf(p.tgtId)}）前面`, !!p.why);
      return true;
    },
    act: (c, kind) => {
      if (kind === "ed-go") {
        edGo(c);
        return true;
      }
      return false;
    },
    onKey: (c, ev) => {
      if (ev.key === "Enter" && ev.target.dataset?.act === "ed") {
        ev.preventDefault();
        edGo(c);
        return true;
      }
      return false;
    },
  },
];

/* ── 07 的三段小逻辑：开 / 关 / 落 ───────────────────────────────────────── */
function paletteRows(c) {
  const q = c.query.trim();
  if (c.step === 1) {
    return entries(c.tree)
      .filter((e) => !isGroup(e.node))
      .map((e) => ({ id: e.node.id, mode: "select", label: treePath(c.tree, e.node.id) }))
      .map((r) => ({ ...r, score: fuzzy(r.label, q) }))
      .filter((r) => r.score > 0)
      .sort((a, b) => (q ? b.score - a.score : 0));
  }
  return targetRows(c.tree, c.pick)
    .map((r) => ({ ...r, score: fuzzy(r.label, q) }))
    .filter((r) => r.score > 0)
    .sort((a, b) => (q ? b.score - a.score : 0));
}

function openPalette(c) {
  c.palette = true;
  c.step = 1;
  c.pick = null;
  c.query = "";
  c.cursor = 0;
  c.say("");
  c.render();
}

function closePalette(c) {
  c.palette = false;
  c.pick = null;
  c.step = 1;
  c.query = "";
  c.cursor = 0;
  c.say("");
  c.render();
}

function paletteGo(c, index) {
  const rows = paletteRows(c);
  const row = rows[Math.max(0, Math.min(rows.length - 1, index))];
  if (!row) return;
  if (c.step === 1) {
    c.pick = row.id;
    c.step = 2;
    c.query = "";
    c.cursor = 0;
    c.say(`源：${treePath(c.tree, row.id)} —— 再挑落点`);
    c.render();
    return;
  }
  const pos = row.mode === "into" ? intoPos(row.id) : beforePos(c.tree, row.id);
  c.palette = false;
  c.step = 1;
  c.query = "";
  c.applyPos(pos, c.pick);
}

/* ── 页面壳子 ─────────────────────────────────────────────────────────────── */
const root = document.documentElement;
const deck = document.getElementById("deck");
const tabs = document.getElementById("tabs");
const themeBtn = document.getElementById("theme");

const chips = [{ id: "all", label: "全部" }].concat(
  VARIANTS.map((v) => ({ id: v.id, label: `${v.id.slice(1).padStart(2, "0")} ${v.name}` })),
);
tabs.innerHTML = chips
  .map(
    (ch, i) =>
      `<button class="chip" data-id="${ch.id}" aria-pressed="${i === 0}">${esc(ch.label)}</button>`,
  )
  .join("");

for (const cfg of VARIANTS) deck.appendChild(new Card(cfg).el);

tabs.addEventListener("click", (ev) => {
  const chip = ev.target.closest(".chip");
  if (!chip) return;
  const want = chip.dataset.id;
  tabs.querySelectorAll(".chip").forEach((c) => c.setAttribute("aria-pressed", String(c === chip)));
  deck.querySelectorAll(".card").forEach((card) => {
    card.hidden = want !== "all" && card.dataset.id !== want;
  });
  window.scrollTo({ top: 0, behavior: "instant" });
});
// ?v=03 直接只看那一格
const deep = new URLSearchParams(location.search).get("v");
if (deep) tabs.querySelector(`.chip[data-id="${deep}"]`)?.click();

/* 主题：和别的 demo 同一把钥匙，来回切不用重新选 */
const paintTheme = () => {
  const dark = root.dataset.theme !== "light";
  themeBtn.textContent = dark ? "浅色" : "深色";
  themeBtn.setAttribute("aria-pressed", String(!dark));
};
themeBtn.addEventListener("click", () => {
  root.dataset.theme = root.dataset.theme === "light" ? "default" : "light";
  localStorage.setItem("demo-theme", root.dataset.theme);
  paintTheme();
});
root.dataset.theme = localStorage.getItem("demo-theme") ?? "default";
paintTheme();

/* Esc 在哪儿按都算数：焦点飘到 body 上也得能取消 */
document.addEventListener("keydown", (ev) => {
  if (ev.key !== "Escape") return;
  for (const card of CARDS) card.escape();
});

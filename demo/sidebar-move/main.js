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
    this.listEl.innerHTML = rows.map((e) => this.rowHTML(e)).join("");
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

  rowHTML(e) {
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
      <span class="glyph" aria-hidden="true"></span>
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

/* ── 十个方案 ───────────────────────────────────────────────────────────────
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

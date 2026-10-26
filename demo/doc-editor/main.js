/* 文档 —— 十个方案。
 *
 * 十格共用同一份假数据（AGENTS.md + 3 篇自己的），也都只回答同一个问题：
 * **「这一块有 4 份文档，我要在其中一份上写两行字」—— 怎么用最少的动作做到。**
 *
 * 全在内存里：打字、切文档、新建、删除、恢复，状态像真的，一个字节都不落盘。
 * AGENTS.md 是 coding agent 进目录时找的那一份 —— 它在任何一条路上都拿不到删除入口。
 * 不引 React、不加依赖、不发 fetch。 */

import "../../src/tokens/tokens.css";

/* ── 那一小块假数据 ────────────────────────────────────────────────────────
   十格共用这一份种子，每格 seed 一次，各写各的，互不影响。
   正文都给真内容 —— 一百来字，够看出排版。 */
const seed = () => [
  {
    id: "d1",
    name: "AGENTS.md",
    fixed: true,
    ago: "3 天前",
    text: [
      "# AGENTS.md",
      "",
      "这一块是 `map/7kq2md/`。进来先读这一份 —— 所有 coding agent 都先找 `AGENTS.md`",
      "这个文件名，换成 `agent.md` 就没人读了。",
      "",
      "## 这块是什么",
      "",
      "盘面上那个 240 值的方块。服务在 `server.js`，页面在 `public/`，`npm start` 就能跑。",
      "数据写自己的 `data/` —— `cp -r` 走的时候数据跟着走，不是宿主替你存。",
      "",
      "## 规矩",
      "",
      "- 听 `PORT`，别写死端口",
      "- `__nous/` 底下的别碰，那是宿主的",
      "- 找邻居去 `__nous/ctx` 拿地址，不许自己拼路径",
    ].join("\n"),
  },
  {
    id: "d2",
    name: "设计.md",
    fixed: false,
    ago: "2 分钟前",
    text: [
      "# 设计",
      "",
      "正文不进 `tree.json`。它有自己的文件、自己的 etag，**不参与 board 的 version** ——",
      "写一篇文章不该把树顶成 409，落一手棋也不该让笔记冲突。",
      "",
      "## 为什么分开",
      "",
      "树是语义的唯一真相，落一手棋就是一次 version。笔记要是也进树，每敲一个字",
      "都在跟别人的落子抢同一把锁。",
      "",
      "## 剩下的事",
      "",
      "- 文件名 sanitize：小写、空格转 `-`",
      "- 删掉的进 `.trash/`，不真删",
      "- `AGENTS.md` 只能改",
    ].join("\n"),
  },
  {
    id: "d3",
    name: "notes.md",
    fixed: false,
    ago: "刚刚",
    text: [
      "# notes",
      "",
      "- 票 3 只做 demo，不碰 `src/`、不碰 `server/`",
      "- 真功能在下一票，人挑完方案才开工",
      "- 十格共用同一份假数据，不许各写各的",
      "",
      "## 04 那格的问题",
      "",
      "桌面上抽屉挡住正文底部 —— 写着写着要抬头找列表。手机最省力的位置，桌面上",
      "不一定顺手，这就是那一格唯一的问题。",
      "",
      "## 08 那格的问题",
      "",
      "手势不可发现。左缘内滑、列表项左滑，没人教就没人知道，所以那一格自带一次引导。",
    ].join("\n"),
  },
  {
    id: "d4",
    name: "待办.md",
    fixed: false,
    ago: "昨天",
    text: [
      "# 待办",
      "",
      "- [x] 菜单里那条「笔记」改名成「文档」",
      "- [ ] 一个节点多份 md：能新建、能改、能删",
      "- [ ] `AGENTS.md` 只能改 —— 删除入口根本不出现",
      "- [ ] 手机形态：375 外框里真的重排，不是缩小",
      "",
      "## 卡住的",
      "",
      "浅色主题下毛玻璃那道顶边高光太弱，要把 `--glass-hi` 的百分比提上去。",
      "别去发明新色值 —— 一样东西只在一个地方定。",
    ].join("\n"),
  },
];

const esc = (s) =>
  String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

const svg = (d) =>
  `<svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.5"
     stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${d}</svg>`;

const I = {
  x: svg(`<path d="M3.6 3.6l6.8 6.8M10.4 3.6l-6.8 6.8"/>`),
  trash: svg(
    `<path d="M2.6 4h8.8M5.6 4V2.9c0-.2.2-.4.4-.4h2c.2 0 .4.2.4.4V4"/><path d="M4.1 4l.5 7.1c0 .3.3.5.6.5h3.6c.3 0 .6-.2.6-.5L9.9 4"/>`,
  ),
  plus: svg(`<path d="M7 2.8v8.4M2.8 7h8.4"/>`),
  chev: svg(`<path d="M4.2 5.4 7 8.2l2.8-2.8"/>`),
  back: svg(`<path d="M8.4 3.4 4.8 7l3.6 3.6"/>`),
  search: svg(`<circle cx="6.3" cy="6.3" r="3.5"/><path d="M9 9 11.6 11.6"/>`),
  rail: svg(`<path d="M2.2 3.6h9.6M2.2 7h9.6M2.2 10.4h9.6"/>`),
  restore: svg(`<path d="M3.2 7a3.9 3.9 0 1 1 1.3 2.9"/><path d="M3.2 3.6V7h3.4"/>`),
  hand: svg(
    `<path d="M5 6.6V3.9a1 1 0 0 1 2 0v2.2"/><path d="M7 6.1V3.2a1 1 0 0 1 2 0v3.4"/><path d="M9 6.4V5a1 1 0 0 1 2 0v3.6a3.4 3.4 0 0 1-3.4 3.4H7.6a2.6 2.6 0 0 1-2.6-2.6V6.8"/>`,
  ),
  arrow: svg(`<path d="M2.6 7h8.8M7.8 3.6 11.4 7 7.8 10.4"/>`),
  more: `<svg viewBox="0 0 14 14" fill="currentColor" aria-hidden="true">
    <circle cx="3.2" cy="7" r="1.15"/><circle cx="7" cy="7" r="1.15"/><circle cx="10.8" cy="7" r="1.15"/></svg>`,
};

/* ── 文件名 sanitize：小写、空格转 `-`、补 `.md` ──────────────────────────── */
const sanitize = (raw) => {
  let s = String(raw ?? "")
    .trim()
    .toLowerCase();
  s = s.replace(/\s+/g, "-").replace(/[^\p{L}\p{N}._-]/gu, "").replace(/-+/g, "-");
  s = s.replace(/^[-.]+|[-.]+$/g, "");
  if (!s) return "";
  if (!/\.(md|markdown)$/.test(s)) s += ".md";
  return s;
};

const words = (d) => d.text.replace(/\s/g, "").length;
const lines = (d) => d.text.split("\n").length;
const preview = (d) => d.text.split("\n").find((l) => l.trim() && !l.startsWith("#")) ?? "";

const stateText = (d) =>
  d.state === "dirty" ? "编辑中" : d.state === "saving" ? "正在保存…" : "已保存";
const stateCls = (d) => (d.state === "saved" ? "saved" : d.state === "dirty" ? "dirty" : "");
const mark = (d) => (d.state === "saving" ? "◌" : "●");

const calm = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

const CARDS = [];

/* ── 卡片 ─────────────────────────────────────────────────────────────────── */

class Card {
  constructor(cfg) {
    this.cfg = cfg;
    this.docs = seed().map((d) => ({ ...d, state: "saved", trashed: false }));
    this.cur = "d2";
    this.mode = "desk";

    // 浮层状态：一次只开一个。
    this.adding = false;
    this.addErr = "";
    this.confirm = null;
    this.more = false;
    this.menu = false;
    this.pal = false;
    this.palQ = "";
    this.palCur = 0;
    this.zoom = null;
    this.drawer = null; // null | "half" | "full"
    this.rail = false;
    this.page = "list"; // 07 手机
    this.query = ""; // 07 搜索
    this.insp = false; // 06 收起
    this.guide = true; // 08 那次引导
    this.focusText = false;
    this.noteText = "";

    this.t1 = 0;
    this.t2 = 0;

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
          <p class="bet">它赌什么：<b>${esc(cfg.bet)}</b> · 已知代价：<span class="cost">${esc(cfg.cost)}</span></p>
          <div class="specs">${cfg.specs.map(([k, v]) => `<span>${k} <b>${esc(v)}</b></span>`).join("")}</div>
        </div>
      </header>
      <div class="plate">
        <div class="card-tools">
          <span class="hintlbl">${esc(cfg.hint)}</span>
          <span class="spacer"></span>
          <button class="mini" data-mode="desk" aria-pressed="true">桌面</button>
          <button class="mini" data-mode="phone" aria-pressed="false">手机</button>
        </div>
        <div class="stage-wrap"></div>
      </div>`;

    this.el = el;
    this.wrapEl = el.querySelector(".stage-wrap");

    el.addEventListener("click", (ev) => this.onClick(ev));
    el.addEventListener("keydown", (ev) => this.onKeyDown(ev));
    el.addEventListener("input", (ev) => this.onInput(ev));
    el.addEventListener("pointerdown", (ev) => this.cfg.pointer?.(this, ev));
    CARDS.push(this);
    this.render();
  }

  /* ── 读 ── */
  docOf(id) {
    return this.docs.find((d) => d.id === id) ?? null;
  }
  /** 正在被编辑的那一份 —— 03 放大时是放大出来那份。 */
  active() {
    const id = this.cfg.typingId?.(this) ?? this.cur;
    return this.docOf(id) ?? this.docs[0];
  }
  live() {
    return this.docs.filter((d) => !d.trashed);
  }
  trash() {
    return this.docs.filter((d) => d.trashed);
  }

  /* ── 写 ── */
  open(id) {
    const d = this.docOf(id);
    if (!d || d.trashed) return;
    if (this.cfg.beforeOpen?.(this, id)) return;
    this.cur = id;
    this.closeOverlays();
    this.render();
  }

  type(text) {
    const d = this.active();
    d.text = text;
    d.state = "dirty";
    clearTimeout(this.t1);
    clearTimeout(this.t2);
    this.t1 = setTimeout(() => {
      d.state = "saving";
      this.patch();
      this.t2 = setTimeout(() => {
        d.state = "saved";
        d.ago = "刚刚";
        this.patch();
      }, 620);
    }, 420);
    this.patch();
  }

  create(raw) {
    const name = sanitize(raw);
    if (!name) return this.addErrGo("名字空了 —— 至少给一个字");
    if (this.docs.some((d) => d.name.toLowerCase() === name)) {
      return this.addErrGo(`${name} 已经在了，换一个`);
    }
    const d = {
      id: `u${this.docs.length + 1}`,
      name,
      fixed: false,
      ago: "刚刚",
      state: "saved",
      trashed: false,
      text: `# ${name.replace(/\.md$/, "")}\n\n`,
    };
    this.docs.push(d);
    this.adding = false;
    this.addErr = "";
    this.cur = d.id;
    this.zoom = null;
    this.focusText = true;
    this.closeOverlays();
    this.render();
    this.note(`${name} 建好了 —— 直接写`);
  }

  /** 重名就地红字，不许 alert，输入框留着让人改。 */
  addErrGo(msg) {
    this.addErr = msg;
    this.render();
    const input = this.wrapEl.querySelector("[data-act='newname']");
    if (input) {
      input.focus();
      input.select();
    }
  }

  askDelete(id) {
    const d = this.docOf(id);
    if (!d) return;
    if (d.fixed) return this.note("AGENTS.md 只能改 —— 没有删除入口");
    this.confirm = id;
    this.render();
    this.wrapEl.querySelector("[data-act='confirm-no']")?.focus();
  }

  doDelete() {
    const d = this.docOf(this.confirm);
    this.confirm = null;
    if (!d) return this.render();
    if (d.fixed) return this.render();
    d.trashed = true;
    if (this.zoom === d.id) this.zoom = null;
    if (this.cur === d.id) {
      const next = this.live()[0];
      if (next) this.cur = next.id;
    }
    this.render();
    this.note(`${d.name} 已移到最近删除 —— 「最近删除」里能恢复`);
  }

  restore(id) {
    const d = this.docOf(id);
    if (!d) return;
    d.trashed = false;
    this.cur = d.id;
    this.render();
    this.note(`${d.name} 回来了`);
  }

  closeOverlays() {
    this.confirm = null;
    this.more = false;
    this.menu = false;
    this.pal = false;
    this.palQ = "";
    this.palCur = 0;
    this.zoom = null;
    this.adding = false;
    this.addErr = "";
  }

  /** Esc：从最上面那层往下关。未提交的编辑不丢 —— 正文一直在模型里。 */
  escape() {
    if (this.confirm) return this.dismiss(() => (this.confirm = null));
    if (this.pal) return this.dismiss(() => (this.pal = false));
    if (this.more) return this.dismiss(() => (this.more = false));
    if (this.menu) return this.dismiss(() => (this.menu = false));
    if (this.drawer) return this.dismiss(() => (this.drawer = null), ".drawer", "closing");
    if (this.zoom) return this.dismiss(() => (this.zoom = null), ".zoomer", "out");
    if (this.cfg.escape?.(this) === true) return true;
    if (this.adding) {
      this.adding = false;
      this.addErr = "";
      this.render();
      return true;
    }
    return false;
  }

  /** 退场 140ms 无过冲，退完再重画。 */
  dismiss(clear, sel = ".ovl", cls = "out") {
    const node = this.wrapEl.querySelector(sel);
    clear();
    if (!node || calm()) return this.render();
    if (cls === "out") {
      node.querySelector(".scrim")?.classList.add("out");
      node.querySelector(".sheetwrap")?.classList.add("out");
    } else {
      node.classList.add(cls);
    }
    setTimeout(() => this.render(), 160);
    return true;
  }

  note(text) {
    this.noteText = text;
    const el = this.wrapEl.querySelector(".status");
    if (el) el.textContent = text;
  }

  setMode(m) {
    this.mode = m;
    this.closeOverlays();
    if (this.cfg.id === "v7") this.page = "list";
    this.render();
  }

  /* ── 画 ── */
  render() {
    const phone = this.mode === "phone";
    const inner = phone ? this.cfg.phone(this) : this.cfg.desk(this);
    const ovl = this.overlayHTML();
    const list = this.cfg.listHTML ?? listHTML;

    const html = phone
      ? `<div class="frame mode-phone">
           <div class="statusbar"><span>9:41</span><span>Nous ▮▮▮</span></div>
           <div class="notch"></div>
           <div class="app">${inner}</div>
           <div class="homebar"></div>
           ${ovl}
         </div>
         <p class="status" aria-live="polite">${esc(this.noteText)}</p>`
      : `<div class="stage mode-desk">${inner}${ovl}</div>
         <p class="status" aria-live="polite">${esc(this.noteText)}</p>`;

    this.wrapEl.innerHTML = html;
    this._list = list;

    this.el.querySelectorAll(".card-tools [data-mode]").forEach((b) => {
      b.setAttribute("aria-pressed", String(b.dataset.mode === this.mode));
    });

    if (this.focusText) {
      this.focusText = false;
      const ta = this.wrapEl.querySelector(".ed-text");
      if (ta) {
        ta.focus();
        ta.setSelectionRange(ta.value.length, ta.value.length);
      }
    }
    this.cfg.after?.(this);
  }

  /** 打字只补状态字，不重画 —— 光标才不跳。 */
  patch() {
    const d = this.active();
    const st = this.wrapEl.querySelector(".state");
    if (st) {
      st.textContent = stateText(d);
      st.className = `state ${stateCls(d)}`;
    }
    const meta = this.wrapEl.querySelector(".ed-meta");
    if (meta) meta.innerHTML = metaHTML(d);
    for (const it of this.wrapEl.querySelectorAll(".item[data-id]")) {
      const dd = this.docOf(it.dataset.id);
      if (!dd) continue;
      const s = it.querySelector(".saved");
      if (s) s.textContent = mark(dd);
      it.classList.toggle("dirty", dd.state !== "saved");
      it.classList.toggle("saving", dd.state === "saving");
    }
  }

  overlayHTML() {
    const d = this.confirm ? this.docOf(this.confirm) : null;
    // AGENTS.md 永远拿不到这张卡片
    if (!d || d.fixed) return "";
    return `<div class="ovl"><div class="scrim" data-act="scrim"></div>
      <div class="sheetwrap"><div class="confirm">
        <div class="confirm-h">移到最近删除？</div>
        <div class="confirm-b"><b>${esc(d.name)}</b> 会挪进 <b>map/&lt;id&gt;/.trash/</b>，<br />没有真的删掉 —— 能恢复。</div>
        <div class="confirm-f">
          <button class="destructive" data-act="confirm-yes" data-id="${d.id}">移到最近删除</button>
          <button data-act="confirm-no">取消</button>
        </div>
      </div></div></div>`;
  }

  /* ── 事件 ── */
  onClick(ev) {
    const modeBtn = ev.target.closest?.("[data-mode]");
    if (modeBtn && this.el.contains(modeBtn)) return this.setMode(modeBtn.dataset.mode);

    const act = ev.target.closest?.("[data-act]");
    const kind = act?.dataset.act;
    if (!kind) return;

    // 各格自己的动作先接（menu / pal / dot / insp / drawer / rail / guide …）
    if (this.cfg.act?.(this, kind, act, ev) === true) return;

    switch (kind) {
      case "open":
        return this.open(act.dataset.id);
      case "del":
        return this.askDelete(act.dataset.id);
      case "restore":
        return this.restore(act.dataset.id);
      case "new":
        this.adding = true;
        this.addErr = "";
        this.render();
        this.wrapEl.querySelector("[data-act='newname']")?.focus();
        return;
      case "newok": {
        const input = this.wrapEl.querySelector("[data-act='newname']");
        return this.create(input ? input.value : "");
      }
      case "confirm-yes":
        return this.doDelete();
      case "confirm-no":
        return this.dismiss(() => (this.confirm = null));
      case "scrim":
        return this.escape();
      case "close":
        return this.escape();
      case "back":
        this.page = "list";
        return this.render();
      default:
        return;
    }
  }

  onKeyDown(ev) {
    const act = ev.target.closest?.("[data-act]");
    const kind = act?.dataset.act;
    if (kind === "newname") {
      if (ev.key === "Enter") {
        ev.preventDefault();
        return this.create(act.value);
      }
      if (ev.key === "Escape") {
        ev.preventDefault();
        this.adding = false;
        this.addErr = "";
        return this.render();
      }
      return;
    }
    if (this.cfg.key?.(this, ev) === true) return;
    if ((ev.key === "Enter" || ev.key === " ") && act && act.dataset.id) {
      ev.preventDefault();
      return this.onClick({ target: act });
    }
  }

  onInput(ev) {
    const kind = ev.target.dataset?.act;
    if (kind === "text") return this.type(ev.target.value);
    if (kind === "newname") {
      this.addErr = "";
      this.wrapEl.querySelector(".err")?.remove();
      return;
    }
    if (kind === "search") {
      this.query = ev.target.value;
      const list = this.wrapEl.querySelector("[data-role='list']");
      if (list) {
        list.innerHTML = this.cfg.listHTML(this);
        // 把同一个输入框还回去，光标不跳
        const next = this.wrapEl.querySelector("[data-act='search']");
        if (next) {
          next.focus();
          next.setSelectionRange(ev.target.value.length, ev.target.value.length);
        }
      }
      return;
    }
    if (kind === "pal") {
      this.palQ = ev.target.value;
      this.palCur = 0;
      const list = this.wrapEl.querySelector(".pal-l");
      if (list) list.innerHTML = palRowsHTML(this);
      return;
    }
  }
}

/* ── 共用的那几块 HTML ───────────────────────────────────────────────────── */

const metaHTML = (d) =>
  `<span>字数 ${words(d)}</span><span>行 ${lines(d)}</span><span>${esc(d.ago)}</span>`;

/** 一行文档。AGENTS.md 没有删除入口 —— 那个按钮根本不生成。 */
const itemHTML = (c, d) => {
  const sel = c.cur === d.id && !d.trashed;
  const cls = ["item", sel ? "sel" : "", d.state !== "saved" ? "dirty" : "", d.state === "saving" ? "saving" : ""];
  const del = !d.fixed && !d.trashed;
  return `<div class="${cls.join(" ")}" data-act="${d.trashed ? "none" : "open"}" data-id="${d.id}"
      role="button" tabindex="0" aria-label="${esc(d.name)}">
    <span class="dot"></span>
    <span class="nm">${esc(d.name)}</span>
    ${d.fixed ? `<span class="tag">入口</span>` : ""}
    <span class="saved">${mark(d)}</span>
    ${
      d.trashed
        ? `<button class="iact wide" data-act="restore" data-id="${d.id}" aria-label="恢复 ${esc(d.name)}">${I.restore} 恢复</button>`
        : ""
    }
    ${
      del
        ? `<button class="iact danger" data-act="del" data-id="${d.id}"
             aria-label="把 ${esc(d.name)} 移到最近删除" title="移到最近删除">${I.trash}</button>`
        : ""
    }
  </div>`;
};

/** 分组列表：入口 / 我的文档 / 最近删除。＋ 在最底下。 */
const listHTML = (c) => {
  const live = c.live();
  const fixed = live.filter((d) => d.fixed);
  const mine = live.filter((d) => !d.fixed);
  const trash = c.trash();
  return `
    <div class="grp"><div class="grp-h">入口<span class="c">${fixed.length}</span></div>
      ${fixed.map((d) => itemHTML(c, d)).join("")}</div>
    <div class="grp"><div class="grp-h">我的文档<span class="c">${mine.length}</span></div>
      ${mine.map((d) => itemHTML(c, d)).join("")}
      ${c.adding ? newrowHTML(c) : plusHTML()}</div>
    <div class="grp"><div class="grp-h">最近删除<span class="c">${trash.length}</span></div>
      ${
        trash.length
          ? trash.map((d) => itemHTML(c, d)).join("")
          : `<div class="empty">删掉的文档会落在这儿，能恢复。<br />清空回收站不做。</div>`
      }</div>`;
};

const plusHTML = () =>
  `<button class="plus" data-act="new" tabindex="0">${I.plus}<span>新建文档</span></button>`;

const newrowHTML = (c) => `
  <div class="newrow">
    <input data-act="newname" placeholder="名字，回车确认" aria-label="新文档名"
           autocapitalize="off" autocorrect="off" spellcheck="false" />
    <button class="ok" data-act="newok">新建</button>
  </div>
  ${c.addErr ? `<p class="err">${esc(c.addErr)}</p>` : ""}`;

const edBodyHTML = (d) => `
  <div class="ed-body">
    <textarea class="ed-text" data-act="text" spellcheck="false" aria-label="${esc(d.name)} 正文">${esc(d.text)}</textarea>
  </div>
  <div class="ed-meta">${metaHTML(d)}</div>`;

const stateHTML = (d) =>
  `<span class="state ${stateCls(d)}" aria-live="polite">${stateText(d)}</span>`;

const titleHTML = (d) =>
  `<div class="ed-title">${esc(d.name)}${d.fixed ? `<span class="tag">入口</span>` : ""}</div>`;

/** 编辑器。titleHTML / extra 由各格自己给（02 是菜单、09 居中、07 手机是返回）。 */
const edHTML = (c, o = {}) => {
  const d = c.active();
  return `<div class="ed">
    <div class="ed-head">
      ${o.titleHTML ?? titleHTML(d)}
      ${stateHTML(d)}
      ${o.extra ?? ""}
    </div>
    ${edBodyHTML(d)}
  </div>`;
};

/** ⋯ 菜单：给「列表不常驻」的那几格一个新建 / 删除 / 恢复的入口。 */
const moreHTML = (c) => {
  const d = c.active();
  const trash = c.trash();
  return `<div class="popover" style="position:relative">
    ${c.adding ? newrowHTML(c) : `<button class="prow" data-act="new" tabindex="0">${I.plus}<span class="nm">新建文档</span></button>`}
    ${
      d.fixed
        ? `<div class="empty" style="padding:5px 8px">AGENTS.md 只能改 —— <br />没有删除入口。</div>`
        : `<button class="prow" data-act="del" data-id="${d.id}" tabindex="0"
            style="color:var(--n-color-danger)">${I.trash}<span class="nm">移到最近删除</span></button>`
    }
    ${trash
      .map(
        (t) =>
          `<button class="prow" data-act="restore" data-id="${t.id}" tabindex="0">${I.restore}<span class="nm">恢复 ${esc(t.name)}</span></button>`,
      )
      .join("")}
  </div>`;
};

const moreBtnHTML = (c) =>
  `<button class="mini" data-act="more" aria-expanded="${c.more}" aria-label="更多操作">${I.more}</button>`;

const morePopHTML = (c) =>
  `<div class="ovl"><div class="scrim" data-act="scrim"></div>
     <div class="sheetwrap" style="justify-content:flex-end;align-items:flex-start;padding:10px 10px 0">
       <div style="width:224px">${moreHTML(c)}</div>
     </div></div>`;

/** 02 那种居中浮层 / 贴底 sheet 里的一份列表。 */
const sheetListHTML = (c) => `<div class="bsheet">${listHTML(c)}</div>`;

/* ── 01 · 侧栏列表 ───────────────────────────────────────────────────────── */

const v1 = {
  id: "v1",
  name: "侧栏列表",
  blurb:
    "左 220 毛玻璃列表（AGENTS.md 置顶分组），右编辑器。Notes / Finder 三栏的压缩版 —— 两栏始终同时在屏幕上。",
  bet: "两栏常驻，切文档永远只要一下",
  cost: "220px 永远不归正文",
  hint: "基线 · 两栏常驻",
  specs: [
    ["列表", "220px 毛玻璃"],
    ["切文档", "1 次点击"],
    ["手机", "顶部下拉"],
  ],
  desk: (c) =>
    `<div class="fx">
      <div class="side glass scroll" style="width:220px;flex:none">${listHTML(c)}</div>
      ${edHTML(c)}
    </div>`,
  phone: (c) =>
    `<div class="navbar">
      <button class="mini" data-act="menu" aria-expanded="${c.menu}">${esc(c.active().name)} ${I.chev}</button>
      <span class="spacer" style="flex:1"></span>
      ${stateHTML(c.active())}
    </div>
    <div class="fx" style="position:relative">
      ${edHTML(c)}
      ${
        c.menu
          ? `<div class="ovl"><div class="scrim" data-act="scrim"></div>
              <div class="sheetwrap" style="align-items:flex-start;padding:6px 8px 0">
                <div style="width:100%">${sheetListHTML(c)}</div>
              </div></div>`
          : ""
      }
    </div>`,
  act: (c, kind) => {
    if (kind !== "menu") return;
    if (c.menu) return c.dismiss(() => (c.menu = false)) && true;
    c.menu = true;
    c.render();
    c.wrapEl.querySelector(".bsheet .item")?.focus();
    return true;
  },
  escape: (c) => {
    if (!c.menu) return false;
    c.menu = false;
    c.render();
    return true;
  },
};

/* ── 02 · 标题菜单 ───────────────────────────────────────────────────────── */

const v2 = {
  id: "v2",
  name: "标题菜单",
  blurb:
    "没有侧栏。标题栏居中一个 chevron 菜单，点开是居中浮层列表（Apple 弹窗手感，缩放进场）。最大写作面积 —— 列表按需出现，不常驻。",
  bet: "列表按需出现，不常驻就不占地方",
  cost: "多一次点击才知道有几份文档",
  hint: "最大正文 · 菜单居中",
  specs: [
    ["列表", "居中浮层"],
    ["切文档", "2 次点击"],
    ["手机", "浮层贴底升起"],
  ],
  desk: (c) =>
    `<div class="fx" style="position:relative">
      ${edHTML(c, {
        titleHTML: `<button class="mini" data-act="menu" aria-expanded="${c.menu}"
          style="margin:0 auto">${esc(c.active().name)} ${I.chev}</button>`,
        extra: moreBtnHTML(c),
      })}
      ${c.more ? morePopHTML(c) : ""}
      ${
        c.menu
          ? `<div class="ovl"><div class="scrim" data-act="scrim"></div>
              <div class="sheetwrap" style="align-items:flex-start;padding:50px 0 0">
                <div class="popover" style="position:relative">${listHTML(c)}</div>
              </div></div>`
          : ""
      }
    </div>`,
  phone: (c) =>
    `<div class="fx" style="position:relative">
      ${edHTML(c, {
        titleHTML: `<button class="mini" data-act="menu" aria-expanded="${c.menu}"
          style="margin:0 auto">${esc(c.active().name)} ${I.chev}</button>`,
        extra: moreBtnHTML(c),
      })}
      ${c.more ? morePopHTML(c) : ""}
      ${
        c.menu
          ? `<div class="ovl"><div class="scrim" data-act="scrim"></div>
              <div class="sheetwrap" style="align-items:flex-end;padding:8px">${sheetListHTML(c)}</div>
            </div>`
          : ""
      }
    </div>`,
  act: (c, kind) => {
    if (kind !== "menu") return;
    if (c.menu) return c.dismiss(() => (c.menu = false)) && true;
    c.menu = true;
    c.render();
    c.wrapEl.querySelector(".popover .item, .bsheet .item")?.focus();
    return true;
  },
  escape: (c) => {
    if (!c.menu) return false;
    c.menu = false;
    c.render();
    return true;
  },
};

/* ── 03 · 卡片墙 ─────────────────────────────────────────────────────────── */

const cardHTML = (d) => `
  <button class="dcard" data-act="zoom" data-id="${d.id}" tabindex="0">
    <h4>${esc(d.name)}${d.fixed ? `<span class="tag">入口</span>` : ""}</h4>
    <p class="pv">${esc(preview(d))}</p>
    <span class="mt">${words(d)} 字 · ${esc(d.ago)}</span>
  </button>`;

/** 放大成编辑器：Esc 缩回卡片墙。 */
const zoomHTML = (c) => {
  const d = c.docOf(c.zoom);
  if (!d || d.trashed) return "";
  return `<div class="zoomer">
    <div class="ed">
      <div class="ed-head">
        ${titleHTML(d)}
        ${stateHTML(d)}
        <button class="mini" data-act="unzoom" aria-label="缩回卡片墙">${I.x}</button>
        ${moreBtnHTML(c)}
      </div>
      ${edBodyHTML(d)}
    </div>
  </div>`;
};

const v3 = {
  id: "v3",
  name: "卡片墙",
  blurb:
    "文档是卡片：名字 + 首段预览两行 + 字数/时间。点卡片 zoom transition 放大成编辑器，Esc 缩回。文档是作品，不是条目。",
  bet: "文档是作品 —— 先看见内容再决定进哪份",
  cost: "4 篇以上就滚起来了",
  hint: "先看见内容 · zoom 放大",
  specs: [
    ["预览", "首段 2 行"],
    ["进场", "zoom 放大"],
    ["手机", "单列"],
  ],
  desk: (c) => wallHTML(c, "repeat(auto-fill,minmax(148px,1fr))"),
  phone: (c) => wallHTML(c, "1fr"),
  typingId: (c) => c.zoom ?? c.cur,
  act: (c, kind, act) => {
    if (kind === "zoom") {
      c.zoom = act.dataset.id;
      c.render();
      c.wrapEl.querySelector(".zoomer .ed-text")?.focus();
      return true;
    }
    if (kind === "unzoom") return c.dismiss(() => (c.zoom = null), ".zoomer", "out");
  },
};

const wallHTML = (c, cols) =>
  `<div class="fx" style="position:relative">
    <div class="wall" style="grid-template-columns:${cols}">
      ${c.live().map((d) => cardHTML(d)).join("")}
    </div>
    ${zoomHTML(c)}
    ${c.more ? morePopHTML(c) : ""}
  </div>`;

/* ── 04 · 底部抽屉 ───────────────────────────────────────────────────────── */

const drawerStage = (c) =>
  `<div class="fx" style="position:relative">
    ${edHTML(c, { extra: moreBtnHTML(c) })}
    ${c.more ? morePopHTML(c) : ""}
    ${c.drawer ? "" : `<button class="fab" data-act="drawer">${I.rail} 文档 ${c.live().length}</button>`}
    ${
      c.drawer
        ? `<div class="ovl"><div class="scrim" data-act="scrim"></div></div>
           <div class="drawer" style="height:${c.drawer === "full" ? "86%" : "54%"}">
             <div class="grabber" data-grab="1" role="separator" aria-label="拖动调整高度，下拉关闭"></div>
             <div class="side scroll" style="padding-bottom:12px">${listHTML(c)}</div>
           </div>`
        : ""
    }
  </div>`;

const v4 = {
  id: "v4",
  name: "底部抽屉",
  blurb:
    "列表从底部升起：grabber、顶部 12px 圆角、下拉关闭、背景渐暗，可停在半高。这一格是手机先设计的那位 —— 桌面上照用。",
  bet: "手机上最省力的位置，桌面上也照用",
  cost: "半高时挡住正文底部",
  hint: "手机优先 · 可停半高",
  specs: [
    ["列表", "底部抽屉"],
    ["停靠", "半高 / 全高"],
    ["关闭", "下拉 / Esc"],
  ],
  desk: drawerStage,
  phone: drawerStage,
  act: (c, kind) => {
    if (kind !== "drawer") return;
    c.drawer = "half";
    c.render();
    c.wrapEl.querySelector(".drawer .item")?.focus();
    return true;
  },
  /** grabber：下拉 46px 关闭，上推 40px 变全高。全程只动 translate。 */
  pointer: (c, ev) => {
    const grab = ev.target.closest?.(".grabber");
    if (!grab || !c.drawer) return;
    ev.preventDefault();
    const drawer = grab.closest(".drawer");
    const y0 = ev.clientY;
    let dy = 0;
    grab.setPointerCapture(ev.pointerId);
    const move = (e) => {
      dy = Math.max(-140, e.clientY - y0);
      drawer.style.translate = `0 ${dy}px`;
    };
    const up = () => {
      grab.removeEventListener("pointermove", move);
      grab.removeEventListener("pointerup", up);
      grab.removeEventListener("pointercancel", up);
      drawer.style.translate = "";
      if (dy > 46) return c.dismiss(() => (c.drawer = null), ".drawer", "closing");
      const want = dy < -40 ? "full" : "half";
      if (want !== c.drawer) {
        c.drawer = want;
        c.render();
      }
    };
    grab.addEventListener("pointermove", move);
    grab.addEventListener("pointerup", up);
    grab.addEventListener("pointercancel", up);
  },
};

/* ── 05 · 顶部标签条 ─────────────────────────────────────────────────────── */

const tabsHTML = (c, bottom = false) => {
  const rows = c
    .live()
    .map((d) => {
      const sel = c.cur === d.id;
      return `<div class="tab ${sel ? "sel" : ""}" data-act="open" data-id="${d.id}"
        role="tab" tabindex="0" aria-selected="${sel}">
        <span class="nm">${esc(d.name)}</span>
        ${
          d.fixed
            ? ""
            : `<button class="x" data-act="closetab" data-id="${d.id}" aria-label="关闭 ${esc(d.name)}">${I.x}</button>`
        }
      </div>`;
    })
    .join("");
  return (
    rows +
    (bottom
      ? ""
      : `<button class="tab" data-act="new" tabindex="0" aria-label="新建文档">${I.plus}</button>`)
  );
};

const v5 = {
  id: "v5",
  name: "顶部标签条",
  blurb:
    "一行标签，非 AGENTS.md 的带 ✕。横向滚动 + 两端渐隐，选中态是填充胶囊。文档是打开着的东西，关掉是显式动作。",
  bet: "文档是「打开着的」—— 关掉要你显式做一次",
  cost: "8 个标签就没地方了",
  hint: "显式关闭 · 填充胶囊",
  specs: [
    ["列表", "一行标签"],
    ["关闭", "✕ 移到最近删除"],
    ["手机", "贴底分段控件"],
  ],
  desk: (c) =>
    `<div class="fx">
      <div class="ed" style="flex:1">
        <div class="tabbar">${tabsHTML(c)}</div>
        <div class="ed-head">${titleHTML(c.active())} ${stateHTML(c.active())} ${moreBtnHTML(c)}</div>
        ${edBodyHTML(c.active())}
      </div>
      ${c.adding ? `<div style="padding:6px 8px">${newrowHTML(c)}</div>` : ""}
      ${c.more ? morePopHTML(c) : ""}
    </div>`,
  phone: (c) =>
    `<div class="fx">
      <div class="ed">
        <div class="ed-head">${titleHTML(c.active())} ${stateHTML(c.active())} ${moreBtnHTML(c)}</div>
        ${edBodyHTML(c.active())}
      </div>
      <div class="tabbar bottom">${tabsHTML(c, true)}</div>
      ${c.adding ? `<div style="padding:6px 8px">${newrowHTML(c)}</div>` : ""}
      ${c.more ? morePopHTML(c) : ""}
    </div>`,
  act: (c, kind, act) => {
    if (kind === "closetab") {
      c.askDelete(act.dataset.id);
      return true;
    }
  },
};

/* ── 06 · 右侧 Inspector ─────────────────────────────────────────────────── */

const v6 = {
  id: "v6",
  name: "右侧 Inspector",
  blurb:
    "编辑器占满，文档列表是右侧 inspector 的一节（可收起），像 Finder 的「显示简介」。正文第一 —— 列表降级成属性面板。",
  bet: "我就是来写字的 —— 列表只配当属性",
  cost: "收起之后就忘了有几份文档",
  hint: "正文第一 · 列表可收起",
  specs: [
    ["列表", "右侧 200px"],
    ["收起", "translate 100%"],
    ["手机", "半屏 sheet"],
  ],
  desk: (c) =>
    `<div class="fx" style="position:relative">
      ${edHTML(c, { extra: `<button class="mini" data-act="insp" aria-label="收起 / 展开列表">${I.rail}</button>` })}
      <div class="insp ${c.insp ? "collapsed" : ""}" style="width:200px;flex:none">
        <div class="insp-h">文档 · ${c.live().length}</div>
        <div class="side scroll">${listHTML(c)}</div>
      </div>
      ${c.more ? morePopHTML(c) : ""}
    </div>`,
  phone: (c) =>
    `<div class="fx" style="position:relative">
      ${edHTML(c)}
      <div class="tabbar bottom" style="justify-content:center">
        <button class="tab sel" data-act="insp" style="max-width:none">${I.rail} 文档 · ${c.live().length}</button>
      </div>
      ${c.more ? morePopHTML(c) : ""}
      ${
        c.insp
          ? `<div class="ovl"><div class="scrim" data-act="scrim"></div>
              <div class="sheetwrap" style="align-items:flex-end;padding:8px">
                <div class="bsheet" style="max-height:56%">${listHTML(c)}</div>
              </div></div>`
          : ""
      }
    </div>`,
  act: (c, kind) => {
    if (kind !== "insp") return;
    c.insp = !c.insp;
    c.render();
    if (c.insp && c.mode === "phone") c.wrapEl.querySelector(".bsheet .item")?.focus();
    return true;
  },
  escape: (c) => {
    if (!c.insp) return false;
    c.insp = false;
    c.render();
    return true;
  },
};

/* ── 07 · 双滚动目录 ─────────────────────────────────────────────────────── */

const searchHTML = (c) =>
  `<div class="search">${I.search}<input data-act="search" value="${esc(c.query)}"
     placeholder="搜名字或正文" aria-label="搜索文档" autocapitalize="off" spellcheck="false" /></div>`;

const v7list = (c) => {
  const q = c.query.trim().toLowerCase();
  const hit = (d) => !q || d.name.toLowerCase().includes(q) || d.text.toLowerCase().includes(q);
  const live = c.live().filter(hit);
  const trash = c.trash().filter(hit);
  const grp = (t, arr) =>
    arr.length
      ? `<div class="grp"><div class="grp-h">${t}<span class="c">${arr.length}</span></div>${arr
          .map((d) => itemHTML(c, d))
          .join("")}</div>`
      : "";
  const body =
    grp("入口", live.filter((d) => d.fixed)) +
    grp("我的文档", live.filter((d) => !d.fixed)) +
    (c.adding ? newrowHTML(c) : plusHTML()) +
    grp("最近删除", trash);
  return (
    body ||
    `<div class="empty">没有命中「${esc(c.query)}」<br /><button class="mini" data-act="clearq">清空</button></div>`
  );
};

const v7 = {
  id: "v7",
  name: "双滚动目录",
  blurb:
    "列表 240 宽、带搜索、分组。手机上真两页：列表全屏 → 点一条进编辑 → 左上角返回。文档会涨到几十份时的样子。",
  bet: "文档会涨到几十份 —— 那时候只有搜索能救",
  cost: "手机上多一层返回栈",
  hint: "带搜索 · 手机真两页",
  specs: [
    ["列表", "240px + 搜索"],
    ["分组", "入口 / 我的 / 最近删除"],
    ["手机", "列表页 + 返回"],
  ],
  listHTML: v7list,
  desk: (c) =>
    `<div class="fx">
      <div class="side scroll" style="width:240px;flex:none;border-right:1px solid var(--n-color-border)">
        ${searchHTML(c)}
        <div data-role="list">${v7list(c)}</div>
      </div>
      ${edHTML(c, { extra: moreBtnHTML(c) })}
    </div>
    ${c.more ? morePopHTML(c) : ""}`,
  phone: (c) =>
    c.page === "list"
      ? `<div class="page scroll">
          ${searchHTML(c)}
          <div data-role="list">${v7list(c)}</div>
        </div>`
      : `<div class="fx" style="position:relative">
          <div class="ed">
            <div class="ed-head">
              <button class="mini" data-act="back" aria-label="返回列表">${I.back} 列表</button>
              ${titleHTML(c.active())}
              ${stateHTML(c.active())}
              ${moreBtnHTML(c)}
            </div>
            ${edBodyHTML(c.active())}
          </div>
          ${c.more ? morePopHTML(c) : ""}
        </div>`,
  /** 手机上点一条 = 进编辑页；桌面上直接换正文。 */
  beforeOpen: (c, id) => {
    if (c.mode !== "phone") return false;
    c.page = "edit";
    c.cur = id;
    c.closeOverlays();
    c.render();
    return true;
  },
  act: (c, kind) => {
    if (kind === "clearq") {
      c.query = "";
      c.render();
      c.wrapEl.querySelector("[data-act='search']")?.focus();
      return true;
    }
  },
  escape: (c) => {
    if (c.mode !== "phone" || c.page !== "edit") return false;
    c.page = "list";
    c.render();
    return true;
  },
};

/* ── 08 · 手势 ───────────────────────────────────────────────────────────── */

/** AGENTS.md 不给滑 —— 它压根不进 swipe 结构。 */
const swipeList = (c) => {
  const live = c.live();
  const fix = live.filter((d) => d.fixed);
  const mine = live.filter((d) => !d.fixed);
  const trash = c.trash();
  const swipe = (d) => `<div class="swipe-wrap" data-swipe="${d.id}">
      <div class="swipe-bg"><button data-act="del" data-id="${d.id}">移到最近删除</button></div>
      <div class="swipe-face">${itemHTML(c, d)}</div>
    </div>`;
  return `
    <div class="grp"><div class="grp-h">入口<span class="c">1</span></div>
      ${fix.map((d) => `<div style="padding:0 6px">${itemHTML(c, d)}</div>`).join("")}
      <div class="empty" style="padding:0 8px 6px">AGENTS.md 不给滑 —— 它没有删除入口。</div></div>
    <div class="grp"><div class="grp-h">我的文档<span class="c">${mine.length}</span></div>
      ${mine.map(swipe).join("")}
      ${c.adding ? newrowHTML(c) : plusHTML()}</div>
    <div class="grp"><div class="grp-h">最近删除<span class="c">${trash.length}</span></div>
      ${
        trash.length
          ? trash.map((d) => `<div style="padding:0 6px">${itemHTML(c, d)}</div>`).join("")
          : `<div class="empty">删掉的文档会落在这儿，能恢复。</div>`
      }</div>`;
};

const v8stage = (c) => {
  const guide = c.guide
    ? `<div class="guide">${I.hand}
        <span>从左缘往里滑拉出侧栏；列表项<b>往左滑</b>露出「移到最近删除」。
        <span class="hintarrow">${I.arrow}</span></span>
        <button class="mini" data-act="guide">知道了</button>
      </div>`
    : "";
  return `<div class="fx" style="position:relative">
    ${guide}
    <div class="ed">
      <div class="ed-head">
        <button class="mini" data-act="rail" aria-expanded="${c.rail}" aria-label="显示 / 收起列表">${I.rail}</button>
        ${titleHTML(c.active())}
        ${stateHTML(c.active())}
        ${moreBtnHTML(c)}
      </div>
      ${edBodyHTML(c.active())}
    </div>
    <div class="edge" data-edge="1" aria-hidden="true"></div>
    ${
      c.rail
        ? `<div class="ovl"><div class="scrim" data-act="scrim"></div></div>
           <div class="rail open"><div class="side glass scroll">${swipeList(c)}</div></div>`
        : ""
    }
    ${c.more ? morePopHTML(c) : ""}
  </div>`;
};

const v8 = {
  id: "v8",
  name: "手势",
  blurb:
    "左缘内滑拉出侧栏（iOS 返回手势的镜像）；列表项左滑露出删除（Apple 邮件手感）。高手顺手 —— 代价是手势不可发现，所以自带一次引导。",
  bet: "高手顺手 —— 一次滑动顶两次点击",
  cost: "手势不可发现，必须自带引导",
  hint: "手势 · 自带一次引导",
  specs: [
    ["侧栏", "左缘内滑"],
    ["删除", "列表项左滑"],
    ["手机", "同一套手势"],
  ],
  desk: v8stage,
  phone: v8stage,
  act: (c, kind) => {
    if (kind === "guide") {
      c.guide = false;
      c.render();
      return true;
    }
    if (kind === "rail") {
      c.rail = !c.rail;
      c.render();
      if (c.rail) c.wrapEl.querySelector(".rail .item")?.focus();
      return true;
    }
  },
  escape: (c) => {
    if (!c.rail) return false;
    c.rail = false;
    c.render();
    return true;
  },
  pointer: (c, ev) => {
    /* 列表项左滑 → 露出「移到最近删除」 */
    const face = ev.target.closest?.(".swipe-face");
    if (face) {
      const id = face.closest(".swipe-wrap")?.dataset.swipe;
      const d = c.docOf(id);
      if (d && !d.fixed) {
        const x0 = ev.clientX;
        let dx = 0;
        let moved = false;
        face.setPointerCapture(ev.pointerId);
        const move = (e) => {
          dx = Math.min(0, e.clientX - x0);
          if (Math.abs(dx) > 8) moved = true;
          face.style.translate = `${Math.max(-76, dx)}px 0`;
        };
        const up = () => {
          face.removeEventListener("pointermove", move);
          face.removeEventListener("pointerup", up);
          face.removeEventListener("pointercancel", up);
          face.classList.add("back");
          setTimeout(() => {
            face.classList.remove("back");
            face.style.translate = "";
          }, 300);
          if (moved && dx < -46) setTimeout(() => c.askDelete(id), 120);
          else face.style.translate = "";
        };
        face.addEventListener("pointermove", move);
        face.addEventListener("pointerup", up);
        face.addEventListener("pointercancel", up);
        return;
      }
    }
    /* 左缘内滑 → 拉出侧栏 */
    const edge = ev.target.closest?.("[data-edge]");
    if (edge && !c.rail) {
      ev.preventDefault();
      const x0 = ev.clientX;
      let dx = 0;
      edge.setPointerCapture(ev.pointerId);
      const move = (e) => {
        dx = Math.max(0, Math.min(240, e.clientX - x0));
        const rail = c.wrapEl.querySelector(".rail");
        if (rail) rail.style.translate = `${dx - 240}px 0`;
      };
      const up = () => {
        edge.removeEventListener("pointermove", move);
        edge.removeEventListener("pointerup", up);
        const want = dx > 44;
        if (!want) {
          c.rail = false;
          c.render();
        } else {
          c.rail = true;
          const rail = c.wrapEl.querySelector(".rail");
          if (rail) {
            rail.style.translate = "";
            rail.classList.add("snap");
          }
          c.render();
        }
      };
      // 拖动时先把 rail 插进 DOM，才有东西可动
      if (!c.wrapEl.querySelector(".rail")) {
        c.rail = true;
        c.render();
        c.wrapEl.querySelector(".rail").style.translate = "-240px 0";
      }
      edge.addEventListener("pointermove", move);
      edge.addEventListener("pointerup", up);
    }
  },
};

/* ── 09 · 圆点导航 ───────────────────────────────────────────────────────── */

const v9stage = (c) => {
  const docs = c.live();
  const i = docs.findIndex((d) => d.id === c.cur);
  return `<div class="fx" style="position:relative">
    <div class="ed">
      <div class="ed-head">
        ${moreBtnHTML(c)}
        ${titleHTML(c.active())}
        ${stateHTML(c.active())}
      </div>
      ${edBodyHTML(c.active())}
      <div class="dots" role="tablist" aria-label="文档">
        ${docs
          .map(
            (d, k) =>
              `<button data-act="dot" data-id="${d.id}" role="tab" aria-current="${k === i}"
                 aria-label="${esc(d.name)}"></button>`,
          )
          .join("")}
      </div>
    </div>
    ${c.more ? morePopHTML(c) : ""}
  </div>`;
};

const v9step = (c, step) => {
  const docs = c.live();
  const i = docs.findIndex((d) => d.id === c.cur);
  const j = Math.max(0, Math.min(docs.length - 1, i + step));
  if (docs[j] && docs[j].id !== c.cur) c.open(docs[j].id);
};

const v9 = {
  id: "v9",
  name: "圆点导航",
  blurb:
    "一次一篇，底部一排圆点（Pages 翻页感），⌘[ / ⌘] 切，文档名在标题栏居中。手机上左右滑切文档，圆点跟着走。最少的选择。",
  bet: "最少的选择 —— 屏幕上只有一份文档",
  cost: "想看第 3 份得先经过第 2 份",
  hint: "一次一篇 · ⌘[ ⌘]",
  specs: [
    ["导航", "底部圆点"],
    ["快捷键", "⌘[ / ⌘]"],
    ["手机", "左右滑"],
  ],
  desk: v9stage,
  phone: v9stage,
  act: (c, kind, act) => {
    if (kind === "dot") {
      c.open(act.dataset.id);
      return true;
    }
    if (kind === "prev" || kind === "next") {
      v9step(c, kind === "next" ? 1 : -1);
      return true;
    }
  },
  key: (c, ev) => {
    if ((ev.metaKey || ev.ctrlKey) && (ev.key === "[" || ev.key === "]")) {
      ev.preventDefault();
      v9step(c, ev.key === "]" ? 1 : -1);
      return true;
    }
  },
  pointer: (c, ev) => {
    if (!ev.target.closest(".ed-body")) return;
    const x0 = ev.clientX;
    let dx = 0;
    const move = (e) => {
      dx = e.clientX - x0;
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      if (Math.abs(dx) > 64) v9step(c, dx < 0 ? 1 : -1);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  },
};

/* ── 10 · 命令面板 ───────────────────────────────────────────────────────── */

const palRowsHTML = (c) => {
  const q = c.palQ.trim().toLowerCase();
  const rows = c
    .live()
    .map((d) => ({ d, hit: q ? d.text.toLowerCase().indexOf(q) : -1 }))
    .filter((r) => !q || r.d.name.toLowerCase().includes(q) || r.hit >= 0)
    .slice(0, 8);
  if (!rows.length) return `<div class="empty">没有命中「${esc(c.palQ)}」</div>`;
  return rows
    .map((r, i) => {
      const snip =
        q && r.hit >= 0
          ? r.d.text.slice(Math.max(0, r.hit - 6), r.hit + 30).replace(/\n/g, " ")
          : "";
      return `<button class="prow ${i === c.palCur ? "cur" : ""}" data-act="open" data-id="${r.d.id}"
        tabindex="0">${r.d.fixed ? `<span class="tag">入口</span>` : `<span class="dot"></span>`}
        <span class="nm">${esc(r.d.name)}</span>
        <span class="snip">${esc(snip)}</span></button>`;
    })
    .join("");
};

const palInputHTML = (c) =>
  `<div class="pal-i">${I.search}<input data-act="pal" value="${esc(c.palQ)}"
     placeholder="搜名字或正文" aria-label="搜索文档" autocapitalize="off" spellcheck="false" /></div>`;

const palHTML = (c) =>
  `<div class="ovl"><div class="scrim" data-act="scrim"></div>
    <div class="sheetwrap" style="align-items:flex-start;padding:14px 0 0">
      <div class="pal" style="position:relative">
        ${palInputHTML(c)}
        <div class="pal-l">${palRowsHTML(c)}</div>
      </div>
    </div></div>`;

const v10 = {
  id: "v10",
  name: "命令面板",
  blurb:
    "⌘K 搜名字 + 正文，回车切。列表默认完全隐藏。手机上是一颗搜索钮 → 全屏搜索页。键盘党的上限。",
  bet: "敲键盘的人不用抬手 —— 正文也能搜",
  cost: "不敲键盘就不知道有几份文档",
  hint: "⌘K · 列表默认隐藏",
  specs: [
    ["列表", "默认完全隐藏"],
    ["搜索", "名字 + 正文"],
    ["手机", "全屏搜索页"],
  ],
  desk: (c) =>
    `<div class="fx" style="position:relative">
      ${edHTML(c, { extra: `<button class="mini" data-act="pal">⌘K</button>${moreBtnHTML(c)}` })}
      ${c.more ? morePopHTML(c) : ""}
      ${c.pal ? palHTML(c) : ""}
    </div>`,
  phone: (c) =>
    c.pal
      ? `<div class="page">
          <div class="navbar"><button class="mini" data-act="close">${I.back} 返回</button></div>
          <div class="pal" style="position:relative;width:auto;margin:6px 8px;border-radius:10px">
            ${palInputHTML(c)}
          </div>
          <div class="pal-l scroll" style="max-height:none;flex:1;padding:6px 8px">${palRowsHTML(c)}</div>
        </div>`
      : `<div class="fx" style="position:relative">
          ${edHTML(c, { extra: `<button class="mini" data-act="pal" aria-label="搜索文档">${I.search}</button>${moreBtnHTML(c)}` })}
          ${c.more ? morePopHTML(c) : ""}
        </div>`,
  act: (c, kind) => {
    if (kind !== "pal") return;
    if (c.pal) return c.dismiss(() => (c.pal = false)) && true;
    c.pal = true;
    c.palQ = "";
    c.palCur = 0;
    c.render();
    return true;
  },
  key: (c, ev) => {
    if (!c.pal) return;
    if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
      ev.preventDefault();
      const n = c.wrapEl.querySelectorAll(".pal-l .prow").length;
      c.palCur = Math.max(0, Math.min(n - 1, c.palCur + (ev.key === "ArrowDown" ? 1 : -1)));
      c.wrapEl.querySelector(".pal-l").innerHTML = palRowsHTML(c);
      return true;
    }
    if (ev.key === "Enter" && ev.target.dataset?.act === "pal") {
      ev.preventDefault();
      const row = c.wrapEl.querySelectorAll(".pal-l .prow")[c.palCur];
      if (row) {
        c.pal = false;
        c.open(row.dataset.id);
      }
      return true;
    }
  },
  after: (c) => {
    if (c.pal) c.wrapEl.querySelector("[data-act='pal']")?.focus();
  },
};

/* ── 十格 ─────────────────────────────────────────────────────────────────── */

const VARIANTS = [v1, v2, v3, v4, v5, v6, v7, v8, v9, v10];

/* ── 页面壳子 ─────────────────────────────────────────────────────────────── */

const root = document.documentElement;
const deck = document.getElementById("deck");
const tabs = document.getElementById("tabs");
const themeBtn = document.getElementById("theme");
const modeSeg = document.getElementById("mode");

const chips = [{ id: "all", label: "全部" }].concat(
  VARIANTS.map((v) => ({ id: v.id, label: `${v.id.slice(1).padStart(2, "0")} ${v.name}` })),
);
tabs.innerHTML = chips
  .map(
    (ch, i) =>
      `<button class="chip" data-id="${ch.id}" aria-pressed="${i === 0}">${esc(ch.label)}</button>`,
  )
  .join("");

let globalMode = localStorage.getItem("demo-doc-mode") ?? "desk";

for (const cfg of VARIANTS) {
  const card = new Card(cfg);
  card.mode = globalMode;
  card.render();
  card.setMode(globalMode);
  deck.appendChild(card.el);
}

const paintMode = () => {
  for (const b of modeSeg.querySelectorAll("[data-mode]")) {
    b.setAttribute("aria-pressed", String(b.dataset.mode === globalMode));
  }
};
modeSeg.addEventListener("click", (ev) => {
  const b = ev.target.closest("[data-mode]");
  if (!b) return;
  globalMode = b.dataset.mode;
  localStorage.setItem("demo-doc-mode", globalMode);
  paintMode();
  for (const c of CARDS) {
    c.mode = globalMode;
    c.closeOverlays();
    if (globalMode === "phone") c.page = "list";
    c.render();
  }
});
paintMode();

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
if (deep) tabs.querySelector(`.chip[data-id="v${deep}"]`)?.click();

/* 主题：和别的 demo 同一把钥匙 */
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

/* Esc 在哪儿按都算数；⌘K / ⌘[ / ⌘] 只给看得见的那一格 */
document.addEventListener("keydown", (ev) => {
  if (ev.key === "Escape") {
    for (const c of CARDS) c.escape();
    return;
  }
  const key = ev.key.toLowerCase();
  if ((ev.metaKey || ev.ctrlKey) && key === "k") {
    const c = CARDS.find((x) => x.cfg.id === "v10" && !x.el.hidden);
    if (c && !c.pal) {
      ev.preventDefault();
      c.onClick({ target: c.el.querySelector("[data-act='pal']") });
    }
    return;
  }
  if ((ev.metaKey || ev.ctrlKey) && (ev.key === "[" || ev.key === "]")) {
    const c = CARDS.find((x) => x.cfg.id === "v9" && !x.el.hidden);
    if (c) c.cfg.key?.(c, ev);
  }
});

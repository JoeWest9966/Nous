/* 主题变量（颜色/间距/圆角）走 app 的 tokens 本体，不在这里复制一份。
   用 import 让 vite 自己注入 <style>：直接 <link> 到 /src/*.css 在 dev 下会被当模块处理，拿到空表。 */
import "../../src/tokens/tokens.css";

/* 八个方案的样板。按钮全部 onClick = 什么都不做 —— 这一轮只交付动效。 */

const icon = (d, extra = "") =>
  `<svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.5"
        stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" ${extra}>${d}</svg>`;

const I = {
  rename: icon(`<path d="M9.2 2.4 11.6 4.8 5.4 11H3V8.6z"/>`),
  copy: icon(`<rect x="2.5" y="2.5" width="7" height="7" rx="1.6"/>
              <path d="M5.5 11.5h4.4a1.6 1.6 0 0 0 1.6-1.6V5.5"/>`),
  link: icon(`<path d="M5.8 8.2a2.5 2.5 0 0 0 3.5 0l1.9-1.9a2.5 2.5 0 0 0-3.5-3.5l-.5.5"/>
              <path d="M8.2 5.8a2.5 2.5 0 0 0-3.5 0L2.8 7.7a2.5 2.5 0 0 0 3.5 3.5l.5-.5"/>`),
  trash: icon(`<path d="M2.6 4h8.8M5.6 4V2.9c0-.2.2-.4.4-.4h2c.2 0 .4.2.4.4V4"/>
               <path d="M4.1 4l.5 7.1c0 .3.3.5.6.5h3.6c.3 0 .6-.2.6-.5L9.9 4"/>`),
  plus: icon(`<path d="M7 2.6v8.8M2.6 7h8.8"/>`),
  chev: icon(`<path d="M5 3.5 8.5 7 5 10.5"/>`, 'width="12" height="12"'),
  dots: `<svg viewBox="0 0 14 14" fill="currentColor" aria-hidden="true">
           <circle cx="7" cy="2.8" r="1.15"/><circle cx="7" cy="7" r="1.15"/>
           <circle cx="7" cy="11.2" r="1.15"/></svg>`,
};

const ROWS = [
  ["设计系统", "128"],
  ["Token 定义与校验", "42"],
  ["组件库 · 交互层", "310"],
  ["性能预算", "8"],
];

/** 一行。acts 由各方案自己给。`idx` 是按钮序号，写给 CSS 做 stagger。 */
const row = (name, value, acts, cls = "") => `
  <div class="row ${cls}" tabindex="0">
    <span class="chev">${I.chev}</span>
    <span class="name">${name}</span>
    <span class="value">${value}</span>
    ${acts}
  </div>`;

const list = (actsFor, rowCls = "") =>
  `<div class="list">${ROWS.map(([n, v], i) => row(n, v, actsFor(i), rowCls)).join("")}</div>`;

const panel = (body) => `
  <div class="panel">
    <div class="panel-head"><span>结构</span><span>值</span></div>
    ${body}
  </div>`;

/** 一组图标按钮，自动带 --i 序号（CSS 靠它错开时间）。 */
const acts = (items, cls = "") =>
  `<div class="acts ${cls}">${items
    .map(([k, label], i) =>
      k === "|"
        ? `<span class="hair"></span>`
        : `<button class="act ${k === "trash" ? "danger" : ""}" style="--i:${i}"
             aria-label="${label}" title="${label}">${I[k]}</button>`,
    )
    .join("")}</div>`;

const V = [
  {
    id: "v1",
    name: "玻璃胶囊",
    blurb:
      "一整条 999px 的磨砂胶囊从右边挤进来，回弹 3%。里面三个动作、一条发丝线、一个删除。只动一个元素 —— 最不容易出错的那种漂亮。",
    specs: [
      ["入场", "420ms · spring 420/30"],
      ["过冲", "3.2%"],
      ["退场", "140ms 无过冲"],
      ["材质", "blur 22 · sat 180"],
    ],
    body: () => panel(list(() => acts([["rename", "重命名"], ["copy", "复制"], ["link", "链接"], ["|"], ["trash", "删除"]]))),
  },
  {
    id: "v2",
    name: "独立气泡",
    blurb:
      "四颗各自独立的玻璃圆，错开 22ms 依次弹入，出去的时候一起走。有节奏感，代价是四个 backdrop-filter 层。",
    specs: [
      ["入场", "420ms · spring"],
      ["stagger", "22ms"],
      ["退场", "140ms 齐步"],
    ],
    body: () => panel(list(() => acts([["rename", "重命名"], ["copy", "复制"], ["link", "链接"], ["trash", "删除"]]))),
  },
  {
    id: "v3",
    name: "一点扇开",
    blurb:
      "四个按钮不是从旁边平移进来，而是从右边缘同一个点长出来，一步 28px。位移和缩放同时走弹簧，看着像材料化开。",
    specs: [
      ["入场", "420ms · spring"],
      ["展开", "28px / 颗"],
      ["stagger", "16ms"],
    ],
    body: () => panel(list(() => acts([["rename", "重命名"], ["copy", "复制"], ["link", "链接"], ["trash", "删除"]], "burst"))),
  },
  {
    id: "v4",
    name: "原位替换",
    blurb:
      "不是浮层，是换班：数值往上淡出，三个裸图标从下面顶上它的位置。最安静 —— 看不出「多出来一个东西」，只看出这行变了。",
    specs: [
      ["入场", "300ms · spring"],
      ["退场", "120ms"],
      ["材质", "无，纯图标"],
    ],
    body: () =>
      panel(list(() => acts([["rename", "重命名"], ["copy", "复制"], ["trash", "删除"]]))),
  },
  {
    id: "v5",
    name: "材质升华",
    blurb:
      "模糊从 8px 化开、上浮 7px，整行同时被抬起（内高光 + 外投影）。最「高级」也最贵：filter: blur 不合成，块多的时候别用。",
    specs: [
      ["入场", "514ms · spring 软"],
      ["blur", "8px → 0"],
      ["退场", "140ms"],
      ["注意", "贵"],
    ],
    body: () =>
      panel(list(() => acts([["rename", "重命名"], ["copy", "复制"], ["trash", "删除"]]))),
  },
  {
    id: "v6",
    name: "两段式 ···",
    blurb:
      "块上只多一个小东西。指上去 180ms 后，它长成一颗带文字的胶囊，标签 30ms 依次滑进来。iOS 那套「收起重量」的做法。",
    specs: [
      ["第一段", "••• 420ms · spring"],
      ["第二段", "260ms 展开"],
      ["stagger", "30ms"],
    ],
    body: () =>
      panel(
        list(
          () => `
        <div class="acts">
          <button class="act lbl" style="--i:0">重命名</button>
          <button class="act lbl" style="--i:1">复制</button>
          <button class="act lbl danger" style="--i:2">删除</button>
          <button class="act more" aria-label="更多操作">${I.dots}</button>
        </div>`,
        ),
      ),
  },
  {
    id: "v7",
    name: "三点 → 浮层菜单",
    blurb:
      "三个以上就不该是一排图标了。点••• 弹出一块 12px 圆角玻璃菜单，从按钮右上角 scale 展开，条目 30ms 依次落下。点外面或按 Esc 关。",
    specs: [
      ["•••", "420ms · spring"],
      ["菜单", "320ms · spring"],
      ["条目", "stagger 30ms"],
      ["关", "120ms"],
    ],
    body: () =>
      panel(
        list(
          () => `<div class="acts">
                   <button class="act kebab" aria-label="更多操作" aria-expanded="false" aria-haspopup="menu">${I.dots}</button>
                 </div>`,
        ),
      ),
  },
  {
    id: "v8",
    name: "光标起点的胶囊",
    blurb:
      "和 01 一样的胶囊，但展开的原点跟着你从行里哪一点进来走。方向和位置都有了理由 —— 最不显眼，也最不容易错。",
    specs: [
      ["入场", "380ms · spring"],
      ["原点", "跟随光标 x"],
      ["退场", "140ms"],
    ],
    body: () => panel(list(() => acts([["rename", "重命名"], ["copy", "复制"], ["trash", "删除"]]))),
  },
  {
    id: "v9",
    name: "长按生成",
    blurb:
      "按住 350ms：行先抬起、底下走一条进度发丝线，满了按钮才从右往左长出来；松手立刻收。触屏上唯一靠谱的入口 —— 手指没有 hover。",
    specs: [
      ["按住", "350ms"],
      ["容差", "移动 10px 内算长按"],
      ["入场", "420ms · spring"],
    ],
    body: () => panel(list(() => acts([["rename", "重命名"], ["copy", "复制"], ["trash", "删除"]]))),
  },
  {
    id: "v10",
    name: "慢悬停 peek",
    blurb:
      "悬停满 440ms 才浮出，扫过去的 200ms 里什么都不出。列表越密越该这么干 —— 不然鼠标一划，满屏按钮跟着闪。",
    specs: [
      ["等待", "440ms"],
      ["入场", "420ms · spring"],
      ["退场", "立即"],
    ],
    body: () => panel(list(() => acts([["rename", "重命名"], ["copy", "复制"], ["trash", "删除"]]))),
  },
  {
    id: "v11",
    name: "磁性吸附",
    blurb:
      "光标进到行里，胶囊朝你这边挪最多 7px 再回去。幅度小到说不上来哪儿不对，手指确实少走了一截。苹果官网那批按钮的招。",
    specs: [
      ["跟随", "±7px"],
      ["阻尼", "200ms ease-out"],
      ["入场", "420ms · spring"],
    ],
    body: () => panel(list(() => acts([["rename", "重命名"], ["copy", "复制"], ["link", "链接"], ["trash", "删除"]]))),
  },
  {
    id: "v12",
    name: "径向菜单",
    blurb:
      "三个圆从行右缘一点沿弧线散开。好看，但在 32px 高的行里它没有生存空间 —— 一展开就压住上一行。这是个故意留着当反例的。",
    specs: [
      ["半径", "42px"],
      ["张开", "24° / 56°"],
      ["stagger", "45ms"],
      ["结论", "别用"],
    ],
    body: () =>
      panel(
        list(
          () => `<div class="acts">
                 <button class="act" style="--i:0;--dx:0px;--dy:0px" aria-label="重命名">${I.rename}</button>
                 <button class="act" style="--i:1;--dx:-38px;--dy:-14px" aria-label="复制">${I.copy}</button>
                 <button class="act" style="--i:2;--dx:-24px;--dy:-34px" aria-label="删除">${I.trash}</button>
               </div>`,
        ),
      ),
  },
  {
    id: "v13",
    name: "层叠卡片",
    blurb:
      "按钮叠成一摞牌，带 4° 一档的偏角；指上去像扇子一样摊平展开，走开再叠回去。收起状态下你还能数出有几张。",
    specs: [
      ["叠角", "4°/张"],
      ["展开", "28px/张"],
      ["stagger", "25ms"],
    ],
    body: () =>
      panel(
        list(
          () => `<div class="acts">
                 <button class="act" style="--i:0" aria-label="重命名">${I.rename}</button>
                 <button class="act" style="--i:1" aria-label="复制">${I.copy}</button>
                 <button class="act" style="--i:2" aria-label="链接">${I.link}</button>
                 <button class="act" style="--i:3" aria-label="删除">${I.trash}</button>
               </div>`,
        ),
      ),
  },
  {
    id: "v14",
    name: "点击才展开",
    blurb:
      "hover 只负责告诉你「这儿有东西」：一颗 20px 的圆点。点它才胀开成三颗。触屏和鼠标共用一套，不怕手抖误触。",
    specs: [
      ["hover", "圆点 420ms"],
      ["展开", "280ms · spring"],
      ["收起", "点外面 / 走开"],
    ],
    body: () =>
      panel(
        list(
          () => `<div class="acts">
                 <button class="act dot" aria-label="更多操作" aria-expanded="false">${I.plus}</button>
                 <button class="act more" style="--i:0" aria-label="重命名">${I.rename}</button>
                 <button class="act more" style="--i:1" aria-label="复制">${I.copy}</button>
                 <button class="act more" style="--i:2" aria-label="删除">${I.trash}</button>
               </div>`,
        ),
      ),
  },
  {
    id: "v15",
    name: "贴名字",
    blurb:
      "按钮不贴右边缘，而是紧跟名字后面 4px —— 你正在看的是名字，按钮就该出现在名字旁边。完全不去碰数值那一列。",
    specs: [
      ["位置", "名字尾 +4px"],
      ["入场", "420ms · spring"],
      ["代价", "每行量一次宽度"],
    ],
    body: () => panel(list(() => acts([["rename", "重命名"], ["copy", "复制"], ["trash", "删除"]]))),
  },
  {
    id: "v16",
    name: "跟随光标",
    blurb:
      "一条迷你工具条横向跟着鼠标走，贴在光标左边 24px，永远夹在行内。macOS 选中文字后浮出的那条东西。",
    specs: [
      ["跟随", "200ms 阻尼"],
      ["偏移", "光标左侧 24px"],
      ["夹住", "行内"],
    ],
    body: () => panel(list(() => acts([["rename", "重命名"], ["copy", "复制"], ["trash", "删除"]]))),
  },
  {
    id: "v17",
    name: "底部抽屉",
    blurb:
      "工具条从行底下滑出来，往下一压，挂在行的下边缘上，不遮自己这行。iOS 底部栏的微缩版。会盖住下一行的右上角，这是它的代价。",
    specs: [
      ["方向", "从下往上"],
      ["入场", "340ms · spring"],
      ["位置", "行下缘 22px"],
    ],
    body: () => panel(list(() => acts([["rename", "重命名"], ["copy", "复制"], ["trash", "删除"]]))),
  },
  {
    id: "v18",
    name: "液体融合",
    blurb:
      "三个圆靠 SVG goo filter 粘成一滴，指上去撕开、走开再融合回去。最抢眼的一个，也最不适合正经界面 —— 但它说明「形」是可以动的。",
    specs: [
      ["filter", "blur 6 → 对比 22"],
      ["分离", "28px/颗"],
      ["入场", "480ms · spring"],
    ],
    body: () =>
      panel(
        list(
          () => `<div class="blobs" aria-hidden="true">
                   <i style="--i:0"></i><i style="--i:1"></i><i style="--i:2"></i>
                 </div>
                 <div class="acts">
                   <button class="act" style="--i:0" aria-label="重命名">${I.rename}</button>
                   <button class="act" style="--i:1" aria-label="复制">${I.copy}</button>
                   <button class="act" style="--i:2" aria-label="删除">${I.trash}</button>
                 </div>`,
        ),
      ),
  },
  {
    id: "v19",
    name: "数值让位",
    blurb:
      "数值自己往左挪 84px 腾地方，按钮直接坐进它原来的位置。不是浮层，是这行的内容重新排了一次 —— 但用的是 transform，所以不重排。名字长到顶住数值的时候这招就废了。",
    specs: [
      ["让位", "84px"],
      ["入场", "420ms · spring"],
      ["代价", "名字不能太长"],
    ],
    body: () =>
      panel(
        `<div class="list">${[
          ["设计系统", "128"],
          ["Token", "42"],
          ["组件库", "310"],
          ["性能预算", "8"],
        ]
          .map(([n, v]) =>
            row(n, v, acts([["rename", "重命名"], ["copy", "复制"], ["trash", "删除"]])),
          )
          .join("")}</div>`,
      ),
  },
];

/* ── 渲染 ─────────────────────────────────────────────────────────────────── */
const deck = document.getElementById("deck");

deck.innerHTML = V.map(
  (v) => `
  <section class="card ${v.id}" id="${v.id}" data-id="${v.id}">
    <header class="card-head">
      <span class="num">${v.id.slice(1).padStart(2, "0")}</span>
      <div>
        <h2>${v.name}</h2>
        <p>${v.blurb}</p>
        <div class="specs">
          ${v.specs.map(([k, val]) => `<span>${k} <b>${val}</b></span>`).join("")}
        </div>
      </div>
    </header>
    <div class="plate">${v.body()}</div>
  </section>`,
).join("");

/* ── 主题切换 ─────────────────────────────────────────────────────────────── */
const root = document.documentElement;
const themeBtn = document.getElementById("theme");
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

/* ── 热区 / 全部切换 ──────────────────────────────────────────────────────── */
const debugBtn = document.getElementById("debug");
debugBtn.addEventListener("click", () => {
  const on = document.body.classList.toggle("debug");
  debugBtn.setAttribute("aria-pressed", String(on));
});

const chips = [...document.querySelectorAll(".chip")];
const cards = [...deck.querySelectorAll(".card")];
chips.forEach((chip) => {
  chip.addEventListener("click", () => {
    const want = chip.dataset.id;
    chips.forEach((c) => c.setAttribute("aria-pressed", String(c === chip)));
    cards.forEach((card) => (card.hidden = want !== "all" && card.dataset.id !== want));
    window.scrollTo({ top: 0, behavior: "instant" });
  });
});

/* ── 全部展开：不靠鼠标也能把八个方案的形状摆在一起比 ─────────────────────── */
const pinBtn = document.getElementById("pin");
let pinned = false;
const setPin = (on) => {
  pinned = on;
  pinBtn.setAttribute("aria-pressed", String(on));
  pinBtn.textContent = on ? "收起全部" : "全部展开";
  deck.querySelectorAll(".row").forEach((r) => r.classList.toggle("pin", on));
  deck.querySelectorAll(".v6 .acts").forEach((a) => a.classList.toggle("pin", on));
};
pinBtn.addEventListener("click", () => setPin(!pinned));
if (new URLSearchParams(location.search).has("pin")) setPin(true);

/* ── 08：入场原点跟着光标 ─────────────────────────────────────────────────── */
deck.querySelectorAll(".v8 .row").forEach((r) => {
  r.addEventListener("pointerenter", (e) => {
    const box = r.getBoundingClientRect();
    // 只取右侧 40% 这一段：光标在名字上时不该从名字那儿长出来
    const x = Math.min(Math.max((e.clientX - box.left) / box.width, 0.55), 1);
    r.querySelector(".acts").style.setProperty("--ox", `${(x * 100).toFixed(1)}%`);
  });
});

/* ── 09：长按生成 ─────────────────────────────────────────────────────────── */
document.querySelectorAll(".v9 .row").forEach((row) => {
  let timer = null;
  let sx = 0;
  let sy = 0;
  const reset = () => {
    clearTimeout(timer);
    timer = null;
    row.classList.remove("holding", "held");
  };
  row.addEventListener("pointerdown", (e) => {
    if (e.target.closest("button")) return;
    if (e.pointerType === "mouse" && e.button !== 0) return;
    sx = e.clientX;
    sy = e.clientY;
    row.classList.add("holding");
    timer = setTimeout(() => {
      row.classList.add("held");
      timer = null;
    }, 350);
  });
  // 手指/鼠标在 350ms 内跑掉 10px 就是在滚动或选中，不算长按
  row.addEventListener("pointermove", (e) => {
    if (!row.classList.contains("holding")) return;
    if (Math.hypot(e.clientX - sx, e.clientY - sy) > 10) reset();
  });
  ["pointerup", "pointercancel", "pointerleave"].forEach((ev) => row.addEventListener(ev, reset));
});

/* ── 11：磁性吸附 ─────────────────────────────────────────────────────────── */
document.querySelectorAll(".v11 .row").forEach((row) => {
  const layer = row.querySelector(".acts");
  row.addEventListener("pointermove", (e) => {
    const a = layer.getBoundingClientRect();
    const d = (e.clientX - (a.left + a.width / 2)) / 22;
    layer.style.setProperty("--mx", `${Math.max(-7, Math.min(7, d)).toFixed(2)}px`);
  });
  row.addEventListener("pointerleave", () => layer.style.setProperty("--mx", "0px"));
});

/* ── 14：点击才展开 ───────────────────────────────────────────────────────── */
document.querySelectorAll(".v14 .row").forEach((row) => {
  const dot = row.querySelector(".dot");
  const close = () => {
    row.classList.remove("open");
    dot.setAttribute("aria-expanded", "false");
  };
  dot.addEventListener("click", (e) => {
    e.stopPropagation();
    const open = row.classList.toggle("open");
    dot.setAttribute("aria-expanded", String(open));
  });
  row.addEventListener("pointerleave", close);
  document.addEventListener("click", (e) => {
    if (!row.contains(e.target)) close();
  });
});

/* ── 15：贴名字 ──────────────────────────────────────────────────────────────
   量名字尾端的 x。pointerenter 时量一次就够，但首屏（和「全部展开」模式）也得是对的，
   所以渲染完、字体加载完、窗口变了都补量一次。 */
const placePlain = (row) => {
  const name = row.querySelector(".name");
  const layer = row.querySelector(".acts");
  const room = row.offsetWidth - (layer.offsetWidth || 82) - 6;
  const x = Math.min(name.offsetLeft + name.offsetWidth + 4, room);
  layer.style.setProperty("--ax", `${Math.max(x, name.offsetLeft + 12)}px`);
};
const placeAllPlain = () => document.querySelectorAll(".v15 .row").forEach(placePlain);
placeAllPlain();
document.fonts?.ready.then(placeAllPlain);
window.addEventListener("resize", placeAllPlain);
document
  .querySelectorAll(".v15 .row")
  .forEach((row) => row.addEventListener("pointerenter", () => placePlain(row)));

/* ── 16：跟随光标 ─────────────────────────────────────────────────────────── */
document.querySelectorAll(".v16 .row").forEach((row) => {
  const layer = row.querySelector(".acts");
  const place = (clientX) => {
    const rb = row.getBoundingClientRect();
    const w = layer.getBoundingClientRect().width || 90;
    const x = Math.max(4, Math.min(clientX - rb.left - w - 24, rb.width - w - 4));
    layer.style.setProperty("--fx", `${x.toFixed(1)}px`);
  };
  row.addEventListener("pointerenter", (e) => place(e.clientX));
  row.addEventListener("pointermove", (e) => place(e.clientX));
});

/* ── 18：goo filter 得在文档里才能被 url(#goo) 引用 ───────────────────────── */
const gooSvg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
gooSvg.setAttribute("aria-hidden", "true");
gooSvg.style.cssText = "position:absolute;width:0;height:0;overflow:hidden";
gooSvg.innerHTML = `<defs>
  <filter id="goo" color-interpolation-filters="sRGB">
    <feGaussianBlur in="SourceGraphic" stdDeviation="6" result="b" />
    <feColorMatrix in="b" mode="matrix"
      values="1 0 0 0 0  0 1 0 0 0  0 0 1 0 0  0 0 0 24 -10" />
  </filter>
</defs>`;
document.body.appendChild(gooSvg);

/* ── 07：浮层菜单 ─────────────────────────────────────────────────────────── */
const menu = document.createElement("div");
menu.className = "menu";
menu.setAttribute("role", "menu");
menu.dataset.open = "false";
menu.innerHTML = `
  <button role="menuitem" style="--i:0">${I.rename}重命名</button>
  <button role="menuitem" style="--i:1">${I.copy}复制</button>
  <button role="menuitem" style="--i:2">${I.link}复制链接</button>
  <div class="sep"></div>
  <button role="menuitem" class="danger" style="--i:3">${I.trash}删除块</button>`;
document.body.appendChild(menu);

let openedFor = null;
const closeMenu = () => {
  if (!openedFor) return;
  menu.dataset.open = "false";
  openedFor.setAttribute("aria-expanded", "false");
  openedFor.closest(".row")?.classList.remove("menu-open");
  openedFor = null;
};

const openMenu = (btn) => {
  if (openedFor === btn) return closeMenu();
  closeMenu();
  const b = btn.getBoundingClientRect();
  // 贴着按钮的右上角，但要被视口夹住 —— 菜单探出屏幕比菜单偏几像素更难看
  const left = Math.min(Math.max(b.right - 188, 8), window.innerWidth - 196);
  const top = b.bottom + 6 + 4 * 30 + 12 > window.innerHeight ? b.top - 6 - (4 * 30 + 20) : b.bottom + 6;
  menu.style.top = `${top}px`;
  menu.style.left = `${left}px`;
  menu.style.transformOrigin = "100% 0";
  // 下一帧再改状态，才有进场动效（同一帧改等于没动画）
  requestAnimationFrame(() => {
    menu.dataset.open = "true";
    btn.setAttribute("aria-expanded", "true");
  });
  btn.closest(".row").classList.add("menu-open");
  openedFor = btn;
};

deck.querySelectorAll(".v7 .act.kebab").forEach((btn) => {
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    openMenu(btn);
  });
});

document.addEventListener("click", (e) => {
  if (openedFor && !menu.contains(e.target)) closeMenu();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeMenu();
});
// 滚动 / 缩窗口的时候菜单会跟按钮脱节，直接关掉比让它漂着好
window.addEventListener("scroll", () => closeMenu(), { passive: true });
window.addEventListener("resize", () => closeMenu());

/* 所有按钮都不做事 —— 这一轮只有动效。 */
document.addEventListener("click", (e) => {
  const b = e.target.closest("button");
  if (b && !b.classList.contains("kebab") && !b.classList.contains("btn") && !b.classList.contains("chip")) {
    e.preventDefault();
  }
});

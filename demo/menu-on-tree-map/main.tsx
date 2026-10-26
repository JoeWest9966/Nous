/**
 * 07 号方案（竖向三点 → 浮层菜单）装在真盘上。
 *
 * 和 13 号那版的分工：那边是「一排按钮」，这边是「一个菜单」——
 * 三个以上动作就不该排图标了，尤其盘面上的块根本排不下。
 *
 * 同一个菜单挂两处：
 *   左列的行   —— 22px 玻璃 •••，顶掉应用自带的那个 ×（一行里两个删除太蠢）
 *   盘面上的块 —— 贴块的右上角，尺寸跟着块走（块小到 30px 时它是 14px）
 *
 * 菜单是 position: fixed 的 —— 块上那个 overflow:hidden 切不掉它，这是 07 能上块的关键。
 * src/ 一行没改，全是运行时注入。
 */
import React, { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "../../src/tokens/theme.css";
import "../../src/motion.css";
import { applyTheme, readTheme } from "../../src/tokens";
import "./menu.css";

applyTheme(readTheme());

/* 应用默认把左栏收起来（localStorage 里没记录就是关的）。这页要演示行上的 •••，
   所以开机先把它掰开 —— 用的还是应用自己那个开关，你随时可以再点收起。 */
try {
  localStorage.setItem("n-sidebar-open", "1");
} catch {
  /* 无痕模式下写不了，无妨 */
}

const svg = (path: string) =>
  `<svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.5"
     stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${path}</svg>`;

/* ── 菜单本身：一个，共享 ──────────────────────────────────────────────────── */
const ITEMS: [string, string, string][] = [
  ["重命名", "rename", `<path d="M9.2 2.4 11.6 4.8 5.4 11H3V8.6z"/>`],
  ["复制", "copy", `<rect x="2.5" y="2.5" width="7" height="7" rx="1.6"/>
                    <path d="M5.5 11.5h4.4a1.6 1.6 0 0 0 1.6-1.6V5.5"/>`],
  ["复制链接", "link", `<path d="M5.8 8.2a2.5 2.5 0 0 0 3.5 0l1.9-1.9a2.5 2.5 0 0 0-3.5-3.5l-.5.5"/>
                        <path d="M8.2 5.8a2.5 2.5 0 0 0-3.5 0L2.8 7.7a2.5 2.5 0 0 0 3.5 3.5l.5-.5"/>`],
];
const TRASH = `<path d="M2.6 4h8.8M5.6 4V2.9c0-.2.2-.4.4-.4h2c.2 0 .4.2.4.4V4"/>
               <path d="M4.1 4l.5 7.1c0 .3.3.5.6.5h3.6c.3 0 .6-.2.6-.5L9.9 4"/>`;

const menu = document.createElement("div");
menu.className = "ctx-menu";
menu.setAttribute("role", "menu");
menu.dataset.open = "false";
menu.innerHTML =
  ITEMS.map(
    ([label, , path], i) =>
      `<button role="menuitem" tabindex="-1" style="--i:${i}">${svg(path)}${label}</button>`,
  ).join("") +
  `<div class="ctx-sep"></div>
   <button role="menuitem" tabindex="-1" class="ctx-danger" style="--i:${ITEMS.length}">${svg(TRASH)}删除</button>`;
document.body.appendChild(menu);

const DOTS = `<circle cx="3" cy="7" r="1.25" fill="currentColor" stroke="none"/>
              <circle cx="7" cy="7" r="1.25" fill="currentColor" stroke="none"/>
              <circle cx="11" cy="7" r="1.25" fill="currentColor" stroke="none"/>`;

const trigger = () => {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "mtg";
  b.setAttribute("aria-haspopup", "menu");
  b.setAttribute("aria-expanded", "false");
  b.setAttribute("aria-label", "更多操作");
  b.title = "更多操作";
  b.innerHTML = svg(DOTS);
  return b;
};

/* ── 开 / 关 ───────────────────────────────────────────────────────────────── */
let openFor: HTMLElement | null = null;
let openHost: HTMLElement | null = null;
let raf = 0;

/** 摆位置：右对齐到按钮，探出视口就往回收；下面放不下就翻上去。 */
const place = (btn: HTMLElement) => {
  const r = btn.getBoundingClientRect();
  const W = menu.offsetWidth || 188;
  const H = menu.offsetHeight || 140;
  const left = Math.min(Math.max(r.right - W, 8), window.innerWidth - W - 8);
  const below = r.bottom + 6;
  const above = r.top - 6 - H;
  const goUp = below + H > window.innerHeight - 8 && above > 8;
  menu.style.left = `${left}px`;
  menu.style.top = `${goUp ? above : below}px`;
  menu.style.transformOrigin = `100% ${goUp ? "100%" : "0"}`;
};

/**
 * 开着的时候每帧跟着按钮走。
 *
 * 一开始是「滚一下就关」，结果发现点开菜单本身会让浏览器 focus 到按钮、顺手滚一下，
 * 菜单刚开就被自己的 scroll 监听关掉了。跟着走没有这个问题，而且本来就该跟着走。
 */
const follow = () => {
  if (!openFor) return;
  const r = openFor.getBoundingClientRect();
  if (r.bottom < 0 || r.top > window.innerHeight || r.right < 0 || r.left > window.innerWidth) {
    close();
    return;
  }
  place(openFor);
  raf = requestAnimationFrame(follow);
};

const close = () => {
  cancelAnimationFrame(raf);
  if (!openFor) return;
  menu.dataset.open = "false";
  openFor.setAttribute("aria-expanded", "false");
  openHost?.classList.remove("menu-open");
  openFor = null;
  openHost = null;
};

const open = (btn: HTMLElement) => {
  if (openFor === btn) return close();
  close();

  place(btn);
  // 同一帧里改 left/top 和 data-open 等于没有进场动效，得等下一帧
  requestAnimationFrame(() => {
    menu.dataset.open = "true";
  });

  btn.setAttribute("aria-expanded", "true");
  (btn.parentElement as HTMLElement).classList.add("menu-open");
  openFor = btn;
  openHost = btn.parentElement as HTMLElement;
  cancelAnimationFrame(raf);
  raf = requestAnimationFrame(follow);
};

/* ── 挂到行和块上 ─────────────────────────────────────────────────────────── */
/** 块的尺寸决定 ••• 的大小：30px 的块塞不下 22px 的按钮。 */
const resize = new ResizeObserver((entries) => {
  for (const e of entries) {
    const el = e.target as HTMLElement;
    const w = el.offsetWidth;
    el.dataset.size = w < 44 ? "xs" : w < 72 ? "s" : "m";
  }
});

const attach = (host: HTMLElement, kind: "row" | "block") => {
  if (host.dataset.menu === "1") return;
  host.dataset.menu = "1";
  host.classList.add("menu-host");
  if (kind === "block") host.classList.add("menu-host--block");
  const btn = trigger();
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    e.preventDefault();
    open(btn);
  });
  host.appendChild(btn);
  if (kind === "block") resize.observe(host);
};

const scan = () => {
  document.querySelectorAll<HTMLElement>('li[role="treeitem"]').forEach((li) => {
    if (li.querySelector(":scope > div[data-node-id]")) attach(li, "row");
  });
  // 盘面上的块：group/leaf 那层是 overflow-hidden 的绝对定位盒子
  document.querySelectorAll<HTMLElement>('[class*="group/leaf"]').forEach((leaf) =>
    attach(leaf, "block"),
  );
};

let queued = false;
const kick = () => {
  if (queued) return;
  queued = true;
  requestAnimationFrame(() => {
    queued = false;
    scan();
  });
};

/* 这页渲染的是真身，所以它得跟着 src/App.tsx 的导出走 —— 那边重构中（可能在改名、
   可能改成 default）的时候，别让整页白屏，按名字找一个像 App 的。 */
const loadApp = async () => {
  const mod = (await import("../../src/App")) as Record<string, unknown>;
  const named = [mod.App, mod.default];
  const upper = Object.entries(mod)
    .filter(([k, v]) => /^[A-Z]/.test(k) && typeof v === "function")
    .map(([, v]) => v);
  return (named.find((c) => typeof c === "function") ?? upper[0]) as React.ComponentType;
};

const root = document.getElementById("root");
if (!root) throw new Error("demo/index.html has no #root");

loadApp().then((App) => {
  if (!App) throw new Error("src/App.tsx 里找不到 App 组件");
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
});

new MutationObserver(kick).observe(document.body, { childList: true, subtree: true });
scan();

/* ── 关掉的时机 ───────────────────────────────────────────────────────────── */
document.addEventListener("click", (e) => {
  if (!openFor) return;
  const t = e.target as HTMLElement;
  if (t.closest(".mtg")) return;
  // 点条目就是「用完了」，关掉；点在菜单的缝隙上不动
  if (menu.contains(t)) {
    if (t.closest("button")) close();
    return;
  }
  close();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    const prev = openFor;
    close();
    prev?.focus();
  }
});
window.addEventListener("resize", () => openFor && place(openFor));

/* 一条小提示，点了就没了 */
const hint = document.createElement("button");
hint.type = "button";
hint.className = "ctx-hint";
hint.textContent = "07 号 · 三点菜单 —— 悬停左列的行或盘面上的块，点 ••• （条目不做事）";
hint.addEventListener("click", () => hint.remove());
document.body.appendChild(hint);

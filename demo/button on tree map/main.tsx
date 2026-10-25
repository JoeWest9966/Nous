/**
 * 13 号方案（层叠卡片）装在真盘上。
 *
 * 不是复刻：这里直接渲染 `src/App`，用的是同一份组件、同一份数据、同一份 CSS。
 * 动作按钮靠运行时挂到左列每一行上 —— 一行 src 都不用改，所以盘子在演进，
 * 这个 demo 跟着一起变。
 *
 * 只挂在左列的行（`li[role="treeitem"] > div[data-node-id]`）上。
 * 盘面上的块没挂：块会被布局挤到 30px 见方，一摞按钮塞不进去 —— 那种尺寸该用 07 号菜单。
 */
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "../../src/tokens/theme.css";
import "../../src/motion.css";
import { applyTheme, readTheme } from "../../src/tokens";
import { App } from "../../src/App";
import "./deck.css";

applyTheme(readTheme());

const ICONS = [
  ["重命名", `<path d="M9.2 2.4 11.6 4.8 5.4 11H3V8.6z"/>`],
  ["复制", `<rect x="2.5" y="2.5" width="7" height="7" rx="1.6"/>
            <path d="M5.5 11.5h4.4a1.6 1.6 0 0 0 1.6-1.6V5.5"/>`],
  ["链接", `<path d="M5.8 8.2a2.5 2.5 0 0 0 3.5 0l1.9-1.9a2.5 2.5 0 0 0-3.5-3.5l-.5.5"/>
           <path d="M8.2 5.8a2.5 2.5 0 0 0-3.5 0L2.8 7.7a2.5 2.5 0 0 0 3.5 3.5l.5-.5"/>`],
  ["删除", `<path d="M2.6 4h8.8M5.6 4V2.9c0-.2.2-.4.4-.4h2c.2 0 .4.2.4.4V4"/>
           <path d="M4.1 4l.5 7.1c0 .3.3.5.6.5h3.6c.3 0 .6-.2.6-.5L9.9 4"/>`],
] as const;

const deck = (): HTMLElement => {
  const wrap = document.createElement("div");
  wrap.className = "deck-acts";
  ICONS.forEach(([label, path], i) => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = i === ICONS.length - 1 ? "deck-act deck-danger" : "deck-act";
    b.style.setProperty("--i", String(i));
    b.setAttribute("aria-label", label);
    b.title = label;
    b.innerHTML = `<svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.5"
      stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${path}</svg>`;
    wrap.appendChild(b);
  });
  return wrap;
};

/**
 * 挂到 li 上，不挂到行那个 div 上。
 *
 * 行 div 是 React 管的，往里插节点将来会和 React 的 insertBefore 抢位置；
 * li 只有一个不会变的子节点，插在它后面永远安全。而且 li:hover 包含「光标停在按钮上」，
 * 所以从行移到按钮上不会掉 hover —— 不闪。
 */
const decorate = (row: HTMLElement) => {
  const host = row.parentElement;
  if (!host || host.dataset.deck === "1") return;
  host.dataset.deck = "1";
  host.classList.add("deck-host");
  host.appendChild(deck());
};

const scan = () => {
  document.querySelectorAll<HTMLElement>('li[role="treeitem"] > div[data-node-id]').forEach(decorate);
};

// React 每次重渲染都可能换掉节点，扫一遍就够了；rAF 合批，别在 mutation 里再触发 mutation。
let queued = false;
const kick = () => {
  if (queued) return;
  queued = true;
  requestAnimationFrame(() => {
    queued = false;
    scan();
  });
};

const root = document.getElementById("root");
if (!root) throw new Error("demo/index.html has no #root");

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

new MutationObserver(kick).observe(document.body, { childList: true, subtree: true });
scan();

/* 一条小提示，点了就没了 —— 这页没有别的东西告诉你「该看哪儿」 */
const hint = document.createElement("button");
hint.type = "button";
hint.className = "deck-hint";
hint.textContent = "13 号方案 · 层叠卡片 —— 悬停左列任意一行，按钮不做事";
hint.addEventListener("click", () => hint.remove());
document.body.appendChild(hint);

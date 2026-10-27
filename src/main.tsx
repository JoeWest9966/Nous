import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { registerSW } from "virtual:pwa-register";
import { App } from "./App";
import { applyTheme, readTheme } from "./tokens";
import "./tokens/theme.css";
import "./motion.css";

applyTheme(readTheme());

/**
 * 自己更新，不等用户想起来按 cmd-shift-R。
 *
 * 这是个 PWA：`sw.js` 预缓存了 index.html 和整包 hash 过的资源，所以**新部署不会自己跑到
 * 已经开着的标签页里** —— 对着一个三小时前的包说「没变化」，这不是用户的错。
 * 两件事一起做：
 *  1. 每 30 秒问一次有没有新 sw；
 *  2. 新的接管了才刷一次。开局那次 `claim()` 不刷（`hadController`），否则每次冷启动都闪一下。
 *
 * 刷新是安全的：正文有本地草稿（`store.ts` 的 Draft），离开前会 flush。
 */
const hadController = !!navigator.serviceWorker?.controller;
navigator.serviceWorker?.addEventListener("controllerchange", () => {
  if (hadController) window.location.reload();
});

registerSW({
  immediate: true,
  onRegisteredSW(_url, reg) {
    if (!reg) return;
    setInterval(() => void reg.update().catch(() => {}), 30_000);
  },
});

const root = document.getElementById("root");
if (!root) throw new Error("index.html has no #root");

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

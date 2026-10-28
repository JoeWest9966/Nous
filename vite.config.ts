import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, loadEnv } from "vite";
import { VitePWA } from "vite-plugin-pwa";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { contentApi } from "./server/api.ts";
import { blocksApi, killAllBlocks } from "./server/blocks.ts";
import { nousApi } from "./server/ctx.ts";
import { BoardStore } from "./server/content.ts";

const ROOT = path.dirname(fileURLToPath(import.meta.url));
/** content/ 是项目根，不是 /demo —— demo 里的东西会被当静态资源 serve 出去。 */
const CONTENT = path.join(ROOT, "content");

/**
 * demo 页面也是 build 的入口。
 *
 * 不能只是把 demo/ 拷进 dist —— 两个 demo 的入口是 `.tsx`，浏览器跑不了；
 * 而且它们 import 了 `src/App`、`src/tokens/*.css`，那些得跟主应用一样过一遍构建。
 * 交给 rollup 当第二个入口，产物自然落在 dist/demo/<名字>/，preview 就能真 serve 了。
 *
 * 扫目录而不是写死清单：新扔一个 demo/<名字>/index.html 进来就自动进去。
 */
const demoInputs = (): Record<string, string> => {
  const dir = path.join(ROOT, "demo");
  if (!fs.existsSync(dir)) return {};
  const out: Record<string, string> = {};
  for (const name of fs.readdirSync(dir, { withFileTypes: true })) {
    if (!name.isDirectory()) continue;
    const html = path.join(dir, name.name, "index.html");
    if (fs.existsSync(html)) out[`demo/${name.name}/index.html`] = html;
  }
  return out;
};

/** 内容 API 只在 dev / preview 里跑。真上线时把同一个 `contentApi` 挂到后端就行。 */
const contentPlugin = () => {
  const mount = (server: { middlewares: { use: (p: string, h: unknown) => void } }) => {
    server.middlewares.use("/api/boards", contentApi(CONTENT) as never);
  };
  return {
    name: "n-content-api",
    configureServer: mount,
    configurePreviewServer: mount,
  };
};

/**
 * 有代码的块是**独立进程**，宿主只做反代。
 *
 * 这里不认 iframe、不认沙箱：`/p/<board>/<id>/` 前缀剥掉，剩下的原样交给那块自己。
 * 壳（文档页）和它共用一个 URL 空间 —— 块有 `scripts.start` 就归进程，没有就归壳。
 */
const blockPlugin = () => {
  const mount = (server: {
    middlewares: { use: (p: string, h: unknown) => void };
    httpServer?: { once: (ev: string, fn: () => void) => void } | null;
    config: { server: { port?: number } };
  }) => {
    const origin = `http://localhost:${server.config.server.port ?? 4790}`;
    const store = new BoardStore(CONTENT);
    // 顺序有讲究：块先挑走自己的路径（`__nous/` 除外），剩下的 `__nous/*` 归宿主。
    server.middlewares.use("/p", blocksApi(store, { root: CONTENT, origin }) as never);
    server.middlewares.use("/p", nousApi(store, { origin }) as never);
    // 关掉 dev server 就让块跟着下班，别留一堆没人看得见的进程。
    server.httpServer?.once("close", killAllBlocks);
  };
  return {
    name: "n-blocks",
    configureServer: mount,
    configurePreviewServer: mount,
  };
};

export default defineConfig(({ mode }) => {
  // Nous 只跑在自己的 Tailscale 内网里：默认听 0.0.0.0，tailnet 里谁都能开。
  // 要收回本机就往 .env 写 NOUS_HOST=127.0.0.1。
  const host = loadEnv(mode, process.cwd(), "").NOUS_HOST || "0.0.0.0";

  return {
    plugins: [
      react(),
      tailwindcss(),
      contentPlugin(),
      blockPlugin(),
      VitePWA({
        registerType: "autoUpdate",
        includeAssets: ["icon.svg"],
        manifest: {
          name: "Nous",
          short_name: "Nous",
          description: "The Durable Agent Network panel.",
          display: "standalone",
          start_url: "/",
          theme_color: "#0b0d11",
          background_color: "#0b0d11",
          icons: [{ src: "icon.svg", sizes: "any", type: "image/svg+xml", purpose: "any" }],
        },
        /**
         * **不预缓存 HTML，也不接导航。**
         *
         * 缓存 index.html 等于把「某一刻的界面」钉死在浏览器里：装过老 sw 的浏览器会
         * 一直拿缓存里那份壳去糊 `/p/…` 的导航，代码改多少遍都看不见 —— 2026-10 就撞
         * 过一次，新建的块还显示 `AGENTS.md`，而服务器上它早没了。`/p/` 的 denylist
         * 只治得了那一刻之后的 sw，治不了已经装上的那个。
         *
         * 所以这里只缓存**哈希过的**静态资源（同一个 URL 永远同一份内容），所有导航
         * 都走网络，打开就是最新的包。代价是离线打不开 —— 这是内网面板，认。
         */
        workbox: {
          globPatterns: ["**/*.{js,css,svg,woff2}"],
          // demo 不进 PWA 预缓存：它们是给人看的实验页，不是 app 的一部分，
          // 而且每个都带着一整份 src/App（chunk 落在 assets/demo/ 下，两处都挡）。
          globIgnores: ["demo/**", "assets/demo/**"],
          // vite-plugin-pwa 默认给 `index.html` 挂一条导航兜底 —— 显式写 undefined 把它
          // 摘掉：导航一律走网络，不存在「SW 拿旧壳糊你」这条路。
          navigateFallback: undefined,
        },
      }),
    ],
    build: {
      rollupOptions: {
        input: { main: path.join(ROOT, "index.html"), ...demoInputs() },
        // 壳是浏览器，这些是 Node 侧的东西（openai / anthropic / aws-sdk / photon wasm）。
        // 标成 external：src/ 里手滑 import 了，build 期就吭声，而不是悄悄打出一个几十兆的 chunk。
        external: (id) => id.startsWith("@earendil-works/"),
      },
    },
    // 自己的端口，4783 留给隔壁那个老项目：两个可以同时开着。
    // allowedHosts 放宽是因为 tailnet 里可能用 MagicDNS 主机名访问，不只 IP。
    server: {
      host,
      port: 4790,
      strictPort: true,
      allowedHosts: true,
      // content/ 是数据不是源码：它每写一次都会触发 HMR，整页重载会把正在编辑的盘打回磁盘版本。
      watch: { ignored: ["**/content/**", "**/dist/**"] },
    },
    preview: { host, port: 4790, strictPort: true, allowedHosts: true },
  };
});

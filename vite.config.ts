import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, loadEnv } from "vite";
import { VitePWA } from "vite-plugin-pwa";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { contentApi } from "./server/api.ts";

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

export default defineConfig(({ mode }) => {
  // Nous 只跑在自己的 Tailscale 内网里：默认听 0.0.0.0，tailnet 里谁都能开。
  // 要收回本机就往 .env 写 NOUS_HOST=127.0.0.1。
  const host = loadEnv(mode, process.cwd(), "").NOUS_HOST || "0.0.0.0";

  return {
    plugins: [
      react(),
      tailwindcss(),
      contentPlugin(),
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
        workbox: {
          // demo 不进 PWA 预缓存：它们是给人看的实验页，不是 app 的一部分，
          // 而且每个都带着一整份 src/App。
          globPatterns: ["**/*.{js,css,html,svg,woff2}"],
          // demo 的 chunk 落在 assets/demo/ 下（入口在 demo/ 下），两处都得挡。
          globIgnores: ["demo/**", "assets/demo/**"],
          navigateFallback: "index.html",
          // 不然刷新 /demo/xxx/ 会被 SW 塞回主应用那张壳 —— 和 4783 之前的 SPA fallback 一个坑。
          navigateFallbackDenylist: [/^\/demo\//],
        },
      }),
    ],
    build: {
      rollupOptions: { input: { main: path.join(ROOT, "index.html"), ...demoInputs() } },
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

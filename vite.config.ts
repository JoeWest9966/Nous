import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { VitePWA } from "vite-plugin-pwa";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { contentApi } from "./server/api.ts";

/** content/ 是项目根，不是 /demo —— demo 里的东西会被当静态资源 serve 出去。 */
const CONTENT = path.join(path.dirname(fileURLToPath(import.meta.url)), "content");

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

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    contentPlugin(),
    VitePWA({
      registerType: "autoUpdate",
      includeAssets: ["icon.svg"],
      manifest: {
        name: "N",
        short_name: "N",
        description: "The Durable Agent Network panel.",
        display: "standalone",
        start_url: "/",
        theme_color: "#0b0d11",
        background_color: "#0b0d11",
        icons: [{ src: "icon.svg", sizes: "any", type: "image/svg+xml", purpose: "any" }],
      },
      workbox: { globPatterns: ["**/*.{js,css,html,svg,woff2}"], navigateFallback: "index.html" },
    }),
  ],
  // Own port, own host: this package never shares a server with the old panel.
  server: {
    host: "0.0.0.0",
    port: 4783,
    strictPort: true,
    // content/ 是数据不是源码：它每写一次都会触发 HMR，整页重载会把正在编辑的盘打回磁盘版本。
    watch: { ignored: ["**/content/**", "**/dist/**"] },
  },
  preview: { host: "0.0.0.0", port: 4783 },
});

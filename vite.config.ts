import { defineConfig, loadEnv } from "vite";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { contentApi } from "./server/api.ts";
import { blocksApi, killAllBlocks } from "./server/blocks.ts";
import { nousApi } from "./server/ctx.ts";
import { BoardStore } from "./server/content.ts";

const ROOT = path.dirname(fileURLToPath(import.meta.url));
/** content/ 是项目根，不是 /demo —— demo 里的东西会被当静态资源 serve 出去。 */
const CONTENT = path.join(ROOT, "content");

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
    plugins: [contentPlugin(), blockPlugin()],
    // 面板前端已删：这里只剩壳的宿主路由（/api/boards、/p/*），没有静态入口可构建。
    // 生产仍由 nous-panel.service 的 `vite preview` 挂这些中间件。
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

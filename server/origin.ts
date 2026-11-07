/**
 * 「外面的人看到我是谁」—— 块之间打电话、`ctx.blocks[].url`、文档链接拼的都是这个。
 *
 * 不能拿 `req.headers.host` 一拍脑袋拼 `http://`：
 *
 * - **协议**是反代的，不是这个进程的。TLS 在 Tailscale / nginx 那头就终结了，
 *   这里看到的是明文 HTTP —— 拼出 `http://…` 的名字，对面 443 上根本不接。
 * - **前缀**是反代剥掉的。`/nous/p/<b>/<id>` 进到这里时已经是 `/p/<b>/<id>`，
 *   `req.url` 里一个字节的前缀都没有，`X-Forwarded-Prefix` 也没人保证会发
 *   （Tailscale serve 就不发）。所以前缀只能靠配置：`NOUS_BASE=/nous`。
 *
 * 三个都缺一不可：`https` + 主机名 + 前缀。少一个，块就打电话打不回家（2026-11 的实况：
 * 块拿 `http://host/p/…` 去 resolve，串到了同域根上的另一个 app）。
 */
import type { IncomingMessage } from "node:http";

/** 头可能是数组（重复头），取第一个非空的。 */
const one = (v: string | string[] | undefined): string | undefined =>
  Array.isArray(v) ? v.find((s) => s.length > 0) : v;

/** 挂载前缀，永远是 "" 或 "/x" 的形状（`NOUS_BASE=/nous/` 也认）。 */
const basePath = (): string => {
  const raw = process.env.NOUS_BASE ?? "";
  const trimmed = raw.replace(/\/+$/, "");
  return trimmed.startsWith("/") ? trimmed : trimmed ? `/${trimmed}` : "";
};

/**
 * 从请求里现算「我的公开地址」，算不出来才退回 `fallback`（宿主自己听的 localhost 端口）。
 *
 * 前缀**只给从反代进来的请求加**：块自己打电话用的是 `NOUS_LOCAL_ORIGIN`（`http://127.0.0.1:4790`），
 * 那条路上没有 `/nous` 这一段，硬塞一个前缀只会把它引到死路（这里踩过：块拿到
 * `http://127.0.0.1:4790/nous/p/…`，落到 SPA fallback 的一张 HTML 上，再当 JSON 解就炸）。
 * 判据就是反代有没有留痕：`x-forwarded-proto` / `x-forwarded-host` 在，就是外面转进来的。
 */
export const publicOrigin = (req: IncomingMessage, fallback: string): string => {
  const forwarded = Boolean(
    one(req.headers["x-forwarded-proto"]) ?? one(req.headers["x-forwarded-host"]),
  );
  const proto = one(req.headers["x-forwarded-proto"])?.split(",")[0]?.trim() || "http";
  const host = one(req.headers["x-forwarded-host"]) ?? req.headers.host;
  if (!host) return fallback;
  return `${proto}://${host}${forwarded ? basePath() : ""}`;
};

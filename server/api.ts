/**
 * 内容层的 HTTP 外壳。故意只依赖 `node:http` 的 req/res 形状，
 * 这样同一份东西既能挂进 vite 的 connect 中间件，也能挂进真后端。
 *
 * 路由（前缀由挂载方负责剥掉）：
 *   GET  /            → 项目列表
 *   POST /            → 建项目 { name }
 *   GET  /:id         → { root, moves, version, name }
 *   PUT  /:id         → { root, moves, baseVersion } → { version } | 409
 */
import type { IncomingMessage, ServerResponse } from "node:http";
import { BoardStore, isBoardId } from "./content.ts";

const MAX_BODY = 4 * 1024 * 1024;

async function body(req: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const c of req) {
    size += (c as Buffer).length;
    if (size > MAX_BODY) throw new Error("body too large");
    chunks.push(c as Buffer);
  }
  if (!chunks.length) return undefined;
  return JSON.parse(Buffer.concat(chunks).toString("utf8")) as unknown;
}

const send = (res: ServerResponse, code: number, value: unknown): void => {
  const text = JSON.stringify(value);
  res.statusCode = code;
  res.setHeader("content-type", "application/json; charset=utf-8");
  res.setHeader("cache-control", "no-store");
  res.end(text);
};

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

export function contentApi(root: string) {
  const store = new BoardStore(root);

  return async (req: IncomingMessage, res: ServerResponse, next: (err?: unknown) => void) => {
    // connect 挂载会剥掉前缀，req.url 到这里长这样："" "/" "/new1"
    const seg = (req.url ?? "/").split("?")[0]!.replace(/^\/+|\/+$/g, "");
    const id = decodeURIComponent(seg);

    try {
      if (!seg) {
        if (req.method === "GET") {
          send(res, 200, await store.list());
          return;
        }
        if (req.method === "POST") {
          const input = await body(req);
          const name = isRecord(input) && typeof input.name === "string" ? input.name : "";
          if (!name.trim()) {
            send(res, 400, { error: "name required" });
            return;
          }
          send(res, 201, await store.create(name));
          return;
        }
        send(res, 405, { error: "method not allowed" });
        return;
      }

      if (!isBoardId(id)) {
        send(res, 400, { error: "bad board id" });
        return;
      }

      if (req.method === "GET") {
        const tree = await store.tree(id);
        if (!tree) {
          send(res, 404, { error: "no such board" });
          return;
        }
        send(res, 200, {
          name: tree.name,
          version: tree.version,
          root: tree.root,
          moves: await store.log(id),
        });
        return;
      }

      if (req.method === "PUT") {
        const input = await body(req);
        if (!isRecord(input) || !("root" in input) || typeof input.baseVersion !== "number") {
          send(res, 400, { error: "root and baseVersion required" });
          return;
        }
        const moves = Array.isArray(input.moves) ? input.moves : [];
        const result = await store.save(id, {
          root: input.root,
          moves,
          baseVersion: input.baseVersion,
        });
        if (result.status === "missing") {
          send(res, 404, { error: "no such board" });
          return;
        }
        if (result.status === "conflict") {
          send(res, 409, { error: "version conflict", version: result.version });
          return;
        }
        send(res, 200, { version: result.version });
        return;
      }

      send(res, 405, { error: "method not allowed" });
    } catch (err) {
      // 别让一个坏 JSON 把 dev server 带下去。
      if (err instanceof SyntaxError) {
        send(res, 400, { error: "bad json" });
        return;
      }
      next(err);
    }
  };
}

/**
 * 内容层的 HTTP 外壳。故意只依赖 `node:http` 的 req/res 形状，
 * 这样同一份东西既能挂进 vite 的 connect 中间件，也能挂进真后端。
 *
 * 路由（前缀由挂载方负责剥掉）：
 *   GET  /                       → 项目列表
 *   POST /                       → 建项目 { name }
 *   GET  /:id                    → { root, moves, version, name }
 *   PUT  /:id                    → { root, moves, baseVersion } → { version } | 409
 *   GET  /:id/nodes/:node/doc    → { text, etag } | 404
 *   PUT  /:id/nodes/:node/doc    → { text, etag } → { etag } | 409 { etag, text }
 *
 * 路径按段读，不按整串读 —— 块自己的地址是 `/p/<board>/<id>/`，同一套 id 得能挂在
 * board 后面往下走。`/nodes/...` 那一段以后还会有 PUT，以及块进程的转发。
 */
import type { IncomingMessage, ServerResponse } from "node:http";
import { BoardStore, isBoardId, type DocEditResult } from "./content.ts";
import { isDocName, normalizeDocName } from "../src/docname.ts";
import { isNodeId } from "./map.ts";

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

/** 新建 / 删 / 恢复 三个动作的回执：成功就是 200，其余一条条翻译成说得出口的话。 */
const sendEdit = (res: ServerResponse, out: DocEditResult): void => {
  if (out.status === "ok") {
    send(res, 200, { name: out.name });
    return;
  }
  const map: Record<Exclude<DocEditResult["status"], "ok">, [number, string]> = {
    "no-board": [404, "no such board"],
    "no-node": [404, "no such node"],
    "bad-name": [400, "bad doc name"],
    missing: [404, "no such doc"],
    // 唯一一条 403：AGENTS.md 只能改 —— 删不掉，也改不了名。
    protected: [403, "AGENTS.md 只能改"],
    exists: [409, "already exists"],
  };
  const [code, error] = map[out.status];
  send(res, code, { error });
};

export function contentApi(root: string) {
  const store = new BoardStore(root);

  return async (req: IncomingMessage, res: ServerResponse, next: (err?: unknown) => void) => {
    // connect 挂载会剥掉前缀，req.url 到这里长这样："" "/" "/new1" "/new1/nodes/n39/doc"
    const at = (req.url ?? "/").indexOf("?");
    const parts = (req.url ?? "/")
      .split("?")[0]!
      .split("/")
      .filter(Boolean);
    const query = new URLSearchParams(at === -1 ? "" : (req.url ?? "").slice(at + 1));
    const seg = parts.length ? decodeURIComponent(parts[0]!) : "";
    const id = seg;

    try {
      if (!parts.length) {
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

      // ---- 锁：/:board / locks ----
      if (parts.length === 2 && parts[1] === "locks") {
        if (req.method === "GET") {
          send(res, 200, { ids: await store.locks(id) });
          return;
        }
        if (req.method === "PUT") {
          const input = await body(req);
          if (!isRecord(input) || typeof input.id !== "string" || typeof input.locked !== "boolean") {
            send(res, 400, { error: "id and locked required" });
            return;
          }
          const out = await store.setLock(
            id,
            input.id,
            input.locked,
            req.socket.remoteAddress ?? "",
          );
          if (out === "no-board") {
            send(res, 404, { error: "no such board" });
            return;
          }
          if (out === "no-node") {
            send(res, 404, { error: "no such node" });
            return;
          }
          send(res, 200, { ids: await store.locks(id) });
          return;
        }
        send(res, 405, { error: "method not allowed" });
        return;
      }

      // ---- 块自己的东西：/ :board / nodes / :node / docs | doc/:name ----
      if (parts.length > 1) {
        const [, kind, nodeId, leaf, name, verb, ...rest] = parts;
        if (!nodeId || !isNodeId(nodeId)) {
          send(res, 400, { error: "bad node id" });
          return;
        }

        // 文档清单（`:docs`）：文件夹里有什么就是什么，不存清单。`?q=` 搜名字和正文。
        if (kind === "nodes" && leaf === "docs" && !name) {
          if (req.method === "GET") {
            const out = await store.docs(id, nodeId, query.get("q") ?? "");
            if (out.status !== "ok") {
              send(res, 404, { error: out.status === "no-board" ? "no such board" : "no such node" });
              return;
            }
            send(res, 200, out);
            return;
          }
          // 新建：POST `docs` { name }。名字 sanitize 在服务端再做一遍，客户端那份只为少一次往返。
          if (req.method === "POST") {
            const input = await body(req);
            const raw = isRecord(input) && typeof input.name === "string" ? input.name : "";
            if (!normalizeDocName(raw)) {
              send(res, 400, { error: "name required" });
              return;
            }
            sendEdit(res, await store.createDoc(id, nodeId, raw));
            return;
          }
          send(res, 405, { error: "method not allowed" });
          return;
        }

        // 一份文档：`doc`（= AGENTS.md，老接口）/ `doc/:name` / `doc/:name/restore`。
        if (!(kind === "nodes" && leaf === "doc")) {
          send(res, 404, { error: "no such route" });
          return;
        }

        const docName = name ? decodeURIComponent(name) : undefined;
        if (docName !== undefined && !isDocName(docName)) {
          send(res, 400, { error: "bad doc name" });
          return;
        }
        // `doc/:name/restore`：从 `.trash/` 搬回来。
        if (verb === "restore") {
          if (rest.length || req.method !== "POST") {
            send(res, 405, { error: "method not allowed" });
            return;
          }
          const out = await store.restoreDoc(id, nodeId, docName!);
          sendEdit(res, out);
          return;
        }
        // `doc/:name/rename`：同一个文件夹里换个名字，正文一个字节都不动。
        if (verb === "rename") {
          if (rest.length || req.method !== "POST") {
            send(res, 405, { error: "method not allowed" });
            return;
          }
          if (!docName) {
            send(res, 400, { error: "name required" });
            return;
          }
          const input = await body(req);
          const raw = isRecord(input) && typeof input.name === "string" ? input.name : "";
          if (!normalizeDocName(raw)) {
            send(res, 400, { error: "name required" });
            return;
          }
          sendEdit(res, await store.renameDoc(id, nodeId, docName, raw));
          return;
        }
        if (verb !== undefined || rest.length) {
          send(res, 404, { error: "no such route" });
          return;
        }

        if (req.method === "GET") {
          const doc = await store.doc(id, nodeId, docName);
          if (doc.status === "no-board") {
            send(res, 404, { error: "no such board" });
            return;
          }
          if (doc.status === "no-node") {
            send(res, 404, { error: "no such node" });
            return;
          }
          if (doc.status === "no-file") {
            send(res, 404, { error: "no such doc", missing: true });
            return;
          }
          send(res, 200, doc);
          return;
        }
        if (req.method === "PUT") {
          const input = await body(req);
          if (!isRecord(input) || typeof input.text !== "string" || typeof input.etag !== "string") {
            send(res, 400, { error: "text and etag required" });
            return;
          }
          const saved = await store.saveDoc(id, nodeId, input.text, input.etag, docName);
          if (saved.status === "no-board" || saved.status === "no-node") {
            send(res, 404, { error: saved.status === "no-board" ? "no such board" : "no such node" });
            return;
          }
          if (saved.status === "conflict") {
            // 409 带上磁盘上那份：调用方要能让人当面选，不能只剩一句「冲突了」。
            send(res, 409, { error: "etag conflict", etag: saved.etag, text: saved.text });
            return;
          }
          send(res, 200, { etag: saved.etag });
          return;
        }
        // 删：DELETE `doc/:name`，进 `.trash/`。
        if (req.method === "DELETE") {
          if (!docName) {
            send(res, 400, { error: "name required" });
            return;
          }
          sendEdit(res, await store.trashDoc(id, nodeId, docName));
          return;
        }
        send(res, 405, { error: "method not allowed" });
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
          // 绝对路径只有服务器知道。前端拼「终端路径」用 —— 不硬编码到客户端，换机器换目录都还对。
          dir: store.absDir(id),
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
        const result = await store.save(
          id,
          { root: input.root, moves, baseVersion: input.baseVersion },
          req.socket.remoteAddress ?? "",
        );
        if (result.status === "missing") {
          send(res, 404, { error: "no such board" });
          return;
        }
        if (result.status === "conflict") {
          send(res, 409, { error: "version conflict", version: result.version });
          return;
        }
        // root 只有在服务器改过它的时候才回来（有锁定的块被接回去了）。
        // 客户端必须认这份，否则本地和盘上就分叉了。
        const corrected = result.restored.length > 0;
        send(res, 200, {
          version: result.version,
          restored: result.restored,
          ...(corrected ? { root: result.root } : {}),
        });
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

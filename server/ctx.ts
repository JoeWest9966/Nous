/**
 * 块问宿主：「我在哪、我旁边有谁、怎么找到它们」。
 *
 * 这块东西存在的理由只有一个：**块之间只走 HTTP**，而 URL 长什么样是宿主的秘密。
 * 如果让块自己拼 `/p/<board>/<id>`，那将来某块搬去自己的域名/容器，得回头改它的代码 ——
 * 而「可插拔」的意思是：块搬家，块代码一行不动。
 *
 * 所以地址由 `resolve` 给，现在就返回 `/p/<board>/<id>`，将来只改这个文件：
 * 一块一张表，查不到再退回路径。**`resolve` 是这整套愿景里唯一不可替换的东西。**
 *
 * 两条铁律（也是这文件只做这么点事的原因）：
 *
 * 1. **块之间只认 HTTP。** 不许 import 别人的代码，不许读别人的文件夹 ——
 *    那是契约之外的耦合，`cp -r` 搬走一块的当天就会断。数据只在自己的 `data/` 里。
 * 2. **没有凭据。** 单用户内网，块与块互信。这里没有 token、没有签名、没有 scope。
 *    要多人、要不可信代码那天，先加进程/容器边界，再加身份 —— 顺序反了全是白工。
 */
import type { IncomingMessage, ServerResponse } from "node:http";
import type { BoardStore } from "./content.ts";
import { adoptNode } from "./map.ts";
import type { Node } from "../src/game.ts";

const send = (res: ServerResponse, status: number, body: unknown): void => {
  res.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
  });
  res.end(JSON.stringify(body));
};

type Flat = {
  id: string;
  name: string;
  /** `N/src/App.tsx` —— 给人看的路径，块要按组筛东西时用它。 */
  path: string;
  kind: "group" | "block";
  /** 绝对地址。宿主的秘密就在这儿，块只认它。 */
  url: string;
};

const flatten = (node: Node, base: string, at: string, out: Flat[]): void => {
  const path = at ? `${at}/${node.name}` : node.name;
  const url = `${base}/${encodeURIComponent(node.id)}`;
  if (node.children) {
    out.push({ id: node.id, name: node.name, path, kind: "group", url });
    for (const kid of node.children) flatten(kid, base, path, out);
  } else {
    out.push({ id: node.id, name: node.name, path, kind: "block", url });
  }
};

const findPath = (node: Node, id: string): Node[] | undefined => {
  if (node.id === id) return [node];
  for (const kid of node.children ?? []) {
    const rest = findPath(kid, id);
    if (rest) return [node, ...rest];
  }
  return undefined;
};

export type CtxOptions = {
  /** 宿主自己的地址，块被搬走后靠它继续找得到邻居。 */
  origin: string;
};

/** `GET /p/<board>/<id>/__nous/ctx`、`GET /p/<board>/<id>/__nous/resolve/<谁>[?path=/底下]` */
export function nousApi(store: BoardStore, opts: CtxOptions) {
  return (req: IncomingMessage, res: ServerResponse, next: (err?: unknown) => void): void => {
    const url = req.url ?? "/";
    const cut = url.indexOf("?");
    const pathname = cut === -1 ? url : url.slice(0, cut);
    const query = new URLSearchParams(cut === -1 ? "" : url.slice(cut + 1));
    const parts = pathname.split("/").filter(Boolean);
    const [board = "", self = "", magic = "", ...rest] = parts;
    if (!board || !self || magic !== "__nous") {
      next();
      return;
    }

    void (async () => {
      const tree = await store.tree(board);
      const root = tree ? adoptNode(tree.root) : undefined;
      if (!root || !findPath(root, self)) {
        next();
        return;
      }
      const host = req.headers.host ? `http://${req.headers.host}` : opts.origin;
      const base = `${host}/p/${board}`;
      const mine = `${base}/${encodeURIComponent(self)}`;

      const [verb = ""] = rest;
      if (verb === "ctx") {
        const all: Flat[] = [];
        flatten(root, base, "", all);
        const chain = findPath(root, self)!;
        send(res, 200, {
          board,
          host,
          /** 我挂在宿主的哪儿。给自己拼链接用它，别自己拼 `/p/...`。 */
          base: mine,
          /** 邻居全在这儿：`ctx.blocks[id].url` 就能打电话。 */
          blocks: all,
          /** 查一个我没在 `blocks` 里看到的 id（留着给将来按组查）。 */
          resolve: `${mine}/__nous/resolve`,
          node: {
            id: self,
            name: chain[chain.length - 1]!.name,
            path: chain.map((n) => n.name).join("/"),
            kind: chain[chain.length - 1]!.children ? "group" : "block",
            url: mine,
          },
        });
        return;
      }

      if (verb === "resolve") {
        const [target = "", ...tail] = rest.slice(1);
        const chain = target ? findPath(root, target) : undefined;
        if (!chain) {
          send(res, 404, { error: "no such block", id: target });
          return;
        }
        // 将来某块在自己的域名上：在这儿查表，返回它的绝对地址。块代码不用改。
        const extra = query.get("path") ?? (tail.length ? `/${tail.join("/")}` : "");
        send(res, 200, {
          id: target,
          name: chain[chain.length - 1]!.name,
          path: chain.map((n) => n.name).join("/"),
          url: `${base}/${encodeURIComponent(target)}${extra}`,
        });
        return;
      }

      next();
    })().catch((err: unknown) => {
      send(res, 500, { error: err instanceof Error ? err.message : String(err) });
    });
  };
}

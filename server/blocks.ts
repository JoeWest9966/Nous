/**
 * 一个块 = 一个独立进程。**没有条件。**
 *
 * - 写了 `scripts.start`：那个进程就是它自己写的，宿主只做反代。
 * - 没写：宿主给它一个内置的空跑 server（`blank.mjs`）—— 文件夹里有 `index.html` 就静态端出去，
 *   没有就一张**真正的空白页**。空白页也是块自己的页面，宿主不替它画。
 *
 * 宿主不认识块的语言、不看它的依赖、不 import 它。`/p/<board>/<id>/*` 剥掉前缀，
 * 剩下的原样怼到那个进程监听的端口上。
 *
 * 由此换来三件在单进程里做不到的事：
 *
 * - `cp -r map/<id> ~/<id> && npm start` 和宿主里打开是同一页（所以块里链接必须写**相对路径**）。
 * - 崩了只坏那一页。宿主拿 stderr 的最后几行当墓碑，树、别的块一概不受影响。
 * - 将来它换 origin、进容器、上域名，块代码一行不改 —— 只改 `ctx.ts` 里那张地址表。
 *
 * 进程是这套架构里唯一 fake 不掉的东西：接口可以口头约定，API 可以绕过，
 * 但「两个东西跑在两个进程里」这件事骗不了人。
 *
 * 给的四个环境变量（就这四个，端口不写进任何文件）：
 *
 * - `PORT`         宿主分配的空闲端口，块必须听它
 * - `NOUS_DATA`    持久化数据放这儿（= 它的文件夹下的 data/），`cp` 走就带走了
 * - `NOUS_ORIGIN`  宿主自己的地址，块之间打电话用它
 * - `NOUS_SELF`    我在宿主里叫什么（`/p/<board>/<id>`）。搬出去跑也取得到自己的上下文
 */
import { spawn, type ChildProcess } from "node:child_process";
import { watch, type FSWatcher } from "node:fs";
import { mkdir } from "node:fs/promises";
import http from "node:http";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { BoardStore } from "./content.ts";
import { adoptNode, hasNode } from "./map.ts";

/** 等一个块把端口听起来的耐心。第一次 `npm start` 有时要先编译一遍。 */
const BOOT_TIMEOUT = 20_000;
/** 没人碰它多久就下桌。块是进程，闲着就是占内存。 */
const IDLE_MS = 10 * 60_000;
const LOG_LIMIT = 8_000;
const START = "start";
/** 没写代码的块用这个跑：它就在宿主这边，不在块的文件夹里。 */
const BLANK = path.join(path.dirname(fileURLToPath(import.meta.url)), "blank.mjs");
/** 空跑进程在 `cmd` 里的标记 —— 用来识别「这一版还是那个空跑 server」。 */
const BLANK_TAG = "__blank__";

type Run = {
  key: string;
  folder: string;
  /** `scripts.start` 原文。改了这一行就得重启，否则跑的是上一版代码。 */
  cmd: string;
  port: number;
  child: ChildProcess;
  ready: boolean;
  logs: string;
  lastUsed: number;
  /** 盯着文件夹：改代码就把它收掉，下一次请求自然带着新代码重开。 */
  watcher?: FSWatcher;
};

const runs = new Map<string, Run>();
/** 同一个块被两个请求同时撞见时，只许起一个进程。 */
const booting = new Map<string, Promise<Run | { error: string; logs: string }>>();

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** 要一个空闲端口。用 0 让内核给，然后再关掉 —— 中间那点缝就当没有。 */
const freePort = (): Promise<number> =>
  new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.unref();
    srv.once("error", reject);
    srv.listen(0, "127.0.0.1", () => {
      const { port } = srv.address() as net.AddressInfo;
      srv.close(() => resolve(port));
    });
  });

const listening = (port: number): Promise<boolean> =>
  new Promise((resolve) => {
    const sock = net.connect(port, "127.0.0.1");
    const done = (ok: boolean) => {
      sock.destroy();
      resolve(ok);
    };
    sock.setTimeout(400, () => done(false));
    sock.once("connect", () => done(true));
    sock.once("error", () => done(false));
  });

const kill = (run: Run): void => {
  runs.delete(run.key);
  booting.delete(run.key);
  run.watcher?.close();
  try {
    // detached 起的进程自己一组：`npm start` 下面还挂着真正的服务，光杀 npm 会留孤儿。
    if (run.child.pid) process.kill(-run.child.pid, "SIGTERM");
  } catch {
    try {
      run.child.kill("SIGTERM");
    } catch {
      // 已经死了。
    }
  }
};

type Launch = { bin: string; args: string[]; cmd: string };

const launchFor = (folder: string, startScript: string): Launch =>
  startScript
    ? { bin: "npm", args: [START], cmd: startScript }
    : // 空块也是进程：内置 server 直接跑，不过 npm（没有安装、没有 package.json）。
      { bin: process.execPath, args: [BLANK, folder], cmd: BLANK_TAG };

const start = async (
  key: string,
  folder: string,
  launch: Launch,
  origin: string,
): Promise<Run | { error: string; logs: string }> => {
  const port = await freePort();
  const data = path.join(folder, "data");
  await mkdir(data, { recursive: true }).catch(() => undefined);

  const child = spawn(launch.bin, launch.args, {
    cwd: folder,
    // detached：给它一个自己的进程组，收尸的时候能连孙进程一起收。
    detached: true,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      PORT: String(port),
      NOUS_DATA: data,
      NOUS_ORIGIN: origin,
      NOUS_SELF: `/p/${key}`,
    },
  });

  const run: Run = {
    key,
    folder,
    cmd: launch.cmd,
    port,
    child,
    ready: false,
    logs: "",
    lastUsed: Date.now(),
  };
  runs.set(key, run);

  const collect = (chunk: Buffer) => {
    run.logs = (run.logs + chunk.toString()).slice(-LOG_LIMIT);
  };
  child.stdout?.on("data", collect);
  child.stderr?.on("data", collect);

  let exited = "";
  child.once("exit", (code, signal) => {
    exited = signal ? `被 ${signal} 收了` : `退出码 ${code}`;
    run.ready = false;
    run.logs += `\n[宿主] 进程${exited}\n`;
    // 从表里摘掉：下一个请求会重新起一份，而不是把一个死进程当活的用。
    if (runs.get(key) === run) runs.delete(key);
  });
  child.once("error", (err) => {
    exited = err.message;
    run.logs += `\n[宿主] 起不来：${err.message}\n`;
    if (runs.get(key) === run) runs.delete(key);
  });

  // 有代码的块盯文件夹：代码一改就收掉（下一次请求带着新代码起来）。
  // 空块不盯：它没有代码可热，改 AGENTS.md 不该把进程重启一遍。
  if (launch.cmd !== BLANK_TAG) {
    try {
      let timer: NodeJS.Timeout | undefined;
      run.watcher = watch(folder, { recursive: true }, (_evt, file) => {
        const rel = file?.toString() ?? "";
        // 数据不是代码；node_modules 是 npm 的地盘，动它等于自杀。
        if (rel.startsWith(`data${path.sep}`) || rel.startsWith(`node_modules${path.sep}`)) return;
        clearTimeout(timer);
        timer = setTimeout(() => kill(run), 300);
      });
      run.watcher.on("error", () => run.watcher?.close());
    } catch {
      // 有些文件系统不支持 recursive watch —— 那就等手动重启，不因为它起不来。
    }
  }

  const until = Date.now() + BOOT_TIMEOUT;
  while (Date.now() < until) {
    if (exited) return { error: `块没起来（${exited}）`, logs: run.logs };
    if (await listening(port)) {
      run.ready = true;
      return run;
    }
    await sleep(120);
  }
  kill(run);
  return { error: `等了 ${BOOT_TIMEOUT / 1000} 秒，它还没听 ${port}`, logs: run.logs };
};

/** 起一个或接上已经起着的那个。 */
const acquire = async (
  key: string,
  folder: string,
  launch: Launch,
  origin: string,
): Promise<Run | { error: string; logs: string }> => {
  const existing = runs.get(key);
  if (existing) {
    // start 脚本被改过了：那份进程跑的是上一版代码，留着只会骗人。
    if (existing.cmd !== launch.cmd) {
      kill(existing);
    } else {
      existing.lastUsed = Date.now();
      return existing;
    }
  }
  const pending = booting.get(key);
  if (pending) return pending;
  const p = start(key, folder, launch, origin).finally(() => booting.delete(key));
  booting.set(key, p);
  return p;
};

const esc = (s: string): string =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** 那一页的墓碑：只坏这一块，树和别人一概不受牵连。 */
const tombstone = (title: string, body: string, logs: string): string => `<!doctype html>
<html lang="zh"><head><meta charset="utf-8"><title>${esc(title)}</title>
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>
  :root{color-scheme:dark}
  body{margin:0;background:#0b0d11;color:#e6e9ef;font:13px/1.7 ui-monospace,SFMono-Regular,Menlo,monospace;padding:32px}
  h1{font-size:15px;margin:0 0 4px;font-weight:600}
  p{margin:0 0 16px;color:#8b93a7}
  a{color:#e6e9ef}
  pre{white-space:pre-wrap;word-break:break-word;background:#12151b;border:1px solid #232833;border-radius:6px;padding:12px;margin:0;color:#aeb6c6;max-height:60vh;overflow:auto}
</style></head><body>
<h1>${esc(title)}</h1>
<p>${esc(body)}</p>
<pre>${esc(logs.trim() || "（它什么都没说）")}</pre>
</body></html>`;

const UP_HOP = new Set([
  "connection",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
  "host",
]);

/** 把一个请求原样怼进块里。剥掉的是前缀，留下的路径归块自己解释。 */
const forward = (req: http.IncomingMessage, res: http.ServerResponse, run: Run, rest: string, prefix: string): void => {
  const headers: Record<string, string | string[]> = {};
  for (const [k, v] of Object.entries(req.headers)) {
    if (v !== undefined && !UP_HOP.has(k.toLowerCase())) headers[k] = v;
  }
  headers.host = `127.0.0.1:${run.port}`;
  // 块想知道自己挂在哪儿时可以看这个 —— 但它不该依赖它，相对路径才是能 `cp` 走的写法。
  headers["x-forwarded-prefix"] = prefix;

  const up = http.request(
    { host: "127.0.0.1", port: run.port, method: req.method, path: rest || "/", headers },
    (upRes) => {
      res.writeHead(upRes.statusCode ?? 502, upRes.headers);
      upRes.pipe(res);
    },
  );
  up.on("error", (err) => {
    if (res.headersSent) {
      res.end();
      return;
    }
    const page = tombstone("这一块没接住", `反代连不上它（${err.message}）。`, run.logs);
    res.writeHead(502, { "content-type": "text/html; charset=utf-8" });
    res.end(page);
  });
  req.pipe(up);
};

export type BlocksOptions = {
  /** content/ 的绝对路径。 */
  root: string;
  /** 宿主自己的地址，喂给块的 `NOUS_ORIGIN`。 */
  origin: string;
};

/**
 * `/p/<board>/<node><剩余路径>` 的中间件。挂在 `use("/p", …)` 上，所以 `req.url` 已经不含 `/p`。
 *
 * 判定全看文件夹里有什么（幂等，不往 `tree.json` 里写任何东西）：
 * `package.json` 有 `scripts.start` → 进程；否则 `next()`，交给壳去显示它的文档页。
 */
export function blocksApi(store: BoardStore, opts: BlocksOptions) {
  // 空闲的块下桌。进程不能像缓存一样躺一整天。
  const sweeper = setInterval(() => {
    const now = Date.now();
    for (const run of [...runs.values()]) {
      if (run.ready && now - run.lastUsed > IDLE_MS) kill(run);
    }
  }, 60_000);
  sweeper.unref();

  return (req: http.IncomingMessage, res: http.ServerResponse, next: (err?: unknown) => void): void => {
    const url = req.url ?? "/";
    const cut = url.indexOf("?");
    const pathname = cut === -1 ? url : url.slice(0, cut);
    const search = cut === -1 ? "" : url.slice(cut);
    const [board = "", node = "", ...tail] = pathname.split("/").filter(Boolean);
    // `__nous/` 是宿主的保留段：一个块可以拿走前缀底下的任何路径，只有这一段不行。
    if (!board || !node || tail[0] === "__nous") {
      next();
      return;
    }

    void (async () => {
      const tree = await store.tree(board);
      const folder = store.folder(board, node);
      const root = tree ? adoptNode(tree.root) : undefined;
      // 树里没这个节点就不许它开机：删掉的块不该还能顺着旧书签跑起来。
      if (!root || !folder || !hasNode(root, node)) {
        next();
        return;
      }
      const launch = launchFor(folder, await store.startCmd(board, node));

      const prefix = `/p/${board}/${node}`;
      const key = `${board}/${node}`;
      const origin = req.headers.host ? `http://${req.headers.host}` : opts.origin;
      const run = await acquire(key, folder, launch, origin);
      if ("error" in run) {
        req.resume();
        res.writeHead(502, { "content-type": "text/html; charset=utf-8" });
        res.end(tombstone(`${node} 起不来`, run.error, run.logs));
        return;
      }
      run.lastUsed = Date.now();
      // 前缀之外原样给它：`/p/b/n/orders/42?x=1` → `/orders/42?x=1`。归块自己解释。
      forward(req, res, run, `/${[...tail].join("/")}${search}`, prefix);
    })().catch((err: unknown) => {
      const page = tombstone("宿主自己出错了", err instanceof Error ? err.message : String(err), "");
      res.writeHead(500, { "content-type": "text/html; charset=utf-8" });
      res.end(page);
    });
  };
}

/** 关了 dev server 就别把块留在后台 —— 它们不在任何人的视野里，只占内存。 */
export const killAllBlocks = (): void => {
  for (const run of [...runs.values()]) kill(run);
};

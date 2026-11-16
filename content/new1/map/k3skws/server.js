// k3skws「element」—— Nous 元素库的前端 + 它自己的后端。
//
// 跟 n8qkkj（触发器）、vbe4pt（扩展）是同一副骨架，但这件事简单得多：
// **元素就是 JSON 文件**，两类，两个目录，一个文件一个元素。
//
//   groups/<id>.json     组：一组包的引用
//   packages/<id>.json   包：一份可复用的东西（提示词 / 技能片段 / 清单 / 文件）
//
// 仓库：`/opt/Nous-Elements`（`NOUS_ELEMENTS_REPO` 可改），私库
// `JoeWest9966/Nous-Elements`。这一块是那份 JSON 的**唯一写入口**。
//
// 几条不许越过的线：
//   · **未知字段原样透传。** 这里不定义 schema 的终稿 —— 只校验 id / name 两个必填，
//     其余字段（包括你没想好的那些）进来什么样、出去什么样，一个键都不删。
//     删字段是页面的活儿，不是后端的。
//   · **写一次提交一次。** 每次成功写入/删除都 `git commit` 一条，提交信息
//     `feat(group): <id>` / `chore(package): rm <id>`。push 是尽力而为：本机提交成功就算成功，
//     推不上去在响应里带 `pushed:false`，页面照实说，绝不回滚本地提交。
//   · **原子写。** 先写 `<dir>/.<id>.json.tmp` 再 rename；读到半个文件是不可能的。
//   · **所有写操作排队。** 两个请求同时 `git add` 会互相吞（跟 n8qkkj 同一个坑）。
//   · **id 就是文件名。** 只收 `[a-z0-9-]`，改名 = 换 id，引用会断 —— 所以没有 rename 接口，
//     要改名就是「新建一份 + 删掉旧的」，让调用方自己决定引用怎么办。

import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdir, readdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC = path.join(HERE, "public");

const DEFAULT_PORT = 4784;
const DEFAULT_REPO = "/opt/Nous-Elements";

const KINDS = {
  group: { dir: "groups", label: "组" },
  package: { dir: "packages", label: "包" },
};

const ID_RE = /^[a-z0-9][a-z0-9-]{0,63}$/;
const PUSH_ENABLED = process.env.NOUS_ELEMENTS_PUSH !== "0";
const PUSH_TIMEOUT = Number(process.env.NOUS_ELEMENTS_PUSH_TIMEOUT ?? 20_000);

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".json": "application/json; charset=utf-8",
  ".ico": "image/x-icon",
};

// ── 小工具 ─────────────────────────────────────────────────────────────────

const send = (res, code, body, type = "application/json; charset=utf-8") => {
  const payload = typeof body === "string" || Buffer.isBuffer(body) ? body : JSON.stringify(body);
  res.writeHead(code, { "content-type": type, "cache-control": "no-store" });
  res.end(payload);
};
const bad = (res, code, error, extra = {}) => send(res, code, { error, ...extra });

/**
 * 跑一条命令，拿全 stdout/stderr。**不抛**：失败也是结果，调用方自己看 code。
 *
 * 超时必须收**整个进程组**：`git push` 底下挂着 `git-remote-https`，只杀 git 本体，
 * 孙进程还攥着管道，`close` 永远等不到 —— 表现出来就是「设了 20 秒超时，请求卡了 134 秒」。
 */
function run(cmd, args, { cwd, timeout = 60_000 } = {}) {
  return new Promise((resolve) => {
    const child = spawn(cmd, args, {
      cwd,
      detached: true,
      env: { ...process.env, GIT_TERMINAL_PROMPT: "0" },
    });
    let out = "";
    let err = "";
    const timer = setTimeout(() => {
      try {
        if (child.pid) process.kill(-child.pid, "SIGKILL");
      } catch {
        try {
          child.kill("SIGKILL");
        } catch {
          // 已经死了
        }
      }
    }, timeout);
    child.stdout.on("data", (chunk) => (out += chunk));
    child.stderr.on("data", (chunk) => (err += chunk));
    child.on("error", (error) => {
      clearTimeout(timer);
      resolve({ code: -1, out, err: String(error?.message ?? error) });
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      resolve({ code, out, err });
    });
  });
}

/** 一个 JSON 对象、不认数组、不认 null。 */
const isObject = (v) => v !== null && typeof v === "object" && !Array.isArray(v);

const firstLine = (s) => String(s ?? "").trim().split("\n")[0] ?? "";

// ── 仓库读写 ───────────────────────────────────────────────────────────────

function pathsFor(repo, kind) {
  const dir = path.join(repo, KINDS[kind].dir);
  return { dir, file: (id) => path.join(dir, `${id}.json`) };
}

/** 只认 `<id>.json`，跳过临时文件、`.gitkeep`、目录。 */
async function listIds(repo, kind) {
  const { dir } = pathsFor(repo, kind);
  let names = [];
  try {
    names = await readdir(dir);
  } catch (error) {
    if (error.code === "ENOENT") return [];
    throw error;
  }
  return names
    .filter((n) => n.endsWith(".json") && !n.startsWith("."))
    .map((n) => n.slice(0, -5))
    .filter((id) => ID_RE.test(id))
    .sort();
}

async function readItem(repo, kind, id) {
  const { file } = pathsFor(repo, kind);
  const raw = await readFile(file(id), "utf8");
  return JSON.parse(raw);
}

async function listItems(repo, kind) {
  const ids = await listIds(repo, kind);
  const out = [];
  for (const id of ids) {
    try {
      const body = await readItem(repo, kind, id);
      const st = await stat(pathsFor(repo, kind).file(id));
      out.push({ ...body, __mtime: st.mtimeMs, __bytes: st.size });
    } catch (error) {
      // 一份坏 JSON 不该让整个列表 500：原样报出来，页面标红，别的照列。
      out.push({ id, __broken: String(error?.message ?? error) });
    }
  }
  return out;
}

/**
 * 原子写：临时文件 + rename。临时文件以 `.` 开头，`listIds` 看不见它，
 * 所以写一半被读到这件事在盘上也不成立。
 */
async function writeItem(repo, kind, id, body) {
  const { dir, file } = pathsFor(repo, kind);
  await mkdir(dir, { recursive: true });
  const text = `${JSON.stringify(body, null, 2)}\n`;
  const tmp = path.join(dir, `.${id}.json.${process.pid}.tmp`);
  await writeFile(tmp, text, "utf8");
  await rename(tmp, file(id));
  return text;
}

// ── git ────────────────────────────────────────────────────────────────────

/** 提交点名路径：别把工作区里别人的脏文件顺手带走。 */
async function commitPaths(repo, message, relPaths) {
  const add = await run("git", ["add", "--", ...relPaths], { cwd: repo });
  if (add.code !== 0) return { committed: false, error: `git add 失败：${firstLine(add.err)}` };

  const staged = await run("git", ["status", "--porcelain", "--", ...relPaths], { cwd: repo });
  if (!staged.out.trim()) return { committed: false, error: "没有实际改动（内容与已提交的一致）" };

  const commit = await run("git", ["commit", "-m", message, "--", ...relPaths], { cwd: repo });
  if (commit.code !== 0) return { committed: false, error: `git commit 失败：${firstLine(commit.err || commit.out)}` };

  const head = await run("git", ["rev-parse", "--short", "HEAD"], { cwd: repo });
  return { committed: true, head: head.out.trim(), message };
}

/**
 * 尽力推：推不上去只报一声，绝不回滚本地提交（本机提交成功就算保存成功）。
 * 参数是「路上没速度就早点断」—— j1900 到 github.com 时不时挂住，不设这个能卡两分钟。
 */
async function pushIfEnabled(repo) {
  if (!PUSH_ENABLED) return { pushed: false, error: "推送已关闭（NOUS_ELEMENTS_PUSH=0）" };
  const push = await run(
    "git",
    ["-c", "http.lowSpeedLimit=2000", "-c", "http.lowSpeedTime=10", "push", "origin", "HEAD"],
    { cwd: repo, timeout: PUSH_TIMEOUT },
  );
  if (push.code === null || push.code === undefined) return { pushed: false, error: "push 超时被杀" };
  if (push.code !== 0) return { pushed: false, error: `push 失败：${firstLine(push.err || push.out)}` };
  return { pushed: true };
}

/** 本地领先远端多少条（推不上去时页面要显示这个数）。拿不到就是 null，不猜。 */
async function aheadCount(repo) {
  const res = await run("git", ["rev-list", "--count", "@{u}..HEAD"], { cwd: repo, timeout: 8_000 });
  if (res.code !== 0) return null;
  const n = Number(res.out.trim());
  return Number.isFinite(n) ? n : null;
}

/**
 * 所有写操作排队：两个请求同时 git add / commit 会互相吞。
 * 前一个失败也要继续排下去 —— 否则一次 4xx 会把整条队列堵死。
 */
function makeQueue() {
  let tail = Promise.resolve();
  return (fn) => {
    const next = tail.then(fn, fn);
    tail = next.then(
      () => {},
      () => {},
    );
    return next;
  };
}

// ── 校验 ───────────────────────────────────────────────────────────────────

/**
 * 只认两件必填：id 和 name。别的一概不碰 —— 未知字段原样透传，删字段是页面的活儿。
 * 认得的字段只做「类型对不对」的检查，不做枚举、不做默认值。
 */
function validate(kind, id, body) {
  if (!isObject(body)) return "body 必须是一个 JSON 对象";
  if (body.id !== id) return `body.id（${JSON.stringify(body.id)}）与路径里的 id（${id}）不一致`;
  if (typeof body.name !== "string" || !body.name.trim()) return "name 必填，且必须是非空字符串";
  if (kind === "group") {
    if (body.packages !== undefined) {
      if (!Array.isArray(body.packages)) return "packages 必须是数组（元素是包的 id 字符串）";
      const bad = body.packages.find((x) => typeof x !== "string" || !ID_RE.test(x));
      if (bad !== undefined) return `packages 里有不合法的引用：${JSON.stringify(bad)}`;
    }
  }
  if (kind === "package") {
    if (body.tags !== undefined) {
      if (!Array.isArray(body.tags) || body.tags.some((t) => typeof t !== "string")) return "tags 必须是字符串数组";
    }
    if (body.body !== undefined && typeof body.body !== "string") return "body 字段必须是字符串（正文内联）";
  }
  return null;
}

// ── 应用 ───────────────────────────────────────────────────────────────────

export function createApp({ repo = DEFAULT_REPO } = {}) {
  const ROOT = path.resolve(repo);
  const serialize = makeQueue();

  const repoInfo = async () => {
    const head = await run("git", ["rev-parse", "--short", "HEAD"], { cwd: ROOT });
    const branch = await run("git", ["rev-parse", "--abbrev-ref", "HEAD"], { cwd: ROOT });
    const dirty = await run("git", ["status", "--porcelain"], { cwd: ROOT });
    const remote = await run("git", ["config", "--get", "remote.origin.url"], { cwd: ROOT });
    return {
      path: ROOT,
      head: head.code === 0 ? head.out.trim() : null,
      branch: branch.code === 0 ? branch.out.trim() : null,
      remote: remote.code === 0 ? remote.out.trim() : null,
      dirty: dirty.code === 0 ? dirty.out.trim().split("\n").filter(Boolean).length : null,
      ahead: await aheadCount(ROOT),
      push: PUSH_ENABLED,
    };
  };

  /** 一次写事务：落盘 → 提交 → 尽力推。串行执行。 */
  const transact = (work) =>
    serialize(async () => {
      const written = await work();
      const commit = await commitPaths(ROOT, written.message, written.paths);
      const push = commit.committed ? await pushIfEnabled(ROOT) : { pushed: false, error: null };
      return { ...written, commit, push };
    });

  const handleApi = async (req, res, url) => {
    const seg = url.pathname.split("/").filter(Boolean); // ["v1", ...]
    const [, a, b, c] = seg;

    if (req.method === "GET" && a === "state") {
      const groups = await listItems(ROOT, "group");
      const packages = await listItems(ROOT, "package");
      return send(res, 200, { repo: await repoInfo(), kinds: KINDS, groups, packages });
    }

    if (req.method === "GET" && a === "health") {
      return send(res, 200, { ok: true, repo: ROOT, pid: process.pid, startedAt: STARTED_AT });
    }

    // 手动补推：网络恢复后页面上的「重试推送」走这里，不重复写文件。
    if (req.method === "POST" && a === "push") {
      const push = await serialize(() => pushIfEnabled(ROOT));
      return send(res, 200, { ok: true, push, repo: await repoInfo() });
    }

    // /v1/item/<kind>            PUT = 新建或覆盖（id 在 body 里）
    // /v1/item/<kind>/<id>       PUT / DELETE / GET
    if (a === "item" && b) {
      if (!KINDS[b]) return bad(res, 404, `未知类别：${b}（只有 group / package）`);

      if (req.method === "GET" && c) {
        if (!ID_RE.test(c)) return bad(res, 400, "id 不合法");
        try {
          return send(res, 200, await readItem(ROOT, b, c));
        } catch (error) {
          if (error.code === "ENOENT") return bad(res, 404, `没有这份：${b}/${c}`);
          return bad(res, 500, `读不出来：${firstLine(error.message)}`);
        }
      }

      if (req.method === "DELETE" && c) {
        if (!ID_RE.test(c)) return bad(res, 400, "id 不合法");
        const { file } = pathsFor(ROOT, b);
        const result = await transact(async () => {
          try {
            await rm(file(c));
          } catch (error) {
            if (error.code === "ENOENT") throw Object.assign(new Error(`没有这份：${b}/${c}`), { status: 404 });
            throw error;
          }
          const rel = path.posix.join(KINDS[b].dir, `${c}.json`);
          return { kind: b, id: c, message: `chore(${b}): rm ${c}`, paths: [rel] };
        }).catch((error) => error);
        if (result instanceof Error) return bad(res, result.status ?? 500, error1(result));
        return send(res, 200, { ok: true, kind: b, id: c, commit: result.commit, push: result.push });
      }

      if (req.method === "PUT") {
        const body = await readJson(req);
        if (body.error) return bad(res, 400, body.error);
        const data = body.value;
        const id = c ?? data?.id;
        if (!id) return bad(res, 400, "缺 id（放 body.id，或放到 URL 上）");
        if (!ID_RE.test(id)) return bad(res, 400, "id 只能用 a-z 0-9 连字符，且不以连字符开头（最长 64）");
        if (!data || data.id !== id) return bad(res, 400, `body.id 必须是 ${JSON.stringify(id)}`);
        const problem = validate(b, id, data);
        if (problem) return bad(res, 422, problem);

        const { file } = pathsFor(ROOT, b);
        const existed = await stat(file(id)).then(
          () => true,
          () => false,
        );
        const result = await transact(async () => {
          await writeItem(ROOT, b, id, data);
          const rel = path.posix.join(KINDS[b].dir, `${id}.json`);
          return { kind: b, id, created: !existed, message: `feat(${b}): ${id}`, paths: [rel] };
        }).catch((error) => error);
        if (result instanceof Error) return bad(res, 500, error1(result));
        return send(res, existed ? 200 : 201, {
          ok: true,
          kind: b,
          id,
          created: result.created,
          commit: result.commit,
          push: result.push,
          repo: await repoInfo(),
        });
      }

      return bad(res, 405, `${req.method} 不支持`);
    }

    return bad(res, 404, `未知接口：/v1${a ? `/${a}` : ""}${b ? `/${b}` : ""}`);
  };

  const serveStatic = async (req, res, url) => {
    let rel = decodeURIComponent(url.pathname);
    if (rel === "/" || rel === "") rel = "/index.html";
    const target = path.join(PUBLIC, path.normalize(rel).replace(/^(\.\.[/\\])+/, ""));
    if (!target.startsWith(PUBLIC)) return bad(res, 403, "越界");
    try {
      const st = await stat(target);
      if (st.isDirectory()) return send(res, 404, "不是文件");
      const buf = await readFile(target);
      const type = TYPES[path.extname(target).toLowerCase()] ?? "application/octet-stream";
      res.writeHead(200, { "content-type": type, "cache-control": "no-store" });
      return res.end(buf);
    } catch {
      return send(res, 404, "没这个文件", "text/plain; charset=utf-8");
    }
  };

  return createServer(async (req, res) => {
    const url = new URL(req.url ?? "/", "http://localhost");
    try {
      if (url.pathname.startsWith("/v1/")) return await handleApi(req, res, url);
      return await serveStatic(req, res, url);
    } catch (error) {
      // 兜底：宁可 500 带一句话，也不要连接挂着不给回音。
      return bad(res, 500, `内部错误：${firstLine(error?.stack ?? error)}`);
    }
  });
}

const error1 = (error) => String(error?.message ?? error);

/** 读 body（上限 8MB，够放几万字的正文），JSON 解析失败也当作 400，不 500。 */
function readJson(req) {
  return new Promise((resolve) => {
    let size = 0;
    const chunks = [];
    req.on("data", (chunk) => {
      size += chunk.length;
      if (size > 8 * 1024 * 1024) {
        resolve({ error: "body 太大了（> 8MB）" });
        req.destroy();
        return;
      }
      chunks.push(chunk);
    });
    req.on("end", () => {
      const text = Buffer.concat(chunks).toString("utf8");
      if (!text.trim()) return resolve({ error: "body 是空的" });
      try {
        resolve({ value: JSON.parse(text) });
      } catch (error) {
        resolve({ error: `body 不是合法 JSON：${firstLine(error.message)}` });
      }
    });
    req.on("error", (error) => resolve({ error: `读 body 出错：${firstLine(error.message)}` }));
  });
}

const STARTED_AT = Date.now();

// 直接 `node server.js` 才监听；被测试 import 时不占端口。
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const port = Number(process.env.PORT ?? DEFAULT_PORT);
  const repo = process.env.NOUS_ELEMENTS_REPO ?? DEFAULT_REPO;
  const server = createApp({ repo });
  server.listen(port, "0.0.0.0", () => {
    console.log(`[k3skws] element 在 :${port}，仓库 ${repo}`);
  });
  const shutdown = () => server.close(() => process.exit(0));
  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
}

export { validate, listIds, pathsFor };

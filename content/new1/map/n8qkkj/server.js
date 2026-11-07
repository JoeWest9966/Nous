// n8qkkj「trigger」—— Nous 触发器的清单前端 + 它自己的后端。
//
// 跟 vbe4pt（扩展清单）是一副骨架：卡片 / 类别 / 代码抽屉 / 发布。但有一点不一样 ——
// **这一块自己干活**。扩展装进 qrd3wh 的进程里，所以状态归它、写盘也归它；触发器是
// 一堆独立的 Rust 项目，Nous 在这件事里只负责「管这些项目」：列出来、改源码、编过再提交。
// 把这些动作塞进会话引擎没道理 —— 它跟 agent 跑不跑没关系。
//
// 仓库：`/opt/Nous-Trigger`（环境变量 NOUS_TRIGGER_REPO 可改）。一个目录 = 一个触发器
// = 一个独立的 Rust 项目：Cargo.toml + src/main.rs + nous.json。
//
// 几条不许越过的线：
//   · 保存 ≠ 发布。PUT /file 只落盘；POST /publish 才 cargo check + commit + push。
//   · 发布只发一个触发器：`<name>/` 底下的全部 + index.json，别人的脏文件一个都不带。
//   · 所有 git / 文件写操作串成一队（serializeWrite）—— 两个请求同时 git add 会互相吞。
//   · index.json 永远由 `node index.mjs` 扫出来，这里不手写它。

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { readdir, readFile, rename, rm, stat, writeFile, mkdir, cp } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC = path.join(HERE, "public");
const PORT = Number(process.env.PORT ?? 4783);
const REPO = process.env.NOUS_TRIGGER_REPO ?? "/opt/Nous-Trigger";
const MANIFEST = "nous.json";
const VOCAB = "categories.json";
const ARCHIVE = ".archive";
const ARCHIVE_LOG = path.join(ARCHIVE, "archived.json");
const SKIP_DIRS = new Set(["node_modules", "target", "scripts", "test"]);

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
};

// ── 小工具 ─────────────────────────────────────────────────────────────────

const send = (res, code, body, type = "application/json; charset=utf-8") => {
  const payload = typeof body === "string" || Buffer.isBuffer(body) ? body : JSON.stringify(body);
  res.writeHead(code, { "content-type": type, "cache-control": "no-store" });
  res.end(payload);
};
const bad = (res, code, error, extra = {}) => send(res, code, { error, ...extra });

/** 跑一条命令，拿全 stdout/stderr。**不抛**：失败也是结果，调用方自己看 code。 */
function run(cmd, args, { cwd = REPO, timeout = 120_000 } = {}) {
  return new Promise((resolve) => {
    const child = spawn(cmd, args, { cwd, env: { ...process.env, GIT_TERMINAL_PROMPT: "0" } });
    let out = "";
    let err = "";
    const timer = setTimeout(() => child.kill("SIGKILL"), timeout);
    child.stdout.on("data", (chunk) => (out += chunk));
    child.stderr.on("data", (chunk) => (err += chunk));
    child.on("error", (error) => {
      clearTimeout(timer);
      resolve({ code: -1, out, err: `${err}${error.message}` });
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      resolve({ code: code ?? -1, out, err });
    });
  });
}

const git = (...args) => run("git", args);

/** 写操作一律排队。git index 是全局状态，并发 add 会互相把对方的改动卷进来。 */
let queue = Promise.resolve();
function serializeWrite(task) {
  const next = queue.then(task, task);
  // 队伍不能被一个失败的请求卡死：吞掉这次的结果只影响这一个调用方。
  queue = next.then(
    () => {},
    () => {},
  );
  return next;
}

const sha = (text) => createHash("sha256").update(text).digest("hex").slice(0, 12);

/**
 * `git add -A -- <paths>`，但只喂**真的存在**的路径。
 *
 * 踩过：归档时先 `git mv <name> .archive/<name>`，再 add 的时候 `<name>/` 已经不在了 ——
 * 一个匹配不到的 pathspec 会让整条 add 以 128 退出，**一个字节都不进去**，而 `git commit -- <paths>`
 * 又带不上 untracked 文件，于是那份账本（.archive/archived.json）连着三次都没进仓库。
 * 已经被 mv / 删掉的路径本来也不需要 add：mv 自己就把两边 staged 了。
 */
async function stage(...rels) {
  const present = [];
  for (const rel of rels) {
    try {
      await stat(path.join(REPO, rel));
      present.push(rel);
    } catch {
      // 不在了就算了 —— 它的删除/移动早就在 index 里了。
    }
  }
  if (present.length) await git("add", "-A", "--", ...present);
}

/** 目录名就是身份，所以名字只许这一套字符：能当目录名、能当二进制名。 */
const NAME_RE = /^[a-z0-9][a-z0-9._-]*$/;

/** 触发器名 → 目录。挡掉穿越和奇怪输入，别让 `../etc` 变成一个合法身份。 */
function dirOf(name) {
  const value = String(name ?? "").trim();
  if (!NAME_RE.test(value)) throw Object.assign(new Error(`名字不合法：${value || "(空)"}`), { status: 400 });
  return path.join(REPO, value);
}

async function readJsonFile(file, fallback) {
  try {
    return JSON.parse(await readFile(file, "utf8"));
  } catch {
    return fallback;
  }
}

async function writeJsonFile(file, value) {
  await writeFile(file, `${JSON.stringify(value, null, 2)}\n`);
}

// ── 仓库状态 ───────────────────────────────────────────────────────────────

/** `git status --porcelain` → 第一段路径。触发器目录由它算「脏」，别的都不是。 */
async function dirtyMap() {
  const { out } = await git("status", "--porcelain=v1", "-z");
  const map = new Map();
  for (const entry of out.split("\0")) {
    if (!entry.trim()) continue;
    const file = entry.slice(3);
    const top = file.split("/")[0];
    if (!top) continue;
    map.set(top, [...(map.get(top) ?? []), file]);
  }
  return map;
}

async function trackedDirs() {
  const { out } = await git("ls-tree", "--name-only", "HEAD");
  return new Set(out.split("\n").map((line) => line.trim()).filter(Boolean));
}

async function repoState() {
  const [head, branch, ahead, behind, status] = await Promise.all([
    git("rev-parse", "--short", "HEAD"),
    git("rev-parse", "--abbrev-ref", "HEAD"),
    git("rev-list", "--count", "@{u}..HEAD"),
    git("rev-list", "--count", "HEAD..@{u}"),
    git("status", "--porcelain=v1"),
  ]);
  return {
    path: REPO,
    head: head.out.trim(),
    branch: branch.out.trim() || "HEAD",
    ahead: Number(ahead.out.trim() || 0),
    behind: Number(behind.out.trim() || 0),
    dirty: Boolean(status.out.trim()),
    dirtyCount: status.out.trim() ? status.out.trim().split("\n").length : 0,
  };
}

/** 磁盘上的目录（排除点开头、target、node_modules…）。 */
async function projectDirs() {
  const entries = await readdir(REPO, { withFileTypes: true });
  return entries
    .filter((entry) => entry.isDirectory() && !entry.name.startsWith(".") && !SKIP_DIRS.has(entry.name))
    .map((entry) => entry.name)
    .sort();
}

/** 一个目录 → 一张卡要的数据。清单坏了也得出一张卡（卡片上要能看出它坏了）。 */
async function readTrigger(name, { tracked, dirty }) {
  const dir = path.join(REPO, name);
  let manifest = null;
  let error = "";
  try {
    manifest = JSON.parse(await readFile(path.join(dir, MANIFEST), "utf8"));
  } catch (failure) {
    error = failure.code === "ENOENT" ? `没有 ${MANIFEST}` : `${MANIFEST} 读不动：${failure.message}`;
  }
  const broken = !manifest || !runningOk(manifest, name);
  return {
    name,
    version: manifest?.version ?? "0.0.0",
    summary: manifest?.summary ?? "",
    category: Array.isArray(manifest?.category) ? manifest.category.filter(Boolean) : [],
    binary: manifest?.binary ?? name,
    // 参数声明：装配台照它画表单，详情页照它说「这个触发器要人填什么」。
    params: Array.isArray(manifest?.params) ? manifest.params : [],
    tracked,
    dirty: Boolean(dirty),
    dirtyFiles: dirty?.length ?? 0,
    error: broken ? error || `清单跟目录对不上（name 是「${manifest?.name ?? "?"}」）` : "",
  };
}

/** 清单合不合法：必填四项 + name 与目录同名。跟 index.mjs 同一套判断。 */
function runningOk(manifest, dir) {
  const required = ["name", "version", "summary", "category"];
  if (required.some((field) => manifest[field] === undefined)) return false;
  if (manifest.name !== dir) return false;
  const category = Array.isArray(manifest.category) ? manifest.category : [manifest.category];
  return category.every((label) => typeof label === "string" && label.trim());
}

async function scan() {
  const [dirs, tracked, dirty] = await Promise.all([projectDirs(), trackedDirs(), dirtyMap()]);
  const triggers = await Promise.all(
    dirs.map((name) => readTrigger(name, { tracked: tracked.has(name), dirty: dirty.get(name) })),
  );
  const archived = await readJsonFile(path.join(REPO, ARCHIVE_LOG), {});
  return { triggers, archived };
}

// ── 索引：永远扫出来，不手写 ────────────────────────────────────────────────

/** 跑仓库自己的 index.mjs：它才是 index.json 的唯一作者（校验也归它）。 */
async function rescanIndex() {
  const result = await run("node", ["index.mjs"], { timeout: 30_000 });
  if (result.code !== 0) {
    throw Object.assign(new Error(`index.mjs 没过：${(result.err || result.out).trim()}`), { status: 422 });
  }
  return result.out.trim();
}

// ── 读一个触发器能编辑的文件 ────────────────────────────────────────────────

const KIND = { ".rs": "code", ".toml": "code", ".json": "json", ".md": "markdown", ".txt": "markdown" };
const EDITABLE = new Set(Object.keys(KIND));

async function listFiles(dir, base = "") {
  const out = [];
  const entries = await readdir(path.join(dir, base), { withFileTypes: true });
  for (const entry of entries) {
    if (entry.name.startsWith(".")) continue;
    const rel = base ? `${base}/${entry.name}` : entry.name;
    if (entry.isDirectory()) {
      if (SKIP_DIRS.has(entry.name)) continue;
      out.push(...(await listFiles(dir, rel)));
      continue;
    }
    const ext = path.extname(entry.name);
    if (!EDITABLE.has(ext)) continue;
    const info = await stat(path.join(dir, rel));
    out.push({ path: rel, size: info.size, kind: KIND[ext] });
  }
  // 清单排第一，然后是 Rust 源码，再是按名字 —— 「先看它是什么，再看它怎么写」。
  const rank = (file) => (file.path === MANIFEST ? 0 : file.path.endsWith(".rs") ? 1 : file.path === "Cargo.toml" ? 2 : 3);
  return out.sort((a, b) => rank(a) - rank(b) || a.path.localeCompare(b.path));
}

/** 路径必须落在这个触发器目录里：`..`、绝对路径、target/ 一律挡掉。 */
function safeJoin(dir, rel) {
  const value = String(rel ?? "").trim();
  if (!value || path.isAbsolute(value) || value.split("/").includes("..")) {
    throw Object.assign(new Error(`路径不合法：${value || "(空)"}`), { status: 400 });
  }
  const full = path.join(dir, value);
  if (!full.startsWith(dir + path.sep)) throw Object.assign(new Error("路径出界"), { status: 400 });
  if (value.split("/")[0] === "target") throw Object.assign(new Error("target/ 是编译产物"), { status: 400 });
  return full;
}

// ── 类别词表 ───────────────────────────────────────────────────────────────

const ancestorsOf = (name) => {
  const parts = String(name).split("/");
  return parts.slice(0, -1).map((_, index) => parts.slice(0, index + 1).join("/"));
};

/** 词表 + 所有触发器上用到的类别（含祖先）。建过但没挂东西的类别也得在词表里。 */
async function categoryVocab(triggers) {
  const vocab = await readJsonFile(path.join(REPO, VOCAB), []);
  const set = new Set(Array.isArray(vocab) ? vocab : []);
  for (const trigger of triggers) {
    for (const label of trigger.category) {
      for (const parent of ancestorsOf(label)) set.add(parent);
      set.add(label);
    }
  }
  return [...set].sort((a, b) => a.localeCompare(b, "zh-CN"));
}

// ── 动作 ───────────────────────────────────────────────────────────────────

/** 写一个触发器的清单（照着手上的对象补字段，别整份覆盖掉别人的字段）。 */
async function writeManifest(name, patch) {
  const dir = dirOf(name);
  const file = path.join(dir, MANIFEST);
  const manifest = await readJsonFile(file, null);
  if (!manifest) throw Object.assign(new Error(`${name}/${MANIFEST} 不存在`), { status: 404 });
  const next = { ...manifest, ...patch };
  await writeJsonFile(file, next);
  return next;
}

async function setCategory(name, category) {
  const dir = dirOf(name);
  const file = path.join(dir, MANIFEST);
  const manifest = await readJsonFile(file, null);
  if (!manifest) throw Object.assign(new Error(`${name}/${MANIFEST} 不存在`), { status: 404 });
  const list = [...new Set((category ?? []).map((label) => String(label).trim()).filter(Boolean))];
  await writeJsonFile(file, { ...manifest, category: list });
  // 词表也要收下新类别 —— 没进词表的话，下次新建时就选不到它了。
  const vocab = await readJsonFile(path.join(REPO, VOCAB), []);
  const merged = new Set(Array.isArray(vocab) ? vocab : []);
  for (const label of list) {
    for (const parent of ancestorsOf(label)) merged.add(parent);
    merged.add(label);
  }
  await writeJsonFile(path.join(REPO, VOCAB), [...merged].sort((a, b) => a.localeCompare(b, "zh-CN")));
}

async function editVocab(op, name, to) {
  const file = path.join(REPO, VOCAB);
  const vocab = await readJsonFile(file, []);
  const list = Array.isArray(vocab) ? vocab : [];
  const label = String(name ?? "").trim();
  if (!label) throw Object.assign(new Error("类别名是空的"), { status: 400 });

  if (op === "add") {
    const merged = new Set(list);
    for (const parent of ancestorsOf(label)) merged.add(parent);
    merged.add(label);
    await writeJsonFile(file, [...merged].sort((a, b) => a.localeCompare(b, "zh-CN")));
    return;
  }
  if (op === "rename") {
    const next = String(to ?? "").trim();
    if (!next) throw Object.assign(new Error("新名字是空的"), { status: 400 });
    // 改名 = 从每个触发器上摘掉旧的挂上新的（子类别一起搬）。
    for (const dir of await projectDirs()) {
      const manifest = await readJsonFile(path.join(REPO, dir, MANIFEST), null);
      if (!manifest) continue;
      const current = Array.isArray(manifest.category) ? manifest.category : [];
      const mapped = current.map((tag) => (tag === label ? next : tag.startsWith(`${label}/`) ? `${next}${tag.slice(label.length)}` : tag));
      if (mapped.join("\0") !== current.join("\0")) await writeJsonFile(path.join(REPO, dir, MANIFEST), { ...manifest, category: mapped });
    }
    const renamed = list.map((tag) => (tag === label ? next : tag.startsWith(`${label}/`) ? `${next}${tag.slice(label.length)}` : tag));
    const merged = new Set(renamed);
    for (const parent of ancestorsOf(next)) merged.add(parent);
    await writeJsonFile(file, [...merged].sort((a, b) => a.localeCompare(b, "zh-CN")));
    return;
  }
  if (op === "remove") {
    for (const dir of await projectDirs()) {
      const manifest = await readJsonFile(path.join(REPO, dir, MANIFEST), null);
      if (!manifest) continue;
      const current = Array.isArray(manifest.category) ? manifest.category : [];
      const kept = current.filter((tag) => tag !== label && !tag.startsWith(`${label}/`));
      if (kept.length !== current.length) await writeJsonFile(path.join(REPO, dir, MANIFEST), { ...manifest, category: kept });
    }
    const kept = list.filter((tag) => tag !== label && !tag.startsWith(`${label}/`));
    await writeJsonFile(file, kept);
    return;
  }
  throw Object.assign(new Error(`不认识的 op：${op}`), { status: 400 });
}

/** 新建：目录 + Cargo.toml + src/main.rs + nous.json。建完**不提交**。 */
async function createTrigger({ name, summary, category }) {
  const dir = dirOf(name);
  try {
    await stat(dir);
    throw Object.assign(new Error(`已经有一个叫 ${name} 的了`), { status: 409 });
  } catch (error) {
    if (error.status) throw error;
  }

  await mkdir(path.join(dir, "src"), { recursive: true });
  await writeFile(
    path.join(dir, "Cargo.toml"),
    [
      "[package]",
      `name = "${name}"`,
      'version = "0.1.0"',
      'edition = "2021"',
      "",
      "[[bin]]",
      `name = "${name}"`,
      'path = "src/main.rs"',
      "",
      "[dependencies]",
      "",
    ].join("\n"),
  );
  // 骨架必须是个**能编过**的程序：建完立刻点发布也不能红。契约写在注释里 ——
  // 「stdin 一行 JSON 进、stdout 一行 JSON 出」这件事，只有这份模板能告诉写它的人。
  await writeFile(
    path.join(dir, "src", "main.rs"),
    [
      `//! ${SUMMARY(summary)}`,
      "//!",
      "//! 契约（见仓库 README）：",
      '//!   stdin  一行 JSON：{"now":<epoch 秒>, "params":{…}}',
      '//!   stdout 一行 JSON：{"fire":true|false,"reason":"…","data":{…}}',
      "//!   退出码 0 = 正常（fire 真假都算）；非 0 = 出错。",
      "//!",
      "//! 触发器只回答「现在该不该发起」—— 起什么样的对话是模板的事，别在这里写模板名。",
      "",
      "fn main() {",
      '    println!(r#"{{"fire":false,"reason":"还没写条件"}}"#);',
      "}",
      "",
    ].join("\n"),
  );
  await writeJsonFile(path.join(dir, MANIFEST), {
    name,
    version: "0.1.0",
    summary: summary || "（还没写说明）",
    category: (category ?? []).map((label) => String(label).trim()).filter(Boolean),
    // 参数声明留个空数组：装配台看这个字段决定要不要画表单。
    params: [],
  });

  await rescanIndex();
  return readTrigger(name, { tracked: false, dirty: null });
}

const SUMMARY = (text) => String(text ?? "").replace(/\s+/g, " ").trim() || "还没写说明";

/** 发布一个触发器：cargo check → 重扫索引 → 只 add 它自己 → commit → push。 */
async function publishTrigger(name) {
  const dir = dirOf(name);
  await stat(dir).catch(() => {
    throw Object.assign(new Error(`${name} 不在磁盘上`), { status: 404 });
  });

  // 产物必须落在这个触发器自己的 target/ 里（调度器按 `<dir>/target/debug/<name>` 找它）。
  // 不写死 --target-dir，机器上的 CARGO_TARGET_DIR 会把它卷到别处去，调度器就找不到。
  const cargo = (args, timeout) =>
    run("cargo", [...args, "--target-dir", path.join(dir, "target")], { cwd: dir, timeout });

  // 编不过就别提交 —— 一个编不过的触发器，push 上去只会让下一个人 pull 到坏东西。
  const check = await cargo(["check", "--quiet"], 180_000);
  if (check.code !== 0) {
    const detail = `${check.err}\n${check.out}`.trim().split("\n").slice(0, 40).join("\n");
    throw Object.assign(new Error(`cargo check 没过：\n${detail}`), { status: 422, stage: "check" });
  }

  // 光 check 不够：调度器跑的是 `target/debug/<name>` 这个**产物**。只 check 不 build，
  // 触发器在调度器眼里就是「没编译、跑不了」。debug 就够（跑一次毫秒级），不用 release。
  const build = await cargo(["build", "--quiet"], 300_000);
  if (build.code !== 0) {
    const detail = `${build.err}\n${build.out}`.trim().split("\n").slice(0, 40).join("\n");
    throw Object.assign(new Error(`cargo build 没过：\n${detail}`), { status: 422, stage: "build" });
  }

  const manifest = await readJsonFile(path.join(dir, MANIFEST), null);
  if (!manifest) throw Object.assign(new Error(`${name}/${MANIFEST} 不见了`), { status: 422 });
  await rescanIndex();

  // 只动这一个：`<name>/` 底下的全部 + index.json。别人的脏文件一个都不许卷进来。
  await stage(`${name}/`, "index.json");
  const staged = await git("diff", "--cached", "--name-only");
  if (!staged.out.trim()) {
    const state = await repoState();
    return { committed: false, pushed: false, ahead: state.ahead, head: state.head };
  }

  const subject = `trigger(${name}): v${manifest.version} · ${SUMMARY(manifest.summary)}`;
  const commit = await git("commit", "-m", subject, "--", `${name}/`, "index.json");
  if (commit.code !== 0) {
    throw Object.assign(new Error(`git commit 失败：${(commit.err || commit.out).trim()}`), { status: 500 });
  }
  const head = (await git("rev-parse", "--short", "HEAD")).out.trim();

  // 分支上没设上游时 `push` 会说「没有上游」—— 第一次发布顺手把上游建起来。
  const push = await git("push");
  const state = await repoState();
  return {
    committed: true,
    pushed: push.code === 0,
    pushError: push.code === 0 ? "" : (push.err || push.out).trim(),
    head,
    branch: state.branch,
    ahead: state.ahead,
  };
}

/** 只提交词表 / 清单（不碰任何 .rs）：类别这种改动不该等一次「发布」。 */
async function syncMeta() {
  const candidates = [VOCAB, "index.json"];
  for (const dir of await projectDirs()) candidates.push(`${dir}/${MANIFEST}`);
  const dirty = await git("status", "--porcelain=v1", "--", ...candidates);
  const files = dirty.out
    .split("\n")
    .map((line) => line.slice(3).trim())
    .filter(Boolean);
  if (!files.length) {
    const state = await repoState();
    return { committed: false, pushed: false, ahead: state.ahead, behind: state.behind };
  }
  await rescanIndex();
  await stage(...candidates);
  const staged = await git("diff", "--cached", "--name-only");
  if (!staged.out.trim()) {
    const state = await repoState();
    return { committed: false, pushed: false, ahead: state.ahead, behind: state.behind };
  }
  await git("commit", "-m", "trigger: 类别词表 / 清单同步", "--", ...candidates);
  const push = await git("push");
  const state = await repoState();
  return {
    committed: true,
    pushed: push.code === 0,
    pushError: push.code === 0 ? "" : (push.err || push.out).trim(),
    ahead: state.ahead,
    behind: state.behind,
    files,
  };
}

/** 归档：搬进 .archive/ 并提交推送。没发布过的直接删 —— 仓库里从来就没有它。 */
async function archiveTrigger(name) {
  const dir = dirOf(name);
  await stat(dir).catch(() => {
    throw Object.assign(new Error(`${name} 不在磁盘上`), { status: 404 });
  });
  const tracked = (await trackedDirs()).has(name);
  await mkdir(path.join(REPO, ARCHIVE), { recursive: true });

  // 搬走之前先记一笔：恢复的时候要照着它把卡片还回去（清单可能已经跟着走了）。
  const manifest = await readJsonFile(path.join(dir, MANIFEST), {});
  const log = await readJsonFile(path.join(REPO, ARCHIVE_LOG), {});
  log[name] = {
    at: new Date().toISOString(),
    version: manifest.version ?? "0.0.0",
    summary: manifest.summary ?? "",
    category: Array.isArray(manifest.category) ? manifest.category : [],
  };
  await writeJsonFile(path.join(REPO, ARCHIVE_LOG), log);

  if (tracked) await git("mv", name, `${ARCHIVE}/${name}`);
  else await rename(dir, path.join(REPO, ARCHIVE, name));

  await rescanIndex();
  await stage(`${ARCHIVE}/`, "index.json");
  await git("commit", "-m", `archive(${name}): 移出仓库`, "--", `${ARCHIVE}/`, "index.json");
  const push = await git("push");
  return {
    dropped: !tracked,
    pushed: push.code === 0,
    pushError: push.code === 0 ? "" : (push.err || push.out).trim(),
  };
}

async function restoreTrigger(name) {
  const from = path.join(REPO, ARCHIVE, name);
  const to = dirOf(name);
  await stat(from).catch(() => {
    throw Object.assign(new Error(`.archive/${name} 不存在`), { status: 404 });
  });
  await git("mv", `${ARCHIVE}/${name}`, name);
  const log = await readJsonFile(path.join(REPO, ARCHIVE_LOG), {});
  delete log[name];
  await writeJsonFile(path.join(REPO, ARCHIVE_LOG), log);
  await rescanIndex();
  await stage(`${name}/`, `${ARCHIVE}/`, "index.json");
  await git("commit", "-m", `restore(${name}): 从归档搬回来`, "--", `${name}/`, `${ARCHIVE}/`, "index.json");
  const push = await git("push");
  return { pushed: push.code === 0, pushError: push.code === 0 ? "" : (push.err || push.out).trim(), at: to };
}

async function purgeTrigger(name) {
  const dir = path.join(REPO, ARCHIVE, name);
  await stat(dir).catch(() => {
    throw Object.assign(new Error(`.archive/${name} 不存在`), { status: 404 });
  });
  await rm(dir, { recursive: true, force: true });
  const log = await readJsonFile(path.join(REPO, ARCHIVE_LOG), {});
  delete log[name];
  await writeJsonFile(path.join(REPO, ARCHIVE_LOG), log);
  await stage(`${ARCHIVE}/`);
  await git("commit", "-m", `purge(${name}): 从归档里彻底删除`, "--", `${ARCHIVE}/`);
  const push = await git("push");
  return { pushed: push.code === 0, pushError: push.code === 0 ? "" : (push.err || push.out).trim() };
}

// ── HTTP ───────────────────────────────────────────────────────────────────

async function body(req) {
  let raw = "";
  for await (const chunk of req) raw += chunk;
  if (!raw) return {};
  try {
    return JSON.parse(raw);
  } catch {
    throw Object.assign(new Error("请求体不是 JSON"), { status: 400 });
  }
}

/** 块挂在 `/p/<board>/<id>/` 下面，进程只认自己那截路径。 */
const stripSelf = (url) => {
  const self = process.env.NOUS_SELF;
  if (self && url.startsWith(self)) {
    const rest = url.slice(self.length);
    return rest.startsWith("/") ? rest : `/${rest}`;
  }
  return url;
};

async function api(req, res, pathname, url) {
  const verb = req.method ?? "GET";

  if (pathname === "/v1/triggers" && verb === "GET") {
    const [{ triggers, archived }, repo] = await Promise.all([scan(), repoState()]);
    const categories = await categoryVocab(triggers);
    return send(res, 200, {
      triggers,
      catalog: triggers,
      repo,
      categories,
      archived: Object.entries(archived)
        .map(([name, meta]) => ({ name, ...meta }))
        .sort((a, b) => String(b.at).localeCompare(String(a.at))),
      // 这一页没有「内置」「核心」「已装载」这些概念：触发器不进进程。
      builtin: [],
      protected: [],
    });
  }

  if (pathname === "/v1/triggers/sync" && verb === "GET") {
    const repo = await repoState();
    return send(res, 200, { ...repo, dirty: repo.dirty ? 1 : 0 });
  }

  if (pathname === "/v1/triggers/sync" && verb === "POST") {
    const result = await serializeWrite(syncMeta);
    return send(res, 200, result);
  }

  if (pathname === "/v1/triggers/files" && verb === "GET") {
    const name = url.searchParams.get("name") ?? "";
    const dir = dirOf(name);
    await stat(dir).catch(() => {
      throw Object.assign(new Error(`${name} 不在磁盘上`), { status: 404 });
    });
    const files = await listFiles(dir);
    return send(res, 200, { files });
  }

  if (pathname === "/v1/triggers/file" && verb === "GET") {
    const name = url.searchParams.get("name") ?? "";
    const rel = url.searchParams.get("path") ?? "";
    const dir = dirOf(name);
    const full = safeJoin(dir, rel);
    let text;
    try {
      text = await readFile(full, "utf8");
    } catch {
      throw Object.assign(new Error(`读不到 ${name}/${rel}`), { status: 404 });
    }
    // base = HEAD 里那一版：差异视图跟它比，不是跟「上次保存」比。
    const base = await git("show", `HEAD:${name}/${rel}`);
    const ext = path.extname(rel);
    return send(res, 200, {
      path: rel,
      kind: KIND[ext] ?? "code",
      text,
      etag: sha(text),
      base: base.code === 0 ? base.out : null,
    });
  }

  if (pathname === "/v1/triggers/file" && verb === "PUT") {
    const payload = await body(req);
    const dir = dirOf(payload.name);
    const full = safeJoin(dir, payload.path);
    const current = await readFile(full, "utf8").catch(() => null);
    if (current === null) throw Object.assign(new Error("文件不存在"), { status: 404 });
    // base 是「我改的是哪一版」：别人先改了，就别把对方的字盖掉。
    if (payload.base && sha(current) !== payload.base) {
      throw Object.assign(new Error("这个文件在别处被改过了（你看到的是旧内容）。刷新一下再改。"), { status: 409 });
    }
    if (typeof payload.text !== "string") throw Object.assign(new Error("text 不是字符串"), { status: 400 });
    await writeFile(full, payload.text);
    return send(res, 200, { etag: sha(payload.text) });
  }

  if (pathname === "/v1/triggers/category" && verb === "POST") {
    const payload = await body(req);
    const name = String(payload.name ?? "");
    await serializeWrite(() => setCategory(name, payload.category));
    const { triggers } = await scan();
    return send(res, 200, { catalog: triggers, categories: await categoryVocab(triggers) });
  }

  if (pathname === "/v1/triggers/categories" && verb === "POST") {
    const payload = await body(req);
    await serializeWrite(() => editVocab(payload.op, payload.name, payload.to));
    const { triggers } = await scan();
    return send(res, 200, { catalog: triggers, categories: await categoryVocab(triggers) });
  }

  if (pathname === "/v1/triggers/create" && verb === "POST") {
    const payload = await body(req);
    const trigger = await serializeWrite(() => createTrigger(payload));
    return send(res, 200, { trigger, manifest: trigger });
  }

  if (pathname === "/v1/triggers/publish" && verb === "POST") {
    const payload = await body(req);
    const result = await serializeWrite(() => publishTrigger(String(payload.name ?? "")));
    return send(res, 200, result);
  }

  if (pathname === "/v1/triggers/archive" && verb === "POST") {
    const payload = await body(req);
    return send(res, 200, await serializeWrite(() => archiveTrigger(String(payload.name ?? ""))));
  }

  if (pathname === "/v1/triggers/restore" && verb === "POST") {
    const payload = await body(req);
    return send(res, 200, await serializeWrite(() => restoreTrigger(String(payload.name ?? ""))));
  }

  if (pathname === "/v1/triggers/purge" && verb === "POST") {
    const payload = await body(req);
    return send(res, 200, await serializeWrite(() => purgeTrigger(String(payload.name ?? ""))));
  }

  return bad(res, 404, `没有这个接口：${verb} ${pathname}`);
}

const server = createServer(async (req, res) => {
  const raw = stripSelf(req.url ?? "/");
  const url = new URL(raw, "http://nous.local");
  const pathname = decodeURIComponent(url.pathname);

  try {
    if (pathname.startsWith("/v1/")) return await api(req, res, pathname, url);

    const file = pathname === "/" || pathname === "" ? "index.html" : pathname.replace(/^\/+/, "");
    const target = path.join(PUBLIC, file);
    if (!target.startsWith(PUBLIC + path.sep)) return send(res, 403, "forbidden", "text/plain; charset=utf-8");
    const content = await readFile(target);
    return send(res, 200, content, TYPES[path.extname(target)] ?? "application/octet-stream");
  } catch (error) {
    // 静态文件找不到不是「服务器坏了」：不要打成 500，也不要在日志里当成事故。
    const status = error.status ?? (error.code === "ENOENT" ? 404 : 500);
    if (status >= 500) console.error(`[n8qkkj] ${req.method} ${pathname} → ${error.message}`);
    return bad(res, status, String(error.message ?? error));
  }
});

server.listen(PORT, () => console.log(`[n8qkkj] 听 ${PORT} · 仓库 ${REPO}`));

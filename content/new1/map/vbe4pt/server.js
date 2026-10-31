// vbe4pt「extend」—— pi 扩展管理器。
//
// 职责：扫描盘上散落的 pi extensions（全局 ~/.pi/agent/extensions/ + 白名单里的
// 项目 .pi/extensions/），和块内库 data/lib/ 对账；「收进库」= 把盘上的副本拷进来，
// 「装出去」= 从库拷到目标安装位。真正的真相永远在安装位 —— pi 只认盘上的文件，
// 这里只是台账 + 搬运工，不做运行时热切换（pi 没这层，生效靠 /reload）。
//
// 扫描范围是白名单制：全局目录 + registry.json 里登记过的目录。不扫家目录。
//
// 宿主给 PORT / NOUS_DATA / NOUS_ORIGIN / NOUS_SELF。这里只用 PORT / NOUS_DATA。

import { createServer } from "node:http";
import {
  readFile,
  writeFile,
  mkdir,
  readdir,
  stat,
  copyFile,
  unlink,
  rename,
  rm,
} from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC = path.join(HERE, "public");
const DATA = process.env.NOUS_DATA ?? path.join(HERE, "data");
const LIB = path.join(DATA, "lib");
const REGISTRY = path.join(DATA, "registry.json");
const PORT = Number(process.env.PORT ?? 0);

const GLOBAL_DIR = path.join(os.homedir(), ".pi", "agent", "extensions");
const REVS = path.join(DATA, "revs"); // 每次保存前留一版快照：revs/<id>/<n>.ts
const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
};

const now = () => new Date().toISOString();
const oneLine = (e) => String(e?.message ?? e).split("\n")[0];

import { TEMPLATES } from "./templates.js";

// ── registry.json：台账，唯一真相是「库里有啥 + 关注哪些目录」 ────────────────

let reg = { v: 1, watch: [], items: [] };
let regLoaded = false;

async function loadRegistry() {
  if (regLoaded) return reg;
  try {
    const raw = JSON.parse(await readFile(REGISTRY, "utf8"));
    if (raw && Array.isArray(raw.items)) reg = { v: 1, watch: raw.watch ?? [], items: raw.items };
  } catch {
    /* 没有 registry 就是空台账 */
  }
  regLoaded = true;
  return reg;
}

async function saveRegistry() {
  await mkdir(DATA, { recursive: true });
  await writeFile(REGISTRY, JSON.stringify(reg, null, 2) + "\n");
}

/** 台账条目：{ id, name, scope: "global"|"project", origin, files: [相对 lib/ 的路径],
 *             installDirs: [安装位目录], notes, updatedAt } */
function findItem(id) {
  return reg.items.find((r) => r.id === id);
}

// ── 扫描 ────────────────────────────────────────────────────────────────────

/** 一个安装位目录 → 发现列表。认 .ts 文件和 <dir>/index.ts 子目录两种形态，
 *  `.disabled.` 后缀的算「已禁用」。 */
async function scanDir(dir) {
  const out = [];
  let names;
  try {
    names = await readdir(dir);
  } catch {
    return out;
  }
  for (const name of names) {
    const p = path.join(dir, name);
    let st;
    try {
      st = await stat(p);
    } catch {
      continue;
    }
    const disabled = /\.disabled(\.|$)/.test(name);
    if (st.isDirectory()) {
      // 子目录形态：有 index.ts（或 index.disabled.ts）才算扩展
      const idx = path.join(p, "index.ts");
      const idxOff = path.join(p, "index.disabled.ts");
      if (existsSync(idx) || existsSync(idxOff)) {
        out.push({ dir, folder: name, file: "index.ts", disabled, mtime: (await stat(existsSync(idx) ? idx : idxOff)).mtime.toISOString() });
      }
    } else if (name.endsWith(".ts")) {
      out.push({ dir, folder: null, file: name, disabled, mtime: st.mtime.toISOString() });
    }
  }
  return out;
}

/** 盘上有什么：全局目录 + watch 白名单里的目录。 */
async function scanDisk() {
  const dirs = [GLOBAL_DIR, ...reg.watch];
  const found = [];
  for (const dir of [...new Set(dirs)]) {
    for (const e of await scanDir(dir)) {
      found.push({ scope: dir === GLOBAL_DIR ? "global" : "project", dir, ...e });
    }
  }
  return found;
}

/** 库里有什么。lib/<id>/ 下是源文件。 */
async function scanLib() {
  const out = [];
  let names;
  try {
    names = await readdir(LIB);
  } catch {
    return out;
  }
  for (const id of names) {
    const dir = path.join(LIB, id);
    if (!(await stat(dir).then((s) => s.isDirectory()).catch(() => false))) continue;
    const files = [];
    for (const f of await readdir(dir)) {
      const st = await stat(path.join(dir, f));
      if (st.isFile()) files.push({ name: f, mtime: st.mtime.toISOString() });
    }
    out.push({ id, files });
  }
  return out;
}

/** 安装位 id → 扩展名。磁盘形态是 <dir>/<name>.ts、<dir>/<name>.disabled.ts 或 <dir>/<name>/index.ts。 */
function extNameFromEntry(e) {
  const base = e.folder ?? e.file;
  return base.replace(/\.disabled\.ts$/, "").replace(/\.ts$/, "");
}

// ── 对账视图：库 ∪ 盘，按扩展名归并 ─────────────────────────────────────────

async function inventory() {
  const reg_ = await loadRegistry();
  const [disk, lib] = await Promise.all([scanDisk(), scanLib()]);
  const meta = new Map(reg_.items.map((r) => [r.id, r]));

  const rows = [];
  const seen = new Set();

  // 1) 库里的每一条
  for (const l of lib) {
    const m = meta.get(l.id);
    const row = {
      id: l.id,
      name: m?.name ?? l.id,
      scope: m?.scope ?? "global",
      notes: m?.notes ?? "",
      lib: l.files,
      installs: [],
      orphan: !m,
    };
    // 盘上装在哪儿
    for (const d of disk) {
      const nm = extNameFromEntry(d);
      if (nm === row.name) row.installs.push({ dir: d.dir, scope: d.scope, disabled: d.disabled, mtime: d.mtime });
    }
    rows.push(row);
    seen.add(row.name);
  }

  // 2) 盘上有、库里没有的（未收编）
  for (const d of disk) {
    const nm = extNameFromEntry(d);
    if (seen.has(nm)) continue;
    seen.add(nm);
    rows.push({
      id: null,
      name: nm,
      scope: d.scope,
      notes: "",
      lib: null,
      installs: [{ dir: d.dir, scope: d.scope, disabled: d.disabled, mtime: d.mtime }],
      orphan: false,
    });
  }

  rows.sort((a, b) => (a.scope === b.scope ? a.name.localeCompare(b.name) : a.scope === "global" ? -1 : 1));
  return { rows, watch: reg_.watch, globalDir: GLOBAL_DIR };
}

// ── 动作 ────────────────────────────────────────────────────────────────────

/** 收进库：把盘上某安装位的源文件拷到 lib/<id>/。 */
async function adopt(body) {
  const dir = body.dir;
  const name = body.name;
  if (!dir || !name) throw new Error("需要 dir 和 name");
  // 安全：dir 必须是已知安装位（全局或白名单），name 不含路径分隔符
  const reg_ = await loadRegistry();
  const allowed = [GLOBAL_DIR, ...reg_.watch];
  if (!allowed.includes(path.resolve(dir))) throw new Error("目录不在白名单里：" + dir);
  if (!/^[\w.-]+$/.test(name)) throw new Error("名字不合法：" + name);

  const disk = await scanDisk();
  const entry = disk.find((e) => e.dir === path.resolve(dir) && extNameFromEntry(e) === name);
  if (!entry) throw new Error("盘上没找到：" + name + " @ " + dir);

  const id = "x" + Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
  const dest = path.join(LIB, id);
  await mkdir(dest, { recursive: true });

  let files;
  if (entry.folder) {
    // 子目录形态：整目录拷贝
    files = await copyDir(path.join(dir, entry.folder), dest);
  } else {
    const src = path.join(dir, entry.file);
    await copyFile(src, path.join(dest, entry.file));
    files = [entry.file];
  }

  const item = {
    id,
    name,
    scope: entry.scope,
    origin: path.join(dir, entry.folder ?? entry.file),
    files,
    installDirs: [],
    notes: "",
    updatedAt: now(),
  };
  reg.items.push(item);
  await saveRegistry();
  return item;
}

async function copyDir(src, dest) {
  const files = [];
  for (const f of await readdir(src, { withFileTypes: true })) {
    if (f.isFile()) {
      await copyFile(path.join(src, f.name), path.join(dest, f.name));
      files.push(f.name);
    } else if (f.isDirectory()) {
      const sub = await copyDir(path.join(src, f.name), path.join(dest, f.name));
      files.push(...sub.map((s) => path.join(f.name, s)));
    }
  }
  return files;
}

/** 装出去：从库拷到目标目录（目录自动建）。target 必须在白名单或以白名单目录开头。 */
async function installTo(id, target) {
  const reg_ = await loadRegistry();
  const item = findItem(id);
  if (!item) throw new Error("库里没有 " + id);
  const allowed = [GLOBAL_DIR, ...reg_.watch];
  const t = path.resolve(target);
  if (!allowed.some((a) => t === a || t.startsWith(a + path.sep))) throw new Error("目标目录不在白名单里：" + target);
  if (!/^[\w.-]+$/.test(item.name)) throw new Error("名字不合法");

  const src = path.join(LIB, id);
  const isIndexForm = item.files.includes("index.ts");
  const dest = isIndexForm ? path.join(t, item.name) : t;
  await mkdir(dest, { recursive: true });
  for (const f of item.files) {
    if (isIndexForm) {
      await mkdir(path.dirname(path.join(dest, f)), { recursive: true });
      await copyFile(path.join(src, f), path.join(dest, f));
    } else {
      await copyFile(path.join(src, f), path.join(dest, f));
    }
  }
  if (!item.installDirs.includes(t)) item.installDirs.push(t);
  item.updatedAt = now();
  await saveRegistry();
  return { installed: path.join(dest) };
}

/** 收回：从某安装位拷回库（覆盖），比对差异。 */
async function pullBack(id, dir) {
  const item = findItem(id);
  if (!item) throw new Error("库里没有 " + id);
  const disk = await scanDisk();
  const entry = disk.find((e) => path.resolve(e.dir) === path.resolve(dir) && extNameFromEntry(e) === item.name);
  if (!entry) throw new Error("盘上没找到：" + item.name + " @ " + dir);

  const src = path.join(entry.dir, entry.folder ?? entry.file);
  const dest = path.join(LIB, id);
  await mkdir(dest, { recursive: true });
  const files = entry.folder ? await copyDir(src, dest) : (await copyFile(src, path.join(dest, entry.file)), [entry.file]);
  item.files = files;
  item.updatedAt = now();
  await saveRegistry();
  return { pulled: files };
}

/** 开关：改安装位上的文件名（<x>.ts ↔ <x>.disabled.ts；目录形态 index.ts ↔ index.disabled.ts）。
 *  pi 只认 .ts / index.ts，改名即停用，/reload 生效。 */
async function toggle(dir, name, enable) {
  const reg_ = await loadRegistry();
  const allowed = [GLOBAL_DIR, ...reg_.watch];
  const d = path.resolve(dir);
  if (!allowed.includes(d)) throw new Error("目录不在白名单里：" + dir);
  if (!/^[\w.-]+$/.test(name)) throw new Error("名字不合法");

  const disk = await scanDisk();
  const entry = disk.find((e) => e.dir === d && extNameFromEntry(e) === name);
  if (!entry) throw new Error("盘上没找到：" + name + " @ " + dir);

  if (entry.folder) {
    const from = path.join(d, entry.folder, entry.disabled ? "index.disabled.ts" : "index.ts");
    const to = path.join(d, entry.folder, entry.disabled ? "index.ts" : "index.disabled.ts");
    if (enable === !entry.disabled) return { ok: true, noop: true }; // 已是目标状态
    await rename(from, to);
  } else {
    const base = entry.file.replace(/\.disabled\.ts$/, ".ts").replace(/\.ts$/, "");
    const from = path.join(d, entry.file);
    const to = path.join(d, base + (entry.disabled ? ".ts" : ".disabled.ts"));
    if (enable === !entry.disabled) return { ok: true, noop: true };
    await rename(from, to);
  }
  return { ok: true };
}

/** 加/移白名单目录。 */
async function watchDir(body) {
  const reg_ = await loadRegistry();
  const d = path.resolve(body.dir ?? "");
  if (!d || !path.isAbsolute(d)) throw new Error("需要绝对路径");
  let st;
  try {
    st = await stat(d);
  } catch {
    throw new Error("目录不存在：" + d);
  }
  if (!st.isDirectory()) throw new Error("不是目录：" + d);
  if (body.remove) {
    reg.watch = reg.watch.filter((w) => w !== d);
  } else if (!reg.watch.includes(d)) {
    reg.watch.push(d);
    // 记住安装过没有 —— 没有也记，扫描会显示为空目录
  }
  await saveRegistry();
  return { watch: reg.watch };
}

/** 删库条目：只删台账和 lib/<id>，不碰安装位。 */
async function removeLib(id) {
  const before = reg.items.length;
  reg.items = reg.items.filter((r) => r.id !== id);
  if (reg.items.length === before) throw new Error("库里没有 " + id);
  await rmrf(path.join(LIB, id));
  await saveRegistry();
}

async function rmrf(p) {
  await rm(p, { recursive: true, force: true });
}
/** 读一个源文件（库里的），带 etag。 */
async function readLibFile(id, file) {
  const item = findItem(id);
  if (!item) return null;
  if (!/^[\w./-]+$/.test(file) || file.includes("..")) return null;
  const p = path.join(LIB, id, file);
  const text = await readFile(p, "utf8").catch(() => null);
  return text;
}

// ── 车间：从模板建新档、库内编辑、快照、出厂自查 ──────────────────────

/** 建新档：lib/<id>/<name>.ts，内容来自模板，台账加一条。origin 标明是厂里造的。 */
async function createFromTemplate(body) {
  const name = String(body.name ?? "").trim();
  const tpl = TEMPLATES[body.template] ?? null;
  if (!tpl) throw [400, "未知模板：" + body.template];
  if (!/^[a-z][\w-]*$/.test(name)) throw [400, "名字要用小写字母开头，字母数字连字符：" + name];
  const reg_ = await loadRegistry();
  if (reg_.items.some((r) => r.name === name)) throw [409, "库/台账里已有同名扩展：" + name];

  const id = "x" + Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
  const file = name + ".ts";
  await mkdir(path.join(LIB, id), { recursive: true });
  await writeFile(path.join(LIB, id, file), tpl.code(name));
  const item = {
    id,
    name,
    scope: body.scope === "project" ? "project" : "global",
    origin: "workshop:" + body.template,
    files: [file],
    installDirs: [],
    notes: tpl.note,
    updatedAt: now(),
  };
  reg.items.push(item);
  await saveRegistry();
  return item;
}

/** 覆写库内文件。保存前把旧版快照进 revs/<id>/，最多留 20 版。 */
async function writeLibFile(body) {
  const item = findItem(body.id);
  if (!item) throw [404, "库里没有 " + body.id];
  const file = String(body.file ?? "");
  if (!/^[\w.-]+\.ts$/.test(file)) throw [400, "文件名不合法：" + file];
  if (!item.files.includes(file)) throw [404, "这条档里没有 " + file];
  const text = String(body.text ?? "");
  if (text.length > 512 * 1024) throw [400, "文件太大"];

  const dest = path.join(LIB, body.id, file);
  const old = await readFile(dest, "utf8").catch(() => null);
  if (old != null && old !== text) {
    const revDir = path.join(REVS, body.id);
    await mkdir(revDir, { recursive: true });
    const existing = (await readdir(revDir)).map(Number).filter((n) => !Number.isNaN(n)).sort((a, b) => a - b);
    const next = (existing.at(-1) ?? 0) + 1;
    await writeFile(path.join(revDir, String(next).padStart(3, "0") + ".ts"), old);
    if (existing.length >= 20) await rm(path.join(revDir, String(existing[0]).padStart(3, "0") + ".ts"), { force: true });
  }
  await writeFile(dest, text);
  item.updatedAt = now();
  await saveRegistry();
  return { ok: true, savedAt: item.updatedAt };
}

/** 库内文件清单 + 每个文件的快照数。 */
async function libRevisions(id) {
  const item = findItem(id);
  if (!item) throw [404, "库里没有 " + id];
  const revDir = path.join(REVS, id);
  const names = (await readdir(revDir).catch(() => [])).filter((n) => n.endsWith(".ts")).sort();
  return { id, revs: names.length };
}

/** 列快照（供将来做回看）。 */
async function listRevs(id) {
  const item = findItem(id);
  if (!item) throw [404, "库里没有 " + id];
  const revDir = path.join(REVS, id);
  const names = (await readdir(revDir).catch(() => [])).sort().reverse();
  const out = [];
  for (const n of names.slice(0, 20)) {
    const text = await readFile(path.join(revDir, n), "utf8").catch(() => null);
    if (text != null) out.push({ rev: n, text });
  }
  return { id, revs: out };
}

/** 出厂自查：静态规则，不跑代码。只查「装到 pi 里会不会当场死」的三条底线。 */
function lintExtension(text) {
  const checks = [];
  const hasDefault = /export\s+default\s+(function|\(|const|[\w$])/m.test(text);
  checks.push({
    id: "default-export",
    ok: hasDefault,
    label: hasDefault ? "有 export default" : "没有 export default —— pi 加载不到入口，装了也不会跑",
  });
  const hasApi = /\bpi\s*\.\s*(on|registerTool|registerCommand)\s*\(/m.test(text);
  checks.push({
    id: "api-use",
    ok: hasApi,
    label: hasApi ? "调用了 pi.on / registerTool / registerCommand" : "没看到 pi.on(...) 或 registerTool/registerCommand —— 可能什么都没做",
  });
  const usesType = /\bType\s*\./m.test(text);
  const importsType = /import\s*\{[^}]*\bType\b[^}]*\}\s*from/m.test(text);
  checks.push({
    id: "typebox-import",
    ok: !usesType || importsType,
    label: usesType && !importsType ? "用到了 Type. 但没 import —— 参数定义会当场 ReferenceError" : "Type 导入正常",
  });
  const balanced = (text.split("{").length - 1) === (text.split("}").length - 1);
  checks.push({
    id: "braces",
    ok: balanced,
    label: balanced ? "花括号配平" : "花括号不配平 —— 语法多半断了",
  });
  return checks;
}

// ── HTTP ────────────────────────────────────────────────────────────────────

const server = createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", `http://${req.headers.host ?? "localhost"}`);
  const p = url.pathname;

  try {
    // 静态
    if (req.method === "GET" && (p === "/" || !p.startsWith("/api/"))) {
      const file = p === "/" ? "index.html" : p.slice(1);
      const fp = path.join(PUBLIC, file);
      if (!fp.startsWith(PUBLIC)) throw [403, "forbidden"];
      const text = await readFile(fp).catch(() => null);
      if (!text) throw [404, "not found"];
      res.writeHead(200, { "content-type": TYPES[path.extname(fp)] ?? "application/octet-stream" });
      res.end(text);
      return;
    }

    const body = await readBody(req);

    if (p === "/api/inventory" && req.method === "GET") {
      return json(res, await inventory());
    }
    if (p === "/api/adopt" && req.method === "POST") {
      return json(res, await adopt(body));
    }
    if (p === "/api/install" && req.method === "POST") {
      return json(res, await installTo(body.id, body.dir));
    }
    if (p === "/api/pull" && req.method === "POST") {
      return json(res, await pullBack(body.id, body.dir));
    }
    if (p === "/api/toggle" && req.method === "POST") {
      return json(res, await toggle(body.dir, body.name, body.enable));
    }
    if (p === "/api/watch" && req.method === "POST") {
      return json(res, await watchDir(body));
    }
    if (p === "/api/remove" && req.method === "POST") {
      return json(res, await removeLib(body.id));
    }
    if (p.startsWith("/api/file/") && req.method === "GET") {
      const [, , , id, ...rest] = p.split("/");
      const text = await readLibFile(id, rest.join("/"));
      if (text == null) throw [404, "not found"];
      return json(res, { text });
    }
    if (p === "/api/save-note" && req.method === "POST") {
      const item = findItem(body.id);
      if (!item) throw [404, "没有这条"];
      item.notes = String(body.notes ?? "");
      item.updatedAt = now();
      await saveRegistry();
      return json(res, { ok: true });
    }
    if (p === "/api/create" && req.method === "POST") {
      return json(res, await createFromTemplate(body));
    }
    if (p === "/api/write" && req.method === "POST") {
      return json(res, await writeLibFile(body));
    }
    if (p === "/api/revs" && req.method === "GET") {
      return json(res, await listRevs(url.searchParams.get("id")));
    }
    if (p === "/api/lint" && req.method === "POST") {
      return json(res, { checks: lintExtension(String(body.text ?? "")) });
    }

    throw [404, "not found"];
  } catch (e) {
    if (Array.isArray(e)) {
      res.writeHead(e[0], { "content-type": "application/json; charset=utf-8" });
      res.end(JSON.stringify({ error: e[1] }));
    } else {
      res.writeHead(500, { "content-type": "application/json; charset=utf-8" });
      res.end(JSON.stringify({ error: oneLine(e) }));
    }
  }
});

function json(res, value, code = 200) {
  res.writeHead(code, { "content-type": "application/json; charset=utf-8" });
  res.end(JSON.stringify(value));
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    let data = "";
    req.on("data", (c) => {
      data += c;
      if (data.length > 1e6) {
        reject(new Error("body too large"));
        req.destroy();
      }
    });
    req.on("end", () => {
      if (!data) return resolve({});
      try {
        resolve(JSON.parse(data));
      } catch {
        reject(new Error("bad json"));
      }
    });
    req.on("error", reject);
  });
}

server.listen(PORT, "127.0.0.1", () => {
  console.log(`[vbe4pt] extend manager on :${PORT}, lib at ${LIB}`);
});

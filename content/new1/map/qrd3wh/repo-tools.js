// 仓库工具：一个 git 仓库 = 一堆目录（每个是一条扩展 / 一条技能）+ 一份词表 + 一份生成物。
//
// **每个函数第一个参数都是 `root`（仓库根），没有默认值。**
// 默认值会让「忘了传」静默写到另一个仓库 —— 那是最难查的那种错：写成功了、测试绿了、
// 东西落在别处。所以宁可每次都写一遍 `EXTEND` / `SKILLS_ROOT`。
//
// 这个文件只 import `node:` 内置模块，导出具名函数，不启动任何服务 —— 于是测试能
// 直接 `import` 它、造两个临时 git 仓、断言「写了 A 不会碰 B」。
// `server.js` 有顶层 await 会起服务，测试永远不能 import 它；所以仓库这套东西在这儿。
//
// 三个必须记住的坑（每一条都踩过）：
//   1. 写坏了不能留半截 —— 先写临时文件、跑 index.mjs 校验，过了才 rename 回来。
//   2. git 操作按仓串行（`serializeRepoWrite`）：轮询的 `git pull` 和写文件会抢 index.lock。
//      两个仓各排各的队 —— 写技能不该等扩展那边的 push。
//   3. 删 tracked 文件必然在工作区留 ` D`；`syncRepo` 默认把它们过滤掉，只有显式
//      `includeDeleted`（归档 / 恢复 / 彻底删除）才允许提交删除。

import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { cp, mkdir, readFile, readdir, realpath, rename, rm, stat, unlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { promisify } from "node:util";
import path from "node:path";

export const run = promisify(execFile);

/** 仓库内的归档区：点开头，扫盘器天然跳过。 */
export const ARCHIVE_DIR = ".archive";
/** 内置类别：像「全部」一样永远在 —— 删不掉、改不了名。但可以往里挂东西、建子类别。 */
export const PROTECTED_CATEGORIES = ["Basics"];
export const MAX_FILE_BYTES = 512 * 1024;
export const EXTENSION_NAME_RE = /^[a-z0-9][a-z0-9._-]*$/i;

/** 错误带上 HTTP 状态；路由只认 `error.status`。 */
export const withStatus = (error, status) => Object.assign(error, { status });

/** execFile 的报错里真正有用的那几行。 */
export const errorDetail = (error) => (error?.stderr || error?.stdout || String(error?.message ?? error)).trim();

/** 搬目录：跨设备时 rename 会 EXDEV，退回复制再删。 */
export const moveDir = async (src, dst) => {
  try {
    await rename(src, dst);
  } catch (error) {
    if (error?.code !== "EXDEV") throw error;
    await cp(src, dst, { recursive: true });
    await rm(src, { recursive: true, force: true });
  }
};

export const writeFileAtomic = async (file, text) => {
  const tmp = `${file}.tmp-${process.pid}-${Date.now()}`;
  try {
    await writeFile(tmp, text, "utf8");
    await rename(tmp, file);
  } catch (error) {
    // 半截临时文件别留在仓库里 —— 下一轮 `git add -A` 会把它当真账提交。
    await unlink(tmp).catch(() => {});
    throw error;
  }
};

// ── 写锁：按仓库排队 ────────────────────────────────────────────────────────

/** Map<root, Promise>：同仓串行（一个 git 队列），两仓互不阻塞。 */
const chains = new Map();
/** Map<root, boolean>：这一仓此刻有没有人在写 —— 轮询的 pull 看见就让一轮。 */
const busy = new Map();

export const repoBusy = (root) => busy.get(String(root)) === true;

export const serializeRepoWrite = (root, task) => {
  const key = String(root);
  const runTask = async () => {
    busy.set(key, true);
    try {
      return await task();
    } finally {
      busy.set(key, false);
    }
  };
  const prev = chains.get(key) ?? Promise.resolve();
  const next = prev.then(runTask, runTask);
  chains.set(key, next.then(() => {}, () => {}));
  return next;
};

// ── 类别词表 ────────────────────────────────────────────────────────────────
//
// 目录的 category 是「哪些类别挂在这个东西上」，categories.json 是「这个仓库认得
// 哪些类别」—— 没有它，一个刚建、还没挂到任何东西上的类别就会当场消失（建了看不见）。
// 两边取并集，词表里的零计数类别也摆着。
//
// 类别是路径：`工具/搜索` = `工具` 底下的 `搜索`。父子关系不存，靠 `/` 推。
// 祖先会自动进词表（打上 `工具/搜索`，`工具` 就得存在），但**不会**写进某个目录的
// category —— 一个东西属于 `工具` 是因为它的标签前缀是 `工具/`，不是因为它多存了一条。

const CATEGORY_SEP = "/";
const MAX_CATEGORY_SEGMENT = 40;

const splitCategory = (value) =>
  String(value ?? "")
    .split(CATEGORY_SEP)
    .map((part) => part.trim())
    .filter(Boolean);

/** 规范化一个类别路径；空串表示空。控制字符 / 超长段直接 400。 */
export const normalizeCategory = (value) => {
  const parts = splitCategory(value);
  if (!parts.length) return "";
  for (const part of parts) {
    if (/[\u0000-\u001f\u007f]/.test(part)) throw withStatus(new Error("类别名里不能有控制字符"), 400);
    if (part.length > MAX_CATEGORY_SEGMENT) {
      throw withStatus(new Error(`类别名太长（一段最多 ${MAX_CATEGORY_SEGMENT} 字）：${part}`), 400);
    }
  }
  return parts.join(CATEGORY_SEP);
};

export const normalizeCategories = (list) =>
  Array.isArray(list) ? list.map((c) => normalizeCategory(c)).filter(Boolean) : [];

/** `a/b/c` → [`a`, `a/b`]。 */
export const ancestorsOf = (value) => {
  const parts = value.split(CATEGORY_SEP);
  return parts.slice(0, -1).map((_, index) => parts.slice(0, index + 1).join(CATEGORY_SEP));
};

export const isUnder = (value, root) => value === root || value.startsWith(root + CATEGORY_SEP);

// 父在子前、同级按段比 —— 直接 localeCompare 比全串会被 `/` 的位置坑到。
const compareCategories = (a, b) => {
  const left = a.split(CATEGORY_SEP);
  const right = b.split(CATEGORY_SEP);
  const common = Math.min(left.length, right.length);
  for (let i = 0; i < common; i++) {
    const cmp = left[i].localeCompare(right[i], "zh-CN");
    if (cmp) return cmp;
  }
  return left.length - right.length;
};

export const sortCategories = (list) => [...new Set(list)].sort(compareCategories);

const categoriesFile = (root) => path.join(root, "categories.json");

/**
 * 类别词表。`protect` 是「这个仓里不许删、不许改名的类别」——**扩展仓有，技能仓没有**
 * （spec §3.6：技能仓不许造受保护的类别）。所以它是参数，不是常量：
 * 写死的话，技能仓删掉 Basics 会 403，而且词表里永远回魂一个 Basics。
 */
export const readCategories = async (root, { protect = PROTECTED_CATEGORIES } = {}) => {
  let list = [];
  try {
    const raw = JSON.parse(await readFile(categoriesFile(root), "utf8"));
    list = normalizeCategories(Array.isArray(raw) ? raw : raw?.categories);
  } catch {
    // 文件没写 / 写坏了：内置类别照样在。
  }
  return [...new Set([...list, ...protect])];
};

export const writeCategories = async (root, list, { protect = PROTECTED_CATEGORIES } = {}) => {
  const expanded = [];
  // 受保护的类别永远写进去 —— 词表被人清空也丢不了。（技能仓传 `protect: []`。）
  for (const item of normalizeCategories([...list, ...protect])) expanded.push(...ancestorsOf(item), item);
  const unique = sortCategories(expanded);
  // `v: 1` 是形状版本（spec §2.3）：扩展仓和技能仓的词表长得一模一样，读的那边才敢
  // 按同一个解析器读。不写它的话，一个格式变更就只能在两边猜。
  await writeFileAtomic(categoriesFile(root), JSON.stringify({ v: 1, categories: unique }, null, 2) + "\n");
  return unique;
};

// ── 目录 / 清单 ─────────────────────────────────────────────────────────────

/** 仓库根下每个目录（点开头的不算，node_modules / scripts 也不算）。 */
export const listDirs = async (root) =>
  (await readdir(root, { withFileTypes: true }))
    .filter((e) => e.isDirectory() && !e.name.startsWith(".") && e.name !== "node_modules" && e.name !== "scripts")
    .map((e) => e.name);

/**
 * 一个目录的清单文件。扩展 / 技能 / 模板都叫 `nous.json`；预设叫 `preset.json`
 * （它是纯配置，没有正文，所以清单就是那个文件本身）。
 */
export const readManifest = async (root, dir, { manifest = "nous.json" } = {}) => {
  const file = path.join(root, dir, manifest);
  try {
    const text = await readFile(file, "utf8");
    return { file, text, manifest: JSON.parse(text) };
  } catch {
    return null; // 没 manifest 或 JSON 坏了：这里不顺手改它
  }
};

// ── 生成器 ──────────────────────────────────────────────────────────────────

/** 两个仓都是 `node index.mjs`，路径不同、行为一样 —— 别写死。 */
const runIndex = async (root) => run(process.execPath, ["index.mjs"], { cwd: root, timeout: 30_000 });

/**
 * 重扫 index.json；失败不提交（提交了就是一份红账）。
 *
 * `required: false` = 生成器不在也算过（技能仓库刚建、index.mjs 还没落进去的时候，
 * 没有生成物不代表数据是错的）。**生成器在、跑挂了，照样抛** —— 那不是「没有」，
 * 那是「校验没过」。
 */
export const regenIndex = async (root, { required = true } = {}) => {
  if (!required && !(await stat(path.join(root, "index.mjs")).catch(() => null))) return { skipped: true };
  try {
    await runIndex(root);
  } catch (error) {
    throw withStatus(new Error(`index.mjs 校验失败，没有提交：${errorDetail(error)}`), 400);
  }
  return { skipped: false };
};

/** 落盘之后重扫；生成器不认就整体回滚（快照 + 词表）。 */
const regenOrRollback = async (root, snapshots, beforeVocab, index, protect) => {
  try {
    await index();
  } catch (error) {
    for (const [file, text] of snapshots) await writeFileAtomic(file, text);
    await writeCategories(root, beforeVocab, { protect });
    throw withStatus(new Error(`index.mjs 校验失败，已回滚：${errorDetail(error)}`), 400);
  }
};

// ── 目录的类别（写 nous.json + 重扫词表）───────────────────────────────────

/** 改一个目录的 category（字符串数组：去空、去重、排序）。返回新清单。 */
export const setNodeCategories = async (
  root,
  name,
  categories,
  { label = "扩展", dir = null, index = null, protect = PROTECTED_CATEGORIES, manifest = "nous.json" } = {},
) => {
  if (!EXTENSION_NAME_RE.test(name)) throw withStatus(new Error(`${label}名不合法：${name}`), 400);
  // `dir` = 这个条目真正住哪儿（分组里的技能是 `<root>/<分组>/<名字>`），不给就是顶层。
  // `manifest` = 这份清单叫什么 —— 扩展 / 技能 / 模板都是 `nous.json`，预设是 `preset.json`
  // （它是纯配置，没有正文，所以清单就是那个文件本身）。
  const file = path.join(dir ?? path.join(root, name), manifest);
  let before;
  try {
    before = await readFile(file, "utf8");
  } catch {
    throw withStatus(new Error(`仓库里没有这个${label}：${name}`), 404);
  }
  const doc = JSON.parse(before);
  const next = sortCategories(normalizeCategories(categories));
  const updated = { ...doc, category: next };
  // 词表先落地，再写 nous.json —— 顺序不能反：生成器会拿 `categories.json` 校验每条
  // nous.json 的类别（`工具/搜索` 得先在词表里，连同祖先 `工具`）。反过来的话，给一条
  // 技能打一个新类别永远是「校验失败，已回滚」，那个输入框等于假的。
  const beforeVocab = await readCategories(root, { protect });
  await writeCategories(root, [...beforeVocab, ...next], { protect });
  await writeFileAtomic(file, JSON.stringify(updated, null, 2) + "\n");
  try {
    // index.json 是生成物：跑生成器，顺带校验（目录名==名字、字段齐全）。
    await (index ? index() : runIndex(root));
  } catch (error) {
    await writeFileAtomic(file, before);
    await writeCategories(root, beforeVocab, { protect });
    throw withStatus(new Error(`index.mjs 校验失败，已回滚：${errorDetail(error)}`), 400);
  }
  return updated;
};

/** 类别词表操作：add 建空类别；rename 改名（有子类别就整枝改）；remove 删整枝。 */
export const mutateCategories = async (
  root,
  { op, name, to },
  { index = null, protect = PROTECTED_CATEGORIES } = {},
) => {
  const regenerate = index ? index : () => runIndex(root);
  const current = await readCategories(root, { protect });
  const target = normalizeCategory(name);
  if (!target) throw withStatus(new Error("类别名不能为空"), 400);

  if (op === "add") {
    return { categories: await writeCategories(root, [...current, target], { protect }), touched: 0 };
  }

  if (op !== "remove" && op !== "rename") {
    throw withStatus(new Error(`未知操作：${op}（只认 add / rename / remove）`), 400);
  }

  // 受保护的类别（或它的祖先）不许删、不许改名 —— 改名等于绕开删除锁。
  // 注意是 `protect`，不是常量：技能仓一个都不护（spec §3.6）。
  if (protect.some((p) => isUnder(p, target))) {
    throw withStatus(new Error(`「${target}」是内置类别，不能${op === "rename" ? "改名" : "删除"}`), 403);
  }

  const dirs = await listDirs(root);

  if (op === "rename") {
    const next = normalizeCategory(to);
    if (!next) throw withStatus(new Error("新类别名不能为空"), 400);
    if (next === target) return { categories: current, touched: 0 };
    if (next.startsWith(target + CATEGORY_SEP)) {
      throw withStatus(new Error(`不能把「${target}」改到它自己的子类别里`), 400);
    }

    // 改名 = 整枝改前缀（所以跨父级改名同时就是“移动”）。先扫一遍：谁挂着这枝、
    // 新名字有没有撞车。撞车不合并 —— 合了就没法反悔。
    const plans = [];
    const known = new Set(current);
    for (const dir of dirs) {
      const found = await readManifest(root, dir);
      if (!found) continue;
      const cats = normalizeCategories(found.manifest.category);
      for (const c of cats) known.add(c);
      if (cats.some((c) => isUnder(c, target))) plans.push({ ...found, cats });
    }
    if ([...known].some((c) => isUnder(c, next))) {
      throw withStatus(new Error(`类别「${next}」已经存在`), 409);
    }
    if (!current.some((c) => isUnder(c, target)) && plans.length === 0) {
      throw withStatus(new Error(`没有这个类别：${target}`), 404);
    }

    const renamePath = (c) => (isUnder(c, target) ? next + c.slice(target.length) : c);
    const snapshots = new Map(plans.map((plan) => [plan.file, plan.text]));
    for (const plan of plans) {
      plan.manifest.category = sortCategories(plan.cats.map(renamePath));
      await writeFileAtomic(plan.file, JSON.stringify(plan.manifest, null, 2) + "\n");
    }
    await writeCategories(root, [...current.map(renamePath), next], { protect });
    await regenOrRollback(root, snapshots, current, regenerate, protect);
    return { categories: await readCategories(root, { protect }), touched: plans.length };
  }

  // remove：删的是整棵子树（`工具` 连 `工具/搜索` 一起），词表和所有 nous.json 同时摘。
  const snapshots = new Map();
  let touched = 0;
  for (const dir of dirs) {
    const found = await readManifest(root, dir);
    if (!found) continue;
    const cats = normalizeCategories(found.manifest.category);
    const kept = cats.filter((c) => !isUnder(c, target));
    if (kept.length === cats.length) continue;
    snapshots.set(found.file, found.text);
    found.manifest.category = kept;
    await writeFileAtomic(found.file, JSON.stringify(found.manifest, null, 2) + "\n");
    touched++;
  }
  await writeCategories(root, current.filter((c) => !isUnder(c, target)), { protect });
  await regenOrRollback(root, snapshots, current, regenerate, protect);
  return { categories: await readCategories(root, { protect }), touched };
};

// ── 文件：读 / 写（只落盘）/ 语法闸 ────────────────────────────────────────

const FILE_KINDS = new Map([
  [".js", "javascript"],
  [".mjs", "javascript"],
  [".json", "json"],
  [".md", "markdown"],
]);

export const etagOf = (text) => createHash("sha1").update(text, "utf8").digest("hex").slice(0, 16);
const kindOfPath = (file) => FILE_KINDS.get(path.extname(file).toLowerCase()) ?? null;

export const repoDir = (root, name, label = "扩展") => {
  const clean = String(name ?? "");
  if (!EXTENSION_NAME_RE.test(clean)) throw withStatus(new Error(`${label}名不合法：${clean}`), 400);
  return { name: clean, dir: path.join(root, clean) };
};

/** 文件所在的 git 仓库根：主树是 root，变体是它自己那棵 checkout。 */
export const gitRootOf = async (dir, fallback = null) => {
  try {
    return (await run("git", ["-C", dir, "rev-parse", "--show-toplevel"], { timeout: 15_000 })).stdout.trim();
  } catch {
    return fallback ?? dir;
  }
};

/**
 * 相对路径 → 绝对路径，且 realpath 之后还得在这个目录里（符号链接是逃逸口）。
 * `dir` = 覆盖目录（变体 / 分组里的技能用它）；不给就是 `<root>/<name>`。
 *
 * `create: true` = 目标可以还没存在（新建文件 / 文件夹）：父目录一路造出来，
 * 造之前先验「最深的已存在那一层」在不在目录里 —— 不然顺着一个指向外面的
 * 符号链接，mkdir 会把目录造到别处去。
 */
export const resolveInside = async (root, name, rel, dir = null, label = "扩展", { create = false } = {}) => {
  const { name: clean, dir: baseDir } = repoDir(root, name, label);
  const base = dir ?? baseDir;
  const raw = String(rel ?? "").replace(/\\/g, "/").replace(/^\/+/, "").replace(/\/+$/, "");
  if (!raw || raw.split("/").some((part) => !part || part === "." || part === "..")) {
    throw withStatus(new Error(`路径不合法：${rel}`), 400);
  }
  const target = path.resolve(base, raw);
  if (!target.startsWith(base + path.sep)) throw withStatus(new Error(`路径跑出了${label}目录`), 403);
  const realBase = await realpath(base).catch(() => base);
  let real;
  try {
    real = await realpath(target);
  } catch {
    if (!create) throw withStatus(new Error(`没有这个文件：${clean}/${raw}`), 404);
    const parent = path.dirname(target);
    // 先找出「最深的已经存在的那一层」验一遍，再建：顺序反过来的话，中途那个 mkdir
    // 已经能顺着符号链接把东西造到目录外面了。
    let probe = parent;
    while (probe !== base && !(await stat(probe).catch(() => null))) probe = path.dirname(probe);
    const realProbe = await realpath(probe).catch(() => null);
    if (!realProbe || (realProbe !== realBase && !realProbe.startsWith(realBase + path.sep))) {
      throw withStatus(new Error(`路径跑出了${label}目录`), 403);
    }
    await mkdir(parent, { recursive: true });
    const realParent = await realpath(parent);
    if (realParent !== realBase && !realParent.startsWith(realBase + path.sep)) {
      throw withStatus(new Error(`路径跑出了${label}目录`), 403);
    }
    real = path.join(realParent, path.basename(target));
  }
  if (real !== realBase && !real.startsWith(realBase + path.sep)) {
    throw withStatus(new Error(`路径跑出了${label}目录`), 403);
  }
  return { name: clean, dir: base, file: real, rel: raw };
};

/**
 * 这个目录里有哪些文件。
 *
 * `kinds` 默认只认这一页能编辑的四种（扩展源码）；传 `null` = 全都要（技能目录里
 * 会有 .txt / .py 这类参考文档和脚本，它们不是「源码」，但一样该列出来）。
 */
export const listRepoFiles = async (root, name, { dir = null, kinds = FILE_KINDS, maxFiles = Infinity, label = "扩展" } = {}) => {
  const { name: clean, dir: baseDir } = repoDir(root, name, label);
  const base0 = dir ?? baseDir;
  const exists = await stat(base0).catch(() => null);
  if (!exists?.isDirectory()) throw withStatus(new Error(`仓库里没有这个${label}：${clean}`), 404);

  const files = [];
  // 递归到底：技能目录里可能有 `/references/a/b/c.md` 这种参考文档，截到第 4 层
  // 就等于「文件在盘上、面板里看不见」。真正刹住的是 `maxFiles`（spec §6.1：200 个）。
  const walk = async (base, relBase) => {
    if (files.length >= maxFiles) return;
    const entries = await readdir(base, { withFileTypes: true }).catch(() => []);
    entries.sort((a, b) => a.name.localeCompare(b.name));
    for (const entry of entries) {
      if (files.length >= maxFiles) return;
      if (entry.name.startsWith(".") || entry.name === "node_modules") continue;
      const rel = relBase ? `${relBase}/${entry.name}` : entry.name;
      if (entry.isDirectory()) {
        await walk(path.join(base, entry.name), rel);
        continue;
      }
      const kind = kindOfPath(entry.name);
      if (kinds && !kind) continue;
      const info = await stat(path.join(base, entry.name)).catch(() => null);
      if (!info || info.size > MAX_FILE_BYTES) continue;
      files.push({ path: rel, kind: kind ?? "text", size: info.size, mtime: Math.round(info.mtimeMs) });
    }
  };
  await walk(base0, "");
  return files;
};

/**
 * 读一个文件 + 它**自己那棵树** HEAD 里的那一版（`base`，面板的 diff 靠它）。
 *
 * `allowAny` = 不按扩展名筛（技能目录里什么都可能有）；二进制给 404 —— 面板不是
 * 二进制编辑器，读一半乱码比明说「看不了」更糟。
 */
export const readRepoFile = async (root, name, rel, { dir = null, allowAny = false, label = "扩展" } = {}) => {
  const { name: clean, file, rel: pathname, dir: baseDir } = await resolveInside(root, name, rel, dir, label);
  const kind = kindOfPath(file);
  if (!allowAny && !kind) throw withStatus(new Error("这一页只编辑 .js / .mjs / .json / .md"), 415);
  const info = await stat(file);
  if (info.size > MAX_FILE_BYTES) throw withStatus(new Error(`文件太大：${info.size} 字节`), 413);
  const bytes = await readFile(file);
  if (allowAny && bytes.includes(0)) {
    throw withStatus(new Error(`${clean}/${pathname} 是二进制，这一页不编辑它`), 404);
  }
  const text = bytes.toString("utf8");
  // 基线取**文件自己那棵树**的 HEAD：变体在自己的分支上，主树的 HEAD 跟它无关。
  // 未见过的文件（未发布）给 null。
  let base = null;
  try {
    const gitRoot = await gitRootOf(baseDir, root);
    const relToRoot = path.relative(gitRoot, file).split(path.sep).join("/");
    base = (await run("git", ["-C", gitRoot, "show", `HEAD:${relToRoot}`], { timeout: 15_000 })).stdout;
  } catch {
    base = null;
  }
  return {
    name: clean,
    path: pathname,
    kind,
    text,
    base,
    size: info.size,
    mtime: Math.round(info.mtimeMs),
    etag: etagOf(text),
  };
};

/** 语法闸：拷成 .mjs 让 node 按 ESM 解析。只解析，不执行 —— 进程里没有 api 可递。 */
export const checkSyntax = async (label, text) => {
  const dir = path.join(tmpdir(), "nous-extend-check");
  await mkdir(dir, { recursive: true });
  const file = path.join(dir, `${process.pid}-${Date.now()}-${Math.random().toString(36).slice(2)}.mjs`);
  try {
    await writeFile(file, text, "utf8");
    await run(process.execPath, ["--check", file], { timeout: 15_000 });
  } catch (error) {
    const lines = errorDetail(error)
      .replaceAll(file, label)
      .split("\n");
    // 切到错误类型那一行为止：再往下是 checkSyntax 自己的调用栈，对改代码的人零信息量。
    const stop = lines.findIndex((line) => /^(SyntaxError|ReferenceError|TypeError)\b/.test(line.trim()));
    const detail = (stop === -1 ? lines : lines.slice(0, stop + 1)).slice(0, 8).join("\n");
    throw withStatus(new Error(`${label} 语法错误：\n${detail}`), 400);
  } finally {
    await unlink(file).catch(() => {});
  }
};

/**
 * 写一个文件：立刻落盘，**不提交**。nous.json 额外过一遍 index.mjs（不过就回滚）。
 *
 * `checkCode: false` 给技能仓库 —— 那边的闸是 frontmatter（内容闸），不是语法闸，
 * 技能自带的参考文档 / 脚本不该因为我们不认识就写不进去。
 */
export const writeRepoFile = async (
  root,
  { name, rel, text, base, dir = null, allowAny = false, checkCode = true, strictIndex = true, label = "扩展" },
) => {
  // `create: true`：写一个还不存在的文件 = 新建。中途的目录（`docs/guide.md` 里的
  // `docs/`）一起造出来，这就是「新建文件夹」的全部实现 —— 空文件夹在 git 里活不下去，
  // 真正立得住的只有「文件夹里有个文件」。
  const { name: clean, file, rel: pathname, dir: baseDir } = await resolveInside(root, name, rel, dir, label, {
    create: true,
  });
  const kind = kindOfPath(file);
  if (!allowAny && !kind) throw withStatus(new Error("这一页只编辑 .js / .mjs / .json / .md"), 415);
  if (typeof text !== "string") throw withStatus(new Error("text 得是字符串（哪怕是空串）"), 400);
  if (Buffer.byteLength(text, "utf8") > MAX_FILE_BYTES) throw withStatus(new Error("文件太大"), 413);

  const existed = Boolean(await stat(file).catch(() => null));
  const before = existed ? await readFile(file, "utf8").catch(() => "") : "";
  // base 是「我改的是哪一版」：别人先改了，这里就 409，别把对方的字盖掉。
  if (base && etagOf(before) !== String(base)) {
    throw withStatus(new Error("这个文件在别处被改过了 —— 先刷新，再改"), 409);
  }

  if (kind === "json") {
    try {
      JSON.parse(text);
    } catch (error) {
      throw withStatus(new Error(`${clean}/${pathname} 不是合法 JSON：${error.message}`), 400);
    }
  }
  if (checkCode && kind === "javascript") await checkSyntax(`${clean}/${pathname}`, text);

  await writeFileAtomic(file, text);

  // 这一层（以及上面每一层）有了真文件，之前铺的 `.gitkeep` 就多余了 —— 顺手收掉。
  // 只收**同一条链上的**：`references/deep/guide.md` 建好之后，`references/.gitkeep`
  // 和 `references/deep/.gitkeep` 都成了占位垃圾（git 靠那个 md 就看得见这两层）。
  if (path.basename(file) !== ".gitkeep") {
    for (let dir = path.dirname(file); ; dir = path.dirname(dir)) {
      const rel = path.relative(baseDir, dir);
      if (rel.startsWith("..") || path.isAbsolute(rel)) break;
      await rm(path.join(dir, ".gitkeep"), { force: true }).catch(() => {});
      if (!rel) break;
    }
  }

  // nous.json 不是「普通 JSON」：目录名 == 名字、字段齐全、入口真在 —— 只有 index.mjs 认。
  // 变体跳过（它那棵树在仓外、有自己的生成器）：拿变体的清单去重扫主树的生成物是错的。
  // 判据是「目录在不在这个仓里」，**不是**「dir 参数空不空」—— 分组里的技能也带着 dir
  // （`<root>/deprecated/demo`），它一样得重扫。
  const insideRoot = path.resolve(baseDir).startsWith(path.resolve(root) + path.sep);
  if (pathname === "nous.json" && insideRoot) {
    const present = await stat(path.join(root, "index.mjs")).catch(() => null);
    if (strictIndex || present) {
      try {
        await runIndex(root);
      } catch (error) {
        // 新建的文件回滚 = 删掉，不是写一份空的进去（空 nous.json 会一直红在那里）。
        if (existed) await writeFileAtomic(file, before);
        else await rm(file, { force: true }).catch(() => {});
        throw withStatus(new Error(`nous.json 没过 index.mjs，已回滚：${errorDetail(error)}`), 400);
      }
    }
  }

  const info = await stat(file);
  const gitRoot = await gitRootOf(baseDir, root);
  return {
    name: clean,
    path: pathname,
    kind,
    size: info.size,
    mtime: Math.round(info.mtimeMs),
    etag: etagOf(text),
    dirty: (await run("git", ["status", "--porcelain"], { cwd: gitRoot })).stdout.trim().length > 0,
  };
};

// ── 工作区现状 ──────────────────────────────────────────────────────────────

/**
 * 建一个空目录。
 *
 * git 不跟踪空目录 —— 一个真正空的文件夹发布那一刻就凭空消失了。所以它的存在感由里面
 * 那份 `.gitkeep` 撑着（约定俗成，不是我们发明的）。第一次往里面写真文件时，
 * `writeRepoFile` 会把它收掉。
 */
export const mkdirRepoDir = async (root, { name, rel, dir = null, label = "扩展" }) => {
  const { name: clean, file, rel: pathname } = await resolveInside(root, name, rel, dir, label, { create: true });
  const info = await stat(file).catch(() => null);
  if (info && !info.isDirectory()) {
    throw withStatus(new Error(`${clean}/${pathname} 已经是一个文件了 —— 文件夹不能用这个名字`), 409);
  }
  await mkdir(file, { recursive: true });
  // 已经有的文件夹不重铺占位：那会把用户手删的 `.gitkeep` 又塞回来。
  const keep = path.join(file, ".gitkeep");
  if (!(await stat(keep).catch(() => null))) await writeFileAtomic(keep, "");
  return { name: clean, path: pathname, kind: "dir" };
};

/**
 * 删一个文件或一整个目录。**不提交** —— 跟写一样，落盘等发布（git 历史就是回收站）。
 *
 * `recursive: false`（默认）时目录里还有东西就 409：一下点掉三个月稿子不是任何人想要的。
 * 真删整棵要显式说 recursive。删完之后父目录空到只剩 `.gitkeep` 就一起收掉 ——
 * 空目录 git 不跟踪，留在盘上就是幽灵。
 */
export const deleteRepoPath = async (root, { name, rel, dir = null, label = "扩展", recursive = false }) => {
  const { name: clean, file, rel: pathname, dir: base } = await resolveInside(root, name, rel, dir, label);
  const info = await stat(file);
  if (info.isDirectory()) {
    const left = (await readdir(file)).filter((entry) => entry !== ".gitkeep");
    if (left.length && !recursive) {
      throw withStatus(new Error(`「${pathname}/」里还有 ${left.length} 个东西 —— 先清空，或者确认整棵删掉`), 409);
    }
    await rm(file, { recursive: true, force: true });
  } else {
    await rm(file, { force: true });
  }
  // 父目录一层层往上收：删完 `docs/guide.md`，`docs/` 空了就没必要留着。
  let parent = path.dirname(file);
  while (parent !== base) {
    const left = (await readdir(parent).catch(() => null))?.filter((entry) => entry !== ".gitkeep");
    if (!left || left.length) break;
    await rm(parent, { recursive: true, force: true });
    parent = path.dirname(parent);
  }
  return { name: clean, path: pathname, kind: info.isDirectory() ? "dir" : "file" };
};
/** 工作区里改了哪些文件（porcelain v1：前两位是状态，改名只认 `->` 右边那个）。 */
export const gitChanges = async (root) => {
  // 别对整份输出 .trim()：第一行是 ` M path`，那个前导空格是状态列的一部分 ——
  // trim 掉再 slice(3) 会把路径首字母一起切掉（`categories.json` → `ategories.json`）。
  const porcelain = (await run("git", ["status", "--porcelain"], { cwd: root, timeout: 15_000 })).stdout.replace(/\n+$/, "");
  if (!porcelain.trim()) return [];
  return porcelain
    .split("\n")
    .map((line) => {
      let rel = line.slice(3).trim().replace(/^"|"$/g, "");
      let from = null;
      if (rel.includes(" -> ")) {
        const sides = rel.split(" -> ");
        from = sides[sides.length - 2];
        rel = sides[sides.length - 1];
      }
      return { status: line.slice(0, 2), path: rel, ...(from ? { from } : {}) };
    })
    .filter((item) => item.path);
};

/** HEAD 里有哪些顶层目录（「已发布」的判据，和卡片上的 tracked 同一个口径）。 */
export const trackedDirs = async (root) => {
  try {
    const out = await run("git", ["-C", root, "ls-tree", "--name-only", "-d", "HEAD"], { timeout: 20_000 });
    return new Set(out.stdout.split("\n").map((line) => line.trim()).filter(Boolean));
  } catch {
    return new Set();
  }
};

/** 这个目录在 HEAD 里吗？false = 未发布（只在本地，别人 pull 不到）。 */
export const isTrackedDir = async (root, name) => {
  try {
    await run("git", ["-C", root, "ls-files", "--error-unmatch", name], { timeout: 15_000 });
    return true;
  } catch {
    return false; // 未发布的新目录：还没有任何文件进 index
  }
};

/**
 * HEAD 里所有目录（任意深度）。技能可以住在分组里（`deprecated/demo`），
 * 只看顶层的 `trackedDirs` 会把它一律当成「未发布」—— 那正是面板上最难解释的幻影。
 */
export const trackedPaths = async (root) => {
  const dirs = new Set();
  try {
    // `ls-tree -r --name-only` 而不是 `ls-files -z HEAD`：这台机器上的 git 在没有
    // pathspec 时给 `ls-files <tree>` 回空，于是每条技能都会被错标成「未发布」。
    const out = await run("git", ["-C", root, "ls-tree", "-r", "--name-only", "HEAD"], { timeout: 20_000 });
    for (const file of out.stdout.split("\n")) {
      if (!file) continue;
      const parts = file.trim().split("/");
      for (let i = 1; i < parts.length; i++) dirs.add(parts.slice(0, i).join("/"));
    }
  } catch {
    return new Set();
  }
  return dirs;
};

/** 某个相对路径在 HEAD 里存在吗（diff 的 `before: null` 靠它判断）。 */
export const trackedFile = async (root, rel) => {
  try {
    const out = await run("git", ["-C", root, "ls-tree", "--name-only", "HEAD", "--", rel], { timeout: 15_000 });
    return out.stdout.trim().length > 0;
  } catch {
    return false;
  }
};

/** 仓库状态：有没有没提交的、领先/落后远端几步。 */
export const repoState = async (root) => {
  const opts = { cwd: root, timeout: 15_000 };
  const head = (await run("git", ["rev-parse", "HEAD"], opts)).stdout.trim();
  const dirty = (await run("git", ["status", "--porcelain"], opts)).stdout.trim().length > 0;
  let ahead = 0;
  let behind = 0;
  try {
    ahead = Number((await run("git", ["rev-list", "--count", "@{u}..HEAD"], opts)).stdout.trim()) || 0;
    behind = Number((await run("git", ["rev-list", "--count", "HEAD..@{u}"], opts)).stdout.trim()) || 0;
  } catch {
    // 没有上游 / 断网：当成 0 说，别让一个查询把整页卡死。
  }
  return { head, dirty, ahead, behind };
};

const trimRemote = (url) => {
  const raw = String(url ?? "").trim();
  const ssh = raw.match(/^git@([^:]+):(.+?)(?:\.git)?$/);
  if (ssh) return `https://${ssh[1]}/${ssh[2]}`;
  return raw.replace(/\.git$/, "");
};

export const remoteUrl = async (root) =>
  run("git", ["-C", root, "remote", "get-url", "origin"], { timeout: 10_000 })
    .then((out) => trimRemote(out.stdout))
    .catch(() => "");

/** 最后一条 commit 的短哈希 + 说明（发布响应里的 `commit: {hash, subject}` 用）。 */
export const lastCommitInfo = async (root) => {
  const line = (await run("git", ["-C", root, "log", "-1", "--format=%h%x09%s"], { timeout: 15_000 })).stdout.trim();
  const [hash, ...subject] = line.split("\t");
  return { hash: hash ?? null, subject: subject.join("\t") };
};

/** 一条已提交的 commit 改了哪些文件 —— 弹窗里点开看，人去不去推它。 */
export const pendingCommit = async (root, hash) => {
  const sha = String(hash ?? "").trim();
  if (!/^[0-9a-f]{4,40}$/i.test(sha)) throw withStatus(new Error("提交号不对"), 400);
  const meta = (await run("git", ["-C", root, "show", "-s", "--format=%H%x09%an%x09%ad%x09%s", "--date=short", sha], {
    timeout: 20_000,
  })).stdout.trim();
  const [full, author, date, ...subject] = meta.split("\t");
  // `--name-status` 而不是 `--numstat`：弹窗要的是「这文件是加了还是删了」，不是行数
  // （spec §6.1 的 `files: [{path, status}]`）。`--format=` 把 commit 头整段打空，
  // 剩下的每一行就是 `状态\t路径`（改名是 `R100\t旧\t新`）。
  const files = (await run("git", ["-C", root, "show", "--name-status", "--format=", sha], { timeout: 60_000 })).stdout
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const [code, ...paths] = line.split("\t");
      // 改名 / 复制有两个路径：取最后一个（= 现在盘上叫的那个名字）。
      return { path: paths[paths.length - 1] ?? "", status: String(code ?? "").charAt(0) || "?" };
    })
    .filter((file) => file.path);
  return { hash: full, short: full.slice(0, 7), author, date, subject: subject.join("\t"), files };
};

/** 一个文件「对 HEAD」的前后内容 —— 给弹窗里的 diff 用。 */
export const pendingFileDiff = async (root, rel) => {
  const clean = String(rel ?? "").trim();
  if (!clean || clean.includes("..") || path.isAbsolute(clean)) throw withStatus(new Error("路径不对"), 400);
  const file = path.join(root, clean);
  if (!file.startsWith(root + path.sep)) throw withStatus(new Error("路径跑到仓库外面了"), 400);
  const info = await stat(file).catch(() => null);
  if (info && info.size > MAX_FILE_BYTES * 4) throw withStatus(new Error("文件太大，不在面板里看 diff"), 413);
  const base = await run("git", ["-C", root, "show", `HEAD:${clean}`], { timeout: 20_000 })
    .then((out) => out.stdout)
    .catch(() => "");
  const text = info ? await readFile(file, "utf8").catch(() => "") : "";
  return { path: clean, base, text, missing: !text && !base };
};

// index.json 是 categories.json 的生成物：只提交词表不提交索引，仓库当场自相矛盾。
const isCategoryFile = (rel) =>
  rel === "categories.json" || rel === "index.json" || rel.endsWith("nous.json");

const EXT_CODE_RE = /\.(mjs|cjs|js)$/i;
const fileKindOf = (rel) => (EXT_CODE_RE.test(rel) ? "code" : isCategoryFile(rel) ? "manifest" : "other");

/** 一条改动属于哪个单元。顶层目录就是一个东西，顶层文件是词表 / 索引 / 说明。 */
const unitOfPath = (rel, unitKind) => {
  const parts = rel.split("/");
  if (parts[0] === ARCHIVE_DIR && parts.length > 2) return { key: `${ARCHIVE_DIR}/${parts[1]}`, kind: "archive", name: parts[1] };
  if (parts.length > 1 && !parts[0].startsWith(".")) return { key: parts[0], kind: unitKind, name: parts[0] };
  if (isCategoryFile(rel)) return { key: "__categories", kind: "categories", name: null };
  return { key: "__other", kind: "other", name: null };
};

const UNIT_ORDER = { extension: 0, skill: 0, prompt: 0, categories: 1, archive: 2, other: 3 };

// ── 待同步清单：把工作区的脏文件按「改动单元」分组 ─────────────────────────
//
// 单元是**一个目录**，不是文件：人想的是「把 123 发出去」，不是「提交 index.js」。
// 文件级的勾选藏在展开里。另外「已提交没推」和「没提交」是两件事，这里分开给。
export const pendingState = async (root, { unitKind = "extension" } = {}) => {
  const changes = await gitChanges(root);
  const state = await repoState(root);
  const tracked = await trackedDirs(root);

  // 行数口径和 git 一致：对 HEAD 比（暂存 + 未暂存都算）。未跟踪的文件 git diff 看不见，单独数。
  const numstat = new Map();
  const trackedPaths = changes.filter((item) => !item.status.startsWith("??")).map((item) => item.path);
  if (trackedPaths.length) {
    const out = (await run("git", ["-C", root, "diff", "--numstat", "HEAD", "--", ...trackedPaths], { timeout: 30_000 }))
      .stdout;
    for (const line of out.split("\n").filter(Boolean)) {
      const [adds, dels, ...rest] = line.split("\t");
      numstat.set(rest.join("\t"), { added: adds === "-" ? 0 : Number(adds), removed: dels === "-" ? 0 : Number(dels) });
    }
  }

  const units = new Map();
  for (const change of changes) {
    const { key, kind, name } = unitOfPath(change.path, unitKind);
    const deleted = change.status.includes("D");
    let lines = numstat.get(change.path) ?? { added: 0, removed: 0 };
    if (change.status.startsWith("??")) {
      // 整份文件都是新增。读不了 / 太大就记 0 —— 这个数只用来判断量级，不参与决策。
      const info = await stat(path.join(root, change.path)).catch(() => null);
      if (info && info.size <= MAX_FILE_BYTES) {
        const text = await readFile(path.join(root, change.path), "utf8").catch(() => "");
        lines = { added: text ? text.replace(/\n+$/, "").split("\n").length : 0, removed: 0 };
      }
    }
    const unit = units.get(key) ?? {
      key,
      kind,
      name,
      tracked: kind === unitKind ? tracked.has(name) : true,
      files: [],
      added: 0,
      removed: 0,
      code: false,
      hasDeleted: false,
    };
    unit.files.push({ path: change.path, status: change.status.trim() || "??", kind: fileKindOf(change.path), deleted, ...lines });
    unit.added += lines.added;
    unit.removed += lines.removed;
    if (fileKindOf(change.path) === "code") unit.code = true;
    if (deleted) unit.hasDeleted = true;
    units.set(key, unit);
  }

  const list = [...units.values()].sort(
    (a, b) => UNIT_ORDER[a.kind] - UNIT_ORDER[b.kind] || String(a.name ?? a.key).localeCompare(String(b.name ?? b.key)),
  );

  let aheadCommits = [];
  if (state.ahead) {
    const log = await run("git", ["-C", root, "log", "--format=%h%x09%s", "-n", "30", "@{u}..HEAD"], { timeout: 20_000 })
      .then((out) => out.stdout)
      .catch(() => "");
    aheadCommits = log
      .split("\n")
      .filter(Boolean)
      .map((line) => {
        const [hash, ...rest] = line.split("\t");
        return { hash, subject: rest.join("\t") };
      });
  }

  return { ...state, remote: await remoteUrl(root), units: list, aheadCommits };
};

// ── 归档 ────────────────────────────────────────────────────────────────────
//
// `.archive/<name>/` 在仓库里跟着 origin 走，clone 下来也在 —— 所以「从库里删」仍然是
// 可逆的。`purge` 才是真没。

/** 归档区里有什么（按归档时间新→旧）。仓库里没有 .archive/ 就返回空。 */
export const archivedEntries = async (root, { manifest = "nous.json" } = {}) => {
  const base = path.join(root, ARCHIVE_DIR);
  let dirs = [];
  try {
    dirs = (await readdir(base, { withFileTypes: true })).filter((entry) => entry.isDirectory()).map((entry) => entry.name);
  } catch {
    return [];
  }
  const out = [];
  for (const name of dirs) {
    const info = await readManifest(root, path.join(ARCHIVE_DIR, name), { manifest });
    let archivedAt = null;
    try {
      archivedAt = (await run("git", ["-C", root, "log", "-1", "--format=%cI", "--", `${ARCHIVE_DIR}/${name}`])).stdout.trim() || null;
    } catch {}
    out.push({
      name,
      version: info?.manifest?.version ?? null,
      summary: info?.manifest?.summary ?? null,
      category: info?.manifest?.category ?? [],
      archivedAt,
    });
  }
  return out.sort((a, b) => String(b.archivedAt ?? "").localeCompare(String(a.archivedAt ?? "")) || a.name.localeCompare(b.name));
};

/** 把一个目录搬进 / 搬出 .archive/；tracked 的走 git mv（保留历史），否则普搬 + git add。 */
export const moveIntoArchive = async (root, name) => {
  // `name` 可以是带分组的相对路径（`deprecated/demo`）。归档目标是**扁的**
  // `.archive/<最后一段>` —— 归档区不跟着分组分层，否则“已删除”里还得再点两层。
  const leaf = path.basename(name);
  const target = `${ARCHIVE_DIR}/${leaf}`;
  await mkdir(path.join(root, ARCHIVE_DIR), { recursive: true });
  if (await stat(path.join(root, target)).catch(() => null)) {
    throw withStatus(new Error(`${target} 已经存在 —— 先到「已删除」里处理掉那一条`), 409);
  }
  if (await isTrackedDir(root, name)) {
    await run("git", ["-C", root, "mv", name, target], { timeout: 30_000 });
  } else {
    await moveDir(path.join(root, name), path.join(root, target));
    await run("git", ["-C", root, "add", "--", target], { timeout: 30_000 });
  }
  // 分组里的最后一条被归档之后，`deprecated/` 会空掉 —— git 不跟踪空目录，
  // 留着它只会让下一次 `mkdir deprecated` 撞上「已经存在」。
  for (let parent = path.dirname(name); parent !== "." && parent !== "/"; parent = path.dirname(parent)) {
    const dir = path.join(root, parent);
    if ((await readdir(dir).catch(() => ["?"])).length) break;
    await rm(dir, { recursive: true, force: true });
  }
};

/**
 * 从 `.archive/<name>` 搬回去。`to` = 目的地（相对仓根），默认就是 `name`。
 * 分组里的技能归档过、再恢复，得回到原来那个分组里 —— 目的地由调用方给。
 */
export const moveOutOfArchive = async (root, name, { to = name } = {}) => {
  const source = `${ARCHIVE_DIR}/${name}`;
  if (!(await stat(path.join(root, source)).catch(() => null))) {
    throw withStatus(new Error(`归档区里没有 ${name}`), 404);
  }
  if (await stat(path.join(root, to)).catch(() => null)) {
    throw withStatus(new Error(`工作区里已经有一个叫 ${to} 的目录了`), 409);
  }
  // `git mv` 不接受一个不存在的目标父目录，分组目录可能早就空了（它自己也未跟踪）
  const parent = path.dirname(to);
  if (parent !== ".") await mkdir(path.join(root, parent), { recursive: true });
  if (await isTrackedDir(root, source)) {
    await run("git", ["-C", root, "mv", source, to], { timeout: 30_000 });
  } else {
    await moveDir(path.join(root, source), path.join(root, to));
    await run("git", ["-C", root, "add", "--", to], { timeout: 30_000 });
  }
};

// ── 提交 + push ─────────────────────────────────────────────────────────────

/**
 * index.json 里有哪些目录**不在 HEAD 里** —— 也就是「未发布」。它们合法地出现在
 * 工作区的 index.json 里，但**不该跟着自动提交走**。
 */
const untrackedIndexNames = async (root) => {
  let entries = [];
  try {
    const doc = JSON.parse(await readFile(path.join(root, "index.json"), "utf8"));
    entries = doc.extensions ?? doc.skills ?? [];
  } catch {
    return [];
  }
  // 条目说的「住哪儿」是 `path`（`deprecated/demo`），老文件没有这个字段就是 `name`。
  const relOf = (item) => String(item.path ?? item.name);
  const tracked = await trackedPaths(root);
  return entries.filter((item) => !tracked.has(relOf(item))).map(relOf);
};

/** 把未发布的目录从 index.json 里摘掉（只动磁盘；提交完再重扫回去）。 */
const pruneIndex = async (root, paths) => {
  const indexFile = path.join(root, "index.json");
  const doc = JSON.parse(await readFile(indexFile, "utf8"));
  const list = doc.extensions ?? doc.skills ?? [];
  const kept = list.filter((item) => !paths.includes(String(item.path ?? item.name)));
  const next = doc.extensions ? { ...doc, extensions: kept } : { ...doc, skills: kept };
  await writeFile(indexFile, JSON.stringify(next, null, 2) + "\n", "utf8");
};

/**
 * 把待提交改动打成一条 commit，然后 push。push 失败要说实话，不能装成功。
 *
 * `only: "categories"` = 只提交词表和清单，**不碰代码**（类别那条 8 秒 debounce 走这个）。
 * `includeDeleted: true` = 允许提交删除（归档 / 恢复 / 彻底删除是显式动作）。
 * `paths` = 人在弹窗里显式勾了哪几条，没勾的一个字节都不提交。
 */
export const syncRepo = async (
  root,
  message,
  { only = null, includeDeleted = false, paths = null, label = "扩展", tag = "extend" } = {},
) => {
  const opts = { cwd: root, timeout: 60_000, env: { ...process.env, GIT_TERMINAL_PROMPT: "0" } };
  let committed = false;
  const changes = await gitChanges(root);
  // 「本地删了某个目录」必然让 tracked 文件变成 ` D`。这条过滤是「本地删除」和「发布」
  // 之间的防火墙：发布推的是工作区，不挡住的话，你删了 demo、隔天改一行点发布，
  // 删除就跟着上 GitHub。
  const renamedFrom = new Map(changes.filter((item) => item.from).map((item) => [item.path, item.from]));
  const deleted = new Set(changes.filter((item) => item.status.includes("D")).map((item) => item.path));
  const changed = changes.map((item) => item.path);
  let targets = (only === "categories" ? changed.filter(isCategoryFile) : changed).filter(
    (rel) => includeDeleted || !deleted.has(rel),
  );
  if (Array.isArray(paths)) {
    // 显式勾选：只碰这几条。没勾的一个字节都不提交 —— 这是「按目录选」的全部实现。
    const want = new Set(paths.map((rel) => String(rel)));
    targets = targets.filter((rel) => want.has(rel));
    // 代码改动要过语法闸：不走抽屉也能提交代码，代价就是这道检查。
    for (const rel of targets) {
      if (!EXT_CODE_RE.test(rel)) continue;
      const text = await readFile(path.join(root, rel), "utf8").catch(() => null);
      if (text !== null) await checkSyntax(rel, text);
    }
  }
  // 类别那条 debounce 是**自动**提交，它不能替谁发布。未发布的目录也在 index.json 里
  // （新建的、复制出来的），整份推上去 = 「仓库说我有它，实际没有」。
  // 所以提交前把不在 HEAD 里的条目从索引里摘掉，提交完再重扫回来（它会重新变脏，正是
  // 该有的样子）。谁进了这次提交，谁就有资格留在索引里。
  let skippedIndex = [];
  if (targets.includes("index.json")) {
    // 「谁进了这次提交，谁就有资格留在索引里」—— 按**路径前缀**比，不是按顶层目录：
    // 分组里的条目是 `deprecated/demo`，拿 `deprecated` 去比会把半截路径当成命中。
    const touches = (target, item) => target === item || target.startsWith(`${item}/`);
    const untracked = await untrackedIndexNames(root);
    const skip = untracked.filter((item) => !targets.some((target) => touches(target, item)));
    if (skip.length) {
      await pruneIndex(root, skip);
      skippedIndex = skip;
    }
  }

  try {
    if (targets.length) {
      // 已删掉的路径不能再 `git add`（“pathspec did not match any files”）；它们的删除
      // 已经由 `git rm` / `git mv` 暂存好了。只 add 还存在的文件。
      const addable = [];
      for (const rel of targets) {
        if (await stat(path.join(root, rel)).catch(() => null)) addable.push(rel);
      }
      if (addable.length) await run("git", ["add", "--", ...addable], opts);
      // 用暂存区判断“真有待提交的东西”：纯删除（git rm）不会有 addable，但也得提交。
      // `--no-renames` 不是省事：改名默认只报目标侧，而 `git commit -- <pathspec>`
      // 必须拿到**两侧**才提交得出一条完整改名。少了源侧，HEAD 里会留一个
      // 「新目录有了、旧目录没删」的半截 —— 踩过：归档的提交只剩 A，D 留在暂存区。
      const stagedAll = (await run("git", ["diff", "--cached", "--name-only", "--no-renames"], opts)).stdout
        .split("\n")
        .map((line) => line.trim())
        .filter(Boolean);
      // **commit 必须带 pathspec。** 不带的话 `git commit` 提交的是整个暂存区 ——
      // 工作区里那条被挡下来的删除（别人 `git rm` 掉的旧目录）就搭着这次发布一起上了。
      // 改名的两侧都要列：只给目标侧会在 HEAD 里留一个「只加不删」的半截改名。
      // 未跟踪目录在 porcelain 里被折成一条 `?? dir/`，而暂存区里是它下面的文件名 ——
      // 所以「这条 pathspec 有没有东西可提交」要按前缀比，不是按等号比。
      const covers = (rel, stagedRel) => stagedRel === rel || stagedRel.startsWith(rel.endsWith("/") ? rel : `${rel}/`);
      const pathspec = [...new Set(targets.flatMap((rel) => [rel, renamedFrom.get(rel)])).values()]
        .filter(Boolean)
        .filter((rel) => rel === "index.json" || addable.includes(rel) || stagedAll.some((staged) => covers(rel, staged)));
      if (pathspec.some((rel) => stagedAll.some((staged) => covers(rel, staged)))) {
        // 默认说明按**真改了的文件**生。混着代码改动却写成 `category:` 是骗人
        // —— 将来的 `git log` 靠这一行找人。
        const when = new Date().toISOString().slice(0, 19).replace("T", " ");
        const onlyCategories = targets.every(isCategoryFile);
        const fallback = onlyCategories
          ? `category: 更新${label}类别（${when}）`
          : `${tag}: 更新 ${targets.slice(0, 4).join("、")}${targets.length > 4 ? ` 等 ${targets.length} 个文件` : ""}`;
        const msg = String(message ?? "").trim() || fallback;
        await run("git", ["commit", "-m", msg, "--", ...pathspec], opts);
        committed = true;
      }
    }
    const hash = committed
      ? (await run("git", ["-C", root, "rev-parse", "--short", "HEAD"], opts)).stdout.trim()
      : null;
    const state = await repoState(root);
    if (!state.ahead) return { committed, hash, files: targets, skippedIndex, ...state, pushed: null };
    try {
      await run("git", ["push"], opts);
    } catch (error) {
      const pushError = errorDetail(error);
      return { committed, hash, files: targets, skippedIndex, ...state, pushed: false, pushError };
    }
    return { committed, hash, files: targets, skippedIndex, ...state, ahead: 0, pushed: true };
  } finally {
    // 索引必须回到「盘上有什么就写什么」，否则页面和生成器看到的是被摘过的版本。
    if (skippedIndex.length) {
      await runIndex(root).catch((error) => console.error("[repo] 重扫 index.json 失败：", errorDetail(error)));
    }
  }
};

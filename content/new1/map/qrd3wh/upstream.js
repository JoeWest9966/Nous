// 上游 —— 订阅、镜像、只读清单。
//
// 一句话：**上游就是一个 git remote。**
//
// 老的 warehouse 那套（`sources` 表 + tarball + `.nw.json` 收据 + 版本号）是在用 HTTP
// 重建 git 本来就有的事实：版本是 sha，漂移是 diff，历史是 log。所以这里只做三件事，
// 而且全是只读的：
//
//   1. `sources.json` —— **订阅了谁**。它是内容，跟 `categories.json` 并列进版本库；
//      换台机器 clone 下来就知道我们跟着哪些仓。
//   2. 每个 source 一个 bare 镜像，落在 `NOUS_DATA/upstream/<name>`。**这是缓存**，
//      不进 git：删了下次 fetch 重 clone，永远能从源头重建。
//   3. 从镜像里读清单 —— `git ls-tree` + `git cat-file`，**不 checkout**。
//      一条技能的清单是一次 tree walk，不展开工作区。
//
// 三条纪律：
//
//   · **不联网除非明说。** 只有 `refresh` 才 `git fetch`。`GET /v1/skills/upstream` 只读
//     已有镜像 —— 打开面板不会去打 GitHub。
//   · **不建立活的追踪关系。** adopt 之后只写一个注脚 `nous.json.upstream`（source/path@sha），
//     `buildIndex` / `load()` / `render()` 都不看它。一旦让它参与判断，就回到「两份真相会漂」。
//   · **不写进别人的仓。** 这里只读上游；写只发生在我们自己的仓库里，而且仍然是
//     未发布状态（adopt 不提交）。

import { mkdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";

export const SOURCES_FILE = "sources.json";
export const MIRROR_DIR = "upstream";
/** 一条正文的上限。跟本地文件同一个数（spec §6.6），免得「上游能看 5MB、本地只能看 512K」。 */
const MAX_TEXT_BYTES = 512 * 1024;

const withStatus = (error, status) => Object.assign(error, { status });
const errorLine = (error) => String(error?.message ?? error).split("\n")[0].slice(0, 400);

/** 名字进 URL、进目录名、进 `nous.json.upstream`，所以认得窄一点。 */
const SOURCE_NAME_RE = /^[a-z0-9][a-z0-9._-]{0,31}$/;
const GIT_ARG_RE = /^[A-Za-z0-9._/-]+$/;

const normalizePrefix = (raw) => {
  const prefix = String(raw ?? "").trim().replace(/^\/+|\/+$/g, "");
  if (!prefix) return null;
  if (!GIT_ARG_RE.test(prefix) || prefix.split("/").includes("..")) {
    throw withStatus(new Error(`prefix 只能是仓库里的相对路径：${raw}`), 400);
  }
  return prefix;
};

/**
 * 一条订阅。`url` 认 git 认识的一切：`https://`、`ssh://`、`git@host:path`、
 * `file:///…`，以及**裸的绝对路径** —— 最后这种让「另一台机器上的 Nous-skills」
 * 也能当上游，不用先架个服务。
 */
export const normalizeSource = (raw) => {
  const name = String(raw?.name ?? "").trim();
  if (!SOURCE_NAME_RE.test(name)) {
    throw withStatus(new Error(`上游名只认小写字母、数字、点、下划线、连字符，≤32 字：${name || "(空)"}`), 400);
  }
  const url = String(raw?.url ?? "").trim();
  if (!url) throw withStatus(new Error(`${name}: url 不能空`), 400);
  if (/\s/.test(url)) throw withStatus(new Error(`${name}: url 里不能有空白`), 400);
  const local = url.startsWith("/");
  if (!local && !/^(https?|ssh|git|file):\/\//.test(url) && !/^[A-Za-z0-9._-]+@[^:]+:/.test(url)) {
    throw withStatus(new Error(`${name}: 认不出这个地址（要 https:// · ssh:// · git@host:path · file:// · 绝对路径）：${url}`), 400);
  }
  const branch = String(raw?.branch ?? "").trim() || null;
  if (branch && !GIT_ARG_RE.test(branch)) throw withStatus(new Error(`${name}: branch 不合法：${branch}`), 400);
  const prefix = normalizePrefix(raw?.prefix);
  return { name, url, ...(branch ? { branch } : {}), ...(prefix ? { prefix } : {}) };
};

export const emptySources = () => ({ v: 1, sources: [] });

/** 文件不在 = 没订阅任何人（不是错误）。文件在但读不懂 = 400，别装作没订阅。 */
export const readSources = async (root) => {
  let text;
  try {
    text = await readFile(path.join(root, SOURCES_FILE), "utf8");
  } catch {
    return emptySources();
  }
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch (error) {
    throw withStatus(new Error(`sources.json 不是合法 JSON：${errorLine(error)}`), 400);
  }
  const list = Array.isArray(parsed?.sources) ? parsed.sources : [];
  const sources = list.map(normalizeSource);
  const seen = new Set();
  for (const source of sources) {
    if (seen.has(source.name)) throw withStatus(new Error(`sources.json 里 ${source.name} 出现了两次`), 400);
    seen.add(source.name);
  }
  return { v: 1, sources };
};

export const writeSources = async (root, list) => {
  const sources = (Array.isArray(list) ? list : []).map(normalizeSource);
  const seen = new Set();
  for (const source of sources) {
    if (seen.has(source.name)) throw withStatus(new Error(`上游名撞了：${source.name}`), 400);
    seen.add(source.name);
  }
  const file = path.join(root, SOURCES_FILE);
  const tmp = `${file}.tmp-${Date.now()}`;
  // 手写 `_about`：跟 index.json 一个口气，别让人以为这文件是某个程序的状态。
  const body = { v: 1, _about: ["订阅了哪些上游。这是内容，进版本库 —— 换台机器 clone 下来照样知道。镜像在块的 data/ 里，删了能重建。"], sources };
  await writeFile(tmp, JSON.stringify(body, null, 2) + "\n", "utf8");
  await rename(tmp, file);
  return sources;
};

// ── 镜像（缓存，不是事实）────────────────────────────────────────────────────

export const mirrorPath = (data, name) => path.join(data, MIRROR_DIR, name);

const exists = async (file) => Boolean(await stat(file).catch(() => null));

const git = (run, dir, args, opts = {}) =>
  run("git", ["-C", dir, ...args], { timeout: 60_000, maxBuffer: 64 << 20, ...opts });

/**
 * 有就用了，没有就 clone 一个 bare 镜像。
 *
 * `--filter=blob:none` 是省空间用的（只拉 refs + tree，blob 按需 —— 反正我们按需
 * `cat-file`）。有的服务端不支持（老 git、某些自建桶），失败了就退回全量 clone：
 * 「省空间失败」不该等于「上游用不了」。
 */
export const ensureMirror = async ({ data, source, run }) => {
  const dir = mirrorPath(data, source.name);
  if (await exists(path.join(dir, "HEAD"))) return dir;
  await mkdir(path.dirname(dir), { recursive: true });
  const clone = async (extra) => {
    await rm(dir, { recursive: true, force: true }).catch(() => {});
    await run("git", ["clone", "--bare", "--quiet", ...extra, source.url, dir], {
      timeout: 180_000,
      maxBuffer: 16 << 20,
    });
  };
  try {
    await clone(["--filter=blob:none", "--no-single-branch"]);
  } catch (error) {
    await clone([]).catch(() => {
      throw withStatus(new Error(`${source.name}: clone 失败 —— ${errorLine(error)}`), 400);
    });
  }
  return dir;
};

/** 唯一的联网动作。调用点是 `POST /v1/skills/upstream/refresh`，别的地方不许调。 */
export const fetchMirror = async ({ data, source, run }) => {
  const dir = await ensureMirror({ data, source, run });
  try {
    await git(run, dir, ["fetch", "--quiet", "--prune", "origin", "+refs/heads/*:refs/heads/*"], {
      timeout: 180_000,
    });
  } catch (error) {
    throw withStatus(new Error(`${source.name}: fetch 失败 —— ${errorLine(error)}`), 400);
  }
  return dir;
};

/** 镜像在不在。`GET /v1/skills/upstream` 只读已有的 —— 打开面板不该去打 GitHub。 */
export const hasMirror = async ({ data, source }) => exists(path.join(mirrorPath(data, source.name), "HEAD"));

/** 这个 source 现在指向哪个 commit：指定的 branch，否则镜像的 HEAD。 */
export const resolveHead = async ({ data, source, run }) => {
  const dir = await ensureMirror({ data, source, run });
  const refs = source.branch ? [`refs/heads/${source.branch}`, "HEAD"] : ["HEAD"];
  let lastError = null;
  for (const ref of refs) {
    try {
      const out = (await git(run, dir, ["rev-parse", `${ref}^{commit}`])).stdout.trim();
      if (out) return out;
    } catch (error) {
      lastError = error;
    }
  }
  throw withStatus(
    new Error(`${source.name}: 找不到 ${source.branch ? `分支 ${source.branch}` : "HEAD"}${lastError ? ` —— ${errorLine(lastError)}` : ""}`),
    400,
  );
};

/**
 * `ls-tree -r` 一次，之后所有「哪个目录里有 SKILL.md」都从这份清单里挑。
 *
 * `long: true` 加一列大小（`ls-tree -l`）—— 面板的文件树要显示 `1.2 KB`，
 * 而为每个文件单独 `cat-file -s` 是 N 个进程。这一列是白送的，不拿白不拿。
 */
const treeOf = async (run, dir, sha, { long = false } = {}) => {
  const out = (await git(run, dir, ["ls-tree", "-r", "-z", ...(long ? ["-l"] : []), sha], { maxBuffer: 64 << 20 })).stdout;
  return out
    .split("\0")
    .filter(Boolean)
    .map((row) => {
      const tab = row.indexOf("\t");
      // `-l` 那一列是**右对齐带空格的**（`…45     171`），按单个空格切会切出一堆空串。
      const head = row.slice(0, tab).trim().split(/\s+/);
      const [mode, type, obj] = head;
      // `-l` 多一列 size，目录（tree）那一列是 `-`。
      const size = long ? Number(head[3]) : undefined;
      return { mode, type, sha: obj, path: row.slice(tab + 1), ...(Number.isFinite(size) ? { size } : {}) };
    });
};

const blobText = async (run, dir, sha) => (await git(run, dir, ["cat-file", "blob", sha])).stdout;

const blobBytes = async (run, dir, sha) =>
  (await git(run, dir, ["cat-file", "blob", sha], { encoding: "buffer", maxBuffer: 64 << 20 })).stdout;

/** 只挑 frontmatter 里那两行，别为了一条技能把整段正文读进内存两遍。 */
const frontmatterOf = (text) => {
  const match = /^\uFEFF?---[ \t]*\r?\n([\s\S]*?)\r?\n---/.exec(text);
  if (!match) return null;
  const fields = {};
  for (const line of match[1].split(/\r?\n/)) {
    const at = line.indexOf(":");
    if (at === -1) continue;
    fields[line.slice(0, at).trim()] = line.slice(at + 1).trim().replace(/^"(.*)"$/, "$1");
  }
  return fields;
};

/**
 * 镜像里有哪些技能。**不 checkout** —— 全部从 tree 和 blob 读。
 *
 * 一条一条 `cat-file` 是 N 个进程，不是 N 次网络：镜像在本地，几十条技能是几十毫秒。
 * 真到了几百条的规模再换 `cat-file --batch`，现在还不用为那个复杂度付费。
 */
export const listEntries = async ({ data, source, run }) => {
  const head = await resolveHead({ data, source, run });
  const dir = mirrorPath(data, source.name);
  const rows = await treeOf(run, dir, head);
  const prefix = source.prefix ?? "";

  const skillDirs = new Map();
  for (const row of rows) {
    if (!row.path.endsWith("/SKILL.md")) continue;
    const home = row.path.slice(0, -"/SKILL.md".length);
    if (!home) continue; // 仓库根上放一个 SKILL.md = 整个仓是一条技能，这种形状不认
    if (home.split("/").some((part) => part.startsWith("."))) continue; // .archive/ .github/ …
    if (prefix && !(home === prefix || home.startsWith(`${prefix}/`))) continue;
    skillDirs.set(home, { home, skillSha: row.sha });
  }

  const entries = [];
  for (const { home, skillSha } of skillDirs.values()) {
    const text = await blobText(run, dir, skillSha);
    const front = frontmatterOf(text) ?? {};
    let manifest = null;
    const nous = rows.find((row) => row.path === `${home}/nous.json`);
    if (nous) {
      try {
        manifest = JSON.parse(await blobText(run, dir, nous.sha));
      } catch {
        manifest = null;
      }
    }
    // 目录的 tree sha：它就是「这条上游技能此刻的版本」。adopt 时抄进 nous.json。
    let treeSha = null;
    try {
      treeSha = (await git(run, dir, ["rev-parse", `${head}:${home}`])).stdout.trim();
    } catch {
      treeSha = null;
    }
    entries.push({
      source: source.name,
      path: home,
      name: path.posix.basename(home),
      description: String(front.description ?? "").trim(),
      summary: String(manifest?.summary ?? "").trim(),
      category: Array.isArray(manifest?.category) ? manifest.category.filter((item) => typeof item === "string") : [],
      sha: treeSha,
      head,
      files: rows.filter((row) => row.path.startsWith(`${home}/`)).length,
    });
  }
  return entries.sort((a, b) => a.path.localeCompare(b.path, "zh-CN"));
};

/** 一条上游技能里的文件清单（对照 + 抽屉里那棵树都吃它）。 */
export const entryFiles = async ({ data, source, run, dir: home }) => {
  const head = await resolveHead({ data, source, run });
  const dir = mirrorPath(data, source.name);
  const rows = await treeOf(run, dir, head, { long: true });
  return rows
    .filter((row) => row.type === "blob" && row.path.startsWith(`${home}/`))
    .map((row) => ({ path: row.path.slice(home.length + 1), sha: row.sha, mode: row.mode, size: row.size ?? 0 }))
    .sort((a, b) => a.path.localeCompare(b.path));
};

/**
 * 读上游一条技能里的一个文件。**只读** —— 上游那一侧没有任何写入口，
 * 这条路径上也不该长出写入口（改上游 = 去他们的仓提 PR，不是在我们的面板里）。
 */
export const entryText = async ({ data, source, run, home, rel }) => {
  const head = await resolveHead({ data, source, run });
  const dir = mirrorPath(data, source.name);
  // 先在 `rel` 上验，再 join：`..` 交给 `path.posix.join` 就**已经被折掉了**，
  // 验折完之后的结果等于什么都没验（`tdd/../../README.md` → `skills/README.md`）。
  if (rel.startsWith("/") || rel.split("/").includes("..")) {
    throw withStatus(new Error(`${source.name}: 路径只能在上游那一条里面 —— ${rel}`), 400);
  }
  const want = path.posix.join(home, rel);
  const rows = await treeOf(run, dir, head, { long: true });
  const row = rows.find((item) => item.path === want);
  if (!row) throw withStatus(new Error(`${source.name}: ${want} 不在上游里`), 404);
  if (row.type !== "blob") throw withStatus(new Error(`${source.name}: ${want} 不是文件`), 400);
  if ((row.size ?? 0) > MAX_TEXT_BYTES) {
    throw withStatus(
      new Error(`${source.name}: ${want} 有 ${Math.round(row.size / 1024)} KB，超过 ${MAX_TEXT_BYTES / 1024} KB —— 面板里不打开`),
      400,
    );
  }
  return { path: rel, size: row.size ?? 0, text: await blobText(run, dir, row.sha) };
};

/**
 * 把一条上游技能摊到 `target` 目录里（调用方给的是点开头的暂存目录）。
 *
 * 不走 `git archive | tar`：这里没有 shell，也不想为一个 tar 引依赖。就按 blob 一个个写，
 * 二进制原样（`encoding: buffer`），路径在 home 底下才收。
 */
export const materialize = async ({ data, source, run, home, target }) => {
  const head = await resolveHead({ data, source, run });
  const dir = mirrorPath(data, source.name);
  const rows = await treeOf(run, dir, head);
  const written = [];
  for (const row of rows) {
    if (!row.path.startsWith(`${home}/`)) continue;
    const rel = row.path.slice(home.length + 1);
    if (row.type !== "blob") continue;
    if (rel.split("/").includes("..")) continue;
    const out = path.join(target, rel);
    await mkdir(path.dirname(out), { recursive: true });
    await writeFile(out, await blobBytes(run, dir, row.sha));
    written.push(rel);
  }
  return { head, files: written };
};

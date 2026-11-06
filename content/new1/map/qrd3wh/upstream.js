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

import { mkdir, readFile, readdir, rename, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";

export const SOURCES_FILE = "sources.json";
export const MIRROR_DIR = "upstream";
/** 一条正文的上限。跟本地文件同一个数（spec §6.6），免得「上游能看 5MB、本地只能看 512K」。 */
const MAX_TEXT_BYTES = 512 * 1024;

const withStatus = (error, status) => Object.assign(error, { status });
const errorLine = (error) => String(error?.message ?? error).split("\n")[0].slice(0, 400);

/** 名字进 URL、进目录名、进 `nous.json.upstream`，所以认得窄一点。 */
const SOURCE_NAME_RE = /^[a-z0-9][a-z0-9._-]{0,31}$/;

/**
 * 订阅名 = **仓库名**。`https://github.com/mattpocock/skills.git` → `skills`。
 *
 * 早先取的是倒数第二段（宿主/人），订阅就叫成了 `matt` —— 一个人的名字，不是那个仓，
 * `#upstream/matt` 看着像谁的私聊。现在跟仓走。
 *
 * 这里跟 `68kazk/public/model.js` 的 `repoNameFromUrl` 是**同一条规则的两份**，
 * 所以规则刻意压到一行能说清：取最后一段、去掉 `.git`、小写。改一处必须两处一起改
 * （前端只用来填输入框，服务端是兜底：没给名字就按这条推）。
 */
export const repoName = (url) => {
  const raw = String(url ?? "").trim().replace(/[/:]+$/, "").replace(/\.git$/i, "");
  if (!raw) return "";
  const last = raw.split(/[/:]/).map((part) => part.trim()).filter(Boolean).at(-1) ?? "";
  return last
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^[-._]+|[-._]+$/g, "")
    .slice(0, 32);
};
const GIT_ARG_RE = /^[A-Za-z0-9._/-]+$/;

/**
 * 我们**认**的那个版本：`{ commit, at }`。
 *
 * 它进 `sources.json`（内容，进版本库），跟镜像 HEAD 是两件事：
 *   · `version` 是人钉住的 —— 创建时记一次，之后只有「记录这个版本」能改
 *   · 镜像 HEAD 是采集到的 —— 每次 refresh 都可能往前跑
 * 两者不相等 = 上游动了。记录被自动改写的话，「记录」这两个字就没意义了。
 */
const normalizeVersion = (raw) => {
  if (!raw) return null;
  const commit = String(typeof raw === "string" ? raw : raw.commit ?? "").trim().toLowerCase();
  if (!commit) return null;
  if (!/^[0-9a-f]{7,40}$/.test(commit)) {
    throw withStatus(new Error(`版本要是一个 commit sha（7-40 位十六进制）：${commit}`), 400);
  }
  const at = String(typeof raw === "string" ? "" : raw.at ?? "").trim();
  // 人看的那层：`v1.3.2`（正好打在 tag 上）或者 `behind`（在 v1.3.2 之后还走了几个提交）。
  // 都没有才退回 sha —— 前端会优先显示这两个。
  const tag = String(typeof raw === "string" ? "" : raw.tag ?? "").trim() || null;
  const behind = String(typeof raw === "string" ? "" : raw.behind ?? "").trim() || null;
  return { commit, at: at || new Date().toISOString(), ...(tag ? { tag } : {}), ...(behind ? { behind } : {}) };
};

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
  const url = String(raw?.url ?? "").trim();
  // 没给名字就用仓库名 —— 「订阅的就是那个仓」，名字该是它自己的。
  const given = String(raw?.name ?? "").trim();
  const name = given || repoName(url);
  if (name && !SOURCE_NAME_RE.test(name)) {
    throw withStatus(new Error(`上游名只认小写字母、数字、点、下划线、连字符，≤32 字：${name || "(空)"}`), 400);
  }
  if (!url) throw withStatus(new Error(`${name || "(空)"}: url 不能空`), 400);
  if (/\s/.test(url)) throw withStatus(new Error(`${name}: url 里不能有空白`), 400);
  const local = url.startsWith("/");
  if (!local && !/^(https?|ssh|git|file):\/\//.test(url) && !/^[A-Za-z0-9._-]+@[^:]+:/.test(url)) {
    throw withStatus(new Error(`${name}: 认不出这个地址（要 https:// · ssh:// · git@host:path · file:// · 绝对路径）：${url}`), 400);
  }
  const branch = String(raw?.branch ?? "").trim() || null;
  if (branch && !GIT_ARG_RE.test(branch)) throw withStatus(new Error(`${name}: branch 不合法：${branch}`), 400);
  const prefix = normalizePrefix(raw?.prefix);
  const version = normalizeVersion(raw?.version);
  return {
    name,
    url,
    ...(branch ? { branch } : {}),
    ...(prefix ? { prefix } : {}),
    ...(version ? { version } : {}),
  };
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

/**
 * 把一个订阅的「认的版本」钉到某个 commit（不给就用镜像现在的 HEAD）。
 *
 * 这是**唯一的**改 `version` 的地方。refresh 只在 `version` 还是空的时候
 * 顺手记一次基线（首拉 = 创建），之后它绝不偷偷改 —— 否则「记录」会自己跑。
 */
export const pinVersion = async ({ root, data, source, run, commit = null, at = null }) => {
  const pinned = (commit ?? (await resolveHead({ data, source, run }))).trim().toLowerCase();
  const named = await describeVersion({ data, source, commit: pinned });
  const { sources } = await readSources(root);
  const idx = sources.findIndex((item) => item.name === source.name);
  if (idx === -1) throw withStatus(new Error(`没有订阅「${source.name}」这个上游`), 400);
  sources[idx] = {
    ...sources[idx],
    version: normalizeVersion({ commit: pinned, at: at ?? new Date().toISOString(), ...named }),
  };
  await writeSources(root, sources);
  return sources[idx].version;
};

// ── 版本号 ──────────────────────────────────────────────────────────────────
//
// `4ae2317` 是给机器看的。人看的是**上游自己打的 tag** —— 上游有 `v1.3.2`，
// 我们就说 `v1.3.2`，而不是把 commit sha 截七位糊在卡片上。
//
// 拿法：`git ls-remote --tags <url>`（联网，只在 clone/fetch 那一刻做），
// 结果缓存在镜像的 `.git/nous-tags.json` —— 打开面板的时候不联网，读缓存。

const TAGS_FILE = "nous-tags.json";
const tagsPath = (dir) => path.join(dir, ".git", TAGS_FILE);

/** 拉一遍远端 tag 表（`name → commit`），写进镜像的缓存。只在联网动作里调。 */
export const refreshTags = async ({ dir, source, run }) => {
  const out = (
    await run("git", ["ls-remote", "--tags", source.url], { timeout: 60_000, maxBuffer: 8 << 20 })
  ).stdout;
  const map = {};
  for (const line of out.split("\n")) {
    const [sha, ref] = line.trim().split(/\s+/);
    if (!sha || !ref || !ref.startsWith("refs/tags/")) continue;
    // `^{}` 是附注 tag 指向的那个对象，名字已经收过了（先不带，别覆盖成 peeled）。
    if (ref.endsWith("^{}")) continue;
    map[ref.slice("refs/tags/".length)] = sha;
  }
  await writeFile(tagsPath(dir), JSON.stringify(map, null, 2) + "\n", "utf8").catch(() => {});
  return map;
};

const tagsOf = async ({ data, source }) => {
  try {
    return JSON.parse(await readFile(tagsPath(mirrorPath(data, source.name)), "utf8"));
  } catch {
    return {};
  }
};

/** 挑「最近发布的那个 tag」：优先 `v1.2.3` 这种语义化的，版本号比大小，别比字符串。 */
const newestTag = (tags) => {
  const names = Object.keys(tags);
  const semver = names.filter((name) => /^v?\d+\.\d+\.\d+/.test(name));
  const pool = semver.length ? semver : names;
  const parts = (name) => name.replace(/^v/, "").split(/[.\-+]/).map((n) => Number.parseInt(n, 10) || 0);
  return (
    pool.sort((a, b) => {
      const pa = parts(a);
      const pb = parts(b);
      for (let i = 0; i < 3; i += 1) if (pa[i] !== pb[i]) return pa[i] - pb[i];
      return a.localeCompare(b);
    }).at(-1) ?? null
  );
};

/**
 * 一个 commit 在人眼里的名字。
 *
 * - 正好打在 tag 上 → `{ tag: "v1.3.2" }`
 * - 在 tag 之后（上游天天提交，tag 一周一次）→ `{ behind: "v1.3.2" }`，前端写成
 *   `v1.3.2+`（「v1.3.2 之后的版本」）
 * - 一个 tag 都没有 → `{}`，前端才退回 commit sha（那种仓确实没有版本号可显示）
 */
export const describeVersion = async ({ data, source, commit }) => {
  const tags = await tagsOf({ data, source });
  const want = String(commit ?? "").trim().toLowerCase();
  const exact = Object.keys(tags).find((name) => tags[name] === want);
  if (exact) return { tag: exact };
  const behind = newestTag(tags);
  return behind ? { behind } : {};
};

// ── 检查更新（只问 refs，不拉内容）──────────────────────────────────────────
//
// 第三个版本：`version`（认的）、镜像 HEAD（采集到的）、**远端 HEAD**（上游真正的现在）。
// 前两个都在本地，第三个只有联网才知道 —— 所以它是个显式动作（「检查更新」），
// 结果缓存在镜像的 `.git/nous-check.json`，打开面板时读缓存、不联网。

const CHECK_FILE = "nous-check.json";
const checkPath = (dir) => path.join(dir, ".git", CHECK_FILE);

/** 远端现在停在哪个 commit。`git ls-remote` 只读 refs，不下载对象。 */
export const remoteHead = async ({ source, run }) => {
  const out = (await run("git", ["ls-remote", source.url], { timeout: 60_000, maxBuffer: 8 << 20 })).stdout;
  let head = null;
  let fallback = null;
  for (const line of out.split("\n")) {
    const [sha, ref] = line.trim().split(/\s+/);
    if (!sha || !ref) continue;
    if (ref === "HEAD") head = sha;
    if (ref === `refs/heads/${source.branch ?? ""}`) fallback = sha;
    if (!fallback && ref.startsWith("refs/heads/")) fallback = sha;
  }
  const found = head ?? fallback;
  if (!found) throw withStatus(new Error(`${source.name}: 远端一个 ref 都没读出来`), 400);
  return found;
};

/**
 * 检查上游有没有往前走：远端 HEAD 是多少、在人眼里是哪一版。
 * 结果是**缓存**（`.git/nous-check.json`），面板靠它显示「上游有更新」。
 */
export const checkUpstream = async ({ data, source, run }) => {
  const dir = mirrorPath(data, source.name);
  // tag 表顺手更新 —— 「v1.3.3 出来了」跟「有新提交」是同一件事的两面。
  await refreshTags({ dir, source, run }).catch(() => {});
  const head = await remoteHead({ source, run });
  const at = new Date().toISOString();
  await writeFile(checkPath(dir), JSON.stringify({ head, at }, null, 2) + "\n", "utf8").catch(() => {});
  return { head, at, version: await describeVersion({ data, source, commit: head }) };
};

/** 上次检查的结果（没有就是 `null`）—— `GET` 只读它，别联网。 */
export const checkedOf = async ({ data, source }) => {
  const dir = mirrorPath(data, source.name);
  try {
    const parsed = JSON.parse(await readFile(checkPath(dir), "utf8"));
    return parsed?.head ? parsed : null;
  } catch {
    return null;
  }
};

// ── 镜像（缓存，不是事实）────────────────────────────────────────────────────

export const mirrorPath = (data, name) => path.join(data, MIRROR_DIR, name);

const exists = async (file) => Boolean(await stat(file).catch(() => null));

const git = (run, dir, args, opts = {}) =>
  run("git", ["-C", dir, ...args], { timeout: 60_000, maxBuffer: 64 << 20, ...opts });

/**
 * **一个仓里哪些目录算技能**：带 `SKILL.md` 的那些（去掉点开头的、去掉 prefix 之外的）。
 *
 * 这一个判据是全流程的枢纽 —— 拉什么、摆什么、取什么，都从它出去：
 *   · 工作区只 check out 这些目录（`syncWorktree`）
 *   · 抽屉里那棵树只有这些目录（`skillFiles`）
 *   · 取用的单位是这里的一条（`listEntries`）
 *
 * 早先这里没有它，`ls-tree -r` 一把全 checkout —— 于是 `.github/`、`.changeset/`、
 * `docs/`、根 README 全都躺到了我们盘上。那些字跟我们一点关系都没有。
 */
const skillHomes = (rows, prefix) => {
  const homes = new Set();
  for (const row of rows) {
    if (!row.path.endsWith("/SKILL.md")) continue;
    const home = row.path.slice(0, -"/SKILL.md".length);
    if (!home) continue; // 仓库根上放一个 SKILL.md = 整个仓是一条技能，这种形状不认
    if (home.split("/").some((part) => part.startsWith("."))) continue; // .archive/ .github/ …
    if (prefix && !(home === prefix || home.startsWith(`${prefix}/`))) continue;
    homes.add(home);
  }
  return [...homes].sort();
};

/**
 * 工作区只放出技能目录。**不是整仓。**
 *
 * `--no-cone` 是必须的：cone 模式会把技能目录的父目录整层也放出来（`skills/engineering/`
 * 里什么文件都会下来）；no-cone 能精确到 `/a/b/c/` 这一枝。
 */
const syncWorktree = async ({ dir, source, run }) => {
  const head = (await git(run, dir, ["rev-parse", "HEAD"])).stdout.trim();
  const rows = await treeOf(run, dir, head);
  const homes = skillHomes(rows, source.prefix ?? "");
  // 一条技能都没有：给一个不存在的 pattern，别让 `set` 空参数退化成「全放出来」。
  const patterns = homes.length ? homes.map((home) => `/${home}/`) : ["/.nous-no-skills/"];
  await git(run, dir, ["sparse-checkout", "set", "--no-cone", ...patterns]);
  await git(run, dir, ["checkout", "--quiet", "--force", "HEAD"]);
  return homes;
};

/**
 * 有就用了，没有就 clone 一份**只要技能**的工作区。
 *
 * 三道闸一起上，缺一道整仓就下来了：
 *   · `--depth=1`  只要最新那棵 tree，不要 553 个 commit 的历史
 *   · `--filter=blob:none`  blob 按需（读某个文件时才拉它）
 *   · `--no-checkout` + sparse-checkout  工作区只放技能目录
 *
 * 没有「失败了退回全量」这条路：退化成全量 clone 正是当初把整仓拉下来的入口，
 * 「省空间失败」的正确答案是报错让人知道，不是偷偷下 184 个文件。
 *
 * 老的 bare 镜像（根上有 `HEAD`、没有 `.git`）结构不一样，直接删了重建 —— 它本来
 * 就是个缓存，重建只花一次 clone。
 */
export const ensureMirror = async ({ data, source, run }) => {
  const dir = mirrorPath(data, source.name);
  if (await exists(path.join(dir, ".git"))) return dir;
  await mkdir(path.dirname(dir), { recursive: true });
  await rm(dir, { recursive: true, force: true }).catch(() => {});
  try {
    await run(
      "git",
      ["clone", "--quiet", "--depth=1", "--filter=blob:none", "--no-checkout", source.url, dir],
      { timeout: 180_000, maxBuffer: 16 << 20 },
    );
    await syncWorktree({ dir, source, run });
    // tag 表是「版本号」的来源，clone 时顺手拉一次（反正已经联网了）。
    await refreshTags({ dir, source, run }).catch(() => {});
  } catch (error) {
    await rm(dir, { recursive: true, force: true }).catch(() => {});
    throw withStatus(new Error(`${source.name}: clone 失败 —— ${errorLine(error)}`), 400);
  }
  return dir;
};

/** 唯一的联网动作。调用点是 `POST /v1/skills/upstream/refresh`，别的地方不许调。 */
export const fetchMirror = async ({ data, source, run }) => {
  const dir = await ensureMirror({ data, source, run });
  const branch = (await git(run, dir, ["rev-parse", "--abbrev-ref", "HEAD"])).stdout.trim() || "main";
  try {
    await git(
      run,
      dir,
      ["fetch", "--quiet", "--depth=1", "--filter=blob:none", "--prune", "origin", "+refs/heads/*:refs/remotes/origin/*"],
      { timeout: 180_000 },
    );
    await git(run, dir, ["reset", "--hard", "--quiet", `origin/${branch}`]);
  } catch (error) {
    throw withStatus(new Error(`${source.name}: fetch 失败 —— ${errorLine(error)}`), 400);
  }
  await refreshTags({ dir, source, run }).catch(() => {});
  await syncWorktree({ dir, source, run });
  // 上游删掉的技能，工作区里也得跟着消失（`reset --hard` 只按旧 sparse 规则收拾过一遍）。
  await git(run, dir, ["clean", "-fdq"]).catch(() => {});
  // 每刷一次都留一份 reflog + 松散对象，几十次之后磁盘比技能本身还大。
  // 这是缓存，历史在这儿没有用 —— best-effort 收一下，失败了不算刷新失败。
  await git(run, dir, ["reflog", "expire", "--expire=now", "--all"]).catch(() => {});
  await git(run, dir, ["gc", "--quiet", "--prune=now"]).catch(() => {});
  return dir;
};

/** 本机那个**能 `cd` 进去看**的目录：复制地址给的就是它（不是 GitHub URL，也不是裸仓）。 */
export const localPath = (data, source) =>
  source.prefix ? path.join(mirrorPath(data, source.name), ...source.prefix.split("/")) : mirrorPath(data, source.name);

/** 镜像在不在。`GET /v1/skills/upstream` 只读已有的 —— 打开面板不该去打 GitHub。 */
export const hasMirror = async ({ data, source }) => exists(path.join(mirrorPath(data, source.name), ".git", "HEAD"));

/**
 * 镜像占了多少地方。**这不是「我们在本机又存了一份内容」**，是缓存 ——
 * 一个 `--filter=blob:none` 的裸仓，删掉下次 fetch 从源头重建。
 * 但它是真的站在你盘上，所以面板上得把数摆出来，别让「本机到底存没存」变成一句猜测。
 */
export const mirrorBytes = async ({ data, source }) => {
  const dir = mirrorPath(data, source.name);
  let total = 0;
  const walk = async (at) => {
    const stat_ = await stat(at).catch(() => null);
    if (!stat_) return;
    if (stat_.isFile()) {
      total += stat_.size;
      return;
    }
    if (!stat_.isDirectory()) return;
    for (const name of await readdir(at)) await walk(path.join(at, name));
  };
  await walk(dir);
  return total;
};

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
  for (const home of skillHomes(rows, prefix)) {
    const skillSha = rows.find((row) => row.path === `${home}/SKILL.md`)?.sha;
    if (skillSha) skillDirs.set(home, { home, skillSha });
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

/**
 * 抽屉里那棵树：**只有技能**（`skillHomes` 选中的那些目录，连同目录里的文件）。
 *
 * 早先这里是 `ls-tree -r` 一把全给 —— 于是第一屏是 `.agents/`、`.changeset/`、
 * `.github/`，133 行里 19 行跟技能有关。上游说的是「一个仓 = 一条技能」，
 * 意思是那个仓**能用的部分**是一捆技能，不是那个仓的每一个字节。
 *
 * 路径已经剥掉 `prefix`（`engineering/tdd/SKILL.md`），树根就是技能那一层；
 * 分组目录（`engineering/`）留着 —— 那是技能在仓里的位置，平铺会把位置丢掉。
 */
export const repoFiles = async ({ data, source, run, maxFiles = 2000 }) => {
  const head = await resolveHead({ data, source, run });
  const dir = mirrorPath(data, source.name);
  const rows = await treeOf(run, dir, head, { long: true });
  const homes = skillHomes(rows, source.prefix ?? "");
  const skip = source.prefix ? `${source.prefix}/` : "";
  const inSkill = (filePath) => homes.some((home) => filePath.startsWith(`${home}/`));
  const files = rows
    .filter((row) => row.type === "blob" && inSkill(row.path))
    .map((row) => ({
      path: skip && row.path.startsWith(skip) ? row.path.slice(skip.length) : row.path,
      size: row.size ?? 0,
    }))
    .sort((a, b) => a.path.localeCompare(b.path));
  if (files.length > maxFiles) {
    throw withStatus(new Error(`${source.name}: 技能里有 ${files.length} 个文件，超过 ${maxFiles} —— 面板里不展开`), 400);
  }
  return { head, files };
};

/** 一条上游技能里的文件清单（对照 + 取用前的核对都吃它）。 */
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

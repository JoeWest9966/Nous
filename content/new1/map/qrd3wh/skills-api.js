// `/v1/skills*` —— 技能仓库的 HTTP 面。
//
// 从 `server.js` 拆出来的理由不是「太长了」，是**测试要能不启动服务**：
// 这个文件只有纯函数和 http 的 req/res，`skills` 和仓库工具都是递进来的。
// 于是测试能注入一个假 `skills` + 一个临时 git 仓，把每条路走一遍。
//
// 一条铁律：**仓库是唯一真相。** 这里不维护第二份名单、不读 `data/`、不联网、
// 不「安装」。盘上有这个目录 = 装着；HEAD 里有这个目录 = 已发布。
//
// 约定（spec §6）：
//   · 统一错误信封 `{ error }` + 状态码：400 校验 / 409 冲突 / 404 不在 / 500 内部。
//   · 每个写端点都在**本仓的**串行锁里（`serializeRepoWrite(root, …)`），写完 `skills.load()`。
//     不 load 的话面板显示新技能、会话里还看不见它 —— 两份真相就是这么长出来的。
//   · `publish` = 内容闸 + 重扫 index.json + commit + push，一步；闸不过不动 index.json。
//   · 不实现 install / uninstall：技能不是「装进来的包」。

import { copyFile, mkdir, readFile, readdir, rename, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";

import * as upstream from "./upstream.js";

// ── HTTP 小工具 ─────────────────────────────────────────────────────────────

const withStatus = (error, status) => Object.assign(error, { status });

const send = (res, status, body) => {
  const text = JSON.stringify(body);
  res.writeHead(status, { "content-type": "application/json; charset=utf-8", "content-length": Buffer.byteLength(text) });
  res.end(text);
};

const readBody = (req) =>
  new Promise((resolve, reject) => {
    let raw = "";
    req.on("data", (chunk) => {
      raw += chunk;
      if (raw.length > 1_000_000) reject(new Error("body too large"));
    });
    req.on("end", () => resolve(raw));
    req.on("error", reject);
  });

const parseJsonBody = async (req) => {
  const raw = (await readBody(req)) || "{}";
  try {
    const value = JSON.parse(raw);
    return value && typeof value === "object" ? value : {};
  } catch {
    throw withStatus(new Error("请求体不是合法 JSON"), 400);
  }
};

const errorLine = (error) => String(error?.message ?? error).split("\n")[0].slice(0, 400);
const fail = (res, error) => send(res, error?.status ?? 500, { error: errorLine(error) });

// ── 技能的名字 / 命名规矩 ───────────────────────────────────────────────────

/** 小写、≤64 字：名字就是目录名、就是会话里的键（I3），新建那一刻定型，不许改。 */
const SKILL_NAME_RE = /^[a-z0-9][a-z0-9._-]*$/;
const MAX_NAME = 64;

/** 仓库根下不是技能的名字（撞上它们 = 把 index.json 当技能编辑）。 */
const RESERVED = new Set([
  "categories.json",
  "index.json",
  "index.mjs",
  "nous.json",
  "SKILL.md",
  "README.md",
  "sources.json",
  "AGENTS.md",
  "test",
  "node_modules",
]);

const checkName = (raw) => {
  const name = String(raw ?? "").trim();
  if (!name) throw withStatus(new Error("技能名不能为空"), 400);
  if (name.length > MAX_NAME) throw withStatus(new Error(`技能名太长（最多 ${MAX_NAME} 字）：${name}`), 400);
  if (!SKILL_NAME_RE.test(name)) {
    throw withStatus(new Error(`技能名只认小写字母、数字、点、下划线、连字符，且以字母或数字开头：${name}`), 400);
  }
  if (RESERVED.has(name)) throw withStatus(new Error(`「${name}」是仓库自己的文件名，不能当技能名`), 400);
  return name;
};

/** 变体 id：跟技能名同一套字符规矩，但**不**是目录名 —— 它只参与 `本名@id` 这个键。 */
const MAX_VARIANT_ID = 32;

const checkVariantId = (raw) => {
  const id = String(raw ?? "").trim();
  if (!id) throw withStatus(new Error("变体 id 不能为空"), 400);
  if (id.length > MAX_VARIANT_ID) throw withStatus(new Error(`变体 id 太长（最多 ${MAX_VARIANT_ID} 字）：${id}`), 400);
  if (!SKILL_NAME_RE.test(id)) {
    throw withStatus(new Error(`变体 id 只认小写字母、数字、点、下划线、连字符，且以字母或数字开头：${id}`), 400);
  }
  if (RESERVED.has(id)) throw withStatus(new Error(`「${id}」是仓库自己的文件名，不能当变体 id`), 400);
  return id;
};

/**
 * 分组（`deprecated` / `engineering`）：只用来放技能的抽屉。
 * 最多一层、每段跟技能名同一套字符规矩 —— 它也是目录名，也得能当路径用。
 */
const checkGroup = (raw) => {
  const group = String(raw ?? "").trim().replace(/^\/+|\/+$/g, "");
  if (!group) return "";
  const segments = group.split("/");
  if (segments.length > 1) throw withStatus(new Error(`分组只能一层：${group} —— 「deprecated」那种，不是多层目录`), 400);
  for (const segment of segments) {
    if (!SKILL_NAME_RE.test(segment)) {
      throw withStatus(new Error(`分组名只认小写字母、数字、点、下划线、连字符，且以字母或数字开头：${segment}`), 400);
    }
    if (segment.length > MAX_NAME) throw withStatus(new Error(`分组名太长（最多 ${MAX_NAME} 字）：${segment}`), 400);
    if (RESERVED.has(segment)) throw withStatus(new Error(`「${segment}」是仓库自己的名字，不能当分组`), 400);
  }
  return segments.join("/");
};

// ── 名字 → 目录 ─────────────────────────────────────────────────────────────
//
// **名字是会话键、是 API 参数；住哪儿是分组的事。** 所有读写都先在这里翻一次 ——
// 别处一律不许自己拼 `path.join(root, name)`：拼了，分组里的技能就全部 404。

/** 这个技能在仓里的相对目录（`demo` / `deprecated/demo`）。名册里没有就按顶层算。 */
const relOf = (skills, name) => {
  let roster = [];
  try {
    roster = skills?.list?.() ?? [];
  } catch {
    roster = [];
  }
  const hit = roster.find((skill) => skill.name === name);
  return typeof hit?.path === "string" && hit.path ? hit.path : String(name);
};

/** 同上，给 `repo-tools` 那套要 `dir` 的函数用。
 *
 * **变体就靠这一行撑起来**：名册里 `tdd@strict` 那一条的 `dir` 是它自己那棵树里的目录（不在 root 底下），
 * 所以读写一律以名册的 `dir` 为准 —— 拼 `root + path` 的话，变体的每一份文件都会摸到本体的盘上。
 */
const homeOf = (root, skills, name) => {
  let roster = [];
  try {
    roster = skills?.list?.() ?? [];
  } catch {
    roster = [];
  }
  const hit = roster.find((skill) => skill.name === name);
  if (typeof hit?.dir === "string" && hit.dir) return hit.dir;
  return path.join(root, relOf(skills, name));
};

/**
 * 变体是按 **`name` + `variant` 两个参数**走的，不是拼成 `tdd@strict` 再拆开 ——
 * 跟扩展那边一笔不差：`repoDir()` 认的是目录名（`tdd`），`@strict` 那一半只是
 * 「哪一棵树」。拼成一个键去解析，等于让路径那一层去理解会话键，两头都脏。
 */
const VARIANT_SEP = "@";

/** `?variant=strict` → `"strict"`；空 / 不给 = 本体。 */
const variantParam = (raw) => {
  const id = String(raw ?? "").trim();
  return id ? checkVariantId(id) : null;
};

/** `tdd@strict` → `{family:"tdd", id:"strict"}`；没有 `@` 就是本体。只给 diff 那条老路径用。 */
const splitKey = (raw) => {
  const text = String(raw ?? "").trim();
  const at = text.indexOf(VARIANT_SEP);
  return at <= 0 ? { family: text, id: null } : { family: text.slice(0, at), id: text.slice(at + 1) };
};

/**
 * 一条技能住在哪儿：本体是 `<root>/<path>`，变体是它自己那棵树里的 `<family>/`。
 * 名册里有就信名册（`dir` 是装载器扫出来的真相），没有就按绑定的树根拼。
 */
const placeOf = async (root, skills, variants, name, variant) => {
  if (!variant) return homeOf(root, skills, name);
  const base = await variants?.rootOf(name, variant);
  if (!base) throw withStatus(new Error(`${name}${VARIANT_SEP}${variant} 没绑 —— 先绑一个变体`), 404);
  const hit = (skills?.list?.() ?? []).find((skill) => skill.name === `${name}${VARIANT_SEP}${variant}`);
  return typeof hit?.dir === "string" && hit.dir ? hit.dir : path.join(base, name);
};

/** 写完之后让内存名册跟上：变体那一条也要重新扫（键是 `name@variant`）。 */
const reloadKey = async (skills, name, variant) =>
  reloadRoster(skills).then(() => (variant ? (skills?.list?.() ?? []).some((skill) => skill.name === `${name}${VARIANT_SEP}${variant}`) : true));

// ── frontmatter：借扩展仓那一份（spec §2.1「解析器用现成的 parseFrontmatter()」）──
//
// **为什么不在这儿手写第二份**：本地那版只认 `key: value` 两个标量，`description: >`
// 这种折叠块会被读成一个 `>`（内容闸看见「非空」就放过去了，扩展那边却把这段折成
// 空串），引号里的 `\"` 也不解转义 —— 于是同一份文件，闸说能发、名册说是坏账。
// `>` / `|` 折叠、去壳解转义，现成那份都支持，那就用它。
//
// 来源跟 `server.js` 顶部那几行动态 import 是同一个（`NOUS_EXTEND`）；同一个模块已经
// 在进程里加载过，这次 import 不会多加载一份。
const EXTEND = process.env.NOUS_EXTEND ?? "/opt/Nous-extend";
const { parseFrontmatter } = await import(`${EXTEND}/skills/index.js`);

export { parseFrontmatter };

/** 内容闸：一条技能对不对。`rel` 是它在仓里的相对目录（`demo` / `deprecated/demo`）。 */
export const checkSkillContent = async (root, rel, dir = null) => {
  const home = dir ?? path.join(root, rel);
  // 名字比的是**最后一段**：分组只是抽屉，不进名字（`deprecated/demo` 的技能还叫 `demo`）。
  const leaf = path.posix.basename(String(rel).replace(/\\/g, "/"));
  const problems = [];
  let text = null;
  try {
    text = await readFile(path.join(home, "SKILL.md"), "utf8");
  } catch {
    problems.push(`${rel}: 没有 SKILL.md`);
  }
  if (text !== null) {
    const front = parseFrontmatter(text);
    if (!front) problems.push(`${rel}: SKILL.md 开头没有 frontmatter（两行 --- 包起来的那一段）`);
    else {
      const declared = String(front.name ?? "").trim();
      if (declared !== leaf) {
        problems.push(`${rel}: frontmatter 里的 name 是「${declared}」，跟目录名「${leaf}」对不上`);
      }
      if (!String(front.description ?? "").trim()) problems.push(`${rel}: description 是空的`);
    }
  }
  let manifest = null;
  try {
    manifest = JSON.parse(await readFile(path.join(home, "nous.json"), "utf8"));
  } catch {
    problems.push(`${rel}: nous.json 读不出来（不存在或不是合法 JSON）`);
  }
  if (manifest) {
    if (manifest.name !== leaf) problems.push(`${rel}: nous.json 里的 name 是「${manifest.name}」，跟目录名对不上`);
    if (typeof manifest.summary !== "string") problems.push(`${rel}: nous.json 缺 summary（可以写空串，但得有）`);
    if (!Array.isArray(manifest.category)) problems.push(`${rel}: nous.json 的 category 得是数组（没有就写 []）`);
  }
  return problems;
};

/**
 * 仓里所有带 SKILL.md 的目录（相对路径）。分组里的也算 —— 闸得看见它们。
 * 分组最多一层（跟 `Nous-skills/index.mjs` 同一条规矩）：再深的不扫，生成器那边会报。
 */
const skillRels = async (root) => {
  const out = [];
  const walk = async (rel, depth) => {
    const dir = rel ? path.join(root, rel) : root;
    const kids = (await readdir(dir, { withFileTypes: true }).catch(() => []))
      .filter((entry) => entry.isDirectory() && !entry.name.startsWith(".") && entry.name !== "node_modules" && entry.name !== "test")
      .map((entry) => entry.name)
      .sort();
    for (const kid of kids) {
      const kidRel = rel ? `${rel}/${kid}` : kid;
      if (await stat(path.join(dir, kid, "SKILL.md")).catch(() => null)) {
        out.push(kidRel);
        continue;
      }
      if (depth < 1) await walk(kidRel, depth + 1);
    }
  };
  await walk("", 0);
  return out;
};

/** 仓库里所有带 SKILL.md 的目录 —— 内容闸扫这一圈（没有 SKILL.md 的目录静默跳过）。 */
const gateRepo = async (root) => {
  const problems = [];
  for (const rel of await skillRels(root)) problems.push(...(await checkSkillContent(root, rel)));
  return problems;
};

const gateMessage = (problems) => {
  const shown = problems.slice(0, 4).join("；");
  return problems.length > 4 ? `${shown}；等 ${problems.length} 项` : shown;
};

// ── 词表 / 生成器 ───────────────────────────────────────────────────────────

const skillsIndex = (repo, root) => () => repo.regenIndex(root, { required: false });

// 技能仓**没有**受保护的类别（spec §3.6：别造 PROTECTED_CATEGORIES）。
// 词表那套工具默认护着扩展仓的 Basics，所以每个调用点都得显式说「一个都不护」。
const NO_PROTECT = { protect: [] };

// ── 响应里那几块公共拼装 ────────────────────────────────────────────────────

const commitOf = async (repo, root, result) => {
  if (!result.committed) return null;
  const info = await repo.lastCommitInfo(root);
  return { hash: info.hash, subject: info.subject };
};

const writeResult = async (repo, root, result) => ({
  // `ok` 的语义是「这一步做完了」，**不是**「推上去了」：没东西可提交（工作区本来就是
  // 干净的）和推成功都算做完了；只有「提交了但没推成」才是 false（那时另有 `error`）。
  ok: result.committed ? result.pushed !== false : true,
  commit: await commitOf(repo, root, result),
  pushed: result.pushed === true,
  ...(result.pushError ? { error: String(result.pushError).slice(0, 400) } : {}),
});

// index.json 的快照 / 复原（I7：半截不留）。
// 发布 / 归档 / 恢复都是「重扫 → commit → push」三步，第一步就已经改了 index.json；
// 后面任一步翻车，盘上会留一份「索引说改了、git 里没有」的账。所以进 try 之前
// 把它的原文抄下来，失败时原样写回（本来就没有这个文件就删掉）。
const snapshotIndex = async (root) => {
  const file = path.join(root, "index.json");
  return { file, text: await readFile(file, "utf8").catch(() => null) };
};

const restoreIndex = async ({ file, text }) => {
  if (text === null) await rm(file, { force: true }).catch(() => {});
  else await writeFile(file, text, "utf8").catch(() => {});
};

/** 写完盘让内存名册跟上。跟不上的话面板说有、会话里没有。 */
const reloadRoster = async (skills) => {
  try {
    await skills.load();
    return null;
  } catch (error) {
    return errorLine(error);
  }
};

// ── 各端点 ─────────────────────────────────────────────────────────────────

const overview = async (repo, root, skills) => {
  let state = { head: null, dirty: false, ahead: 0, behind: 0 };
  let error = null;
  try {
    state = await repo.repoState(root);
  } catch (caught) {
    // 仓库读不了要给页面一句话，不是 500 —— 白屏说不清「是仓库没了还是技能没了」。
    error = errorLine(caught);
  }
  const [remote, tracked, vocab, archived, roster] = await Promise.all([
    repo.remoteUrl(root),
    repo.trackedPaths(root),
    repo.readCategories(root, NO_PROTECT),
    repo.archivedEntries(root),
    Promise.resolve()
      .then(() => skills.list())
      .catch(() => []),
  ]);
  // 真相是**两份取并集**（spec §3.2）：词表里的（可能有零计数的空类别）
  // + 每条技能自己挂着的。少一半，左栏就会漏掉「还没挂到任何技能上的类别」
  // 或者「挂在技能上但词表没登记（手改 nous.json）的类别」。
  const categories = repo.sortCategories([
    ...vocab,
    ...(roster ?? []).flatMap((skill) => (Array.isArray(skill.category) ? skill.category : [])),
  ]);
  return {
    root,
    repo: { ...state, remote, error },
    // `tracked: false` + 目录在 = 未发布。跟面板上的琥珀虚线是同一个判据。
    // 比的是 `path` 不是 `name`：分组里的技能（`deprecated/demo`）用名字去 HEAD 里找永远找不到。
    skills: (roster ?? []).map((skill) => ({ ...skill, tracked: tracked.has(skill.path ?? skill.name) })),
    archived: archived.map((entry) => entry.name).sort((a, b) => a.localeCompare(b, "zh-CN")),
    categories,
    // 抽屉（`deprecated` 这种只装技能的目录）：新建技能时可以直接选，不用手打。
    groups: await groupsOf(root),
  };
};

/** 仓里现有的分组：顶层目录里**没有** SKILL.md 的那几个（点开头的、生成的都不算）。 */
const groupsOf = async (root) => {
  const entries = await readdir(root, { withFileTypes: true }).catch(() => []);
  const out = [];
  for (const entry of entries) {
    if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
    if (entry.name === "node_modules" || entry.name === "test") continue;
    if (await stat(path.join(root, entry.name, "SKILL.md")).catch(() => null)) continue;
    out.push(entry.name);
  }
  return out.sort((a, b) => a.localeCompare(b, "zh-CN"));
};

const createSkill = async (repo, root, body) => {
  const name = checkName(body.name);
  const group = checkGroup(body.group);
  // 分组只是抽屉，名字还是最后一段：`deprecated/demo` 的技能叫 `demo`。
  const rel = group ? `${group}/${name}` : name;
  const description = String(body.description ?? "").trim();
  if (!description) throw withStatus(new Error(`${name}: description 不能空 —— 模型靠它判断该不该用这条技能`), 400);
  const categories = repo.sortCategories(repo.normalizeCategories(body.category ?? []));

  if (group && (await stat(path.join(root, group, "SKILL.md")).catch(() => null))) {
    throw withStatus(new Error(`「${group}」已经是一条技能了 —— 技能里面不能再套技能`), 400);
  }
  if (await stat(path.join(root, rel)).catch(() => null)) {
    throw withStatus(new Error(`${rel}/ 已经在了 —— 换个名字，或者直接去改它`), 400);
  }
  if (await stat(path.join(root, repo.ARCHIVE_DIR, name)).catch(() => null)) {
    throw withStatus(new Error(`${name}/ 在 .archive/ 里 —— 点「恢复」把它捡回来，别建第二个`), 400);
  }

  const beforeVocab = await repo.readCategories(root, NO_PROTECT);
  // 点开头的目录扫盘器看不见 —— 校验过了才改名进去，中途翻车就整棵删掉（I7：半截不留）。
  const staging = path.join(root, `.new-${name}-${Date.now()}`);
  await mkdir(staging, { recursive: false });
  let landed = false;
  try {
    const front = ["---", `name: ${name}`, `description: ${JSON.stringify(description)}`, "---", "", `# ${name}`, ""].join("\n");
    await writeFile(path.join(staging, "SKILL.md"), front, "utf8");
    await writeFile(
      path.join(staging, "nous.json"),
      JSON.stringify({ name, summary: "", category: categories }, null, 2) + "\n",
      "utf8",
    );

    // 先自己过一遍闸：一个建出来就装载失败的目录，等于让人工去猜哪里写错了。
    // 闸比的是**最后一段**目录名 —— 这里传 rel，它自己拆。
    const problems = await checkSkillContent(root, rel, staging);
    if (problems.length) throw withStatus(new Error(gateMessage(problems)), 400);

    if (categories.length) await repo.writeCategories(root, [...beforeVocab, ...categories], NO_PROTECT);
    // 分组目录（`deprecated/`）可能还不存在：建之前先把它造出来。
    if (group) await mkdir(path.join(root, group), { recursive: true });
    await rename(staging, path.join(root, rel));
    landed = true;
    await repo.regenIndex(root, { required: false });
  } catch (error) {
    if (landed) await rm(path.join(root, rel), { recursive: true, force: true }).catch(() => {});
    await rm(staging, { recursive: true, force: true }).catch(() => {});
    if (categories.length) await repo.writeCategories(root, beforeVocab, NO_PROTECT).catch(() => {});
    throw error;
  }
  return { ok: true, name, path: rel };
};

// ── 上游 ───────────────────────────────────────────────────────────────────
//
// 「上游就是一个 git remote」。这里只加三样东西：订阅清单的读写、镜像的清单/对照、
// 以及取用（adopt）。**没有自动同步、没有定时 fetch、没有双向推送** ——
// 打开面板只读已有镜像，联网只发生在 `upstream/refresh`。

/** `nous.json.upstream` 那个注脚：`<source>/<path>@<tree-sha>`。只写不读逻辑。 */
const upstreamFoot = (source, home, sha) => `${source}/${home}@${sha ?? "unknown"}`;

const footOf = async (root, rel) => {
  try {
    const manifest = JSON.parse(await readFile(path.join(root, rel, "nous.json"), "utf8"));
    return typeof manifest?.upstream === "string" ? manifest.upstream : null;
  } catch {
    return null;
  }
};

/**
 * 本地这条跟上一条什么关系。四种：
 *   none    本地没有这条 → 可以取用
 *   same    取用过，且上游没变
 *   changed 取用过，上游又改了 → 面板上「上游有更新」
 *   taken   本地有同名，但不是从这条取的（名字占了）→ 只能改名取用
 */
const localStateOf = (roster, foot, entry) => {
  const mine = roster.find((skill) => skill.name === entry.name);
  if (!mine) return "none";
  if (!foot) return "taken";
  const prefix = `${entry.source}/${entry.path}@`;
  if (!foot.startsWith(prefix)) return "taken";
  return foot.endsWith(`@${entry.sha}`) ? "same" : "changed";
};

const readManifestOf = (dir) =>
  readFile(path.join(dir, "nous.json"), "utf8")
    .then((text) => JSON.parse(text))
    .catch(() => null);

/** 只改 frontmatter 里那一行 name；正文一个字都不碰。 */
const renameInFrontmatter = (text, name) => {
  const match = /^(\uFEFF?---[ \t]*\r?\n)([\s\S]*?)(\r?\n---)/.exec(text);
  if (!match) return text; // 没有 frontmatter：闸会报，别在这儿编一个
  const body = /^name:.*$/m.test(match[2]) ? match[2].replace(/^name:.*$/m, `name: ${name}`) : `name: ${name}\n${match[2]}`;
  return match[1] + body + match[3] + text.slice(match[0].length);
};

/**
 * 取用：把上游一条技能摊进点开头的暂存目录，补齐/改名 nous.json，过闸，才 rename。
 *
 * 这跟 `createSkill` 是同一个形状（I7 半截不留），区别只有一个：骨架不是空的，
 * 是从 `git cat-file` 里摊出来的。**仍然不提交** —— 第一个正确状态是「未发布」。
 */
const adoptSkill = async (repo, root, data, body) => {
  const sourceName = String(body.source ?? "").trim();
  const home = String(body.path ?? "").trim().replace(/^\/+|\/+$/g, "");
  if (!sourceName) throw withStatus(new Error("没说从哪个上游取（source 是空的）"), 400);
  if (!home) throw withStatus(new Error("没说取哪一条（path 是空的）"), 400);

  const { sources } = await upstream.readSources(root);
  const source = sources.find((item) => item.name === sourceName);
  if (!source) throw withStatus(new Error(`没有订阅「${sourceName}」这个上游`), 400);
  if (!(await upstream.hasMirror({ data, source }))) {
    throw withStatus(new Error(`${sourceName}: 还没拉过镜像 —— 先点刷新（POST /v1/skills/upstream/refresh）`), 400);
  }

  const name = checkName(body.name ?? path.posix.basename(home));
  // 取用也能直接放进一个分组（`deprecated`）；不给就是顶层。
  const group = checkGroup(body.group);
  const rel = group ? `${group}/${name}` : name;
  if (group && (await stat(path.join(root, group, "SKILL.md")).catch(() => null))) {
    throw withStatus(new Error(`「${group}」已经是一条技能了 —— 技能里面不能再套技能`), 400);
  }
  if (await stat(path.join(root, rel)).catch(() => null)) {
    throw withStatus(new Error(`${rel}/ 已经在了 —— 想换成上游这版就改个名字，或者先归档本地这条`), 409);
  }
  if (await stat(path.join(root, repo.ARCHIVE_DIR, name)).catch(() => null)) {
    throw withStatus(new Error(`${name}/ 在 .archive/ 里 —— 先恢复或彻底删掉，别建第二个`), 409);
  }

  const beforeVocab = await repo.readCategories(root, NO_PROTECT);
  const staging = path.join(root, `.new-${name}-${Date.now()}`);
  await mkdir(staging, { recursive: false });
  let landed = false;
  let fileCount = 0;
  try {
    const { head, files } = await upstream.materialize({ data, source, run: repo.run, home, target: staging });
    fileCount = files.length;
    if (!files.includes("SKILL.md")) throw withStatus(new Error(`${home}: 上游这一条里没有 SKILL.md`), 400);

    // 名字对齐：目录名、frontmatter 的 name、nous.json 的 name 必须是同一个（I3）。
    const skillFile = path.join(staging, "SKILL.md");
    const text = await readFile(skillFile, "utf8");
    const renamed = renameInFrontmatter(text, name);
    if (renamed !== text) await writeFile(skillFile, renamed, "utf8");

    const manifest = (await readManifestOf(staging)) ?? {};
    const categories = repo.sortCategories(
      repo.normalizeCategories([
        ...(Array.isArray(manifest.category) ? manifest.category : []),
        ...(Array.isArray(body.category) ? body.category : []),
      ]),
    );
    const entry = (await upstream.listEntries({ data, source, run: repo.run })).find((item) => item.path === home);
    await writeFile(
      path.join(staging, "nous.json"),
      JSON.stringify(
        {
          name,
          summary: typeof manifest.summary === "string" ? manifest.summary : "",
          category: categories,
          upstream: upstreamFoot(source.name, home, entry?.sha ?? head),
        },
        null,
        2,
      ) + "\n",
      "utf8",
    );

    // 闸在这个位置跑：骨架是别人的，名字是我们改的 —— 两件事都可能出问题，一次说清。
    const problems = await checkSkillContent(root, rel, staging);
    if (problems.length) throw withStatus(new Error(gateMessage(problems)), 400);

    if (categories.length) await repo.writeCategories(root, [...beforeVocab, ...categories], NO_PROTECT);
    if (group) await mkdir(path.join(root, group), { recursive: true });
    await rename(staging, path.join(root, rel));
    landed = true;
    await repo.regenIndex(root, { required: false });
  } catch (error) {
    if (landed) await rm(path.join(root, rel), { recursive: true, force: true }).catch(() => {});
    await rm(staging, { recursive: true, force: true }).catch(() => {});
    throw error;
  }
  return { ok: true, name, path: rel, fileCount };
};

/**
 * 整目录拷一份。`.git` / `node_modules` 不拷 —— 那是仓的，不是技能的。
 * 用 `copyFile` 一文件一文件拷，不 `cp -r` 那样的黑盒子：翻车时知道拷到第几个。
 */
const copyTree = async (from, to, rel = "") => {
  let count = 0;
  const entries = await readdir(from, { withFileTypes: true });
  for (const entry of entries) {
    if (entry.name === ".git" || entry.name === "node_modules") continue;
    const src = path.join(from, entry.name);
    const dst = path.join(to, entry.name);
    if (entry.isDirectory()) {
      await mkdir(dst, { recursive: true });
      count += await copyTree(src, dst, rel ? `${rel}/${entry.name}` : entry.name);
    } else if (entry.isFile()) {
      await copyFile(src, dst);
      count++;
    }
  }
  return count;
};

/**
 * 复制：把仓里**已有的**一条技能整目录拷成另一条。
 *
 * 跟 `createSkill` / `adoptSkill` 同一副骨头 —— 点开头的暂存目录、过闸、才 rename，
 * 翻车整棵删掉（I7 半截不留）。区别只在骨架从哪儿来：不是空的，也不是别人的镜像，
 * 是自家隔壁那条。
 *
 * 三件事不因为是复制就松：新名字照样过 `checkName` + 撞目录 / 撞 `.archive/`；
 * `SKILL.md` 的 frontmatter 和 `nous.json` 的 name 一起改成新名字（I3）；**不提交** ——
 * 复制出来就是「未发布」，发布仍走 `/publish`。
 *
 * 三个注脚不搬过来：
 *   · `upstream`（取用是一次性拷贝，复制品的来源是隔壁那条，不是那个上游）；
 *   · `archiveOrigin`（归档态才写的字段，复制品从来没归档过）；
 *   · `variant` 以外的任何东西都是照抄的 —— 正文、附件、summary、category 原样跟着走。
 *
 * `variant` 给了 = 顺手标成「某条的变体」：`nous.json` 多一格 `variant: { family, id }`。
 * `family` 永远是**源那条**的名字 —— 变体不能改姓。
 */
const duplicateSkill = async (repo, root, skills, body) => {
  const from = checkName(body.from);
  const fromRel = relOf(skills, from);
  if (!(await stat(path.join(root, fromRel, "SKILL.md")).catch(() => null))) {
    throw withStatus(new Error(`仓里没有「${from}」这一条 —— 复制谁？`), 404);
  }

  const name = checkName(body.name);
  const group = checkGroup(body.group);
  const rel = group ? `${group}/${name}` : name;
  const fromGroup = path.posix.dirname(fromRel).replace(/^\.$/, "");
  if (name === from && group === fromGroup) {
    throw withStatus(new Error(`${from} 复制到它自己身上了 —— 换个名字`), 400);
  }
  if (group && (await stat(path.join(root, group, "SKILL.md")).catch(() => null))) {
    throw withStatus(new Error(`「${group}」已经是一条技能了 —— 技能里面不能再套技能`), 400);
  }
  if (await stat(path.join(root, rel)).catch(() => null)) {
    throw withStatus(new Error(`${rel}/ 已经在了 —— 换个名字`), 409);
  }
  if (await stat(path.join(root, repo.ARCHIVE_DIR, name)).catch(() => null)) {
    throw withStatus(new Error(`${name}/ 在 .archive/ 里 —— 先恢复或彻底删掉，别建第二个`), 409);
  }

  const variant =
    body.variant === undefined || body.variant === null || String(body.variant).trim() === ""
      ? null
      : { family: from, id: checkVariantId(body.variant) };

  const beforeVocab = await repo.readCategories(root, NO_PROTECT);
  const staging = path.join(root, `.new-${name}-${Date.now()}`);
  await mkdir(staging, { recursive: false });
  let landed = false;
  let fileCount = 0;
  try {
    fileCount = await copyTree(path.join(root, fromRel), staging);

    // 名字对齐：目录名、frontmatter 的 name、nous.json 的 name 必须是同一个（I3）。
    const skillFile = path.join(staging, "SKILL.md");
    const text = await readFile(skillFile, "utf8");
    const renamed = renameInFrontmatter(text, name);
    if (renamed !== text) await writeFile(skillFile, renamed, "utf8");

    const sourceManifest = (await readManifestOf(path.join(root, fromRel))) ?? {};
    const kept = { ...sourceManifest };
    delete kept.upstream;
    delete kept.archiveOrigin;
    // 源自己就是个变体的话，`variant` 也得先摘掉 —— 搬过来的那格写着别人的 family。
    delete kept.variant;
    const categories = repo.sortCategories(
      repo.normalizeCategories([
        ...(Array.isArray(kept.category) ? kept.category : []),
        ...(Array.isArray(body.category) ? body.category : []),
      ]),
    );
    await writeFile(
      path.join(staging, "nous.json"),
      JSON.stringify(
        {
          ...kept,
          name,
          summary: typeof kept.summary === "string" ? kept.summary : "",
          category: categories,
          ...(variant ? { variant } : {}),
        },
        null,
        2,
      ) + "\n",
      "utf8",
    );

    const problems = await checkSkillContent(root, rel, staging);
    if (problems.length) throw withStatus(new Error(gateMessage(problems)), 400);

    if (categories.length) await repo.writeCategories(root, [...beforeVocab, ...categories], NO_PROTECT);
    if (group) await mkdir(path.join(root, group), { recursive: true });
    await rename(staging, path.join(root, rel));
    landed = true;
    await repo.regenIndex(root, { required: false });
  } catch (error) {
    if (landed) await rm(path.join(root, rel), { recursive: true, force: true }).catch(() => {});
    await rm(staging, { recursive: true, force: true }).catch(() => {});
    if (beforeVocab.length) await repo.writeCategories(root, beforeVocab, NO_PROTECT).catch(() => {});
    throw error;
  }
  return { ok: true, name, path: rel, from, variant, fileCount };
};

/**
 * 归档时把**原址**记进 nous.json 的 `archiveOrigin`。分组里的技能（`deprecated/demo`）
 * 归档后躺在扁的 `.archive/demo/` 里，恢复时得回到那个分组 —— 不记就永远回不去了。
 * 不另建一张表：这张表迟早会跟目录对不上（跟 index.json 一样的道理，但它没法重扫）。
 */
const stampOrigin = async (home, rel) => {
  const file = path.join(home, "nous.json");
  try {
    const manifest = JSON.parse(await readFile(file, "utf8"));
    await writeFile(file, JSON.stringify({ ...manifest, archiveOrigin: rel }, null, 2) + "\n", "utf8");
  } catch {
    // 没有 / 读不出来的 nous.json：归档照样做，恢复时落回顶层（它本来就过不了闸）
  }
};

/** 读回原址，顺手把 `archiveOrigin` 从 nous.json 里摘掉（那是归档态的字段）。 */
const takeOrigin = async (home) => {
  const file = path.join(home, "nous.json");
  try {
    const manifest = JSON.parse(await readFile(file, "utf8"));
    const rel = typeof manifest.archiveOrigin === "string" ? manifest.archiveOrigin.trim() : "";
    if (rel) {
      delete manifest.archiveOrigin;
      await writeFile(file, JSON.stringify(manifest, null, 2) + "\n", "utf8");
    }
    return rel;
  } catch {
    return "";
  }
};

const archiveSkill = async (repo, root, name, skills) => {
  const rel = relOf(skills, name);
  if (!(await stat(path.join(root, rel)).catch(() => null))) {
    throw withStatus(new Error(`工作区里没有 ${name}/`), 404);
  }
  const snapshot = await snapshotIndex(root);
  try {
    // 未发布的也搬进 .archive/：那是「删掉的东西不真删」（I10），只有 purge 才真没。
    await repo.moveIntoArchive(root, rel);
    await stampOrigin(path.join(root, repo.ARCHIVE_DIR, name), rel);
    await repo.regenIndex(root, { required: false });
    return await repo.syncRepo(root, `skill: 归档 ${name}`, { includeDeleted: true, label: "技能", tag: "skill" });
  } catch (error) {
    await repo.run("git", ["-C", root, "reset", "-q", "--", rel, `${repo.ARCHIVE_DIR}/${name}`]).catch(() => {});
    await repo.moveDir(path.join(root, repo.ARCHIVE_DIR, name), path.join(root, rel)).catch(() => {});
    await restoreIndex(snapshot);
    throw error.status ? error : withStatus(new Error(`归档失败，已回滚：${errorLine(error)}`), 400);
  }
};

const restoreSkill = async (repo, root, name) => {
  const home = path.join(root, repo.ARCHIVE_DIR, name);
  if (!(await stat(home).catch(() => null))) throw withStatus(new Error(`归档区里没有 ${name}`), 404);
  // 回原来的分组（归档时记下的）；没记过就落回顶层。
  const to = (await takeOrigin(home)) || name;
  const snapshot = await snapshotIndex(root);
  try {
    await repo.moveOutOfArchive(root, name, { to });
    await repo.regenIndex(root, { required: false });
    return await repo.syncRepo(root, `skill: 从归档恢复 ${name}`, { includeDeleted: true, label: "技能", tag: "skill" });
  } catch (error) {
    await repo.run("git", ["-C", root, "reset", "-q", "--", to, `${repo.ARCHIVE_DIR}/${name}`]).catch(() => {});
    await repo.moveDir(path.join(root, to), path.join(root, repo.ARCHIVE_DIR, name)).catch(() => {});
    if (to !== name) await stampOrigin(path.join(root, repo.ARCHIVE_DIR, name), to).catch(() => {});
    await restoreIndex(snapshot);
    throw error.status ? error : withStatus(new Error(`恢复失败，已回滚：${errorLine(error)}`), 400);
  }
};

const purgeSkill = async (repo, root, name) => {
  const rel = `${repo.ARCHIVE_DIR}/${name}`;
  const source = path.join(root, rel);
  const exists = await stat(source).catch(() => null);
  // 上一次彻底删除提交/推送挂了：文件已经删了、删除还留在暂存区 —— 那一次要收尾，
  // 而不是 404 把人卡在一个半截状态里。
  const halfDone =
    !exists && (await repo.run("git", ["-C", root, "status", "--porcelain", "--", rel]).catch(() => ({ stdout: "" }))).stdout.trim().length > 0;
  if (!exists && !halfDone) throw withStatus(new Error(`归档区里没有 ${name}`), 404);
  const tracked = await repo.isTrackedDir(root, rel);
  try {
    if (exists && tracked) await repo.run("git", ["-C", root, "rm", "-r", "-f", "--", rel], { timeout: 30_000 });
    else if (exists) await rm(source, { recursive: true, force: true });
    return await repo.syncRepo(root, `skill: 彻底删除 ${name}`, { includeDeleted: true, label: "技能", tag: "skill" });
  } catch (error) {
    // 提交/推送挂了：文件已经没了，但 HEAD 里还有 —— 拿回来，别留半截的暂存删除。
    if (tracked) {
      await repo.run("git", ["-C", root, "reset", "-q", "--", rel]).catch(() => {});
      await repo.run("git", ["-C", root, "checkout", "HEAD", "--", rel]).catch(() => {});
    }
    throw error.status ? error : withStatus(new Error(`彻底删除失败，已回滚：${errorLine(error)}`), 400);
  }
};

// ── 路由 ────────────────────────────────────────────────────────────────────

/**
 * 处理 `/v1/skills*`。`res` 由这里写，返回值没人看。
 *
 * `repo` = `repo-tools.js` 的整包（`syncRepo` / `pendingState` / `listRepoFiles` …），
 * 每个函数都收 `root`。测试递同一包、换一个 `root` 就是另一个仓。
 */
export async function handleSkills({ req, res, url, parts, root, skills, repo, data, variants = null, reload = null }) {
  const method = req.method ?? "GET";
  const at = parts.slice(2);
  // 镜像落在块的 data/ 里（缓存，不是事实）。没有 data 的上游路由无处落盘 —— 直说，
  // 别悄悄往仓库根里 clone（那会把几 MB 的 bare 仓提交进内容仓）。
  const mirrors = () => {
    if (!data) throw withStatus(new Error("这个进程没有 data 目录，上游镜像无处落盘"), 500);
    return data;
  };

  try {
    if (!at.length) {
      if (method !== "GET") return send(res, 405, { error: "技能仓库只有 GET" });
      return send(res, 200, await overview(repo, root, skills));
    }

    if (at[0] === "categories") {
      if (method === "GET") return send(res, 200, { categories: await repo.readCategories(root, NO_PROTECT) });
      if (method !== "POST") return send(res, 405, { error: "类别词表只有 GET / POST" });
      const body = await parseJsonBody(req);
      const result = await repo.serializeRepoWrite(root, () =>
        repo.mutateCategories(
          root,
          { op: body.op, name: body.name, to: body.to },
          { index: skillsIndex(repo, root), ...NO_PROTECT },
        ),
      );
      const loadError = await reloadRoster(skills);
      return send(res, 200, {
        categories: result.categories,
        skills: skills.list(),
        touched: result.touched,
        ...(loadError ? { loadError } : {}),
      });
    }

    if (at[0] === "category") {
      if (method !== "POST") return send(res, 405, { error: "单个技能的类别只有 POST" });
      const body = await parseJsonBody(req);
      const name = checkName(body.name);
      await repo.serializeRepoWrite(root, () =>
        repo.setNodeCategories(root, name, body.category ?? [], {
          label: "技能",
          // 分组里的技能，nous.json 在 `<root>/<分组>/<名字>/` 下 —— 不传 dir 会改到顶层去。
          dir: homeOf(root, skills, name),
          index: skillsIndex(repo, root),
          ...NO_PROTECT,
        }),
      );
      const loadError = await reloadRoster(skills);
      const roster = skills.list();
      return send(res, 200, {
        skill: roster.find((skill) => skill.name === name) ?? null,
        skills: roster,
        categories: await repo.readCategories(root, NO_PROTECT),
        ...(loadError ? { loadError } : {}),
      });
    }

    if (at[0] === "files") {
      if (method !== "GET") return send(res, 405, { error: "文件列表只有 GET" });
      const name = checkName(url.searchParams.get("name"));
      const variant = variantParam(url.searchParams.get("variant"));
      const files = await repo.listRepoFiles(root, name, {
        dir: await placeOf(root, skills, variants, name, variant),
        kinds: null,
        maxFiles: 200,
        label: "技能",
      });
      return send(res, 200, { name, files: files.map((file) => ({ path: file.path, size: file.size })) });
    }

    // 建一个空目录。**不提交**，跟写文件一样等发布。
    // 空目录 git 不跟踪，所以里面会铺一份 `.gitkeep` 撑着 —— 第一次写真文件时它会被收掉。
    if (at[0] === "mkdir") {
      if (method !== "POST") return send(res, 405, { error: "新建文件夹只有 POST" });
      const body = await parseJsonBody(req);
      const name = checkName(body.name);
      const variant = variantParam(body.variant);
      const dir = await placeOf(root, skills, variants, name, variant);
      const made = await repo.serializeRepoWrite(root, () =>
        repo.mkdirRepoDir(root, { name, rel: body.path, dir, label: "技能" }),
      );
      return send(res, 200, made);
    }

    if (at[0] === "file") {
      if (method === "GET") {
        const name = checkName(url.searchParams.get("name"));
        const variant = variantParam(url.searchParams.get("variant"));
        const rel = String(url.searchParams.get("path") ?? "");
        const file = await repo.readRepoFile(root, name, rel, {
          dir: await placeOf(root, skills, variants, name, variant),
          allowAny: true,
          label: "技能",
        });
        return send(res, 200, { name: file.name, path: file.path, text: file.text, etag: file.etag });
      }
      if (method === "DELETE") {
        // 删文件 / 删目录（`.trash` 那套在技能仓里没必要 —— git 历史就是回收站）。
        // 目录里还有东西时不带 `recursive=1` 就是 409：一下点掉一堆稿子不是任何人想要的。
        const name = checkName(url.searchParams.get("name"));
        const variant = variantParam(url.searchParams.get("variant"));
        const rel = String(url.searchParams.get("path") ?? "");
        const recursive = url.searchParams.get("recursive") === "1";
        const dir = await placeOf(root, skills, variants, name, variant);
        const gone = await repo.serializeRepoWrite(root, () =>
          repo.deleteRepoPath(root, {
            name,
            rel,
            dir,
            label: "技能",
            recursive,
          }),
        );
        await reloadKey(skills, name, variant);
        return send(res, 200, gone);
      }
      if (method !== "PUT") return send(res, 405, { error: "文件只有 GET / PUT / DELETE" });
      const body = await parseJsonBody(req);
      const name = checkName(body.name);
      const variant = variantParam(body.variant);
      const dir = await placeOf(root, skills, variants, name, variant);
      const saved = await repo.serializeRepoWrite(root, () =>
        repo.writeRepoFile(root, {
          name,
          rel: body.path,
          text: body.text,
          base: body.base,
          // 路径可以指向一个还不存在的文件 = 新建（中间的目录一起造出来）。
          dir,
          allowAny: true,
          // 技能里的闸是 frontmatter（publish 时才拦），不是 `node --check`。
          checkCode: false,
          strictIndex: false,
          label: "技能",
        }),
      );
      await reloadKey(skills, name, variant);
      return send(res, 200, { etag: saved.etag, path: saved.path, created: !body.base, variant });
    }

    if (at[0] === "create") {
      if (method !== "POST") return send(res, 405, { error: "新建只有 POST" });
      const body = await parseJsonBody(req);
      await repo.serializeRepoWrite(root, () => createSkill(repo, root, body));
      const loadError = await reloadRoster(skills);
      const roster = skills.list();
      return send(res, 200, {
        skill: roster.find((skill) => skill.name === String(body.name ?? "").trim()) ?? null,
        skills: roster,
        ...(loadError ? { loadError } : {}),
      });
    }

    if (at[0] === "duplicate") {
      if (method !== "POST") return send(res, 405, { error: "复制只有 POST" });
      const body = await parseJsonBody(req);
      const result = await repo.serializeRepoWrite(root, () => duplicateSkill(repo, root, skills, body));
      const loadError = await reloadRoster(skills);
      const roster = skills.list();
      return send(res, 200, {
        ok: true,
        name: result.name,
        path: result.path,
        from: result.from,
        variant: result.variant,
        files: result.fileCount,
        skill: roster.find((skill) => skill.name === result.name) ?? null,
        skills: roster,
        ...(loadError ? { loadError } : {}),
      });
    }

    // ── 变体（照搬扩展那套：`本名@id` + 本机绑定 + 自己的分支） ────────────────
    //
    // 变体**不是复制**：内容在那条分支上，这里只管三件事 —— 这台绑了谁、绑一个、解一个。
    // 真正的装载靠 server.js 重建 skills service（`reload`），不然页面说有、会话里没有。
    if (at[0] === "variant") {
      if (!variants) throw withStatus(new Error("这个进程没有变体层（data/ 不可写？）"), 400);

      if (method === "GET") {
        const family = checkName(url.searchParams.get("name"));
        const bound = (await variants.list()).filter((entry) => entry.family === family);
        const roster = skills.list() ?? [];
        return send(res, 200, {
          name: family,
          // 绑了哪些 + 每条现在装得怎么样（failed 的会把原因带上来，面板上就是红卡）。
          variants: bound.map((entry) => ({
            id: entry.id,
            key: `${family}${VARIANT_SEP}${entry.id}`,
            root: entry.root,
            branch: entry.branch,
            exists: entry.exists,
            skill: roster.find((skill) => skill.name === `${family}${VARIANT_SEP}${entry.id}`) ?? null,
          })),
        });
      }

      if (method !== "POST") return send(res, 405, { error: "变体只有 GET / POST" });
      const body = await parseJsonBody(req);
      const family = checkName(body.name);
      const op = String(body.op ?? "bind").trim();

      if (op === "bind") {
        const id = checkVariantId(body.id);
        const made = await variants.bind({ family, id, branch: body.branch });
        // 绑完必须重造 service：`variants` 是 createSkills 的构造参数，不重造就装不进来。
        if (reload) await reload();
        return send(res, 200, {
          ok: true,
          op,
          key: `${family}${VARIANT_SEP}${id}`,
          root: made.root,
          branch: made.branch,
          skills: skills.list(),
        });
      }

      if (op === "unbind") {
        const id = checkVariantId(body.id);
        const gone = await variants.unbind({ family, id });
        if (reload) await reload();
        return send(res, 200, {
          ok: true,
          op,
          key: `${family}${VARIANT_SEP}${id}`,
          // 解绑只摘记录：树和分支都还在（页面上得说清楚，不然人会以为字被删了）。
          root: gone.root,
          branch: gone.branch,
          skills: skills.list(),
        });
      }

      throw withStatus(new Error(`变体没有「${op}」这个动作（bind / unbind）`), 400);
    }

    if (at[0] === "publish") {
      if (method !== "POST") return send(res, 405, { error: "发布只有 POST" });
      const body = await parseJsonBody(req);
      // `variant` 给了 = 发**变体那条分支**：在它自己的树上 commit + push，主干一个字节不碰。
      // 不给就是原来的「发整个技能仓」。
      const variant = variantParam(body.variant);
      if (variant) {
        if (!variants) throw withStatus(new Error("这个进程没有变体层"), 400);
        const family = checkName(body.name);
        const out = await variants.publish({
          family,
          id: variant,
          message: body.message,
        });
        return send(res, 200, {
          ok: out.pushed !== false,
          committed: Boolean(out.committed),
          pushed: out.pushed === true,
          branch: out.branch,
          changed: out.changed ?? [],
          ...(out.pushError ? { error: String(out.pushError).slice(0, 400) } : {}),
        });
      }
      const result = await repo.serializeRepoWrite(root, async () => {
        const snapshot = await snapshotIndex(root);
        try {
          // 闸在最前面：不过就一个字节都不动（index.json 也不重扫，否则会留下「扫了一半」的假象）。
          const problems = await gateRepo(root);
          if (problems.length) throw withStatus(new Error(gateMessage(problems)), 400);
          await repo.regenIndex(root, { required: false });
          return await repo.syncRepo(root, String(body.message ?? "").trim() || "skill: 更新技能仓库", {
            label: "技能",
            tag: "skill",
          });
        } catch (error) {
          await restoreIndex(snapshot);
          throw error;
        }
      });
      const loadError = await reloadRoster(skills);
      const warnings = [];
      if (result.skippedIndex?.length) {
        warnings.push(`未发布（HEAD 里没有）的没跟着这次提交走：${result.skippedIndex.join("、")}`);
      }
      if (loadError) warnings.push(`已提交，但装载失败：${loadError}`);
      return send(res, 200, { ...(await writeResult(repo, root, result)), warnings });
    }

    if (at[0] === "archive" || at[0] === "restore" || at[0] === "purge") {
      if (method !== "POST") return send(res, 405, { error: `${{ archive: "归档", restore: "恢复", purge: "彻底删除" }[at[0]]}只有 POST` });
      const body = await parseJsonBody(req);
      const name = checkName(body.name);
      const task = { archive: archiveSkill, restore: restoreSkill, purge: purgeSkill }[at[0]];
      const result = await repo.serializeRepoWrite(root, () =>
        at[0] === "archive" ? task(repo, root, name, skills) : task(repo, root, name),
      );
      await reloadRoster(skills);
      return send(res, 200, { ...(await writeResult(repo, root, result)), name });
    }

    if (at[0] === "pending") {
      if (method !== "GET") return send(res, 405, { error: "待同步清单只有 GET" });
      if (at[1] === "diff") {
        // 变体的基线在**它那棵树**的 HEAD 上：拿主干的 root 去 diff，会永远报「没有这个文件」。
        const rawPath = String(url.searchParams.get("path") ?? "");
        const variant = variantParam(url.searchParams.get("variant"));
        const gitRoot = variant ? (await variants?.rootOf(splitKey(rawPath.split("/")[0]).family, variant)) ?? root : root;
        const diff = await repo.pendingFileDiff(gitRoot, rawPath);
        // `before: null` = HEAD 里从来没有过这个文件（未发布）；空串是「有，但内容是空的」。
        const tracked = await repo.trackedFile(gitRoot, diff.path);
        return send(res, 200, { path: diff.path, before: tracked ? diff.base : null, after: diff.missing ? null : diff.text });
      }
      if (at[1] === "commit") {
        return send(res, 200, await repo.pendingCommit(root, url.searchParams.get("hash")));
      }
      return send(res, 200, await repo.pendingState(root, { unitKind: "skill" }));
    }

    if (at[0] === "sync") {
      if (method !== "POST") return send(res, 405, { error: "同步只有 POST" });
      const body = await parseJsonBody(req);
      const paths = Array.isArray(body.paths) ? body.paths.map(String) : null;
      const result = await repo.serializeRepoWrite(root, () =>
        repo.syncRepo(root, String(body.message ?? "").trim() || null, { paths, label: "技能", tag: "skill" }),
      );
      await reloadRoster(skills);
      return send(res, 200, await writeResult(repo, root, result));
    }

    if (at[0] === "reload") {
      if (method !== "POST") return send(res, 405, { error: "重读只有 POST" });
      return send(res, 200, await repo.serializeRepoWrite(root, async () => {
        const error = await reloadRoster(skills);
        const roster = skills.list();
        return error ? { ok: false, error, skills: roster } : { ok: true, skills: roster };
      }));
    }

    if (at[0] === "sources") {
      if (method === "GET") return send(res, 200, { sources: (await upstream.readSources(root)).sources });
      if (method !== "POST") return send(res, 405, { error: "订阅清单只有 GET / POST" });
      const body = await parseJsonBody(req);
      // 整份替换：这文件就几行，diff 一眼看得完。写盘不提交 —— 它跟技能一样是内容，
      // 「加了一个上游」也得走发布（并且在面板上是琥珀虚线）。
      const sources = await repo.serializeRepoWrite(root, () => upstream.writeSources(root, body.sources));
      return send(res, 200, { ok: true, sources });
    }

    if (at[0] === "upstream") {
      if (!at[1]) {
        if (method !== "GET") return send(res, 405, { error: "上游清单只有 GET" });
        const { sources } = await upstream.readSources(root);
        const roster = skills.list() ?? [];
        const reported = [];
        const entries = [];
        const errors = [];
        for (const source of sources) {
          // **打开面板不联网**：镜像不在就报 missing，让人自己点刷新。
          const ready = await upstream.hasMirror({ data: mirrors(), source });
          // 镜像现在停在哪个 commit（采集到的），跟 `version`（我们认的）分开摆。
          const head = ready ? await upstream.resolveHead({ data: mirrors(), source, run: repo.run }) : null;
          // 采集到的那一版在人眼里叫什么（`v1.3.2` / `v1.3.2+`）。读镜像里缓存的 tag 表，不联网。
          const headVersion = head ? await upstream.describeVersion({ data: mirrors(), source, commit: head }) : {};
          reported.push({
            ...source,
            mirror: ready ? "ready" : "missing",
            head,
            headVersion,
            // 采集到的跟认的不一样 = 上游动了。前端只摆这个布尔，不自己比。
            drift: Boolean(head && source.version?.commit && head !== source.version.commit),
            // 本机那个目录：只装了技能（sparse checkout），复制路径给的就是它。
            dir: upstream.localPath(mirrors(), source),
            // 占了多少（面板上摆出来：删了下次 fetch 重建，不是第二份真相）。
            bytes: ready ? await upstream.mirrorBytes({ data: mirrors(), source }) : 0,
          });
          if (!ready) continue;
          try {
            for (const entry of await upstream.listEntries({ data, source, run: repo.run })) {
              entries.push({ ...entry, local: localStateOf(roster, await footOf(root, relOf(skills, entry.name)), entry) });
            }
          } catch (error) {
            errors.push({ source: source.name, error: errorLine(error) });
          }
        }
        return send(res, 200, { sources: reported, entries, errors });
      }

      if (at[1] === "refresh") {
        if (method !== "POST") return send(res, 405, { error: "刷新只有 POST" });
        const body = await parseJsonBody(req);
        const only = String(body.name ?? "").trim() || null;
        const { sources } = await upstream.readSources(root);
        const picked = only ? sources.filter((source) => source.name === only) : sources;
        if (only && !picked.length) throw withStatus(new Error(`没有订阅「${only}」这个上游`), 400);
        const out = [];
        // 一个挂了不该拖住别的：每个 source 各自报 ok/error，面板上分开显示。
        for (const source of picked) {
          try {
            await upstream.fetchMirror({ data: mirrors(), source, run: repo.run });
            const head = await upstream.resolveHead({ data: mirrors(), source, run: repo.run });
            // **首拉 = 创建时的基线**：这时候还没有 `version`，记一次。
            // 之后 refresh 只管采集，绝不改动已记录的版本（改了「记录」就自己会跑）。
            const pinned = source.version
              ? source.version
              : await repo.serializeRepoWrite(root, () =>
                  upstream.pinVersion({ root, data: mirrors(), source, run: repo.run, commit: head }),
                );
            out.push({ name: source.name, ok: true, head, version: pinned });
          } catch (error) {
            out.push({ name: source.name, ok: false, error: errorLine(error) });
          }
        }
        return send(res, 200, { ok: out.every((item) => item.ok), sources: out });
      }

      // 一整个仓的文件清单。「一个订阅 = 一条技能」，所以抽屉里那棵树的根是**仓根**。
      // 把「我们认的版本」钉到某个 commit（不给就用镜像 HEAD）。**只有这一个写入口。**
      if (at[1] === "pin") {
        if (method !== "POST") return send(res, 405, { error: "记录版本只有 POST" });
        const body = await parseJsonBody(req);
        const sourceName = String(body.source ?? body.name ?? "").trim();
        const { sources } = await upstream.readSources(root);
        const source = sources.find((item) => item.name === sourceName);
        if (!source) throw withStatus(new Error(`没有订阅「${sourceName}」这个上游`), 400);
        if (!(await upstream.hasMirror({ data: mirrors(), source }))) {
          throw withStatus(new Error(`${sourceName}: 还没拉过镜像 —— 先刷新，才有版本可记`), 400);
        }
        const commit = String(body.commit ?? "").trim() || null;
        const version = await repo.serializeRepoWrite(root, () =>
          upstream.pinVersion({ root, data: mirrors(), source, run: repo.run, commit }),
        );
        return send(res, 200, { ok: true, source: source.name, version });
      }

      if (at[1] === "tree") {
        if (method !== "GET") return send(res, 405, { error: "上游文件树只有 GET" });
        const sourceName = String(url.searchParams.get("source") ?? "").trim();
        const { sources } = await upstream.readSources(root);
        const source = sources.find((item) => item.name === sourceName);
        if (!source) throw withStatus(new Error(`没有订阅「${sourceName}」这个上游`), 400);
        if (!(await upstream.hasMirror({ data: mirrors(), source }))) {
          throw withStatus(new Error(`${sourceName}: 还没拉过镜像 —— 先点刷新`), 400);
        }
        const tree = await upstream.repoFiles({ data: mirrors(), source, run: repo.run });
        return send(res, 200, { source: source.name, head: tree.head, files: tree.files });
      }

      // 上游的正文：**只读**。抽屉里那棵文件树吃这一条，一条一条点也是这一条。
      // 别人的仓我们没有写入口 —— 这条路上只有 GET。POST 是 405，不是 400。
      // `path` 是仓里的绝对路径（`skills/x/SKILL.md`）；也认「目录 + 文件名」两个参数的老形状。
      if (at[1] === "file") {
        if (method !== "GET") return send(res, 405, { error: "上游正文只有 GET —— 别人的仓我们只读" });
        const sourceName = String(url.searchParams.get("source") ?? "").trim();
        const home = String(url.searchParams.get("path") ?? "").trim().replace(/^\/+|\/+$/g, "");
        const named = String(url.searchParams.get("file") ?? "").trim();
        const rel = named || (home ? "SKILL.md" : "");
        const { sources } = await upstream.readSources(root);
        const source = sources.find((item) => item.name === sourceName);
        if (!source) throw withStatus(new Error(`没有订阅「${sourceName}」这个上游`), 400);
        if (!rel) throw withStatus(new Error("没说看哪个文件（file 是空的）"), 400);
        if (!(await upstream.hasMirror({ data: mirrors(), source }))) {
          throw withStatus(new Error(`${sourceName}: 还没拉过镜像 —— 先点刷新`), 400);
        }
        const file = await upstream.entryText({ data: mirrors(), source, run: repo.run, home, rel });
        return send(res, 200, {
          source: source.name,
          path: home,
          file: home ? `${home}/${file.path}` : file.path,
          text: file.text,
          size: file.size,
        });
      }

      if (at[1] === "files") {
        if (method !== "GET") return send(res, 405, { error: "上游文件清单只有 GET" });
        const sourceName = String(url.searchParams.get("source") ?? "").trim();
        const home = String(url.searchParams.get("path") ?? "").trim().replace(/^\/+|\/+$/g, "");
        const { sources } = await upstream.readSources(root);
        const source = sources.find((item) => item.name === sourceName);
        if (!source) throw withStatus(new Error(`没有订阅「${sourceName}」这个上游`), 400);
        if (!home) throw withStatus(new Error("没说看哪一条（path 是空的）"), 400);

        // 本地对照的那一侧：优先找「从这条取的」那个本地技能（可能取用时改过名）。
        const roster = skills.list() ?? [];
        const adopted = (
          await Promise.all(roster.map(async (skill) => ({ skill, foot: await footOf(root, skill.path ?? skill.name) })))
        ).find(
          (item) => item.foot?.startsWith(`${source.name}/${home}@`),
        );
        const localName = adopted?.skill.name ?? path.posix.basename(home);
        // 本地对照用的是**它住哪儿**（`deprecated/demo`），不是它叫什么 —— 用名字会找错文件。
        const localRel = adopted?.skill.path ?? localName;
        const files = await upstream.entryFiles({ data: mirrors(), source, run: repo.run, dir: home });
        const out = [];
        for (const file of files) {
          const localFile = path.join(root, localRel, file.path);
          let state = "missing";
          if (await stat(localFile).catch(() => null)) {
            // 比 git blob sha：内容一致就是 same，一个字节不同就是 diff。
            const localSha = (await repo.run("git", ["-C", root, "hash-object", "--no-filters", "--", localFile])).stdout.trim();
            state = localSha === file.sha ? "same" : "diff";
          }
          out.push({ path: file.path, state, size: file.size });
        }
        return send(res, 200, { source: source.name, path: home, localName, adopted: Boolean(adopted), files: out });
      }

      if (at[1] === "adopt") {
        if (method !== "POST") return send(res, 405, { error: "取用只有 POST" });
        const body = await parseJsonBody(req);
        const result = await repo.serializeRepoWrite(root, () => adoptSkill(repo, root, mirrors(), body));
        const loadError = await reloadRoster(skills);
        const roster = skills.list();
        return send(res, 200, {
          ok: true,
          name: result.name,
          files: result.fileCount,
          skill: roster.find((skill) => skill.name === result.name) ?? null,
          skills: roster,
          ...(loadError ? { loadError } : {}),
        });
      }

      return send(res, 404, { error: "upstream 没有这条路", path: at.join("/") });
    }

    return send(res, 404, { error: "skills 没有这条路", path: at.join("/") });
  } catch (error) {
    return fail(res, error);
  }
}

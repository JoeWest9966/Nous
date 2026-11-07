// `/v1/presets/<段>/*` —— 模板私库的 HTTP 面。**一个仓按段放**，每段一套端点、一套数据。
//
// 跟 `prompts-api.js` 是同一个模子（错误信封 / 串行锁 / `regenOrRollback` / 归档三件套 /
// `pending` 四件套），差别只有三处语义：
//
//   · 一条预设 = `preset.json`（纯配置 **JSON**，没有正文、没有 frontmatter）。它引用别的
//     命名空间（模型 / 技能 / 扩展 / 模板），自己不拥有任何内容。
//   · 除了「形状」，还有「引用」要校验 —— 那是 `presets.js` 的 `referenceProblems`，需要活的
//     registry / skills / prompts / models。所以这里多两个只读端点 `validate` / `resolve`。
//   · **没有 `only: "categories"` 的自动提交。** 预设这一页不走「改个类别顺手推一次」那条路：
//     所有提交都是 publish / sync / 归档三件套，都是人显式点的。
//
// 一条铁律：**仓库是唯一真相。** 这里不维护第二份名单、不读 `data/`、不联网。
// 盘上有这个目录 = 这条存在；HEAD 里有这个目录 = 已发布。
//
// **段（section）是路径的第一段。** 目录名 = 段 ID = git 路径前缀 = URL 段：只有一份事实源。
// 段的隔离靠三条：文件名操作收在 `<段>/` 里（`root` 就是段目录）、git 那边一律带 `scope`
// （`Session-Template`），所以「在 Session 里点发布」碰不到 Ultimate 里没写完的东西。
//
// 约定（跟 prompts-api / skills-api 同一套）：
//   · 统一错误信封 `{ error }` + 状态码：400 校验 / 409 冲突 / 404 不在 / 500 内部。
//   · 每个写端点都在**本仓的**串行锁里（`serializeRepoWrite(repoRoot, …)` —— 锁是整仓的，
//     因为重扫索引是整仓的动作），写完 `presets.load()`。
//     不 load 的话面板显示新预设、建会话时还看不见它。
//   · `publish` = 形状闸 + 重扫 index.json + commit + push，一步；闸不过不动 index.json。

import { mkdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";

import {
  resolveSpec,
  resolveUltimate,
  shapeProblems,
  oneLine,
  ultimateReferenceProblems,
  ultimateShapeProblems,
  validatePresetSpec,
} from "./presets.js";

const withStatus = (error, status) => Object.assign(error, { status });

const send = (res, status, body) => {
  const text = JSON.stringify(body);
  res.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(text),
    "cache-control": "no-store",
  });
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

const fail = (res, error) => send(res, error?.status ?? 500, { error: oneLine(error) });

// ── 名字 ────────────────────────────────────────────────────────────────────

/** 小写、≤64 字：名字就是目录名、就是会话里的键（`preset:<名>` 不存在，但出处记着它），建完不改。 */
const PRESET_NAME_RE = /^[a-z0-9][a-z0-9._-]*$/;
const MAX_NAME = 64;

/** 仓库根下不是预设的名字（撞上它们 = 把索引 / 词表当预设编辑）。 */
const RESERVED = new Set([
  "categories.json",
  "index.json",
  "index.mjs",
  "preset.json",
  "PRESET.md",
  "README.md",
  "AGENTS.md",
  "test",
  "node_modules",
]);

const checkName = (raw) => {
  const name = String(raw ?? "").trim();
  if (!name) throw withStatus(new Error("预设名不能为空"), 400);
  if (name.length > MAX_NAME) throw withStatus(new Error(`预设名太长（最多 ${MAX_NAME} 字）：${name}`), 400);
  if (!PRESET_NAME_RE.test(name)) {
    throw withStatus(new Error(`预设名「${name}」不合格：小写字母 / 数字开头，只留 . _ -`), 400);
  }
  if (RESERVED.has(name)) throw withStatus(new Error(`「${name}」是仓库自用的名字，不能当预设名`), 400);
  return name;
};

// ── 内容闸 ─────────────────────────────────────────────────────────────────

/**
 * 一段自己的「形状」规矩：**清单文件名 + 校验器**都由段决定
 * （`preset.json` / shapeProblems vs `ultimate.json` / ultimateShapeProblems）。
 */
const shapeKit = (section) => ({
  manifest: section?.manifest ?? "preset.json",
  check: section?.checker === "ultimate" ? ultimateShapeProblems : shapeProblems,
});

/** 一条东西的形状对不对（不看引用）。返回问题清单，空 = 能发布。 */
export const checkPresetContent = async (root, name, dir = null, kit = null) => {
  const { manifest, check } = kit ?? { manifest: "preset.json", check: shapeProblems };
  const home = dir ?? path.join(root, name);
  let raw;
  try {
    raw = await readFile(path.join(home, manifest), "utf8");
  } catch {
    return [`${name}: 没有 ${manifest}`];
  }
  let spec;
  try {
    spec = JSON.parse(raw);
  } catch (error) {
    return [`${name}: ${manifest} 不是合法 JSON（${oneLine(error)}）`];
  }
  return check(spec, { dirName: name, where: `${name}/${manifest}` });
};

/** 仓库里所有带清单文件的目录 —— 内容闸扫这一圈（没有清单文件的目录静默跳过）。 */
const gateRepo = async (repo, root, kit = null) => {
  const { manifest } = kit ?? { manifest: "preset.json" };
  let dirs = [];
  try {
    dirs = await repo.listDirs(root);
  } catch {
    return []; // 仓库都不在：没东西可发布，也就没东西可拦
  }
  const problems = [];
  for (const dir of dirs) {
    if (!(await stat(path.join(root, dir, manifest)).catch(() => null))) continue;
    problems.push(...(await checkPresetContent(root, dir, null, kit)));
  }
  return problems;
};

const gateMessage = (problems) => {
  const shown = problems.slice(0, 4).join("；");
  return problems.length > 4 ? `${shown}；等 ${problems.length} 项` : shown;
};

// ── 词表 / 生成器 ───────────────────────────────────────────────────────────

// 生成器住在**仓根**（它按段扫，各写各的 `<段>/index.json`），不是段目录里。
const presetsIndex = (repo, repoRoot) => () => repo.regenIndex(repoRoot, { required: false });

// 预设仓**没有**受保护的类别：词表工具默认护着扩展仓的 Basics，所以每个调用点都得显式说
// 「一个都不护」。
const NO_PROTECT = { protect: [] };

// ── 公共拼装 ────────────────────────────────────────────────────────────────

const commitOf = async (repo, repoRoot, result) => {
  if (!result.committed) return null;
  const info = await repo.lastCommitInfo(repoRoot);
  return { hash: info.hash, subject: info.subject };
};

/**
 * 推送结果。`ok` 的语义是「这一步做完了」，**不是**「推上去了」：没东西可提交和推成功都
 * 算做完了；只有「提交了但没推成」才是 false（那时另有 `error`）。
 *
 * 一个例外，而且是本仓特有的：**没配 remote 不是失败。** 「专属私仓」的 origin 归用户
 * 自己配，本地单用户也是合法用法。`syncRepo` 在这件事上会报一句 git 的
 * `No configured push destination` —— 那句话对扩展仓是对的（它有 origin），对这一页
 * 就是假红。所以这里把它折成 `pushed: null` + `noRemote: true`，页面照实显示「只在本地」。
 */
const writeResult = async (repo, repoRoot, result) => {
  const remote = await repo.remoteUrl(repoRoot).catch(() => null);
  const noRemote = !remote && result.pushed === false;
  // `pushed` 三态：true 推了 / false 推挂了 / null 没东西可推（或本来就没远端）。
  const pushed = noRemote ? null : result.pushed === true ? true : result.pushed === false ? false : null;
  return {
    ok: pushed === false ? false : true,
    commit: await commitOf(repo, repoRoot, result),
    pushed,
    ...(noRemote ? { noRemote: true } : {}),
    ...(result.pushError && pushed === false ? { error: String(result.pushError).slice(0, 400) } : {}),
  };
};

// 半截不留：重扫 → commit → push 三步，第一步就改了 index.json；后面翻车会留一份
// 「索引说改了、git 里没有」的账。进 try 之前抄原文，失败时写回。
const snapshotIndex = async (root) => {
  const file = path.join(root, "index.json");
  return { file, text: await readFile(file, "utf8").catch(() => null) };
};

const restoreIndex = async ({ file, text }) => {
  if (text === null) await rm(file, { force: true }).catch(() => {});
  else await writeFile(file, text, "utf8").catch(() => {});
};

/** 写完盘让内存名册跟上。跟不上的话面板说有、建会话时没有。 */
const reloadRoster = async (presets) => {
  try {
    await presets.load();
    return null;
  } catch (error) {
    return oneLine(error);
  }
};

/** `ctx` 由 server 递进来（活的 registry / skills / prompts / models），这里只负责调。 */
const ctxOf = (ctx) => (typeof ctx === "function" ? ctx() : (ctx ?? {}));

// ── 各端点 ─────────────────────────────────────────────────────────────────

const overview = async ({ repo, repoRoot, section, root, presets, ctx, ultimateCtx }) => {
  const kit = shapeKit(section);
  const isUltimate = section.checker === "ultimate";
  let state = { head: null, dirty: false, ahead: 0, behind: 0 };
  let error = null;
  try {
    // `dirty` 只认这一段：另一段没写完不该让这一段的顶栏变脏（历史是共用的，ahead 是整仓的）。
    state = await repo.repoState(repoRoot, { scope: section.id });
  } catch (caught) {
    // 仓库读不了要给页面一句话，不是 500 —— 白屏说不清「是仓库没了还是预设没了」。
    error = oneLine(caught);
  }
  const [remote, tracked, vocab, archived, roster] = await Promise.all([
    repo.remoteUrl(repoRoot).catch(() => null),
    // 「已发布」的判据要**带段前缀**：HEAD 里叫 `Session-Template/weekly-report`。
    repo.trackedPaths(repoRoot).catch(() => new Set()),
    repo.readCategories(root, NO_PROTECT).catch(() => []),
    // 归档条目的摘要住在**那一段的清单文件**里（不是 `nous.json`）—— 不给这个参数就会显示成「没有摘要」。
    repo.archivedEntries(root, { manifest: kit.manifest }).catch(() => []),
    Promise.resolve().then(() => presets.list()).catch(() => []),
  ]);

  // 引用校验用的是**此刻**的进程状态 —— 跟「启动」那一下走同一个 `presets.js`，
  // 所以面板说红的就是启动会失败的那条。
  const resolvedCtx = ctxOf(ctx);
  // 终极模板要问**另一个段**的名册（会话模板在 Session-Template 段里）+ 触发器表。
  const ultimate = isUltimate ? await ultimateCtx?.() : null;
  const rows = roster.map((item) => ({
    name: item.name,
    summary: item.summary,
    description: item.description,
    category: item.category,
    spec: item.spec,
    // `specError` = 这条自己写坏了（JSON / 形状）。引用坏了不算 —— 那是「现在开不了」。
    specError: item.specError,
    tracked: tracked.has(`${section.id}/${item.name}`),
    dir: item.dir,
    resolved: item.specError
      ? null
      : isUltimate
        ? resolveUltimate(item.spec, ultimate ?? {})
        : resolveSpec(item.spec, resolvedCtx),
  }));

  // 真相是**两份取并集**：词表里的（可能有零计数的空类别）+ 每条预设自己挂着的。
  const categories = repo.sortCategories([...vocab, ...roster.flatMap((item) => item.category ?? [])]);

  return {
    section: section.id,
    title: section.title,
    root,
    repo: { ...state, remote, error },
    presets: rows,
    archived: archived.map((entry) => ({
      name: entry.name,
      summary: entry.summary,
      category: entry.category ?? [],
      archivedAt: entry.archivedAt ?? null,
    })),
    categories,
  };
};

const createPreset = async ({ repo, repoRoot, root, body, ctx, section, ultimateCtx }) => {
  const kit = shapeKit(section);
  const name = checkName(body.name);
  const summary = String(body.summary ?? "").trim();
  if (!summary) throw withStatus(new Error(`${name}: summary 不能空 —— 人靠这一行决定开不开它`), 400);

  const categories = repo.sortCategories(repo.normalizeCategories(body.category ?? []));

  // 起手式三种：
  //   `spec`   —— 整份原样落（页面表单拼好的那份，或 JSON tab 里的原文）
  //   `from`   —— 从设备默认模型起手（页面上的第二个选项）
  //   什么都没有 —— 空白：所有可选键都不写（template 也不写 = 跟随仓库默认）
  let spec;
  if (body.spec && typeof body.spec === "object" && !Array.isArray(body.spec)) {
    spec = { ...body.spec };
  } else {
    spec = {};
  }
  spec.v = 1;
  spec.name = name;
  spec.summary = summary;
  spec.category = categories;
  // 终极模板的三个字段（触发器 / 参数 / 会话模板）。**不在这儿校验引用** ——
  // 引用要问触发器表和另一个段的名册，那是引擎的事；这里只管形状。
  if (section?.checker === "ultimate") {
    const trigger = String(body.trigger ?? "").trim();
    const session = String(body.session ?? "").trim();
    if (!trigger) throw withStatus(new Error(`${name}: 得选一个触发器 —— 没有触发器就没有「什么时候」`), 400);
    if (!session) throw withStatus(new Error(`${name}: 得选一条会话模板 —— 触发器只管「什么时候」`), 400);
    spec.trigger = trigger;
    spec.session = session;
    const params = body.params && typeof body.params === "object" && !Array.isArray(body.params) ? body.params : {};
    spec.params = params;
    spec.enabled = body.enabled !== false;
  }
  if (body.description !== undefined) spec.description = String(body.description ?? "");
  if (body.from === "device") {
    const device = ctxOf(ctx)?.deviceModel ?? null;
    if (device?.provider && device?.modelId) spec.model = { provider: device.provider, modelId: device.modelId };
  }

  // 先自己过一遍闸：一条建出来就 `specError` 的预设，等于让人工去猜哪里写错了。
  const problems = kit.check(spec, { dirName: name, where: `${name}/${kit.manifest}` });
  if (problems.length) throw withStatus(new Error(gateMessage(problems)), 400);

  // 终极模板还得过**引用**闸：触发器还在吗、参数跟它的声明对得上吗、会话模板还在吗。
  // 存一条引用已经坏掉的行 = 造一条红卡 —— 那一行从落地那一刻就红着，没人能修（它本来就该在落地前被拦）。
  if (section?.checker === "ultimate") {
    const uctx = (await ultimateCtx?.()) ?? {};
    const reference = ultimateReferenceProblems(spec, uctx);
    if (reference.length) throw withStatus(new Error(gateMessage(reference)), 400);
  }

  if (await stat(path.join(root, name)).catch(() => null)) {
    throw withStatus(new Error(`${name}/ 已经在了 —— 换个名字，或者直接去改它`), 400);
  }
  if (await stat(path.join(root, repo.ARCHIVE_DIR, name)).catch(() => null)) {
    throw withStatus(new Error(`${name}/ 在 .archive/ 里 —— 点「恢复」把它捡回来，别建第二个`), 400);
  }

  const beforeVocab = await repo.readCategories(root, NO_PROTECT);
  // 点开头的目录扫盘器看不见 —— 校验过了才改名进去，中途翻车就整棵删掉（半截不留）。
  const staging = path.join(root, `.new-${name}-${Date.now()}`);
  await mkdir(staging, { recursive: false });
  let landed = false;
  try {
    await writeFile(path.join(staging, kit.manifest), JSON.stringify(spec, null, 2) + "\n", "utf8");
    // `body` 是给人看的说明书（PRESET.md），**永不进提示词**。给了才落。
    if (typeof body.body === "string" && body.body.trim()) {
      await writeFile(path.join(staging, "PRESET.md"), body.body, "utf8");
    }
    if (categories.length) await repo.writeCategories(root, [...beforeVocab, ...categories], NO_PROTECT);
    await rename(staging, path.join(root, name));
    landed = true;
    await repo.regenIndex(repoRoot, { required: false });
  } catch (error) {
    if (landed) await rm(path.join(root, name), { recursive: true, force: true }).catch(() => {});
    await rm(staging, { recursive: true, force: true }).catch(() => {});
    if (categories.length) await repo.writeCategories(root, beforeVocab, NO_PROTECT).catch(() => {});
    throw error;
  }
  return { ok: true, name };
};

const archivePreset = async ({ repo, repoRoot, scope, root, name }) => {
  if (!(await stat(path.join(root, name)).catch(() => null))) {
    throw withStatus(new Error(`工作区里没有 ${name}/`), 404);
  }
  const snapshot = await snapshotIndex(root);
  try {
    // 未发布的也搬进 .archive/：那是「删掉的东西不真删」，只有 purge 才真没。
    await repo.moveIntoArchive(root, name);
    await repo.regenIndex(repoRoot, { required: false });
    return await repo.syncRepo(repoRoot, `session: 归档 ${name}`, {
      includeDeleted: true,
      scope,
      label: "预设",
      tag: "session",
    });
  } catch (error) {
    await repo
      .run("git", ["-C", repoRoot, "reset", "-q", "--", `${scope}/${name}`, `${scope}/${repo.ARCHIVE_DIR}/${name}`])
      .catch(() => {});
    await repo.moveDir(path.join(root, repo.ARCHIVE_DIR, name), path.join(root, name)).catch(() => {});
    await restoreIndex(snapshot);
    throw error.status ? error : withStatus(new Error(`归档失败，已回滚：${oneLine(error)}`), 400);
  }
};

const restorePreset = async ({ repo, repoRoot, scope, root, name }) => {
  const snapshot = await snapshotIndex(root);
  try {
    await repo.moveOutOfArchive(root, name);
    await repo.regenIndex(repoRoot, { required: false });
    return await repo.syncRepo(repoRoot, `session: 从归档恢复 ${name}`, {
      includeDeleted: true,
      scope,
      label: "预设",
      tag: "session",
    });
  } catch (error) {
    await repo
      .run("git", ["-C", repoRoot, "reset", "-q", "--", `${scope}/${name}`, `${scope}/${repo.ARCHIVE_DIR}/${name}`])
      .catch(() => {});
    await repo.moveDir(path.join(root, name), path.join(root, repo.ARCHIVE_DIR, name)).catch(() => {});
    await restoreIndex(snapshot);
    throw error.status ? error : withStatus(new Error(`恢复失败，已回滚：${oneLine(error)}`), 400);
  }
};

const purgePreset = async ({ repo, repoRoot, scope, root, name }) => {
  const rel = `${repo.ARCHIVE_DIR}/${name}`;
  const source = path.join(root, rel);
  const exists = await stat(source).catch(() => null);
  // 上一次彻底删除提交/推送挂了：文件已经删了、删除还留在暂存区 —— 那一次要收尾，
  // 而不是 404 把人卡在一个半截状态里。
  const halfDone =
    !exists &&
    (await repo.run("git", ["-C", root, "status", "--porcelain", "--", rel]).catch(() => ({ stdout: "" }))).stdout
      .trim().length > 0;
  if (!exists && !halfDone) throw withStatus(new Error(`归档区里没有 ${name}`), 404);
  const tracked = await repo.isTrackedDir(root, rel);
  try {
    if (exists && tracked) await repo.run("git", ["-C", root, "rm", "-r", "-f", "--", rel], { timeout: 30_000 });
    else if (exists) await rm(source, { recursive: true, force: true });
    return await repo.syncRepo(repoRoot, `session: 彻底删除 ${name}`, {
      includeDeleted: true,
      scope,
      label: "预设",
      tag: "session",
    });
  } catch (error) {
    // 提交/推送挂了：文件已经没了，但 HEAD 里还有 —— 拿回来，别留半截的暂存删除。
    if (tracked) {
      await repo.run("git", ["-C", root, "reset", "-q", "--", rel]).catch(() => {});
      await repo.run("git", ["-C", root, "checkout", "HEAD", "--", rel]).catch(() => {});
    }
    throw error.status ? error : withStatus(new Error(`彻底删除失败，已回滚：${oneLine(error)}`), 400);
  }
};

// ── 路由 ────────────────────────────────────────────────────────────────────

/**
 * 处理 `/v1/presets/<段>/*`。`res` 由这里写，返回值没人看。
 *
 * `repo` = `repo-tools.js` 的整包，每个函数都收 `root`。测试递同一包、换一个 `root` 就是另一个仓。
 * `ctx` = 一个**函数**（每次现造）：引用校验要的是此刻的进程状态，不是启动那一刻的快照。
 *
 * `sections` = 段清单（`presets.js` 的 `readSections`），`presetsOf(id)` = 那一段的名册。
 * 段以外的路径一律 404：**没有「默认段」这种隐式状态** —— 拿不准是哪一段就说明调用方拼错了。
 */
export async function handlePresets({ req, res, url, parts, repoRoot, sections, presetsOf, repo, ctx, ultimateCtx }) {
  const method = req.method ?? "GET";
  const at = parts.slice(2);

  try {
    if (!at.length) {
      if (method !== "GET") return send(res, 405, { error: "模板仓只有 GET" });
      // 仓的目录：每段几条、脏没脏。页面不该自己去猜有哪些段。
      return send(res, 200, {
        root: repoRoot,
        sections: sections.map((section) => {
          const roster = presetsOf(section.id);
          return {
            id: section.id,
            title: section.title,
            count: roster ? roster.list().length : null,
          };
        }),
      });
    }

    // **段是第一段路径。** 认不出来就 404 —— 别猜、别退回默认段。
    const section = sections.find((entry) => entry.id === at[0]) ?? null;
    if (!section) return send(res, 404, { error: `没有这一段：${at[0]}` });
    const presets = presetsOf(section.id);
    if (!presets) return send(res, 503, { error: `这一段还没装好：${section.id}` });
    // 文件名操作收在段目录里（`root`），git 操作一律仓根 + `scope`（`repoRoot` + 段 ID）。
    const root = path.join(repoRoot, section.id);
    const scope = section.id;
    const route = at.slice(1);
    const kit = shapeKit(section);
    const isUltimate = section.checker === "ultimate";

    if (!route.length) {
      if (method !== "GET") return send(res, 405, { error: "预设仓库只有 GET" });
      return send(res, 200, await overview({ repo, repoRoot, section, root, presets, ctx, ultimateCtx }));
    }

    if (route[0] === "categories") {
      if (method === "GET") return send(res, 200, { categories: await repo.readCategories(root, NO_PROTECT) });
      if (method !== "POST") return send(res, 405, { error: "类别词表只有 GET / POST" });
      const body = await parseJsonBody(req);
      const result = await repo.serializeRepoWrite(repoRoot, () =>
        repo.mutateCategories(
          root,
          { op: body.op, name: body.name, to: body.to },
          { index: presetsIndex(repo, root), ...NO_PROTECT },
        ),
      );
      const loadError = await reloadRoster(presets);
      return send(res, 200, {
        categories: result.categories,
        presets: presets.list(),
        touched: result.touched,
        ...(loadError ? { loadError } : {}),
      });
    }

    if (route[0] === "category") {
      if (method !== "POST") return send(res, 405, { error: "单条预设的类别只有 POST" });
      const body = await parseJsonBody(req);
      const name = checkName(body.name);
      await repo.serializeRepoWrite(repoRoot, () =>
        repo.setNodeCategories(root, name, body.category ?? [], {
          label: "预设",
          manifest: kit.manifest,
          index: presetsIndex(repo, root),
          ...NO_PROTECT,
        }),
      );
      const loadError = await reloadRoster(presets);
      const roster = presets.list();
      return send(res, 200, {
        preset: roster.find((preset) => preset.name === name) ?? null,
        presets: roster,
        categories: await repo.readCategories(root, NO_PROTECT),
        ...(loadError ? { loadError } : {}),
      });
    }

    if (route[0] === "files") {
      if (method !== "GET") return send(res, 405, { error: "文件列表只有 GET" });
      const name = checkName(url.searchParams.get("name"));
      const files = await repo.listRepoFiles(root, name, { kinds: null, maxFiles: 200, label: "预设" });
      return send(res, 200, { name, files: files.map((file) => ({ path: file.path, size: file.size })) });
    }

    if (route[0] === "file") {
      if (method === "GET") {
        const name = checkName(url.searchParams.get("name"));
        const rel = String(url.searchParams.get("path") ?? "");
        const file = await repo.readRepoFile(root, name, rel, { allowAny: true, label: "预设" });
        return send(res, 200, { name: file.name, path: file.path, text: file.text, etag: file.etag });
      }
      if (method !== "PUT") return send(res, 405, { error: "文件只有 GET / PUT" });
      const body = await parseJsonBody(req);
      const name = checkName(body.name);
      // 写**那一段的清单文件**时先过形状闸：写坏了就不是「未保存」，而是这条当场失效。
      // 别的文件（PRESET.md）不闸 —— 说明书怎么写都不影响它能不能跑。
      if (String(body.path ?? "") === kit.manifest) {
        let spec;
        try {
          spec = JSON.parse(String(body.text ?? ""));
        } catch (error) {
          throw withStatus(new Error(`${kit.manifest} 不是合法 JSON：${oneLine(error)}`), 400);
        }
        const problems = kit.check(spec, { dirName: name, where: `${name}/${kit.manifest}` });
        if (problems.length) throw withStatus(new Error(gateMessage(problems)), 400);
        // 终极模板：引用也得对 —— 改坏一条去写坏引用，跟新建写坏一样是当场拦。
        if (isUltimate) {
          const reference = ultimateReferenceProblems(spec, (await ultimateCtx?.()) ?? {});
          if (reference.length) throw withStatus(new Error(gateMessage(reference)), 400);
        }
      }
      const saved = await repo.serializeRepoWrite(repoRoot, () =>
        repo.writeRepoFile(root, {
          name,
          rel: body.path,
          text: body.text,
          base: body.base,
          allowAny: true,
          // 预设的闸是「形状 + 引用」，在写盘和 publish 时拦；不是 `node --check`。
          checkCode: false,
          strictIndex: false,
          label: "预设",
        }),
      );
      await reloadRoster(presets);
      return send(res, 200, { etag: saved.etag });
    }

    if (route[0] === "create") {
      if (method !== "POST") return send(res, 405, { error: "新建只有 POST" });
      const body = await parseJsonBody(req);
      await repo.serializeRepoWrite(repoRoot, () =>
        createPreset({ repo, repoRoot, root, body, ctx, section, ultimateCtx }),
      );
      const loadError = await reloadRoster(presets);
      const roster = presets.list();
      return send(res, 200, {
        preset: roster.find((preset) => preset.name === String(body.name ?? "").trim()) ?? null,
        presets: roster,
        ...(loadError ? { loadError } : {}),
      });
    }

    // 两个只读端点：面板的编辑框靠它们做校验 / 画 chip。**只读，不写盘、不提交。**
    if (route[0] === "validate" || route[0] === "resolve") {
      if (method !== "GET" && method !== "POST") {
        return send(res, 405, { error: `${route[0]} 只有 GET / POST` });
      }
      const body = method === "POST" ? await parseJsonBody(req) : {};
      const byName = method === "GET" ? url.searchParams.get("name") : body.name;
      let spec;
      let dirName = null;
      if (byName) {
        const name = checkName(byName);
        const item = presets.get(name);
        if (!item) throw withStatus(new Error(`没有这条预设：${name}`), 404);
        spec = item.spec;
        dirName = name;
        if (item.specError) throw withStatus(new Error(item.specError), 400);
      } else if (body.spec && typeof body.spec === "object") {
        spec = body.spec;
        // 草稿还没落盘：不要求 name 等于目录名，但要求它合法（复制粘贴一个半截 JSON 很常见）。
        dirName = null;
      } else {
        throw withStatus(new Error("要么给 name，要么给 spec"), 400);
      }
      if (isUltimate) {
        // 终极模板：形状（本地那张表）+ 引用（触发器表 + 会话模板名册）。
        const shape = ultimateShapeProblems(spec, { dirName, where: `${name ?? "ultimate.json"}/${kit.manifest}` });
        if (shape.length) {
          return send(res, 200, { ok: false, errors: shape, warnings: [], spec: null, resolved: null });
        }
        const uctx = (await ultimateCtx?.()) ?? {};
        const errors = ultimateReferenceProblems(spec, uctx);
        const resolved = resolveUltimate(spec, uctx);
        if (route[0] === "resolve") {
          return send(res, 200, { ok: errors.length === 0, errors, warnings: [], resolved });
        }
        return send(res, 200, { ok: errors.length === 0, errors, warnings: [], spec, resolved });
      }
      const out = validatePresetSpec(spec, ctxOf(ctx), { dirName });
      if (route[0] === "resolve") {
        return send(res, 200, { ok: out.ok, errors: out.errors, warnings: out.warnings, resolved: out.resolved });
      }
      return send(res, 200, out);
    }

    if (route[0] === "publish") {
      if (method !== "POST") return send(res, 405, { error: "发布只有 POST" });
      const body = await parseJsonBody(req);
      const result = await repo.serializeRepoWrite(repoRoot, async () => {
        const snapshot = await snapshotIndex(root);
        try {
          // 闸在最前面：不过就一个字节都不动（index.json 也不重扫，否则会留下「扫了一半」的假象）。
          const problems = await gateRepo(repo, root, kit);
          if (problems.length) throw withStatus(new Error(gateMessage(problems)), 400);
          await repo.regenIndex(repoRoot, { required: false });
          return await repo.syncRepo(repoRoot, String(body.message ?? "").trim() || "session: 更新预设仓库", {
            scope,
            label: "预设",
            tag: "session",
          });
        } catch (error) {
          await restoreIndex(snapshot);
          throw error;
        }
      });
      const loadError = await reloadRoster(presets);
      const warnings = [];
      if (result.skippedIndex?.length) {
        warnings.push(`未发布（HEAD 里没有）的没跟着这次提交走：${result.skippedIndex.join("、")}`);
      }
      if (loadError) warnings.push(`已提交，但装载失败：${loadError}`);
      return send(res, 200, { ...(await writeResult(repo, repoRoot, result)), warnings });
    }

    if (route[0] === "archive" || route[0] === "restore" || route[0] === "purge") {
      if (method !== "POST") {
        return send(res, 405, {
          error: `${{ archive: "归档", restore: "恢复", purge: "彻底删除" }[route[0]]}只有 POST`,
        });
      }
      const body = await parseJsonBody(req);
      const name = checkName(body.name);
      const task = { archive: archivePreset, restore: restorePreset, purge: purgePreset }[route[0]];
      const result = await repo.serializeRepoWrite(repoRoot, () =>
        task({ repo, repoRoot, scope, root, name }),
      );
      await reloadRoster(presets);
      return send(res, 200, { ...(await writeResult(repo, repoRoot, result)), name });
    }

    if (route[0] === "pending") {
      if (method !== "GET") return send(res, 405, { error: "待同步清单只有 GET" });
      if (route[1] === "diff") {
        const rel = String(url.searchParams.get("path") ?? "");
        // git 只认仓根口径：`<段>/<文件>`。回给页面的 `path` 还原成段内口径。
        const diff = await repo.pendingFileDiff(repoRoot, `${scope}/${rel}`);
        // `before: null` = HEAD 里从来没有过这个文件（未发布）；空串是「有，但内容是空的」。
        const tracked = await repo.trackedFile(repoRoot, `${scope}/${rel}`);
        return send(res, 200, {
          path: rel,
          before: tracked ? diff.base : null,
          after: diff.missing ? null : diff.text,
        });
      }
      if (route[1] === "commit") {
        return send(res, 200, await repo.pendingCommit(root, url.searchParams.get("hash")));
      }
      return send(res, 200, await repo.pendingState(repoRoot, { unitKind: "preset", scope }));
    }

    if (route[0] === "sync") {
      if (method !== "POST") return send(res, 405, { error: "同步只有 POST" });
      const body = await parseJsonBody(req);
      const paths = Array.isArray(body.paths) ? body.paths.map(String) : null;
      const result = await repo.serializeRepoWrite(repoRoot, () =>
        repo.syncRepo(repoRoot, String(body.message ?? "").trim() || null, {
          paths,
          scope,
          label: "预设",
          tag: "session",
        }),
      );
      await reloadRoster(presets);
      return send(res, 200, await writeResult(repo, repoRoot, result));
    }

    if (route[0] === "reload") {
      if (method !== "POST") return send(res, 405, { error: "重读只有 POST" });
      return send(
        res,
        200,
        await repo.serializeRepoWrite(repoRoot, async () => {
          const error = await reloadRoster(presets);
          const roster = presets.list();
          return error ? { ok: false, error, presets: roster } : { ok: true, presets: roster };
        }),
      );
    }

    return send(res, 404, { error: "presets 没有这条路", path: route.join("/") });
  } catch (error) {
    return fail(res, error);
  }
}

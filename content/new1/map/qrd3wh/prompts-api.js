// `/v1/prompts*` —— 提示词私库的 HTTP 面。
//
// 跟 `skills-api.js` 是同一个模子（连回滚 / 闸 / 串行锁的顺序都一样），差别只有两处语义：
//
//   · 一条模板 = `PROMPT.md`（正文）+ `nous.json`（名字 / 一句话摘要 / 类别）。
//     技能那边 `description` 在 frontmatter 里，因为模型要靠它决定用不用；模板这边正文
//     整段进提示词，**摘要只给挑模板的人看**，所以它住在 `nous.json`。这里没有 frontmatter 解析。
//   · `settings.json` 的 `default` 指向哪条模板：它必须指向一条真存在的模板，由 `index.mjs`
//     在每次写盘 / 发布时校验（见 `regenIndex`），这里只负责读出来给页面和建会话用。
//
// 一条铁律：**仓库是唯一真相。** 这里不维护第二份名单、不读 `data/`、不联网、不「安装」。
// 盘上有这个目录 = 装着；HEAD 里有这个目录 = 已发布。
//
// 约定（跟 skills-api 同一套）：
//   · 统一错误信封 `{ error }` + 状态码：400 校验 / 409 冲突 / 404 不在 / 500 内部。
//   · 每个写端点都在**本仓的**串行锁里（`serializeRepoWrite(root, …)`），写完 `prompts.load()`。
//     不 load 的话面板显示新模板、会话里还看不见它 —— 两份真相就是这么长出来的。
//   · `publish` = 内容闸 + 重扫 index.json + commit + push，一步；闸不过不动 index.json。

import { mkdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";

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

// ── 模板的名字 / 命名规矩 ───────────────────────────────────────────────────

/** 小写、≤64 字：名字就是目录名、就是会话里的键（`prompt:<名>`），新建那一刻定型，不许改。 */
const PROMPT_NAME_RE = /^[a-z0-9][a-z0-9._-]*$/;
const MAX_NAME = 64;

/** 仓库根下不是模板的名字（撞上它们 = 把索引 / 词表 / 设置当模板编辑）。 */
const RESERVED = new Set([
  "categories.json",
  "index.json",
  "index.mjs",
  "settings.json",
  "PROMPT.md",
  "nous.json",
  "README.md",
  "AGENTS.md",
  "test",
  "node_modules",
]);

const checkName = (raw) => {
  const name = String(raw ?? "").trim();
  if (!name) throw withStatus(new Error("模板名不能为空"), 400);
  if (name.length > MAX_NAME) throw withStatus(new Error(`模板名太长（最多 ${MAX_NAME} 字）：${name}`), 400);
  if (!PROMPT_NAME_RE.test(name)) {
    throw withStatus(new Error(`模板名只认小写字母、数字、点、下划线、连字符，且以字母或数字开头：${name}`), 400);
  }
  if (RESERVED.has(name)) throw withStatus(new Error(`「${name}」是仓库自己的文件名，不能当模板名`), 400);
  return name;
};

// ── 内容闸 ─────────────────────────────────────────────────────────────────

/** 内容闸：一条模板对不对。返回问题清单（空 = 能发布）。 */
export const checkPromptContent = async (root, name, dir = null) => {
  const home = dir ?? path.join(root, name);
  const problems = [];
  let body = null;
  try {
    body = await readFile(path.join(home, "PROMPT.md"), "utf8");
  } catch {
    problems.push(`${name}: 没有 PROMPT.md`);
  }
  // 空模板选中等于没选 —— 它会往提示词里塞一段空白，而人以为自己选了规则。
  if (body !== null && !body.trim()) problems.push(`${name}: PROMPT.md 是空的`);

  let manifest = null;
  try {
    manifest = JSON.parse(await readFile(path.join(home, "nous.json"), "utf8"));
  } catch {
    problems.push(`${name}: nous.json 读不出来（不存在或不是合法 JSON）`);
  }
  if (manifest) {
    if (manifest.name !== name) problems.push(`${name}: nous.json 里的 name 是「${manifest.name}」，跟目录名对不上`);
    // summary 是人在选择器里看到的唯一一句话；正文 7KB，没有摘要等于闭眼选。
    if (typeof manifest.summary !== "string" || !manifest.summary.trim()) {
      problems.push(`${name}: nous.json 的 summary 为空 —— 人靠这一行决定选不选它`);
    }
    if (!Array.isArray(manifest.category)) problems.push(`${name}: nous.json 的 category 得是数组（没有就写 []）`);
  }
  return problems;
};

/** 仓库里所有带 PROMPT.md 的目录 —— 内容闸扫这一圈（没有 PROMPT.md 的目录静默跳过）。 */
const gateRepo = async (repo, root) => {
  const problems = [];
  let dirs = [];
  try {
    dirs = await repo.listDirs(root);
  } catch {
    return []; // 仓库都不在：没东西可发布，也就没东西可拦
  }
  for (const dir of dirs) {
    if (!(await stat(path.join(root, dir, "PROMPT.md")).catch(() => null))) continue;
    problems.push(...(await checkPromptContent(root, dir)));
  }
  return problems;
};

const gateMessage = (problems) => {
  const shown = problems.slice(0, 4).join("；");
  return problems.length > 4 ? `${shown}；等 ${problems.length} 项` : shown;
};

// ── 词表 / 生成器 ───────────────────────────────────────────────────────────

const promptsIndex = (repo, root) => () => repo.regenIndex(root, { required: false });

// 模板仓**没有**受保护的类别：词表那套工具默认护着扩展仓的 Basics，所以每个调用点都得
// 显式说「一个都不护」。
const NO_PROTECT = { protect: [] };

// ── 响应里那几块公共拼装 ────────────────────────────────────────────────────

const commitOf = async (repo, root, result) => {
  if (!result.committed) return null;
  const info = await repo.lastCommitInfo(root);
  return { hash: info.hash, subject: info.subject };
};

const writeResult = async (repo, root, result) => ({
  // `ok` 的语义是「这一步做完了」，**不是**「推上去了」：没东西可提交和推成功都算做完了；
  // 只有「提交了但没推成」才是 false（那时另有 `error`）。
  ok: result.committed ? result.pushed !== false : true,
  commit: await commitOf(repo, root, result),
  pushed: result.pushed === true,
  ...(result.pushError ? { error: String(result.pushError).slice(0, 400) } : {}),
});

// index.json 的快照 / 复原（半截不留）。发布 / 归档 / 恢复都是「重扫 → commit → push」
// 三步，第一步就已经改了 index.json；后面任一步翻车，盘上会留一份「索引说改了、git 里
// 没有」的账。所以进 try 之前把原文抄下来，失败时原样写回。
const snapshotIndex = async (root) => {
  const file = path.join(root, "index.json");
  return { file, text: await readFile(file, "utf8").catch(() => null) };
};

const restoreIndex = async ({ file, text }) => {
  if (text === null) await rm(file, { force: true }).catch(() => {});
  else await writeFile(file, text, "utf8").catch(() => {});
};

/** 写完盘让内存名册跟上。跟不上的话面板说有、会话里没有。 */
const reloadRoster = async (prompts) => {
  try {
    await prompts.load();
    return null;
  } catch (error) {
    return errorLine(error);
  }
};

// ── 各端点 ─────────────────────────────────────────────────────────────────

/** `settings.json` 只读一个字段，坏了也别让整页 500 —— 报出来，页面自己决定怎么显示。 */
const readSettings = async (root) => {
  try {
    const parsed = JSON.parse(await readFile(path.join(root, "settings.json"), "utf8"));
    return { default: typeof parsed?.default === "string" ? parsed.default : null, settingsError: null };
  } catch (error) {
    return { default: null, settingsError: errorLine(error) };
  }
};

const overview = async (repo, root, prompts) => {
  let state = { head: null, dirty: false, ahead: 0, behind: 0 };
  let error = null;
  try {
    state = await repo.repoState(root);
  } catch (caught) {
    // 仓库读不了要给页面一句话，不是 500 —— 白屏说不清「是仓库没了还是模板没了」。
    error = errorLine(caught);
  }
  const [remote, tracked, vocab, archived, roster, settings] = await Promise.all([
    repo.remoteUrl(root),
    repo.trackedDirs(root),
    repo.readCategories(root, NO_PROTECT),
    repo.archivedEntries(root),
    Promise.resolve()
      .then(() => prompts.list())
      .catch(() => []),
    readSettings(root),
  ]);
  // 真相是**两份取并集**：词表里的（可能有零计数的空类别）+ 每条模板自己挂着的。
  const categories = repo.sortCategories([
    ...vocab,
    ...(roster ?? []).flatMap((prompt) => (Array.isArray(prompt.category) ? prompt.category : [])),
  ]);
  return {
    root,
    repo: { ...state, remote, error },
    // `tracked: false` + 目录在 = 未发布。跟面板上的琥珀虚线是同一个判据。
    prompts: (roster ?? []).map((prompt) => ({ ...prompt, tracked: tracked.has(prompt.name) })),
    archived: archived.map((entry) => entry.name).sort((a, b) => a.localeCompare(b, "zh-CN")),
    categories,
    default: settings.default,
    ...(settings.settingsError ? { settingsError: settings.settingsError } : {}),
  };
};

const createPrompt = async (repo, root, body) => {
  const name = checkName(body.name);
  const summary = String(body.summary ?? "").trim();
  if (!summary) throw withStatus(new Error(`${name}: summary 不能空 —— 人靠这一行决定选不选它`), 400);
  const categories = repo.sortCategories(repo.normalizeCategories(body.category ?? []));
  // 正文可以由调用方给；不给就落一个标题。**标题也会进提示词** —— 这是新建模板唯一
  // 「先有个能用的东西、但不是空壳」的落点，页面上紧接着就该打开编辑器。
  const text =
    typeof body.body === "string" && body.body.trim() ? body.body : `# ${name}\n`;

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
    await writeFile(path.join(staging, "PROMPT.md"), text, "utf8");
    await writeFile(
      path.join(staging, "nous.json"),
      JSON.stringify({ name, summary, category: categories }, null, 2) + "\n",
      "utf8",
    );

    // 先自己过一遍闸：一个建出来就装载失败的目录，等于让人工去猜哪里写错了。
    const problems = await checkPromptContent(root, name, staging);
    if (problems.length) throw withStatus(new Error(gateMessage(problems)), 400);

    if (categories.length) await repo.writeCategories(root, [...beforeVocab, ...categories], NO_PROTECT);
    await rename(staging, path.join(root, name));
    landed = true;
    await repo.regenIndex(root, { required: false });
  } catch (error) {
    if (landed) await rm(path.join(root, name), { recursive: true, force: true }).catch(() => {});
    await rm(staging, { recursive: true, force: true }).catch(() => {});
    if (categories.length) await repo.writeCategories(root, beforeVocab, NO_PROTECT).catch(() => {});
    throw error;
  }
  return { ok: true, name };
};

const archivePrompt = async (repo, root, name) => {
  if (!(await stat(path.join(root, name)).catch(() => null))) {
    throw withStatus(new Error(`工作区里没有 ${name}/`), 404);
  }
  const snapshot = await snapshotIndex(root);
  try {
    // 未发布的也搬进 .archive/：那是「删掉的东西不真删」，只有 purge 才真没。
    await repo.moveIntoArchive(root, name);
    await repo.regenIndex(root, { required: false });
    return await repo.syncRepo(root, `prompt: 归档 ${name}`, { includeDeleted: true, label: "模板", tag: "prompt" });
  } catch (error) {
    await repo.run("git", ["-C", root, "reset", "-q", "--", name, `${repo.ARCHIVE_DIR}/${name}`]).catch(() => {});
    await repo.moveDir(path.join(root, repo.ARCHIVE_DIR, name), path.join(root, name)).catch(() => {});
    await restoreIndex(snapshot);
    throw error.status ? error : withStatus(new Error(`归档失败，已回滚：${errorLine(error)}`), 400);
  }
};

const restorePrompt = async (repo, root, name) => {
  const snapshot = await snapshotIndex(root);
  try {
    await repo.moveOutOfArchive(root, name);
    await repo.regenIndex(root, { required: false });
    return await repo.syncRepo(root, `prompt: 从归档恢复 ${name}`, { includeDeleted: true, label: "模板", tag: "prompt" });
  } catch (error) {
    await repo.run("git", ["-C", root, "reset", "-q", "--", name, `${repo.ARCHIVE_DIR}/${name}`]).catch(() => {});
    await repo.moveDir(path.join(root, name), path.join(root, repo.ARCHIVE_DIR, name)).catch(() => {});
    await restoreIndex(snapshot);
    throw error.status ? error : withStatus(new Error(`恢复失败，已回滚：${errorLine(error)}`), 400);
  }
};

const purgePrompt = async (repo, root, name) => {
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
    return await repo.syncRepo(root, `prompt: 彻底删除 ${name}`, {
      includeDeleted: true,
      label: "模板",
      tag: "prompt",
    });
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
 * 处理 `/v1/prompts*`。`res` 由这里写，返回值没人看。
 *
 * `repo` = `repo-tools.js` 的整包，每个函数都收 `root`。测试递同一包、换一个 `root`
 * 就是另一个仓。
 */
export async function handlePrompts({ req, res, url, parts, root, prompts, repo }) {
  const method = req.method ?? "GET";
  const at = parts.slice(2);

  try {
    if (!at.length) {
      if (method !== "GET") return send(res, 405, { error: "模板仓库只有 GET" });
      return send(res, 200, await overview(repo, root, prompts));
    }

    if (at[0] === "categories") {
      if (method === "GET") return send(res, 200, { categories: await repo.readCategories(root, NO_PROTECT) });
      if (method !== "POST") return send(res, 405, { error: "类别词表只有 GET / POST" });
      const body = await parseJsonBody(req);
      const result = await repo.serializeRepoWrite(root, () =>
        repo.mutateCategories(
          root,
          { op: body.op, name: body.name, to: body.to },
          { index: promptsIndex(repo, root), ...NO_PROTECT },
        ),
      );
      const loadError = await reloadRoster(prompts);
      return send(res, 200, {
        categories: result.categories,
        prompts: prompts.list(),
        touched: result.touched,
        ...(loadError ? { loadError } : {}),
      });
    }

    if (at[0] === "category") {
      if (method !== "POST") return send(res, 405, { error: "单条模板的类别只有 POST" });
      const body = await parseJsonBody(req);
      const name = checkName(body.name);
      await repo.serializeRepoWrite(root, () =>
        repo.setNodeCategories(root, name, body.category ?? [], {
          label: "模板",
          index: promptsIndex(repo, root),
          ...NO_PROTECT,
        }),
      );
      const loadError = await reloadRoster(prompts);
      const roster = prompts.list();
      return send(res, 200, {
        prompt: roster.find((prompt) => prompt.name === name) ?? null,
        prompts: roster,
        categories: await repo.readCategories(root, NO_PROTECT),
        ...(loadError ? { loadError } : {}),
      });
    }

    if (at[0] === "files") {
      if (method !== "GET") return send(res, 405, { error: "文件列表只有 GET" });
      const name = checkName(url.searchParams.get("name"));
      const files = await repo.listRepoFiles(root, name, { kinds: null, maxFiles: 200, label: "模板" });
      return send(res, 200, { name, files: files.map((file) => ({ path: file.path, size: file.size })) });
    }

    if (at[0] === "file") {
      if (method === "GET") {
        const name = checkName(url.searchParams.get("name"));
        const rel = String(url.searchParams.get("path") ?? "");
        const file = await repo.readRepoFile(root, name, rel, { allowAny: true, label: "模板" });
        return send(res, 200, { name: file.name, path: file.path, text: file.text, etag: file.etag });
      }
      if (method !== "PUT") return send(res, 405, { error: "文件只有 GET / PUT" });
      const body = await parseJsonBody(req);
      const name = checkName(body.name);
      const saved = await repo.serializeRepoWrite(root, () =>
        repo.writeRepoFile(root, {
          name,
          rel: body.path,
          text: body.text,
          base: body.base,
          allowAny: true,
          // 模板的闸是「正文非空 + 清单字段」，在 publish 时才拦；不是 `node --check`。
          checkCode: false,
          strictIndex: false,
          label: "模板",
        }),
      );
      await reloadRoster(prompts);
      return send(res, 200, { etag: saved.etag });
    }

    if (at[0] === "create") {
      if (method !== "POST") return send(res, 405, { error: "新建只有 POST" });
      const body = await parseJsonBody(req);
      await repo.serializeRepoWrite(root, () => createPrompt(repo, root, body));
      const loadError = await reloadRoster(prompts);
      const roster = prompts.list();
      return send(res, 200, {
        prompt: roster.find((prompt) => prompt.name === String(body.name ?? "").trim()) ?? null,
        prompts: roster,
        ...(loadError ? { loadError } : {}),
      });
    }

    if (at[0] === "publish") {
      if (method !== "POST") return send(res, 405, { error: "发布只有 POST" });
      const body = await parseJsonBody(req);
      const result = await repo.serializeRepoWrite(root, async () => {
        const snapshot = await snapshotIndex(root);
        try {
          // 闸在最前面：不过就一个字节都不动（index.json 也不重扫，否则会留下「扫了一半」的假象）。
          const problems = await gateRepo(repo, root);
          if (problems.length) throw withStatus(new Error(gateMessage(problems)), 400);
          // 重扫顺带校验 `settings.json` 的 default 指向一条真存在的模板。
          await repo.regenIndex(root, { required: false });
          return await repo.syncRepo(root, String(body.message ?? "").trim() || "prompt: 更新模板仓库", {
            label: "模板",
            tag: "prompt",
          });
        } catch (error) {
          await restoreIndex(snapshot);
          throw error;
        }
      });
      const loadError = await reloadRoster(prompts);
      const warnings = [];
      if (result.skippedIndex?.length) {
        warnings.push(`未发布（HEAD 里没有）的没跟着这次提交走：${result.skippedIndex.join("、")}`);
      }
      if (loadError) warnings.push(`已提交，但装载失败：${loadError}`);
      return send(res, 200, { ...(await writeResult(repo, root, result)), warnings });
    }

    if (at[0] === "archive" || at[0] === "restore" || at[0] === "purge") {
      if (method !== "POST") {
        return send(res, 405, {
          error: `${{ archive: "归档", restore: "恢复", purge: "彻底删除" }[at[0]]}只有 POST`,
        });
      }
      const body = await parseJsonBody(req);
      const name = checkName(body.name);
      const task = { archive: archivePrompt, restore: restorePrompt, purge: purgePrompt }[at[0]];
      const result = await repo.serializeRepoWrite(root, () => task(repo, root, name));
      await reloadRoster(prompts);
      return send(res, 200, { ...(await writeResult(repo, root, result)), name });
    }

    if (at[0] === "pending") {
      if (method !== "GET") return send(res, 405, { error: "待同步清单只有 GET" });
      if (at[1] === "diff") {
        const diff = await repo.pendingFileDiff(root, url.searchParams.get("path"));
        // `before: null` = HEAD 里从来没有过这个文件（未发布）；空串是「有，但内容是空的」。
        const tracked = await repo.trackedFile(root, diff.path);
        return send(res, 200, { path: diff.path, before: tracked ? diff.base : null, after: diff.missing ? null : diff.text });
      }
      if (at[1] === "commit") {
        return send(res, 200, await repo.pendingCommit(root, url.searchParams.get("hash")));
      }
      return send(res, 200, await repo.pendingState(root, { unitKind: "prompt" }));
    }

    if (at[0] === "sync") {
      if (method !== "POST") return send(res, 405, { error: "同步只有 POST" });
      const body = await parseJsonBody(req);
      const paths = Array.isArray(body.paths) ? body.paths.map(String) : null;
      const result = await repo.serializeRepoWrite(root, () =>
        repo.syncRepo(root, String(body.message ?? "").trim() || null, { paths, label: "模板", tag: "prompt" }),
      );
      await reloadRoster(prompts);
      return send(res, 200, await writeResult(repo, root, result));
    }

    if (at[0] === "reload") {
      if (method !== "POST") return send(res, 405, { error: "重读只有 POST" });
      return send(res, 200, await repo.serializeRepoWrite(root, async () => {
        const error = await reloadRoster(prompts);
        const roster = prompts.list();
        return error ? { ok: false, error, prompts: roster } : { ok: true, prompts: roster };
      }));
    }

    return send(res, 404, { error: "prompts 没有这条路", path: at.join("/") });
  } catch (error) {
    return fail(res, error);
  }
}

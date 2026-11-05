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

// ── frontmatter：只读 `name` / `description` 这两个标量 ─────────────────────
//
// 为什么这儿还有一份解析：**内容闸是服务端的，不能挂在扩展模块上**。
// 它是发布前的最后一道门（§6.4），测试也要能直接跑它 —— 所以不 import
// `Nous-extend/skills/index.js`（那个文件的路径由 NOUS_EXTEND 决定，测试环境里不一定对）。
// 只认 `key: value`（和引号包裹的标量）：写错了就该红，而不是被宽容地猜过去。

export const parseFrontmatter = (text) => {
  const match = /^\uFEFF?---[ \t]*\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/.exec(String(text ?? ""));
  if (!match) return undefined;
  const out = {};
  for (const line of match[1].split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const at = trimmed.indexOf(":");
    if (at === -1) continue;
    const key = trimmed.slice(0, at).trim();
    let value = trimmed.slice(at + 1).trim();
    const quote = value[0];
    if (value.length > 1 && (quote === '"' || quote === "'") && value.endsWith(quote)) value = value.slice(1, -1);
    out[key] = value;
  }
  return out;
};

/** 内容闸：一条技能对不对。返回问题清单（空 = 能发布）。 */
export const checkSkillContent = async (root, name, dir = null) => {
  const home = dir ?? path.join(root, name);
  const problems = [];
  let text = null;
  try {
    text = await readFile(path.join(home, "SKILL.md"), "utf8");
  } catch {
    problems.push(`${name}: 没有 SKILL.md`);
  }
  if (text !== null) {
    const front = parseFrontmatter(text);
    if (!front) problems.push(`${name}: SKILL.md 开头没有 frontmatter（两行 --- 包起来的那一段）`);
    else {
      const declared = String(front.name ?? "").trim();
      if (declared !== name) problems.push(`${name}: frontmatter 里的 name 是「${declared}」，跟目录名对不上`);
      if (!String(front.description ?? "").trim()) problems.push(`${name}: description 是空的`);
    }
  }
  let manifest = null;
  try {
    manifest = JSON.parse(await readFile(path.join(home, "nous.json"), "utf8"));
  } catch {
    problems.push(`${name}: nous.json 读不出来（不存在或不是合法 JSON）`);
  }
  if (manifest) {
    if (manifest.name !== name) problems.push(`${name}: nous.json 里的 name 是「${manifest.name}」，跟目录名对不上`);
    if (typeof manifest.summary !== "string") problems.push(`${name}: nous.json 缺 summary（可以写空串，但得有）`);
    if (!Array.isArray(manifest.category)) problems.push(`${name}: nous.json 的 category 得是数组（没有就写 []）`);
  }
  return problems;
};

/** 仓库里所有带 SKILL.md 的目录 —— 内容闸扫这一圈（没有 SKILL.md 的目录静默跳过）。 */
const gateRepo = async (repo, root) => {
  const problems = [];
  let dirs = [];
  try {
    dirs = await repo.listDirs(root);
  } catch {
    return []; // 仓库都不在：没东西可发布，也就没东西可拦
  }
  for (const dir of dirs) {
    if (!(await stat(path.join(root, dir, "SKILL.md")).catch(() => null))) continue;
    problems.push(...(await checkSkillContent(root, dir)));
  }
  return problems;
};

const gateMessage = (problems) => {
  const shown = problems.slice(0, 4).join("；");
  return problems.length > 4 ? `${shown}；等 ${problems.length} 项` : shown;
};

// ── 词表 / 生成器 ───────────────────────────────────────────────────────────

const skillsIndex = (repo, root) => () => repo.regenIndex(root, { required: false });

// ── 响应里那几块公共拼装 ────────────────────────────────────────────────────

const commitOf = async (repo, root, result) => {
  if (!result.committed) return null;
  const info = await repo.lastCommitInfo(root);
  return { hash: info.hash, subject: info.subject };
};

const writeResult = async (repo, root, result) => ({
  ok: result.pushed !== false,
  commit: await commitOf(repo, root, result),
  pushed: result.pushed === true,
  ...(result.pushError ? { error: String(result.pushError).slice(0, 400) } : {}),
});

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
  const [remote, tracked, categories, archived, roster] = await Promise.all([
    repo.remoteUrl(root),
    repo.trackedDirs(root),
    repo.readCategories(root),
    repo.archivedEntries(root),
    Promise.resolve()
      .then(() => skills.list())
      .catch(() => []),
  ]);
  return {
    root,
    repo: { ...state, remote, error },
    // `tracked: false` + 目录在 = 未发布。跟面板上的琥珀虚线是同一个判据。
    skills: (roster ?? []).map((skill) => ({ ...skill, tracked: tracked.has(skill.name) })),
    archived: archived.map((entry) => entry.name).sort((a, b) => a.localeCompare(b, "zh-CN")),
    categories,
  };
};

const createSkill = async (repo, root, body) => {
  const name = checkName(body.name);
  const description = String(body.description ?? "").trim();
  if (!description) throw withStatus(new Error(`${name}: description 不能空 —— 模型靠它判断该不该用这条技能`), 400);
  const categories = repo.sortCategories(repo.normalizeCategories(body.category ?? []));

  if (await stat(path.join(root, name)).catch(() => null)) {
    throw withStatus(new Error(`${name}/ 已经在了 —— 换个名字，或者直接去改它`), 400);
  }
  if (await stat(path.join(root, repo.ARCHIVE_DIR, name)).catch(() => null)) {
    throw withStatus(new Error(`${name}/ 在 .archive/ 里 —— 点「恢复」把它捡回来，别建第二个`), 400);
  }

  const beforeVocab = await repo.readCategories(root);
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
    const problems = await checkSkillContent(root, name, staging);
    if (problems.length) throw withStatus(new Error(gateMessage(problems)), 400);

    if (categories.length) await repo.writeCategories(root, [...beforeVocab, ...categories]);
    await rename(staging, path.join(root, name));
    landed = true;
    await repo.regenIndex(root, { required: false });
  } catch (error) {
    if (landed) await rm(path.join(root, name), { recursive: true, force: true }).catch(() => {});
    await rm(staging, { recursive: true, force: true }).catch(() => {});
    if (categories.length) await repo.writeCategories(root, beforeVocab).catch(() => {});
    throw error;
  }
  return { ok: true, name };
};

const archiveSkill = async (repo, root, name) => {
  if (!(await stat(path.join(root, name)).catch(() => null))) {
    throw withStatus(new Error(`工作区里没有 ${name}/`), 404);
  }
  try {
    // 未发布的也搬进 .archive/：那是「删掉的东西不真删」（I10），只有 purge 才真没。
    await repo.moveIntoArchive(root, name);
    await repo.regenIndex(root, { required: false });
    return await repo.syncRepo(root, `skill: 归档 ${name}`, { includeDeleted: true, label: "技能", tag: "skill" });
  } catch (error) {
    await repo.run("git", ["-C", root, "reset", "-q", "--", name, `${repo.ARCHIVE_DIR}/${name}`]).catch(() => {});
    await repo.moveDir(path.join(root, repo.ARCHIVE_DIR, name), path.join(root, name)).catch(() => {});
    throw error.status ? error : withStatus(new Error(`归档失败，已回滚：${errorLine(error)}`), 400);
  }
};

const restoreSkill = async (repo, root, name) => {
  try {
    await repo.moveOutOfArchive(root, name);
    await repo.regenIndex(root, { required: false });
    return await repo.syncRepo(root, `skill: 从归档恢复 ${name}`, { includeDeleted: true, label: "技能", tag: "skill" });
  } catch (error) {
    await repo.run("git", ["-C", root, "reset", "-q", "--", name, `${repo.ARCHIVE_DIR}/${name}`]).catch(() => {});
    await repo.moveDir(path.join(root, name), path.join(root, repo.ARCHIVE_DIR, name)).catch(() => {});
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
export async function handleSkills({ req, res, url, parts, root, skills, repo }) {
  const method = req.method ?? "GET";
  const at = parts.slice(2);

  try {
    if (!at.length) {
      if (method !== "GET") return send(res, 405, { error: "技能仓库只有 GET" });
      return send(res, 200, await overview(repo, root, skills));
    }

    if (at[0] === "categories") {
      if (method === "GET") return send(res, 200, { categories: await repo.readCategories(root) });
      if (method !== "POST") return send(res, 405, { error: "类别词表只有 GET / POST" });
      const body = await parseJsonBody(req);
      const result = await repo.serializeRepoWrite(root, () =>
        repo.mutateCategories(root, { op: body.op, name: body.name, to: body.to }, { index: skillsIndex(repo, root) }),
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
        repo.setNodeCategories(root, name, body.category ?? [], { label: "技能", index: skillsIndex(repo, root) }),
      );
      const loadError = await reloadRoster(skills);
      const roster = skills.list();
      return send(res, 200, {
        skill: roster.find((skill) => skill.name === name) ?? null,
        skills: roster,
        categories: await repo.readCategories(root),
        ...(loadError ? { loadError } : {}),
      });
    }

    if (at[0] === "files") {
      if (method !== "GET") return send(res, 405, { error: "文件列表只有 GET" });
      const name = checkName(url.searchParams.get("name"));
      const files = await repo.listRepoFiles(root, name, { kinds: null, maxFiles: 200, label: "技能" });
      return send(res, 200, { name, files: files.map((file) => ({ path: file.path, size: file.size })) });
    }

    if (at[0] === "file") {
      if (method === "GET") {
        const name = checkName(url.searchParams.get("name"));
        const rel = String(url.searchParams.get("path") ?? "");
        const file = await repo.readRepoFile(root, name, rel, { allowAny: true, label: "技能" });
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
          // 技能里的闸是 frontmatter（publish 时才拦），不是 `node --check`。
          checkCode: false,
          strictIndex: false,
          label: "技能",
        }),
      );
      await reloadRoster(skills);
      return send(res, 200, { etag: saved.etag });
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

    if (at[0] === "publish") {
      if (method !== "POST") return send(res, 405, { error: "发布只有 POST" });
      const body = await parseJsonBody(req);
      const result = await repo.serializeRepoWrite(root, async () => {
        // 闸在最前面：不过就一个字节都不动（index.json 也不重扫，否则会留下「扫了一半」的假象）。
        const problems = await gateRepo(repo, root);
        if (problems.length) throw withStatus(new Error(gateMessage(problems)), 400);
        await repo.regenIndex(root, { required: false });
        return repo.syncRepo(root, String(body.message ?? "").trim() || "skill: 更新技能仓库", {
          label: "技能",
          tag: "skill",
        });
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
      const result = await repo.serializeRepoWrite(root, () => task(repo, root, name));
      await reloadRoster(skills);
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

    return send(res, 404, { error: "skills 没有这条路", path: at.join("/") });
  } catch (error) {
    return fail(res, error);
  }
}

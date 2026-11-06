// 技能变体：**一条技能多个可装载名字**（`tdd@strict`）。
//
// 设计照搬扩展那一套（`server.js` 的 `variantKey` / `createVariantWorktree`），因为两边
// 解决的是同一个问题：本体在 `main` 上，变体在**自己那条分支**上，一台机器可以同时绑好几个。
// 差别只有一处、而且是变体这个概念本身带来的：扩展的变体带**参数**（造扩展时递进去的那个包），
// 技能的变体只带**正文** —— 技能没有工厂函数，它的"参数"就是 `SKILL.md` 那几段字。
// 所以技能这边绑定的产物就是一棵装着另一个版本的树，不需要 `nous.json.variants` 那张表。
//
// 三个真相分得干干净净：
//   · 分支 + 那棵树 —— 变体的内容，**跟着仓库走**（push 上去，别的机器 `git worktree add` 就能拿）；
//   · `data/skills.local.json` 的绑定 —— **只在这台有意义**（路径是本机的，提交上去就是垃圾）；
//   · `data/` 是宿主的 watcher 不看的目录，写它不会把进程重启。
import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import path from "node:path";

import { errorDetail, gitRootOf, run, withStatus, writeFileAtomic } from "./repo-tools.js";

/** `本名@id` 的那个 `@`。跟扩展、跟 `Nous-extend/skills/index.js` 同一个字符。 */
export const VARIANT_SEP = "@";
export const VARIANT_ID_RE = /^[a-z0-9][a-z0-9._-]*$/;
const MAX_ID = 32;

export const variantKey = (family, id) => `${family}${VARIANT_SEP}${id}`;

/** `tdd@strict` → `{family:"tdd", id:"strict"}`；没有 `@` 的就是本体。 */
export const splitVariantKey = (key) => {
  const text = String(key ?? "");
  const at = text.indexOf(VARIANT_SEP);
  if (at <= 0) return { family: text, id: null };
  return { family: text.slice(0, at), id: text.slice(at + 1) };
};

export const assertVariantId = (raw) => {
  const id = String(raw ?? "").trim();
  if (!id) throw withStatus(new Error("变体 id 不能为空"), 400);
  if (id.length > MAX_ID) throw withStatus(new Error(`变体 id 太长（最多 ${MAX_ID} 字）：${id}`), 400);
  if (!VARIANT_ID_RE.test(id) || id.includes(VARIANT_SEP)) {
    throw withStatus(new Error(`变体 id 只认小写字母、数字、点、下划线、连字符：${id}`), 400);
  }
  return id;
};

/**
 * 变体那棵树放哪：跟扩展一个规矩 —— 放在**仓的隔壁**，不进仓里。
 * 放进仓里 git 会把它当 untracked 一整个目录报上来，`publish` 会顺手把它推上去。
 */
export const worktreePath = (skillsRoot, family, id) =>
  path.join(path.dirname(skillsRoot), `nous-skill-${family}-${id}`);

const defaultBranch = (family, id) => `skill/${family}-${id}`;

/**
 * 变体的本机绑定表。形状跟 `extensions.local.json` 一样（就是 `variants` 那一层）。
 * 读不出来就当没绑过 —— 一份坏 JSON 不该让整个面板打不开。
 */
export const createVariantStore = ({ dataDir, skillsRoot }) => {
  const file = path.join(dataDir, "skills.local.json");

  const read = async () => {
    try {
      const raw = JSON.parse(await readFile(file, "utf8"));
      const variants = raw?.variants;
      return variants && typeof variants === "object" && !Array.isArray(variants) ? variants : {};
    } catch {
      return {};
    }
  };

  const write = async (variants) => {
    await mkdir(dataDir, { recursive: true });
    const clean = {};
    for (const [family, map] of Object.entries(variants)) {
      if (map && Object.keys(map).length) clean[family] = map;
    }
    await writeFileAtomic(
      file,
      JSON.stringify(
        {
          _about: [
            "本机绑了哪些技能变体。`root` / `branch` 是本机路径和分支，**不该被提交**。",
            "变体不是复制：内容在那条分支上（跟着仓库走），这里只记「这台把它 checkout 在哪」。",
          ],
          v: 1,
          variants: clean,
        },
        null,
        2,
      ) + "\n",
    );
  };

  /** 给装载器用：`[{family, id, root}]`。树不见了的那条跳过（删了目录不该让整页空掉）。 */
  const list = async () => {
    const variants = await read();
    const out = [];
    for (const [family, map] of Object.entries(variants)) {
      for (const [id, bound] of Object.entries(map ?? {})) {
        const root = String(bound?.root ?? "");
        if (!root) continue;
        out.push({ family, id, root, branch: bound?.branch ?? null, exists: Boolean(await stat(root).catch(() => null)) });
      }
    }
    return out;
  };

  /** 能装的那几条：树不在的不喂给装载器（喂了只会得到一条 failed 卡）。 */
  const loadable = async () => (await list()).filter((entry) => entry.exists).map(({ family, id, root }) => ({ family, id, root }));

  /**
   * 绑一条变体 = 开一棵 worktree。分支已存在就 checkout 它（**别的机器 push 上来的变体，
   * 在这台绑一下就有了**）；不存在就 `-b` 开一条新的。
   *
   * 跟扩展那边同一个坑：分支是从 HEAD 起的，**没发布的技能在分支里根本不存在**。
   * 所以目标目录里没有本家那条时，把工作区那份拷过去并提交 —— 不然「给刚建的技能开变体」
   * 拿到的是一棵空树，而且 `git worktree remove` 见到 untracked 还会拒绝。
   */
  const bind = async ({ family, id, branch = "" }) => {
    const safeId = assertVariantId(id);
    const target = worktreePath(skillsRoot, family, safeId);
    if (await stat(target).catch(() => null)) {
      // 已经绑过、树也在：幂等 —— 重绑就是重新记一次，不重开树。
      const variants = await read();
      const bound = variants?.[family]?.[safeId];
      if (bound) return { root: target, branch: bound.branch ?? null, existed: true };
      throw withStatus(new Error(`${target} 已经存在但没绑过 —— 先拆掉它，或在命令行处理`), 409);
    }

    const wanted = String(branch ?? "").trim() || defaultBranch(family, safeId);
    if (!/^[A-Za-z0-9._/-]+$/.test(wanted) || wanted.startsWith("-") || wanted.includes("..")) {
      throw withStatus(new Error(`分支名不合法：${wanted}`), 400);
    }
    const hasBranch = await run("git", ["-C", skillsRoot, "rev-parse", "--verify", `refs/heads/${wanted}`], {
      timeout: 15_000,
    })
      .then(() => true)
      .catch(() => false);

    try {
      await run(
        "git",
        ["-C", skillsRoot, "worktree", "add", ...(hasBranch ? [] : ["-b", wanted]), target, ...(hasBranch ? [wanted] : [])],
        { timeout: 120_000 },
      );
    } catch (error) {
      throw withStatus(new Error(`开不出变体的树：${errorDetail(error)}`), 500);
    }

    // 没发布的本体在分支里不存在 —— 从工作区拷过去，让变体有个起点。
    const familyFrom = path.join(skillsRoot, family);
    const familyTo = path.join(target, family);
    if (!(await stat(familyTo).catch(() => null)) && (await stat(familyFrom).catch(() => null))) {
      const { cp } = await import("node:fs/promises");
      await cp(familyFrom, familyTo, { recursive: true });
      try {
        await run("git", ["-C", target, "add", "--", family], { timeout: 60_000 });
        await run(
          "git",
          ["-C", target, "commit", "-m", `skill: 变体 ${family}@${safeId} 起点（本体还没发布，从工作区拷入）`],
          { timeout: 60_000 },
        );
      } catch {
        // 起点提交失败不影响绑定：树是干净的，可以手动提交。
      }
    }

    const variants = await read();
    variants[family] = { ...(variants[family] ?? {}), [safeId]: { root: target, branch: wanted } };
    await write(variants);
    return { root: target, branch: wanted, existed: false };
  };

  /**
   * 解绑：**只摘记录，不动树也不动分支**。
   * 树里有没提交的正文时，删树就是把人的字删了 —— 那得是另一个显式动作。
   */
  const unbind = async ({ family, id }) => {
    const safeId = assertVariantId(id);
    const variants = await read();
    const familyMap = { ...(variants[family] ?? {}) };
    const bound = familyMap[safeId] ?? null;
    delete familyMap[safeId];
    const next = { ...variants };
    if (Object.keys(familyMap).length) next[family] = familyMap;
    else delete next[family];
    await write(next);
    return { bound, root: bound?.root ? String(bound.root) : null, branch: bound?.branch ?? null };
  };

  /**
   * 发布变体：在那棵树上 commit + push 到**它自己的分支**，主干一个字节都不碰。
   * 跟扩展的 `publishVariant` 是同一段逻辑，只是技能这边没有 `node --check` 那一步
   * （正文的内容闸在 `publish` 主路径上，按技能仓跑）。
   */
  const publish = async ({ family, id, message }) => {
    const safeId = assertVariantId(id);
    const variants = await read();
    const bound = variants?.[family]?.[safeId];
    if (!bound) throw withStatus(new Error(`${family}@${safeId} 没绑 —— 先绑一个变体`), 404);
    const root = String(bound.root);
    const gitRoot = await gitRootOf(root, root);
    const branch =
      bound.branch ??
      (await run("git", ["-C", gitRoot, "rev-parse", "--abbrev-ref", "HEAD"], { timeout: 15_000 }))
        .stdout.trim()
        .trim();
    if (!branch || branch === "HEAD" || branch === "main" || branch === "master") {
      throw withStatus(new Error(`变体那棵树在 ${branch || "游离 HEAD"} 上 —— 不在自己那条分支上，不敢发布`), 409);
    }

    const changed = (await run("git", ["-C", gitRoot, "status", "--porcelain"], { timeout: 20_000 })).stdout
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => line.slice(3).trim())
      .filter(Boolean);
    if (!changed.length) return { committed: false, branch, changed, pushed: false };

    await run("git", ["-C", gitRoot, "add", "-A"], { timeout: 60_000 });
    await run(
      "git",
      ["-C", gitRoot, "commit", "-m", String(message ?? "").trim() || `skill: ${family}@${safeId}`],
      { timeout: 60_000 },
    );
    const head = (await run("git", ["-C", gitRoot, "rev-parse", "HEAD"], { timeout: 15_000 })).stdout.trim();
    let pushed = false;
    let pushError = "";
    try {
      await run("git", ["-C", gitRoot, "push", "-u", "origin", branch], { timeout: 120_000 });
      pushed = true;
    } catch (error) {
      pushError = errorDetail(error);
    }
    return { committed: true, branch, changed, head, pushed, pushError };
  };

  return {
    file,
    read,
    list,
    loadable,
    bind,
    unbind,
    publish,
    rootOf: async (family, id) => {
      const bound = (await read())?.[family]?.[String(id)];
      return bound?.root ? String(bound.root) : null;
    },
    worktreePath: (family, id) => worktreePath(skillsRoot, family, id),
  };
};

/** 写 `nous.json` 里的 `variant` 那一格（新建变体时用；正文上的绑定不靠它）。 */
export const stampVariant = async (dir, { family, id }) => {
  const file = path.join(dir, "nous.json");
  const manifest = JSON.parse(await readFile(file, "utf8").catch(() => "{}"));
  await writeFile(file, JSON.stringify({ ...manifest, variant: { family, id } }, null, 2) + "\n", "utf8");
};

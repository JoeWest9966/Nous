// 技能变体：绑定 / 解绑 / 文件读写 / 发变体分支。
//
// 这里测的是**契约**：worktree 开在哪儿、绑定表长什么样、`tdd@strict` 的读写落到哪棵树、
// 发变体是不是只动它自己那条分支。装载器把变体并进名册那一半在
// `Nous-extend/test/skills.test.js` 里测（那是真装载器）。
//
// 夹具多一层：仓放在 `<tmp>/skills`，worktree 开在 `<tmp>/nous-skill-<family>-<id>` ——
// 跟生产一个形状（仓的隔壁），而且一次 `rm(<tmp>)` 全清。

import test from "node:test";
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { handleSkills } from "../skills-api.js";
import * as repo from "../repo-tools.js";
import { createVariantStore } from "../skills-variants.js";
import { CATEGORIES, MINI_INDEX, call, makeRepo, run, skillFiles, status } from "./helpers.js";

const exists = async (target) => Boolean(await stat(target).catch(() => null));

/**
 * 假的前端名册：本体 + 每一条绑定。真装载器的并集在 Nous-extend 那边测，
 * 这里只需要「`tdd@strict` 那一条的 `dir` 指向它自己那棵树」——`homeOf` 就是靠这一格找对盘的。
 */
const makeVariantSkills = (root, store) => {
  const state = { loads: 0, variants: [] };
  const list = () => {
    const out = [
      {
        name: "tdd",
        path: "tdd",
        dir: path.join(root, "tdd"),
        file: path.join(root, "tdd", "SKILL.md"),
        description: "本体",
        category: [],
        hidden: false,
        load: { state: "ok" },
      },
    ];
    for (const bound of state.variants) {
      const dir = path.join(bound.root, bound.family);
      out.push({
        name: `${bound.family}@${bound.id}`,
        path: bound.family,
        dir,
        file: path.join(dir, "SKILL.md"),
        description: "变体",
        category: [],
        hidden: false,
        load: { state: "ok" },
        variant: { family: bound.family, id: bound.id },
      });
    }
    return out;
  };
  return {
    dir: root,
    get loads() {
      return state.loads;
    },
    async load() {
      state.loads++;
      state.variants = await store.loadable();
      return list();
    },
    list,
    has: (name) => list().some((skill) => skill.name === name),
  };
};

/** 一个仓 + 一个绑定表 + 一个假名册；`data/` 就在仓的隔壁，跟生产一样分开。 */
const setup = async ({ files = null } = {}) => {
  const base = await mkdtemp(path.join(tmpdir(), "nous-skillvar-"));
  const root = path.join(base, "skills");
  await mkdir(root, { recursive: true });
  const made = await makeRepo({ files: { "categories.json": CATEGORIES, "index.mjs": MINI_INDEX, ...(files ?? skillFiles("tdd")) } });
  // makeRepo 自己开了临时目录：把它整棵搬进 base/skills，省得再造一遍 git。
  await run("cp", ["-a", `${made}/.`, root]);
  await rm(made, { recursive: true, force: true });

  const dataDir = path.join(base, "data");
  await mkdir(dataDir, { recursive: true });
  const store = createVariantStore({ dataDir, skillsRoot: root });
  const skills = makeVariantSkills(root, store);
  await skills.load();
  const api = {
    root,
    skills,
    store,
    call: (method, target, body) => call(handleSkills, { method, path: target, body, root, skills, repo, variants: store, reload: () => skills.load() }),
  };
  return { base, root, skills, store, api };
};

const readJson = async (file) => JSON.parse(await readFile(file, "utf8"));
const git = async (cwd, ...args) => (await run("git", ["-C", cwd, ...args])).stdout.trim();

test("bind：开一棵 worktree、记一笔绑定、名册多一个 `tdd@strict`", async () => {
  const { base, root, skills, api } = await setup();
  try {
    const before = skills.loads;
    const bound = await api.call("POST", "/v1/skills/variant", { op: "bind", name: "tdd", id: "strict" });
    assert.equal(bound.status, 200);
    assert.equal(bound.body.key, "tdd@strict");
    assert.equal(bound.body.branch, "skill/tdd-strict");

    // 树开在仓的隔壁（不是仓里 —— 放仓里会被 publish 顺手推上去）。
    const tree = path.join(base, "nous-skill-tdd-strict");
    assert.equal(await exists(tree), true);
    assert.equal(await exists(path.join(tree, "tdd", "SKILL.md")), true);
    assert.equal(await git(tree, "rev-parse", "--abbrev-ref", "HEAD"), "skill/tdd-strict");

    // 绑定表：本机私事，形状 `{variants: {family: {id: {root, branch}}}}`。
    const local = await readJson(path.join(base, "data", "skills.local.json"));
    assert.equal(local.variants.tdd.strict.branch, "skill/tdd-strict");
    assert.equal(local.variants.tdd.strict.root, tree);

    // 绑完必须重造名册（不重造就是「页面说有、会话里没有」）。
    assert.ok(skills.loads > before);
    assert.deepEqual(
      bound.body.skills.map((skill) => skill.name),
      ["tdd", "tdd@strict"],
    );

    // 主干的 HEAD 一个字节没动。
    assert.equal(await git(root, "rev-parse", "HEAD"), await git(tree, "rev-parse", "HEAD"));
  } finally {
    await rm(base, { recursive: true, force: true });
  }
});

test("绑定之后：`tdd@strict` 的读写落在它那棵树上，本体不动", async () => {
  const { base, root, api } = await setup();
  try {
    await api.call("POST", "/v1/skills/variant", { op: "bind", name: "tdd", id: "strict" });
    const tree = path.join(base, "nous-skill-tdd-strict");

    const read = await api.call("GET", "/v1/skills/file?name=tdd&variant=strict&path=SKILL.md");
    assert.equal(read.status, 200);
    assert.match(read.body.text, /name: tdd/);

    const saved = await api.call("PUT", "/v1/skills/file", {
      name: "tdd",
      variant: "strict",
      path: "SKILL.md",
      text: "---\nname: tdd\ndescription: 变体那版\n---\n\n# 变体正文\n",
    });
    assert.equal(saved.status, 200);
    assert.equal(saved.body.variant, "strict");

    // 变体那棵树里改了，本体那棵树没改 —— 这就是「一条技能两个名字」的全部意思。
    assert.match(await readFile(path.join(tree, "tdd", "SKILL.md"), "utf8"), /变体正文/);
    assert.doesNotMatch(await readFile(path.join(root, "tdd", "SKILL.md"), "utf8"), /变体正文/);
    // 改动停在变体那棵树的**自己那条分支**上，还没提交。
    assert.match(await status(tree), /^ M tdd\/SKILL\.md$/m);
    assert.equal(await status(root), "");
  } finally {
    await rm(base, { recursive: true, force: true });
  }
});

test("unbind：只摘记录，树和分支都留着（删字得是另一个显式动作）", async () => {
  const { base, skills, api } = await setup();
  try {
    await api.call("POST", "/v1/skills/variant", { op: "bind", name: "tdd", id: "strict" });
    const tree = path.join(base, "nous-skill-tdd-strict");

    const gone = await api.call("POST", "/v1/skills/variant", { op: "unbind", name: "tdd", id: "strict" });
    assert.equal(gone.status, 200);
    assert.equal(gone.body.root, tree);

    const local = await readJson(path.join(base, "data", "skills.local.json"));
    assert.deepEqual(local.variants, {});
    // 树还在、分支还在。
    assert.equal(await exists(path.join(tree, "tdd", "SKILL.md")), true);
    assert.deepEqual(
      skills.list().map((skill) => skill.name),
      ["tdd"],
    );
  } finally {
    await rm(base, { recursive: true, force: true });
  }
});

test("publish 带 `name=tdd@strict`：只提交 + 推变体那条分支，主干 HEAD 不动", async () => {
  const { base, root, api } = await setup();
  try {
    await api.call("POST", "/v1/skills/variant", { op: "bind", name: "tdd", id: "strict" });
    const tree = path.join(base, "nous-skill-tdd-strict");
    const mainHead = await git(root, "rev-parse", "HEAD");

    await api.call("PUT", "/v1/skills/file", {
      name: "tdd",
      variant: "strict",
      path: "SKILL.md",
      text: "---\nname: tdd\ndescription: 变体那版\n---\n\n# 变体正文\n",
    });

    const out = await api.call("POST", "/v1/skills/publish", {
      name: "tdd",
      variant: "strict",
      message: "skill: 变体的改动",
    });
    assert.equal(out.status, 200);
    assert.equal(out.body.committed, true);
    assert.equal(out.body.branch, "skill/tdd-strict");
    // 没有 origin：提交成功、推送失败，如实报（不是 500，也不是假装成功）。
    assert.equal(out.body.pushed, false);
    assert.ok(out.body.error);

    assert.equal(await git(tree, "log", "-1", "--pretty=%s"), "skill: 变体的改动");
    assert.equal(await status(tree), "");
    assert.equal(await git(root, "rev-parse", "HEAD"), mainHead, "主干一个字节都不该动");
  } finally {
    await rm(base, { recursive: true, force: true });
  }
});

test("GET /v1/skills/variant 列出这一条技能绑了哪些变体", async () => {
  const { base, api } = await setup();
  try {
    await api.call("POST", "/v1/skills/variant", { op: "bind", name: "tdd", id: "strict" });
    const listed = await api.call("GET", "/v1/skills/variant?name=tdd");
    assert.equal(listed.status, 200);
    assert.equal(listed.body.variants.length, 1);
    assert.equal(listed.body.variants[0].id, "strict");
    assert.equal(listed.body.variants[0].key, "tdd@strict");
    assert.equal(listed.body.variants[0].skill.name, "tdd@strict");
    assert.equal(listed.body.variants[0].branch, "skill/tdd-strict");
    // 没绑过的一条：空的，不是 404。
    const none = await api.call("GET", "/v1/skills/variant?name=other");
    assert.equal(none.status, 200);
    assert.deepEqual(none.body.variants, []);
    // 名字先得过关（checkName 在门口）。
    const bad = await api.call("GET", "/v1/skills/variant?name=Bad%20Name");
    assert.equal(bad.status, 400);
  } finally {
    await rm(base, { recursive: true, force: true });
  }
});

test("变体 id 不合法：400，且不开树、不写绑定", async () => {
  const { base, api } = await setup();
  try {
    const bad = await api.call("POST", "/v1/skills/variant", { op: "bind", name: "tdd", id: "Bad Id" });
    assert.equal(bad.status, 400);
    assert.match(bad.body.error, /Bad Id/);
    assert.equal(await exists(path.join(base, "nous-skill-tdd-Bad Id")), false);
    const local = await readJson(path.join(base, "data", "skills.local.json")).catch(() => ({ variants: {} }));
    assert.deepEqual(local.variants ?? {}, {});
  } finally {
    await rm(base, { recursive: true, force: true });
  }
});

test("没发布的技能也能开变体：从工作区把那条拷进树里当起点", async () => {
  const { base, root, api } = await setup();
  try {
    // 新建一条不提交的技能（未发布），再给它开变体。
    const made = await api.call("POST", "/v1/skills/create", { name: "fresh", description: "还没发布" });
    assert.equal(made.status, 200);
    await writeFile(path.join(root, "fresh", "SKILL.md"), "---\nname: fresh\ndescription: 还没发布\n---\n\n# fresh\n", "utf8");

    const out = await api.call("POST", "/v1/skills/variant", { op: "bind", name: "fresh", id: "dev" });
    assert.equal(out.status, 200);
    const tree = path.join(base, "nous-skill-fresh-dev");
    assert.equal(await exists(path.join(tree, "fresh", "SKILL.md")), true);
    // 起点已经提交过一笔 —— 不然 `git worktree remove` 会因为 untracked 拒绝拆树。
    assert.equal(await status(tree), "");
    assert.match(await git(tree, "log", "-1", "--pretty=%s"), /起点/);
  } finally {
    await rm(base, { recursive: true, force: true });
  }
});

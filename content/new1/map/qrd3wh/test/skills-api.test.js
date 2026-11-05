// `/v1/skills*` 的行为 —— 注入假的 skills service + 真的临时 git 仓，不起引擎。
//
// 测的是「契约」：状态码、响应体的字段名、盘上的结果、git 里的结果。
// 不测 DOM，也不 import `server.js`（它有顶层 await，import 一下就起服务）。

import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { handleSkills } from "../skills-api.js";
import * as repo from "../repo-tools.js";
import {
  CATEGORIES,
  MINI_INDEX,
  call,
  cleanup,
  exists,
  makeRepo,
  makeSkills,
  run,
  showNames,
  skillFiles,
  status,
} from "./helpers.js";

/** 一个能跑的技能仓：词表 + 生成器（+ 需要的话，预置几条技能）。 */
const makeSkillsRepo = async (files = {}) =>
  makeRepo({ files: { "categories.json": CATEGORIES, "index.mjs": MINI_INDEX, ...files } });

/** 仓 + 假 service（先 load 一次，名册才是热的 —— 跟真进程启动时一样）。 */
const setup = async (files = {}) => {
  const root = await makeSkillsRepo(files);
  const skills = makeSkills(root);
  await skills.load();
  return { root, skills };
};

const api = (root, skills) => ({
  root,
  skills,
  repo,
  call: (method, target, body) => call(handleSkills, { method, path: target, body, root, skills, repo }),
});

const read = (root, rel) => readFile(path.join(root, rel), "utf8");
const readJson = async (root, rel) => JSON.parse(await read(root, rel));

// ── 1. 空仓 ─────────────────────────────────────────────────────────────────

test("GET /v1/skills：空仓给的就是空仓，不是错误页", async () => {
  const { root, skills } = await setup();
  const api1 = api(root, skills);
  try {
    const { status: code, body } = await api1.call("GET", "/v1/skills");
    assert.equal(code, 200);
    assert.equal(body.root, root);
    assert.equal(body.repo.error, null);
    assert.match(body.repo.head, /^[0-9a-f]{40}$/);
    assert.deepEqual(body.skills, []);
    assert.deepEqual(body.archived, []);
    assert.deepEqual(body.categories, ["Basics"]);
  } finally {
    await cleanup(root);
  }
});

// ── 2. create ───────────────────────────────────────────────────────────────

test("POST /v1/skills/create：目录 + frontmatter，但**不提交**", async () => {
  const { root, skills } = await setup();
  const api1 = api(root, skills);
  try {
    const made = await api1.call("POST", "/v1/skills/create", { name: "fresh", description: "一句话说明" });
    assert.equal(made.status, 200);
    assert.equal(made.body.skill.name, "fresh");

    const front = await read(root, "fresh/SKILL.md");
    assert.match(front, /^---\n/);
    assert.match(front, /name: fresh/);
    assert.match(front, /description: "一句话说明"/);
    // 骨架只有 name + description + 一行标题，不写示例正文。
    assert.match(front, /\n# fresh\n/);

    // 没提交：目录还在工作区里没跟踪。
    assert.match(await status(root), /^\?\? fresh\/$/m);

    const overview = await api1.call("GET", "/v1/skills");
    const fresh = overview.body.skills.find((skill) => skill.name === "fresh");
    assert.equal(fresh.tracked, false, "刚建的技能必须是「未发布」");
    assert.equal(skills.loads > 0, true);
  } finally {
    await cleanup(root);
  }
});

// ── 3. 撞名 ─────────────────────────────────────────────────────────────────

test("POST /v1/skills/create 撞已有目录：400，且点得出撞了谁", async () => {
  const { root, skills } = await setup(skillFiles("taken"));
  const api1 = api(root, skills);
  try {
    const { status: code, body } = await api1.call("POST", "/v1/skills/create", { name: "taken", description: "x" });
    assert.equal(code, 400);
    assert.match(body.error, /taken/);
  } finally {
    await cleanup(root);
  }
});

test("POST /v1/skills/create 名字不合法：400，且不留下 .new-* 半截目录", async () => {
  const { root, skills } = await setup();
  const api1 = api(root, skills);
  try {
    const { status: code, body } = await api1.call("POST", "/v1/skills/create", { name: "Bad Name", description: "x" });
    assert.equal(code, 400);
    assert.match(body.error, /Bad Name/);
    const left = (await readdir(root)).filter((name) => name.startsWith(".new-"));
    assert.deepEqual(left, []);
  } finally {
    await cleanup(root);
  }
});

// ── 4. 类别 ─────────────────────────────────────────────────────────────────

test("POST /v1/skills/category：nous.json 改了、词表补齐祖先、index.json 重扫", async () => {
  const { root, skills } = await setup(skillFiles("cat"));
  const api1 = api(root, skills);
  try {
    const { status: code, body } = await api1.call("POST", "/v1/skills/category", {
      name: "cat",
      category: ["工具/搜索"],
    });
    assert.equal(code, 200);
    assert.deepEqual(body.skill.category, ["工具/搜索"]);
    assert.deepEqual(body.skill.load, { state: "ok" });

    assert.deepEqual((await readJson(root, "cat/nous.json")).category, ["工具/搜索"]);
    const vocab = (await readJson(root, "categories.json")).categories;
    assert.ok(vocab.includes("工具"), `词表里没有祖先「工具」：${vocab.join(",")}`);
    assert.ok(vocab.includes("工具/搜索"));
    // index.json 是生成器扫出来的 —— 它必须已经跟着改。
    assert.deepEqual((await readJson(root, "index.json")).skills[0].category, ["工具/搜索"]);
  } finally {
    await cleanup(root);
  }
});

// ── 5. PUT /v1/skills/file 的 base ─────────────────────────────────────────

test("PUT /v1/skills/file：旧 base → 409，新 base → 200 且 etag 换人", async () => {
  const { root, skills } = await setup(skillFiles("edit"));
  const api1 = api(root, skills);
  try {
    const first = await api1.call("GET", "/v1/skills/file?name=edit&path=SKILL.md");
    assert.equal(first.status, 200);
    assert.equal(first.body.name, "edit");
    assert.equal(first.body.path, "SKILL.md");
    const base = first.body.etag;
    assert.ok(base);

    const saved = await api1.call("PUT", "/v1/skills/file", {
      name: "edit",
      path: "SKILL.md",
      text: `${first.body.text}\n加一行。\n`,
      base,
    });
    assert.equal(saved.status, 200);
    assert.notEqual(saved.body.etag, base);
    assert.match(await read(root, "edit/SKILL.md"), /加一行。/);

    const stale = await api1.call("PUT", "/v1/skills/file", {
      name: "edit",
      path: "SKILL.md",
      text: "拿旧 base 盖回去\n",
      base,
    });
    assert.equal(stale.status, 409);
    assert.match(stale.body.error, /改过/);
  } finally {
    await cleanup(root);
  }
});

// ── 6. 发布过滤 D ───────────────────────────────────────────────────────────

test("publish 不带上工作区里的删除；archive 带上（显式动作才允许提交删除）", async () => {
  const { root, skills } = await setup();
  const api1 = api(root, skills);
  try {
    await api1.call("POST", "/v1/skills/create", { name: "doomed", description: "要被删掉的" });
    const first = await api1.call("POST", "/v1/skills/publish", { message: "skill: 先发布 doomed" });
    assert.equal(first.status, 200);
    assert.equal(first.body.pushed, false); // 没有 origin：不装成功
    assert.ok(first.body.commit?.hash, "应该真的 commit 了");

    // 工作区里删掉它（还没提交）。
    await run("git", ["rm", "-r", "-f", "--", "doomed"], { cwd: root });

    await api1.call("POST", "/v1/skills/create", { name: "second", description: "新的" });
    const second = await api1.call("POST", "/v1/skills/publish", { message: "skill: 发 second" });
    assert.equal(second.status, 200);
    assert.equal(second.body.commit !== null, true);

    const names = await showNames(root);
    assert.ok(names.some((line) => line.includes("second/SKILL.md")), "该发的没发");
    assert.equal(
      names.some((line) => line.includes("doomed/")),
      false,
      "把没打算发的删除一起推上去了",
    );

    // archive 是显式动作：includeDeleted，于是那条删除跟着落地。
    const archived = await api1.call("POST", "/v1/skills/archive", { name: "second" });
    assert.equal(archived.status, 200);
    const after = await showNames(root);
    assert.ok(
      after.some((line) => line.startsWith("D") && line.includes("doomed/SKILL.md")),
      `archive 没把删除带上：${after.join(" | ")}`,
    );
  } finally {
    await cleanup(root);
  }
});

// ── 7. 内容闸 ───────────────────────────────────────────────────────────────

test("publish 时 description 空：400、点名技能、index.json 一个字节不动", async () => {
  const { root, skills } = await setup(skillFiles("strict", { description: "有说明" }));
  const api1 = api(root, skills);
  try {
    const published = await api1.call("POST", "/v1/skills/publish", { message: "skill: 初始化" });
    assert.equal(published.status, 200);
    const indexBefore = await read(root, "index.json");

    // 把 description 抹掉（面板上就是改一行然后保存）。
    const file = await api1.call("GET", "/v1/skills/file?name=strict&path=SKILL.md");
    const broken = file.body.text.replace(/description: ".*"/, 'description: ""');
    const saved = await api1.call("PUT", "/v1/skills/file", {
      name: "strict",
      path: "SKILL.md",
      text: broken,
      base: file.body.etag,
    });
    assert.equal(saved.status, 200);

    const denied = await api1.call("POST", "/v1/skills/publish", { message: "skill: 发布" });
    assert.equal(denied.status, 400);
    assert.match(denied.body.error, /strict/);
    assert.match(denied.body.error, /description/);
    assert.equal(await read(root, "index.json"), indexBefore, "闸没过却动过 index.json");
  } finally {
    await cleanup(root);
  }
});

// ── 8. 归档 / 恢复 / 彻底删除 ───────────────────────────────────────────────

test("archive 消失 → restore 回来 → purge 真没", async () => {
  const { root, skills } = await setup(skillFiles("keeper"));
  const api1 = api(root, skills);
  try {
    await api1.call("POST", "/v1/skills/publish", { message: "skill: 初始化" });

    const archived = await api1.call("POST", "/v1/skills/archive", { name: "keeper" });
    assert.equal(archived.status, 200);
    let overview = await api1.call("GET", "/v1/skills");
    assert.deepEqual(
      overview.body.skills.map((skill) => skill.name),
      [],
    );
    assert.deepEqual(overview.body.archived, ["keeper"]);
    assert.equal(await exists(path.join(root, "keeper")), false);
    assert.equal(await exists(path.join(root, ".archive/keeper/SKILL.md")), true);

    const restored = await api1.call("POST", "/v1/skills/restore", { name: "keeper" });
    assert.equal(restored.status, 200);
    overview = await api1.call("GET", "/v1/skills");
    assert.deepEqual(
      overview.body.skills.map((skill) => skill.name),
      ["keeper"],
    );
    assert.deepEqual(overview.body.archived, []);

    await api1.call("POST", "/v1/skills/archive", { name: "keeper" });
    const purged = await api1.call("POST", "/v1/skills/purge", { name: "keeper" });
    assert.equal(purged.status, 200);
    assert.equal(await exists(path.join(root, ".archive/keeper")), false);
    const names = await showNames(root);
    assert.ok(names.some((line) => line.startsWith("D") && line.includes(".archive/keeper")), names.join(" | "));
  } finally {
    await cleanup(root);
  }
});

// ── 9. pending ──────────────────────────────────────────────────────────────

test("GET /v1/skills/pending：未跟踪技能是一个 skill 单元，diff 的 before 是 null", async () => {
  const { root, skills } = await setup();
  const api1 = api(root, skills);
  try {
    await api1.call("POST", "/v1/skills/create", { name: "unpub", description: "还没发" });

    const pending = await api1.call("GET", "/v1/skills/pending");
    assert.equal(pending.status, 200);
    const unit = pending.body.units.find((item) => item.name === "unpub");
    assert.ok(unit, `没有 unpub 这个单元：${JSON.stringify(pending.body.units)}`);
    assert.equal(unit.kind, "skill");
    assert.equal(unit.tracked, false);
    // git 会把整个未跟踪目录折成一条 `?? unpub/` —— 单元里就是这一条。
    assert.ok(unit.files.some((file) => file.path.startsWith("unpub")));

    const diff = await api1.call("GET", "/v1/skills/pending/diff?path=unpub/SKILL.md");
    assert.equal(diff.status, 200);
    assert.equal(diff.body.path, "unpub/SKILL.md");
    assert.equal(diff.body.before, null);
    assert.match(diff.body.after, /name: unpub/);
  } finally {
    await cleanup(root);
  }
});

// ── 10. 仓库读不了 ──────────────────────────────────────────────────────────

test("root 不存在：GET /v1/skills 仍 200，repo.error 是一行话（不 500、不白屏）", async () => {
  const root = path.join(await mkdtemp(path.join(tmpdir(), "nous-skills-gone-")), "nope");
  const skills = makeSkills(root);
  const api1 = api(root, skills);
  try {
    const { status: code, body } = await api1.call("GET", "/v1/skills");
    assert.equal(code, 200);
    assert.equal(typeof body.repo.error, "string");
    assert.ok(body.repo.error.length > 0);
    assert.equal(body.repo.head, null);
    assert.deepEqual(body.skills, []);
    assert.deepEqual(body.archived, []);
    // 技能仓不护 Basics（spec §3.6），词表又读不出来 —— 那就是空的，不许凭空造。
    assert.deepEqual(body.categories, []);
  } finally {
    await rm(path.dirname(root), { recursive: true, force: true });
  }
});

// ── 11. 技能仓没有受保护的类别（spec §3.6）─────────────────────────────────

test("POST /v1/skills/categories remove Basics：200，而且它不会自己回魂", async () => {
  const { root, skills } = await setup();
  const api1 = api(root, skills);
  try {
    assert.deepEqual((await api1.call("GET", "/v1/skills/categories")).body.categories, ["Basics"]);

    const removed = await api1.call("POST", "/v1/skills/categories", { op: "remove", name: "Basics" });
    assert.equal(removed.status, 200);
    assert.deepEqual(removed.body.categories, []);
    assert.deepEqual((await readJson(root, "categories.json")).categories, []);

    // 再读一次：没有注入，没有回魂。
    assert.deepEqual((await api1.call("GET", "/v1/skills/categories")).body.categories, []);
    assert.deepEqual((await api1.call("GET", "/v1/skills")).body.categories, []);
  } finally {
    await cleanup(root);
  }
});

// ── 12. 词表 ∪ 每条技能的 category（spec §3.2）────────────────────────────

test("GET /v1/skills 的 categories = 词表 ∪ 所有 skill.category，升序", async () => {
  const vocab = { v: 1, categories: ["Basics", "只在词表"] };
  const { root, skills } = await setup({
    "categories.json": JSON.stringify(vocab, null, 2) + "\n",
    ...skillFiles("a", { category: ["工具/搜索"] }),
    ...skillFiles("b"),
  });
  const api1 = api(root, skills);
  try {
    const { body } = await api1.call("GET", "/v1/skills");
    const expected = repo.sortCategories(["Basics", "只在词表", "工具/搜索"]);
    assert.deepEqual(body.categories, expected);
    // 「只在词表」那条挂不到任何技能上，但它必须在（词表存在的唯一理由）。
    assert.ok(body.categories.includes("只在词表"));
  } finally {
    await cleanup(root);
  }
});

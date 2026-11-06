// `/v1/skills*` 的行为 —— 注入假的 skills service + 真的临时 git 仓，不起引擎。
//
// 测的是「契约」：状态码、响应体的字段名、盘上的结果、git 里的结果。
// 不测 DOM，也不 import `server.js`（它有顶层 await，import 一下就起服务）。

import test from "node:test";
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { handleSkills, parseFrontmatter } from "../skills-api.js";
import * as repo from "../repo-tools.js";
import {
  CATEGORIES,
  MINI_INDEX,
  STRICT_INDEX,
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

// ── 3. duplicate ────────────────────────────────────────────────────────────

test("POST /v1/skills/duplicate：整目录拷过来、名字改成新的、不提交", async () => {
  const { root, skills } = await setup({
    ...skillFiles("demo", { summary: "一句", category: ["Basics"] }),
    "demo/references/notes.md": "附属文件也要跟着走\n",
  });
  const api1 = api(root, skills);
  try {
    const made = await api1.call("POST", "/v1/skills/duplicate", { from: "demo", name: "demo-copy" });
    assert.equal(made.status, 200);
    assert.equal(made.body.name, "demo-copy");
    assert.equal(made.body.from, "demo");
    assert.equal(made.body.files, 3);

    // 目录名 / frontmatter / nous.json 三处名字必须同一个（I3）。
    const front = await read(root, "demo-copy/SKILL.md");
    assert.match(front, /^---\nname: demo-copy\n/);
    const manifest = await readJson(root, "demo-copy/nous.json");
    assert.equal(manifest.name, "demo-copy");
    // summary / category 照搬。
    assert.equal(manifest.summary, "一句");
    assert.deepEqual(manifest.category, ["Basics"]);
    // 附件跟著走。
    assert.equal(await read(root, "demo-copy/references/notes.md"), "附属文件也要跟着走\n");

    // 没提交 = 未发布；名册和 index.json 都跟上了。
    assert.match(await status(root), /^\?\? demo-copy\/$/m);
    const overview = await api1.call("GET", "/v1/skills");
    const copy = overview.body.skills.find((skill) => skill.name === "demo-copy");
    assert.equal(copy.tracked, false, "复制出来必须是「未发布」");
    assert.ok(
      (await readJson(root, "index.json")).skills.some((skill) => skill.name === "demo-copy"),
      "复制之后要重扫 index.json",
    );
  } finally {
    await cleanup(root);
  }
});

test("POST /v1/skills/duplicate 撞已有目录 / 撞归档：409，不留半截", async () => {
  const { root, skills } = await setup({ ...skillFiles("demo"), ...skillFiles("taken") });
  const api1 = api(root, skills);
  try {
    const hit = await api1.call("POST", "/v1/skills/duplicate", { from: "demo", name: "taken" });
    assert.equal(hit.status, 409);
    assert.match(hit.body.error, /taken/);

    // 同名复制回自己身上：400，不是“拷完再覆盖”。
    const self = await api1.call("POST", "/v1/skills/duplicate", { from: "demo", name: "demo" });
    assert.equal(self.status, 400);

    // 源不存在：404。
    const missing = await api1.call("POST", "/v1/skills/duplicate", { from: "nope", name: "x" });
    assert.equal(missing.status, 404);

    // 归档里那个名字也占着。
    await api1.call("POST", "/v1/skills/archive", { name: "taken" });
    const archived = await api1.call("POST", "/v1/skills/duplicate", { from: "demo", name: "taken" });
    assert.equal(archived.status, 409);
    assert.match(archived.body.error, /archive/);

    const left = (await readdir(root)).filter((name) => name.startsWith(".new-"));
    assert.deepEqual(left, [], "翻车不能留点开头的半截目录");
  } finally {
    await cleanup(root);
  }
});

test("POST /v1/skills/duplicate：取用注脚不搬（复制品的来源是隔壁那条）", async () => {
  const { root, skills } = await setup({
    "adopted/SKILL.md": `---\nname: adopted\ndescription: "从上游取的"\n---\n\n# adopted\n`,
    "adopted/nous.json":
      JSON.stringify({ name: "adopted", summary: "", category: [], upstream: "matt/skills/tdd@abc123" }, null, 2) + "\n",
  });
  const api1 = api(root, skills);
  try {
    const made = await api1.call("POST", "/v1/skills/duplicate", { from: "adopted", name: "adopted-copy" });
    assert.equal(made.status, 200);
    const manifest = await readJson(root, "adopted-copy/nous.json");
    assert.equal(manifest.upstream, undefined, "复制品不是那个上游的取用，别带注脚");
    // 原件的一个字都不动。
    assert.equal((await readJson(root, "adopted/nous.json")).upstream, "matt/skills/tdd@abc123");
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

test("POST /v1/skills/category：新类别先登记词表 —— 生成器会校验，顺序反了就永远 400", async () => {
  // 真 index.mjs 拿 categories.json 校验每条 nous.json 的类别。先写 nous.json、后补词表
  // 的顺序在真仓里是死路（`工具/搜索` 不在词表 → 生成器红 → 整体回滚）。
  const root = await makeRepo({
    files: { "categories.json": CATEGORIES, "index.mjs": STRICT_INDEX, ...skillFiles("cat") },
  });
  const skills = makeSkills(root);
  await skills.load();
  const api1 = api(root, skills);
  try {
    const { status: code, body } = await api1.call("POST", "/v1/skills/category", {
      name: "cat",
      category: ["工具/搜索"],
    });
    assert.equal(code, 200, `词表顺序不对就会 400：${JSON.stringify(body)}`);
    assert.deepEqual(body.skill.category, ["工具/搜索"]);
    const vocab = (await readJson(root, "categories.json")).categories;
    assert.ok(vocab.includes("工具"), `词表里没有祖先「工具」：${vocab.join(",")}`);
    assert.ok(vocab.includes("工具/搜索"));
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

// ── 13. frontmatter：用扩展仓那一份（spec §2.1「解析器用现成的」）─────────────
//
// 这一节钉的是**为什么换**：本地那版只认 `key: value`，`>` / `|` 折叠块会把 `>`
// 当成值，引号里的 `\"` 也不会解 —— 结果就是内容闸看见一个「非空」的假值。

test("frontmatter 支持 `>` 折叠块、`|` 保留换行、引号解转义", () => {
  const folded = parseFrontmatter('---\nname: demo\ndescription: >\n  第一段\n  第二段\n---\n\n正文\n');
  assert.equal(folded.description, "第一段 第二段");

  const literal = parseFrontmatter("---\nname: demo\ndescription: |\n  第一行\n  第二行\n---\n");
  assert.equal(literal.description, "第一行\n第二行");

  const quoted = parseFrontmatter('---\nname: "demo"\ndescription: "他说 \\"这样\\""\n---\n');
  assert.equal(quoted.name, "demo");
  assert.equal(quoted.description, '他说 "这样"');
});

// ── 14. 内容闸看得见折叠块里的空描述（换成扩展仓那版之后才拦得住）──────────

test("内容闸：`description: >` 折出来是空的，照样 400", async () => {
  const { root, skills } = await setup({
    // `>` 折出来是空串：引用式折叠块里一个字都没有。
    "empty/SKILL.md": "---\nname: empty\ndescription: >\n---\n\n# empty\n",
    "empty/nous.json": JSON.stringify({ name: "empty", summary: "", category: [] }, null, 2) + "\n",
  });
  const api1 = api(root, skills);
  try {
    const denied = await api1.call("POST", "/v1/skills/publish", { message: "skill: 发布" });
    assert.equal(denied.status, 400);
    assert.match(denied.body.error, /empty/);
    assert.match(denied.body.error, /description/);
  } finally {
    await cleanup(root);
  }
});

// ── 15. 文件列表：递归到底（spec §6.1 没有深度上限，只有 200 个的上限）───────

test("GET /v1/skills/files：深层文件照样列，点开头 / node_modules 跳过", async () => {
  const { root, skills } = await setup(skillFiles("deep"));
  const api1 = api(root, skills);
  try {
    const nested = path.join("deep", "a/b/c/d/e/f/note.md");
    await mkdir(path.dirname(path.join(root, nested)), { recursive: true });
    await writeFile(path.join(root, nested), "深六层\n");
    await writeFile(path.join(root, "deep/.hidden"), "点开头\n");
    await mkdir(path.join(root, "deep/node_modules/dep"), { recursive: true });
    await writeFile(path.join(root, "deep/node_modules/dep/index.js"), "x\n");

    const { status: code, body } = await api1.call("GET", "/v1/skills/files?name=deep");
    assert.equal(code, 200);
    assert.equal(body.name, "deep");
    const paths = body.files.map((file) => file.path);
    assert.ok(paths.includes("a/b/c/d/e/f/note.md"), `深六层的文件没列出来：${paths.join(",")}`);
    assert.equal(paths.some((item) => item.includes(".hidden")), false);
    assert.equal(paths.some((item) => item.includes("node_modules")), false);
    for (const file of body.files) assert.deepEqual(Object.keys(file).sort(), ["path", "size"]);
  } finally {
    await cleanup(root);
  }
});

test("GET /v1/skills/files 最多 200 个（spec 的上限还在）", async () => {
  const { root, skills } = await setup(skillFiles("many"));
  const api1 = api(root, skills);
  try {
    for (let i = 0; i < 205; i += 1) {
      await writeFile(path.join(root, "many", `n-${String(i).padStart(3, "0")}.md`), "x\n");
    }
    const { body } = await api1.call("GET", "/v1/skills/files?name=many");
    assert.equal(body.files.length, 200);
  } finally {
    await cleanup(root);
  }
});

// ── 16. 读文件：不存在 / 二进制都是 404 + {error} ───────────────────────────

test("GET /v1/skills/file：不存在 → 404 {error}，二进制 → 404 {error}", async () => {
  const { root, skills } = await setup(skillFiles("bin"));
  const api1 = api(root, skills);
  try {
    await writeFile(path.join(root, "bin/blob.bin"), Buffer.from([0, 1, 2, 3]));
    const missing = await api1.call("GET", "/v1/skills/file?name=bin&path=nope.md");
    assert.equal(missing.status, 404);
    assert.equal(typeof missing.body.error, "string");

    const binary = await api1.call("GET", "/v1/skills/file?name=bin&path=blob.bin");
    assert.equal(binary.status, 404);
    assert.match(binary.body.error, /二进制/);
  } finally {
    await cleanup(root);
  }
});

// ── 17. sync / reload ──────────────────────────────────────────────────────

test("POST /v1/skills/sync：点名的路径提交，返回 ok/commit/pushed", async () => {
  const { root, skills } = await setup();
  const api1 = api(root, skills);
  try {
    await api1.call("POST", "/v1/skills/create", { name: "syncme", description: "要同步的" });
    // `paths` 是**抽屉里勾的那几条文件**（不是目录名）：先照 `/pending` 给的清单取。
    const pending = await api1.call("GET", "/v1/skills/pending");
    const unit = pending.body.units.find((item) => item.name === "syncme");
    assert.ok(unit, `没有 syncme 这个单元：${JSON.stringify(pending.body.units)}`);
    const paths = unit.files.map((file) => file.path);

    const { status: code, body } = await api1.call("POST", "/v1/skills/sync", {
      paths,
      message: "skill: 手动同步 syncme",
    });
    assert.equal(code, 200);
    assert.equal(body.ok, true, "没有远端的时候 pushed:false 也算 ok");
    assert.match(body.commit.hash, /^[0-9a-f]{7,}$/);
    assert.equal(body.pushed, false);
    const names = await showNames(root);
    assert.ok(names.some((line) => line.includes("syncme/SKILL.md")), names.join(" | "));
  } finally {
    await cleanup(root);
  }
});

test("POST /v1/skills/reload：重扫一次，回 {ok, skills}", async () => {
  const { root, skills } = await setup(skillFiles("r1"));
  const api1 = api(root, skills);
  try {
    const loadsBefore = skills.loads;
    const { status: code, body } = await api1.call("POST", "/v1/skills/reload");
    assert.equal(code, 200);
    assert.equal(body.ok, true);
    assert.deepEqual(
      body.skills.map((skill) => skill.name),
      ["r1"],
    );
    assert.equal(skills.loads, loadsBefore + 1);
  } finally {
    await cleanup(root);
  }
});

// ── 18. create 撞仓库自己的文件名 ──────────────────────────────────────────

test("POST /v1/skills/create 撞 RESERVED（index.json）：400 且点得出那个名字", async () => {
  const { root, skills } = await setup();
  const api1 = api(root, skills);
  try {
    const { status: code, body } = await api1.call("POST", "/v1/skills/create", {
      name: "index.json",
      description: "想当技能名",
    });
    assert.equal(code, 400);
    assert.match(body.error, /index\.json/);
  } finally {
    await cleanup(root);
  }
});

// ── 19. categories.json 带 v:1（spec §2.3，两份仓统一）─────────────────────

test("POST /v1/skills/categories 写出的 categories.json 是 {v:1, categories}", async () => {
  const { root, skills } = await setup();
  const api1 = api(root, skills);
  try {
    await api1.call("POST", "/v1/skills/categories", { op: "add", name: "工具/搜索" });
    const doc = await readJson(root, "categories.json");
    assert.equal(doc.v, 1);
    assert.deepEqual(doc.categories, repo.sortCategories(["Basics", "工具", "工具/搜索"]));
  } finally {
    await cleanup(root);
  }
});

// ── 20. pending/commit 的 files 是 {path,status}（spec §6.1）────────────────

test("GET /v1/skills/pending/commit：files 是 {path,status}，A/M/D/R 从 --name-status 取", async () => {
  const { root, skills } = await setup(skillFiles("first"));
  const api1 = api(root, skills);
  try {
    const published = await api1.call("POST", "/v1/skills/publish", { message: "skill: 初始化" });
    const initial = await api1.call("GET", `/v1/skills/pending/commit?hash=${published.body.commit.hash}`);
    assert.equal(initial.status, 200);
    assert.ok(initial.body.files.length > 0);
    for (const file of initial.body.files) {
      assert.deepEqual(Object.keys(file).sort(), ["path", "status"]);
      assert.match(file.status, /^[AMDR]$/);
    }
    assert.ok(initial.body.files.every((file) => file.status === "A"));

    // 一条混合改动的 commit：改名（R）、新增（A）、删除（D）。
    // 改名检测有体量门槛：三行的 SKILL.md 会被报成 A + D，所以先给它一点正文。
    const body = Array.from({ length: 40 }, (_, i) => `第 ${i} 行`).join("\n");
    const front = '---\nname: first\ndescription: "原来的说明"\n---\n';
    await writeFile(path.join(root, "first/SKILL.md"), `${front}\n${body}\n`);
    await run("git", ["add", "-A"], { cwd: root });
    await run("git", ["commit", "-q", "-m", "正文"], { cwd: root });

    await run("git", ["mv", "first", "renamed"], { cwd: root });
    await writeFile(path.join(root, "renamed/SKILL.md"), `${front.replace("原来的说明", "改过的")}\n${body}\n`);
    await mkdir(path.join(root, "added"), { recursive: true });
    await writeFile(path.join(root, "added/SKILL.md"), '---\nname: added\ndescription: "新的"\n---\n\n# added\n');
    await run("git", ["rm", "-q", "-f", "--", "renamed/nous.json"], { cwd: root });
    await run("git", ["add", "-A"], { cwd: root });
    await run("git", ["commit", "-q", "-m", "mix"], { cwd: root });

    const mixedHash = (await run("git", ["rev-parse", "HEAD"], { cwd: root })).stdout.trim();
    const mix = await api1.call("GET", `/v1/skills/pending/commit?hash=${mixedHash}`);
    assert.equal(mix.status, 200);
    const byPath = new Map(mix.body.files.map((file) => [file.path, file.status]));
    assert.equal(byPath.get("added/SKILL.md"), "A");
    // 删除报的是 HEAD 里那个路径（= 改名前的那一侧），改名报的是新名字。
    assert.equal(byPath.get("first/nous.json"), "D");
    assert.equal(byPath.get("renamed/SKILL.md"), "R");
    for (const file of mix.body.files) assert.equal("added" in file || "removed" in file, false);
  } finally {
    await cleanup(root);
  }
});

// ── 21. I7 半截：syncRepo 抛错时 index.json 必须退回原样 ────────────────────
//
// 发布 = 重扫 + commit + push。commit 之前的重扫已经把 index.json 改了；
// 后面挂了却不复原，盘上就留着一份「说已经改了、其实没提交」的索引 —— 半截。

test("I7：publish / archive / restore 失败时 index.json 一个字节不差地复原", async () => {
  const { root, skills } = await setup(skillFiles("keeper", { description: "原来的说明" }));
  const good = api(root, skills);
  const brokenRepo = {
    ...repo,
    syncRepo: async () => {
      throw new Error("断网了");
    },
  };
  const broken = (method, target, body) =>
    call(handleSkills, { method, path: target, body, root, skills, repo: brokenRepo });

  try {
    // 先有一条正常的发布，index.json 才有底版。
    assert.equal((await good.call("POST", "/v1/skills/publish", { message: "skill: 初始化" })).status, 200);
    const baseline = await read(root, "index.json");

    // 改描述 → 下次重扫会写出不同的 index.json。
    const file = await good.call("GET", "/v1/skills/file?name=keeper&path=SKILL.md");
    assert.equal(file.status, 200);
    await good.call("PUT", "/v1/skills/file", {
      name: "keeper",
      path: "SKILL.md",
      text: file.body.text.replace("原来的说明", "换过的说明"),
      base: file.body.etag,
    });

    const published = await broken("POST", "/v1/skills/publish", { message: "skill: 发布" });
    assert.match(published.body.error, /断网了/);
    assert.equal(await read(root, "index.json"), baseline, "发布失败却把 index.json 留在了半截");

    // 归档失败：目录要搬回来，索引也要回来。
    const archived = await broken("POST", "/v1/skills/archive", { name: "keeper" });
    assert.match(archived.body.error, /断网了/);
    assert.equal(await read(root, "index.json"), baseline, "归档失败却把 index.json 留在了半截");
    assert.equal(await exists(path.join(root, "keeper")), true, "归档失败要把目录搬回来");
    assert.equal(await exists(path.join(root, ".archive/keeper")), false);

    // 真的归档一次，拿到「归档后」的底版，再测恢复失败。
    assert.equal((await good.call("POST", "/v1/skills/archive", { name: "keeper" })).status, 200);
    const archivedBaseline = await read(root, "index.json");

    const restored = await broken("POST", "/v1/skills/restore", { name: "keeper" });
    assert.match(restored.body.error, /断网了/);
    assert.equal(await read(root, "index.json"), archivedBaseline, "恢复失败却把 index.json 留在了半截");
    assert.equal(await exists(path.join(root, ".archive/keeper")), true, "恢复失败要把目录搬回去");
    assert.equal(await exists(path.join(root, "keeper")), false);
  } finally {
    await cleanup(root);
  }
});

// ── 10. 分组：`deprecated/<名字>/`（mattpocock 那个仓的形状）──────────────────
//
// 分组只是抽屉：`name` 还是最后一段、还是会话键；住哪儿由 `path` 说。
// 所有文件 / 类别 / 归档的读写都得先按 path 找到目录 —— 找不到就全 404。

test("POST /v1/skills/create 带 group：落在 <分组>/<名字>/，name 还是名字", async () => {
  const { root, skills } = await setup();
  const api1 = api(root, skills);
  try {
    const made = await api1.call("POST", "/v1/skills/create", {
      name: "old",
      description: "退休的。",
      group: "deprecated",
    });
    assert.equal(made.status, 200, JSON.stringify(made.body));
    assert.equal(made.body.skill.name, "old");
    assert.equal(made.body.skill.path, "deprecated/old");

    assert.match(await read(root, "deprecated/old/SKILL.md"), /name: old/);
    assert.equal(await exists(path.join(root, "old")), false, "不该在顶层多出一个 old/");
    // 生成器扫出来的 index.json 也得带 path。
    assert.equal((await readJson(root, "index.json")).skills[0].path, "deprecated/old");

    const overview = await api1.call("GET", "/v1/skills");
    assert.deepEqual(overview.body.groups, ["deprecated"]);
    assert.equal(overview.body.skills[0].tracked, false);
  } finally {
    await cleanup(root);
  }
});

test("create 的 group 不合法 / 分组已经是一条技能：400，不留下半截", async () => {
  const { root, skills } = await setup(skillFiles("taken"));
  const api1 = api(root, skills);
  try {
    const deep = await api1.call("POST", "/v1/skills/create", {
      name: "demo",
      description: "x",
      group: "a/b",
    });
    assert.equal(deep.status, 400);
    assert.match(deep.body.error, /一层/);

    const inside = await api1.call("POST", "/v1/skills/create", {
      name: "demo",
      description: "x",
      group: "taken",
    });
    assert.equal(inside.status, 400);
    assert.match(inside.body.error, /taken/);
    assert.equal(await exists(path.join(root, "taken", "demo")), false);
  } finally {
    await cleanup(root);
  }
});

test("分组里的技能：files / file / category 都按 path 找得到", async () => {
  const { root, skills } = await setup({ ...skillFiles("deprecated/demo") });
  const api1 = api(root, skills);
  try {
    const listed = await api1.call("GET", "/v1/skills/files?name=demo");
    assert.equal(listed.status, 200);
    assert.deepEqual(
      listed.body.files.map((file) => file.path).sort(),
      ["SKILL.md", "nous.json"],
    );

    const one = await api1.call("GET", "/v1/skills/file?name=demo&path=SKILL.md");
    assert.equal(one.status, 200);
    assert.match(one.body.text, /name: demo/);

    const saved = await api1.call("PUT", "/v1/skills/file", {
      name: "demo",
      path: "SKILL.md",
      text: `${one.body.text}\n改一行。\n`,
      base: one.body.etag,
    });
    assert.equal(saved.status, 200);
    assert.match(await read(root, "deprecated/demo/SKILL.md"), /改一行。/);
    assert.equal(await exists(path.join(root, "demo")), false);

    const cat = await api1.call("POST", "/v1/skills/category", { name: "demo", category: [] });
    assert.equal(cat.status, 200);
    assert.deepEqual(cat.body.skill.category, []);
    assert.deepEqual((await readJson(root, "deprecated/demo/nous.json")).category, []);
  } finally {
    await cleanup(root);
  }
});

test("分组里的技能：归档进扁的 .archive/<名字>，恢复回原来那个分组", async () => {
  const { root, skills } = await setup({ ...skillFiles("deprecated/demo") });
  const api1 = api(root, skills);
  try {
    const gone = await api1.call("POST", "/v1/skills/archive", { name: "demo" });
    assert.equal(gone.status, 200, JSON.stringify(gone.body));
    assert.equal(await exists(path.join(root, "deprecated/demo")), false);
    assert.equal(await exists(path.join(root, ".archive/demo")), true);
    assert.equal((await readJson(root, ".archive/demo/nous.json")).archiveOrigin, "deprecated/demo");

    const back = await api1.call("POST", "/v1/skills/restore", { name: "demo" });
    assert.equal(back.status, 200, JSON.stringify(back.body));
    assert.equal(await exists(path.join(root, "deprecated/demo/SKILL.md")), true, "恢复必须回到原来的分组");
    assert.equal(await exists(path.join(root, ".archive/demo")), false);
    // archiveOrigin 是归档态的字段，回来了就该摘掉。
    assert.equal(Object.hasOwn(await readJson(root, "deprecated/demo/nous.json"), "archiveOrigin"), false);
  } finally {
    await cleanup(root);
  }
});

// ── 11. 写路径：新建文件 / 新建文件夹 / 删除 ────────────────────────────────
//
// 「新建文件」= PUT 一个还不存在的路径；「新建文件夹」= 建目录 + 一份 `.gitkeep`
// （空目录 git 不跟踪，没有它这个文件夹发布那一刻就消失）。删除不提交，git 是回收站。

test("PUT 一个不存在的路径 = 新建；中间的目录一起造出来；.gitkeep 顺手收掉", async () => {
  const { root, skills } = await setup(skillFiles("grow"));
  const api1 = api(root, skills);
  try {
    const made = await api1.call("POST", "/v1/skills/mkdir", { name: "grow", path: "references" });
    assert.equal(made.status, 200, JSON.stringify(made.body));
    assert.match(await read(root, "grow/references/.gitkeep"), /^$/);

    const saved = await api1.call("PUT", "/v1/skills/file", {
      name: "grow",
      path: "references/deep/guide.md",
      text: "# 指南\n",
    });
    assert.equal(saved.status, 200, JSON.stringify(saved.body));
    assert.match(await read(root, "grow/references/deep/guide.md"), /# 指南/);
    assert.equal(await exists(path.join(root, "grow/references/.gitkeep")), false);

    const listed = await api1.call("GET", "/v1/skills/files?name=grow");
    assert.ok(listed.body.files.some((file) => file.path === "references/deep/guide.md"));
  } finally {
    await cleanup(root);
  }
});

test("PUT 的路径跑出技能目录（../）：403，盘上什么都不留", async () => {
  const { root, skills } = await setup(skillFiles("safe"));
  const api1 = api(root, skills);
  try {
    const bad = await api1.call("PUT", "/v1/skills/file", { name: "safe", path: "../escape.md", text: "x" });
    assert.ok([400, 403].includes(bad.status), `路径逃逸必须被挡（收到 ${bad.status}）`);
    assert.equal(await exists(path.join(root, "escape.md")), false);
  } finally {
    await cleanup(root);
  }
});

test("DELETE 文件：删掉，空掉的父目录一起收；非空目录要显式 recursive", async () => {
  const { root, skills } = await setup(skillFiles("trim"));
  const api1 = api(root, skills);
  try {
    await api1.call("PUT", "/v1/skills/file", { name: "trim", path: "notes/a.md", text: "a" });
    await api1.call("PUT", "/v1/skills/file", { name: "trim", path: "notes/b.md", text: "b" });

    const blocked = await api1.call("DELETE", "/v1/skills/file?name=trim&path=notes");
    assert.equal(blocked.status, 409);
    assert.match(blocked.body.error, /notes/);

    const one = await api1.call("DELETE", "/v1/skills/file?name=trim&path=notes/a.md");
    assert.equal(one.status, 200);
    assert.equal(await exists(path.join(root, "trim/notes/a.md")), false);
    assert.equal(await exists(path.join(root, "trim/notes")), true, "还有 b.md，目录得留着");

    const last = await api1.call("DELETE", "/v1/skills/file?name=trim&path=notes/b.md");
    assert.equal(last.status, 200);
    assert.equal(await exists(path.join(root, "trim/notes")), false, "空了就连目录一起收");

    await api1.call("POST", "/v1/skills/mkdir", { name: "trim", path: "junk" });
    await api1.call("PUT", "/v1/skills/file", { name: "trim", path: "junk/x.md", text: "x" });
    const whole = await api1.call("DELETE", "/v1/skills/file?name=trim&path=junk&recursive=1");
    assert.equal(whole.status, 200, JSON.stringify(whole.body));
    assert.equal(await exists(path.join(root, "trim/junk")), false);
  } finally {
    await cleanup(root);
  }
});

test("DELETE 不存在的路径：404，不是 500", async () => {
  const { root, skills } = await setup(skillFiles("trim"));
  const api1 = api(root, skills);
  try {
    const gone = await api1.call("DELETE", "/v1/skills/file?name=trim&path=nope.md");
    assert.equal(gone.status, 404);
  } finally {
    await cleanup(root);
  }
});

test("tracked：HEAD 里有的算已发布，分组里的也算（不能只看顶层目录）", async () => {
  const { root, skills } = await setup({
    ...skillFiles("plain"),
    ...skillFiles("deprecated/old"),
    ...skillFiles("deprecated/fresh"),
  });
  // `fresh` 还没进 HEAD 那次提交 —— 只有它是「未发布」。
  await repo.run("git", ["-C", root, "rm", "-r", "-q", "--cached", "--", "deprecated/fresh"]);
  await repo.run("git", ["-C", root, "commit", "-q", "-m", "只提交 plain + deprecated/old"]);
  await skills.load();

  const api1 = api(root, skills);
  const overview = await api1.call("GET", "/v1/skills");
  const byName = new Map(overview.body.skills.map((skill) => [skill.name, skill]));
  assert.equal(byName.get("plain").tracked, true);
  assert.equal(byName.get("old").tracked, true, "分组里的已发布技能不能算未发布");
  assert.equal(byName.get("fresh").tracked, false, "没进 HEAD 的才是未发布");
  await cleanup(root);
});

test("files：卡底那个「文件 N 个」—— 数文件不数目录，`.git` / `node_modules` / 点开头的不算", async () => {
  const { root, skills } = await setup({
    ...skillFiles("counted"),
    "counted/references/deep/more.md": "深一层也算",
    "counted/.hidden": "点开头的不算（那是本机杂碎，不是技能内容）",
    "counted/node_modules/pkg/index.js": "依赖也不算",
    "counted/empty-dir/.keep": "",
  });
  const api1 = api(root, skills);
  try {
    const overview = await api1.call("GET", "/v1/skills");
    const skill = overview.body.skills.find((item) => item.name === "counted");
    // SKILL.md + nous.json + references/deep/more.md + empty-dir/.keep 里那个点开头的也不算
    assert.equal(skill.files, 3, "应该只数 SKILL.md、nous.json、references/deep/more.md");
  } finally {
    await cleanup(root);
  }
});

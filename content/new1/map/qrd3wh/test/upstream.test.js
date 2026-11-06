// 上游（订阅 / 镜像 / 取用）—— 全部用**本地 git 仓**当上游，不联网。
//
// 这套设计要证明的只有一件事：**上游就是一个 git remote**。所以测试里那个
// 「别人的技能仓」就是一个临时目录 —— 跟 github.com 在 git 眼里没有区别。
// 这也是文档里那句「另一台机器上的 Nous-skills 也能当上游」的实测版本。

import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { handleSkills } from "../skills-api.js";
import * as repo from "../repo-tools.js";
import * as upstream from "../upstream.js";
import {
  CATEGORIES,
  MINI_INDEX,
  cleanup,
  commitAll,
  exists,
  head,
  makeRepo,
  makeSkills,
  run,
  skillFiles,
  status,
  writeTree,
  call,
} from "./helpers.js";

/** 上游那个仓：两条技能 + 一条藏在 .archive/ 里的（不该出现在清单里）。 */
const makeUpstreamRepo = async () =>
  makeRepo({
    files: {
      "skills/engineering/code-review/SKILL.md": `---\nname: code-review\ndescription: "审查从某个点以来的改动"\n---\n\n正文\n`,
      "skills/engineering/code-review/nous.json": JSON.stringify({ name: "code-review", summary: "审代码", category: ["Basics"] }, null, 2) + "\n",
      "skills/engineering/tdd/SKILL.md": `---\nname: tdd\ndescription: "先写测试"\n---\n\n正文\n`,
      "skills/in-progress/ask-matt/SKILL.md": `---\nname: ask-matt\ndescription: "问 Matt"\n---\n\n正文\n`,
      ".archive/dead/SKILL.md": `---\nname: dead\ndescription: "早就不用了"\n---\n\n正文\n`,
      "README.md": "别人的技能仓\n",
    },
  });

/** 我们的仓 + 订阅了一份上游。 */
const setup = async ({ sources = [] } = {}) => {
  const root = await makeRepo({
    files: { "categories.json": CATEGORIES, "index.mjs": MINI_INDEX, ...(sources.length ? { "sources.json": JSON.stringify({ v: 1, sources }, null, 2) + "\n" } : {}) },
  });
  const data = await mkdtemp(path.join(tmpdir(), "nous-skills-data-"));
  const skills = makeSkills(root);
  await skills.load();
  const call_ = (method, target, body) => call(handleSkills, { method, path: target, body, root, skills, repo, data });
  return { root, data, skills, call: call_ };
};

const teardown = async (...dirs) => {
  for (const dir of dirs) await cleanup(dir);
};

const readJson = async (root, rel) => JSON.parse(await readFile(path.join(root, rel), "utf8"));

// ── 订阅清单 ────────────────────────────────────────────────────────────────

test("sources.json 不在 = 没订阅任何人（不是错误）", async () => {
  const ctx = await setup();
  try {
    const out = await ctx.call("GET", "/v1/skills/sources");
    assert.equal(out.status, 200);
    assert.deepEqual(out.body.sources, []);
  } finally {
    await teardown(ctx.root, ctx.data);
  }
});

test("POST sources 落盘但不提交：加一个上游也得走发布", async () => {
  const ctx = await setup();
  try {
    const out = await ctx.call("POST", "/v1/skills/sources", { sources: [{ name: "matt", url: "https://github.com/mattpocock/skills", prefix: "skills" }] });
    assert.equal(out.status, 200);
    assert.match((await status(ctx.root)).toString(), /sources\.json/);
    const saved = await readJson(ctx.root, "sources.json");
    assert.equal(saved.sources[0].name, "matt");
    assert.equal(saved.sources[0].prefix, "skills");
    // 未发布 = 工作区脏，HEAD 里没有
    assert.equal((await run("git", ["-C", ctx.root, "ls-tree", "--name-only", "HEAD"])).stdout.trim(), "categories.json\nindex.mjs".split("\n").join("\n"));
  } finally {
    await teardown(ctx.root, ctx.data);
  }
});

test("上游名不合法 / 地址认不出 → 400，说清是谁", async () => {
  const ctx = await setup();
  try {
    const bad = await ctx.call("POST", "/v1/skills/sources", { sources: [{ name: "Matt Pocock", url: "https://x/y" }] });
    assert.equal(bad.status, 400);
    assert.match(bad.body.error, /上游名/);

    const weird = await ctx.call("POST", "/v1/skills/sources", { sources: [{ name: "matt", url: "ftp:/nope" }] });
    assert.equal(weird.status, 400);
    assert.match(weird.body.error, /认不出这个地址/);
  } finally {
    await teardown(ctx.root, ctx.data);
  }
});

// ── 镜像 / 清单 ─────────────────────────────────────────────────────────────

test("GET upstream 不联网：镜像没有就报 missing，也不偷偷 clone", async () => {
  const source = await makeUpstreamRepo();
  const ctx = await setup({ sources: [{ name: "matt", url: source }] });
  try {
    const out = await ctx.call("GET", "/v1/skills/upstream");
    assert.equal(out.status, 200);
    assert.equal(out.body.sources[0].mirror, "missing");
    assert.deepEqual(out.body.entries, []);
    assert.equal(await exists(path.join(ctx.data, "upstream", "matt")), false);
  } finally {
    await teardown(ctx.root, ctx.data, source);
  }
});

test("refresh 才拉镜像；清单从 tree 里读，.archive/ 不算", async () => {
  const source = await makeUpstreamRepo();
  const ctx = await setup({ sources: [{ name: "matt", url: source, prefix: "skills" }] });
  try {
    const refreshed = await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    assert.equal(refreshed.status, 200);
    assert.equal(refreshed.body.ok, true);
    assert.equal(refreshed.body.sources[0].head, await head(source));

    const out = await ctx.call("GET", "/v1/skills/upstream");
    const paths = out.body.entries.map((entry) => entry.path);
    assert.deepEqual(paths, ["skills/engineering/code-review", "skills/engineering/tdd", "skills/in-progress/ask-matt"]);
    const review = out.body.entries[0];
    assert.equal(review.name, "code-review");
    assert.equal(review.description, "审查从某个点以来的改动");
    assert.equal(review.summary, "审代码");
    assert.deepEqual(review.category, ["Basics"]);
    assert.match(review.sha, /^[0-9a-f]{40}$/);
    assert.equal(review.local, "none");
  } finally {
    await teardown(ctx.root, ctx.data, source);
  }
});

test("prefix 收窄范围：只列 prefix 底下的", async () => {
  const source = await makeUpstreamRepo();
  const ctx = await setup({ sources: [{ name: "matt", url: source, prefix: "skills/in-progress" }] });
  try {
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    const out = await ctx.call("GET", "/v1/skills/upstream");
    assert.deepEqual(out.body.entries.map((entry) => entry.name), ["ask-matt"]);
  } finally {
    await teardown(ctx.root, ctx.data, source);
  }
});

// ── 取用 ────────────────────────────────────────────────────────────────────

test("adopt 摊出文件、写 upstream 注脚、不提交", async () => {
  const source = await makeUpstreamRepo();
  const ctx = await setup({ sources: [{ name: "matt", url: source, prefix: "skills" }] });
  try {
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    const loadsBefore = ctx.skills.loads;
    const out = await ctx.call("POST", "/v1/skills/upstream/adopt", { source: "matt", path: "skills/engineering/code-review" });
    assert.equal(out.status, 200, out.body?.error);
    assert.equal(out.body.name, "code-review");

    const manifest = await readJson(ctx.root, "code-review/nous.json");
    assert.equal(manifest.name, "code-review");
    assert.equal(manifest.summary, "审代码");
    assert.match(manifest.upstream, /^matt\/skills\/engineering\/code-review@[0-9a-f]{40}$/);
    assert.match(await readFile(path.join(ctx.root, "code-review/SKILL.md"), "utf8"), /description: "审查从某个点以来的改动"/);

    // 名册跟上了（不然面板说有、会话里没有）
    assert.ok(ctx.skills.loads > loadsBefore);
    assert.ok(ctx.skills.list().some((skill) => skill.name === "code-review"));
    // 未发布：目录在，HEAD 里没有
    assert.match((await status(ctx.root)).toString(), /code-review\//);
    assert.equal(await repo.isTrackedDir(ctx.root, "code-review"), false);
    // 拿过来的类别进词表了（否则真生成器会红）
    assert.ok((await repo.readCategories(ctx.root, { protect: [] })).includes("Basics"));
    // 暂存目录不留半截
    assert.equal((await readdir(ctx.root)).some((name) => name.startsWith(".new-")), false);
  } finally {
    await teardown(ctx.root, ctx.data, source);
  }
});

test("取用过的再取一次 → 409；上游改了 → 清单说 changed", async () => {
  const source = await makeUpstreamRepo();
  const ctx = await setup({ sources: [{ name: "matt", url: source, prefix: "skills" }] });
  try {
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    await ctx.call("POST", "/v1/skills/upstream/adopt", { source: "matt", path: "skills/engineering/code-review" });

    const again = await ctx.call("POST", "/v1/skills/upstream/adopt", { source: "matt", path: "skills/engineering/code-review" });
    assert.equal(again.status, 409);
    assert.match(again.body.error, /已经在了/);

    let list = await ctx.call("GET", "/v1/skills/upstream");
    assert.equal(list.body.entries.find((entry) => entry.name === "code-review").local, "same");

    // 上游改了一行 → 刷新后这条变 changed
    await writeTree(source, "skills/engineering/code-review/SKILL.md", `---\nname: code-review\ndescription: "改了"\n---\n\n新正文\n`);
    await commitAll(source, "upstream: 改 code-review");
    await ctx.call("POST", "/v1/skills/upstream/refresh", { name: "matt" });

    list = await ctx.call("GET", "/v1/skills/upstream");
    assert.equal(list.body.entries.find((entry) => entry.name === "code-review").local, "changed");
    // 本地那份一个字没动 —— 「有更新」是提示，不是自动覆盖
    assert.match(await readFile(path.join(ctx.root, "code-review/SKILL.md"), "utf8"), /审查从某个点以来的改动/);
  } finally {
    await teardown(ctx.root, ctx.data, source);
  }
});

test("取用时改名字：frontmatter 和 nous.json 一起改，目录名 == 名字", async () => {
  const source = await makeUpstreamRepo();
  const ctx = await setup({ sources: [{ name: "matt", url: source, prefix: "skills" }] });
  try {
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    const out = await ctx.call("POST", "/v1/skills/upstream/adopt", {
      source: "matt",
      path: "skills/engineering/tdd",
      name: "tdd-matt",
    });
    assert.equal(out.status, 200, out.body?.error);
    assert.match(await readFile(path.join(ctx.root, "tdd-matt/SKILL.md"), "utf8"), /^---\nname: tdd-matt\n/);
    assert.equal((await readJson(ctx.root, "tdd-matt/nous.json")).name, "tdd-matt");

    // 改过名的那一行必然跟上游不一样 —— 对照表要说 diff，不是假装一致。
    const files = await ctx.call("GET", "/v1/skills/upstream/files?source=matt&path=skills/engineering/tdd");
    assert.equal(files.status, 200);
    assert.equal(files.body.localName, "tdd-matt");
    assert.deepEqual(files.body.files.map((file) => [file.path, file.state]), [["SKILL.md", "diff"]]);
    // 抽屉里那棵树要显示大小：`ls-tree -l` 白送的那一列
    assert.ok(files.body.files[0].size > 0);
    assert.equal(files.body.adopted, true);
  } finally {
    await teardown(ctx.root, ctx.data, source);
  }
});

test("本地改了正文 → 文件对照说 diff；上游有本地没取的 skill → local 是 none", async () => {
  const source = await makeUpstreamRepo();
  const ctx = await setup({ sources: [{ name: "matt", url: source, prefix: "skills" }] });
  try {
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    await ctx.call("POST", "/v1/skills/upstream/adopt", { source: "matt", path: "skills/engineering/tdd" });

    // 刚取用、一个字没改 → same
    let files = await ctx.call("GET", "/v1/skills/upstream/files?source=matt&path=skills/engineering/tdd");
    assert.deepEqual(files.body.files.map((file) => [file.path, file.state]), [["SKILL.md", "same"]]);

    await writeTree(ctx.root, "tdd/SKILL.md", `---\nname: tdd\ndescription: "我们自己改过"\n---\n\n不同的正文\n`);
    files = await ctx.call("GET", "/v1/skills/upstream/files?source=matt&path=skills/engineering/tdd");
    assert.deepEqual(files.body.files.map((file) => [file.path, file.state]), [["SKILL.md", "diff"]]);

    const list = await ctx.call("GET", "/v1/skills/upstream");
    assert.equal(list.body.entries.find((entry) => entry.name === "ask-matt").local, "none");
    // 本地改过 ≠ 上游有更新：`changed` 说的是**上游动了**（文件对照负责说本地改了）。
    assert.equal(list.body.entries.find((entry) => entry.name === "tdd").local, "same");
  } finally {
    await teardown(ctx.root, ctx.data, source);
  }
});

test("没拉过镜像就取用 → 400 说清先刷新", async () => {
  const source = await makeUpstreamRepo();
  const ctx = await setup({ sources: [{ name: "matt", url: source, prefix: "skills" }] });
  try {
    const out = await ctx.call("POST", "/v1/skills/upstream/adopt", { source: "matt", path: "skills/engineering/tdd" });
    assert.equal(out.status, 400);
    assert.match(out.body.error, /先点刷新/);
    // 没有留半截目录
    assert.equal(await exists(path.join(ctx.root, "tdd")), false);
  } finally {
    await teardown(ctx.root, ctx.data, source);
  }
});

test("上游那条的 SKILL.md 没有 frontmatter → 400，不留半截", async () => {
  const source = await makeRepo({ files: { "x/broken/SKILL.md": "没有 frontmatter 的正文\n" } });
  const ctx = await setup({ sources: [{ name: "matt", url: source }] });
  try {
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    const out = await ctx.call("POST", "/v1/skills/upstream/adopt", { source: "matt", path: "x/broken" });
    assert.equal(out.status, 400);
    assert.match(out.body.error, /frontmatter/);
    assert.equal(await exists(path.join(ctx.root, "broken")), false);
    assert.equal((await readdir(ctx.root)).some((name) => name.startsWith(".new-")), false);
  } finally {
    await teardown(ctx.root, ctx.data, source);
  }
});

test("materialize 只摊 blob，路径不许跑出目标目录", async () => {
  const source = await makeRepo({ files: { "a/b/SKILL.md": "---\nname: b\ndescription: x\n---\n" } });
  const target = await mkdtemp(path.join(tmpdir(), "nous-adopt-"));
  try {
    const out = await upstream.materialize({
      data: await mkdtemp(path.join(tmpdir(), "nous-data-")),
      source: { name: "x", url: source },
      run,
      home: "a/b",
      target,
    });
    assert.deepEqual(out.files, ["SKILL.md"]);
    assert.match(await readFile(path.join(target, "SKILL.md"), "utf8"), /name: b/);
  } finally {
    await rm(target, { recursive: true, force: true });
    await cleanup(source);
  }
});

// ── 正文（只读）────────────────────────────────────────────────────────────

test("上游正文从镜像里读：GET 拿得到，POST 是 405 —— 别人的仓我们没有写入口", async () => {
  const source = await makeUpstreamRepo();
  const ctx = await setup({ sources: [{ name: "matt", url: source, prefix: "skills" }] });
  try {
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    const out = await ctx.call("GET", "/v1/skills/upstream/file?source=matt&path=skills/engineering/tdd");
    assert.equal(out.status, 200, out.body?.error);
    // `file` 回的是**仓里的路径**（不带上「哪一条」那一层，那层在 path 里）
    assert.equal(out.body.file, "skills/engineering/tdd/SKILL.md");
    assert.match(out.body.text, /name: tdd/);
    assert.ok(out.body.size > 0);

    const nope = await ctx.call("GET", "/v1/skills/upstream/file?source=matt&path=skills/engineering/tdd&file=none.md");
    assert.equal(nope.status, 404);

    const write = await ctx.call("POST", "/v1/skills/upstream/file", {});
    assert.equal(write.status, 405);
    assert.match(write.body.error, /只读/);

    // 路径不许跑出那一条
    const escape = await ctx.call("GET", "/v1/skills/upstream/file?source=matt&path=skills/engineering/tdd&file=../../README.md");
    assert.equal(escape.status, 400);
  } finally {
    await teardown(ctx.root, ctx.data, source);
  }
});

test("上游的文件清单带大小和子目录 —— 抽屉里那棵树要的就是这个", async () => {
  const source = await makeRepo({
    files: {
      "skills/x/SKILL.md": `---\nname: x\ndescription: "x"\n---\n\n正文\n`,
      "skills/x/references/a.md": "# a\n",
      "skills/x/references/deep/b.md": "# b\n",
      "skills/x/scripts/run.mjs": "console.log(1)\n",
    },
  });
  const ctx = await setup({ sources: [{ name: "matt", url: source, prefix: "skills" }] });
  try {
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    const out = await ctx.call("GET", "/v1/skills/upstream/files?source=matt&path=skills/x");
    assert.equal(out.status, 200, out.body?.error);
    assert.equal(out.body.adopted, false); // 没取用过：对照的三种状态没意义
    // 顺序是 `localeCompare` 的（`SKILL.md` 排在小写后面）；树怎么摆是面板的事
    assert.deepEqual(
      out.body.files.map((file) => file.path).sort(),
      ["SKILL.md", "references/a.md", "references/deep/b.md", "scripts/run.mjs"].sort(),
    );
    for (const file of out.body.files) assert.ok(file.size > 0, file.path);

    // 隔层那份也读得到（`fileTree` 靠它建出 references/deep/）
    const deep = await ctx.call("GET", "/v1/skills/upstream/file?source=matt&path=skills/x&file=references/deep/b.md");
    assert.equal(deep.status, 200, deep.body?.error);
    assert.equal(deep.body.text, "# b\n");
  } finally {
    await teardown(ctx.root, ctx.data, source);
  }
});

test("抽屉那棵树只有技能：仓里的 README / .github 一个字都不进来", async () => {
  const source = await makeRepo({
    files: {
      "skills/engineering/x/SKILL.md": `---\nname: x\ndescription: "x"\n---\n\n正文\n`,
      "skills/engineering/x/references/a.md": "# a\n",
      "skills/in-progress/y/SKILL.md": `---\nname: y\ndescription: "y"\n---\n\n正文\n`,
      "README.md": "别人的仓\n",
      ".github/workflows/ci.yml": "on: push\n",
      "docs/guide.md": "文档\n",
      "skills/engineering/README.md": "分组说明，不是技能\n",
    },
  });
  const ctx = await setup({ sources: [{ name: "matt", url: source, prefix: "skills" }] });
  try {
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    const out = await ctx.call("GET", "/v1/skills/upstream/tree?source=matt");
    assert.equal(out.status, 200, out.body?.error);
    // 只有「带 SKILL.md 的目录」里的文件，而且路径已经剥掉 prefix
    assert.deepEqual(out.body.files.map((file) => file.path).sort(), [
      "engineering/x/SKILL.md",
      "engineering/x/references/a.md",
      "in-progress/y/SKILL.md",
    ]);
  } finally {
    await teardown(ctx.root, ctx.data);
  }
});

test("工作区只 check out 技能目录 —— 别人的 README 不该躺在我们盘上", async () => {
  const source = await makeRepo({
    files: {
      "skills/engineering/x/SKILL.md": `---\nname: x\ndescription: "x"\n---\n\n正文\n`,
      "skills/in-progress/y/SKILL.md": `---\nname: y\ndescription: "y"\n---\n\n正文\n`,
      "README.md": "别人的仓\n",
      ".github/workflows/ci.yml": "on: push\n",
    },
  });
  const ctx = await setup({ sources: [{ name: "matt", url: source, prefix: "skills" }] });
  try {
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    const home = path.join(ctx.data, "upstream", "matt");
    const onDisk = [];
    const walk = async (at, rel = "") => {
      for (const item of await readdir(at, { withFileTypes: true })) {
        if (item.name === ".git") continue;
        const next = rel ? `${rel}/${item.name}` : item.name;
        if (item.isDirectory()) await walk(path.join(at, item.name), next);
        else onDisk.push(next);
      }
    };
    await walk(home);
    assert.deepEqual(onDisk.sort(), [
      "skills/engineering/x/SKILL.md",
      "skills/in-progress/y/SKILL.md",
    ]);
  } finally {
    await teardown(ctx.root, ctx.data);
  }
});

// ── 版本 ────────────────────────────────────────────────────────────────────
//
// 两件事必须分开：**我们认的版本**（`sources.json` 里的 `version`，人钉的，
// 进版本库）和**采集到的**（镜像 HEAD，每次 fetch 会往前跑）。
// 它们一旦被同一个东西改写，「记录版本」这四个字就没意义了。

test("首拉就记下基线版本；之后的刷新绝不偷偷改它", async () => {
  const upstreamRepo = await makeRepo({
    files: { "skills/x/SKILL.md": `---\nname: x\ndescription: "x"\n---\n\n正文\n` },
  });
  const ctx = await setup({ sources: [{ name: "matt", url: upstreamRepo, prefix: "skills" }] });
  try {
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    const first = await readJson(ctx.root, "sources.json");
    assert.match(first.sources[0].version.commit, /^[0-9a-f]{40}$/);
    assert.ok(first.sources[0].version.at);

    // 上游往前走一个 commit
    await writeFile(path.join(upstreamRepo, "skills/x/extra.md"), "新文件\n");
    await commitAll(upstreamRepo, "上游又加了一个文件");
    const newHead = await head(upstreamRepo);

    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    const second = await readJson(ctx.root, "sources.json");
    // 采集到了新的（镜像 HEAD 变了），但**认的还是老的** —— 这是关键
    assert.equal(second.sources[0].version.commit, first.sources[0].version.commit);

    const list = await ctx.call("GET", "/v1/skills/upstream");
    const source = list.body.sources[0];
    assert.equal(source.head, newHead);
    assert.equal(source.drift, true, "采集到的和认的不一样，得说出来");
  } finally {
    await teardown(ctx.root, ctx.data);
  }
});

test("记录版本：钉到镜像 HEAD，drift 立刻变 false", async () => {
  const upstreamRepo = await makeRepo({
    files: { "skills/x/SKILL.md": `---\nname: x\ndescription: "x"\n---\n\n正文\n` },
  });
  const ctx = await setup({ sources: [{ name: "matt", url: upstreamRepo, prefix: "skills" }] });
  try {
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    await writeFile(path.join(upstreamRepo, "skills/x/extra.md"), "新文件\n");
    await commitAll(upstreamRepo, "上游又加了一个文件");
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    const newHead = await head(upstreamRepo);

    const out = await ctx.call("POST", "/v1/skills/upstream/pin", { source: "matt" });
    assert.equal(out.status, 200, out.body?.error);
    assert.equal(out.body.version.commit, newHead);
    assert.equal((await readJson(ctx.root, "sources.json")).sources[0].version.commit, newHead);

    const list = await ctx.call("GET", "/v1/skills/upstream");
    assert.equal(list.body.sources[0].drift, false);
  } finally {
    await teardown(ctx.root, ctx.data);
  }
});

test("记录版本：没镜像就没得记（不是「记个空」）", async () => {
  const ctx = await setup({ sources: [{ name: "matt", url: "https://example.invalid/none.git" }] });
  try {
    const out = await ctx.call("POST", "/v1/skills/upstream/pin", { source: "matt" });
    assert.equal(out.status, 400);
    assert.match(out.body.error, /先刷新/);
  } finally {
    await teardown(ctx.root, ctx.data);
  }
});

test("sources.json 里的 version 只认 sha，别的一律 400", async () => {
  const ctx = await setup({
    sources: [{ name: "matt", url: "https://example.invalid/x.git", version: { commit: "不是sha" } }],
  });
  try {
    const out = await ctx.call("GET", "/v1/skills/upstream");
    assert.equal(out.status, 400);
    assert.match(out.body.error, /commit sha/);
  } finally {
    await teardown(ctx.root, ctx.data);
  }
});

// ── 版本号（人看的）────────────────────────────────────────────────────────
//
// `4ae2317` 是给机器看的。上游自己打了 `v1.3.2`，就该说 `v1.3.2`。

test("版本号用上游的 tag：正好打在 tag 上是 v1.3.2，在它之后是 v1.3.2+", async () => {
  const upstreamRepo = await makeRepo({
    files: { "skills/x/SKILL.md": `---\nname: x\ndescription: "x"\n---\n\n正文\n` },
  });
  await run("git", ["-C", upstreamRepo, "tag", "v1.0.0"]);
  const ctx = await setup({ sources: [{ name: "matt", url: upstreamRepo, prefix: "skills" }] });
  try {
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    const atTag = await ctx.call("POST", "/v1/skills/upstream/pin", { source: "matt" });
    assert.equal(atTag.body.version.tag, "v1.0.0");
    assert.equal(atTag.body.version.behind, undefined);

    // 上游往前走了几步，还没打新 tag
    await writeFile(path.join(upstreamRepo, "skills/x/extra.md"), "新文件\n");
    await commitAll(upstreamRepo, "上游又加了一个文件");
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    const ahead = await ctx.call("POST", "/v1/skills/upstream/pin", { source: "matt" });
    assert.equal(ahead.body.version.behind, "v1.0.0", "在 v1.0.0 之后 —— 前端写成 v1.0.0+");
    assert.equal(ahead.body.version.tag, undefined);

    // 采集到的那一版也给人类版本号
    const list = await ctx.call("GET", "/v1/skills/upstream");
    assert.equal(list.body.sources[0].headVersion.behind, "v1.0.0");
  } finally {
    await teardown(ctx.root, ctx.data);
  }
});

test("一个 tag 都没有的仓：不编版本号，留给前端退回 sha", async () => {
  const upstreamRepo = await makeRepo({
    files: { "skills/x/SKILL.md": `---\nname: x\ndescription: "x"\n---\n\n正文\n` },
  });
  const ctx = await setup({ sources: [{ name: "matt", url: upstreamRepo, prefix: "skills" }] });
  try {
    await ctx.call("POST", "/v1/skills/upstream/refresh", {});
    const out = await ctx.call("POST", "/v1/skills/upstream/pin", { source: "matt" });
    assert.equal(out.body.version.tag, undefined);
    assert.equal(out.body.version.behind, undefined);
    assert.match(out.body.version.commit, /^[0-9a-f]{40}$/);
  } finally {
    await teardown(ctx.root, ctx.data);
  }
});

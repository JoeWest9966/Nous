// `/v1/prompts*` 的行为 —— 注入假的 prompts service + 真的临时 git 仓，不起引擎。
//
// 测的是「契约」：状态码、响应体的字段名、盘上的结果、git 里的结果。
// 不测 DOM，也不 import `server.js`（它有顶层 await，import 一下就起服务）。
//
// 跟 skills-api.test.js 同一个骨架。差别是模板仓的闸：正文非空 + summary 非空 +
// `settings.default` 必须指向一条真存在的模板（最后那条由生成器管，这里也测一次）。

import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";

import { handlePrompts } from "../prompts-api.js";
import * as repo from "../repo-tools.js";
import {
  CATEGORIES,
  PROMPTS_MINI_INDEX,
  cleanup,
  exists,
  makePrompts,
  makeRepo,
  makeReq,
  makeRes,
  promptFiles,
  run,
  showNames,
} from "./helpers.js";

const SETTINGS = JSON.stringify({ v: 1, default: null }, null, 2) + "\n";

/** 一个能跑的模板仓：词表 + 生成器 + 设置（+ 需要的话，预置几条模板）。 */
const makePromptsRepo = async (files = {}) =>
  makeRepo({
    files: {
      "categories.json": CATEGORIES,
      "settings.json": SETTINGS,
      "index.mjs": PROMPTS_MINI_INDEX,
      ...files,
    },
  });

/** 仓 + 假 service（先 load 一次，名册才是热的 —— 跟真进程启动时一样）。 */
const setup = async (files = {}) => {
  const root = await makePromptsRepo(files);
  const prompts = makePrompts(root);
  await prompts.load();
  return { root, prompts };
};

/** 一次完整的 `handlePrompts` 调用；返回 { status, body }。 */
const api = (root, prompts) => ({
  call: async (method, target, body) => {
    const url = new URL(`http://block.local${target}`);
    const parts = url.pathname.split("/").filter(Boolean);
    const req = makeReq(method, body);
    const { res, out } = makeRes();
    await handlePrompts({ req, res, url, parts, root, prompts, repo });
    return { status: out.status, body: out.body, raw: out.text };
  },
});

const read = (root, relative) => readFile(path.join(root, relative), "utf8");

test("GET /v1/prompts：名册 + tracked + 类别 + 默认值", async () => {
  const { root, prompts } = await setup(promptFiles("one", { category: ["Basics"] }));
  try {
    const got = await api(root, prompts).call("GET", "/v1/prompts");
    assert.equal(got.status, 200);
    assert.equal(got.body.prompts.length, 1);
    assert.equal(got.body.prompts[0].name, "one");
    assert.equal(got.body.prompts[0].tracked, true);
    assert.equal(got.body.default, null);
    assert.deepEqual(got.body.categories, ["Basics"]);
    assert.equal(got.body.repo.error, null);
  } finally {
    await cleanup(root);
  }
});

test("create：落盘 + 重扫索引 + 未发布；重复 / 空摘要 / 坏名字都 400", async () => {
  const { root, prompts } = await setup();
  const call = api(root, prompts).call;
  try {
    const made = await call("POST", "/v1/prompts/create", {
      name: "concise",
      summary: "一句话回复",
      body: "Answer in one sentence.\n",
      category: ["Basics"],
    });
    assert.equal(made.status, 200);
    assert.equal(made.body.prompt.name, "concise");
    assert.equal(made.body.prompt.load.state, "ok");
    assert.equal(await exists(path.join(root, "concise", "PROMPT.md")), true);

    const index = JSON.parse(await read(root, "index.json"));
    assert.deepEqual(
      index.prompts.map((prompt) => prompt.name),
      ["concise"],
    );
    // 还没提交：面板上该是琥珀色「未发布」。
    const overview = await call("GET", "/v1/prompts");
    assert.equal(overview.body.prompts[0].tracked, false);

    assert.equal((await call("POST", "/v1/prompts/create", { name: "concise", summary: "再来一次" })).status, 400);
    assert.equal((await call("POST", "/v1/prompts/create", { name: "mute", summary: "  " })).status, 400);
    assert.equal((await call("POST", "/v1/prompts/create", { name: "Bad Name", summary: "x" })).status, 400);
    assert.equal((await call("POST", "/v1/prompts/create", { name: "settings.json", summary: "x" })).status, 400);
  } finally {
    await cleanup(root);
  }
});

test("PUT /file：base 对不上 409；对上了落盘并重扫名册", async () => {
  const { root, prompts } = await setup(promptFiles("one"));
  const call = api(root, prompts).call;
  try {
    const file = await call("GET", "/v1/prompts/file?name=one&path=PROMPT.md");
    assert.equal(file.status, 200);
    assert.equal(file.body.text, "Answer in one sentence.\n");

    const stale = await call("PUT", "/v1/prompts/file", {
      name: "one",
      path: "PROMPT.md",
      text: "changed\n",
      base: "0000",
    });
    assert.equal(stale.status, 409);

    const before = prompts.loads;
    const saved = await call("PUT", "/v1/prompts/file", {
      name: "one",
      path: "PROMPT.md",
      text: "Be terse.\n",
      base: file.body.etag,
    });
    assert.equal(saved.status, 200);
    assert.ok(saved.body.etag && saved.body.etag !== file.body.etag);
    assert.equal(await read(root, "one/PROMPT.md"), "Be terse.\n");
    assert.equal(prompts.loads, before + 1, "写完必须重扫，否则面板说有、会话里没有");
  } finally {
    await cleanup(root);
  }
});

test("publish：正文空 → 400、点名模板、index.json 一个字节不动", async () => {
  const { root, prompts } = await setup(promptFiles("strict"));
  const call = api(root, prompts).call;
  try {
    assert.equal((await call("POST", "/v1/prompts/publish", { message: "prompt: 初始化" })).status, 200);
    const indexBefore = await read(root, "index.json");

    const file = await call("GET", "/v1/prompts/file?name=strict&path=PROMPT.md");
    const saved = await call("PUT", "/v1/prompts/file", {
      name: "strict",
      path: "PROMPT.md",
      text: "   \n",
      base: file.body.etag,
    });
    assert.equal(saved.status, 200);

    const denied = await call("POST", "/v1/prompts/publish", { message: "prompt: 发布" });
    assert.equal(denied.status, 400);
    assert.match(denied.body.error, /strict/);
    assert.match(denied.body.error, /空的/);
    assert.equal(await read(root, "index.json"), indexBefore, "闸不过不许动索引");
  } finally {
    await cleanup(root);
  }
});

test("publish：默认值指向不存在的模板 → 400（生成器是最后一道闸）", async () => {
  const { root, prompts } = await setup({ ...promptFiles("only"), "settings.json": SETTINGS.replace("null", '"ghost"') });
  const call = api(root, prompts).call;
  try {
    const denied = await call("POST", "/v1/prompts/publish", { message: "prompt: 发布" });
    assert.equal(denied.status, 400);
    assert.match(denied.body.error, /ghost/);
  } finally {
    await cleanup(root);
  }
});

test("publish 不带工作区里的删除；archive 带上", async () => {
  const { root, prompts } = await setup();
  const call = api(root, prompts).call;
  try {
    await call("POST", "/v1/prompts/create", { name: "doomed", summary: "要被删掉的" });
    const first = await call("POST", "/v1/prompts/publish", { message: "prompt: 先发布 doomed" });
    assert.equal(first.status, 200);
    assert.equal(first.body.pushed, false); // 没有 origin：不装成功
    assert.ok(first.body.commit?.hash, "应该真的 commit 了");

    await run("git", ["rm", "-r", "-f", "--", "doomed"], { cwd: root });

    await call("POST", "/v1/prompts/create", { name: "second", summary: "新的" });
    const second = await call("POST", "/v1/prompts/publish", { message: "prompt: 发 second" });
    assert.equal(second.status, 200);
    assert.ok(second.body.commit, "该发的没发");

    const names = await showNames(root);
    assert.ok(
      names.some((line) => line.includes("second/PROMPT.md")),
      `该发的没发：${names.join(" | ")}`,
    );
    assert.equal(
      names.some((line) => line.includes("doomed/")),
      false,
      "把没打算发的删除一起推上去了",
    );

    const archived = await call("POST", "/v1/prompts/archive", { name: "second" });
    assert.equal(archived.status, 200);
    const after = await showNames(root);
    assert.ok(
      after.some((line) => line.startsWith("D") && line.includes("doomed/PROMPT.md")),
      `archive 没把删除带上：${after.join(" | ")}`,
    );
    assert.equal(await exists(path.join(root, ".archive", "second", "PROMPT.md")), true);
  } finally {
    await cleanup(root);
  }
});

test("archive / restore / purge：三步都动仓库，归档区不静默消失", async () => {
  const { root, prompts } = await setup(promptFiles("one"));
  const call = api(root, prompts).call;
  try {
    assert.equal((await call("POST", "/v1/prompts/publish", { message: "prompt: 初始化" })).status, 200);

    assert.equal((await call("POST", "/v1/prompts/archive", { name: "one" })).status, 200);
    assert.equal(await exists(path.join(root, "one")), false);
    let overview = await call("GET", "/v1/prompts");
    assert.deepEqual(overview.body.archived, ["one"]);

    assert.equal((await call("POST", "/v1/prompts/restore", { name: "one" })).status, 200);
    assert.equal(await exists(path.join(root, "one", "PROMPT.md")), true);

    assert.equal((await call("POST", "/v1/prompts/archive", { name: "one" })).status, 200);
    assert.equal((await call("POST", "/v1/prompts/purge", { name: "one" })).status, 200);
    assert.equal(await exists(path.join(root, ".archive", "one")), false);
    overview = await call("GET", "/v1/prompts");
    assert.deepEqual(overview.body.archived, []);
  } finally {
    await cleanup(root);
  }
});

test("pending：一个模板 = 一个单元，不是一行行文件", async () => {
  const { root, prompts } = await setup(promptFiles("one"));
  const call = api(root, prompts).call;
  try {
    const file = await call("GET", "/v1/prompts/file?name=one&path=PROMPT.md");
    await call("PUT", "/v1/prompts/file", {
      name: "one",
      path: "PROMPT.md",
      text: "Changed body.\n",
      base: file.body.etag,
    });
    const pending = await call("GET", "/v1/prompts/pending");
    assert.equal(pending.status, 200);
    const unit = pending.body.units.find((item) => item.key === "one");
    assert.ok(unit, `没找到 one 这个单元：${JSON.stringify(pending.body.units)}`);
    assert.equal(unit.kind, "prompt");
    assert.ok(unit.files.some((entry) => entry.path === "one/PROMPT.md"));
  } finally {
    await cleanup(root);
  }
});

test("类别：建词表 → 挂到模板上 → 名册跟着重扫", async () => {
  const { root, prompts } = await setup(promptFiles("one"));
  const call = api(root, prompts).call;
  try {
    const added = await call("POST", "/v1/prompts/categories", { op: "add", name: "沟通" });
    assert.equal(added.status, 200);
    assert.ok(added.body.categories.includes("沟通"));

    const tagged = await call("POST", "/v1/prompts/category", { name: "one", category: ["沟通"] });
    assert.equal(tagged.status, 200);
    assert.deepEqual(tagged.body.prompt.category, ["沟通"]);
    assert.deepEqual(prompts.list()[0].category, ["沟通"]);
  } finally {
    await cleanup(root);
  }
});

// `/v1/presets*` 的行为 —— 真的 `createPresets` 名册 + 真的临时 git 仓，不起引擎。
//
// 测的是「契约」：状态码、响应体的字段名、盘上的结果、git 里的结果。
// 不测 DOM，也不 import `server.js`（它有顶层 await，import 一下就起服务）。
//
// 跟 prompts-api.test.js 同一个骨架。差别：预设的清单文件叫 `preset.json`，闸是
// 「形状 + 引用」（引用由注入的 ctx 判定），而且它有两个只读端点 `validate` / `resolve`。

import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";

import { handlePresets } from "../presets-api.js";
import { createPresets } from "../presets.js";
import * as repo from "../repo-tools.js";
import { CATEGORIES, cleanup, exists, head, makeRepo, makeReq, makeRes, showNames } from "./helpers.js";

/** 模板仓的迷你生成器：跟 `Nous-Template/index.mjs` 同形状（按段扫盘、校验、写索引）。 */
const PRESETS_MINI_INDEX = `import { readdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.dirname(fileURLToPath(import.meta.url));
// 按段扫：每段各写各的 index.json（跟真仓 index.mjs 同一个口径，只是只有一段）。
const SECTION = "Session-Template";
const root = path.join(repoRoot, SECTION);
const items = [];
for (const entry of await readdir(root, { withFileTypes: true })) {
  if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
  let raw;
  try {
    raw = await readFile(path.join(root, entry.name, "preset.json"), "utf8");
  } catch {
    continue;
  }
  let spec;
  try {
    spec = JSON.parse(raw);
  } catch {
    console.error(entry.name + ": preset.json 不是合法 JSON");
    process.exit(1);
  }
  if (spec.name !== entry.name) {
    console.error(entry.name + ": name 跟目录名对不上");
    process.exit(1);
  }
  if (typeof spec.summary !== "string" || !spec.summary.trim()) {
    console.error(entry.name + ": summary 是空的");
    process.exit(1);
  }
  items.push({ name: entry.name, summary: spec.summary, category: spec.category ?? [] });
}
items.sort((a, b) => a.name.localeCompare(b.name));
await writeFile(
  path.join(root, "index.json"),
  JSON.stringify({ v: 1, section: SECTION, items, categories: JSON.parse(await readFile(path.join(root, "categories.json"), "utf8")).categories }, null, 2) + "\\n",
);
`;

/** 一条预设落成两个文件（`preset.json` 是必需的，`PRESET.md` 可选）。 */
const presetFiles = (name, spec = {}, { body = null } = {}) => ({
  [`${name}/preset.json`]:
    JSON.stringify({ v: 1, name, summary: spec.summary ?? `${name} 的一句话`, category: [], ...spec }, null, 2) + "\n",
  ...(body ? { [`${name}/PRESET.md`]: body } : {}),
});

const SECTION = "Session-Template";
const SECTIONS = JSON.stringify(
  { v: 1, sections: [{ id: SECTION, title: "会话模板", manifest: "preset.json", checker: "preset" }] },
  null,
  2,
);

/**
 * 造一个**按段**的临时仓：`sections.json` + `index.mjs` 在仓根，数据全在 `Session-Template/`。
 * 传进来的相对路径（`one/preset.json`）自动落到段里 —— 测试正文只关心段内口径。
 */
const makePresetsRepo = async (files = {}) =>
  makeRepo({
    files: {
      "sections.json": SECTIONS,
      "index.mjs": PRESETS_MINI_INDEX,
      [`${SECTION}/categories.json`]: CATEGORIES,
      ...Object.fromEntries(Object.entries(files).map(([rel, text]) => [`${SECTION}/${rel}`, text])),
    },
  });

/** 假的引用 ctx：两个技能、两条模板、两个扩展、一台模型（档位三档）。 */
const fakeCtx = () => ({
  skillsHas: (name) => ["code-review", "deep-dive"].includes(name),
  promptsHas: (name) => ["smart-ass", "one-shot"].includes(name),
  extensionsHas: (name) => ["nous", "skills"].includes(name),
  catalogHas: (ref) => ref.provider === "volc-ark",
  levelsOf: (ref) => (ref?.provider === "volc-ark" ? ["off", "medium", "high"] : []),
  deviceModel: { provider: "volc-ark", modelId: "deepseek-v4-flash-260425" },
});

const setup = async (files = {}) => {
  const repoRoot = await makePresetsRepo(files);
  const presets = await createPresets({ dir: path.join(repoRoot, SECTION) });
  return { repoRoot, root: repoRoot, presets };
};

const sections = [{ id: SECTION, title: "会话模板", manifest: "preset.json", checker: "preset" }];

/**
 * 调一次接口。测试正文写 `/v1/presets/file?…`，这里补上段名 —— 但**段名本身也要能测**，
 * 所以给了 `raw: true` 就不补。
 */
const api = (repoRoot, presets, ctx = fakeCtx) => ({
  call: async (method, target, body, { raw = false } = {}) => {
    const full = raw || !target.startsWith("/v1/presets") ? target : target.replace("/v1/presets", `/v1/presets/${SECTION}`);
    const url = new URL(`http://block.local${full}`);
    const parts = url.pathname.split("/").filter(Boolean);
    const req = makeReq(method, body);
    const { res, out } = makeRes();
    await handlePresets({
      req,
      res,
      url,
      parts,
      repoRoot,
      sections,
      presetsOf: (id) => (id === SECTION ? presets : null),
      repo,
      ctx,
    });
    return { status: out.status, body: out.body, raw: out.text };
  },
});

const read = (root, relative) => readFile(path.join(root, SECTION, relative), "utf8");

// ── 列表 ────────────────────────────────────────────────────────────────────

test("GET /v1/presets：名册 + tracked + 解析结果 + 类别", async () => {
  const { root, presets } = await setup(
    presetFiles("weekly", { category: ["Basics"], template: "smart-ass", skills: ["code-review"] }),
  );
  try {
    const got = await api(root, presets).call("GET", "/v1/presets");
    assert.equal(got.status, 200);
    assert.equal(got.body.presets.length, 1);
    const row = got.body.presets[0];
    assert.equal(row.name, "weekly");
    assert.equal(row.tracked, true);
    assert.equal(row.specError, null);
    // 解析结果由服务端算 —— 面板不许自己算第二份。
    assert.equal(row.resolved.model.state, "auto");
    assert.equal(row.resolved.model.value, "volc-ark/deepseek-v4-flash-260425");
    assert.equal(row.resolved.template.mode, "picked");
    assert.deepEqual(row.resolved.skills.map((item) => item.state), ["ok"]);
    assert.deepEqual(got.body.categories, ["Basics"]);
    assert.deepEqual(got.body.archived, []);
    assert.equal(got.body.repo.error, null);
  } finally {
    await cleanup(root);
  }
});

test("GET /v1/presets：引用失效的条目不进 specError，却在 resolved 里报红", async () => {
  const { root, presets } = await setup(
    presetFiles("gone", { skills: ["deleted-skill"], template: "deleted-template" }),
  );
  try {
    const row = (await api(root, presets).call("GET", "/v1/presets")).body.presets[0];
    assert.equal(row.specError, null);
    assert.equal(row.resolved.skills[0].state, "invalid");
    assert.equal(row.resolved.template.state, "invalid");
  } finally {
    await cleanup(root);
  }
});

test("GET /v1/presets：写坏了的那条 specError 有原文、resolved 是 null", async () => {
  const { root, presets } = await setup({ "bad/preset.json": "{ 这不是 json" });
  try {
    const row = (await api(root, presets).call("GET", "/v1/presets")).body.presets[0];
    assert.match(row.specError, /不是合法 JSON/);
    assert.equal(row.resolved, null);
  } finally {
    await cleanup(root);
  }
});

// ── 新建 ────────────────────────────────────────────────────────────────────

test("create：落盘 + 重扫索引 + 未发布（第一个正确状态就是未发布）", async () => {
  const { root, presets } = await setup();
  const call = api(root, presets).call;
  try {
    const made = await call("POST", "/v1/presets/create", {
      name: "concise",
      summary: "一句话回复",
      category: ["Basics"],
      spec: { template: "smart-ass", skills: ["code-review"] },
    });
    assert.equal(made.status, 200);
    assert.equal(made.body.preset.name, "concise");
    assert.equal(await exists(path.join(root, SECTION, "concise", "preset.json")), true);

    const spec = JSON.parse(await read(root, "concise/preset.json"));
    assert.deepEqual(spec, {
      v: 1,
      name: "concise",
      summary: "一句话回复",
      category: ["Basics"],
      template: "smart-ass",
      skills: ["code-review"],
    });

    const index = JSON.parse(await read(root, "index.json"));
    assert.deepEqual(index.items.map((item) => item.name), ["concise"]);

    // 还没提交：面板上该是琥珀色「未发布」。
    assert.equal((await call("GET", "/v1/presets")).body.presets[0].tracked, false);
  } finally {
    await cleanup(root);
  }
});

test("create：空摘要 / 坏名字 / 撞已有 / 撞保留名 / 撞归档 → 400", async () => {
  const { root, presets } = await setup(presetFiles("taken"));
  const call = api(root, presets).call;
  try {
    assert.equal((await call("POST", "/v1/presets/create", { name: "mute", summary: "  " })).status, 400);
    assert.equal((await call("POST", "/v1/presets/create", { name: "Bad Name", summary: "x" })).status, 400);
    assert.equal((await call("POST", "/v1/presets/create", { name: "taken", summary: "x" })).status, 400);
    assert.equal((await call("POST", "/v1/presets/create", { name: "index.json", summary: "x" })).status, 400);
  } finally {
    await cleanup(root);
  }
});

test("create：从设备默认模型起手（from:device）—— 型号来自 ctx，不是前端猜的", async () => {
  const { root, presets } = await setup();
  const call = api(root, presets).call;
  try {
    await call("POST", "/v1/presets/create", { name: "pinned", summary: "钉一台", from: "device" });
    const spec = JSON.parse(await read(root, "pinned/preset.json"));
    assert.deepEqual(spec.model, { provider: "volc-ark", modelId: "deepseek-v4-flash-260425" });
  } finally {
    await cleanup(root);
  }
});

test("create：spec 里有未知键 → 400（建一个一出生就坏掉的目录比报错更糟）", async () => {
  const { root, presets } = await setup();
  const call = api(root, presets).call;
  try {
    const got = await call("POST", "/v1/presets/create", {
      name: "typo",
      summary: "拼错了",
      spec: { tempalte: "smart-ass" },
    });
    assert.equal(got.status, 400);
    assert.match(got.body.error, /tempalte/);
    assert.equal(await exists(path.join(root, "typo")), false);
  } finally {
    await cleanup(root);
  }
});

// ── 读写文件 ────────────────────────────────────────────────────────────────

test("file：读 preset.json 带 etag；写坏 JSON / 坏形状当场 400（不留坏账）", async () => {
  const { root, presets } = await setup(presetFiles("one"));
  const call = api(root, presets).call;
  try {
    const got = await call("GET", "/v1/presets/file?name=one&path=preset.json");
    assert.equal(got.status, 200);
    assert.ok(got.body.etag);

    const bad = await call("PUT", "/v1/presets/file", { name: "one", path: "preset.json", text: "{ 坏了" });
    assert.equal(bad.status, 400);
    assert.match(bad.body.error, /不是合法 JSON/);

    const typo = await call("PUT", "/v1/presets/file", {
      name: "one",
      path: "preset.json",
      text: JSON.stringify({ v: 1, name: "one", summary: "x", category: [], tempalte: "smart-ass" }),
    });
    assert.equal(typo.status, 400);
    assert.match(typo.body.error, /tempalte/);
    // 盘上还是原来那份。
    assert.equal(JSON.parse(await read(root, "one/preset.json")).summary, "one 的一句话");
  } finally {
    await cleanup(root);
  }
});

test("file：etag 对不上 → 409（409 那一句是面板要认的）", async () => {
  const { root, presets } = await setup(presetFiles("one"));
  const call = api(root, presets).call;
  try {
    const got = await call("PUT", "/v1/presets/file", {
      name: "one",
      path: "preset.json",
      text: JSON.stringify({ v: 1, name: "one", summary: "改了", category: [] }),
      base: "deadbeefdeadbeef",
    });
    assert.equal(got.status, 409);
  } finally {
    await cleanup(root);
  }
});

test("file：PRESET.md 不过形状闸（说明书怎么写都不影响它能不能跑）", async () => {
  const { root, presets } = await setup(presetFiles("one"));
  const call = api(root, presets).call;
  try {
    const got = await call("PUT", "/v1/presets/file", {
      name: "one",
      path: "PRESET.md",
      text: "# 随手写的\n\n不是 JSON 也没关系。\n",
    });
    assert.equal(got.status, 200);
    assert.match(await read(root, "one/PRESET.md"), /不是 JSON/);
  } finally {
    await cleanup(root);
  }
});

test("files：列目录里的文件（预设一般只有 preset.json + PRESET.md）", async () => {
  const { root, presets } = await setup(presetFiles("one", {}, { body: "# 说明书\n" }));
  try {
    const got = await api(root, presets).call("GET", "/v1/presets/files?name=one");
    assert.deepEqual(got.body.files.map((file) => file.path).sort(), ["PRESET.md", "preset.json"]);
  } finally {
    await cleanup(root);
  }
});

// ── 两个只读端点 ────────────────────────────────────────────────────────────

test("validate：按 name 校验已落盘的那条，ok 带 resolved", async () => {
  const { root, presets } = await setup(presetFiles("one", { skills: ["code-review"] }));
  const call = api(root, presets).call;
  try {
    const got = await call("POST", "/v1/presets/validate", { name: "one" });
    assert.equal(got.status, 200);
    assert.equal(got.body.ok, true);
    assert.deepEqual(got.body.errors, []);
    assert.equal(got.body.resolved.skills[0].state, "ok");
  } finally {
    await cleanup(root);
  }
});

test("validate：按 spec 校验草稿（编辑框里还没落盘的那份）", async () => {
  const { root, presets } = await setup();
  const call = api(root, presets).call;
  try {
    const draft = { v: 1, name: "draft", summary: "草稿", category: [], skills: ["nope"] };
    const got = await call("POST", "/v1/presets/validate", { spec: draft });
    assert.equal(got.status, 200);
    assert.equal(got.body.ok, false);
    assert.match(got.body.errors[0], /没装/);
  } finally {
    await cleanup(root);
  }
});

test("validate / resolve：既没 name 也没 spec → 400（别拿空校验骗面板说「没问题」）", async () => {
  const { root, presets } = await setup();
  const call = api(root, presets).call;
  try {
    assert.equal((await call("POST", "/v1/presets/validate", {})).status, 400);
    assert.equal((await call("GET", "/v1/presets/resolve")).status, 400);
  } finally {
    await cleanup(root);
  }
});

test("resolve：不存在的 name → 404；写坏的那条 → 400 带原文", async () => {
  const { root, presets } = await setup({ "bad/preset.json": "{ 坏了" });
  const call = api(root, presets).call;
  try {
    assert.equal((await call("GET", "/v1/presets/resolve?name=nope")).status, 404);
    const bad = await call("GET", "/v1/presets/resolve?name=bad");
    assert.equal(bad.status, 400);
    assert.match(bad.body.error, /不是合法 JSON/);
  } finally {
    await cleanup(root);
  }
});

test("resolve：template:null 与「键缺席」在 resolved 里分得开（这个区别丢了就等于三态丢了）", async () => {
  const { root, presets } = await setup({
    ...presetFiles("none", { template: null }),
    ...presetFiles("default"),
  });
  const call = api(root, presets).call;
  try {
    const none = await call("GET", "/v1/presets/resolve?name=none");
    const dflt = await call("GET", "/v1/presets/resolve?name=default");
    assert.equal(none.body.resolved.template.mode, "none");
    assert.equal(dflt.body.resolved.template.mode, "default");
    assert.equal(none.body.resolved.absence.template, false);
    assert.equal(dflt.body.resolved.absence.template, true);
  } finally {
    await cleanup(root);
  }
});

// ── 发布 / 归档三件套 ──────────────────────────────────────────────────────

test("publish：形状闸 → 重扫 → commit；坏的一条挡住整批，且一个字节都不动", async () => {
  const { root, presets } = await setup({ ...presetFiles("good"), "bad/preset.json": "{ 坏了" });
  const call = api(root, presets).call;
  try {
    const before = await head(root);
    const blocked = await call("POST", "/v1/presets/publish", {});
    assert.equal(blocked.status, 400);
    assert.match(blocked.body.error, /不是合法 JSON/);
    // HEAD 一个字节都没动（闸在最前面，连 index.json 都没重扫）。
    assert.equal(await head(root), before);

    // 修好之后能发。
    await call("PUT", "/v1/presets/file", {
      name: "bad",
      path: "preset.json",
      text: JSON.stringify({ v: 1, name: "bad", summary: "修好了", category: [] }),
    });
    const ok = await call("POST", "/v1/presets/publish", {});
    assert.equal(ok.status, 200);
    // 临时仓没有 remote：那是「只在本地」，**不是**推送失败（专属私仓允许不配 origin）。
    assert.equal(ok.body.pushed, null);
    assert.equal(ok.body.ok, true);
    assert.ok(ok.body.commit?.hash);
    const names = (await showNames(root, "HEAD")).map((line) => line.split("\t")[1]);
    assert.ok(names.includes(`${SECTION}/bad/preset.json`));
  } finally {
    await cleanup(root);
  }
});

test("archive → restore → purge：可逆的那两步留 git 历史，purge 才真没", async () => {
  const { root, presets } = await setup(presetFiles("temp"));
  const call = api(root, presets).call;
  try {
    await call("POST", "/v1/presets/publish", {});
    assert.equal((await call("POST", "/v1/presets/archive", { name: "temp" })).status, 200);
    assert.equal(await exists(path.join(root, SECTION, "temp")), false);
    assert.equal(await exists(path.join(root, SECTION, ".archive", "temp", "preset.json")), true);

    const overview = await call("GET", "/v1/presets");
    assert.deepEqual(overview.body.archived.map((entry) => entry.name), ["temp"]);
    assert.equal(overview.body.presets.length, 0);

    assert.equal((await call("POST", "/v1/presets/restore", { name: "temp" })).status, 200);
    assert.equal(await exists(path.join(root, SECTION, "temp", "preset.json")), true);

    await call("POST", "/v1/presets/archive", { name: "temp" });
    assert.equal((await call("POST", "/v1/presets/purge", { name: "temp" })).status, 200);
    assert.equal(await exists(path.join(root, SECTION, ".archive", "temp")), false);
  } finally {
    await cleanup(root);
  }
});

test("archive：工作区里没有这条 → 404（别把「不在」当成「归档成功」）", async () => {
  const { root, presets } = await setup();
  try {
    assert.equal((await api(root, presets).call("POST", "/v1/presets/archive", { name: "nope" })).status, 404);
  } finally {
    await cleanup(root);
  }
});

// ── 类别 / 待同步 / 重扫 ────────────────────────────────────────────────────

test("类别：新建 / 改名整枝 / 删除整枝（删掉的类别会从每条预设里摘掉）", async () => {
  const { root, presets } = await setup(presetFiles("one", { category: ["Basics"] }));
  const call = api(root, presets).call;
  try {
    const added = await call("POST", "/v1/presets/categories", { op: "add", name: "工程/审查" });
    assert.equal(added.status, 200);
    assert.ok(added.body.categories.includes("工程/审查"));

    const renamed = await call("POST", "/v1/presets/categories", { op: "rename", name: "工程", to: "技术" });
    assert.ok(renamed.body.categories.includes("技术/审查"));

    const removed = await call("POST", "/v1/presets/categories", { op: "remove", name: "技术" });
    assert.equal(removed.status, 200);
    assert.equal(removed.body.categories.includes("技术/审查"), false);
  } finally {
    await cleanup(root);
  }
});

test("category：改单条的类别 → 写 preset.json 并重扫（词表里有它）", async () => {
  const { root, presets } = await setup(presetFiles("one"));
  const call = api(root, presets).call;
  try {
    await call("POST", "/v1/presets/categories", { op: "add", name: "工程" });
    const got = await call("POST", "/v1/presets/category", { name: "one", category: ["工程"] });
    assert.equal(got.status, 200);
    assert.deepEqual(JSON.parse(await read(root, "one/preset.json")).category, ["工程"]);
    assert.ok(got.body.categories.includes("工程"));
  } finally {
    await cleanup(root);
  }
});

test("pending：改一个文件之后按「预设」分组列出来（单元是预设不是文件）", async () => {
  const { root, presets } = await setup(presetFiles("one"));
  const call = api(root, presets).call;
  try {
    await call("POST", "/v1/presets/publish", {});
    await call("PUT", "/v1/presets/file", {
      name: "one",
      path: "preset.json",
      text: JSON.stringify({ v: 1, name: "one", summary: "改了", category: [] }),
    });
    const got = await call("GET", "/v1/presets/pending");
    assert.equal(got.status, 200);
    const unit = got.body.units.find((item) => item.key === "one");
    assert.ok(unit);
    assert.equal(unit.kind, "preset");
    assert.equal(unit.files[0].path, "one/preset.json");
  } finally {
    await cleanup(root);
  }
});

test("reload：命令行里直接改了盘 → 点重扫，名册立刻跟上", async () => {
  const { root, presets } = await setup();
  const call = api(root, presets).call;
  try {
    const { mkdir, writeFile } = await import("node:fs/promises");
    await mkdir(path.join(root, SECTION, "handmade"));
    await writeFile(
      path.join(root, SECTION, "handmade", "preset.json"),
      JSON.stringify({ v: 1, name: "handmade", summary: "手写的", category: [] }),
    );
    const before = await call("GET", "/v1/presets");
    assert.equal(before.body.presets.some((item) => item.name === "handmade"), false);

    const reloaded = await call("POST", "/v1/presets/reload", {});
    assert.equal(reloaded.status, 200);
    assert.ok(reloaded.body.presets.some((item) => item.name === "handmade"));
  } finally {
    await cleanup(root);
  }
});

test("未知路径 → 404 带路径（别拿一个 200 空对象糊过去）", async () => {
  const { root, presets } = await setup();
  try {
    const got = await api(root, presets).call("GET", "/v1/presets/nope");
    assert.equal(got.status, 404);
    assert.equal(got.body.path, "nope");
  } finally {
    await cleanup(root);
  }
});

// ── 分段：一个仓放两段，各管各的 ────────────────────────────────────────────
//
// 这组测的就是「共用」不会偷偷变成「共享状态」。仓里另加一个 Ultimate-Template（形状还没定，
// 只是躺着），然后看 Session 的发布 / 待同步 / 脏标记会不会把它带走。

const withSecondSection = async (files = {}) => {
  const repoRoot = await makePresetsRepo(files);
  const { writeTree, commitAll } = await import("./helpers.js");
  await writeTree(repoRoot, "Ultimate-Template/categories.json", CATEGORIES);
  // 第二段里放一个没写完的文件：它不该被第一段的发布碰。
  await writeTree(repoRoot, "Ultimate-Template/.scratch.md", "还没写完\n");
  await commitAll(repoRoot, "plus ultimate");
  const presets = await createPresets({ dir: path.join(repoRoot, SECTION) });
  return { repoRoot, presets };
};

const twoSections = [
  { id: SECTION, title: "会话模板", manifest: "preset.json", checker: "preset" },
  { id: "Ultimate-Template", title: "终极模板", manifest: "ultimate.json", checker: null },
];

test("GET /v1/presets：仓的目录（每段几条），段名从这里来", async () => {
  const { repoRoot, presets } = await withSecondSection(presetFiles("one"));
  try {
    const url = new URL("http://block.local/v1/presets");
    const { res, out } = makeRes();
    await handlePresets({
      req: makeReq("GET"),
      res,
      url,
      parts: ["v1", "presets"],
      repoRoot,
      sections: twoSections,
      presetsOf: (id) => (id === SECTION ? presets : null),
      repo,
      ctx: fakeCtx,
    });
    assert.equal(out.status, 200);
    assert.deepEqual(
      out.body.sections.map((section) => [section.id, section.count]),
      [
        [SECTION, 1],
        ["Ultimate-Template", null], // 形状还没定 = 数不出来，不是 0
      ],
    );
  } finally {
    await cleanup(repoRoot);
  }
});

test("没登记的段 → 404（不猜、不退回默认段）", async () => {
  const { root, presets } = await setup(presetFiles("one"));
  try {
    const got = await api(root, presets).call("GET", "/v1/presets/Nope-Template", null, { raw: true });
    assert.equal(got.status, 404);
    assert.match(got.body.error, /没有这一段：Nope-Template/);
  } finally {
    await cleanup(root);
  }
});

test("发布一段不会带走另一段没写完的文件（一个库最容易烂的地方）", async () => {
  const { repoRoot, presets } = await withSecondSection(presetFiles("one"));
  const call = api(repoRoot, presets).call;
  try {
    // 另一段留一条未提交的改动。
    const { writeTree } = await import("./helpers.js");
    await writeTree(repoRoot, "Ultimate-Template/notes.md", "草稿\n");

    const made = await call("POST", "/v1/presets/publish", {});
    assert.equal(made.status, 200);
    const committed = (await showNames(repoRoot, "HEAD")).map((line) => line.split("\t")[1]);
    // 这一段的索引跟着上去了……
    assert.ok(committed.some((name) => name.startsWith(`${SECTION}/`)));
    // ……另一段的两条脏文件一条都没进这次提交。
    assert.equal(committed.includes("Ultimate-Template/notes.md"), false);
    assert.equal(committed.includes("Ultimate-Template/.scratch.md"), false);

    // 待同步清单也只报自己那段：这一段的发布刚做完，它就是干净的 ——
    // 哪怕另一段这会儿有未提交的文件（`dirty` 必须是 false，否则「分开管理」是假的）。
    const pending = await call("GET", "/v1/presets/pending");
    assert.equal(pending.body.dirty, false);
    assert.deepEqual(pending.body.units, []);
    // 而整个仓是脏的（另一段那条没写完的）：脏不脏是两回事，别混成一个 chip。
    assert.deepEqual((await repo.gitChanges(repoRoot)).map((item) => item.path), ["Ultimate-Template/notes.md"]);
  } finally {
    await cleanup(repoRoot);
  }
});

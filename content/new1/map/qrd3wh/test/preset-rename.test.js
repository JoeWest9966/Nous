// `POST /v1/presets/<段>/rename` —— 改一条模板的名字。
//
// 名字 = 目录名 = 主键（形状校验要求 `name` 等于目录名），所以一次改名要同时动三处：
// 目录（`git mv`，保历史）、清单里的 `name`、以及**引用了它的终极模板的 `session`**。
// 少做一件就是红：只改目录 → `specError`；不改引用 → 别人的终极模板红。
//
// 这里用**真两段**的临时仓（Session-Template + Ultimate-Template），因为「引用连带改」
// 是跨段的 —— 单段的夹具测不出来。

import test from "node:test";
import assert from "node:assert/strict";
import { mkdir, readFile } from "node:fs/promises";
import path from "node:path";

import { handlePresets } from "../presets-api.js";
import { createPresets } from "../presets.js";
import * as repo from "../repo-tools.js";
import { CATEGORIES, cleanup, exists, head, makeRepo, makeReq, makeRes, showNames, status, writeTree } from "./helpers.js";

const SESSION = "Session-Template";
const ULTIMATE = "Ultimate-Template";

/** 两段的迷你生成器：各扫各的目录、各写各的 `index.json`。 */
const TWO_SECTIONS_INDEX = `import { readdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.dirname(fileURLToPath(import.meta.url));
for (const [section, manifest] of [["${SESSION}", "preset.json"], ["${ULTIMATE}", "ultimate.json"]]) {
  const root = path.join(repoRoot, section);
  const items = [];
  for (const entry of await readdir(root, { withFileTypes: true }).catch(() => [])) {
    if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
    let raw;
    try {
      raw = await readFile(path.join(root, entry.name, manifest), "utf8");
    } catch {
      continue;
    }
    const spec = JSON.parse(raw);
    if (spec.name !== entry.name) {
      console.error(entry.name + ": name 跟目录名对不上");
      process.exit(1);
    }
    items.push({ name: entry.name, summary: spec.summary ?? "", category: spec.category ?? [] });
  }
  items.sort((a, b) => a.name.localeCompare(b.name));
  await writeFile(
    path.join(root, "index.json"),
    JSON.stringify({ v: 1, section, items }, null, 2) + "\\n",
  );
}
`;

const SECTIONS_JSON = JSON.stringify(
  {
    v: 1,
    sections: [
      { id: SESSION, title: "会话模板", manifest: "preset.json", checker: "preset" },
      { id: ULTIMATE, title: "终极模板", manifest: "ultimate.json", checker: "ultimate" },
    ],
  },
  null,
  2,
);

const SECTIONS = [
  { id: SESSION, title: "会话模板", manifest: "preset.json", checker: "preset" },
  { id: ULTIMATE, title: "终极模板", manifest: "ultimate.json", checker: "ultimate" },
];

const presetFile = (name, spec = {}) =>
  JSON.stringify({ v: 1, name, summary: `${name} 的一句话`, category: [], ...spec }, null, 2) + "\n";

/** 一条终极模板：它用 `session` 指着会话模板段里的名字。 */
const ultimateFile = (name, session, extra = {}) =>
  JSON.stringify(
    {
      v: 1,
      name,
      summary: `${name} 的一句话`,
      category: [],
      trigger: "single-shot",
      params: { epoch: 1800000060 },
      session,
      enabled: true,
      ...extra,
    },
    null,
    2,
  ) + "\n";

/** 两段的临时仓。`session` / `ultimate` 是段内相对路径 → 文本。 */
const setup = async ({ session = {}, ultimate = {} } = {}) => {
  const files = {
    "sections.json": SECTIONS_JSON,
    "index.mjs": TWO_SECTIONS_INDEX,
    [`${SESSION}/categories.json`]: CATEGORIES,
    [`${ULTIMATE}/categories.json`]: CATEGORIES,
  };
  for (const [rel, text] of Object.entries(session)) files[`${SESSION}/${rel}`] = text;
  for (const [rel, text] of Object.entries(ultimate)) files[`${ULTIMATE}/${rel}`] = text;
  const repoRoot = await makeRepo({ files });
  const rosters = {
    [SESSION]: await createPresets({ dir: path.join(repoRoot, SESSION) }),
    [ULTIMATE]: await createPresets({
      dir: path.join(repoRoot, ULTIMATE),
      manifest: "ultimate.json",
      checker: "ultimate",
    }),
  };
  return { repoRoot, rosters };
};

const api = (repoRoot, rosters) => ({
  call: async (method, target, body) => {
    const url = new URL(`http://block.local${target}`);
    const parts = url.pathname.split("/").filter(Boolean);
    const req = makeReq(method, body);
    const { res, out } = makeRes();
    await handlePresets({
      req,
      res,
      url,
      parts,
      repoRoot,
      sections: SECTIONS,
      presetsOf: (id) => rosters[id] ?? null,
      repo,
      ctx: {},
    });
    return { status: out.status, body: out.body };
  },
});

const read = (root, rel) => readFile(path.join(root, rel), "utf8");

// ── 成功路径 ────────────────────────────────────────────────────────────────

test("rename：目录 + 清单 name 一起动，引用它的终极模板跟着改，一条提交", async () => {
  const { repoRoot, rosters } = await setup({
    session: { "weekly/preset.json": presetFile("weekly") },
    ultimate: { "digest/ultimate.json": ultimateFile("digest", "weekly") },
  });
  try {
    const before = await head(repoRoot);
    const got = await api(repoRoot, rosters).call("POST", `/v1/presets/${SESSION}/rename`, {
      name: "weekly",
      to: "weekly-report",
    });
    assert.equal(got.status, 200);
    // 连带改了谁要回给调用方（界面得说「还动了别人」，不能悄悄改）。
    assert.deepEqual(got.body.refs, ["digest"]);

    // 盘上：旧目录没了、新目录在，清单里的 name 跟上了。
    assert.equal(await exists(path.join(repoRoot, SESSION, "weekly")), false);
    assert.equal(await exists(path.join(repoRoot, SESSION, "weekly-report/preset.json")), true);
    assert.equal(JSON.parse(await read(repoRoot, `${SESSION}/weekly-report/preset.json`)).name, "weekly-report");

    // 引用：终极模板的 `session` 一起改（不改的话它立刻是「会话模板不在」的红）。
    assert.equal(JSON.parse(await read(repoRoot, `${ULTIMATE}/digest/ultimate.json`)).session, "weekly-report");

    // 一条提交，且**是 git 认得出的改名**（R），不是「删掉再新建」—— 历史要留下。
    assert.notEqual(await head(repoRoot), before);
    const names = await showNames(repoRoot, "HEAD");
    assert.ok(
      names.some((line) => line.startsWith("R") && line.includes(`${SESSION}/weekly/preset.json`)),
      `HEAD 里该有改名记录，实际：${names.join(" | ")}`,
    );
    assert.ok(names.some((line) => line.includes(`${ULTIMATE}/digest/ultimate.json`)));

    // 工作区干净：改名不该留半截脏东西。
    assert.equal((await status(repoRoot)).trim(), "");

    // 名册也跟上了（不然面板显示新名字、建会话时还看不见它）。
    assert.deepEqual(rosters[SESSION].list().map((item) => item.name), ["weekly-report"]);
  } finally {
    await cleanup(repoRoot);
  }
});

test("rename：改名后的那条仍然过形状校验（name 等于目录名）", async () => {
  const { repoRoot, rosters } = await setup({ session: { "asd/preset.json": presetFile("asd") } });
  try {
    const got = await api(repoRoot, rosters).call("POST", `/v1/presets/${SESSION}/rename`, {
      name: "asd",
      to: "asd-2",
    });
    assert.equal(got.status, 200);
    const row = (await api(repoRoot, rosters).call("GET", `/v1/presets/${SESSION}`)).body.presets[0];
    assert.equal(row.name, "asd-2");
    assert.equal(row.specError, null);
  } finally {
    await cleanup(repoRoot);
  }
});

test("rename：没有终极模板引用它时，只动自己（不留多余的脏文件）", async () => {
  const { repoRoot, rosters } = await setup({
    session: { "solo/preset.json": presetFile("solo") },
    ultimate: { "other/ultimate.json": ultimateFile("other", "another-template") },
  });
  try {
    assert.equal(
      (await api(repoRoot, rosters).call("POST", `/v1/presets/${SESSION}/rename`, { name: "solo", to: "solo-2" }))
        .status,
      200,
    );
    assert.equal(JSON.parse(await read(repoRoot, `${ULTIMATE}/other/ultimate.json`)).session, "another-template");
    assert.equal((await status(repoRoot)).trim(), "");
  } finally {
    await cleanup(repoRoot);
  }
});

test("rename：未发布的那条也能改（`git mv` 认不了它，退回直接搬）", async () => {
  const { repoRoot, rosters } = await setup();
  await writeTree(repoRoot, `${SESSION}/fresh/preset.json`, presetFile("fresh"));
  try {
    assert.equal((await status(repoRoot)).includes("fresh"), true, "前提：这条是未跟踪的");
    const got = await api(repoRoot, rosters).call("POST", `/v1/presets/${SESSION}/rename`, {
      name: "fresh",
      to: "fresh-2",
    });
    assert.equal(got.status, 200);
    assert.equal(await exists(path.join(repoRoot, SESSION, "fresh")), false);
    assert.equal(await exists(path.join(repoRoot, SESSION, "fresh-2/preset.json")), true);
  } finally {
    await cleanup(repoRoot);
  }
});

// ── 拒绝路径：盘上必须一个字节都没动 ────────────────────────────────────────

test("rename：新名字被占了 → 409，两条都原样留着", async () => {
  const { repoRoot, rosters } = await setup({
    session: { "one/preset.json": presetFile("one"), "two/preset.json": presetFile("two") },
  });
  try {
    const before = await head(repoRoot);
    const got = await api(repoRoot, rosters).call("POST", `/v1/presets/${SESSION}/rename`, {
      name: "one",
      to: "two",
    });
    assert.equal(got.status, 409);
    assert.match(got.body.error, /two/);
    assert.equal(await exists(path.join(repoRoot, SESSION, "one/preset.json")), true);
    assert.equal(await exists(path.join(repoRoot, SESSION, "two/preset.json")), true);
    assert.equal(await head(repoRoot), before);
    assert.equal((await status(repoRoot)).trim(), "");
  } finally {
    await cleanup(repoRoot);
  }
});

test("rename：工作区里没有这条 → 404", async () => {
  const { repoRoot, rosters } = await setup({ session: { "one/preset.json": presetFile("one") } });
  try {
    const got = await api(repoRoot, rosters).call("POST", `/v1/presets/${SESSION}/rename`, {
      name: "ghost",
      to: "ghost-2",
    });
    assert.equal(got.status, 404);
    assert.match(got.body.error, /ghost/);
  } finally {
    await cleanup(repoRoot);
  }
});

test("rename：新名字不合格 / 和旧的一样 → 400，且一个字节都不动", async () => {
  const { repoRoot, rosters } = await setup({ session: { "one/preset.json": presetFile("one") } });
  try {
    const before = await head(repoRoot);
    const call = (to) =>
      api(repoRoot, rosters).call("POST", `/v1/presets/${SESSION}/rename`, { name: "one", to });

    assert.equal((await call("One")).status, 400, "大写不合格（形状规则是小写开头）");
    assert.equal((await call("")).status, 400);
    assert.equal((await call("one")).status, 400, "新旧同名没有意义");
    assert.equal((await call(".archive")).status, 400, "仓库自用的名字不许当预设名");
    assert.equal(await head(repoRoot), before);
    assert.equal((await status(repoRoot)).trim(), "");
  } finally {
    await cleanup(repoRoot);
  }
});

test("rename：只有 POST；GET 走 405", async () => {
  const { repoRoot, rosters } = await setup({ session: { "one/preset.json": presetFile("one") } });
  try {
    assert.equal((await api(repoRoot, rosters).call("GET", `/v1/presets/${SESSION}/rename`)).status, 405);
  } finally {
    await cleanup(repoRoot);
  }
});

// `POST /v1/presets/<段>/order` —— 手动排序。
//
// 顺序**不能写进 `preset.json`**：那两份键表（`PRESET_KEYS` / `ULTIMATE_KEYS`）是白名单，
// 多一个键就报「不认识这个键」。所以单独一个 `order.json` 放段目录里 —— 是**文件不是目录**，
// `scan` 和 `index.mjs` 都只认目录，不会把它当成一条模板。
//
// 全局一份：所有类别共用一套顺序，跟通讯录一样。

import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";

import { handlePresets } from "../presets-api.js";
import { ORDER_FILE, applyOrder, createPresets, readOrder } from "../presets.js";
import * as repo from "../repo-tools.js";
import { CATEGORIES, cleanup, head, makeRepo, makeReq, makeRes, showNames, status, writeTree } from "./helpers.js";

const SECTION = "Session-Template";
const SECTIONS = [{ id: SECTION, title: "会话模板", manifest: "preset.json", checker: "preset" }];

const presetFile = (name) => JSON.stringify({ v: 1, name, summary: `${name} 的一句话`, category: [] }, null, 2) + "\n";

/** 三条：字母序是 alpha / bravo / charlie —— 测试里要把它们排成别的顺序。 */
const setup = async () => {
  const files = {
    [`${SECTION}/categories.json`]: CATEGORIES,
    [`${SECTION}/alpha/preset.json`]: presetFile("alpha"),
    [`${SECTION}/bravo/preset.json`]: presetFile("bravo"),
    [`${SECTION}/charlie/preset.json`]: presetFile("charlie"),
  };
  const repoRoot = await makeRepo({ files });
  const presets = await createPresets({ dir: path.join(repoRoot, SECTION) });
  return { repoRoot, presets };
};

const api = (repoRoot, presets) => ({
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
      presetsOf: (id) => (id === SECTION ? presets : null),
      repo,
      ctx: {},
    });
    return { status: out.status, body: out.body, raw: out.text };
  },
});

const names = (body) => body.presets.map((row) => row.name);

// ── 纯函数 ──────────────────────────────────────────────────────────────────

test("applyOrder：表里没有的排后面，按名字（新模板位置稳定，不是每刷新一次换一处）", () => {
  const items = [{ name: "alpha" }, { name: "bravo" }, { name: "charlie" }];
  assert.deepEqual(
    applyOrder(items, ["charlie", "alpha"]).map((i) => i.name),
    ["charlie", "alpha", "bravo"],
  );
  // 空表 / 没表 = 原样（调用方已经按名字排好了）。
  assert.deepEqual(applyOrder(items, []).map((i) => i.name), ["alpha", "bravo", "charlie"]);
  assert.deepEqual(applyOrder(items, null).map((i) => i.name), ["alpha", "bravo", "charlie"]);
  // 表里多出来的名字（已经删掉的模板）不影响剩下的。
  assert.deepEqual(applyOrder(items, ["ghost", "bravo"]).map((i) => i.name), ["bravo", "alpha", "charlie"]);
});

test("readOrder：读不到 / 读坏了都当「没有顺序」，不许让整段打不开", async () => {
  const { repoRoot } = await setup();
  try {
    assert.deepEqual(await readOrder(path.join(repoRoot, SECTION)), []);
    await writeTree(repoRoot, `${SECTION}/${ORDER_FILE}`, "{ 这不是 json");
    assert.deepEqual(await readOrder(path.join(repoRoot, SECTION)), []);
    await writeTree(repoRoot, `${SECTION}/${ORDER_FILE}`, JSON.stringify({ v: 1, order: "不是数组" }));
    assert.deepEqual(await readOrder(path.join(repoRoot, SECTION)), []);
    await writeTree(repoRoot, `${SECTION}/${ORDER_FILE}`, JSON.stringify({ v: 1, order: [" bravo ", 42, ""] }));
    assert.deepEqual(await readOrder(path.join(repoRoot, SECTION)), ["bravo"]);
  } finally {
    await cleanup(repoRoot);
  }
});

// ── 端点 ────────────────────────────────────────────────────────────────────

test("order：写表 → 落盘 + 一条提交 → 列表顺序跟着变", async () => {
  const { repoRoot, presets } = await setup();
  try {
    assert.deepEqual(names((await api(repoRoot, presets).call("GET", `/v1/presets/${SECTION}`)).body), [
      "alpha",
      "bravo",
      "charlie",
    ]);
    const before = await head(repoRoot);
    const got = await api(repoRoot, presets).call("POST", `/v1/presets/${SECTION}/order`, {
      order: ["charlie", "bravo", "alpha"],
    });
    assert.equal(got.status, 200);
    assert.deepEqual(got.body.order, ["charlie", "bravo", "alpha"]);

    // 盘上：那个文件真的是文件（不会变成一条模板）。
    const stored = JSON.parse(await readFile(path.join(repoRoot, SECTION, ORDER_FILE), "utf8"));
    assert.deepEqual(stored.order, ["charlie", "bravo", "alpha"]);

    // 列表顺序跟上（roster 也 reload 了，不然下一次开页面还是旧的）。
    assert.deepEqual(names((await api(repoRoot, presets).call("GET", `/v1/presets/${SECTION}`)).body), [
      "charlie",
      "bravo",
      "alpha",
    ]);

    // 一条提交，只动那个文件。
    assert.notEqual(await head(repoRoot), before);
    const names2 = await showNames(repoRoot, "HEAD");
    assert.equal(names2.length, 1, `该只有一个文件，实际：${names2.join(" | ")}`);
    assert.match(names2[0], /order\.json/);
    assert.equal((await status(repoRoot)).trim(), "");

    // 它**没有**变成第四条模板。
    const list = (await api(repoRoot, presets).call("GET", `/v1/presets/${SECTION}`)).body.presets;
    assert.equal(list.length, 3);
  } finally {
    await cleanup(repoRoot);
  }
});

test("order：盘上不存在的名字被滤掉（删掉的模板不该留在表里）", async () => {
  const { repoRoot, presets } = await setup();
  try {
    const got = await api(repoRoot, presets).call("POST", `/v1/presets/${SECTION}/order`, {
      order: ["ghost", "bravo", "alpha", "bravo"],
    });
    assert.equal(got.status, 200);
    assert.deepEqual(got.body.order, ["bravo", "alpha"]);
  } finally {
    await cleanup(repoRoot);
  }
});

test("order：GET 回当前表；没写过就是空数组", async () => {
  const { repoRoot, presets } = await setup();
  try {
    assert.deepEqual((await api(repoRoot, presets).call("GET", `/v1/presets/${SECTION}/order`)).body.order, []);
    await api(repoRoot, presets).call("POST", `/v1/presets/${SECTION}/order`, { order: ["bravo"] });
    assert.deepEqual((await api(repoRoot, presets).call("GET", `/v1/presets/${SECTION}/order`)).body.order, [
      "bravo",
    ]);
  } finally {
    await cleanup(repoRoot);
  }
});

test("order：body 不是数组 → 400，盘上不动", async () => {
  const { repoRoot, presets } = await setup();
  try {
    const before = await head(repoRoot);
    assert.equal((await api(repoRoot, presets).call("POST", `/v1/presets/${SECTION}/order`, { order: "nope" })).status, 400);
    assert.equal((await api(repoRoot, presets).call("POST", `/v1/presets/${SECTION}/order`, {})).status, 400);
    assert.equal(await head(repoRoot), before);
  } finally {
    await cleanup(repoRoot);
  }
});

test("order：PUT 走 405", async () => {
  const { repoRoot, presets } = await setup();
  try {
    assert.equal((await api(repoRoot, presets).call("PUT", `/v1/presets/${SECTION}/order`, { order: [] })).status, 405);
  } finally {
    await cleanup(repoRoot);
  }
});

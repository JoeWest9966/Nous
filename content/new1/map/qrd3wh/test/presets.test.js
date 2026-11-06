// `presets.js` 的行为 —— 纯逻辑，不起引擎、不碰 git。
//
// 这张「形状矩阵」跟 `Nous-presets/test/index.test.js` 里那张是**同一张**。两处实现
// （那个仓要能单独 checkout、不许 import 引擎），靠这两张表互相咬住 —— 漂了会红。
//
// 最重要的一组是「三态」：`template` 缺席 / `null` / 字符串必须展开成**不同的东西**。
// 没有这一组，那三种语义一定会在某个 refactor 里合并掉（引擎注释说它已经错过一次）。

import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import {
  THINKING_LEVELS,
  createPresets,
  expandPresetSpec,
  presetContext,
  referenceProblems,
  resolveSpec,
  shapeProblems,
  validatePresetSpec,
} from "../presets.js";

const minimal = (name, extra = {}) => ({ v: 1, name, summary: `${name} 的一句话`, category: [], ...extra });

/** 假 ctx：只有这两个技能、一个模板、一台模型，档位两档。 */
const fakeCtx = (over = {}) => ({
  skillsHas: (name) => ["code-review", "deep-dive"].includes(name),
  promptsHas: (name) => ["smart-ass", "one-sentence"].includes(name),
  extensionsHas: (name) => ["nous", "skills"].includes(name),
  catalogHas: (ref) => ref.provider === "volc-ark",
  levelsOf: (ref) => (ref?.provider === "volc-ark" ? ["off", "medium", "high"] : []),
  deviceModel: { provider: "volc-ark", modelId: "deepseek-v4-flash-260425" },
  ...over,
});

// ── 形状矩阵（与 Nous-presets/test/index.test.js 同一张）────────────────────

test("形状：最小可用的那一条过", () => {
  assert.deepEqual(shapeProblems(minimal("a"), { dirName: "a" }), []);
});

test("形状：未知键必须报错（拼错的键等于那条设置静默没生效）", () => {
  const problems = shapeProblems({ ...minimal("a"), tempalte: "x" }, { dirName: "a" });
  assert.equal(problems.length, 1);
  assert.match(problems[0], /tempalte/);
});

test("形状：v 不是 1 过不了", () => {
  assert.match(shapeProblems({ ...minimal("a"), v: 2 }, { dirName: "a" }).join(), /v 得是 1/);
});

test("形状：name 必须等于目录名；没有目录名时只要求名字合法", () => {
  assert.match(shapeProblems(minimal("a"), { dirName: "b" }).join(), /必须等于目录名/);
  assert.deepEqual(shapeProblems(minimal("a-b_c.d")), []);
  assert.match(shapeProblems(minimal("A")).join(), /不合格/);
  assert.match(shapeProblems({ ...minimal("a"), name: "" }).join(), /缺 name/);
});

test("形状：summary 为空 / 太长都不过", () => {
  assert.match(shapeProblems({ ...minimal("a"), summary: "  " }, { dirName: "a" }).join(), /summary 为空/);
  assert.match(
    shapeProblems({ ...minimal("a"), summary: "x".repeat(201) }, { dirName: "a" }).join(),
    /summary 太长/,
  );
});

test("形状：description 得是字符串且不超 2000", () => {
  assert.deepEqual(shapeProblems(minimal("a", { description: "说明书" }), { dirName: "a" }), []);
  assert.match(shapeProblems(minimal("a", { description: 3 }), { dirName: "a" }).join(), /得是字符串/);
  assert.match(
    shapeProblems(minimal("a", { description: "x".repeat(2001) }), { dirName: "a" }).join(),
    /太长/,
  );
});

test("形状：category 必须是字符串数组（没有就写 []）", () => {
  assert.match(shapeProblems({ v: 1, name: "a", summary: "s" }, { dirName: "a" }).join(), /缺 category 数组/);
  assert.match(shapeProblems(minimal("a", { category: [1] }), { dirName: "a" }).join(), /得是字符串数组/);
});

test("形状：model 要么 {provider, modelId} 要么整个键不写；null 不过", () => {
  assert.deepEqual(shapeProblems(minimal("a", { model: { provider: "p", modelId: "m" } }), { dirName: "a" }), []);
  assert.match(shapeProblems(minimal("a", { model: null }), { dirName: "a" }).join(), /model 得是/);
  assert.match(
    shapeProblems(minimal("a", { model: { provider: "p" } }), { dirName: "a" }).join(),
    /只能有 provider 和 modelId/,
  );
  assert.match(shapeProblems(minimal("a", { model: { provider: "", modelId: "m" } }), { dirName: "a" }).join(), /provider 为空/);
});

test("形状：thinkingLevel 只认那七档", () => {
  for (const level of THINKING_LEVELS) {
    assert.deepEqual(shapeProblems(minimal("a", { thinkingLevel: level }), { dirName: "a" }), [], level);
  }
  assert.match(shapeProblems(minimal("a", { thinkingLevel: "maxx" }), { dirName: "a" }).join(), /不在/);
});

test("形状：skills / extensions 是数组，[] 是有效值，null 不是", () => {
  assert.deepEqual(shapeProblems(minimal("a", { skills: [], extensions: [] }), { dirName: "a" }), []);
  assert.match(shapeProblems(minimal("a", { skills: null }), { dirName: "a" }).join(), /skills 得是字符串数组/);
  assert.match(shapeProblems(minimal("a", { skills: [""] }), { dirName: "a" }).join(), /空的/);
});

test("形状：template 三态 —— 缺席 / null / 字符串都过，空串和数字不过", () => {
  assert.deepEqual(shapeProblems(minimal("a"), { dirName: "a" }), []);
  assert.deepEqual(shapeProblems(minimal("a", { template: null }), { dirName: "a" }), []);
  assert.deepEqual(shapeProblems(minimal("a", { template: "smart-ass" }), { dirName: "a" }), []);
  assert.match(shapeProblems(minimal("a", { template: "" }), { dirName: "a" }).join(), /template 得是模板名/);
  assert.match(shapeProblems(minimal("a", { template: 3 }), { dirName: "a" }).join(), /template 得是模板名/);
});

test("形状：readOnly 得是布尔", () => {
  assert.deepEqual(shapeProblems(minimal("a", { readOnly: true }), { dirName: "a" }), []);
  assert.match(shapeProblems(minimal("a", { readOnly: "yes" }), { dirName: "a" }).join(), /readOnly 得是 true/);
});

// ── 三态：这一组是安全带 ───────────────────────────────────────────────────

test("三态：template 缺席 / null / 字符串 展开成三种不同的东西", () => {
  assert.equal("template" in expandPresetSpec(minimal("a")), false);
  assert.equal("template" in expandPresetSpec(minimal("a", { template: null })), true);
  assert.equal(expandPresetSpec(minimal("a", { template: null })).template, null);
  assert.equal(expandPresetSpec(minimal("a", { template: "smart-ass" })).template, "smart-ass");
});

test("三态：template:auto 被规范化成键缺席（hb7k3j 的 picker 用它表示「跟随默认」）", () => {
  assert.equal("template" in expandPresetSpec(minimal("a", { template: "auto" })), false);
  assert.equal(resolveSpec(minimal("a", { template: "auto" }), fakeCtx()).template.mode, "default");
});

test("三态：skills / extensions 缺席不出现，[] 原样是 []", () => {
  const absent = expandPresetSpec(minimal("a"));
  assert.equal("skills" in absent, false);
  assert.equal("extensions" in absent, false);
  assert.deepEqual(expandPresetSpec(minimal("a", { skills: [] })).skills, []);
  assert.deepEqual(expandPresetSpec(minimal("a", { extensions: [] })).extensions, []);
});

test("三态：model / thinkingLevel / readOnly 缺席不出现（不填默认值）", () => {
  const out = expandPresetSpec(minimal("a"));
  assert.deepEqual(Object.keys(out), []);
  assert.equal(expandPresetSpec(minimal("a", { readOnly: false })).readOnly, false);
  assert.equal("readOnly" in expandPresetSpec(minimal("a")), false);
});

// ── 引用校验 ────────────────────────────────────────────────────────────────

test("引用：技能 / 模板 / 扩展 / 模型都在 → 没有错误", () => {
  const spec = minimal("a", {
    skills: ["code-review"],
    extensions: ["nous"],
    template: "smart-ass",
    model: { provider: "volc-ark", modelId: "deepseek-v4-flash-260425" },
  });
  assert.deepEqual(referenceProblems(spec, fakeCtx()).errors, []);
});

test("引用：技能不在 → 点名是哪条", () => {
  const { errors } = referenceProblems(minimal("a", { skills: ["gone"] }), fakeCtx());
  assert.equal(errors.length, 1);
  assert.match(errors[0], /技能「gone」没装/);
});

test("引用：模板不在 → 点名；template:null 不算「不在」", () => {
  assert.match(referenceProblems(minimal("a", { template: "gone" }), fakeCtx()).errors[0], /模板「gone」不在/);
  assert.deepEqual(referenceProblems(minimal("a", { template: null }), fakeCtx()).errors, []);
  assert.deepEqual(referenceProblems(minimal("a"), fakeCtx()).errors, []);
});

test("引用：名牌混进 extensions → 说清该写哪个字段", () => {
  const { errors } = referenceProblems(minimal("a", { extensions: ["prompt:smart-ass"] }), fakeCtx());
  assert.match(errors[0], /是名牌，不是扩展/);
});

test("引用：扩展没装 → 点名", () => {
  assert.match(referenceProblems(minimal("a", { extensions: ["herdr-agent-state"] }), fakeCtx()).errors[0], /没装/);
});

test("引用：模型不在编目 / 档位不在那台模型的档位表里 → 都报错", () => {
  const bad = minimal("a", { model: { provider: "nope", modelId: "x" }, thinkingLevel: "high" });
  assert.match(referenceProblems(bad, fakeCtx()).errors.join(" "), /不在编目里/);
  const badLevel = minimal("a", {
    model: { provider: "volc-ark", modelId: "deepseek-v4-flash-260425" },
    thinkingLevel: "xhigh",
  });
  assert.match(referenceProblems(badLevel, fakeCtx()).errors.join(" "), /不在 .* 支持的档位里/);
});

test("引用：钉了模型但档位合法 → 不报错", () => {
  const ok = minimal("a", {
    model: { provider: "volc-ark", modelId: "deepseek-v4-flash-260425" },
    thinkingLevel: "high",
  });
  assert.deepEqual(referenceProblems(ok, fakeCtx()).errors, []);
});

test("引用：没钉模型却有档位 → warning 不是 error（到时候按设备默认模型夹）", () => {
  const { errors, warnings } = referenceProblems(minimal("a", { thinkingLevel: "high" }), fakeCtx());
  assert.deepEqual(errors, []);
  assert.equal(warnings.length, 1);
  assert.match(warnings[0], /设备默认模型/);
});

// ── resolve：面板唯一可用的那份 ────────────────────────────────────────────

test("resolve：没钉模型 → state auto，value 是设备默认（面板别自己猜）", () => {
  const resolved = resolveSpec(minimal("a"), fakeCtx());
  assert.equal(resolved.model.state, "auto");
  assert.equal(resolved.model.value, "volc-ark/deepseek-v4-flash-260425");
  assert.deepEqual(resolved.model.thinkingLevels, ["off", "medium", "high"]);
  assert.equal(resolved.absence.model, true);
});

test("resolve：钉了不存在的模型 → invalid", () => {
  const resolved = resolveSpec(minimal("a", { model: { provider: "nope", modelId: "x" } }), fakeCtx());
  assert.equal(resolved.model.state, "invalid");
  assert.equal(resolved.model.ref, "nope/x");
});

test("resolve：template 的三种 mode 分得开", () => {
  assert.equal(resolveSpec(minimal("a"), fakeCtx()).template.mode, "default");
  assert.equal(resolveSpec(minimal("a", { template: null }), fakeCtx()).template.mode, "none");
  assert.equal(resolveSpec(minimal("a", { template: "smart-ass" }), fakeCtx()).template.mode, "picked");
  assert.equal(resolveSpec(minimal("a", { template: "gone" }), fakeCtx()).template.state, "invalid");
});

test("resolve：skills / extensions 逐条给 state，absence 说明键在不在", () => {
  const resolved = resolveSpec(
    minimal("a", { skills: ["code-review", "gone"], extensions: ["nous", "prompt:x"] }),
    fakeCtx(),
  );
  assert.deepEqual(resolved.skills.map((row) => row.state), ["ok", "invalid"]);
  assert.deepEqual(resolved.extensions.map((row) => row.state), ["ok", "invalid"]);
  assert.equal(resolved.absence.skills, false);
  assert.equal(resolved.absence.extensions, false);
  assert.equal(resolved.absence.template, true);
});

test("resolve：钉了模型但档位不合法 → thinking invalid（不改值，报红）", () => {
  const resolved = resolveSpec(
    minimal("a", { model: { provider: "volc-ark", modelId: "m" }, thinkingLevel: "xhigh" }),
    fakeCtx(),
  );
  assert.equal(resolved.thinking.state, "invalid");
  assert.equal(resolved.thinking.value, null);
});

// ── validatePresetSpec：形状先于引用 ───────────────────────────────────────

test("validate：形状不过就只回形状错误（不要拿引用错误盖住更基本的错）", () => {
  const out = validatePresetSpec({ ...minimal("a"), tempalte: "x", skills: ["gone"] }, fakeCtx(), { dirName: "a" });
  assert.equal(out.ok, false);
  assert.match(out.errors.join(" "), /tempalte/);
  assert.doesNotMatch(out.errors.join(" "), /gone/);
  assert.equal(out.resolved, null);
});

test("validate：引用不过 → ok:false 但 spec / resolved 都给（面板要能画红 chip 而不是白屏）", () => {
  const out = validatePresetSpec(minimal("a", { skills: ["gone"] }), fakeCtx(), { dirName: "a" });
  assert.equal(out.ok, false);
  assert.ok(out.spec);
  assert.equal(out.resolved.skills[0].state, "invalid");
});

// ── presetContext：宿主侧的判定只有这一处 ──────────────────────────────────

test("presetContext：把活的 registry / skills / prompts / models 折成问句函数", () => {
  const ctx = presetContext({
    skills: { has: (name) => name === "s1" },
    prompts: { has: (name) => name === "p1" },
    registry: { snapshot: () => ({ installed: () => [{ name: "ext1" }] }) },
    models: { getModel: (provider, id) => (provider === "p" ? { provider, id } : null) },
    levelsOf: () => ["off"],
    defaultModel: { provider: "p", modelId: "m" },
  });
  assert.equal(ctx.skillsHas("s1"), true);
  assert.equal(ctx.skillsHas("s2"), false);
  assert.equal(ctx.promptsHas("p1"), true);
  assert.equal(ctx.extensionsHas("ext1"), true);
  assert.equal(ctx.catalogHas({ provider: "p", modelId: "m" }), true);
  assert.equal(ctx.catalogHas({ provider: "q", modelId: "m" }), false);
  assert.deepEqual(ctx.levelsOf({ provider: "p", modelId: "m" }), ["off"]);
  assert.deepEqual(ctx.deviceModel, { provider: "p", modelId: "m" });
});

test("presetContext：registry 炸了 / 没给 → 一律「没有」而不是抛", () => {
  const ctx = presetContext({
    registry: {
      snapshot: () => {
        throw new Error("boooom");
      },
    },
  });
  assert.equal(ctx.extensionsHas("anything"), false);
});

// ── 扫盘 ────────────────────────────────────────────────────────────────────

async function makeRepo(presets) {
  const root = await mkdtemp(path.join(tmpdir(), "nous-presets-qrd3wh-"));
  for (const [name, spec] of Object.entries(presets)) {
    await mkdir(path.join(root, name), { recursive: true });
    await writeFile(path.join(root, name, "preset.json"), typeof spec === "string" ? spec : JSON.stringify(spec));
  }
  return root;
}

test("扫盘：没有 preset.json 的目录跳过，点开头的跳过，坏 JSON 留成 specError", async () => {
  const root = await makeRepo({ a: minimal("a"), b: "{ 坏了", ".archive": minimal("old") });
  await mkdir(path.join(root, "notes"));
  try {
    const presets = await createPresets({ dir: root });
    const names = presets.list().map((item) => item.name);
    assert.deepEqual(names, ["a", "b"]);
    assert.equal(presets.get("b").specError.includes("不是合法 JSON"), true);
    assert.equal(presets.get("a").specError, null);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("扫盘：形状坏了进 specError，引用坏了不算（那是「现在开不了」不是「写坏了」）", async () => {
  const root = await makeRepo({
    shape: { ...minimal("shape"), tempalte: "x" },
    ref: minimal("ref", { skills: ["gone"] }),
  });
  try {
    const presets = await createPresets({ dir: root });
    assert.match(presets.get("shape").specError, /tempalte/);
    assert.equal(presets.get("ref").specError, null);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("扫盘：仓库还不存在 → 空名册，不是错误", async () => {
  const presets = await createPresets({ dir: path.join(tmpdir(), "definitely-not-here-" + Date.now()) });
  assert.deepEqual(presets.list(), []);
});

test("扫盘：load() 之后新加的目录能被看见（写端点靠它）", async () => {
  const root = await makeRepo({ a: minimal("a") });
  try {
    const presets = await createPresets({ dir: root });
    assert.equal(presets.has("b"), false);
    await mkdir(path.join(root, "b"), { recursive: true });
    await writeFile(path.join(root, "b", "preset.json"), JSON.stringify(minimal("b")));
    assert.equal(presets.has("b"), false); // 没 load 就是没看见 —— 别指望它自己变
    await presets.load();
    assert.equal(presets.has("b"), true);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("扫盘：summary / category 进名册（面板卡片画的是这几个字段）", async () => {
  const root = await makeRepo({ a: minimal("a", { summary: "  一句话  ", category: ["工程", "工程"] }) });
  try {
    const presets = await createPresets({ dir: root });
    assert.equal(presets.get("a").summary, "一句话");
    assert.deepEqual(presets.get("a").category, ["工程"]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

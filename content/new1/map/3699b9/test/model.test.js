// `model.js` 的纯计算 —— 不碰 DOM，不碰 fetch。
//
// 这一层是这一页唯一的「判断的地方」：卡片算什么状态、这一屏该画哪些、chip 上写什么、
// 三态在表单里怎么摊开。服务端给的 `resolved` 是它的**输入**，不是它的替代品。
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  ancestorsOf,
  canLaunch,
  cardState,
  categoryImpact,
  chipRows,
  defaultSessionName,
  draftFromText,
  emptyHint,
  facetTree,
  filterPresets,
  isUnder,
  launchWarnings,
  nameLooksValid,
  specFromDraft,
  viewCounts,
  visibleViews,
} from "../public/model.js";

/** 一条「健康」的预设：服务端已经算好 resolved 的形状。 */
const healthy = (name, extra = {}) => ({
  name,
  summary: `${name} 的摘要`,
  category: [],
  tracked: true,
  specError: null,
  resolved: {
    name,
    model: { state: "auto", ref: null, value: "volc-ark/deepseek-v4-flash-260425", thinkingLevels: ["off", "medium"] },
    thinking: { state: "ok", ref: null, value: null },
    skills: [],
    extensions: [],
    template: { state: "ok", ref: null, value: null, mode: "default" },
    readOnly: false,
    absence: { skills: true, extensions: true, template: true, model: true, thinkingLevel: true, readOnly: true },
    ...(extra.resolved ?? {}),
  },
  ...Object.fromEntries(Object.entries(extra).filter(([key]) => key !== "resolved")),
});

// ── 卡片状态 ────────────────────────────────────────────────────────────────

test("状态：健康的 = ok", () => {
  assert.equal(cardState(healthy("a")), "ok");
});

test("状态：未发布 = unpublished（琥珀虚线，能启动）", () => {
  const preset = healthy("a", { tracked: false });
  assert.equal(cardState(preset), "unpublished");
  assert.equal(canLaunch(preset), true);
});

test("状态：写坏了 = failed（优先级最高，能盖住引用问题）", () => {
  const preset = { ...healthy("a"), specError: "preset.json 不是合法 JSON", resolved: null };
  assert.equal(cardState(preset), "failed");
  assert.equal(canLaunch(preset), false);
});

test("状态：任一引用失效 = invalid（挡启动，因为开了也是个跑不对的会话）", () => {
  const preset = healthy("a", {
    resolved: { skills: [{ state: "invalid", ref: "gone" }] },
  });
  assert.equal(cardState(preset), "invalid");
  assert.equal(canLaunch(preset), false);

  const modelBad = healthy("a", { resolved: { model: { state: "invalid", ref: "nope/x" } } });
  assert.equal(cardState(modelBad), "invalid");
});

test("状态：invalid 盖得住 unpublished（先修引用，再谈发布）", () => {
  const preset = healthy("a", { tracked: false, resolved: { template: { state: "invalid", ref: "x", mode: "picked" } } });
  assert.equal(cardState(preset), "invalid");
});

// ── 类别树 ──────────────────────────────────────────────────────────────────

test("类别：扁平路径 → 缩进树，父级计数含后代且按预设去重", () => {
  const tree = facetTree(
    ["工程", "工程/审查", "写作"],
    [
      { name: "a", category: ["工程", "工程/审查"] },
      { name: "b", category: ["工程/审查"] },
    ],
  );
  const byPath = Object.fromEntries(tree.map((node) => [node.path, node]));
  assert.equal(byPath["工程"].count, 2); // a、b；a 同时挂父子只算一次
  assert.equal(byPath["工程/审查"].count, 2);
  assert.equal(byPath["写作"].count, 0); // 词表里有、没人挂 → 保留，灰
  assert.equal(byPath["工程/审查"].depth, 1);
  assert.equal(byPath["工程"].label, "工程");
  assert.deepEqual(byPath["工程"].children, ["工程/审查"]);
});

test("类别：父级在词表里没有也能长出来（祖先补齐）", () => {
  const tree = facetTree(["写作/周报"], [{ name: "a", category: ["写作/周报"] }]);
  assert.deepEqual(tree.map((node) => node.path), ["写作", "写作/周报"]);
});

test("类别：排序是「先段数、再字典序」——子级永远贴着父级", () => {
  const tree = facetTree(["b/x", "a", "b", "a/y"], []);
  assert.deepEqual(tree.map((node) => node.path), ["a", "a/y", "b", "b/x"]);
});

test("类别：isUnder 认自己、认同前缀，但不认 a/bc 是 a/b 的子级", () => {
  assert.equal(isUnder("工程/审查", "工程"), true);
  assert.equal(isUnder("工程", "工程"), true);
  assert.equal(isUnder("工程x", "工程"), false);
  assert.equal(isUnder("工程/审查/深", "工程/审查"), true);
});

test("类别：ancestorsOf 给全链", () => {
  assert.deepEqual(ancestorsOf("a/b/c"), ["a", "a/b", "a/b/c"]);
});

// ── 筛选 / 视图 ────────────────────────────────────────────────────────────

test("视图：零计数的档不出现，「全部」永远在", () => {
  const rows = [{ ...healthy("a"), category: ["工程"] }];
  const keys = visibleViews(viewCounts(rows, [])).map((view) => view.key);
  assert.deepEqual(keys, ["all"]); // 有类别、已发布、没坏账 → 别的档全是零
});

test("视图：归档非空才出现「已归档」", () => {
  const counts = viewCounts([healthy("a")], [{ name: "old" }]);
  assert.ok(visibleViews(counts).some((view) => view.key === "archived"));
});

test("筛选：全部里坏账也看得见（藏着的坏账等于没有账）", () => {
  const rows = [healthy("ok"), { ...healthy("bad"), specError: "坏了", resolved: null }];
  assert.deepEqual(filterPresets(rows, { view: "all" }).map((row) => row.name), ["ok", "bad"]);
});

test("筛选：按类别筛出整枝", () => {
  const rows = [
    { ...healthy("a"), category: ["工程"] },
    { ...healthy("b"), category: ["工程/审查"] },
    { ...healthy("c"), category: ["写作"] },
  ];
  assert.deepEqual(filterPresets(rows, { category: "工程" }).map((row) => row.name), ["a", "b"]);
});

test("筛选：搜索打在名字 / 摘要 / 类别上，大小写不敏感", () => {
  const rows = [{ ...healthy("Weekly"), category: ["写作/周报"] }];
  assert.equal(filterPresets(rows, { query: "weekly" }).length, 1);
  assert.equal(filterPresets(rows, { query: "周报" }).length, 1);
  assert.equal(filterPresets(rows, { query: "zzz" }).length, 0);
});

test("筛选：未分类 = category 空数组", () => {
  const rows = [healthy("a"), { ...healthy("b"), category: ["工程"] }];
  assert.deepEqual(filterPresets(rows, { view: "uncategorized" }).map((row) => row.name), ["a"]);
});

test("空态：搜索没命中给「清空搜索」，不是「什么都没有」", () => {
  const hint = emptyHint({ view: "all", query: "zzz", presets: [healthy("a")] });
  assert.equal(hint.action.kind, "clear-search");
});

test("空态：一条都没有时说「私仓里还没有预设」", () => {
  const hint = emptyHint({ view: "all", category: null, query: "", presets: [], archived: [] });
  assert.match(hint.text, /还没有预设/);
  assert.equal(hint.action, null);
});

// ── chip ────────────────────────────────────────────────────────────────────

test("chip：三种「没选」长相不同 —— 默认（灰）/ 无（正常色）/ 都不要", () => {
  const dflt = chipRows(healthy("a"));
  assert.equal(dflt.find((row) => row.key === "template").state, "default");
  assert.equal(dflt.find((row) => row.key === "template").text, "默认");
  assert.equal(dflt.find((row) => row.key === "skills").text, "默认");

  const none = chipRows(
    healthy("a", {
      resolved: {
        template: { state: "ok", ref: null, value: null, mode: "none" },
        skills: [],
        absence: { skills: false, extensions: true, template: false },
      },
    }),
  );
  assert.equal(none.find((row) => row.key === "template").state, "none");
  assert.equal(none.find((row) => row.key === "template").text, "无");
  assert.equal(none.find((row) => row.key === "skills").text, "都不要");
});

test("chip：模型 auto 时显示「自动 · <设备默认>」—— 值来自 resolved，不自己猜", () => {
  const row = chipRows(healthy("a")).find((item) => item.key === "model");
  assert.equal(row.state, "auto");
  assert.match(row.text, /自动 · volc-ark\/deepseek-v4-flash-260425/);
});

test("chip：引用失效那条变红并显引用原文", () => {
  const rows = chipRows(healthy("a", { resolved: { template: { state: "invalid", ref: "gone", mode: "picked" } } }));
  const row = rows.find((item) => item.key === "template");
  assert.equal(row.state, "invalid");
  assert.equal(row.text, "gone");
});

test("chip：只读只在 true 时出现", () => {
  assert.equal(chipRows(healthy("a")).some((row) => row.key === "readOnly"), false);
  assert.equal(chipRows(healthy("a", { resolved: { readOnly: true } })).some((row) => row.key === "readOnly"), true);
});

test("启动警告：缺什么点名，跟随默认的写一句说明", () => {
  const warnings = launchWarnings(
    healthy("a", {
      resolved: {
        skills: [{ state: "invalid", ref: "gone" }],
        extensions: [{ state: "invalid", ref: "ext-gone" }],
        template: { state: "invalid", ref: "tmpl-gone", mode: "picked" },
      },
    }),
  );
  assert.deepEqual(warnings.missingSkills, ["gone"]);
  assert.deepEqual(warnings.missingExtensions, ["ext-gone"]);
  assert.equal(warnings.missingTemplate, "tmpl-gone");
  assert.ok(warnings.notes.some((note) => note.includes("自动") || note.includes("默认")));
});

// ── 表单三态 ────────────────────────────────────────────────────────────────

test("表单：preset.json → 草稿，「键缺席」和「null」摊成两种 mode", () => {
  const dflt = draftFromText(JSON.stringify({ v: 1, name: "a", summary: "s", category: [] }));
  assert.equal(dflt.templateMode, "default");
  assert.equal(dflt.skillsMode, "default");
  assert.equal(dflt.extMode, "default");

  const none = draftFromText(JSON.stringify({ v: 1, name: "a", summary: "s", category: [], template: null, skills: [] }));
  assert.equal(none.templateMode, "none");
  assert.equal(none.skillsMode, "none");
  assert.equal(none.extMode, "default"); // extensions 缺席 —— 这三态各管各的
});

test("表单：坏 JSON 给 broken，不许假装能编辑", () => {
  assert.equal(draftFromText("{ 坏了").broken, true);
});

test("表单：草稿 → preset.json 复原三态（这是安全带：塌一种就等于语义丢了）", () => {
  const dflt = specFromDraft(draftFromText(JSON.stringify({ v: 1, name: "a", summary: "s", category: [] })), "a");
  assert.equal("template" in dflt, false);
  assert.equal("skills" in dflt, false);
  assert.equal("extensions" in dflt, false);
  assert.equal("model" in dflt, false);
  assert.equal("thinkingLevel" in dflt, false);

  const none = specFromDraft(
    draftFromText(JSON.stringify({ v: 1, name: "a", summary: "s", category: [], template: null, skills: [] })),
    "a",
  );
  assert.equal(none.template, null);
  assert.deepEqual(none.skills, []);

  const picked = specFromDraft(
    {
      summary: "s",
      category: [],
      model: { provider: "volc-ark", modelId: "m" },
      thinkingLevel: "medium",
      skillsMode: "picked",
      skills: ["a", "b"],
      extMode: "none",
      extensions: [],
      templateMode: "picked",
      template: "smart-ass",
      readOnly: true,
    },
    "x",
  );
  assert.deepEqual(picked.model, { provider: "volc-ark", modelId: "m" });
  assert.equal(picked.thinkingLevel, "medium");
  assert.deepEqual(picked.skills, ["a", "b"]);
  assert.deepEqual(picked.extensions, []);
  assert.equal(picked.template, "smart-ass");
  assert.equal(picked.readOnly, true);
});

test("表单：readOnly:false 不写（缺席和 false 是同一件事，别给文件添噪音）", () => {
  const spec = specFromDraft({ summary: "s", category: [], readOnly: false }, "a");
  assert.equal("readOnly" in spec, false);
});

test("表单：来回一圈不改语义（文件 → 草稿 → 文件）", () => {
  const original = {
    v: 1,
    name: "a",
    summary: "s",
    category: ["工程"],
    description: "说明",
    model: { provider: "volc-ark", modelId: "m" },
    thinkingLevel: "high",
    skills: ["x"],
    extensions: [],
    template: null,
    readOnly: true,
  };
  const back = specFromDraft(draftFromText(JSON.stringify(original)), "a");
  assert.deepEqual(back, original);
});

// ── 类别影响范围 / 名字 / 会话名 ────────────────────────────────────────────

test("类别影响：删一整枝时先说清带多少条、多少子级", () => {
  const rows = [
    { ...healthy("a"), category: ["工程", "工程/审查"] },
    { ...healthy("b"), category: ["工程/审查/深"] },
    { ...healthy("c"), category: ["写作"] },
  ];
  const impact = categoryImpact(rows, "工程");
  assert.equal(impact.presets, 2);
  assert.equal(impact.children, 2); // 工程/审查、工程/审查/深
});

test("名字：乐观校验跟服务端同一套字符集（判定仍然在服务端）", () => {
  assert.equal(nameLooksValid("weekly-report"), true);
  assert.equal(nameLooksValid("Weekly"), false);
  assert.equal(nameLooksValid("-x"), false);
  assert.equal(nameLooksValid("a".repeat(65)), false);
});

test("会话名：预填「<预设> MM-DD HH:MM」", () => {
  const name = defaultSessionName("weekly", new Date(2026, 10, 6, 14, 32));
  assert.equal(name, "weekly 11-06 14:32");
});

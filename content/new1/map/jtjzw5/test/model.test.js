// model.js 的纯逻辑测试：一个 DOM 都不碰。
//
// 这一页只读，所以「算什么」比「画什么」重要：卡状态、chip 的三态、叠放顺序、筛选。

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  canLaunch,
  cardState,
  chipRows,
  emptyHint,
  facetTree,
  filterPresets,
  isUnder,
  launchBlockedReason,
  nameLooksValid,
  refState,
  refsOf,
  sortCategories,
  splitRef,
  stackOf,
  tileStyle,
  viewCounts,
  visibleViews,
} from "../public/model.js";

const KNOWN = { presets: new Set(["Session-Template/deep-dive", "Session-Template/scratch"]), prompts: new Set(["smart-ass"]) };

const ultimate = (extra = {}) => ({ name: "a", summary: "一句话", category: [], presets: [], prompt: "smart-ass", ...extra });

// ── 卡状态：优先级就是「先修哪儿」 ──────────────────────────────────────────

test("cardState：failed > invalid > unpublished > ok", () => {
  const good = refsOf(ultimate(), KNOWN);
  assert.equal(cardState({ refs: good }), "ok");
  assert.equal(cardState({ refs: good, tracked: false }), "unpublished");
  assert.equal(cardState({ refs: refsOf(ultimate({ presets: ["Session-Template/gone"] }), KNOWN) }), "invalid");
  // 自己写坏了就轮不到谈引用：那条连解析都做不了。
  assert.equal(
    cardState({ specError: "presets 得是数组", refs: refsOf(ultimate({ presets: ["Session-Template/gone"] }), KNOWN) }),
    "failed",
  );
  assert.equal(cardState({ specError: "坏了", tracked: false }), "failed");
});

test("refState：没有「已知清单」时是 unknown，不是 missing（不知道就别诬告）", () => {
  assert.equal(refState("Session-Template/deep-dive", KNOWN.presets), "ok");
  assert.equal(refState("Session-Template/gone", KNOWN.presets), "missing");
  assert.equal(refState("Session-Template/gone", undefined), "unknown");
});

// ── chip ────────────────────────────────────────────────────────────────────

const chipOf = (row, kind) => chipRows(row, KNOWN).find((chip) => chip.kind === kind);

test("chip：预设三种长相（缺席 / 空数组 / N 条）", () => {
  assert.equal(chipOf({ name: "a", summary: "s" }, "presets").state, "default");
  assert.equal(chipOf(ultimate({ presets: [] }), "presets").text, "预设 无");
  assert.equal(chipOf(ultimate({ presets: ["Session-Template/deep-dive"] }), "presets").text, "预设 1 条");
  const missing = chipOf(ultimate({ presets: ["Session-Template/deep-dive", "Session-Template/gone"] }), "presets");
  assert.equal(missing.state, "bad");
  assert.match(missing.text, /2 条 · 1 条不在/);
  assert.match(missing.title, /缺：Session-Template\/gone/);
});

test("chip：提示词三态 —— 「默认」和「无」必须是两句话", () => {
  assert.equal(chipOf({ name: "a", summary: "s" }, "prompt").text, "提示词 默认");
  assert.equal(chipOf({ name: "a", summary: "s", prompt: null }, "prompt").text, "提示词 无");
  assert.equal(chipOf(ultimate(), "prompt").text, "提示词 smart-ass");
  assert.equal(chipOf(ultimate({ prompt: "nope" }), "prompt").state, "bad");
});

test("chip：只读只在 true 时出现（false 是常态，不占一行）", () => {
  assert.equal(chipRows(ultimate({ readOnly: false }), KNOWN).some((chip) => chip.kind === "readOnly"), false);
  assert.equal(chipRows(ultimate({ readOnly: true }), KNOWN).some((chip) => chip.kind === "readOnly"), true);
});

// ── 类别 / 筛选 ─────────────────────────────────────────────────────────────

test("facetTree：父级计数含后代，零计数的类别留着但灰", () => {
  const rows = [
    { name: "x", category: ["工程/审查"] },
    { name: "y", category: ["工程"] },
  ];
  const tree = facetTree(["Basics", "工程", "工程/审查"], rows);
  const byName = Object.fromEntries(tree.map((node) => [node.name, node]));
  assert.equal(byName["工程"].count, 2);
  assert.equal(byName["工程/审查"].count, 1);
  assert.equal(byName.Basics.count, 0);
  assert.equal(byName["工程/审查"].depth, 1);
  assert.equal(byName["工程/审查"].label, "审查");
});

test("sortCategories：父在子前、同级按段比（跟引擎同一个比较器）", () => {
  assert.deepEqual(sortCategories(["写作/周报", "工程", "写作", "工程/审查"]), ["工程", "工程/审查", "写作", "写作/周报"]);
});

test("filterPresets：类别筛出整枝（含子类别）", () => {
  const rows = [
    { name: "a", category: ["工程/审查"], state: "ok" },
    { name: "b", category: ["写作"], state: "ok" },
  ];
  assert.deepEqual(filterPresets(rows, { category: "工程" }).map((row) => row.name), ["a"]);
  assert.deepEqual(filterPresets(rows, { category: "写作" }).map((row) => row.name), ["b"]);
  assert.equal(isUnder("工程/审查", "工程"), true);
  assert.equal(isUnder("工程学", "工程"), false);
});

test("filterPresets：view=all 不藏坏账（藏着的坏账等于没有账）", () => {
  const rows = [
    { name: "a", category: [], state: "invalid" },
    { name: "b", category: [], state: "unpublished" },
    { name: "c", category: [], state: "ok" },
  ];
  assert.equal(filterPresets(rows, { view: "all" }).length, 3);
  assert.deepEqual(filterPresets(rows, { view: "invalid" }).map((row) => row.name), ["a"]);
  assert.deepEqual(filterPresets(rows, { view: "unpublished" }).map((row) => row.name), ["b"]);
  // 归档是另一份数据，永远不进这张列表。
  assert.deepEqual(filterPresets(rows, { view: "archived" }), []);
});

test("filterPresets：搜名字 / 摘要 / 类别", () => {
  const rows = [{ name: "full-stack", summary: "一次开工", category: ["工程"], state: "ok" }];
  assert.equal(filterPresets(rows, { query: "full" }).length, 1);
  assert.equal(filterPresets(rows, { query: "开工" }).length, 1);
  assert.equal(filterPresets(rows, { query: "工程" }).length, 1);
  assert.equal(filterPresets(rows, { query: "nope" }).length, 0);
});

test("viewCounts / visibleViews：非零才出现，全部和已归档永远在", () => {
  const rows = [
    { name: "a", category: [], state: "invalid" },
    { name: "b", category: ["工程"], state: "ok" },
  ];
  const counts = viewCounts(rows, { archived: 0 });
  assert.deepEqual(counts, { all: 2, archived: 0, uncategorized: 1, invalid: 1, failed: 0, unpublished: 0 });
  assert.deepEqual(visibleViews(counts).map((view) => view.id), ["all", "uncategorized", "invalid", "archived"]);
});

test("emptyHint：整段空 vs 这一档空，两句话", () => {
  assert.match(emptyHint({ total: 0, view: "all" }).title, /这一段还是空的/);
  assert.match(emptyHint({ total: 3, view: "invalid", category: null }).title, /这一档里没有/);
  assert.match(emptyHint({ total: 3, view: "all", query: "abc" }).title, /搜「abc」/);
});

// ── 引用 / 叠放 ─────────────────────────────────────────────────────────────

test("splitRef：`段/名` 拆开；没有段名的不瞎猜", () => {
  assert.deepEqual(splitRef("Session-Template/deep-dive"), { section: "Session-Template", name: "deep-dive" });
  assert.deepEqual(splitRef("deep-dive"), { section: null, name: "deep-dive" });
  assert.deepEqual(splitRef("Session-Template/"), { section: null, name: "Session-Template/" });
});

test("stackOf：顺序就是叠的顺序（1 开始，缺的标出来）", () => {
  const stack = stackOf(ultimate({ presets: ["Session-Template/deep-dive", "Session-Template/gone"] }), KNOWN);
  assert.deepEqual(
    stack.map((item) => [item.order, item.name, item.state]),
    [
      [1, "deep-dive", "ok"],
      [2, "gone", "missing"],
    ],
  );
});

test("lucky：名字规则 + 字母砖颜色稳定", () => {
  assert.equal(nameLooksValid("full-stack"), true);
  assert.equal(nameLooksValid("Full Stack"), false);
  assert.equal(nameLooksValid(""), false);
  assert.equal(tileStyle("full-stack"), tileStyle("full-stack"));
  assert.notEqual(tileStyle("a"), tileStyle("b"));
});

test("启动：这一段还没接，按钮必须灰 —— 而且说得出为什么", () => {
  assert.equal(canLaunch(), false);
  assert.match(launchBlockedReason(), /形状定下来/);
});

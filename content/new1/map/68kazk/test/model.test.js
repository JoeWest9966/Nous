// 面板的纯计算部分：卡片状态、facet 计数、排序、删除类别的影响范围、未分类桶。
//
// 不碰 DOM、不 fetch、不看时间 —— 所以能裸跑 node --test。DOM 那半截在 app.js。
// 判据来自 spec §7：卡片状态只取第一个命中的（failed > unpublished > ok），
// 归档不在网格里、沉到最后；facet 计数对**全量**算，不是对当前筛选算。

import test from "node:test";
import assert from "node:assert/strict";

import {
  ARCHIVED,
  UNCATEGORIZED,
  buildView,
  cardState,
  categoriesOf,
  categoryImpact,
  childCount,
  facetCounts,
  filterCards,
  sortUpstream,
  stateFilterOf,
  suggestedAdoptName,
  upstreamAt,
  upstreamCrumb,
  upstreamSummary,
  upstreamTree,
} from "../public/model.js";

// fixture 就在这个文件里（以前读 public/mock/skills.json，那个目录已经删了 ——
// spec I2：前端连副本都不许有）。形状抄 `GET /v1/skills`（§6.2）：正常 ×3、未发布 ×1、
// 装载失败 ×1、未分类 ×1、归档 ×1。
const FIXTURE = {
  root: "/opt/Nous-skills",
  repo: { head: "4f1c2ab", dirty: true, ahead: 1, behind: 0, remote: "https://github.com/JoeWest9966/Nous-skills", error: null },
  skills: [
    { name: "code-review", description: "审查从某个固定点以来的改动。", category: ["Basics"], load: { state: "ok" }, tracked: true },
    { name: "write-tests", description: "先写失败的那个测试。", category: ["工具"], load: { state: "ok" }, tracked: true },
    { name: "search-docs", description: "按关键词翻文档。", category: ["工具/搜索"], load: { state: "ok" }, tracked: true },
    { name: "draft-mail", description: "起草一封邮件。", category: ["工具"], load: { state: "ok" }, tracked: false },
    {
      name: "broken-notes",
      description: "故意写坏的：用来看红卡。",
      category: ["工具"],
      load: { state: "failed", error: "broken-notes: SKILL.md 的 name 跟目录名对不上" },
      tracked: false,
    },
    { name: "notes-to-self", description: "没挂类别的一条。", category: [], hidden: true, load: { state: "ok" }, tracked: true },
  ],
  archived: ["legacy-helper"],
  categories: ["Basics", "工具", "工具/搜索", "工具/爬虫"],
};
const fixture = async () => JSON.parse(JSON.stringify(FIXTURE));
const names = (list) => list.map((item) => item.name);

test("七条 fixture：卡片状态照 spec §7.1，归档的不在网格里", async () => {
  const data = await fixture();
  const view = buildView(data);

  assert.equal(view.total, 7);
  assert.equal(view.skillCount, 6);
  // 网格顺序 = 正常 → 未发布 → 装载失败，同类按 name 升序。
  assert.deepEqual(names(view.cards), [
    "code-review",
    "notes-to-self",
    "search-docs",
    "write-tests",
    "draft-mail",
    "broken-notes",
  ]);
  assert.deepEqual(
    view.cards.map((card) => card.state),
    ["ok", "ok", "ok", "ok", "unpublished", "failed"],
  );
  // 归档项不在网格里，只在 archived 里，带自己的状态。
  assert.deepEqual(view.archived, [{ name: "legacy-helper", state: "archived" }]);
  assert.ok(!view.cards.some((card) => card.name === "legacy-helper"));
});

test("failed 优先于 unpublished（一条既没发布又装载失败 → failed）", () => {
  assert.equal(cardState({ name: "x", tracked: false, load: { state: "failed", error: "坏了" } }), "failed");
  assert.equal(cardState({ name: "x", tracked: true, load: { state: "failed", error: "坏了" } }), "failed");
  assert.equal(cardState({ name: "x", tracked: false, load: { state: "ok" } }), "unpublished");
  assert.equal(cardState({ name: "x", tracked: true, load: { state: "ok" } }), "ok");
  // 没有 load 字段（老响应 / 读不到）就当正常，别把整页标红。
  assert.equal(cardState({ name: "x", tracked: true }), "ok");
});

test("facet 计数对全量算：筛出一半，计数还是全量的", async () => {
  const data = await fixture();
  const view = buildView(data);

  const onlyBasics = filterCards(view.cards, { filter: "Basics" });
  assert.deepEqual(names(onlyBasics), ["code-review"]);
  // 筛出来只剩 1 条，但「工具」那格的计数还是全量的 4 —— facet 是导航，不是统计。
  assert.equal(view.counts.get("工具"), 4);
  assert.equal(view.counts.get("Basics"), 1);
  // 同一份数据再算一遍，结果一样（计数不读筛选态）。
  assert.deepEqual([...buildView(data).counts], [...view.counts]);
  // 计数是「条数」不是「标签数」：搜索改的是结果集，不动计数。
  assert.equal(filterCards(view.cards, { query: "工具" }).length, 4);
  assert.equal(view.counts.get("工具/搜索"), 1);
});

test("类别路径：`工具/搜索` 挂在 `工具` 下，父级计数含子树；词表里的空类别仍然出现", async () => {
  const data = await fixture();
  const view = buildView(data);

  const search = view.rows.find((row) => row.path === "工具/搜索");
  assert.equal(search.label, "搜索");
  assert.equal(search.depth, 1);
  assert.equal(view.rows.find((row) => row.path === "工具").parent, true);

  assert.equal(view.counts.get("工具/搜索"), 1);
  assert.equal(view.counts.get("工具"), 4); // 自身 3 条 + 子树 1 条
  // 词表里有、还没挂到任何技能上的类别：仍然出现，计数 0（建了不能看不见）。
  assert.equal(view.counts.get("工具/爬虫"), 0);
  assert.ok(view.categoryNames.includes("工具/爬虫"));

  // 同时挂父和子只算一次。
  const deduped = facetCounts([
    { name: "a", category: ["工具", "工具/搜索"] },
    { name: "b", category: ["工具/搜索"] },
  ]);
  assert.equal(deduped.get("工具"), 2);
  assert.equal(deduped.get("工具/搜索"), 2);

  // 祖先不写进技能的 category，但要在树里出现。
  const leafonly = facetCounts([{ name: "c", category: ["工具/搜索"] }]);
  assert.equal(leafonly.get("工具"), 1);
  assert.deepEqual(categoriesOf({ category: [" 工具 // 搜索 "] }), ["工具/搜索"]);
});

test("排序：正常 → 未发布 → 装载失败，同类按 name 升序；归档沉到最后", async () => {
  const data = await fixture();
  const view = buildView(data);
  const ranks = { ok: 0, unpublished: 1, failed: 2 };
  const states = view.cards.map((card) => card.state);
  assert.deepEqual(states, [...states].sort((a, b) => ranks[a] - ranks[b]));
  for (const state of ["ok", "unpublished", "failed"]) {
    const group = names(view.cards.filter((card) => card.state === state));
    assert.deepEqual(group, [...group].sort((a, b) => a.localeCompare(b)));
  }
  // 归档自成一段，按 name 升序。
  const archived = buildView({ ...data, archived: ["zeta", "alpha"] }).archived;
  assert.deepEqual(names(archived), ["alpha", "zeta"]);
  assert.deepEqual(archived.map((item) => item.state), ["archived", "archived"]);
});

test("删除类别的影响范围 = 整枝命中的技能数", async () => {
  const data = await fixture();
  // 删「工具」连「工具/搜索」一起 —— 影响的是整枝，不是只挂「工具」那三条。
  assert.equal(categoryImpact(data.skills, "工具"), 4);
  assert.equal(categoryImpact(data.skills, "工具/搜索"), 1);
  assert.equal(categoryImpact(data.skills, "Basics"), 1);
  assert.equal(categoryImpact(data.skills, "工具/爬虫"), 0);
  // 子级数量也照整枝数（确认框里那句「连同 N 个子类别」）。
  const view = buildView(data);
  assert.equal(childCount(view.categoryNames, "工具"), 2);
});

test("未分类（category: []）落在「未分类」桶里，而不是消失", async () => {
  const data = await fixture();
  const view = buildView(data);

  assert.equal(view.uncategorized, 1);
  const only = filterCards(view.cards, { filter: UNCATEGORIZED });
  assert.deepEqual(names(only), ["notes-to-self"]);
  // 「未分类」是算出来的，不进类别树、也不进词表。
  assert.ok(!view.categoryNames.includes("未分类"));
  assert.ok(!view.categoryNames.includes(UNCATEGORIZED));
  // 归档筛选值也不是类别。
  assert.ok(!view.categoryNames.includes(ARCHIVED));
  assert.deepEqual(
    only.map((card) => categoriesOf(card)),
    [[]],
  );
  // 状态筛选走自己的前缀，不会跟叫「未发布」的类别撞车。
  assert.equal(stateFilterOf("__state__:failed"), "failed");
  assert.equal(stateFilterOf("工具"), null);
  assert.deepEqual(names(filterCards(view.cards, { filter: "__state__:unpublished" })), ["draft-mail"]);
});

// ── 上游 ────────────────────────────────────────────────────────────────────
// 四种关系（未取用 / 上游有更新 / 名字被占 / 已取用）是服务端算的，这里只测「怎么摆」：
// 摘要把 `same` 排除在待办外，排序把要动手的放前面。

test("upstreamSummary：same 不算待办", () => {
  const entries = [{ local: "none" }, { local: "none" }, { local: "changed" }, { local: "taken" }, { local: "same" }];
  assert.deepEqual(upstreamSummary(entries), { none: 2, changed: 1, taken: 1, same: 1, todo: 4, total: 5 });
  assert.deepEqual(upstreamSummary([]), { none: 0, changed: 0, taken: 0, same: 0, todo: 0, total: 0 });
});

test("upstreamSummary：不认识的 local 当成未取用，不静默丢掉", () => {
  assert.equal(upstreamSummary([{ local: undefined }]).none, 1);
  assert.equal(upstreamSummary([{ local: "weird" }]).total, 1);
});

test("sortUpstream：未取用 → 有更新 → 名字被占 → 已取用；同档按路径", () => {
  const sorted = sortUpstream([
    { path: "b/same", local: "same" },
    { path: "c/taken", local: "taken" },
    { path: "a/none", local: "none" },
    { path: "a/none2", local: "none" },
    { path: "b/changed", local: "changed" },
  ]).map((entry) => entry.path);
  assert.deepEqual(sorted, ["a/none", "a/none2", "b/changed", "c/taken", "b/same"]);
});

test("suggestedAdoptName：名字被占时退一格", () => {
  assert.equal(suggestedAdoptName({ name: "code-review", local: "none" }), "code-review");
  assert.equal(suggestedAdoptName({ name: "code-review", local: "taken" }), "code-review-upstream");
  assert.equal(suggestedAdoptName({ name: "", local: "none" }), "");
});

// ── 上游的树 ────────────────────────────────────────────────────────────────

const up = (path, local = "none") => ({ source: "matt", path, name: path.split("/").at(-1), local });

test("upstreamTree：平铺清单退化成一层", () => {
  const tree = upstreamTree([up("code-review"), up("weekly-report")]);
  assert.equal(tree.n, 2);
  assert.deepEqual(
    tree.children.map((node) => [node.kind, node.name]),
    [["skill", "code-review"], ["skill", "weekly-report"]],
  );
});

test("upstreamTree：多层路径自己长出目录，目录在前技能在后", () => {
  const tree = upstreamTree([up("skills/b"), up("plugins/x/skills/c"), up("aaa")]);
  assert.equal(tree.n, 3);
  assert.deepEqual(
    tree.children.map((node) => node.name),
    ["plugins", "skills", "aaa"],
  );
  const plugins = tree.children[0];
  assert.equal(plugins.kind, "dir");
  assert.equal(plugins.n, 1);
  assert.equal(plugins.children[0].path, "plugins/x");
  assert.equal(plugins.children[0].children[0].path, "plugins/x/skills");
  assert.equal(plugins.children[0].children[0].children[0].kind, "skill");
});

test("upstreamTree：目录上那个数是「这一层往下的技能条数」，todo 只数要动手的", () => {
  const tree = upstreamTree([up("a/x", "none"), up("a/y", "same"), up("a/z", "changed")]);
  const a = tree.children[0];
  assert.equal(a.n, 3);
  assert.equal(a.todo, 2);
  assert.equal(tree.n, 3);
  assert.equal(tree.todo, 2);
});

test("upstreamTree：一个目录自己就是技能、里面还有子目录，不丢那一条", () => {
  const tree = upstreamTree([up("x"), up("x/y")]);
  const x = tree.children[0];
  assert.equal(x.kind, "dir");
  assert.equal(x.entry.path, "x");
  assert.equal(x.n, 2);
  assert.equal(x.children[0].path, "x/y");
});

test("upstreamAt：钻到任意一层；路径脏了就退回根，不炸", () => {
  const tree = upstreamTree([up("a/b/c"), up("a/d")]);
  assert.equal(upstreamAt(tree, "a").path, "a");
  assert.equal(upstreamAt(tree, "a/b/c").name, "c");
  assert.equal(upstreamAt(tree, "").path, "");
  assert.equal(upstreamAt(tree, "不存在/xxx").path, "");
  assert.equal(upstreamAt(tree, "a/nope").path, "a");
});

test("upstreamCrumb：面包屑每一层都能点回去", () => {
  assert.deepEqual(upstreamCrumb("a/b/c"), [
    { name: "a", path: "a" },
    { name: "b", path: "a/b" },
    { name: "c", path: "a/b/c" },
  ]);
  assert.deepEqual(upstreamCrumb(""), []);
});

// ── 正文抽屉的文件树 ────────────────────────────────────────────────────────
//
// 列表是扁的（`docs/a.md`），抽屉里要长成树。文件夹不是存出来的：有文件住在这儿它才在。

import { dirPaths, fileTree, flattenTree } from "../public/model.js";

test("fileTree：/ 拆成层级，同层目录在前、文件在后", () => {
  const tree = fileTree([
    { path: "SKILL.md", size: 10 },
    { path: "nous.json", size: 2 },
    { path: "docs/guide.md", size: 5 },
    { path: "docs/deep/a.md", size: 1 },
  ]);
  assert.deepEqual(
    tree.children.map((node) => [node.type, node.path, node.depth]),
    [
      ["dir", "docs", 0],
      ["file", "nous.json", 0],
      ["file", "SKILL.md", 0],
    ],
  );
  const docs = tree.children[0];
  assert.deepEqual(
    docs.children.map((node) => [node.type, node.path]),
    [
      ["dir", "docs/deep"],
      ["file", "docs/guide.md"],
    ],
  );
});

test("fileTree：没有文件的目录不会凭空出现", () => {
  const tree = fileTree([{ path: "a/b/c.md" }]);
  assert.deepEqual(
    tree.children.map((node) => node.path),
    ["a"],
  );
  assert.deepEqual(
    tree.children[0].children.map((node) => node.path),
    ["a/b"],
  );
});

test("fileTree：隔层也能对上 —— 先深后浅的文件不会建出两个 docs", () => {
  const tree = fileTree([{ path: "docs/deep/a.md" }, { path: "docs/b.md" }]);
  assert.equal(tree.children.length, 1);
  assert.deepEqual(
    tree.children[0].children.map((node) => node.path),
    ["docs/deep", "docs/b.md"],
  );
});

test("flattenTree：折叠的目录不展开它下面那几层", () => {
  const tree = fileTree([{ path: "docs/a.md" }, { path: "docs/b.md" }, { path: "top.md" }]);
  assert.deepEqual(
    flattenTree(tree).map((node) => node.path),
    ["docs", "docs/a.md", "docs/b.md", "top.md"],
  );
  assert.deepEqual(
    flattenTree(tree, () => false).map((node) => node.path),
    ["docs", "top.md"],
  );
});

test("dirPaths：所有文件夹（打开抽屉时默认全展开）", () => {
  const tree = fileTree([{ path: "a/b/c.md" }, { path: "d.md" }]);
  assert.deepEqual(dirPaths(tree), ["a", "a/b"]);
});

// ── 上游并进同一张网格 ──────────────────────────────────────────────────────
//
// 上游的条目现在也住「全部」和「未分类」里，所以 buildView 收两份数据。
// 认身份靠 `kind`，不靠名字：上游撞上来我们管不了（见 spec §12）。

const ENTRIES = [
  {
    source: "matt",
    path: "skills/engineering/code-review",
    name: "code-review",
    description: "别人家的审查",
    category: [],
    sha: "a".repeat(40),
    local: "none",
  },
  {
    source: "matt",
    path: "skills/engineering/zip",
    name: "zip",
    description: "别人的打包",
    category: ["Basics"],
    sha: "b".repeat(40),
    local: "same",
  },
];

test("buildView：上游并进网格，沉在本地后面，靠 kind 认身份", () => {
  const view = buildView(FIXTURE, ENTRIES);
  assert.equal(view.skillCount, view.localCount + view.upstreamCount);
  assert.equal(view.upstreamCount, 2);
  const kinds = view.cards.map((card) => card.kind ?? "local");
  // 本地的都在前面（上游 rank 3），但每一段内部照旧按名字排。
  assert.deepEqual(kinds.slice(-2), ["upstream", "upstream"]);
  const up = view.cards.filter((card) => card.kind === "upstream");
  assert.deepEqual(up.map((card) => card.name), ["code-review", "zip"]);
  assert.equal(up[0].state, "upstream");
});

test("buildView：本地和上游同名 —— 两条都在，本地在前", () => {
  const view = buildView(FIXTURE, ENTRIES);
  const same = view.cards.filter((card) => card.name === "code-review");
  assert.equal(same.length, 2);
  assert.equal(same[0].kind ?? "local", "local");
  assert.equal(same[1].kind, "upstream");
  // 倒过来喂也一个样：顺序不能跟着数据来
  const reversed = buildView(FIXTURE, [...ENTRIES].reverse());
  assert.deepEqual(
    reversed.cards.filter((card) => card.name === "code-review").map((card) => card.kind ?? "local"),
    ["local", "upstream"],
  );
});

test("buildView：未分类和类别计数都把上游算进去", () => {
  const view = buildView(FIXTURE, ENTRIES);
  // 上游那条 code-review 没类别 → 未分类 +1
  assert.equal(view.uncategorized, buildView(FIXTURE).uncategorized + 1);
  // 「Basics」底下多了一条上游的
  assert.equal(view.counts.get("Basics"), buildView(FIXTURE).counts.get("Basics") + 1);
  // 类别词表认得上游那条挂的类别（哪怕 categories.json 里没登记）
  assert.ok(view.categoryNames.includes("Basics"));
});

test("buildView：上游的类别也进筛选 —— 按类别筛能筛到它", () => {
  const view = buildView(FIXTURE, ENTRIES);
  const shown = filterCards(view.cards, { filter: "Basics" });
  assert.ok(shown.some((card) => card.kind === "upstream" && card.name === "zip"));
  const none = filterCards(view.cards, { filter: UNCATEGORIZED });
  assert.ok(none.some((card) => card.kind === "upstream" && card.name === "code-review"));
});

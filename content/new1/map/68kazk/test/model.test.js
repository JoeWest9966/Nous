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
  dirPaths,
  facetCounts,
  fileTree,
  filterCards,
  flattenTree,
  stateFilterOf,
  suggestedAdoptName,
  upstreamSummary,
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

test("suggestedAdoptName：名字被占时退一格", () => {
  assert.equal(suggestedAdoptName({ name: "code-review", local: "none" }), "code-review");
  assert.equal(suggestedAdoptName({ name: "code-review", local: "taken" }), "code-review-upstream");
  assert.equal(suggestedAdoptName({ name: "", local: "none" }), "");
});

// ── 上游的树 ────────────────────────────────────────────────────────────────

const up = (path, local = "none") => ({ source: "matt", path, name: path.split("/").at(-1), local });

// 上游那一层不再有「清单折成树」这件事：一个订阅 = 一条技能（整个仓），
// 目录只活在抽屉里那棵用 `fileTree` 算出来的树里。`upstreamTree` 跟着删了。

// 目录是**筛子**不是页面：上游那一屏的地址只到订阅（`#upstream/matt`），
// 所以「钻到某一层」「面包屑」这两个纯函数跟着一起去掉了 —— 那套是文件管理器的形状。

// ── 正文抽屉的文件树 ────────────────────────────────────────────────────────
//
// 列表是扁的（`docs/a.md`），抽屉里要长成树。文件夹不是存出来的：有文件住在这儿它才在。

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


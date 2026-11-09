// 开发期自检：把 API 里每一张图都真的用 pele 渲染一遍。
//
// 为什么值得有这个脚本：Mermaid 的语法错误在浏览器里只表现成「一张图不见了」，
// 而且往往是在最难看的那个空位上。在 node 里跑一遍，错在哪儿、第几行都能看见。
//
//   node dev/check.mjs [baseUrl] [dbId]
//
// 需要块已经在跑（npm start）。

import { readFileSync } from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE = process.argv[2] ?? "http://127.0.0.1:4782";
const DB = process.argv[3] ?? "";

const sandbox = { console, window: {}, document: undefined };
vm.createContext(sandbox);
vm.runInContext(readFileSync(path.join(HERE, "..", "public", "vendor", "pele.global.js"), "utf8"), sandbox);
const pele = sandbox.pele;

const get = async (route) => {
  const url = `${BASE}${route}${DB ? `${route.includes("?") ? "&" : "?"}db=${DB}` : ""}`;
  const response = await fetch(url);
  const body = await response.json();
  if (!response.ok) throw new Error(`${route} → ${response.status} ${JSON.stringify(body).slice(0, 200)}`);
  return body;
};

let failed = 0;
const report = (where, source) => {
  if (!source) return console.log(`  · ${where.padEnd(18)} (空)`);
  try {
    const started = performance.now();
    const result = pele.render(source);
    const ms = Math.round(performance.now() - started);
    console.log(
      `  ✓ ${where.padEnd(18)} ${String(Math.round(result.width)).padStart(4)}×${String(
        Math.round(result.height),
      ).padStart(4)}  ${String(ms).padStart(3)}ms  ${source.split("\n").length} 行`,
    );
  } catch (error) {
    failed += 1;
    console.log(`  ✗ ${where}  ${error.code ?? ""} ${error.message}`);
    if (error.snippet) console.log(`\n${error.snippet}\n`);
    console.log(source.split("\n").slice(0, 24).map((line, i) => `    ${String(i + 1).padStart(2)}| ${line}`).join("\n"));
  }
};

const targets = await get("/api/targets");
console.log(`库：${targets.targets.map((t) => `${t.id}${t.own ? "(自己)" : ""} ${t.entries}e`).join(" · ")}`);
const id = DB || targets.default;

console.log("\n① 体检");
const overview = await get(`/api/overview?db=${id}`);
console.log(`  ${overview.tables.length} 张表 · ${overview.file.db} B · schema v${overview.pragmas.schemaVersion}`);
report("treemap", overview.diagrams.treemap);
report("treemapKinds", overview.diagrams.treemapKinds);
report("er", overview.diagrams.er);
if (overview.attention.length) console.log(`  ! ${overview.attention.join("\n  ! ")}`);

console.log("\n② 会话树");
const conversations = await get(`/api/conversations?db=${id}`);
console.log(`  ${conversations.count} 条会话 · ${conversations.totals.tokens} tokens`);
report("gitGraph", conversations.diagrams.git);
report("timeline", conversations.diagrams.timeline);

const first = conversations.list[0];
if (first) {
  console.log(`\n③ 回放 #${first.id}`);
  const detail = await get(`/api/conversation?db=${id}&id=${first.id}`);
  console.log(`  ${detail.entries.length} 条 entry · ${detail.tasks.length} 个任务 · ${detail.documents.length} 份文档`);
  console.log(`  prompt: ${detail.prompt.totals.messages} 条消息 / ${detail.prompt.totals.chars} 字符`);
  report("context", detail.diagrams.context);
  report("sequence", detail.diagrams.sequence);
}

console.log("\n④ 账单");
const cost = await get(`/api/cost?db=${id}`);
console.log(`  ${cost.totals.tokens} tokens · $${cost.totals.cost.toFixed(4)} · ${cost.byModel.length} 个模型 · ${cost.byDay.length} 天`);
report("xyTokens", cost.diagrams.xyTokens);
report("pieModels", cost.diagrams.pieModels);
report("sankey", cost.diagrams.sankey);

console.log("\n⑤ 健康度");
const health = await get(`/api/health?db=${id}`);
console.log(
  `  任务 ${health.tasks.total} · 卡住 ${health.tasks.stuck.length} · 未答 ${health.submissions.unanswered.length} · delta ${
    health.documents.delta
  }/${health.documents.revisions}`,
);
report("kanban", health.diagrams.kanban);
report("quadrant", health.diagrams.quadrant);
report("pieKinds", health.diagrams.pieKinds);
report("pieSubmissions", health.diagrams.pieSubmissions);

console.log(failed ? `\n✗ ${failed} 张图没画出来` : "\n✓ 全部图画得出来");
process.exit(failed ? 1 : 0);

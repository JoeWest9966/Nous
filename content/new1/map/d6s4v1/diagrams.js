// d6s4v1 · 图表层：数据进，Mermaid 源码出。
//
// 这里一行 sqlite 都没有 —— 它只认识普通对象，所以能在 node 里被 pele 直接渲染一遍验语法。
// 验证脚本：dev/check.mjs。**每个函数要么返回合法源码，要么返回 null**（数据不够就别画，
// 别画一张空图还硬说这是「没有数据」）。

/** 图表标签里最危险的是这几个字符：换行会截断语句，逗号会切碎 sankey 的 CSV。 */
const clean = (value, max = 42) =>
  String(value ?? "")
    .replace(/[\r\n\t]+/g, " ")
    .replace(/["'`]/g, "")
    .replace(/[[\]{}]/g, "·")
    .replace(/[<>]/g, "-")
    .trim()
    .slice(0, max) || "·";

/** sankey / er 这类拿逗号当分隔符的地方，逗号本身也不许出现。 */
const csv = (value, max = 30) => clean(value, max).replace(/,/g, "/");

export const human = (bytes) => {
  const n = Number(bytes) || 0;
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(n < 10_240 ? 1 : 0)} KB`;
  return `${(n / 1024 ** 2).toFixed(1)} MB`;
};

// ── ① 解剖：字节都长在哪 ────────────────────────────────────────────────────
// dbstat 给的是真页数，不是估算。层级 = 文件 → 表 → entries 里的 kind，
// 因为「entries 吃掉 55% 的盘」这句话，不说清是 assistant 还是 system，等于没说。

export function treemap(bytes, byKind, indexes = []) {
  const total = bytes?.db ?? 0;
  if (total <= 0) return null;
  // 这张图是「按页」的：dbstat 数的是 4KB 的页，不是 JSON 的字符数。
  // 所以不要把 entries 拆成 kind 塞进来 —— kind 是按 record 长度算的（167KB），
  // 页是 220KB，两套尺子混在一张图里，百分比就全是假的。kind 的分布看下面那张表。
  const kb = (value) => Math.max(1, Math.round(value / 1024));
  const tables = [...(bytes.tables ?? [])].sort((a, b) => b.bytes - a.bytes);
  const tableSum = tables.reduce((sum, row) => sum + row.bytes, 0);
  const indexSum = indexes.reduce((sum, row) => sum + row.bytes, 0);
  const lines = ["treemap-beta", `    "agent.sqlite · ${human(total)}"`];

  lines.push(`        "表 · ${human(tableSum)}"`);
  const head = tables.slice(0, 10);
  for (const table of head) {
    if (table.bytes < 1024) continue;
    lines.push(`            "${clean(table.name, 20)}": ${kb(table.bytes)}`);
  }
  const rest = tables.slice(10).reduce((sum, row) => sum + row.bytes, 0);
  if (rest > 0) lines.push(`            "其他 ${tables.length - 10} 张": ${kb(rest)}`);

  // 索引是那张表之外的第二块盘：404KB 的库里 68KB 花在这上面，平时没人查它。
  if (indexSum > 4096) {
    lines.push(`        "索引 · ${human(indexSum)}"`);
    for (const index of [...indexes].sort((a, b) => b.bytes - a.bytes).slice(0, 6)) {
      lines.push(`            "${clean(index.name, 24)}": ${kb(index.bytes)}`);
    }
    const indexRest = indexes.slice(6).reduce((sum, row) => sum + row.bytes, 0);
    if (indexRest > 0) lines.push(`            "其他索引": ${kb(indexRest)}`);
  }

  // 差值要说出来：页子加起来不等于文件大小，那几十 KB 不是「不见了」。
  // 藏起来的话，这张图的百分比全是假的。
  const slack = total - tableSum - indexSum;
  if (slack > 4096) {
    lines.push(`        "空闲 / 未归属 · ${human(slack)}"`);
    lines.push(`            "其中空闲页 ${human(bytes.free ?? 0)}": ${kb(slack)}`);
  }
  return lines.length > 2 ? lines.join("\n") : null;
}

/**
 * 同一份数据，换个尺子：entries 里 JSON 字节的分布（按 kind）。
 * 和上面那张「按页」的图放一起看，才知道「assistant 占 55%」到底是记录长还是页多。
 */
export function treemapKinds(byKind) {
  const rows = [...(byKind ?? [])].sort((a, b) => b.bytes - a.bytes);
  if (rows.length < 2) return null;
  const total = rows.reduce((sum, row) => sum + row.bytes, 0);
  const lines = ["treemap-beta", `    "entries · ${human(total)} JSON"`];
  for (const row of rows.slice(0, 6)) {
    lines.push(`        "${clean(row.kind.replace(/^pi\./, ""), 16)}": ${Math.max(1, Math.round(row.bytes / 1024))}`);
  }
  const rest = rows.slice(6).reduce((sum, row) => sum + row.bytes, 0);
  if (rest > 0) lines.push(`        "其他": ${Math.max(1, Math.round(rest / 1024))}`);
  return lines.join("\n");
}

// ── ② schema：pi-durable 自己那张 ER 图 ─────────────────────────────────────
// 这是「本块唯一认得的东西」的自画像：12 张表、6 条引用。
// 版本写在 durable_schema 里，页面把它和这张图对一下就知道认不认识这个库。

export function erSchema(version) {
  return `erDiagram
    conversations ||--o{ entries : "conversation_id"
    conversations ||--o{ tasks : "conversation_id"
    conversations ||--o{ submissions : "conversation_id"
    conversations ||--o{ conversations : "owner_conversation_id"
    tasks ||--o{ conversations : "owner_task_id"
    documents ||--o{ document_revisions : "document_id"

    conversations {
        int id PK
        int owner_conversation_id FK
        int owner_task_id FK
        json record
    }
    entries {
        int id PK
        int conversation_id FK
        int head "压缩点"
        int commit_seq
        json record
    }
    tasks {
        int id PK
        int conversation_id FK
        text kind
        text status
        int background
        json record
    }
    submissions {
        int id PK
        int conversation_id FK
        text request_id
        text status
        json record
    }
    documents {
        int id PK
        text kind
        int family
        text scope_kind
        int owner_id
        int retired_at
        json record
    }
    document_revisions {
        int document_id FK
        int seq PK
        text kind "base/delta"
        int version
        json content
    }
    durable_metadata {
        int next_id
        int next_seq
    }
    record_ids {
        int id PK
        text record_type
    }
    durable_schema {
        int version "schema v${Number(version) || 0}"
    }`;
}

// ── ③ 会话树：谁从谁身上分叉 ──────────────────────────────────────────────
// conversations.owner_conversation_id / owner_task_id 就是那棵树，
// 只是它一直躺在 JSON 旁边没人画过。gitGraph 是唯一画得出「分叉」的原生图。

export function gitTree(conversations) {
  if (!conversations?.length) return null;
  const children = new Map();
  for (const conv of conversations) {
    const parent = conv.ownerConv ?? 0;
    if (!children.has(parent)) children.set(parent, []);
    children.get(parent).push(conv);
  }
  const lines = ["gitGraph"];
  const walk = (parentId, depth) => {
    for (const conv of children.get(parentId) ?? []) {
      if (depth === 0) {
        lines.push(`    commit id:"#${conv.id} ${conv.entries}e"`);
      } else {
        lines.push(`    branch f${conv.id}`);
        lines.push(`    checkout f${conv.id}`);
        lines.push(`    commit id:"#${conv.id} ${conv.entries}e"`);
        lines.push(`    checkout main`);
      }
      walk(conv.id, depth + 1);
      if (depth > 0) lines.push(`    checkout main`);
    }
  };
  walk(0, 0);
  if (lines.length <= 1) return null;
  return lines.join("\n");
}

// ── ④ 时间线：一条会话的 entries ───────────────────────────────────────────

export function timeline(conversation, entries) {
  // 不画每一条 entry：工具回包和 system 提示会把时间轴堆成一堆微缩字。
  // 只画人和模型说的话（最近 12 句），那就是这条会话的故事。
  const all = (entries ?? []).filter(
    (e) => e.at && (e.role === "user" || (e.role === "assistant" && e.text)),
  );
  const rows = all.slice(-12);
  if (rows.length < 2) return null;
  const pad = (n) => String(n).padStart(2, "0");
  // 时间里的冒号会让 mermaid 的 timeline 解析器当场翻车（`06:23 : …`），
  // 所以这一张图里的时间写成 `06.23`。难看不难看是小事，画不出来才是大事。
  const clock = (ms) => {
    const d = new Date(ms);
    return `${pad(d.getHours())}.${pad(d.getMinutes())}.${pad(d.getSeconds())}`;
  };
  const sections = new Map();
  for (const entry of rows) {
    const hour = `${pad(new Date(entry.at).getHours())}h`;
    if (!sections.has(hour)) sections.set(hour, []);
    sections.get(hour).push(entry);
  }
  const lines = [
    "timeline",
    `    title "会话 #${conversation.id} · 最近 ${rows.length} / ${all.length} 条"`,
  ];
  for (const [hour, list] of sections) {
    lines.push(`    section ${hour}`);
    for (const entry of list) {
      const who = entry.role === "user" ? "我" : "它";
      const text = String(entry.text ?? "").replace(/\s+/g, " ").trim();
      lines.push(`        ${clock(entry.at).slice(3, 8)} : ${clean(`${who} ${text}`, 34)}`);
    }
  }
  return lines.join("\n");
}

// ── ④b 时间线（会话版）：什么时候跟它说过话 ───────────────────────────────
// mermaid 的 timeline 天生横着长：一条会话画满 12 句就是 2000px，字得缩到看不清。
// 所以「会话内的时间线」不画 —— 画「会话的时间线」：每条会话一个点，一眼看清哪几天在忙。
export function timelineConversations(list) {
  const rows = (list ?? []).filter((row) => row.lastAt);
  if (rows.length < 2) return null;
  const pad = (n) => String(n).padStart(2, "0");
  const day = (ms) => {
    const d = new Date(ms);
    return `${pad(d.getMonth() + 1)}.${pad(d.getDate())}`;
  };
  const time = (ms) => {
    const d = new Date(ms);
    return `${pad(d.getHours())}.${pad(d.getMinutes())}`;
  };
  const sections = new Map();
  for (const row of rows) {
    const key = day(row.lastAt);
    if (!sections.has(key)) sections.set(key, []);
    sections.get(key).push(row);
  }
  const lines = ["timeline", `    title ${rows.length} 条会话，按最后一次发言排`];
  for (const [key, items] of [...sections].sort((a, b) => a[0].localeCompare(b[0]))) {
    lines.push(`    section ${key}`);
    for (const row of items.sort((a, b) => a.lastAt - b.lastAt)) {
      lines.push(`        ${time(row.lastAt)} : ${clean(`#${row.id} ${row.title}`, 30)}`);
    }
  }
  return lines.join("\n");
}

// ── ⑤ 回放：这条会话说了什么 ───────────────────────────────────────────────
// 参与者不是写死的模特，是从记录里现拼的：用户、模型（可能换过）、被调的工具。

export function sequence(entries) {
  const all = (entries ?? []).filter((e) => e.role);
  // 一条会话能有 90 条发言，全画出来是 3000px 高。留最后 30 条，够看清「它到底在干什么」。
  const rows = all.slice(-30);
  if (rows.length < 2) return null;
  const models = [...new Set(rows.filter((e) => e.model).map((e) => e.model))];
  const tools = [...new Set(rows.filter((e) => e.tool).map((e) => e.tool))].slice(0, 5);
  const lines = ["sequenceDiagram", "    participant U as 用户"];
  models.forEach((model, index) => lines.push(`    participant M${index} as ${clean(model, 22)}`));
  tools.forEach((tool, index) => lines.push(`    participant T${index} as ${clean(tool, 14)}`));
  const modelId = (name) => `M${Math.max(0, models.indexOf(name))}`;
  const toolId = (name) => `T${Math.max(0, tools.indexOf(name))}`;
  const preview = (text) =>
    clean(String(text ?? "").replace(/\s+/g, " "), 60) || "(空)";
  for (const entry of rows) {
    const from = entry.role === "user" ? "U" : modelId(entry.model);
    if (entry.role === "user") {
      lines.push(`    U->>${modelId(entry.model ?? models[0] ?? "M0")}: ${preview(entry.text)}`);
    } else if (entry.role === "toolResult") {
      const ms = entry.ms != null ? ` · ${entry.ms}ms` : "";
      lines.push(`    ${toolId(entry.tool)}-->>${modelId(entry.model)}: ${entry.isError ? "失败" : "完成"}${ms}`);
    } else if (entry.toolCalls?.length) {
      for (const call of entry.toolCalls.slice(0, 4)) {
        lines.push(`    ${from}->>${toolId(call.tool)}: ${clean(call.tool, 14)}`);
      }
    } else {
      lines.push(`    ${from}-->>U: ${preview(entry.text)}`);
    }
  }
  return lines.join("\n");
}

// ── ⑥ 账单 ────────────────────────────────────────────────────────────────

export function xySeries(rows, key, unit, kind = "bar") {
  const data = (rows ?? []).filter((r) => (r[key] ?? 0) > 0);
  if (data.length < 2) return null;
  const max = Math.max(...data.map((r) => r[key]));
  const lines = [
    "xychart",
    `    x-axis [${data.map((r) => `"${clean(r.label, 12)}"`).join(", ")}]`,
    `    y-axis "${clean(unit, 20)}" 0 --> ${Math.ceil(Math.max(max, Number.EPSILON) * 1.15 * 1000) / 1000}`,
  ];
  if (kind === "line") lines.push(`    line [${data.map((r) => Number(r[key].toFixed(4))).join(", ")}]`);
  else lines.push(`    bar [${data.map((r) => Number(r[key].toFixed(4))).join(", ")}]`);
  return lines.join("\n");
}

export function pie(rows, key) {
  const data = (rows ?? []).filter((r) => (r[key] ?? 0) > 0);
  if (data.length < 1) return null;
  const lines = ["pie"];
  for (const row of data.slice(0, 8)) {
    lines.push(`    "${clean(row.label, 20)}" : ${Number(Number(row[key]).toFixed(4))}`);
  }
  return lines.length > 1 ? lines.join("\n") : null;
}

/** token 从哪来、到哪去：会话 → 模型 → (输入/输出/缓存)。 */
export function sankeyTokens(flows) {
  const rows = (flows ?? []).filter((f) => f.value > 0);
  if (rows.length < 3) return null;
  return ["sankey", ...rows.map((f) => `${csv(f.from)},${csv(f.to)},${Math.round(f.value)}`)].join("\n");
}

// ── ⑦ 健康度 ──────────────────────────────────────────────────────────────

export function kanban(tasks) {
  const columns = (tasks?.byStatus ?? []).filter((c) => c.count > 0);
  if (!columns.length) return null;
  const lines = ["kanban"];
  for (const column of columns) {
    const key = `s${column.status}`;
    lines.push(`    ${key}[${clean(column.status, 12)} · ${column.count}]`);
    for (const task of (column.sample ?? []).slice(0, 4)) {
      lines.push(`        ${key}t${task.id}[#${task.id} ${clean(task.kind.replace(/^pi\./, ""), 12)}]`);
    }
  }
  return lines.length > 1 ? lines.join("\n") : null;
}

export function gantt(tasks) {
  const rows = (tasks ?? []).filter((t) => t.startedAt && t.endedAt && t.endedAt >= t.startedAt);
  if (rows.length < 3) return null;
  const lines = [
    "gantt",
    `    title "任务耗时（${rows.length} 条，止于最近一次提交）"`,
    "    dateFormat X",
    "    axisFormat %H:%M:%S",
  ];
  const sections = new Map();
  for (const task of rows.slice(-40)) {
    const kind = task.kind.replace(/^pi\./, "");
    if (!sections.has(kind)) sections.set(kind, []);
    sections.get(kind).push(task);
  }
  for (const [kind, list] of sections) {
    lines.push(`    section ${clean(kind, 16)}`);
    for (const task of list) {
      lines.push(
        `        #${task.id} : ${Math.floor(task.startedAt / 1000)}, ${Math.max(
          Math.floor(task.endedAt / 1000),
          Math.floor(task.startedAt / 1000) + 1,
        )}`,
      );
    }
  }
  return lines.join("\n");
}

/**
 * 卡住 vs 跑得快：横轴 = 什么时候开的（0 早 → 1 晚），纵轴 = 跑了多久（0 快 → 1 慢）。
 * 这张图专治「谁卡住了」：右上角那个点就是又晚又慢的那次调用。
 * gantt 换成了这个，因为 pele 的 gantt 会把每根条都拉满（start 被吃掉），
 * 一张所有条一样长的图等于没画。
 */
export function quadrantTasks(tasks) {
  const rows = (tasks ?? [])
    .filter((t) => t.startedAt && t.endedAt && t.endedAt >= t.startedAt)
    .sort((a, b) => b.endedAt - b.startedAt - (a.endedAt - a.startedAt))
    .slice(0, 14);
  if (rows.length < 3) return null;
  const starts = rows.map((t) => t.startedAt);
  const spans = rows.map((t) => t.endedAt - t.startedAt);
  const minStart = Math.min(...starts);
  const spanStart = Math.max(1, Math.max(...starts) - minStart);
  const maxSpan = Math.max(1, Math.max(...spans));
  const lines = [
    "quadrantChart",
    "    x-axis 早 --> 晚",
    "    y-axis 快 --> 慢",
    "    quadrant-1 又晚又慢 · 先看这里",
    "    quadrant-2 早但慢",
    "    quadrant-3 早且快",
    "    quadrant-4 晚但快",
  ];
  // pele 的 quadrant 词法分析器见了 `1.0` 就报错（0..1 含右端点会当场翻车），
  // 所以坐标一律压到 0.999 以下。这不是美学问题，是画不画得出来的问题。
  const clamp01 = (value) => Math.min(0.999, Math.max(0, value)).toFixed(3);
  for (const task of rows) {
    const x = clamp01((task.startedAt - minStart) / spanStart);
    const y = clamp01((task.endedAt - task.startedAt) / maxSpan);
    lines.push(`    #${task.id}: [${x}, ${y}]`);
  }
  return lines.join("\n");
}

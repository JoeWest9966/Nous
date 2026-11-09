// d6s4v1 · storage / pi-durable 的 X 光机
//
// 一块的 `data/agent.sqlite` 里躺着它的全部真相：会话、每一条发言、每次工具调用、
// 每个任务的起止、每一分钱。这些现在全靠「打开另一个块的页面去猜」。这一页把它们摊开。
//
// 宿主给的四个环境变量：PORT / NOUS_DATA / NOUS_ORIGIN / NOUS_SELF。这里只用前两个。
//
//   GET /healthz
//   GET /api/targets               能看哪几个库（自己的排第一）
//   GET /api/overview?db=          文件、pragma、每张表的字节与行数、entries 的字节分布
//   GET /api/conversations?db=     会话树（owner_conversation_id 就是那棵树）
//   GET /api/conversation?db=&id=  一条会话的 entries + 「此刻模型看到了什么」
//   GET /api/cost?db=              tokens / 钱：按天、按模型、按会话
//   GET /api/health?db=            卡住的任务、没回答的输入、base/delta、WAL、维修工单
//   GET /…                         public/ 里的静态文件
//
// 三条硬规矩（写在这页的骨头里，不是为了好看）：
//
//   1. **只读。** 一律 `readOnly: true`。这页要是能手滑写库，它就是第二个写者，
//      而 pi-durable 的 WAL 只有一个写者。
//   2. **不建文件。** 不落快照、不建索引、不 VACUUM。想修？命令给你复制，你自己去跑。
//   3. **认出邻居，但不假装没这回事。** 默认只看自己的 `data/agent.sqlite`。
//      隔壁块的库是「旁路读盘」：能读、明确标注、可关（XRAY_SIBLINGS=0），
//      因为铁律说块之间走 HTTP —— 而 HTTP 那头还没有 `/__nous/ledger` 这个端点。

import { createServer } from "node:http";
import { statSync } from "node:fs";
import { readdir, readFile, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { DatabaseSync } from "node:sqlite";

import * as D from "./diagrams.js";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC = path.join(HERE, "public");
const DATA = process.env.NOUS_DATA ?? path.join(HERE, "data");
/** 块的 data/ 往上两层是 map/，再上一层是 board/。 */
const MAP = path.dirname(path.dirname(DATA));
const SELF_ID = path.basename(path.dirname(DATA));
const OWN_DB = path.join(DATA, "agent.sqlite");
const PORT = Number(process.env.PORT ?? 4782);
/** 旁路读邻居的库：默认开，但一定标出来。关掉它是 XRAY_SIBLINGS=0。 */
const SIBLINGS = process.env.XRAY_SIBLINGS !== "0";
/** 卡多久算「卡住」：10 分钟。比一次长的模型调用还久，比一顿饭短。 */
const STUCK_MS = Number(process.env.XRAY_STUCK_MS ?? 10 * 60 * 1000);
const STARTED = Date.now();

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".txt": "text/plain; charset=utf-8",
};

const num = (value) => (typeof value === "number" && Number.isFinite(value) ? value : 0);
const clamp = (value, max) => String(value ?? "").slice(0, max);
const now = () => Date.now();

// ── 找库 ───────────────────────────────────────────────────────────────────────
// 一个候选必须真的长着 `durable_metadata` 这张表才算数：光看文件名会让一页
// 「不是 pi-durable 的 sqlite」也被当成病人，然后在第一屏炸掉。

const looksDurable = (file) => {
  try {
    const db = new DatabaseSync(file, { readOnly: true });
    try {
      const row = db
        .prepare("select count(*) n from sqlite_master where type='table' and name='durable_metadata'")
        .get();
      return row.n === 1;
    } finally {
      db.close();
    }
  } catch {
    return false;
  }
};

const quickStats = async (file) => {
  try {
    const db = new DatabaseSync(file, { readOnly: true });
    try {
      const entries = num(db.prepare("select count(*) n from entries").get()?.n);
      const conversations = num(db.prepare("select count(*) n from conversations").get()?.n);
      return { entries, conversations, ok: true };
    } finally {
      db.close();
    }
  } catch (error) {
    return { entries: 0, conversations: 0, ok: false, why: String(error?.message ?? error) };
  }
};

let targetCache = { at: 0, list: [] };

async function discover(force = false) {
  if (!force && now() - targetCache.at < 5000) return targetCache.list;
  const candidates = [{ id: SELF_ID, file: OWN_DB, own: true }];
  if (SIBLINGS) {
    let dirs = [];
    try {
      dirs = await readdir(MAP, { withFileTypes: true });
    } catch {
      dirs = [];
    }
    for (const dir of dirs) {
      if (!dir.isDirectory() || dir.name === SELF_ID || dir.name.startsWith(".")) continue;
      candidates.push({ id: dir.name, file: path.join(MAP, dir.name, "data", "agent.sqlite"), own: false });
    }
  }
  const list = [];
  for (const candidate of candidates) {
    let info;
    try {
      info = await stat(candidate.file);
    } catch {
      continue;
    }
    if (!info.isFile() || info.size === 0) continue;
    if (!looksDurable(candidate.file)) continue;
    const stats = await quickStats(candidate.file);
    list.push({
      id: candidate.id,
      own: candidate.own,
      path: candidate.file,
      bytes: info.size,
      updatedAt: info.mtimeMs,
      entries: stats.entries,
      conversations: stats.conversations,
      ok: stats.ok,
      why: stats.why,
    });
  }
  list.sort((a, b) => Number(b.own) - Number(a.own) || b.entries - a.entries);
  targetCache = { at: now(), list };
  return list;
}

/** 只接受 targets 里出现过的 id —— 这页不开任意路径读取的口子。 */
async function resolveTarget(id) {
  const list = await discover();
  return list.find((item) => item.id === id) ?? list[0] ?? null;
}

// ── 读库 ───────────────────────────────────────────────────────────────────────

function withDb(target, fn) {
  const db = new DatabaseSync(target.path, { readOnly: true, timeout: 2500 });
  try {
    return fn(db);
  } finally {
    db.close();
  }
}

const tables = (db) =>
  db.prepare("select name from sqlite_master where type='table' and name not like 'sqlite_%' order by name").all();

/** dbstat：真页数，不是按 json 长度估的。 */
function dbstat(db) {
  try {
    const rows = db.prepare("select name, sum(pgsize) bytes, count(*) pages from dbstat group by name").all();
    const tbl = new Set(tables(db).map((t) => t.name));
    const indexes = new Map(
      db
        .prepare("select name, tbl_name from sqlite_master where type='index' and tbl_name not like 'sqlite_%'")
        .all()
        .map((r) => [r.name, r.tbl_name]),
    );
    const table = [];
    const index = [];
    for (const row of rows) {
      const entry = { name: row.name, bytes: num(row.bytes), pages: num(row.pages) };
      if (tbl.has(row.name)) table.push(entry);
      else if (indexes.has(row.name)) index.push({ ...entry, table: indexes.get(row.name) });
    }
    return { table, index, ok: true };
  } catch (error) {
    return { table: [], index: [], ok: false, why: String(error?.message ?? error) };
  }
}

/** tasks.kind / documents.kind 在盘上是 JSON 字面量（`"pi.generation"`），显示前把引号去掉。 */
const unquote = (value) => {
  const text = String(value ?? "");
  if (text.length > 1 && text.startsWith('"') && text.endsWith('"')) {
    try {
      return String(JSON.parse(text));
    } catch {
      return text.slice(1, -1);
    }
  }
  return text;
};

const rowCount = (db, name) => {
  try {
    return num(db.prepare(`select count(*) n from "${name.replace(/"/g, '""')}"`).get()?.n);
  } catch {
    return null;
  }
};

/** entries 的 record 是 JSON：一条 40KB 的 system 提示和一条 20 字节的「收到」同住一张表。 */
function entriesByKind(db) {
  try {
    return db
      .prepare(
        `select coalesce(json_extract(record,'$.kind'),'?') kind, count(*) n, sum(length(record)) bytes
         from entries group by 1 order by bytes desc`,
      )
      .all()
      .map((row) => ({ kind: String(row.kind), count: num(row.n), bytes: num(row.bytes) }));
  } catch {
    return [];
  }
}

/** 把一条 entry 的 record 拆成这页关心的几样东西：谁说的、多大、多少钱、调了谁。 */
function shapeEntry(row) {
  let record = {};
  try {
    record = JSON.parse(row.record);
  } catch {
    return null;
  }
  const model = Array.isArray(record.model) ? record.model[0] : null;
  const kind = String(record.kind ?? "");
  const content = Array.isArray(model?.content) ? model.content : [];
  const text = content
    .filter((part) => part?.type === "text" || part?.type === "thinking")
    .map((part) => String(part.text ?? part.thinking ?? ""))
    .join("\n");
  const flat =
    typeof model?.content === "string" ? model.content : text || "";
  const toolCalls = content
    .filter((part) => part?.type === "toolCall")
    .map((part) => ({ id: String(part.id ?? ""), tool: String(part.name ?? ""), args: part.arguments ?? {} }));
  const usage = model?.usage ?? null;
  const toolResult = model?.role === "toolResult";
  return {
    id: num(row.id),
    conv: num(row.conversation_id),
    seq: num(row.commit_seq),
    head: row.head == null ? null : num(row.head),
    kind,
    bytes: String(row.record ?? "").length,
    at: num(model?.timestamp),
    role: String(model?.role ?? ""),
    model: model?.model ? String(model.model) : null,
    provider: model?.provider ? String(model.provider) : null,
    tokens: num(usage?.totalTokens),
    cost: num(usage?.cost?.total),
    usage: usage
      ? {
          input: num(usage.input),
          output: num(usage.output),
          cacheRead: num(usage.cacheRead),
          cacheWrite: num(usage.cacheWrite),
          reasoning: num(usage.reasoning),
          totalTokens: num(usage.totalTokens),
          cost: num(usage.cost?.total),
        }
      : null,
    tool: toolResult ? String(model.toolName ?? "") : null,
    toolCalls,
    isError: Boolean(model?.isError),
    ms: model?.durationMs != null ? num(model.durationMs) : null,
    stopReason: model?.stopReason ? String(model.stopReason) : null,
    toolsAdded: Array.isArray(model?.toolsAdded)
      ? model.toolsAdded.map((tool) => String(tool?.name ?? "")).filter(Boolean)
      : [],
    chars: flat.length,
    text: clamp(flat.replace(/\s+/g, " ").trim(), 400),
  };
}

const entriesOf = (db, conv, limit = 400) =>
  db
    .prepare("select id, conversation_id, head, commit_seq, record from entries where conversation_id=? order by id desc limit ?")
    .all(conv, limit)
    .map(shapeEntry)
    .filter(Boolean)
    .reverse();

const allEntries = (db, limit = 5000) =>
  db
    .prepare("select id, conversation_id, head, commit_seq, record from entries order by id desc limit ?")
    .all(limit)
    .map(shapeEntry)
    .filter(Boolean)
    .reverse();

// ── 各屏 ───────────────────────────────────────────────────────────────────────

async function overview(target) {
  const [fileStat, walStat, shmStat] = await Promise.all([
    stat(target.path),
    stat(`${target.path}-wal`).catch(() => null),
    stat(`${target.path}-shm`).catch(() => null),
  ]);
  return withDb(target, (db) => {
    const pragma = (name) => {
      try {
        const row = db.prepare(`pragma ${name}`).get();
        return row ? Object.values(row)[0] : null;
      } catch {
        return null;
      }
    };
    const pageSize = num(pragma("page_size"));
    const pageCount = num(pragma("page_count"));
    const freelist = num(pragma("freelist_count"));
    const stats = dbstat(db);
    const tableList = tables(db).map((row) => {
      const found = stats.table.find((item) => item.name === row.name);
      return { name: row.name, bytes: found?.bytes ?? 0, pages: found?.pages ?? 0, rows: rowCount(db, row.name) };
    });
    tableList.sort((a, b) => b.bytes - a.bytes);
    const byKind = entriesByKind(db);
    const version = (() => {
      try {
        return num(db.prepare("select version from durable_schema").get()?.version);
      } catch {
        return 0;
      }
    })();
    const counts = {};
    for (const table of tableList) counts[table.name] = table.rows;
    const total = num(fileStat.size);
    const freeBytes = freelist * pageSize;
    let integrity = { checked: false, ok: null, messages: [] };
    if (total < 64 * 1024 * 1024) {
      try {
        const rows = db.prepare("pragma integrity_check").all().map((row) => String(Object.values(row)[0]));
        integrity = { checked: true, ok: rows.length === 1 && rows[0] === "ok", messages: rows.slice(0, 4) };
      } catch (error) {
        integrity = { checked: true, ok: false, messages: [String(error?.message ?? error)] };
      }
    }
    const bytes = {
      db: total,
      wal: num(walStat?.size),
      shm: num(shmStat?.size),
      free: freeBytes,
      used: total - freeBytes,
      onDisk: total + num(walStat?.size) + num(shmStat?.size),
    };
    const attention = [];
    if (integrity.checked && integrity.ok === false) attention.push("integrity_check 没通过 —— 先备份，再谈别的");
    if (bytes.wal > 4 * 1024 * 1024) attention.push(`WAL 已 ${D.human(bytes.wal)}：没有别的读者在等它 checkpoint`);
    if (bytes.free > total * 0.2) attention.push(`空闲页 ${D.human(bytes.free)}（${Math.round((bytes.free / total) * 100)}%）：VACUUM 能拿回来`);
    if (version < 2) attention.push(`schema v${version}：比这个页面认识的 v2 旧`);
    if (!target.own) attention.push("旁路：这份库属于别的块，这页只是隔着文件名读它");
    return {
      target: { id: target.id, own: target.own, path: target.path },
      file: {
        db: total,
        wal: num(walStat?.size),
        shm: num(shmStat?.size),
        mtime: num(fileStat.mtimeMs),
        walMtime: num(walStat?.mtimeMs),
      },
      pragmas: {
        pageSize,
        pageCount,
        freelist,
        journalMode: String(pragma("journal_mode") ?? ""),
        autoVacuum: num(pragma("auto_vacuum")),
        userVersion: num(pragma("user_version")),
        schemaVersion: version,
      },
      bytes,
      counts,
      tables: tableList,
      indexes: stats.index,
      entriesByKind: byKind,
      integrity,
      attention,
      diagrams: {
        treemap: D.treemap({ db: total, tables: tableList, free: freeBytes }, byKind, stats.index),
        treemapKinds: D.treemapKinds(byKind),
        er: D.erSchema(version),
      },
    };
  });
}

function conversationList(target) {
  return withDb(target, (db) => {
    const rows = db.prepare("select id, owner_conversation_id, owner_task_id from conversations order by id").all();
    const entries = allEntries(db, 20000);
    const byConv = new Map();
    for (const entry of entries) {
      if (!byConv.has(entry.conv)) byConv.set(entry.conv, []);
      byConv.get(entry.conv).push(entry);
    }
    const list = rows.map((row) => {
      const items = byConv.get(num(row.id)) ?? [];
      const user = items.find((entry) => entry.role === "user");
      const last = items.at(-1);
      return {
        id: num(row.id),
        ownerConv: row.owner_conversation_id == null ? null : num(row.owner_conversation_id),
        ownerTask: row.owner_task_id == null ? null : num(row.owner_task_id),
        entries: items.length,
        tokens: items.reduce((sum, entry) => sum + entry.tokens, 0),
        cost: items.reduce((sum, entry) => sum + entry.cost, 0),
        toolCalls: items.reduce((sum, entry) => sum + entry.toolCalls.length, 0),
        models: [...new Set(items.map((entry) => entry.model).filter(Boolean))],
        firstAt: items[0]?.at ?? 0,
        lastAt: last?.at ?? 0,
        title: clamp(String(user?.text ?? "").slice(0, 80) || "(没有用户输入)", 90),
      };
    });
    return {
      count: list.length,
      list,
      diagrams: { git: D.gitTree(list), timeline: D.timelineConversations(list) },
      totals: {
        entries: list.reduce((sum, row) => sum + row.entries, 0),
        tokens: list.reduce((sum, row) => sum + row.tokens, 0),
        cost: list.reduce((sum, row) => sum + row.cost, 0),
      },
    };
  });
}

function conversationDetail(target, id) {
  return withDb(target, (db) => {
    const head = db.prepare("select id, owner_conversation_id, owner_task_id, record from conversations where id=?").get(id);
    if (!head) return null;
    const entries = entriesOf(db, id, 400);
    const tasks = db
      .prepare("select id, kind, status, record from tasks where conversation_id=? order by id")
      .all(id)
      .map((row) => {
        let record = {};
        try {
          record = JSON.parse(row.record);
        } catch {}
        return {
          id: num(row.id),
          kind: unquote(row.kind),
          status: String(row.status),
          startedAt: num(record.startedAt),
          endedAt: num(record.endedAt),
          ms: num(record.endedAt) - num(record.startedAt),
        };
      });
    const docs = db
      .prepare(
        `select d.id, d.kind, d.key_value, d.created_at, d.retired_at,
                (select count(*) from document_revisions r where r.document_id=d.id) revisions,
                (select coalesce(sum(length(r.content)),0) from document_revisions r where r.document_id=d.id) bytes
         from documents d where d.scope_kind='conversation' and d.owner_id=? order by d.kind`,
      )
      .all(id);
    const submissions = db
      .prepare("select id, status, request_id from submissions where conversation_id=? order by id")
      .all(id)
      .map((row) => ({ id: num(row.id), status: String(row.status), requestId: row.request_id ?? null }));
    return {
      id,
      ownerConv: head.owner_conversation_id == null ? null : num(head.owner_conversation_id),
      ownerTask: head.owner_task_id == null ? null : num(head.owner_task_id),
      entries,
      tasks,
      documents: docs.map((row) => ({
        id: num(row.id),
        kind: unquote(row.kind),
        key: String(row.key_value),
        revisions: num(row.revisions),
        bytes: num(row.bytes),
        createdAt: num(row.created_at),
        retired: row.retired_at != null,
      })),
      submissions,
      diagrams: {
        timeline: D.timeline({ id }, entries),
        sequence: D.sequence(entries),
        // 「上下文是怎么涨上去的」：每次调用的 input + cacheRead 就是它当时看到的总量。
        context: D.xySeries(
          entries
            .filter((entry) => entry.kind === "pi.assistant" && entry.usage)
            .map((entry) => ({ label: `#${entry.id}`, tokens: (entry.usage.input ?? 0) + (entry.usage.cacheRead ?? 0) })),
          "tokens",
          "tokens",
          "line",
        ),
      },
    };
  });
}

/**
 * 「此刻模型看到了什么」：把 upTo 之前的所有 entry 按顺序摊成一条消息流。
 * 这不是模拟 —— 用的就是库里存的那一条条 record，只补了两个数：
 * 真实 input tokens（那条 assistant 自己报的）和本条 entry 自己的体积。
 */
function promptAt(target, conv, upTo) {
  return withDb(target, (db) => {
    const rows = upTo
      ? db
          .prepare("select id, conversation_id, head, commit_seq, record from entries where conversation_id=? and id<=? order by id")
          .all(conv, upTo)
      : db.prepare("select id, conversation_id, head, commit_seq, record from entries where conversation_id=? order by id").all(conv);
    const messages = [];
    let chars = 0;
    let toolChars = 0;
    for (const row of rows) {
      const entry = shapeEntry(row);
      if (!entry) continue;
      chars += entry.chars;
      if (entry.kind === "pi.system") toolChars += entry.chars;
      let label = entry.kind;
      if (entry.role === "user") label = "user";
      else if (entry.role === "assistant") label = `assistant · ${entry.model ?? "?"}`;
      else if (entry.role === "toolResult") label = `tool-result · ${entry.tool}`;
      else if (entry.role === "system")
        label = `system · ${entry.toolsAdded.length} 个工具描述（${entry.toolsAdded.slice(0, 6).join(" ")}）`;
      messages.push({
        id: entry.id,
        label,
        kind: entry.kind,
        bytes: entry.bytes,
        chars: entry.chars,
        tokens: entry.tokens,
        cost: entry.cost,
        head: entry.head != null,
        text: entry.text,
      });
    }
    const target = rows.at(-1) ? shapeEntry(rows.at(-1)) : null;
    const inputTokens = target?.usage?.input ?? messages.filter((m) => m.kind === "pi.assistant").at(-1)?.tokens ?? 0;
    return {
      conv,
      upTo: upTo ?? null,
      messages,
      totals: {
        messages: messages.length,
        chars,
        toolChars,
        approxTokens: Math.round(chars / 3.6),
        realInputTokens: inputTokens,
      },
    };
  });
}

function cost(target) {
  return withDb(target, (db) => {
    const entries = allEntries(db, 20000).filter((entry) => entry.kind === "pi.assistant");
    const totals = {
      entries: entries.length,
      tokens: 0,
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
      reasoning: 0,
      cost: 0,
    };
    const byDay = new Map();
    const byHour = new Map();
    const byModel = new Map();
    const byConv = new Map();
    const flowsByPair = new Map();
    for (const entry of entries) {
      totals.tokens += entry.tokens;
      totals.input += entry.usage?.input ?? 0;
      totals.output += entry.usage?.output ?? 0;
      totals.cacheRead += entry.usage?.cacheRead ?? 0;
      totals.cacheWrite += entry.usage?.cacheWrite ?? 0;
      totals.reasoning += entry.usage?.reasoning ?? 0;
      totals.cost += entry.cost;
      const at = entry.at ? new Date(entry.at) : null;
      const day = at ? at.toISOString().slice(5, 10) : "?";
      const hour = at ? `${day} ${String(at.getHours()).padStart(2, "0")}h` : "?";
      const model = entry.model ?? "?";
      const bump = (map, key, extra) => {
        const row = map.get(key) ?? { label: key, tokens: 0, cost: 0, calls: 0, input: 0, output: 0, cacheRead: 0, ...extra };
        row.tokens += entry.tokens;
        row.cost += entry.cost;
        row.calls += 1;
        row.input += entry.usage?.input ?? 0;
        row.output += entry.usage?.output ?? 0;
        row.cacheRead += entry.usage?.cacheRead ?? 0;
        map.set(key, row);
      };
      bump(byDay, day);
      bump(byHour, hour);
      bump(byModel, model);
      bump(byConv, `#${entry.conv}`);
      const pair = `#${entry.conv}\u0000${model}`;
      flowsByPair.set(pair, (flowsByPair.get(pair) ?? 0) + (entry.tokens || 0));
    }
    const days = [...byDay.values()].sort((a, b) => String(a.label).localeCompare(String(b.label)));
    // 只有一天数据时，按天画等于一根柱子 —— 退到小时桶，让这张图还有话说。
    const hours = [...byHour.values()].sort((a, b) => String(a.label).localeCompare(String(b.label)));
    const series = days.length >= 2 ? days : hours;
    const seriesLabel = days.length >= 2 ? "按天" : "按小时";
    const models = [...byModel.values()].sort((a, b) => b.tokens - a.tokens);
    const convs = [...byConv.values()].sort((a, b) => b.tokens - a.tokens);
    // token 从哪来、到哪去：会话 → 模型 → (输入 / 输出 / 缓存)。三段流量对得上，图才可信。
    const flows = [];
    for (const [key, value] of flowsByPair) {
      const [convLabel, modelLabel] = key.split("\u0000");
      flows.push({ from: convLabel, to: modelLabel, value });
    }
    for (const model of models.slice(0, 4)) {
      if (model.input) flows.push({ from: model.label, to: "input", value: Math.round(model.input) });
      if (model.output) flows.push({ from: model.label, to: "output", value: Math.round(model.output) });
      if (model.cacheRead) flows.push({ from: model.label, to: "cache", value: Math.round(model.cacheRead) });
    }
    return {
      totals,
      byDay: days,
      byHour: hours,
      byModel: models,
      byConv: convs,
      diagrams: {
        xyTokens: D.xySeries(series, "tokens", "tokens"),
        pieModels: D.pie(models, "tokens"),
        sankey: D.sankeyTokens(flows),
      },
      note: "cost 为 0 不代表免费：provider 没回 price 的时候 pi-durable 记 0，tokens 才是真数。",
    };
  });
}

function health(target) {
  return withDb(target, (db) => {
    const taskRows = db
      .prepare("select id, conversation_id, kind, status, background, record from tasks order by id desc limit 2000")
      .all()
      .map((row) => {
        let record = {};
        try {
          record = JSON.parse(row.record);
        } catch {}
        return {
          id: num(row.id),
          conv: num(row.conversation_id),
          kind: unquote(row.kind),
          status: String(row.status),
          background: Boolean(row.background),
          startedAt: num(record.startedAt),
          endedAt: num(record.endedAt),
          outcome: record?.state?.outcome?.status ?? null,
        };
      });
    const byStatusMap = new Map();
    for (const task of taskRows) {
      const row = byStatusMap.get(task.status) ?? { status: task.status, count: 0, sample: [] };
      row.count += 1;
      if (row.sample.length < 4) row.sample.push({ id: task.id, kind: task.kind });
      byStatusMap.set(task.status, row);
    }
    const order = ["running", "completing", "waiting", "sleeping", "pending", "terminal"];
    const byStatus = [...byStatusMap.values()].sort((a, b) => order.indexOf(a.status) - order.indexOf(b.status));
    const stuck = taskRows
      .filter((task) => ["running", "completing", "waiting"].includes(task.status) && task.startedAt && now() - task.startedAt > STUCK_MS)
      .map((task) => ({ ...task, ageMs: now() - task.startedAt }));
    const sleeping = taskRows.filter((task) => task.status === "sleeping");
    const submissions = db
      .prepare("select id, conversation_id, status from submissions order by id desc limit 2000")
      .all()
      .map((row) => ({ id: num(row.id), conv: num(row.conversation_id), status: String(row.status) }));
    const subStatus = new Map();
    for (const row of submissions) subStatus.set(row.status, (subStatus.get(row.status) ?? 0) + 1);
    const unanswered = submissions.filter((row) => row.status === "unanswered");
    const revisions = (() => {
      try {
        return db.prepare("select kind, count(*) n from document_revisions group by kind").all();
      } catch {
        return [];
      }
    })();
    const base = num(revisions.find((row) => row.kind === "base")?.n);
    const delta = num(revisions.find((row) => row.kind === "delta")?.n);
    const docs = (() => {
      try {
        return db.prepare("select kind, count(*) n from documents group by kind order by n desc").all();
      } catch {
        return [];
      }
    })();
    const walStat = (() => {
      try {
        return statSync(`${target.path}-wal`).size;
      } catch {
        return 0;
      }
    })();
    const freelance = (() => {
      try {
        const pageSize = num(Object.values(db.prepare("pragma page_size").get() ?? {})[0]);
        const free = num(Object.values(db.prepare("pragma freelist_count").get() ?? {})[0]);
        return { free, bytes: free * pageSize };
      } catch {
        return { free: 0, bytes: 0 };
      }
    })();
    const durations = taskRows.filter((task) => task.startedAt && task.endedAt >= task.startedAt);
    const avg = durations.length ? durations.reduce((sum, task) => sum + (task.endedAt - task.startedAt), 0) / durations.length : 0;
    const sorted = durations.map((task) => task.endedAt - task.startedAt).sort((a, b) => a - b);
    const p95 = sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * 0.95))] : 0;
    const slowest = [...durations].sort((a, b) => b.endedAt - b.startedAt - (a.endedAt - a.startedAt)).slice(0, 6);
    const maintenance = [
      {
        label: "先做一次备份",
        why: "改动任何东西之前，先把库整份复制走 —— VACUUM 也是写操作。",
        cmd: `cp "${target.path}" "${target.path}.$(date +%Y%m%d-%H%M).bak"`,
        danger: false,
      },
      {
        label: "把 WAL 收回去",
        why: `WAL 现在 ${D.human(walStat)}。checkpoint 只读的页面做不了，得由写者或 sqlite3 来。`,
        cmd: `sqlite3 "${target.path}" "PRAGMA wal_checkpoint(TRUNCATE);"`,
        danger: false,
      },
      {
        label: "VACUUM 要回空闲页",
        why: `空闲页 ${freelance.free} 个（${D.human(freelance.bytes)}）。这块停止服务后再跑 —— 它要独占写者。`,
        cmd: `sqlite3 "${target.path}" "VACUUM;"`,
        danger: false,
      },
      {
        label: "看它到底是不是好的",
        why: "integrity_check 会把整库走一遍；库大了要几分钟。",
        cmd: `sqlite3 "${target.path}" "PRAGMA integrity_check;"`,
        danger: false,
      },
      {
        label: "把老会话搬去 storage-Archive",
        why: "先导出、再删。别直接 DELETE —— 那等于把历史删掉，而不是归档。",
        cmd: `sqlite3 "${target.path}" ".dump conversations entries tasks submissions" > conversations-$(date +%F).sql`,
        danger: true,
      },
    ];
    return {
      tasks: { total: taskRows.length, byStatus, stuck, sleeping, avgMs: Math.round(avg), p95, slowest },
      submissions: { total: submissions.length, byStatus: [...subStatus].map(([status, n]) => ({ status, n })), unanswered },
      documents: {
        documents: docs.reduce((sum, row) => sum + num(row.n), 0),
        byKind: docs.map((row) => ({ kind: unquote(row.kind), n: num(row.n) })),
        revisions: base + delta,
        base,
        delta,
        deltaRatio: base + delta ? delta / (base + delta) : 0,
      },
      walBytes: walStat,
      freelist: freelance,
      maintenance,
      diagrams: {
        kanban: D.kanban({ byStatus }),
        quadrant: D.quadrantTasks(taskRows),
        // 生成 vs 工具：这个比值才说明「钱花在想上，还是花在翻文件上」。
        pieKinds: D.pie(
          [...taskRows.reduce((map, task) => map.set(task.kind, (map.get(task.kind) ?? 0) + 1), new Map())].map(
            ([label, n]) => ({ label, tokens: n }),
          ),
          "tokens",
        ),
        pieSubmissions: D.pie(
          [...subStatus].map(([status, n]) => ({ label: status, tokens: n })),
          "tokens",
        ),
      },
    };
  });
}

// ── 路由 ───────────────────────────────────────────────────────────────────────

const json = (res, body, status = 200) => {
  const text = JSON.stringify(body);
  res.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
    "content-length": Buffer.byteLength(text),
  });
  res.end(text);
};

async function serveStatic(res, pathname) {
  const rel = pathname.replace(/^\/+/, "");
  const file = path.join(PUBLIC, rel === "" ? "index.html" : rel);
  if (!file.startsWith(PUBLIC)) return json(res, { error: "越界" }, 400);
  try {
    const info = await stat(file);
    if (info.isDirectory()) return serveStatic(res, path.posix.join(pathname, "index.html"));
    const body = await readFile(file);
    const ext = path.extname(file);
    res.writeHead(200, {
      "content-type": MIME[ext] ?? "application/octet-stream",
      "cache-control": rel.startsWith("vendor/") ? "public, max-age=86400" : "no-cache",
      "content-length": body.length,
    });
    res.end(body);
  } catch {
    json(res, { error: "没有这个文件", path: pathname }, 404);
  }
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  const route = url.pathname;
  try {
    if (route === "/healthz") {
      return json(res, { ok: true, up: now() - STARTED, targets: (await discover()).length });
    }
    if (route === "/api/targets") {
      const list = await discover(url.searchParams.has("fresh"));
      return json(res, { targets: list, default: list[0]?.id ?? null, siblings: SIBLINGS, self: SELF_ID });
    }
    if (route.startsWith("/api/")) {
      const target = await resolveTarget(url.searchParams.get("db") ?? "");
      if (!target) return json(res, { error: "没有可看的 agent.sqlite", targets: [] }, 404);
      switch (route) {
        case "/api/overview":
          return json(res, await overview(target));
        case "/api/conversations":
          return json(res, conversationList(target));
        case "/api/conversation": {
          const id = Number(url.searchParams.get("id"));
          const detail = conversationDetail(target, id);
          if (!detail) return json(res, { error: `会话 #${id} 不在库里` }, 404);
          const upTo = url.searchParams.has("upTo") ? Number(url.searchParams.get("upTo")) : null;
          detail.prompt = promptAt(target, id, upTo);
          return json(res, detail);
        }
        case "/api/prompt": {
          const id = Number(url.searchParams.get("id"));
          const upTo = url.searchParams.get("upTo");
          return json(res, promptAt(target, id, upTo ? Number(upTo) : null));
        }
        case "/api/cost":
          return json(res, cost(target));
        case "/api/health":
          return json(res, health(target));
        default:
          return json(res, { error: "没有这个端点", route }, 404);
      }
    }
    return serveStatic(res, route);
  } catch (error) {
    json(res, { error: String(error?.message ?? error), route }, 500);
  }
});

server.listen(PORT, () => {
  console.log(`[d6s4v1] xray on :${PORT} · data=${DATA} · self=${SELF_ID} · siblings=${SIBLINGS}`);
});

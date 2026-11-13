// 归档四条 BFF 路由的契约 —— 真的 server.js 起在临时端口，对面是一个 stub 引擎
// （走 BJES6B_ENGINE_URL 那个逃生口，绕过 resolve）。测的是「转发」这件事本身：
// 方法、路径、body 一个字不改；引擎的错误连状态码带 {error} 原样回来。
// 不测 DOM（那是 smoke-archive.mjs 的活），也不测引擎内部的归档语义（qrd3wh 自己测）。
import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));

/* ── stub 引擎：形状照 qrd3wh 的 /v1/*，行为最小 ──
   registry：id → { name, gen, tombstone: {at,gen,purged}|null, live }。
   - archive  = 复印 + 墓碑（tombstone.purged=false，会话从 /v1/sessions 里藏掉）
   - purge    = 撕原件（tombstone.purged=true，live=false；没墓碑就 409）
   - restore  = 打捞（墓碑抹掉，live=true）                                             */
const hits = [];
const registry = new Map([
  [4, { name: "档案甲", gen: "20261110", tombstone: { at: 1, gen: "20261110", purged: false }, live: true }],
  [5, { name: "档案乙", gen: "20261110", tombstone: { at: 2, gen: "20261110", purged: true }, live: false }],
  [6, { name: "档案丙", gen: "20261108", tombstone: null, live: true }],
  [7, { name: "档案丁", gen: "20261108", tombstone: null, live: false }],
]);

const baseSessions = () => [
  { id: 1, name: "会话甲", parent: null, busy: null, last: { id: 11, text: "你好" } },
  { id: 2, name: "会话乙", parent: null, busy: null, last: { id: 12, text: "在吗" } },
  { id: 3, name: "忙会话", parent: null, busy: { since: 1 }, last: { id: 13, text: "跑着呢" } },
];

const liveSessions = () =>
  baseSessions().filter((s) => {
    const r = registry.get(s.id);
    return !r || r.tombstone === null;
  });

const browseView = () => {
  const byGen = new Map();
  for (const [id, r] of registry) {
    const item = {
      id,
      parent: null,
      entries: 0,
      last: null,
      name: r.name,
      archived: r.tombstone ? { ...r.tombstone } : null,
      live: r.live,
    };
    if (!byGen.has(r.gen)) byGen.set(r.gen, []);
    byGen.get(r.gen).push(item);
  }
  return { gens: [...byGen.entries()].map(([gen, items]) => ({ gen, items })) };
};

const jsonReply = (res, status, value) => {
  const body = JSON.stringify(value);
  res.writeHead(status, { "content-type": "application/json", "content-length": Buffer.byteLength(body) });
  res.end(body);
};

const engine = createServer((req, res) => {
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", () => {
    const body = Buffer.concat(chunks).toString("utf8");
    hits.push({ method: req.method, path: new URL(req.url, "http://x").pathname, body });
    const p = new URL(req.url, "http://x").pathname;

    if (p === "/v1/sessions" && req.method === "GET") return jsonReply(res, 200, { items: liveSessions() });

    const session = p.match(/^\/v1\/sessions\/(\d+)\/(archive|purge)$/);
    if (session) {
      const id = Number(session[1]);
      const verb = session[2];
      const record = baseSessions().find((s) => s.id === id);
      if (!record) return jsonReply(res, 404, { error: "没有这个会话", id });
      if (verb === "archive") {
        if (record.busy) return jsonReply(res, 409, { error: "会话正在忙，等它说完再归档", id });
        registry.set(id, { name: record.name, gen: "20261110", tombstone: { at: 9, gen: "20261110", purged: false }, live: true });
        return jsonReply(res, 200, { gen: "20261110", ids: [id], rows: 3 });
      }
      const r = registry.get(id);
      if (!r || r.tombstone === null) return jsonReply(res, 409, { error: "它还没归档过 —— 先归档再撕，别裸删", id });
      if (r.tombstone.purged) return jsonReply(res, 409, { error: "原件已经撕过了", id });
      r.tombstone.purged = true;
      r.live = false;
      return jsonReply(res, 200, { purged: [id], gen: r.gen, rows: 3 });
    }

    if (p === "/v1/archive" && req.method === "GET") return jsonReply(res, 200, browseView());

    const restore = p.match(/^\/v1\/archive\/(\d+)\/restore$/);
    if (restore && req.method === "POST") {
      const gen = String(JSON.parse(body || "{}").gen ?? "").replace(/\D/g, "");
      if (!gen) return jsonReply(res, 400, { error: "gen 得是日期（YYYYMMDD）" });
      const r = registry.get(Number(restore[1]));
      if (!r) return jsonReply(res, 404, { error: "归档库里没有这个会话", id: restore[1] });
      r.tombstone = null;
      r.live = true;
      return jsonReply(res, 200, { gen, restored: [Number(restore[1])], rows: 3 });
    }

    jsonReply(res, 404, { error: "stub 引擎没有这条路", path: p });
  });
});
await new Promise((r) => engine.listen(0, "127.0.0.1", r));
const ENGINE_URL = `http://127.0.0.1:${engine.address().port}`;

/* ── 真的 server.js，起在临时端口，指着 stub 引擎 ── */
const PORT = 4877 + Math.floor(Math.random() * 100);
const DATA = mkdtempSync(path.join(tmpdir(), "bjes6b-bff-"));
const child = spawn(process.execPath, [path.join(HERE, "..", "server.js")], {
  env: { ...process.env, PORT: String(PORT), BJES6B_ENGINE_URL: ENGINE_URL, NOUS_DATA: DATA },
  stdio: "ignore",
});
const BASE = `http://127.0.0.1:${PORT}`;
const call = async (path, init) => {
  const res = await fetch(`${BASE}${path}`, init);
  return { status: res.status, text: await res.text() };
};
let booted = false;
for (let i = 0; i < 50 && !booted; i++) {
  try {
    booted = (await call("/api/health")).status === 200;
  } catch {
    await new Promise((r) => setTimeout(r, 100));
  }
}
if (!booted) {
  child.kill();
  throw new Error("server.js 没起来");
}

test.after(() => {
  child.kill();
  engine.close();
});

test("GET /api/archive 原样转发：形状、字段、四态原料一个不改", async () => {
  const mine = await call("/api/archive");
  const up = await fetch(`${ENGINE_URL}/v1/archive`);
  assert.equal(mine.status, up.status);
  assert.deepEqual(JSON.parse(mine.text), await up.json());
  // 引擎侧只该收到一次 GET /v1/archive
  assert.ok(hits.filter((h) => h.method === "GET" && h.path === "/v1/archive").length >= 1);
});

test("POST /api/sessions/:id/archive 原样转发 + 成功回原文", async () => {
  const out = await call("/api/sessions/1/archive", { method: "POST" });
  assert.equal(out.status, 200);
  assert.deepEqual(JSON.parse(out.text), { gen: "20261110", ids: [1], rows: 3 });
  const hit = hits.find((h) => h.method === "POST" && h.path === "/v1/sessions/1/archive");
  assert.ok(hit, "引擎没收到 archive");
});

test("POST /api/sessions/:id/archive 引擎 409 连状态码带 {error} 原样回", async () => {
  const out = await call("/api/sessions/3/archive", { method: "POST" });
  assert.equal(out.status, 409);
  assert.deepEqual(JSON.parse(out.text), { error: "会话正在忙，等它说完再归档", id: 3 });
});

test("POST /api/sessions/:id/purge 没归档就撕 → 引擎原文 409", async () => {
  const out = await call("/api/sessions/2/purge", { method: "POST" });
  assert.equal(out.status, 409);
  assert.deepEqual(JSON.parse(out.text), { error: "它还没归档过 —— 先归档再撕，别裸删", id: 2 });
});

test("POST /api/sessions/:id/purge 已归档的撕成功", async () => {
  const out = await call("/api/sessions/1/purge", { method: "POST" });
  assert.equal(out.status, 200);
  assert.deepEqual(JSON.parse(out.text), { purged: [1], gen: "20261110", rows: 3 });
});

test("POST /api/archive/:id/restore body 原样转发（gen 一字不改）", async () => {
  const raw = JSON.stringify({ gen: "20261110" });
  const out = await call("/api/archive/4/restore", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: raw,
  });
  assert.equal(out.status, 200);
  assert.deepEqual(JSON.parse(out.text), { gen: "20261110", restored: [4], rows: 3 });
  const hit = hits.find((h) => h.method === "POST" && h.path === "/v1/archive/4/restore");
  assert.ok(hit, "引擎没收到 restore");
  assert.equal(hit.body, raw, "BFF 改写了 body");
});

test("GET /api/sessions 依旧只认主线：归档过的会话被藏掉", async () => {
  const out = await call("/api/sessions");
  const ids = JSON.parse(out.text).items.map((s) => s.id);
  assert.ok(!ids.includes(1), "归档过的会话还在列表里");
  assert.ok(ids.includes(2) && ids.includes(3));
});

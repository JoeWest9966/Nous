// archive.js —— 会话归档：复印 + 撕原件，agent.sqlite 永不关。
//
// pi-durable 在会话粒度没有任何删除 API（ConversationRecord 无墓碑字段、Tx 无 delete、
// record 无 retire 路径），而活 Session 绝不能关。所以「删除」在这里拆成两个在线动作：
//
//   归档（复印）：第二个**只读**连接把死子树的行原样读出来（id 不变），写进
//                data/archives/gen-YYYYMMDD.sqlite（同 durable schema、按天一个文件）；
//                墓碑记进 Sessions 文档（server.js 那边管），列表即不可见。
//   真删（撕）：  纯手动。条件苛刻：必须已复印 / 会话 idle / 活库里没有它的活 fork 后代；
//                第二个连接一个事务 DELETE 它在九张表里的全部行，墓碑记 purged。
//   打捞（restore）：归档库的行原样 INSERT 回活库 + 抹墓碑 —— SPIKE 验证过：
//                撕掉和捞回的全程 Session 活着不炸，id 原样保留，捞回可继续聊。
//
// 这是低于 durable 官方 API 的手术：我们直接动它的表布局（九张表名来自
// pi-durable/src/storage/sqlite/schema.ts）。兜底两道：pi-durable 钉死 1.4.0（package.json），
// 开工前校验目标库的 durable_schema 版本，不认就拒跑。decision 见
// /opt/Nous/docs/adr/0001-session-archive-copy-and-purge.md。
//
// 单独一个文件是为了能被单独验：不 import server.js（那个有顶层 await + harness）。

import { mkdir } from "node:fs/promises";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

// durable 的 SQLite 布局（1.4.0）。名字变了 = 上游动了表，手术立刻停。
// owner 列没列在这里，因为每张表的 owner 列名不一样，写在下面的 TABLES 里。
const DURABLE_SCHEMA_VERSION = 2;

const TABLES = {
  conversations: { owner: "id" },
  entries: { owner: "conversation_id" },
  tasks: { owner: "conversation_id" },
  submissions: { owner: "conversation_id" },
  documents: { owner: "owner_id", scope: "scope_kind", scopeValue: "conversation" },
};

// record_ids 是全局 id 表：会话本身的行按 id 删，entry/task/submission/document 的 id 一起带走。
// durable_metadata / durable_schema 是全库单例，归档永远不碰。

export const openArchive = ({ dataDir, liveDbPath, log = () => {} }) => {
  const ARCHIVE_DIR = path.join(dataDir, "archives");
  const genPath = (gen) => path.join(ARCHIVE_DIR, `${gen}.sqlite`);
  const today = () => new Date().toISOString().slice(0, 10).replaceAll("-", "");

  const checkSchema = (db, label) => {
    const row = db.prepare("SELECT version FROM durable_schema WHERE singleton = 1").get();
    if (!row || row.version !== DURABLE_SCHEMA_VERSION) {
      throw new Error(
        `${label} 的 durable_schema 版本不认（要 ${DURABLE_SCHEMA_VERSION}，盘上是 ${row?.version ?? "无"}）—— pi-durable 升级过，手术停，先回看 ADR 0001`,
      );
    }
  };

  /** 读出「归档死子树」的全部行：会话本身 + 它的全部后代（fork 树闭包）+ 各自的 entry/task/submission/document。 */
  const readClosure = (db, rootId) => {
    // 先把闭包算出来：root + 递归所有 parent 指着它的活会话。
    // ConversationRecord.parent 存在 record json 里，SQL 没法递归 json —— 内存里走。
    const all = db.prepare("SELECT id, record FROM conversations").all();
    const parentOf = new Map();
    for (const row of all) {
      const rec = JSON.parse(row.record);
      if (rec.parent) parentOf.set(rec.id, rec.parent.conversationId);
    }
    const ids = [rootId];
    for (let i = 0; i < ids.length; i += 1) {
      for (const [id, parent] of parentOf) if (parent === ids[i] && !ids.includes(id)) ids.push(id);
    }
    // 调用方（server.js）已经查过「root 没有活后代」；闭包里出现 root 之外的 id 只会发生在
    // root 之下 —— 递归是向下的（parent === ids[i]），不会把祖先卷进来。

    const closure = { conversations: [], entries: [], tasks: [], submissions: [], documents: [], documentRevisions: [], recordIds: [] };
    const placeholders = ids.map(() => "?").join(",");

    closure.conversations = db
      .prepare(`SELECT * FROM conversations WHERE id IN (${placeholders})`)
      .all(...ids);
    closure.entries = db.prepare(`SELECT * FROM entries WHERE conversation_id IN (${placeholders})`).all(...ids);
    closure.tasks = db.prepare(`SELECT * FROM tasks WHERE conversation_id IN (${placeholders})`).all(...ids);
    closure.submissions = db.prepare(`SELECT * FROM submissions WHERE conversation_id IN (${placeholders})`).all(...ids);
    closure.documents = db
      .prepare(`SELECT * FROM documents WHERE scope_kind = 'conversation' AND owner_id IN (${placeholders})`)
      .all(...ids);

    const docIds = closure.documents.map((d) => d.id);
    closure.documentRevisions = docIds.length
      ? db.prepare(`SELECT * FROM document_revisions WHERE document_id IN (${docIds.map(() => "?").join(",")})`).all(...docIds)
      : [];
    // record_ids 要盖会话 id 和文档 id 两拨 —— 占位符按总长重算，别复用会话那个（参数对不上 = column index out of range）
    const allIds = [...ids, ...docIds];
    closure.recordIds = db
      .prepare(`SELECT * FROM record_ids WHERE id IN (${allIds.map(() => "?").join(",")})`)
      .all(...allIds);

    // record_ids 的 record_type 是小写表名（commit.ts 的 TableName），document 行也在里面。
    return { ids, closure };
  };

  /** 把一组行 INSERT 进目标库（列名从 PRAGMA 拿，两边同 schema，按名对齐、缺列报错）。 */
  const insertRows = (db, table, rows) => {
    if (!rows.length) return 0;
    const cols = db.prepare(`PRAGMA table_info(${table})`).all().map((c) => c.name);
    const sqlCols = rows.length ? Object.keys(rows[0]) : [];
    for (const c of sqlCols) if (!cols.includes(c)) throw new Error(`${table} 有不认识的列 ${c} —— 布局对不上了，停`);
    const stmt = db.prepare(
      `INSERT INTO ${table} (${sqlCols.join(",")}) VALUES (${sqlCols.map(() => "?").join(",")})`,
    );
    for (const row of rows) stmt.run(...sqlCols.map((c) => row[c]));
    return rows.length;
  };

  /** gen 文件第一次当复印纸用之前，先铺好 durable schema。 */
  // 建表语句从活库原样抄（node:sqlite 报 PRAGMA 不给 sql，只能照写）。STRICT + CHECK 一个不落 ——
  // 复印纸的列约束和活库不一样的话，行搬过去再搬回来就可能过不了活库的检查。
  const initGenSchema = (db) => {
    db.exec(`
      CREATE TABLE IF NOT EXISTS durable_schema (singleton INTEGER NOT NULL, version INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS durable_metadata (singleton INTEGER NOT NULL, next_id TEXT NOT NULL, next_seq INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS record_ids (
        id INTEGER PRIMARY KEY,
        record_type TEXT NOT NULL CHECK (record_type IN ('conversation', 'entry', 'task', 'submission', 'document'))
      ) STRICT;
      CREATE TABLE IF NOT EXISTS conversations (
        id INTEGER PRIMARY KEY,
        owner_conversation_id INTEGER,
        owner_task_id INTEGER,
        record TEXT NOT NULL CHECK (json_valid(record))
      ) STRICT;
      CREATE TABLE IF NOT EXISTS entries (
        id INTEGER PRIMARY KEY,
        conversation_id INTEGER NOT NULL,
        head INTEGER,
        commit_seq INTEGER NOT NULL,
        record TEXT NOT NULL CHECK (json_valid(record))
      ) STRICT;
      CREATE TABLE IF NOT EXISTS "tasks" (
        id INTEGER PRIMARY KEY,
        conversation_id INTEGER NOT NULL,
        kind TEXT NOT NULL,
        status TEXT NOT NULL CHECK (status IN ('pending', 'running', 'waiting', 'sleeping', 'completing', 'terminal')),
        abort_requested INTEGER NOT NULL CHECK (abort_requested IN (0, 1)),
        background INTEGER NOT NULL CHECK (background IN (0, 1)),
        record TEXT NOT NULL CHECK (json_valid(record))
      ) STRICT;
      CREATE TABLE IF NOT EXISTS submissions (
        id INTEGER PRIMARY KEY,
        conversation_id INTEGER NOT NULL,
        request_id TEXT,
        status TEXT NOT NULL CHECK (status IN ('queued', 'placed', 'done', 'unanswered')),
        record TEXT NOT NULL CHECK (json_valid(record))
      ) STRICT;
      CREATE TABLE IF NOT EXISTS documents (
        id INTEGER PRIMARY KEY,
        kind TEXT NOT NULL,
        family INTEGER NOT NULL CHECK (family IN (0, 1)),
        key_value TEXT NOT NULL,
        scope_kind TEXT NOT NULL CHECK (scope_kind IN ('session', 'conversation', 'task')),
        owner_id INTEGER NOT NULL,
        created_at INTEGER NOT NULL,
        retired_at INTEGER,
        record TEXT NOT NULL CHECK (json_valid(record))
      ) STRICT;
      CREATE TABLE IF NOT EXISTS document_revisions (
        document_id INTEGER NOT NULL,
        seq INTEGER NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('base', 'delta')),
        version INTEGER NOT NULL,
        content TEXT NOT NULL CHECK (json_valid(content)),
        PRIMARY KEY (document_id, seq)
      ) STRICT;
      INSERT OR IGNORE INTO durable_schema (singleton, version) VALUES (1, ${DURABLE_SCHEMA_VERSION});
      INSERT OR IGNORE INTO durable_metadata (singleton, next_id, next_seq) VALUES (1, '1', 1);
    `);
  };

  return {
    /** 归档库目录（给浏览端点列文件用）。 */
    dir: ARCHIVE_DIR,

    // ── 归档（复印）────────────────────────────────────────────────────
    //
    // 前置（server.js 管，这里再兜一遍）：root 存在、idle、没有活后代。
    // 这里管：复印进 gen 文件。**不删活库的行** —— 撕是 purge 的事。
    //
    archive: async ({ rootId }) => {
      await mkdir(ARCHIVE_DIR, { recursive: true });
      const gen = today();
      const target = genPath(gen);

      const live = new DatabaseSync(liveDbPath, { readOnly: true });
      let copied;
      try {
        checkSchema(live, "活库");
        if (!live.prepare("SELECT id FROM conversations WHERE id = ?").get(rootId)) {
          const error = new Error(`没有这个会话：${rootId}`);
          error.status = 404;
          throw error;
        }
        const { ids, closure } = readClosure(live, rootId);

        const genDb = new DatabaseSync(target);
        try {
          initGenSchema(genDb); // 复印纸第一次用：铺 schema（幂等，已有表就跳过）
          checkSchema(genDb, `归档库 ${gen}`);
          // 同一天二次归档 = 同一个 gen 文件里已有旧行。旧复印整体作废：
          // 新复印一定包含闭包全部行，留下的旧行只会是过期副本 —— 全清再插。
          const staleIds = closure.conversations.map((c) => c.id);
          genDb.exec("BEGIN IMMEDIATE");
          if (staleIds.length) {
            const docIds = closure.documents.map((d) => d.id);
            const ph = [...staleIds, ...docIds].map(() => "?").join(",");
            if (docIds.length) genDb.prepare(`DELETE FROM document_revisions WHERE document_id IN (${docIds.map(() => "?").join(",")})`).run(...docIds);
            genDb.prepare(`DELETE FROM documents WHERE owner_id IN (${ph})`).run(...staleIds, ...docIds);
            genDb.prepare(`DELETE FROM record_ids WHERE id IN (${ph})`).run(...staleIds, ...docIds);
            genDb.prepare(`DELETE FROM submissions WHERE conversation_id IN (${ph})`).run(...staleIds);
            genDb.prepare(`DELETE FROM tasks WHERE conversation_id IN (${ph})`).run(...staleIds);
            genDb.prepare(`DELETE FROM entries WHERE conversation_id IN (${ph})`).run(...staleIds);
            genDb.prepare(`DELETE FROM conversations WHERE id IN (${ph})`).run(...staleIds);
          }
          const n =
            insertRows(genDb, "conversations", closure.conversations) +
            insertRows(genDb, "entries", closure.entries) +
            insertRows(genDb, "tasks", closure.tasks) +
            insertRows(genDb, "submissions", closure.submissions) +
            insertRows(genDb, "documents", closure.documents) +
            insertRows(genDb, "document_revisions", closure.documentRevisions) +
            insertRows(genDb, "record_ids", closure.recordIds);
          genDb.exec("COMMIT");
          copied = { gen, ids, rows: n };
        } catch (error) {
          genDb.exec("ROLLBACK");
          throw error;
        } finally {
          genDb.close();
        }
      } finally {
        live.close();
      }
      log(`[archive] 会话 ${ids_label(copied.ids)} 复印进 ${gen}（${copied.rows} 行）`);
      return copied;
    },

    // ── 真删（撕原件）──────────────────────────────────────────────────
    //
    // 前置：这条（整棵闭包）都已经复印在某个 gen 里 —— 没复印件的行撕了就是真没了。
    // 这里管：验证复印件在 + 事务 DELETE 活库九张表。墓碑的 purged 由 server.js 写。
    //
    purge: async ({ rootId }) => {
      const live = new DatabaseSync(liveDbPath);
      let result;
      try {
        checkSchema(live, "活库");
        if (!live.prepare("SELECT id FROM conversations WHERE id = ?").get(rootId)) {
          const error = new Error(`活库里没有这个会话：${rootId}（撕过了？）`);
          error.status = 404;
          throw error;
        }

        // 复印件在哪：翻所有 gen 文件，确认整棵闭包的行都在。
        const gens = (await import("node:fs/promises")).readdir;
        const files = (await gens(ARCHIVE_DIR)).filter((f) => f.endsWith(".sqlite")).sort();
        const { ids, closure } = readClosure(new DatabaseSync(liveDbPath, { readOnly: true }), rootId);
        let foundGen = null;
        for (const file of files) {
          const genDb = new DatabaseSync(genPath(file.slice(0, -7)), { readOnly: true });
          try {
            const missing = [];
            for (const row of closure.conversations) {
              if (!genDb.prepare("SELECT id FROM conversations WHERE id = ?").get(row.id)) missing.push(row.id);
            }
            const convIds = closure.conversations.map((c) => c.id);
            for (const row of closure.entries) {
              if (!genDb.prepare("SELECT id FROM entries WHERE id = ?").get(row.id)) {
                // entry 只在它自己会话的闭包里；闭包完整复印的话，同批复印过
                missing.push(`entry ${row.id}（conv ${row.conversation_id}）`);
              }
            }
            if (missing.length === 0 && convIds.every((id) =>
              genDb.prepare("SELECT id FROM conversations WHERE id = ?").get(id))) {
              foundGen = file.slice(0, -7);
              break;
            }
          } finally {
            genDb.close();
          }
        }
        if (!foundGen) {
          const error = new Error(`会话 ${ids_label(ids)} 没有完整复印件 —— 先归档再撕，别裸删`);
          error.status = 409;
          throw error;
        }

        // 撕：一个事务删干净。文档 revisions 按 document_id 连带。
        const docIds = closure.documents.map((d) => d.id);
        const allIds = [...ids, ...docIds];
        const placeholders = allIds.map(() => "?").join(",");
        live.exec("BEGIN IMMEDIATE");
        try {
          if (docIds.length) {
            live.prepare(`DELETE FROM document_revisions WHERE document_id IN (${docIds.map(() => "?").join(",")})`).run(...docIds);
          }
          live.prepare(`DELETE FROM documents WHERE scope_kind = 'conversation' AND owner_id IN (${placeholders})`).run(...allIds);
          live.prepare(`DELETE FROM record_ids WHERE id IN (${placeholders})`).run(...allIds);
          live.prepare(`DELETE FROM submissions WHERE conversation_id IN (${placeholders})`).run(...allIds);
          live.prepare(`DELETE FROM tasks WHERE conversation_id IN (${placeholders})`).run(...allIds);
          live.prepare(`DELETE FROM entries WHERE conversation_id IN (${placeholders})`).run(...allIds);
          live.prepare(`DELETE FROM conversations WHERE id IN (${placeholders})`).run(...allIds);
          live.exec("COMMIT");
        } catch (error) {
          live.exec("ROLLBACK");
          throw error;
        }
        result = { purged: ids, gen: foundGen, rows: closure.conversations.length + closure.entries.length + closure.tasks.length + closure.submissions.length + closure.documents.length };
      } finally {
        live.close();
      }
      log(`[archive] 会话 ${ids_label(result.purged)} 原件已撕（复印件在 ${result.gen}，${result.rows} 行）`);
      return result;
    },

    // ── 打捞（restore）────────────────────────────────────────────────
    //
    // 把 gen 里那棵子树原样 INSERT 回活库 + 抹墓碑（墓碑 server.js 管）。
    // SPIKE（2026-11-10）：活 Session 在读在写，第二个连接 INSERT 回来它直接认。
    //
    restore: async ({ rootId, gen }) => {
      const source = genPath(gen);
      const live = new DatabaseSync(liveDbPath);
      let result;
      try {
        checkSchema(live, "活库");
        const genDb = new DatabaseSync(source, { readOnly: true });
        let closure;
        try {
          checkSchema(genDb, `归档库 ${gen}`);
          ({ closure } = readClosure(genDb, rootId));
          if (!closure.conversations.length) {
            const error = new Error(`归档库 ${gen} 里没有会话 ${rootId}`);
            error.status = 404;
            throw error;
          }
        } finally {
          genDb.close();
        }

        // 撞车检查：活库里已经有的 id 不许再插（打捞两次 / 或 id 被新会话占了 —— 后者
        // 不可能，durable 的 id 只往前走，但两次打捞完全可能）。
        const already = [];
        for (const row of closure.conversations) {
          if (live.prepare("SELECT id FROM conversations WHERE id = ?").get(row.id)) already.push(row.id);
        }
        if (already.length) {
          const error = new Error(`会话 ${ids_label(already)} 已经在活库里 —— 不用捞`);
          error.status = 409;
          throw error;
        }

        const docIds = closure.documents.map((d) => d.id);
        live.exec("BEGIN IMMEDIATE");
        try {
          const n =
            insertRows(live, "conversations", closure.conversations) +
            insertRows(live, "entries", closure.entries) +
            insertRows(live, "tasks", closure.tasks) +
            insertRows(live, "submissions", closure.submissions) +
            insertRows(live, "documents", closure.documents) +
            insertRows(live, "document_revisions", closure.documentRevisions) +
            insertRows(live, "record_ids", closure.recordIds);
          live.exec("COMMIT");
          result = { restored: closure.conversations.map((c) => c.id), gen, rows: n };
        } catch (error) {
          live.exec("ROLLBACK");
          throw error;
        }
      } finally {
        live.close();
      }
      log(`[archive] 会话 ${ids_label(result.restored)} 从 ${gen} 捞回活库（${result.rows} 行）`);
      return result;
    },

    // ── 浏览（只读）───────────────────────────────────────────────────
    //
    // 列出归档库和里面的会话。mcfpqm 当遥控器，走 server.js 的端点来读。
    //
    browse: async () => {
      await mkdir(ARCHIVE_DIR, { recursive: true });
      const { readdir } = await import("node:fs/promises");
      const files = (await readdir(ARCHIVE_DIR)).filter((f) => f.endsWith(".sqlite")).sort().reverse();
      const gens = [];
      for (const file of files) {
        const gen = file.slice(0, -7);
        const db = new DatabaseSync(genPath(gen), { readOnly: true });
        try {
          checkSchema(db, `归档库 ${gen}`);
          const rows = db.prepare("SELECT id, record FROM conversations ORDER BY id DESC").all();
          const items = rows.map((row) => {
            const rec = JSON.parse(row.record);
            const last = db
              .prepare("SELECT record FROM entries WHERE conversation_id = ? ORDER BY id DESC LIMIT 1")
              .get(row.id);
            const count = db.prepare("SELECT COUNT(*) n FROM entries WHERE conversation_id = ?").get(row.id);
            return {
              id: row.id,
              parent: rec.parent ? { id: rec.parent.conversationId, at: rec.parent.at } : null,
              entries: count.n,
              last: last ? lastEntryText(last.record) : null,
            };
          });
          gens.push({ gen, items });
        } finally {
          db.close();
        }
      }
      return { gens };
    },
  };
};

// 会话列表里给 mcfpqm 看的最后一句。entry 的 record json 里 model 是 pi-ai 消息数组，
// 正文在 content（字符串或 block 数组）—— 抽纯文本，跟 server.js 的 textOf 一个意思，
// 但这边不 import server.js（它有顶层 await），所以自己写一份小的。
const lastEntryText = (recordJson) => {
  try {
    const record = typeof recordJson === "string" ? JSON.parse(recordJson) : recordJson;
    const model = record?.model;
    const last = Array.isArray(model) ? model[model.length - 1] : model;
    const content = last?.content;
    if (typeof content === "string") return content.slice(0, 120) || null;
    if (Array.isArray(content)) {
      const text = content
        .filter((block) => block?.type === "text" || typeof block === "string")
        .map((block) => (typeof block === "string" ? block : block.text))
        .join(" ")
        .trim();
      return text.slice(0, 120) || null;
    }
    return null;
  } catch {
    return null;
  }
};

const ids_label = (ids) => (ids.length <= 3 ? ids.join("/") : `${ids[0]}…${ids.length} 个`);

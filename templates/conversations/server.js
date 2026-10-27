// qrd3wh —— 会开很多对话的块。
//
// 一个磁盘一个 sqlite，里面 N 个 conversation。会话不是列表，是一棵树：
// 「新对话」在顶层平级长一根，「从这条分叉」在节点下面长子枝。pi-durable 的
// ConversationRecord.parent 就是那根枝，不用自己记。
//
//   GET  /                            那棵树
//   GET  /v1/device                   我是谁、模型
//   GET  /v1/sessions                 会话树（含忙闲、模型、最后一句）
//   POST /v1/sessions                 {name} → 新对话
//   POST /v1/sessions/:id/rename      {name}
//   POST /v1/sessions/:id/fork        {entryId} 或 {} （空 = 从最后一条分叉）
//   POST /v1/sessions/:id/config      {readOnly} 只读就是不给会写的工具
//   GET  /v1/sessions/:id/entries     历史（新的在前）
//   POST /v1/sessions/:id/messages    {content, requestId?, waitMs?}
//   GET  /v1/sessions/:id/events      SSE
//   POST /v1/sessions/:id/abort
//
// 所有地址都写相对的：宿主把 /p/<board>/<id>/ 反代进来。

import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { builtinModels } from "@earendil-works/pi-ai/providers/all";
import { fauxAssistantMessage, fauxProvider, fauxText, fauxToolCall } from "@earendil-works/pi-ai/providers/faux";
import { contentText } from "@earendil-works/pi-ai/utils/text";
import { AssistantEntry, Harness, createRegistry, defineDoc, watchEvents } from "@earendil-works/pi-durable";
import { CodingTools, createBashTool, createEditTool, createWriteTool } from "@earendil-works/pi-durable/tools";
import { NodeExecutionEnv } from "@earendil-works/pi-durable/env/node";
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";

const ctx = BACKGROUND_CONTEXT;
const HERE = path.dirname(fileURLToPath(import.meta.url));
const PORT = Number(process.env.PORT ?? 4780);
const DATA = process.env.NOUS_DATA ?? path.join(HERE, "data");
const startedAt = Date.now();

// 会话名不在 ConversationRecord 里（那东西没有 name 字段），也不许进 tree.json。
// 存在一个 session 级文档里 —— 名字是这盘的状态，跟棋谱一起落 sqlite。
const Sessions = defineDoc({
  kind: "qrd3wh.sessions",
  version: 1,
  scope: "session",
  initial: () => ({ names: {} }),
  // 这张表很小、改得也不勤：每次都给一份完整 base，别攒 delta。
  checkpointWhen: () => true,
});

// bash 能写，所以只读也要收掉它。
// 这里要的是**工具注册对象**不是名字 —— configure() 内部是 item.name 取名字。
const WRITE_TOOLS = [createWriteTool(), createEditTool(), createBashTool()];
const WRITE_NAMES = WRITE_TOOLS.map((tool) => tool.name);

const textOf = (content) => contentText(content ?? "").trim();
// entry.model 里的 content 可能直接是字符串（用户那条），也可能是一串 block。
// contentText 只吃后者 —— 混着来会静悄悄地变成空字符串。
const textOfEntry = (entry) => {
  if (!entry?.model?.length) return null;
  const parts = entry.model.map((m) => (typeof m.content === "string" ? m.content : contentText(m.content)));
  return parts.join("").trim() || null;
};
const key = (id) => String(id);

// ── 模型 ─────────────────────────────────────────────────────────────────

const installFaux = (models) => {
  const handle = fauxProvider({
    provider: "faux",
    models: [{ id: "echo", name: "Faux Echo", contextWindow: 128_000, maxTokens: 8_192 }],
    tokensPerSecond: 400,
  });
  const reply = (transcript) => {
    handle.setResponses([reply]);
    const messages = transcript.messages ?? [];
    const last = messages.at(-1);
    if (last?.role === "toolResult") return fauxAssistantMessage(fauxText(`工具跑完了：\n${textOf(last.content).slice(0, 800)}`));
    const prompt = textOf([...messages].reverse().find((m) => m.role === "user")?.content);
    if (prompt.startsWith("!")) {
      return fauxAssistantMessage([fauxText("跑一下。"), fauxToolCall("bash", { command: prompt.slice(1).trim() })], {
        stopReason: "toolUse",
      });
    }
    return fauxAssistantMessage(fauxText(`回声：${prompt}`));
  };
  handle.setResponses([reply]);
  models.setProvider(handle.provider);
};

const pickModel = async (models) => {
  const wanted = process.env.AGENT_MODEL?.trim();
  if (wanted?.includes("/")) {
    const at = wanted.indexOf("/");
    const provider = wanted.slice(0, at);
    const modelId = wanted.slice(at + 1);
    if (models.getModel(provider, modelId)) return { model: { provider, modelId }, faux: false };
  }
  const available = await models.getAvailable();
  if (available.length > 0) return { model: { provider: available[0].provider, modelId: available[0].id }, faux: false };
  return { model: { provider: "faux", modelId: "echo" }, faux: true };
};

// ── 开机 ─────────────────────────────────────────────────────────────────

const models = builtinModels();
const { model: defaultModel, faux } = await pickModel(models);
if (faux) installFaux(models);

const storage = await openNodeSqliteStorage(path.join(DATA, "agent.sqlite"));
const registry = createRegistry();
registry.install(CodingTools);

const harness = await Harness.open(
  storage,
  {
    models,
    registry,
    env: ({ cwd }) => new NodeExecutionEnv({ cwd: cwd ?? HERE }),
    settings: { extensions: [CodingTools] },
    // 新会话一出生就带一张自己的名字卡片，省得后面到处判空。
    conversationCreated: async (tx, conversation) => {
      const doc = await tx.doc(Sessions);
      doc.names[key(conversation.id)] = `对话 ${conversation.id}`;
    },
    onReport: (error, info) => console.error("[durable]", info?.kind ?? "", String(error)),
  },
  ctx,
);

const names = async () => (await harness.snapshot(Sessions, ctx))?.names ?? {};

const setName = (conversationId, name) =>
  harness.commit(async (tx) => {
    const doc = await tx.doc(Sessions);
    doc.names[key(conversationId)] = name;
  }, ctx);

/** 每个会话在忙什么：inspect() 一次拿全，别按会话一个个问。 */
const busyMap = async () => {
  const inspection = await harness.inspect(ctx);
  const busy = new Map();
  for (const submission of inspection.submissions) busy.set(key(submission.conversationId), "queued");
  for (const task of inspection.tasks) {
    const id = task.record.conversationId;
    if (id !== undefined) busy.set(key(id), task.state.kind);
  }
  return busy;
};

const tree = async () => {
  const page = await harness.commit((tx) => tx.scanConversations({}, 200, undefined), ctx);
  const [namesNow, busy] = await Promise.all([names(), busyMap()]);
  const nodes = [];
  for (const record of page.items) {
    const conversation = await harness.conversation(record.id, ctx);
    if (!conversation) continue;
    const last = await conversation.entries({}, 1, undefined, ctx);
    const agent = await conversation.agent(ctx);
    nodes.push({
      id: record.id,
      name: namesNow[key(record.id)] ?? `对话 ${record.id}`,
      parent: record.parent ? { id: record.parent.conversationId, at: record.parent.at } : null,
      owner: record.owner ? record.owner.taskId : null,
      model: agent.model ? `${agent.model.provider}/${agent.model.modelId}` : null,
      readOnly: !agent.tools?.some?.((tool) => WRITE_NAMES.includes(tool.name)),
      busy: busy.get(key(record.id)) ?? null,
      last: last.items[0] ? { id: last.items[0].id, kind: last.items[0].kind, text: textOfEntry(last.items[0]) } : null,
    });
  }
  return nodes;
};

// ── HTTP ─────────────────────────────────────────────────────────────────

const json = (res, status, body) => {
  const text = JSON.stringify(body);
  res.writeHead(status, { "content-type": "application/json; charset=utf-8", "content-length": Buffer.byteLength(text) });
  res.end(text);
};

const readBody = (req) =>
  new Promise((resolve, reject) => {
    let raw = "";
    req.on("data", (chunk) => {
      raw += chunk;
      if (raw.length > 1_000_000) reject(new Error("body too large"));
    });
    req.on("end", () => resolve(raw));
    req.on("error", reject);
  });

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const device = async () => ({
  self: process.env.NOUS_SELF ?? "/",
  origin: process.env.NOUS_ORIGIN ?? null,
  node: process.version,
  startedAt,
  model: defaultModel,
  faux,
  data: DATA,
  available: (await models.getAvailable()).slice(0, 60).map((m) => `${m.provider}/${m.id}`),
});

const stripPrefix = (parts) => (parts[0] === "p" && parts.length >= 3 ? parts.slice(3) : parts);

/** /v1/sessions/:id/<rest> */
const sessionRoute = (parts) => {
  if (parts[0] !== "v1" || parts[1] !== "sessions" || !parts[2]) return null;
  return { id: Number(parts[2]), rest: parts[3] ?? "", tail: parts.slice(4) };
};

const route = async (req, res, url) => {
  const parts = stripPrefix(url.pathname.split("/").filter(Boolean));

  if (req.method === "GET" && parts.length === 0) {
    res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    return res.end(await readFile(new URL("./index.html", import.meta.url), "utf8"));
  }

  if (req.method === "GET" && parts[0] === "healthz") return json(res, 200, { ok: true });
  if (req.method === "GET" && parts[0] === "v1" && parts[1] === "device") return json(res, 200, await device());

  if (req.method === "GET" && parts[0] === "v1" && parts[1] === "sessions" && !parts[2]) {
    return json(res, 200, { items: await tree() });
  }

  if (req.method === "POST" && parts[0] === "v1" && parts[1] === "sessions" && !parts[2]) {
    const body = JSON.parse((await readBody(req)) || "{}");
    const created = await harness.createConversation(
      { ownership: { kind: "ownerless" }, agent: { cwd: HERE, model: body.model ?? defaultModel } },
      ctx,
    );
    if (body.name) await setName(created.id, String(body.name).slice(0, 120));
    return json(res, 201, { id: created.id, name: (await names())[key(created.id)] });
  }

  const session = sessionRoute(parts);
  if (session) {
    const { id, rest } = session;
    if (!Number.isFinite(id)) return json(res, 400, { error: "会话 id 得是数字" });
    const conversation = await harness.conversation(id, ctx);
    if (!conversation) return json(res, 404, { error: "没有这个会话", id });

    if (req.method === "POST" && rest === "rename") {
      const body = JSON.parse((await readBody(req)) || "{}");
      const name = String(body.name ?? "").trim().slice(0, 120);
      if (!name) return json(res, 400, { error: "name 是空的" });
      await setName(id, name);
      return json(res, 200, { id, name });
    }

    if (req.method === "POST" && rest === "config") {
      const body = JSON.parse((await readBody(req)) || "{}");
      if (typeof body.readOnly === "boolean") {
        // 只读不是「拦住」，是根本不把会写的工具递给模型。
        // 走 modelTools（按名字过滤），不走 tools.remove（那个要的是工具对象）。
        await conversation.configure({ modelTools: body.readOnly ? { remove: WRITE_TOOLS } : null }, ctx);
      }
      return json(res, 200, { id, readOnly: body.readOnly ?? null });
    }

    if (req.method === "POST" && rest === "fork") {
      const body = JSON.parse((await readBody(req)) || "{}");
      let at = body.entryId;
      if (!at) {
        const last = await conversation.entries({}, 1, undefined, ctx);
        at = last.items[0]?.id;
        if (!at) return json(res, 400, { error: "这个会话还没有可分叉的内容" });
      }
      const forked = await conversation.fork(Number(at), { ownership: { kind: "ownerless" } }, ctx);
      if (body.name) await setName(forked.id, String(body.name).slice(0, 120));
      return json(res, 201, { id: forked.id, at });
    }

    if (req.method === "GET" && rest === "entries") {
      const limit = Math.min(Number(url.searchParams.get("limit") ?? 200) || 200, 1000);
      const page = await conversation.entries({}, limit, undefined, ctx);
      return json(res, 200, {
        items: page.items
          // system 那条是工具目录和提示词，一整屏噪音，界面上没收它的位置。
          .filter((entry) => entry.kind !== "pi.system")
          .map((entry) => ({ id: entry.id, kind: entry.kind, text: textOfEntry(entry), parent: entry.head ?? null })),
        next: page.next ?? null,
      });
    }

    if (req.method === "POST" && rest === "messages") {
      const body = JSON.parse((await readBody(req)) || "{}");
      const content = String(body.content ?? "").trim();
      if (!content) return json(res, 400, { error: "content 是空的" });

      // requestId 是幂等的锚：重连重发同一句，拿到的是同一次提交，不会答两遍。
      const submission = await conversation.submit(
        {
          type: "input",
          content,
          ...(body.requestId ? { requestId: String(body.requestId).slice(0, 128) } : {}),
          ...(body.whenBusy ? { whenBusy: body.whenBusy } : {}),
        },
        ctx,
      );

      const waitMs = Math.min(Number(body.waitMs ?? 0) || 0, 300_000);
      if (!waitMs) return json(res, 202, { submissionId: submission.id, status: "queued" });
      const settled = await Promise.race([submission.wait(ctx), sleep(waitMs).then(() => null)]);
      if (!settled) return json(res, 202, { submissionId: submission.id, status: "running" });
      if (settled.status === "done") {
        const entry = await conversation.commit((tx) => tx.entry(AssistantEntry, settled.answer), ctx);
        return json(res, 200, {
          submissionId: submission.id,
          status: "done",
          answer: { entryId: settled.answer, text: textOfEntry(entry) },
        });
      }
      return json(res, 200, {
        submissionId: submission.id,
        status: "unanswered",
        reason: settled.reason,
        detail: settled.detail ?? null,
      });
    }

    if (req.method === "POST" && rest === "abort") {
      await conversation.abort(ctx);
      return json(res, 200, { status: "aborted" });
    }

    if (req.method === "GET" && rest === "events") {
      res.writeHead(200, {
        "content-type": "text/event-stream; charset=utf-8",
        "cache-control": "no-cache, no-transform",
        connection: "keep-alive",
        "x-accel-buffering": "no",
      });
      const send = (event) => res.write(`data: ${JSON.stringify(event)}\n\n`);
      const stream = await watchEvents(harness, id, ctx);
      send(stream.snapshot);
      stream.start(async (events) => {
        for (const event of events) send(event);
      });
      const stop = () => stream.stop().catch(() => {});
      req.on("close", stop);
      res.on("close", stop);
      return;
    }

    return json(res, 404, { error: "这个会话没有这条路", rest });
  }

  json(res, 404, { error: "没有这条路", path: url.pathname });
};

const server = createServer((req, res) => {
  route(req, res, new URL(req.url ?? "/", "http://localhost")).catch((error) => {
    console.error("[qrd3wh]", error);
    if (!res.headersSent) json(res, 500, { error: String(error?.message ?? error) });
    else res.end();
  });
});

server.listen(PORT, () => {
  console.log(`[qrd3wh] 听 ${PORT} · 模型 ${defaultModel.provider}/${defaultModel.modelId}${faux ? "（假 provider）" : ""} · 数据 ${DATA}`);
});

const shutdown = async () => {
  server.close();
  await harness.close(ctx).catch(() => {});
  process.exit(0);
};
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);

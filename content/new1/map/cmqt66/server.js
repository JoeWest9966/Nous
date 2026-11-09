// 这个块的 agent 后端。
//
// 宿主给四个环境变量：PORT / NOUS_DATA / NOUS_ORIGIN / NOUS_SELF。
// 会话、transcript、每一次工具调用都在 pi-durable 的 sqlite 里 —— 进程被杀，重开还是那一轮。
//
//   GET  /healthz                        活着吗
//   GET  /v1/device                      我是谁、有什么模型
//   GET  /v1/entries?limit=              存下来的历史
//   POST /v1/messages {content, waitMs?} 发一句话（waitMs 就长等到有答案）
//   GET  /v1/events                      SSE：增量文本、工具调用、快照
//   POST /v1/abort                       停掉手头的活
//   GET  /                               一个能说话的页面
//
// 页面里所有地址都是相对的：宿主把 /p/<board>/<id>/ 反代进来，写绝对路径就逃出前缀了。

import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { builtinModels } from "@earendil-works/pi-ai/providers/all";
import { fauxAssistantMessage, fauxProvider, fauxText, fauxToolCall } from "@earendil-works/pi-ai/providers/faux";
import { contentText } from "@earendil-works/pi-ai/utils/text";
import { AssistantEntry, Harness, createRegistry, watchEvents } from "@earendil-works/pi-durable";
import { CodingTools } from "@earendil-works/pi-durable/tools";
import { NodeExecutionEnv } from "@earendil-works/pi-durable/env/node";
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";

const ctx = BACKGROUND_CONTEXT;
const HERE = path.dirname(fileURLToPath(import.meta.url));
const PORT = Number(process.env.PORT ?? 4780);
const DATA = process.env.NOUS_DATA ?? path.join(HERE, "data");
const startedAt = Date.now();

const textOf = (content) => contentText(content ?? "").trim();
const textOfEntry = (entry) => {
  if (!entry?.model?.length) return null;
  return entry.model.map((m) => (typeof m.content === "string" ? m.content : contentText(m.content))).join("").trim() || null;
};

// ── 模型：有 key 就用真的，没有就假 provider ──────────────────────────────
// 假的那套能跑完整条链路（生成 → 工具调用 → 结果 → 收尾），不联网也不烧钱。
// 发 "!ls -la" 会真的让 bash 工具跑一次。

const installFaux = (models) => {
  const handle = fauxProvider({
    provider: "faux",
    models: [{ id: "echo", name: "Faux Echo", contextWindow: 128_000, maxTokens: 8_192 }],
    tokensPerSecond: 400,
  });
  const reply = (transcript) => {
    handle.setResponses([reply]); // 让下一次请求也有话可说
    const messages = transcript.messages ?? [];
    const last = messages.at(-1);
    if (last?.role === "toolResult") {
      return fauxAssistantMessage(fauxText(`工具跑完了：\n${textOf(last.content).slice(0, 800)}`));
    }
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
  const wanted = process.env.AGENT_MODEL?.trim(); // "provider/modelId"
  if (wanted?.includes("/")) {
    const at = wanted.indexOf("/");
    const provider = wanted.slice(0, at);
    const modelId = wanted.slice(at + 1);
    if (models.getModel(provider, modelId)) return { model: { provider, modelId }, faux: false };
  }
  const available = await models.getAvailable(); // 只返回配好了凭据的
  if (available.length > 0) return { model: { provider: available[0].provider, modelId: available[0].id }, faux: false };
  return { model: { provider: "faux", modelId: "echo" }, faux: true };
};

// ── 开机 ─────────────────────────────────────────────────────────────────

const models = builtinModels();
const { model, faux } = await pickModel(models);
if (faux) installFaux(models);

const storage = await openNodeSqliteStorage(path.join(DATA, "agent.sqlite"));
const registry = createRegistry();
registry.install(CodingTools); // read / write / edit / bash —— 没有东西自动装，这一步才算装上

const harness = await Harness.open(
  storage,
  {
    models,
    registry,
    // 每次工具调用现建一个环境：文件就落在块自己的文件夹里。
    env: ({ cwd }) => new NodeExecutionEnv({ cwd: cwd ?? HERE }),
    settings: { extensions: [CodingTools] },
    onReport: (error, info) => console.error("[durable]", info?.kind ?? "", String(error)),
  },
  ctx,
);

const root = await harness.root(ctx, { agent: { cwd: HERE, model } });

const answerOf = async (entryId) => {
  if (!entryId) return null;
  return textOfEntry(await root.commit((tx) => tx.entry(AssistantEntry, entryId), ctx));
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
  model,
  faux,
  data: DATA,
  available: (await models.getAvailable()).slice(0, 60).map((m) => `${m.provider}/${m.id}`),
});

// 宿主把 /p/<board>/<id> 剥掉再反代，但独立跑（node server.js）时前缀还在。
const stripPrefix = (parts) =>
  parts[0] === "p" && parts.length >= 3 ? parts.slice(3) : parts;

const route = async (req, res, url) => {
  const parts = stripPrefix(url.pathname.split("/").filter(Boolean));

  if (req.method === "GET" && parts.length === 0) {
    res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    return res.end(await readFile(new URL("./index.html", import.meta.url), "utf8"));
  }

  if (req.method === "GET" && parts[0] === "healthz") return json(res, 200, { ok: true });
  if (req.method === "GET" && parts[0] === "v1" && parts[1] === "device") return json(res, 200, await device());

  if (req.method === "GET" && parts[0] === "v1" && parts[1] === "entries") {
    const limit = Math.min(Number(url.searchParams.get("limit") ?? 100) || 100, 500);
    const page = await root.entries({}, limit, undefined, ctx);
    return json(res, 200, {
      items: page.items.map((e) => ({ id: e.id, kind: e.kind, text: textOfEntry(e), data: e.data ?? null })),
      next: page.next ?? null,
    });
  }

  if (req.method === "POST" && parts[0] === "v1" && parts[1] === "messages") {
    const body = JSON.parse((await readBody(req)) || "{}");
    const content = String(body.content ?? "").trim();
    if (!content) return json(res, 400, { error: "content 是空的" });

    const submission = await root.submit({ type: "input", content }, ctx);
    const waitMs = Math.min(Number(body.waitMs ?? 0) || 0, 300_000);
    if (!waitMs) return json(res, 202, { submissionId: submission.id, status: "queued" });

    // 只等这么久：等不到就照常回 202，活还在后头跑，客户端接着听 SSE。
    const settled = await Promise.race([submission.wait(ctx), sleep(waitMs).then(() => null)]);
    if (!settled) return json(res, 202, { submissionId: submission.id, status: "running" });
    if (settled.status === "done") {
      return json(res, 200, {
        submissionId: submission.id,
        status: "done",
        answer: { entryId: settled.answer, text: await answerOf(settled.answer) },
      });
    }
    return json(res, 200, {
      submissionId: submission.id,
      status: "unanswered",
      reason: settled.reason,
      detail: settled.detail ?? null,
    });
  }

  if (req.method === "POST" && parts[0] === "v1" && parts[1] === "abort") {
    await root.abort(ctx);
    return json(res, 200, { status: "aborted" });
  }

  if (req.method === "GET" && parts[0] === "v1" && parts[1] === "events") {
    res.writeHead(200, {
      "content-type": "text/event-stream; charset=utf-8",
      "cache-control": "no-cache, no-transform",
      connection: "keep-alive",
      "x-accel-buffering": "no",
    });
    const send = (event) => res.write(`data: ${JSON.stringify(event)}\n\n`);
    const stream = await watchEvents(harness, root.id, ctx);
    send(stream.snapshot); // 先给一份现状，晚了才连上的客户端不会看到空屏
    stream.start(async (events) => {
      for (const event of events) send(event);
    });
    const stop = () => stream.stop().catch(() => {});
    req.on("close", stop);
    res.on("close", stop);
    return;
  }

  json(res, 404, { error: "没有这条路", path: url.pathname });
};

const server = createServer((req, res) => {
  route(req, res, new URL(req.url ?? "/", "http://localhost")).catch((error) => {
    console.error("[cmqt66]", error);
    if (!res.headersSent) json(res, 500, { error: String(error?.message ?? error) });
    else res.end();
  });
});

server.listen(PORT, () => {
  console.log(`[cmqt66] 听 ${PORT} · 模型 ${model.provider}/${model.modelId}${faux ? "（假 provider）" : ""} · 数据 ${DATA}`);
});

// 宿主收进程时把会话关干净，不留半截 commit。
const shutdown = async () => {
  server.close();
  await harness.close(ctx).catch(() => {});
  process.exit(0);
};
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);

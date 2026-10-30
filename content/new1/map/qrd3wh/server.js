// qrd3wh —— 会开很多对话的块。
//
// 一个磁盘一个 sqlite，里面 N 个 conversation。会话不是列表，是一棵树：
// 「新对话」在顶层平级长一根，「从这条分叉」在节点下面长子枝。pi-durable 的
// ConversationRecord.parent 就是那根枝，不用自己记。
//
//   GET  /                            那棵树
//   GET  /v1/device                   我是谁、模型
//   GET  /v1/selection                现在盯着哪个对话（别的块挑的也是它）
//   POST /v1/selection                {id} 或 {id:null}
//   GET  /v1/models                   编目里登记过的模型（含档位表、窗口、输出上限）
//   POST /v1/catalog/refresh          重拉编目（换了密钥 / 编目里加了供应商之后用）
//   GET  /v1/sessions                 会话树（含忙闲、模型、思考档、最后一句）
//   POST /v1/sessions                 {name, model, thinkingLevel} → 新对话
//   POST /v1/sessions/:id/rename      {name}
//   POST /v1/sessions/:id/fork        {entryId} 或 {} （空 = 从最后一条分叉）
//   POST /v1/sessions/:id/config      {readOnly, thinkingLevel} 只读 = 不给会写的工具
//   GET  /v1/sessions/:id/entries     历史（新的在前）
//   POST /v1/sessions/:id/messages    {content, requestId?, waitMs?}
//   GET  /v1/sessions/:id/events      SSE
//   POST /v1/sessions/:id/abort
//
// 所有地址都写相对的：宿主把 /p/<board>/<id>/ 反代进来。

import { mkdir, readFile, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { builtinModels } from "@earendil-works/pi-ai/providers/all";
import { clampThinkingLevel, getSupportedThinkingLevels } from "@earendil-works/pi-ai/models";
import { fauxAssistantMessage, fauxProvider, fauxText, fauxToolCall } from "@earendil-works/pi-ai/providers/faux";
import { contentText } from "@earendil-works/pi-ai/utils/text";
import { AssistantEntry, Harness, createRegistry, defineDoc, watchEvents } from "@earendil-works/pi-durable";
import { CodingTools, createBashTool, createEditTool, createWriteTool } from "@earendil-works/pi-durable/tools";
import { installCatalog } from "./catalog.js";
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

/**
 * 思考档位只有一个规律：**档位表是模型的**，不是我们的。
 *
 *   非 reasoning 模型 → 只有 ["off"]（pi-ai 原话：`if (!model.reasoning) return ["off"]`）
 *   reasoning 模型 → 七档里它能映射的那几档，`xhigh` / `max` 还要模型显式写过才算
 *
 * 所以面板能画什么、服务端收什么，都从这一个函数出。判断“能不能思考”不要自己看
 * `model.reasoning` —— 能思考但只映射了 `high` 的模型也一样存在，那还是得看档位表。
 */
const levelsOf = (model) => (model ? getSupportedThinkingLevels(model) : []);

/**
 * 七个档位的全名。照抄 pi-ai 的 `ModelThinkingLevelSchema`（`providers/model-schema.d.ts:8`）——
 * 它只导出 TypeBox schema，没有现成的字符串数组可以 import，所以这一份是手抄的。
 * 漏掉哪个都不会静默出错：不在这张表里的值一律当“没收到”。
 */
const THINKING_LEVELS = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/**
 * 把客户端发来的档位夹回模型真有的那一档。
 *
 * 客户端发的值不可信（可以是任何字符串），这一层不收就落盘，等于把“面板写着 high、
 * 实际跑 medium”请回来。夹完的值才是真相：调用方拿返回值去存、去 echo 回前端。
 *
 * 两种失败分开处理，因为它们的语义不一样：
 *   · 认识的档位但这模型没有（比如给只到 high 的模型发 max）→ **降级**夹到最近的下档；
 *   · 根本不认识的字符串（打错字、旧前端）→ 返回 undefined 当没收到，**不**默认成 off
 *     —— 一个错字就把思考关掉，比不生效更难查。
 */
const fitLevel = (model, level) => {
  if (!model || typeof level !== "string") return undefined;
  if (!THINKING_LEVELS.includes(level)) return undefined;
  const levels = levelsOf(model);
  if (!levels.length) return undefined;
  return clampThinkingLevel(model, level);
};
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

const pickModel = async (models, source) => {
  const wanted = process.env.AGENT_MODEL?.trim();
  if (wanted?.includes("/")) {
    const at = wanted.indexOf("/");
    const provider = wanted.slice(0, at);
    const modelId = wanted.slice(at + 1);
    if (models.getModel(provider, modelId)) return { model: { provider, modelId }, faux: false };
  }
  // 编目里配好密钥的，优先 —— 那才是真能打的。
  for (const provider of source.installed) {
    if (!provider.hasKey) continue;
    const first = models.getModels(provider.id)[0];
    if (first) return { model: { provider: provider.id, modelId: first.id }, faux: false };
  }
  const available = await models.getAvailable();
  if (available.length > 0) return { model: { provider: available[0].provider, modelId: available[0].id }, faux: false };
  return { model: { provider: "faux", modelId: "echo" }, faux: true };
};

// ── 开机 ─────────────────────────────────────────────────────────────────

const models = builtinModels();

/**
 * 编目是唯一的模型来源（NW_BASE 可改地址，NW_TOKEN 走 panel-gate 才要）。
 *
 * **不读 `~/.pi/agent/models.json`**：那是 pi CLI 的配置，跟 Nous 无关。上一版读它，
 * 结果同一台机器上有两份会漂的事实（文件里手挑的 23 个 vs 编目里登记过的 148 个）。
 *
 * 密钥只在内存：启动拉一次，之后 provider 的 resolve 从这张 Map 取。代价是
 * **换密钥要重启本进程** —— 用 POST /v1/catalog/refresh 抵消，不必真重启。
 */
const catalog = await installCatalog(models);
const reloadCatalog = async () => {
  const next = await installCatalog(models, { keys: catalog.keys });
  catalog.installed = next.installed;
  catalog.skipped = next.skipped;
  catalog.at = next.at;
  catalog.error = next.error;
  catalog.base = next.base;
  for (const skip of next.skipped) console.warn(`[qrd3wh] 跳过 ${skip.id}：${skip.reason}`);
  return next;
};
if (catalog.error) console.error(`[qrd3wh] 编目：${catalog.error}（地址 ${catalog.base}）`);
else console.log(`[qrd3wh] 编目装了 ${catalog.installed.length} 个供应商：${catalog.installed.map((p) => `${p.id}(${p.models}${p.hasKey ? "" : ",无密钥"})`).join(" ")}`);
for (const skip of catalog.skipped) console.warn(`[qrd3wh] 跳过 ${skip.id}：${skip.reason}`);

const picked = await pickModel(models, catalog);
if (picked.faux) installFaux(models);

// 默认模型是这块的状态，不是代码：放 data/ 里，跟会话一起被拷走。
const DEFAULT_FILE = path.join(DATA, "default-model.json");
const readDefault = async () => {
  try {
    const saved = JSON.parse(await readFile(DEFAULT_FILE, "utf8"));
    if (saved?.provider && saved?.modelId && models.getModel(saved.provider, saved.modelId)) return saved;
  } catch {}
  return null;
};
let defaultModel = (await readDefault()) ?? picked.model;
const faux = picked.faux && defaultModel.provider === "faux";
const setDefault = async (next) => {
  defaultModel = next;
  await mkdir(DATA, { recursive: true });
  await writeFile(DEFAULT_FILE, JSON.stringify(next, null, 2) + "\n");
};

// 「现在盯着哪个对话」不是会话本身的状态，也不进 tree.json —— 它是这一盘的眼睛。
// pxbgng 那个列表挑的也是它，所以两边改的是同一个值，各自轮询着跟过来。
const SELECTION_FILE = path.join(DATA, "selection.json");
const readSelection = async () => {
  try {
    const saved = JSON.parse(await readFile(SELECTION_FILE, "utf8"));
    if (saved && (saved.id === null || Number.isFinite(saved.id))) return { id: saved.id, at: Number(saved.at) || 0 };
  } catch {}
  return { id: null, at: 0 };
};
let selection = await readSelection();
const setSelection = async (id) => {
  selection = { id, at: Date.now() };
  await mkdir(DATA, { recursive: true });
  await writeFile(SELECTION_FILE, JSON.stringify(selection, null, 2) + "\n");
};

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
    // agent.model 是 ModelRef（俩字符串），不是模型本体 —— 要查档位表得自己 getModel 一次。
    // 直接丟给 levelsOf 会得到 ["off"]：ModelRef 上没有 reasoning 这个字段。
    const model = agent.model ? models.getModel(agent.model.provider, agent.model.modelId) : undefined;
    nodes.push({
      id: record.id,
      name: namesNow[key(record.id)] ?? `对话 ${record.id}`,
      parent: record.parent ? { id: record.parent.conversationId, at: record.parent.at } : null,
      owner: record.owner ? record.owner.taskId : null,
      model: agent.model ? `${agent.model.provider}/${agent.model.modelId}` : null,
      // 生效的档位（不是“发上去的那个值”）：agent 已经过模型档位表和缺省值。
      // 面板该拿这个去画，所以列表里就得有它 —— 少一个字段，前端就只能自己猜。
      thinkingLevel: agent.thinkingLevel ?? null,
      thinkingLevels: levelsOf(model),
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
  catalog: { base: catalog.base, at: catalog.at, error: catalog.error, providers: catalog.installed.length },
  available: (await models.getAvailable()).slice(0, 60).map((m) => `${m.provider}/${m.id}`),
});

const stripPrefix = (parts) => (parts[0] === "p" && parts.length >= 3 ? parts.slice(3) : parts);

/** 页面带的静态文件：样式表一份，字体一个目录。别的一律 404。 */
const assetType = (path) => {
  if (path === "pocket.css") return "text/css; charset=utf-8";
  if (/^fonts\/[A-Za-z0-9._-]+\.woff2$/.test(path)) return "font/woff2";
  return null;
};

/** /v1/sessions/:id/<rest> */
const sessionRoute = (parts) => {
  if (parts[0] !== "v1" || parts[1] !== "sessions" || !parts[2]) return null;
  return { id: Number(parts[2]), rest: parts[3] ?? "", tail: parts.slice(4) };
};

const route = async (req, res, url) => {
  const parts = stripPrefix(url.pathname.split("/").filter(Boolean));

  if (req.method === "GET" && parts.length === 0) {
    // no-store：没有校验头的 HTML 会被浏览器启发式缓存，改完页面刷不出来。
    res.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" });
    return res.end(await readFile(new URL("./index.html", import.meta.url), "utf8"));
  }

  // 页面自己的样式和字体。白名单，不是文件服务器。
  const asset = req.method === "GET" ? assetType(parts.join("/")) : null;
  if (asset) {
    res.writeHead(200, { "content-type": asset, "cache-control": "no-cache" });
    return res.end(await readFile(new URL(`./${parts.join("/")}`, import.meta.url)));
  }

  if (req.method === "GET" && parts[0] === "healthz") return json(res, 200, { ok: true });
  if (req.method === "GET" && parts[0] === "v1" && parts[1] === "device") return json(res, 200, await device());

  if (req.method === "GET" && parts[0] === "v1" && parts[1] === "models") {
    // 只报「这台机器真能打的」：models.json 里配了的 + 环境里配好凭据的。
    // 内置目录有一千多个静态条目，全列出来选择框就没法用了。
    const list = [];
    const seen = new Set();
    const add = (m) => {
      const key = `${m.provider}/${m.id}`;
      if (seen.has(key)) return;
      seen.add(key);
      // thinkingLevels 必须跟着模型一起报：面板只能画这个模型真有的档，
      // 否则会出现“点得下去、服务端静悄悄夹成别的档” —— 那正是要防的那种谎。
      list.push({
        provider: m.provider,
        modelId: m.id,
        name: m.name ?? m.id,
        thinkingLevels: levelsOf(m),
        contextWindow: m.contextWindow ?? null,
        maxTokens: m.maxTokens ?? null,
      });
    };
    for (const provider of catalog.installed) for (const m of models.getModels(provider.id)) add(m);
    for (const m of await models.getAvailable()) add(m);
    // 编目通不通必须跟着列表一起报：只回 items 的话，「编目挂了」在面板上长得跟
    // 「编目里没模型」一模一样，而这两种要干的事完全不同（一个去修网络，一个去登记）。
    return json(res, 200, {
      default: defaultModel,
      items: list,
      catalog: {
        base: catalog.base,
        at: catalog.at,
        error: catalog.error,
        providers: catalog.installed.map((p) => ({ id: p.id, models: p.models, hasKey: p.hasKey })),
        skipped: catalog.skipped,
      },
    });
  }

  if (req.method === "POST" && parts[0] === "v1" && parts[1] === "catalog" && parts[2] === "refresh") {
    const next = await reloadCatalog();
    return json(res, 200, {
      ok: !next.error,
      at: next.at,
      error: next.error,
      providers: next.installed.map((p) => ({ id: p.id, models: p.models, hasKey: p.hasKey })),
      skipped: next.skipped,
    });
  }

  if (req.method === "PUT" && parts[0] === "v1" && parts[1] === "default") {
    const body = JSON.parse((await readBody(req)) || "{}");
    const provider = String(body.provider ?? "");
    const modelId = String(body.modelId ?? "");
    if (!models.getModel(provider, modelId)) return json(res, 400, { error: `没有这个模型：${provider}/${modelId}` });
    await setDefault({ provider, modelId });
    return json(res, 200, { default: defaultModel });
  }

  if (parts[0] === "v1" && parts[1] === "selection") {
    if (req.method === "GET") {
      // 那个对话被删了就当没选过：别让一条死指针把两边的页面都卡在 404 上。
      if (selection.id !== null && !(await harness.conversation(selection.id, ctx))) {
        selection = { id: null, at: selection.at };
      }
      return json(res, 200, selection);
    }
    if (req.method === "POST") {
      const body = JSON.parse((await readBody(req)) || "{}");
      if (body.id === null) {
        await setSelection(null);
        return json(res, 200, selection);
      }
      const id = Number(body.id);
      if (!Number.isFinite(id)) return json(res, 400, { error: "id 得是会话号，或者 null" });
      if (!(await harness.conversation(id, ctx))) return json(res, 404, { error: "没有这个会话", id });
      await setSelection(id);
      return json(res, 200, selection);
    }
    return json(res, 405, { error: "selection 只有 GET 和 POST" });
  }

  if (req.method === "GET" && parts[0] === "v1" && parts[1] === "sessions" && !parts[2]) {
    return json(res, 200, { items: await tree() });
  }

  if (req.method === "POST" && parts[0] === "v1" && parts[1] === "sessions" && !parts[2]) {
    const body = JSON.parse((await readBody(req)) || "{}");
    const model = body.model ?? defaultModel;
    // 档位在建会话的**同一个 commit** 里落定（AgentChange 的 thinkingLevel），
    // 不是先建再 configure —— 后者中间那一下，第一条消息可能已经跑出去了。
    const thinkingLevel = fitLevel(models.getModel(model.provider, model.modelId), body.thinkingLevel);
    const created = await harness.createConversation(
      {
        ownership: { kind: "ownerless" },
        agent: { cwd: HERE, model, ...(thinkingLevel ? { thinkingLevel } : {}) },
      },
      ctx,
    );
    if (body.name) await setName(created.id, String(body.name).slice(0, 120));
    const agent = await created.agent(ctx);
    return json(res, 201, {
      id: created.id,
      name: (await names())[key(created.id)],
      model: agent.model ? `${agent.model.provider}/${agent.model.modelId}` : null,
      thinkingLevel: agent.thinkingLevel ?? null,
    });
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
      // 档位改的是**这条对话**，不是全局：configure 落的 AgentState 跟着会话走，
      // 下一条消息就吃到新值（不用重建会话，也就不会丢历史）。
      if (body.thinkingLevel !== undefined) {
        const now = await conversation.agent(ctx);
        const fit = fitLevel(now.model ? models.getModel(now.model.provider, now.model.modelId) : undefined, body.thinkingLevel);
        // 夹不成（模型认不出、或发来的不是字符串）就当没收到，绝不把没验证的值落盘。
        if (fit) await conversation.configure({ thinkingLevel: fit }, ctx);
      }
      // 回的是**生效值**，不是请求值：前端拿它重画，面板和真相就不会分叉。
      const agent = await conversation.agent(ctx);
      return json(res, 200, {
        id,
        model: agent.model ? `${agent.model.provider}/${agent.model.modelId}` : null,
        thinkingLevel: agent.thinkingLevel ?? null,
        readOnly: !agent.tools?.some?.((tool) => WRITE_NAMES.includes(tool.name)),
      });
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

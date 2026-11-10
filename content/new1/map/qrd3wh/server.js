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
//   GET  /v1/fs/dirs?path=           目录浏览器：空 = start dirs；给路径 = 列它的子目录（?machine= 列那台机器的）
//   GET  /v1/machines                 机器编目（从 vq0f03 读，缓存 60s；?fresh=1 重拉）
//   GET  /v1/sessions                 会话树（含忙闲、模型、思考档、cwd、机器、最后一句、最后写库时刻）
//   GET  /v1/unread                   每个会话的未读数 + 上次读的时刻（平铺的 map，引擎算好）
//   POST /v1/read/:id                 把这个会话标成读到最新
//   POST /v1/sessions                 {name, model, thinkingLevel, skills, template, cwd, machine} → 新对话
//   POST /v1/sessions/:id/rename      {name}
//   POST /v1/sessions/:id/fork        {entryId} 或 {} （空 = 从最后一条分叉）
//   POST /v1/sessions/:id/config      {readOnly, model, thinkingLevel, skills, template, cwd, machine} 只读 = 不给会写的工具
//   POST /v1/sessions/:id/archive     归档 = 复印进 data/archives/gen-YYYYMMDD.sqlite + 写墓碑（ADR 0001）
//   POST /v1/sessions/:id/purge       真删：撕原件（必须先归档过、闲置、无活后代）
//   GET  /v1/archive                  浏览归档库（gens + 每裸子树的会话）
//   POST /v1/archive/:id/restore      {gen} 打捞：原样 INSERT 回活库 + 抹墓碑
//   GET  /v1/skills                   技能仓库全景（repo 状态 + list() + archived + 类别）
//   …                                全套 /v1/skills* 在 skills-api.js（见那个文件的头）
//   GET  /v1/prompts                  模板私库全景（repo 状态 + list() + archived + 类别 + 默认值）
//   …                                全套 /v1/prompts* 在 prompts-api.js（见那个文件的头）
//   GET  /v1/extensions               进程里装着什么 + 仓库 catalog（扩展清单页吃这个）
//   POST /v1/extensions/category      {name, category[]} 改扩展类别：写 nous.json + 重扫 index.json
//   GET  /v1/extensions/categories     类别词表（categories.json）
//   POST /v1/extensions/categories     {op:"add"|"rename"|"remove", name, to?} 建 / 改名 / 删类别（改名和删除都作用于所有扩展）
//   GET  /v1/extensions/sync          仓库脏不脏、领先/落后 origin 几步
//   POST /v1/extensions/sync          只提交词表 / 清单并 push（代码走 publish）
//   GET  /v1/extensions/files?name=   这一个扩展目录里有哪些能编辑的文件
//   GET  /v1/extensions/file?name=&path=  读一个文件
//   PUT  /v1/extensions/file          {name, path, text, base?} 写一个文件（只落盘，不提交）
//   POST /v1/extensions/publish       {message?} 校验 + 重扫 + commit + push
//   POST /v1/extensions/uninstall     {name} 本地删除：源码移进 data/ 垃圾站 + 撤装（不动仓库）
//   POST /v1/extensions/install       {name} 从垃圾站 / HEAD 装回来
//   POST /v1/extensions/create        {name, summary, category} 新建扩展：建目录 + 空骨架（不提交）
//   POST /v1/extensions/archive       {name} 从仓库移除：git mv 到 .archive/ + commit + push
//   POST /v1/extensions/restore       {name} 从 .archive/ 搬回仓库 + commit + push
//   POST /v1/extensions/purge         {name} 真删：git rm -r .archive/<name> + commit + push
//   POST /v1/extensions/reload        立刻按磁盘现状重装 + 重扫（不用等 30 秒轮询）
//   GET  /v1/extensions/sessions/:id/entries     历史（新的在前；?system=1 额外给一份回放后的提示词）
//   POST /v1/sessions/:id/messages    {content, requestId?, waitMs?}
//   GET  /v1/sessions/:id/events      SSE
//   POST /v1/sessions/:id/abort
//
// 所有地址都写相对的：宿主把 /p/<board>/<id>/ 反代进来。

import { cp, mkdir, readFile, readdir, realpath, rename, rm, stat, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { builtinModels } from "@earendil-works/pi-ai/providers/all";
import { clampThinkingLevel, getSupportedThinkingLevels } from "@earendil-works/pi-ai/models";
import { fauxAssistantMessage, fauxProvider, fauxText, fauxToolCall } from "@earendil-works/pi-ai/providers/faux";
import { contentText } from "@earendil-works/pi-ai/utils/text";
import { AssistantEntry, Harness, createRegistry, defineDoc, defineExtension, section, watchEvents } from "@earendil-works/pi-durable";
import { CodingTools, createBashTool, createEditTool, createWriteTool } from "@earendil-works/pi-durable/tools";
import { installCatalog } from "./catalog.js";
import { openActivity } from "./activity.js";
import { collectPages } from "./pages.js";
import { openUnread } from "./unread.js";
import { openArchive } from "./archive.js";
// 自研扩展的**源码不在这棵树里** —— 在 Nous-extend 仓库的本地副本（`NOUS_EXTEND`）。
// 块里只留两件事：去哪儿拿（下面这两行）+ 钉着哪个 commit（extensions.lock.json）。
// 用动态 import 是因为路径得能配；静态 import 只能写字面量，一换机器就炸。
const EXTEND = process.env.NOUS_EXTEND ?? "/opt/Nous-extend";
// 技能仓库的本地副本（票 A 建的那个仓）。跟 EXTEND 并排：两个仓都只有一条真理 —— 盘上的文件。
// 这里**不**往 data/ 抄一份：技能不是「装进来的包」，仓库里有什么就是什么。
const SKILLS_ROOT = process.env.NOUS_SKILLS ?? "/opt/Nous-skills";
// 提示词私库的本地副本。跟技能仓并排：一条模板 = `<名>/PROMPT.md` + `nous.json`。
// 同样**不**往 data/ 抄一份：仓库里有什么就是什么。
const PROMPTS_ROOT = process.env.NOUS_PROMPTS ?? "/opt/Nous-prompts";
// 会话预设私库。一条预设 = `<名>/preset.json`（纯配置，引用别的命名空间）。
// **不是扩展**：不进 registry、不建名牌、不进提示词 —— 所以它住在这块里（见 presets.js 头）。
const PRESETS_ROOT = process.env.NOUS_PRESETS ?? "/opt/Nous-Template";
// 不带段名的 `preset` 落到这一段。两段都只认自己的名字 —— 没有「默认段」那种隐式状态。
const PRESET_SESSION_SECTION = "Session-Template";
// 扩展是造扩展的函数，宿主把 api 递进去 —— 它们不自己 import pi-durable（见仓库 README）。
const api = { defineExtension, section };
const { default: createNous } = await import(`${EXTEND}/nous/index.js`);
const { createSkills, skillNamesOf, SKILL_PREFIX } = await import(`${EXTEND}/skills/index.js`);
const { createPrompts, templateOf, PROMPT_PREFIX } = await import(`${EXTEND}/prompts/index.js`);
// 运行时空白名牌的前缀。会话里存的就是这些名字，扩展面板里不该把它们当成「自研扩展」列出来。
const NAMEPLATE_PREFIXES = [SKILL_PREFIX, PROMPT_PREFIX];
// 回放提示词增量 —— 自己在 `prompt-log.js`，纯读，能单验。
import { replayPrompt } from "./prompt-log.js";
// 技能变体：本机绑定 + worktree + 变体分支发布（跟扩展那套同一个形状）。
import { createVariantStore } from "./skills-variants.js";
// 仓库工具（参数化过 root 的那套）：本文件用它管扩展仓，`skills-api.js` 用它管技能仓。
// 抽成独立模块是因为这个文件有顶层 await、会起服务 —— 测试永远不能 import 它。
// 这里既要把常用的一串名字拆出来（本文件到处在用），又要留着整包递进 handleSkills。
import * as repo from "./repo-tools.js";

const {
  ARCHIVE_DIR,
  EXTENSION_NAME_RE,
  PROTECTED_CATEGORIES,
  archivedEntries,
  checkSyntax,
  gitChanges,
  gitRootOf,
  isTrackedDir,
  listRepoFiles,
  moveDir,
  moveIntoArchive,
  moveOutOfArchive,
  mutateCategories,
  normalizeCategories,
  pendingCommit,
  pendingFileDiff,
  pendingState,
  readCategories,
  readRepoFile,
  regenIndex,
  repoBusy,
  repoState,
  run,
  serializeRepoWrite,
  setNodeCategories,
  sortCategories,
  syncRepo,
  withStatus,
  writeCategories,
  writeFileAtomic,
  writeRepoFile,
} = repo;
import { handleSkills } from "./skills-api.js";
import * as upstream from "./upstream.js";
import { handlePrompts } from "./prompts-api.js";
import { handlePresets } from "./presets-api.js";
import { createPresets, expandPresetSpec, presetContext, readSections } from "./presets.js";
import { readTriggerCatalog } from "./triggers.js";
import { NodeExecutionEnv } from "@earendil-works/pi-durable/env/node";
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";
// 多服务器：远端会话的 bash/文件读写走这个 —— ExecutionEnv 接口翻译成 ssh 命令。
import { SshExecutionEnv } from "./ssh-env.js";

const ctx = BACKGROUND_CONTEXT;
const HERE = path.dirname(fileURLToPath(import.meta.url));
const PORT = Number(process.env.PORT ?? 4780);
const DATA = process.env.NOUS_DATA ?? path.join(HERE, "data");
// 目录浏览器的「首页」：NOUS_START_DIRS（冒号分隔），缺省 = 这一盘的 map 根（所有块）。
// 不是沙箱 —— 浏览器有「..」能到根、能手输绝对路径，这只是打开时落在哪。
const START_DIRS = (process.env.NOUS_START_DIRS ?? path.dirname(HERE))
  .split(":")
  .map((p) => p.trim())
  .filter(Boolean);
const HOME = process.env.HOME ?? "";
const startedAt = Date.now();

// ── 机器编目：从 vq0f03 读，不自己读 nw-panel ──────────────────────────────
//
// vq0f03 是「机器编目」那块（id 固定）。它已经有 /api/catalog（机器 + 注解），
// 这里只取 ssh 目标（name/user/port/ips），**不碰密钥** —— 密钥是宿主机 ~/.ssh 的事
// （vq0f03 的 /ssh 端点自己写着「密钥、known_hosts、跳板都不在编目里」）。
//
// 地址两个来源：默认走宿主 `__nous/ctx` 找 vq0f03；`NOUS_VPS_BASE` 可以指死
// （测试 / 远程部署时用，跟 NW_BASE 一个道理）。
const MACHINES_BLOCK_ID = "vq0f03";
const MACHINES_TTL_MS = 60_000;
let machinesState = { at: 0, machines: [], error: null };

/** vq0f03 的地址：宿主 `__nous/ctx` 现找；`NOUS_VPS_BASE` 可以指死（测试 / 远程部署时用）。 */
const vpsBase = async () => {
  if (process.env.NOUS_VPS_BASE) return String(process.env.NOUS_VPS_BASE).replace(/\/+$/, "");
  const origin = process.env.NOUS_ORIGIN ?? "http://127.0.0.1";
  const self = process.env.NOUS_SELF ?? "/";
  const ctxUrl = `${origin}${self.endsWith("/") ? self : self + "/"}__nous/ctx`;
  const ctxRes = await fetch(ctxUrl);
  if (!ctxRes.ok) throw new Error(`__nous/ctx ${ctxRes.status}`);
  const ctxDoc = await ctxRes.json();
  const blocks = Array.isArray(ctxDoc.blocks)
    ? ctxDoc.blocks
    : Object.entries(ctxDoc.blocks ?? {}).map(([id, b]) => ({ id, ...(b ?? {}) }));
  const vps = blocks.find((b) => b?.id === MACHINES_BLOCK_ID);
  if (!vps?.url) throw new Error(`__nous/ctx 里没有 ${MACHINES_BLOCK_ID}`);
  return String(vps.url).replace(/\/+$/, "");
};

const fetchMachineCatalog = async () => {
  const base = await vpsBase();
  const res = await fetch(`${base}/api/catalog`);
  if (!res.ok) throw new Error(`vq0f03 编目 ${res.status}`);
  const body = await res.json();
  const raw = body?.data?.machines ?? body?.machines ?? [];
  return raw
    .filter((m) => m?.name)
    .map((m) => {
      const ips = (m.ips ?? []).map((i) => ({ addr: String(i?.addr ?? ""), label: String(i?.label ?? "") })).filter((i) => i.addr);
      return {
        name: String(m.name),
        user: m.user ?? "root",
        port: Number(m.port) || 22,
        ips,
        addr: ips[0]?.addr ?? "",
        note: m.note ?? "",
        tags: m.tags ?? [],
      };
    });
};

/** 读编目（默认走缓存；`fresh` 强制重拉）。挂掉时回上一份 + error，不把旧清单扔掉。 */
const machines = async (fresh = false) => {
  const now = Date.now();
  if (!fresh && machinesState.at && now - machinesState.at < MACHINES_TTL_MS) return machinesState;
  try {
    machinesState = { at: now, machines: await fetchMachineCatalog(), error: null };
  } catch (error) {
    machinesState = { at: now, machines: machinesState.machines, error: String(error?.message ?? error) };
  }
  return machinesState;
};

const resolveMachine = async (name) => {
  const { machines: list } = await machines();
  return list.find((m) => m.name === name) ?? null;
};

// ── 凭据：只在内存里过一手 ────────────────────────────────────────────────
// 从 vq0f03 的 `/api/vps/<名>/secret` 取（那条只在**回环面板**上有，远程门默认 404）。
// 回来的是明文 PEM：**不落块的文件夹、不进日志、不进审计**，只有 ssh 要的 0600 临时文件。
// 密码型（kind=password）一律当「没有密钥」—— 得先在目标机装好公钥，见 AGENTS.md。
const SECRET_TTL_MS = 60_000;
const secrets = new Map(); // name -> { at, key }

const machineSecret = async (name, { fresh = false } = {}) => {
  const hit = secrets.get(name);
  if (!fresh && hit && Date.now() - hit.at < SECRET_TTL_MS) return hit.key;
  let base;
  try {
    base = await vpsBase();
  } catch (error) {
    console.error(`[machine] 拿不到 vq0f03 的地址：${String(error?.message ?? error)}`);
    return null;
  }
  const res = await fetch(`${base}/api/vps/${encodeURIComponent(name)}/secret`);
  const body = await res.json().catch(() => null);
  const key = body?.data?.key;
  if (!key) {
    // 密码型 / 上游挂了：都不是错，只是这台机器用不了远端。凭据一律不回显。
    console.error(`[machine] ${name} 拿不到密钥（status=${res.status} kind=${body?.data?.kind ?? "?"}）`);
    secrets.delete(name);
    return null;
  }
  secrets.set(name, { at: Date.now(), key });
  return key;
};

/** 给一台机器造 ExecutionEnv。拿不到密钥就报错 —— 静默跑在本机比报错难查得多。 */
const envForMachine = async (machine, cwd) => {
  const secretKey = await machineSecret(machine.name);
  if (!secretKey) throw withStatus(new Error(`「${machine.name}」的凭据里没有可用的密钥（密码型还不支持，先给那台机器装公钥）`), 400);
  return new SshExecutionEnv({ cwd: cwd ?? "~", host: machine, key: secretKey });
};

// ── 本地装卸：这一台的私事，跟仓库没关系 ───────────────────────────────────
//
// 状态全放 data/（宿主的 watcher 跳过 data/，写它不会把进程重启；也不在
// Nous-extend 工作区里，所以不会脏到 git）。两个真相：
//   · extensions.local.json —— 「这台不装谁」是**唯一真值**，不靠「目录在不在」。
//     别人一 push、git pull 一跑，文件可能自己回来；没有这份名单扩展就悄悄复活了。
//   · extend-trash/<name>-<时间>/ —— 删掉的源码本体，点安装就搬回来。
//
// 仓库（HEAD / origin）一个字节都不动：删除只表现为工作区里的 ` D`，而 publish
// 会把删除过滤掉（见 repo-tools.js 的 syncRepo），绝不会把「本地删除」顺手推上 GitHub。
const CORE_EXTENSIONS = ["nous", "skills", "prompts"]; // 引擎启动路径硬编码 import，删了就起不来
const LOCAL_STATE_FILE = path.join(DATA, "extensions.local.json");
const TRASH_DIR = path.join(DATA, "extend-trash");

const emptyLocalState = () => ({
  _about: ["本机装卸清单：哪些扩展这台不装。不在 Nous-extend 仓库里，也不该被提交。"],
  deleted: {},
  // 变体的本机绑定：`{ 扩展名: { 变体 id: { dir, branch } } }`。
  // 仓库里那份 variants 只带参数（跟着代码走）；**指向某个 checkout 的绑定只在这台有意义**，
  // 写进 nous.json 就等于把 /opt/... 这种本机路径提交上 GitHub。
  variants: {},
});

const readLocalState = async () => {
  try {
    const raw = JSON.parse(await readFile(LOCAL_STATE_FILE, "utf8"));
    return {
      ...emptyLocalState(),
      ...raw,
      deleted: raw?.deleted && typeof raw.deleted === "object" && !Array.isArray(raw.deleted) ? raw.deleted : {},
      variants: raw?.variants && typeof raw.variants === "object" && !Array.isArray(raw.variants) ? raw.variants : {},
    };
  } catch {
    return emptyLocalState();
  }
};

const writeLocalState = async (value) => {
  await mkdir(DATA, { recursive: true });
  await writeFileAtomic(LOCAL_STATE_FILE, JSON.stringify(value, null, 2) + "\n");
};

/**
 * 跑生成器，再把「本地删了、仓库里还有」的清单补回 index.json。
 * index.mjs 只认工作区里的目录，删掉的扩展会被它扫掉；可仓库（HEAD）里文件还在，
 * 生成物不跟着列出来就跟仓库自相矛盾，而且下一次类别 debounce 会把这份残缺推上去。
 */
const runIndexGenerator = async () => {
  await run(process.execPath, ["index.mjs"], { cwd: EXTEND, timeout: 30_000 });
  const { deleted } = await readLocalState();
  if (!Object.keys(deleted).length) return;
  const file = path.join(EXTEND, "index.json");
  const doc = JSON.parse(await readFile(file, "utf8"));
  const list = Array.isArray(doc.extensions) ? doc.extensions : [];
  // 只补「HEAD 里真有这个目录」的。未发布的新扩展 HEAD 里没有，把它写进生成物
  // 等于让仓库的 index.json 指向一份不存在的文件 —— 页面那张灰卡照样从
  // extensions.local.json 里长出来（见 readExtendCatalog），不靠生成物。
  let tracked = [];
  try {
    tracked = (await run("git", ["-C", EXTEND, "ls-tree", "--name-only", "HEAD"], { timeout: 15_000 })).stdout
      .split("\n").map((line) => line.trim()).filter(Boolean);
  } catch {}
  for (const [name, record] of Object.entries(deleted)) {
    if (!tracked.includes(name)) continue;
    // 已经归档的不补：`git mv` 到 .archive/ 之后、commit 落地之前，HEAD 里还挂着
    // 这个目录 —— 这段时间把清单写回去，就成了一份「清单说它在、目录已归档」的矛盾账。
    if (await stat(path.join(EXTEND, ARCHIVE_DIR, name)).catch(() => null)) continue;
    if (record?.manifest && !list.some((item) => item.name === name)) list.push(record.manifest);
  }
  list.sort((a, b) => String(a.name).localeCompare(String(b.name)));
  await writeFileAtomic(file, JSON.stringify({ ...doc, extensions: list }, null, 2) + "\n");
};

/** index.json 是清单；本地删过的也一起给（前端要摆灰卡）。 */
const readExtendCatalog = async () => {
  let list = [];
  try {
    const doc = JSON.parse(await readFile(path.join(EXTEND, "index.json"), "utf8"));
    list = Array.isArray(doc.extensions) ? doc.extensions : [];
  } catch {
    // 清单坏了 / 没生成：别让整个 GET 500，灰卡还能从本地名单里长出来。
  }
  const { deleted } = await readLocalState();
  for (const [name, record] of Object.entries(deleted)) {
    if (record?.manifest && !list.some((item) => item.name === name)) list.push(record.manifest);
  }
  return list;
};

/**
 * 按清单装载**非核心**扩展：nous / skills 仍是启动路径上两行硬编码 import，
 * 它们不能删、也不该被一个坏清单拖垮。其余每个扩展自己 import + install。
 * 一个扩展写坏了只跳过它，不拖引擎下水。
 */
const extraLoaded = new Map();

// 每个扩展上一次装载的真实结果。**这是页面上唯一能看出「它其实没生效」的地方**：
// 失败了我们不卸旧版本（旧代码还能用），但状态必须变 failed —— 否则页面显示绿卡、
// 版本号是 index.json 里的新版本，而进程里跑的是旧的。
const extensionLoad = new Map();
const noteLoad = (name, state, detail = {}) =>
  extensionLoad.set(name, { state, error: detail.error ?? null, at: Date.now(), version: detail.version ?? null });

/** 错误只留第一行、截断 400 字：卡片上放得下，也够定位。 */
const errorLine = (error) => String(error?.message ?? error).split("\n")[0].slice(0, 400);
// ── 变体：同一个扩展的多个可装载版本 ────────────────────────────────────────
//
// 会话里存的是**名字**（durable 的 `AgentState.extensions` 就是字符串数组），所以
// 「变体」在运行时唯一的形态就是**另一个注册名**：`nous` 之外还有 `nous@review`。
// 名字里用 `@`：`skill:` 是运行时空白名牌的特殊前缀，沾上会被 skillNamesOf 误伤。
//
// 来源两处，语义不同，别混：
//   · `nous.json` 的 `variants` —— 只带**参数**，跟着仓库走，别人的机器上也有；
//   · `extensions.local.json` 的 `variants` —— 指向**本机某个 checkout**（worktree），
//     只在这台存在。它的代码不在这棵树里，所以不参与清单对账（`live` 里没有它）。
//
// 变体不改扩展自己的代码：造法跟本体一模一样，只是额外递一个 `{ variant, ...params }`，
// 再把造出来的对象的 `name` 换成 `名字@变体`（会话靠这个名字挑，扩展不用知道 `@`）。
const VARIANT_SEP = "@";
const VARIANT_ID_RE = /^[a-z0-9][a-z0-9._-]*$/i;
const variantKey = (family, id) => `${family}${VARIANT_SEP}${id}`;
/** worktree 放 EXTEND 旁边（`/opt/nous-ext-nous-review`）：同盘、共用一个 .git、删了不碰生产树。 */
const variantWorktreePath = (family, id) => path.join(path.dirname(EXTEND), `nous-ext-${family}-${id}`);
const variantFamilyOf = (key) => (key.includes(VARIANT_SEP) ? key.slice(0, key.indexOf(VARIANT_SEP)) : key);

/** 一个扩展的全部变体来源：仓库声明的 + 本机绑定的（同 id 以本机为准）。 */
const variantSources = async (family, manifest) => {
  const local = (await readLocalState()).variants?.[family] ?? {};
  const out = new Map();
  for (const [id, spec] of Object.entries(manifest?.variants ?? {})) {
    // 坏 id 静默跳过：一个写错的变体不该拖垮整个扩展。
    if (!VARIANT_ID_RE.test(id) || id.includes(VARIANT_SEP)) continue;
    out.set(id, { id, source: "repo", params: spec?.params ?? {}, dir: null, branch: null });
  }
  for (const [id, spec] of Object.entries(local)) {
    if (!VARIANT_ID_RE.test(id) || id.includes(VARIANT_SEP)) continue;
    out.set(id, {
      id,
      source: "local",
      params: spec?.params ?? {},
      dir: typeof spec?.dir === "string" ? spec.dir : null,
      branch: typeof spec?.branch === "string" ? spec.branch : null,
    });
  }
  return [...out.values()];
};

/** 装一个变体。失败由调用方记 failed —— 变体挂了不能连坐本体。 */
const installVariant = async ({ family, manifest, bag, version, source }) => {
  const key = variantKey(family, source.id);
  const root = source.dir ?? path.join(EXTEND, family);
  // 变体如果自己带清单，就以它为准（分支可以改自己的 file / entry / needs）——
  // 但 name 必须还是本体：名字是会话里的契约，分支不许改。
  let spec = manifest;
  try {
    const own = JSON.parse(await readFile(path.join(root, "nous.json"), "utf8"));
    if (own?.name && own.name !== family) throw new Error(`变体清单的 name 是 ${own.name}，不是 ${family}`);
    spec = { ...manifest, ...own };
  } catch (error) {
    if (String(error?.message ?? error).includes("变体清单的 name")) throw error;
    // 没有清单 / 坏了：用本体那份（老行为）。
  }
  for (const need of Object.keys(spec.needs ?? {})) {
    if (!(need in bag)) throw new Error(`宿主给不了 ${need}`);
  }
  const entry = Array.isArray(spec.entry) ? spec.entry[0] : spec.entry;
  const module = await import(
    `${root}/${spec.file}?v=${encodeURIComponent(version)}-${encodeURIComponent(key)}-${Date.now()}`
  );
  const factory = module[entry];
  if (typeof factory !== "function") throw new Error(`导出 ${entry} 不是函数`);
  const made = await factory(bag, { variant: source.id, ...source.params });
  const extension = made?.extension ?? made;
  if (!extension?.name) throw new Error("造出来的东西没有 name");
  const renamed = { ...extension, name: key };
  registry.install(renamed);
  extraLoaded.set(key, renamed);
  noteLoad(key, "ok", { version: manifest.version });
  if (typeof made?.load === "function") await made.load();
  if (typeof made?.backfill === "function") void made.backfill();
};

/**
 * 核心两条（nous / skills）的变体。**本体在启动路径上硬编码 import**，所以它们的变体
 * 只能在装完本体之后补一刀 —— 不能走 syncExtraExtensions（那里把核心家族整体跳过）。
 *
 * 用函数声明而不是 const：启动序列在文件中间就跑，const 会撞 TDZ。
 */
async function loadCoreVariants(version) {
  const catalog = await readExtendCatalog();
  const bag = { api, defineExtension, section, dir: path.join(DATA, "skills"), promptsDir: PROMPTS_ROOT, registry };
  for (const family of CORE_EXTENSIONS) {
    const manifest = catalog.find((item) => item?.name === family);
    if (!manifest) continue; // 清单坏了：本体照样跑，变体下一轮再说
    const sources = await variantSources(family, manifest);
    const keep = new Set(sources.map((source) => variantKey(family, source.id)));
    for (const [key, extension] of extraLoaded) {
      if (!key.startsWith(family + VARIANT_SEP) || keep.has(key)) continue;
      registry.uninstall(extension);
      extraLoaded.delete(key);
      extensionLoad.delete(key);
    }
    for (const source of sources) {
      try {
        await installVariant({ family, manifest, bag, version, source });
      } catch (error) {
        noteLoad(variantKey(family, source.id), "failed", { error: errorLine(error), version: manifest.version });
        console.error(`[extend] 变体 ${variantKey(family, source.id)} 装载失败：`, errorLine(error));
      }
    }
  }
}

const assertVariantId = (id) => {
  if (!VARIANT_ID_RE.test(id) || id.includes(VARIANT_SEP)) {
    throw withStatus(new Error(`变体 id 只能用字母、数字和 . _ -：${id}`), 400);
  }
  return id;
};

const variantManifestOf = async (family) => {
  if (!EXTENSION_NAME_RE.test(family)) throw withStatus(new Error(`扩展名不合法：${family}`), 400);
  const manifest = (await readExtendCatalog()).find((item) => item?.name === family);
  if (!manifest) throw withStatus(new Error(`没有 ${family} 这个扩展`), 404);
  return manifest;
};

/** 把一个目录绑成变体。这是唯一能把「A 的变体」指到「B 的代码」的地方，所以查得细。 */
const bindVariantDir = async ({ family, manifest, id, dir, branch = null, params = null }) => {
  assertVariantId(id);
  const raw = String(dir ?? "").trim();
  if (!raw) {
    throw withStatus(
      new Error("目录不能为空 —— 填某个扩展的目录（里面有 nous.json 的那个文件夹），或者改用上面的「新开一条分支树」"),
      400,
    );
  }
  // 相对路径相对**仓库根**解析，不是相对进程 cwd —— cwd 是块自己的事，人看不见。
  const root = path.isAbsolute(raw) ? path.resolve(raw) : path.resolve(EXTEND, raw);
  if (root === path.resolve(EXTEND)) {
    // 说清「差在哪一层」：仓库根是装扩展的地方，不是扩展本身。
    throw withStatus(
      new Error(`${EXTEND} 是仓库根，不是某个扩展的目录 —— 得再往下走一层，比如 ${EXTEND}/${family}`),
      400,
    );
  }
  const info = await stat(root).catch(() => null);
  if (!info) throw withStatus(new Error(`目录不存在：${root}`), 400);
  if (!info.isDirectory()) throw withStatus(new Error(`${root} 是个文件，不是目录 —— 要填扩展所在的那个文件夹`), 400);
  let own = null;
  try {
    own = JSON.parse(await readFile(path.join(root, "nous.json"), "utf8"));
  } catch {}
  // 那个目录自己带清单时，name 必须等于本体 —— 指错目录是这里唯一真危险的错误：
  // 装出来的东西叫 A、内容是 B，而且从名字上看不出来。
  if (own?.name && own.name !== family) {
    throw withStatus(new Error(`那个目录的 nous.json 写着 name=${own.name}，不是 ${family}`), 409);
  }
  const file = String(own?.file ?? manifest.file);
  if (!(await stat(path.join(root, file)).catch(() => null))) {
    throw withStatus(
      new Error(`${root} 里没有 ${file} —— 那个目录得是一个扩展的目录（清单 + 入口文件），比如 ${EXTEND}/${family}`),
      400,
    );
  }
  const local = await readLocalState();
  const familyMap = { ...(local.variants?.[family] ?? {}) };
  familyMap[id] = { dir: root, branch: branch ?? null, ...(params && typeof params === "object" ? { params } : {}) };
  local.variants = { ...(local.variants ?? {}), [family]: familyMap };
  await writeLocalState(local);
  return { dir: root, branch: branch ?? null };
};

const unbindVariant = async (family, id) => {
  assertVariantId(id);
  const local = await readLocalState();
  const familyMap = { ...(local.variants?.[family] ?? {}) };
  delete familyMap[id];
  // 最后一个变体解绑后别留一个空壳 `{ "zz-var": {} }`：那是垃圾，而且看不出是「没有变体」还是「没写过」。
  const next = { ...(local.variants ?? {}) };
  if (Object.keys(familyMap).length) next[family] = familyMap;
  else delete next[family];
  local.variants = next;
  await writeLocalState(local);
};

/** 开一条开发分支的 worktree。分支已存在就 checkout 它，不另开一条同名的。 */
const createVariantWorktree = async ({ family, id, branch }) => {
  assertVariantId(id);
  const target = variantWorktreePath(family, id);
  if (await stat(target).catch(() => null)) {
    throw withStatus(new Error(`${target} 已经存在 —— 先用「拆树」，或在命令行处理`), 409);
  }
  const wanted = (typeof branch === "string" && branch.trim()) || `ext/${family}-${id}`;
  if (!/^[A-Za-z0-9._/-]+$/.test(wanted) || wanted.startsWith("-") || wanted.includes("..")) {
    throw withStatus(new Error(`分支名不合法：${wanted}`), 400);
  }
  const exists = await run("git", ["-C", EXTEND, "rev-parse", "--verify", `refs/heads/${wanted}`], { timeout: 15_000 })
    .then(() => true)
    .catch(() => false);
  await run(
    "git",
    ["-C", EXTEND, "worktree", "add", ...(exists ? [] : ["-b", wanted]), target, ...(exists ? [wanted] : [])],
    { timeout: 120_000 },
  );
  // 新树是 HEAD 的完整 checkout，所以变体目录就在 <树>/<扩展名>。
  const familyDir = path.join(EXTEND, family);
  const targetFamily = path.join(target, family);
  if (!(await stat(targetFamily).catch(() => null)) && (await stat(familyDir).catch(() => null))) {
    // 未发布的扩展在分支里**根本不存在**（分支是从 HEAD 起的）：把工作区那份拷过去，
    // 否则「给刚建的扩展开个分支变体」拿到的是一棵空树。
    await cp(familyDir, targetFamily, { recursive: true });
    // 拷进来的东西在那边是 untracked，而 `git worktree remove` 见到 untracked 就拒绝 ——
    // 于是「刚建的变体连拆都拆不掉」。就地提一笔：变体分支的起点，历史干净可回。
    try {
      await run("git", ["-C", target, "add", "--", family], { timeout: 60_000 });
      await run("git", ["-C", target, "commit", "-m", `extend: 变体 ${id} 起点（${family} 还没发布，从工作区拷入）`], {
        timeout: 60_000,
      });
    } catch (error) {
      console.error("[extend] 变体起点提交失败（树还是干净的，可以先手动提交）：", errorLine(error));
    }
  }
  return { target, branch: wanted, dir: targetFamily };
};

/**
 * 删掉一条变体分支：本地 + 远端一起。**绝不碰 main**。
 * 只由「拆树之后显式确认」调用 —— 破坏性动作必须有人亲口说要，不能挂在拆树上自动发生。
 */
const dropVariantBranch = async (branch) => {
  const name = String(branch ?? "").trim();
  if (!name || name === "main" || name.startsWith("-")) {
    throw withStatus(new Error(`不删这个分支：${name || "(空)"}`), 403);
  }
  let remote = false;
  let remoteError = "";
  try {
    await run("git", ["-C", EXTEND, "push", "origin", "--delete", name], { timeout: 120_000 });
    remote = true;
  } catch (error) {
    // 远端本来就没有这条 → 不算失败；别的错误原样带回去。
    const text = String(error.stderr || error.stdout || String(error?.message ?? error)).trim();
    if (!/remote ref does not exist|unable to delete .*unable to find/i.test(text)) remoteError = text;
  }
  const local = await run("git", ["-C", EXTEND, "branch", "-D", name], { timeout: 30_000 })
    .then(() => true)
    .catch(() => false);
  return { branch: name, local, remote, remoteError };
};

/**
 * 拆树。**不带 --force**：里面有没提交的代码就拒绝 —— 那是人写的，不是垃圾。
 *
 * 分支**按是否已合并分档**，不搞「永远留着」那种保守：
 *   · 提交全在 main 里（含空壳：和 main 同一个提交）→ 顺手 `branch -d`，零损失；
 *   · 还有没合并的提交 → 留着，并把这笔数报上去（页面会说「还有 N 个提交没并进 main」）。
 * 远端同名分支**不自动删**：那是共享的，可能是给别人看的。
 */
const removeVariantWorktree = async (family, id) => {
  assertVariantId(id);
  const target = variantWorktreePath(family, id);
  const bound = (await readLocalState()).variants?.[family]?.[id];
  const branch = bound?.branch ?? null;
  const exists = Boolean(await stat(target).catch(() => null));
  if (exists) {
    try {
      await run("git", ["-C", EXTEND, "worktree", "remove", target], { timeout: 120_000 });
    } catch (error) {
      const text = String(error?.stderr || error?.message || error).trim().split("\n").slice(-2).join(" ");
      throw withStatus(new Error(`树里还有没提交的东西，没拆：${text} —— 先在那边提交，或命令行 git worktree remove --force`), 409);
    }
  }
  // 拆树就是「这个变体我不要了」：分支一起删，远端也一起删。
  // 有没合并的提交时前端会先手输确认（它在拆之前就问过），到这里就是人已经点了头。
  let branchDeleted = false;
  let remoteDeleted = false;
  let remoteError = "";
  let unmerged = 0;
  if (branch) {
    try {
      unmerged = Number(
        (await run("git", ["-C", EXTEND, "rev-list", "--count", `main..${branch}`], { timeout: 30_000 })).stdout.trim() || 0,
      );
    } catch {}
    const hadRemote = await run("git", ["-C", EXTEND, "rev-parse", "--verify", `refs/remotes/origin/${branch}`], {
      timeout: 15_000,
    })
      .then(() => true)
      .catch(() => false);
    if (hadRemote) {
      try {
        await run("git", ["-C", EXTEND, "push", "origin", "--delete", branch], { timeout: 120_000 });
        remoteDeleted = true;
      } catch (error) {
        // 远端已经被别人删了 / 网络不通：把原话带回去，别装作删成功。
        const text = String(error.stderr || error.stdout || String(error?.message ?? error)).trim();
        if (!/remote ref does not exist|unable to find/i.test(text)) remoteError = text;
        else remoteDeleted = true;
      }
    }
    try {
      await run("git", ["-C", EXTEND, "branch", "-D", branch], { timeout: 30_000 });
      branchDeleted = true;
    } catch (error) {
      console.error("[extend] 删变体分支失败：", errorLine(error));
    }
  }
  return { removed: exists, target, branch, branchDeleted, remoteDeleted, remoteError, unmerged };
};

const syncExtraExtensions = async (version) => {
  const { deleted } = await readLocalState();
  const catalog = await readExtendCatalog();
  const wanted = new Map(
    catalog
      .filter((manifest) => manifest?.name && !CORE_EXTENSIONS.includes(manifest.name) && !deleted[manifest.name])
      .map((manifest) => [manifest.name, manifest]),
  );
  // 想要的名字 = 本体 + 每个变体。**卸载前先算全**：只拿本体算，
  // 变体每次都会被当成多余的东西卸掉再装回来（每次同步白跑一轮 import）。
  const wantedKeys = new Set(wanted.keys());
  const sourcesOf = new Map();
  for (const [name, manifest] of wanted) {
    const sources = await variantSources(name, manifest);
    sourcesOf.set(name, sources);
    for (const source of sources) wantedKeys.add(variantKey(name, source.id));
  }
  for (const [name, extension] of extraLoaded) {
    if (CORE_EXTENSIONS.includes(variantFamilyOf(name))) continue; // 核心的变体归 installCore 管
    if (!wantedKeys.has(name)) {
      registry.uninstall(extension);
      extraLoaded.delete(name);
      extensionLoad.delete(name); // 本地删 / 归档了：状态也跟着清掉，不再算失败
    }
  }
  const bag = {
    api,
    defineExtension,
    section,
    dir: path.join(DATA, "skills"),
    registry,
  };
  for (const [name, manifest] of wanted) {
    try {
      for (const need of Object.keys(manifest.needs ?? {})) {
        if (!(need in bag)) throw new Error(`宿主给不了 ${need}`);
      }
      const entry = Array.isArray(manifest.entry) ? manifest.entry[0] : manifest.entry;
      const module = await import(`${EXTEND}/${name}/${manifest.file}?v=${encodeURIComponent(version)}-${Date.now()}`);
      const factory = module[entry];
      if (typeof factory !== "function") throw new Error(`导出 ${entry} 不是函数`);
      const made = await factory(bag);
      const extension = made?.extension ?? made;
      if (!extension?.name) throw new Error("造出来的东西没有 name");
      registry.install(extension);
      extraLoaded.set(name, extension);
      noteLoad(name, "ok", { version: manifest.version });
      if (typeof made?.load === "function") await made.load();
      if (typeof made?.backfill === "function") void made.backfill();
    } catch (error) {
      // 不卸旧的：旧版本继续跑。但状态如实标 failed + 错误原文。
      noteLoad(name, "failed", { error: errorLine(error), version: manifest.version });
      console.error(`[extend] ${name} 装载失败，继续跑旧版本：`, errorLine(error));
    }
    // 变体：本体挂了也照样试 —— 变体可能指向另一份代码（本机 checkout），
    // 本体那份坏掉不该把它一起判死。
    for (const source of sourcesOf.get(name) ?? []) {
      const key = variantKey(name, source.id);
      try {
        await installVariant({ family: name, manifest, bag, version, source });
      } catch (error) {
        noteLoad(key, "failed", { error: errorLine(error), version: manifest.version });
        console.error(`[extend] 变体 ${key} 装载失败：`, errorLine(error));
      }
    }
  }
};

// lock 不钉就是没钉：启动时说一句实话。不一致**不拦** —— 开发时天天不一致，
// 拦了每次改扩展都得先提交；它只负责让人知道自己跑的不是钉的那版。
try {
  const lock = JSON.parse(await readFile(path.join(HERE, "extensions.lock.json"), "utf8"));
  const head = (await run("git", ["-C", EXTEND, "rev-parse", "HEAD"])).stdout.trim();
  if (head !== lock.commit) {
    console.error(`[extend] ${EXTEND} 在 ${head.slice(0, 7)}，lock 钉的是 ${lock.commit.slice(0, 7)}`);
  }
} catch (error) {
  console.error("[extend] lock 没读出来，或 git 不在：", String(error?.message ?? error));
}

// ── 扩展类别：真相在仓库里，不在页面上 ─────────────────────────────────────
//
// 客户端只发 POST：改的是 <EXTEND>/<name>/nous.json 的 category，再重跑
// index.mjs 把 index.json 这个**生成物**刷出来。commit/push 单独一条路：
// 用户连点五个标签只该长一条 commit —— 所以写盘是立刻的，提交由客户端 debounce。
//
// 两个必须挡的坑：
//   1. 写坏了不能留工作区 —— 先写临时文件、跑 index.mjs 校验，过了才 rename 回来；
//      没过就把旧内容原样写回去。
//   2. git 操作串行 —— 轮询的 `git pull` 和这里写文件会抢 index.lock，用一条链排队。

// 类别词表：扩展目录的 category 是「哪些类别挂在这个扩展上」，
// categories.json 是「这个仓库认得哪些类别」—— 没有它，一个刚建、还没挂到任何
// 扩展上的类别就会当场消失（建了看不见）。两边取并集，词表里的零计数类别也摆着。
//
// 类别是路径：`工具/搜索` = `工具` 底下的 `搜索`。父子关系不存，靠 `/` 推。
// 祖先会自动进词表（打上 `工具/搜索`，`工具` 就得存在），但**不会**写进扩展的
// category —— 一个扩展属于 `工具` 是因为它的标签前缀是 `工具/`，不是因为它多存了一条。
// 扩展仓的类别工具：root 就是 EXTEND；生成器是本文件里那个（它还要补本机删除的清单）。
const setExtensionCategories = (name, categories) =>
  setNodeCategories(EXTEND, name, categories, { index: runIndexGenerator });
const mutateExtensionCategories = (body) => mutateCategories(EXTEND, body, { index: runIndexGenerator });

// ── 扩展源码：读 / 写（只落盘）/ 发布（校验 + commit + push）──────────────────
//
// **保存和发布是两个动作，不许合成一个。**
//
// 改类别是「连点五个标签 = 一条 commit」，所以客户端 8 秒 debounce 直接推。敲代码
// 不是：半截文件被推上去，下一轮热重载就会去 import 一个语法错误的模块；而
// `server.js` 顶上那两行是**顶层 await import**，所以坏代码留在 master 上，下一次
// 进程启动直接起不来 —— 不是这一块坏，是整个会话引擎起不来。
//
// 所以：保存只写盘（随时可改），发布要过语法闸 + index.mjs，再显式 commit/push。
// 页面上的红字必须说清是「没保存」还是「没发布」—— 这两件事坏起来长得不一样。

/**
 * 变体的代码根目录。没绑定就是 404 —— 别悄悄回落到主树：那会让人以为在改变体，
 * 实际改的是本体（两份字节一模一样时，错误要好几天后才炸）。
 */
const variantRootOf = async (name, variant) => {
  const id = String(variant ?? "");
  if (!VARIANT_ID_RE.test(id)) throw withStatus(new Error(`变体 id 不合法：${id}`), 400);
  const bound = (await readLocalState()).variants?.[name]?.[id];
  if (!bound?.dir) throw withStatus(new Error(`${name}@${id} 没有绑定目录 —— 先在卡片上绑一个`), 404);
  const info = await stat(bound.dir).catch(() => null);
  if (!info?.isDirectory()) throw withStatus(new Error(`变体目录不在了：${bound.dir}`), 404);
  return { dir: bound.dir, branch: bound.branch ?? null };
};

// 扩展仓的文件工具：root 是 EXTEND；变体走它自己那棵 checkout（`dir` 覆盖目录）。
const variantDirOf = async (name, variant) => (variant ? (await variantRootOf(name, variant)).dir : null);

const listExtensionFiles = async (name, variant = null) =>
  listRepoFiles(EXTEND, name, { dir: await variantDirOf(name, variant) });

const readExtensionFile = async (name, rel, variant = null) => ({
  ...(await readRepoFile(EXTEND, name, rel, { dir: await variantDirOf(name, variant) })),
  variant: variant ?? null,
});

const writeExtensionFile = async ({ name, rel, text, base, variant = null }) => ({
  ...(await writeRepoFile(EXTEND, { name, rel, text, base, dir: await variantDirOf(name, variant) })),
  variant: variant ?? null,
});

/** 发布：校验工作区里改过的每个 .js，重扫 index.json，然后 commit + push。 */
const publishExtensions = async ({ name = null, message = null } = {}) => {
  // 发布**只发这一个扩展**（`name/` 底下的一切）+ 重扫出来的 index.json。
  // 以前是「把整份工作区推上去」：抽屉里点一次发布，别人未发布的目录、你正在试的副本
  // 会一起上 GitHub —— 踩过，测试副本就是这么上去的。
  const changes = await gitChanges(EXTEND);
  const scoped = name ? changes.filter((item) => item.path === name || item.path.startsWith(`${name}/`)) : changes;
  if (!scoped.length) {
    return { committed: false, ...(await repoState(EXTEND)), pushed: null, checked: [], changed: [] };
  }
  const checked = [];
  for (const { status, path: rel } of scoped) {
    if (!/\.(js|mjs)$/.test(rel)) continue;
    if (status.includes("D")) continue; // 删掉的文件没什么好查的
    const text = await readFile(path.join(EXTEND, rel), "utf8").catch(() => null);
    if (text === null) continue;
    await checkSyntax(rel, text);
    checked.push(rel);
  }
  await regenIndex(EXTEND);
  const paths = [...new Set([...scoped.map((item) => item.path), "index.json"])];
  const result = await syncRepo(EXTEND, String(message ?? "").trim() || null, { paths });
  return { ...result, checked, changed: scoped.map((item) => item.path) };
};

/** 当前分支名（变体的发布要知道推到哪儿）。 */
const branchOf = async (gitRoot) =>
  (await run("git", ["-C", gitRoot, "rev-parse", "--abbrev-ref", "HEAD"], { timeout: 15_000 })).stdout.trim();

/**
 * 发布一个变体：在**它自己那棵树**里 commit + push 到**它自己的分支**。
 * 跟主树的发布完全隔离 —— 变体分支推坏了，主树和线上一个字节不动。
 *
 * 这类发布的语法闸要过全树改过的 js：那棵树是变体的，别的都没人在里面写。
 */
const publishVariant = async ({ name, variant, message }) => {
  assertVariantId(variant);
  const { dir } = await variantRootOf(name, variant);
  const gitRoot = await gitRootOf(dir, EXTEND);
  const branch = await branchOf(gitRoot);
  const changed = (await run("git", ["-C", gitRoot, "status", "--porcelain"], { timeout: 20_000 })).stdout
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => line.slice(3).trim())
    .filter(Boolean);
  if (!changed.length) return { committed: false, branch, changed, checked: [] };

  const checked = [];
  for (const rel of changed) {
    const file = path.join(gitRoot, rel);
    if (!/\.(mjs|cjs|js)$/i.test(file)) continue;
    const text = await readFile(file, "utf8").catch(() => null);
    if (text === null) continue; // 删掉的文件不用查
    await checkSyntax(rel, text);
    checked.push(rel);
  }

  await run("git", ["-C", gitRoot, "add", "-A"], { timeout: 60_000 });
  await run("git", ["-C", gitRoot, "commit", "-m", String(message ?? "").trim() || `extend: ${name}@${variant}`], {
    timeout: 60_000,
  });
  const head = (await run("git", ["-C", gitRoot, "rev-parse", "HEAD"])).stdout.trim();
  let pushed;
  let pushError = "";
  try {
    // -u：第一次推这条分支时把 upstream 建起来，之后就是普通 push。
    await run("git", ["-C", gitRoot, "push", "-u", "origin", branch], { timeout: 120_000 });
    pushed = true;
  } catch (error) {
    pushed = false;
    pushError = String(error.stderr || error.stdout || String(error?.message ?? error)).trim();
  }
  return { committed: true, head, branch, pushed, pushError, changed, checked };
};

/**
 * 删本地：源码整个搬进 data/extend-trash/，写进本机清单，撤装。
 * 仓库（HEAD / origin）一个字节不动；工作区里留下 ` D` 是删 tracked 文件的必然结果，
 * publish 会把它们过滤掉。任何一步失败就整体回滚，不留半个删除。
 */
const localUninstall = async (name) => {
  if (!EXTENSION_NAME_RE.test(name)) throw withStatus(new Error(`扩展名不合法：${name}`), 400);
  if (CORE_EXTENSIONS.includes(name)) {
    throw withStatus(new Error(`${name} 是引擎启动路径上硬编码 import 的核心扩展，删了下次就起不来`), 403);
  }
  const local = await readLocalState();
  if (local.deleted[name]) throw withStatus(new Error(`${name} 已经从本地删过了`), 409);
  const catalog = await readExtendCatalog();
  const manifest = catalog.find((item) => item.name === name);
  if (!manifest) throw withStatus(new Error(`清单里没有这个扩展：${name}`), 404);
  const dir = path.join(EXTEND, name);
  if (!(await stat(dir).catch(() => null))) throw withStatus(new Error(`本地没有 ${name}/ 目录`), 404);

  const trash = path.join(TRASH_DIR, `${name}-${new Date().toISOString().replace(/[:.]/g, "-")}`);
  const indexFile = path.join(EXTEND, "index.json");
  const beforeIndex = await readFile(indexFile, "utf8").catch(() => null);
  await mkdir(TRASH_DIR, { recursive: true });
  await moveDir(dir, trash);
  try {
    let head = null;
    try {
      head = (await run("git", ["-C", EXTEND, "rev-parse", "HEAD"])).stdout.trim();
    } catch {}
    await writeLocalState({
      ...local,
      deleted: { ...local.deleted, [name]: { at: Date.now(), head, trash: path.relative(DATA, trash), manifest } },
    });
    await runIndexGenerator();
    await syncExtraExtensions(head ?? "local");
    return { name, deleted: true, trash: path.relative(DATA, trash) };
  } catch (error) {
    // 回滚：目录搬回去、名单写回原样、生成物也还原。
    await moveDir(trash, dir).catch(() => {});
    await writeLocalState(local).catch(() => {});
    if (beforeIndex !== null) await writeFileAtomic(indexFile, beforeIndex).catch(() => {});
    throw withStatus(new Error(`本地删除失败，已回滚：${String(error?.message ?? error)}`), 400);
  }
};

/** 装回来：优先从垃圾站搬，没有就问 HEAD 要；再撤掉本机清单里那条。 */
const localInstall = async (name) => {
  if (!EXTENSION_NAME_RE.test(name)) throw withStatus(new Error(`扩展名不合法：${name}`), 400);
  const local = await readLocalState();
  const record = local.deleted[name];
  if (!record) throw withStatus(new Error(`${name} 没被本地删过，不用装`), 409);
  const dir = path.join(EXTEND, name);
  const trash = record.trash ? path.join(DATA, record.trash) : null;
  if (!(await stat(dir).catch(() => null))) {
    if (trash && (await stat(trash).catch(() => null))) {
      await moveDir(trash, dir);
    } else {
      // 垃圾站被清了：从本地仓库的 HEAD 拿回来。未发布的扩展 HEAD 里没有，只能如实报错。
      try {
        await run("git", ["-C", EXTEND, "checkout", "HEAD", "--", name], { timeout: 30_000 });
        await run("git", ["-C", EXTEND, "reset", "-q", "HEAD", "--", name], { timeout: 30_000 });
      } catch (error) {
        throw withStatus(new Error(`垃圾站没有 ${name}，HEAD 里也找不到：${String(error?.message ?? error)}`), 409);
      }
    }
  }
  const { [name]: _gone, ...rest } = local.deleted;
  try {
    await writeLocalState({ ...local, deleted: rest });
    await runIndexGenerator();
    await syncExtraExtensions(`restore-${Date.now()}`);
  } catch (error) {
    await writeLocalState(local).catch(() => {});
    throw withStatus(new Error(`装回来失败：${String(error?.message ?? error)}`), 400);
  }
  return { name, deleted: false };
};

// ── 归档：从仓库里拿走，但留一份可恢复的备份 ─────────────────────────────────
//
// `.archive/<name>/` 在仓库里跟着 origin 走，clone 下来也在 —— 所以「从库里删」
// 仍然是可逆的。`purge` 才是真没。三个动作都要 commit + push（走发布同一个仓库），
// 所以都串进 serializeRepoWrite(EXTEND, …)，并带上 includeDeleted。

/** 三个动作共用的验前检查。 */
const guardExtensionName = (name, { allowCore = false } = {}) => {
  if (!EXTENSION_NAME_RE.test(name)) throw withStatus(new Error(`扩展名不合法：${name}`), 400);
  if (!allowCore && CORE_EXTENSIONS.includes(name)) {
    throw withStatus(new Error(`${name} 是引擎启动路径上硬编码 import 的核心扩展，不能动`), 403);
  }
};

// ── 新建扩展 ───────────────────────────────────────────────────────────────
//
// 这里只建「合法的空壳」：目录名 = 扩展名 = 清单里的 name，入口是个不装任何东西的
// 函数。**故意不给能跑的例子** —— 新扩展默默往提示词里塞一段内容，是最恶心的惊喜。
//
// 三条必须守住：
//   1. 名字就是 API（会话里存的是名字），建之前就得确认没撞上任何东西：目录、
//      .archive 里的旧账，都要挡。
//   2. 半截不能留：先建 `.new-<name>-<ts>/`（点开头，index.mjs 天然跳过），两个文件
//      都写好了、node --check 过了，才 rename 成正式目录。校验不过整棵删掉。
//   3. 建完不提交 —— 未发布的扩展才是它该有的第一个状态，让用户自己按发布。
const SKELETON_JS = `// __NAME__ —— __SUMMARY__
//
// 一个扩展 = 一个目录 + 一份清单（nous.json）+ 一个入口（这个文件）。
// 现在它是空的：只声明名字，什么都不装。装载不会红，但它也什么都不会做。
//
// 让它干活，从这三件事里挑一件开始：
//
// 1. 往系统提示词里加一段：
//      export default ({ defineExtension, section }) =>
//        defineExtension({
//          name: "__NAME__",
//          sections: [section("__NAME__", () => "这段话会进系统提示词")],
//        });
//    section 的键记得同时写进 nous.json 的 sections（页面拿它数「提示词 N 段」）。
//
// 2. 加工具：tools: [defineTool({ name, description, parameters, execute })]。
//
// 3. 要宿主的东西：在 nous.json 的 needs 里写键名，宿主会把值递进来。只认五个：
//      api / defineExtension / section / dir / registry
//    （dir = 这块自己的 data 目录，registry = 扩展注册表。）
//
// 写完点「发布」：先过 node --check 和 index.mjs，才 commit + push。
export default ({ defineExtension }) => defineExtension({ name: "__NAME__" });
`;

const createExtension = async ({ name, summary, category }) => {
  guardExtensionName(name);
  const clean = String(summary ?? "").trim();
  if (!clean) throw withStatus(new Error("一句话说明不能空 —— 卡片上就靠它认人"), 400);
  if (clean.length > 200) throw withStatus(new Error("说明太长了（200 字以内）"), 400);

  // 撞名三连：工作区、归档区。核心两条 guardExtensionName 已经挡了。
  if (await stat(path.join(EXTEND, name)).catch(() => null)) {
    throw withStatus(new Error(`${name}/ 已经在了（清单里可能叫别的）`), 409);
  }
  if (await stat(path.join(EXTEND, ARCHIVE_DIR, name)).catch(() => null)) {
    throw withStatus(new Error(`.archive/${name} 还留着 —— 点「已删除」里的「恢复」把它捡回来，别建第二个`), 409);
  }

  const cats = sortCategories(normalizeCategories(category));
  const manifest = {
    name,
    version: "0.1.0",
    file: "index.js",
    entry: "default",
    sections: [],
    summary: clean,
    needs: {},
    ...(cats.length ? { category: cats } : {}),
  };
  const code = SKELETON_JS.replace(/__NAME__/g, name).replace(/__SUMMARY__/g, clean.replace(/\n/g, " "));

  // 点开头 = 生成器看不见它。校验过了才改名，中途翻车就整棵删掉。
  const staging = path.join(EXTEND, `.new-${name}-${Date.now()}`);
  await mkdir(staging, { recursive: false });
  try {
    await writeFile(path.join(staging, "nous.json"), JSON.stringify(manifest, null, 2) + "\n", "utf8");
    await writeFile(path.join(staging, "index.js"), code, "utf8");
    await run(process.execPath, ["--check", path.join(staging, "index.js")], { cwd: EXTEND, timeout: 20_000 });
    await rename(staging, path.join(EXTEND, name));
  } catch (error) {
    await rm(staging, { recursive: true, force: true });
    const detail = (error.stderr || error.stdout || String(error?.message ?? error)).trim();
    throw withStatus(new Error(`建不出来，已清干净：${detail}`), 400);
  }

  // 目录落了才算数。生成器不认就把它删掉 —— 别留一个 index.json 里没有、
  // 页面也不认识的目录。
  try {
    await runIndexGenerator();
  } catch (error) {
    await rm(path.join(EXTEND, name), { recursive: true, force: true });
    const detail = (error.stderr || error.stdout || String(error?.message ?? error)).trim();
    throw withStatus(new Error(`index.mjs 不认这个骨架，已删掉：${detail}`), 400);
  }
  if (cats.length) await writeCategories(EXTEND, [...(await readCategories(EXTEND)), ...cats]);
  return { ok: true, name, manifest };
};

// ── 复制成新扩展 ───────────────────────────────────────────────────────────
//
// 复制 = 新目录 + 新名字 + 新清单，**历史不带**（旧提交留在原扩展里）。落盘即「未发布」，
// 提交/推送仍走发布那两条路 —— 这里只负责把目录造对。
//
// 唯一会咬人的地方：**名字是写死在入口里的**（`defineExtension({ name: "123" })`）。
// 不换掉它，下次 reload 两个扩展注册同一个名字，新的把原来的顶掉，而页面上两张卡都好好的。
// 所以：窄匹配只改 `name: "旧名"` 这种字面量；改不到就当场删掉整个副本，绝不留下一个会撞名的目录。

const DUP_SKIP = new Set(["node_modules", ".git"]);

const escapeRe = (value) => String(value).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** 只认「名字字段」的字符串字面量：`name: "x"` / `"name": "x"`，别的一概不动。 */
const renameInCode = (code, from, to) => {
  // 两个模式都是「head = 组 1、引号 = 组 2」，所以反引用统一写 \2。
  const quote = `(["'\`])`;
  const patterns = [
    new RegExp(`(\\bname\\s*:\\s*)${quote}${escapeRe(from)}\\2`, "g"),
    new RegExp(`(["'\`]name["'\`]\\s*:\\s*)${quote}${escapeRe(from)}\\2`, "g"),
  ];
  let hits = 0;
  let text = String(code);
  for (const pattern of patterns) {
    text = text.replace(pattern, (match, head, quote) => {
      hits++;
      return `${head}${quote}${to}${quote}`;
    });
  }
  return { text, hits };
};

const duplicateExtension = async ({ from, variant = null, name, summary, category, unbind = false }) => {
  guardExtensionName(from);
  guardExtensionName(name);
  if (from === name) throw withStatus(new Error("复制到同名？改个名字"), 400);

  // 源可以是主树里的扩展，也可以是**某个变体自己那棵树**（「转正」就是这一条）。
  // 变体那颗树上可能有还没提交的改动 —— 那正是它的价值：试出来的东西原样落地。
  const source = variant
    ? { ...(await variantRootOf(from, variant)), kind: "variant" }
    : { dir: path.join(EXTEND, from), branch: null, kind: "extension" };
  const src = source.dir;
  const info = await stat(src).catch(() => null);
  if (!info?.isDirectory()) throw withStatus(new Error(`${from}/ 不在工作区里`), 404);

  if (await stat(path.join(EXTEND, name)).catch(() => null)) {
    throw withStatus(new Error(`${name}/ 已经在了 —— 换个名字`), 409);
  }
  if (await stat(path.join(EXTEND, ARCHIVE_DIR, name)).catch(() => null)) {
    throw withStatus(new Error(`.archive/${name} 还留着 —— 那是「恢复」不是「复制」，别同名`), 409);
  }

  let manifest;
  try {
    manifest = JSON.parse(await readFile(path.join(src, "nous.json"), "utf8"));
  } catch {
    throw withStatus(new Error(`${from}/ 没有能读的 nous.json`), 409);
  }
  if (manifest.name !== from) {
    throw withStatus(new Error(`${from}/nous.json 里的 name 是 ${manifest.name}，跟目录名对不上 —— 先修它再复制`), 409);
  }

  const entryFile = String(manifest.file ?? "index.js");
  const clean = String(summary ?? "").trim() || String(manifest.summary ?? "").trim() || name;
  if (clean.length > 200) throw withStatus(new Error("说明太长了（200 字以内）"), 400);
  const picked = Array.isArray(category)
    ? normalizeCategories(category)
    : normalizeCategories(String(category ?? "").split(","));
  const cats = sortCategories(picked.length ? picked : normalizeCategories(manifest.category ?? []));

  // 点开头 = 生成器看不见它。校验过了才改名，中途翻车就整棵删掉。
  const staging = path.join(EXTEND, `.copy-${name}-${Date.now()}`);
  let rewritten = [];
  try {
    await cp(src, staging, {
      recursive: true,
      filter: (source) => !DUP_SKIP.has(path.basename(source)),
    });

    // ① 入口里写死的名字
    const entryPath = path.join(staging, entryFile);
    const code = await readFile(entryPath, "utf8").catch(() => null);
    if (code === null) {
      throw withStatus(new Error(`${from}/ 里找不到入口 ${entryFile} —— 清单写着它但不在这儿`), 409);
    }
    const patched = renameInCode(code, from, name);
    if (!patched.hits) {
      throw withStatus(
        new Error(`${from}/${entryFile} 里没有写死的 name: "${from}"，可能是从别处读的名字 —— 那样复制过去会跟本体撞名，不敢造`),
        409,
      );
    }
    if (patched.text !== code) {
      await writeFile(entryPath, patched.text, "utf8");
      rewritten.push({ path: `${name}/${entryFile}`, from: `name: "${from}"`, to: `name: "${name}"` });
    }

    // ② 清单：名字换掉，其余（file/entry/sections/needs/version）原样 —— 代码没变，段落和依赖就没变。
    const nextManifest = {
      ...manifest,
      name,
      summary: clean,
      ...(cats.length ? { category: cats } : {}),
    };
    if (!cats.length) delete nextManifest.category;
    await writeFile(path.join(staging, "nous.json"), JSON.stringify(nextManifest, null, 2) + "\n", "utf8");

    // ③ 语法闸：改过的入口得能过（改名字也是改代码）。
    if (patched.text !== code) await checkSyntax(`${name}/${entryFile}`, patched.text);

    await rename(staging, path.join(EXTEND, name));
  } catch (error) {
    await rm(staging, { recursive: true, force: true });
    throw error.status ? error : withStatus(new Error(`复制不出来，已清干净：${String(error?.message ?? error)}`), 400);
  }

  try {
    await runIndexGenerator();
  } catch (error) {
    await rm(path.join(EXTEND, name), { recursive: true, force: true });
    const detail = (error.stderr || error.stdout || String(error?.message ?? error)).trim();
    throw withStatus(new Error(`index.mjs 不认这个副本，已删掉：${detail}`), 400);
  }
  if (cats.length) await writeCategories(EXTEND, [...(await readCategories(EXTEND)), ...cats]);
  // 转正之后「变体要不要留」是**第二个决定**：默认一动不动（树、分支、绑定都还在），
  // 人勾了才解绑，而且解绑也不拆树不删分支 —— 那是它自己那条路。
  let unbound = false;
  if (variant && unbind) {
    await unbindVariant(from, variant);
    unbound = true;
  }
  return {
    ok: true,
    name,
    from,
    variant,
    manifest: { ...manifest, name, summary: clean },
    rewritten,
    source: { kind: source.kind, dir: src, branch: source.branch ?? null },
    unbound,
  };
};

const archiveExtension = async (name) => {
  guardExtensionName(name);
  const local = await readLocalState();
  if (local.deleted[name]) {
    throw withStatus(new Error(`${name} 已经在本机删了（灰卡）—— 先在卡片上点「安装」再归档`), 409);
  }
  if (!(await stat(path.join(EXTEND, name)).catch(() => null))) {
    throw withStatus(new Error(`工作区里没有 ${name}/`), 404);
  }
  // 未发布（HEAD 里没有这个目录）= 从来没进过仓库。归档它等于「先把没发布的东西推上去，
  // 再归档」—— 那是个反着的动作，而且会把半成品代码塞进 GitHub。没有历史可留，直接删。
  if (!(await isTrackedDir(EXTEND, name))) {
    await rm(path.join(EXTEND, name), { recursive: true, force: true });
    await runIndexGenerator();
    await syncExtraExtensions(`drop-${Date.now()}`);
    return { name, archived: false, dropped: true, committed: false, pushed: false, head: null };
  }
  const indexFile = path.join(EXTEND, "index.json");
  const beforeIndex = await readFile(indexFile, "utf8").catch(() => null);
  try {
    await moveIntoArchive(EXTEND, name);
    await runIndexGenerator();
    // 先从库里撤下来（停装），再提交。顺序反过来会有一小段时间「清单里有、目录没了」。
    await syncExtraExtensions(`archive-${Date.now()}`);
    const result = await syncRepo(EXTEND, `extend: 归档 ${name}`, { includeDeleted: true });
    return { name, archived: true, ...result };
  } catch (error) {
    await run("git", ["-C", EXTEND, "reset", "-q", "--", name, `${ARCHIVE_DIR}/${name}`]).catch(() => {});
    await moveDir(path.join(EXTEND, ARCHIVE_DIR, name), path.join(EXTEND, name)).catch(() => {});
    if (beforeIndex !== null) await writeFileAtomic(indexFile, beforeIndex).catch(() => {});
    await syncExtraExtensions(`archive-rollback-${Date.now()}`).catch(() => {});
    throw withStatus(new Error(`归档失败，已回滚：${String(error?.message ?? error)}`), 400);
  }
};

const restoreExtension = async (name) => {
  guardExtensionName(name);
  try {
    await moveOutOfArchive(EXTEND, name);
    await runIndexGenerator();
    await syncExtraExtensions(`unarchive-${Date.now()}`);
    const result = await syncRepo(EXTEND, `extend: 从归档恢复 ${name}`, { includeDeleted: true });
    return { name, archived: false, ...result };
  } catch (error) {
    await run("git", ["-C", EXTEND, "reset", "-q", "--", name, `${ARCHIVE_DIR}/${name}`]).catch(() => {});
    await moveDir(path.join(EXTEND, name), path.join(EXTEND, ARCHIVE_DIR, name)).catch(() => {});
    await syncExtraExtensions(`unarchive-rollback-${Date.now()}`).catch(() => {});
    throw withStatus(new Error(`恢复失败，已回滚：${String(error?.message ?? error)}`), 400);
  }
};

const purgeExtension = async (name) => {
  guardExtensionName(name);
  const source = path.join(EXTEND, ARCHIVE_DIR, name);
  const exists = await stat(source).catch(() => null);
  // 上一次彻底删除提交/推送挂了：文件已经删了、删除还留在暂存区 —— 那一次要把
  // 它收尾，而不是报 404 把人卡在一个半截状态里。
  const halfDone = !exists
    && (await run("git", ["-C", EXTEND, "status", "--porcelain", "--", `${ARCHIVE_DIR}/${name}`])).stdout.trim().length > 0;
  if (!exists && !halfDone) throw withStatus(new Error(`归档区里没有 ${name}`), 404);
  const tracked = await isTrackedDir(EXTEND, `${ARCHIVE_DIR}/${name}`);
  try {
    if (exists && tracked) {
      await run("git", ["-C", EXTEND, "rm", "-r", "-f", "--", `${ARCHIVE_DIR}/${name}`], { timeout: 30_000 });
    } else if (exists) {
      await rm(source, { recursive: true, force: true });
    }
    const result = await syncRepo(EXTEND, `extend: 彻底删除 ${name}`, { includeDeleted: true });
    return { name, purged: true, ...result };
  } catch (error) {
    // 提交/推送挂了：文件已经没了，但 HEAD 里还有 —— 拿回来，别留一个半截的暂存删除。
    if (tracked) {
      await run("git", ["-C", EXTEND, "reset", "-q", "--", `${ARCHIVE_DIR}/${name}`]).catch(() => {});
      await run("git", ["-C", EXTEND, "checkout", "HEAD", "--", `${ARCHIVE_DIR}/${name}`]).catch(() => {});
    }
    throw withStatus(new Error(`彻底删除失败，已回滚：${String(error?.message ?? error)}`), 400);
  }
};

/** 有哪些会话的扩展列表里还点着这个名字（`123@dev` 也算一个名字）。 */
const sessionsUsing = async (key) => {
  const records = (await harness.commit((tx) => tx.scanConversations({}, 200, undefined), ctx)).items;
  const ids = [];
  for (const record of records) {
    const conversation = await harness.conversation(record.id, ctx);
    if (!conversation) continue;
    const agent = await conversation.agent(ctx);
    if ((agent.extensions ?? []).some((item) => item.name === key)) ids.push(record.id);
  }
  return ids;
};

// 会话名不在 ConversationRecord 里（那东西没有 name 字段），也不许进 tree.json。
// 存在一个 session 级文档里 —— 名字是这盘的状态，跟棋谱一起落 sqlite。
const Sessions = defineDoc({
  kind: "qrd3wh.sessions",
  version: 1,
  scope: "session",
  // archived 是归档墓碑（ADR 0001）：{ id: { at, gen, purged } }。老盘没有这个键 ——
  // setArchived 里和 machines 一个待遇（`??=`），不靠 initial。
  initial: () => ({ names: {}, machines: {}, archived: {} }),
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

// 归档机器（ADR 0001：复印 + 撕原件，agent.sqlite 永不关）。
// 用第二个连接动表 —— 活 Session 一次都不关；机器内部自己校验 durable_schema 版本。
const archive = openArchive({ dataDir: DATA, liveDbPath: path.join(DATA, "agent.sqlite"), log: (m) => console.log(m) });

// ── 提示词 ───────────────────────────────────────────────────────────────
//
// 提示词一类的自研扩展（`nous` = 自我介绍 + cwd）在 Nous-extend 仓库的 `nous/`，不写在这儿。
// 当初为什么先有它再有 skills：registry 里只有 CodingTools 时一条 section 都没有，
// 系统提示词是空的 —— 技能清单挂上去会像「一个不知道自己是谁的家伙手里拿着一张技能表」。

const nousExtension = createNous(api);
// skills 的 API 用间接引用：热重载时要换整个 service 对象。直接替换变量不够，
// 路由里的闭包还指着旧对象——所以所有调用都走代理，读当前那个 service。
// skills / prompts 的 API 用间接引用：热重载时要换整个 service 对象。直接替换变量不够，
// 路由里的闭包还指着旧对象——所以所有调用都走代理，读当前那个 service。
const serviceProxy = () => {
  const holder = { current: null };
  const proxy = new Proxy({}, {
    get(_, key) {
      const service = holder.current;
      const value = service[key];
      return typeof value === "function" ? value.bind(service) : value;
    },
  });
  return { holder, proxy };
};
const skillsService = serviceProxy();
const skills = skillsService.proxy;

/**
 * 上游镜像里已经 check out 的那些技能根 —— 别人的仓，**只读**。
 *
 * 只认盘上已经有的：这里不联网、不 clone、不 fetch。没刷新过就是没有，
 * 勾「上游」时会如实说「先去技能面板刷新」，而不是打开页面就偷偷下载 39 条。
 * 它们并进会话的可加载名册（`usable`），但**不进 `list()`** —— 面板那张网格是
 * 「你自己的仓」，别人的 39 条混进去，第二天就没人认得出哪个是自己的。
 */
const upstreamSkillRoots = async () => {
  // 订阅表是**内容**，住技能仓（`sources.json` 进版本库）；镜像才是块的 `data/`。
  const { sources } = await upstream.readSources(SKILLS_ROOT).catch(() => ({ sources: [] }));
  const out = [];
  for (const source of sources) {
    if (!(await upstream.hasMirror({ data: DATA, source }).catch(() => false))) continue;
    out.push({ name: source.name, root: upstream.localPath(DATA, source) });
  }
  return out;
};

// 技能变体：绑定表在 data/ 里，树在仓的隔壁。装载器要的是 `[{family, id, root}]`，
// 每次重建 service 都从绑定表现场读一遍 —— 绑一条就多一个 `本名@id`。
const skillVariants = createVariantStore({ dataDir: DATA, skillsRoot: SKILLS_ROOT });

/** 重新造一遍 skills service（绑 / 解绑之后要让新变体进名册，不然页面说有、会话里没有）。 */
const rebuildSkills = async () => {
  skillsService.holder.current = await createSkills({
    dir: SKILLS_ROOT,
    registry,
    api,
    variants: await skillVariants.loadable(),
    upstream: await upstreamSkillRoots(),
  });
  registry.install(skillsService.holder.current.extension);
  await skillsService.holder.current.load();
  return skillsService.holder.current;
};

skillsService.holder.current = await createSkills({
  dir: SKILLS_ROOT,
  registry,
  api,
  variants: await skillVariants.loadable(),
  upstream: await upstreamSkillRoots(),
});
const { skillExtension } = skills;
// 提示词模板同一套：私库在 /opt/Nous-prompts，每条模板一个 `prompt:<名>` 空白名牌。
// **不同之处**：正文整段进系统提示词，所以选择是单值（`templateOf` = 第一条），不是集合。
const promptsService = serviceProxy();
const prompts = promptsService.proxy;
promptsService.holder.current = await createPrompts({ promptsDir: PROMPTS_ROOT, registry, api });
const { promptExtension } = prompts;
/** 仓库 `settings.json` 的默认模板。只在建会话时读盘（不是每轮请求）—— 改完立刻生效。 */
const readDefaultTemplate = async () => {
  try {
    const parsed = JSON.parse(await readFile(path.join(PROMPTS_ROOT, "settings.json"), "utf8"));
    return typeof parsed?.default === "string" ? parsed.default : null;
  } catch {
    return null;
  }
};

/**
 * `agent.extensions` 既可能是名字也可能是 `{name}`（取决于从哪一层读到的），两种都收。
 * 名牌（`skill:` / `prompt:`）**不混进「扩展」列表**：它们是运行时生成的家族，
 * 混进来面板就会多出几十行能点但没意义的「生成物」。
 */
const extensionNamesOf = (agent) =>
  (agent?.extensions ?? [])
    .map((extension) => (typeof extension === "string" ? extension : extension?.name))
    .filter((name) => typeof name === "string" && !NAMEPLATE_PREFIXES.some((prefix) => name.startsWith(prefix)));
registry.install(nousExtension);
registry.install(skills.extension);
registry.install(prompts.extension);
await skills.load();
await prompts.load();
// 模板仓按**段**放（Session-Template / Ultimate-Template）：一段一个名册，各扫各的目录。
// 名册只是个投影（无 registry 副作用、无渲染）。扫一次就够；每次写盘由端点自己 load()。
const presetSections = await readSections(PRESETS_ROOT);
const presetsBySection = new Map();
for (const section of presetSections) {
  if (!section.checker) continue; // 形状还没定的段：不装名册，端点会回「这一段还没装好」
  presetsBySection.set(
    section.id,
    await createPresets({
      dir: path.join(PRESETS_ROOT, section.id),
      manifest: section.manifest,
      checker: section.checker,
    }),
  );
}
const presetsOf = (id) => presetsBySection.get(id) ?? null;
/**
 * 引用校验 / 显示的 ctx。**每次现造** —— 它问的是此刻的进程状态（技能装了没、模型在不在
 * 编目里），不是启动那一刻的快照。面板、校验端点、建会话三处共用它，所以「面板说能跑」
 * 和「真的能跑」是同一个判定。
 */
const presetCtx = () => presetContext({ skills, prompts, registry, models, levelsOf, defaultModel });

/**
 * 触发器的投影（另一个仓）。**每次现读** —— 触发器是别人写的代码，它随时会变；
 * 缓存一份 = 面板说「触发器在」，其实已经改名了。
 */
const triggerCatalog = async () => readTriggerCatalog();

/**
 * 终极模板（Ultimate-Template 段）要的 ctx：触发器表 + 会话模板名册。
 * 会话模板是**另一个段**的名册 —— 终极模板引用它，所以这里得能问「那条会话模板还在吗」。
 */
const ultimateCtx = async () => {
  const triggers = await triggerCatalog();
  const sessions = presetsOf("Session-Template");
  return {
    triggers,
    sessionsHas: (name) => Boolean(sessions?.has?.(String(name ?? ""))),
  };
};
// 老收据时代的补版本已经没了（技能没有版本号）。留着 `?.` 是因为：
// 扩展仓可以比这块新或旧，缺一个方法不该让启动路径炸掉。
void skills.backfill?.();
// 核心的变体：**启动时就得装**。只靠 installCore 的话，进程一重启它们就没了，
// 得等人点一次「重试装载」或者推一次代码才回来 —— 那是「页面说装了、进程里没有」的翻版。
await loadCoreVariants("boot");
// 核心之外，按 index.json 装其余扩展。放最后：一个坏扩展不该拖垮启动路径。
await syncExtraExtensions("boot");

// ── B + 自动触发：push 代码，进程里立刻生效，不用点装 ─────────────────────
//
// B 方案的核心特性：`registry.install()` 是同名就地替换 —— 不重启、不重放历史，
// 所有会话下一轮就吃到新代码。装这个动作根本不需要人点，人只做一件事：写代码 + push。
//
// 用轮询（不用 webhook）：每 N 秒一次 `git rev-parse HEAD`，变了就
// `git pull --ff-only`，然后用带 `?v=<sha>` 的 `import()` 热重载（ESM 缓存要绕过）。
// skills 得用间接引用（闭包太多，直接换变量不行），nous 直接覆盖就行。
// 重装失败不崩进程，下一轮再试；lock 自动把 lock 写成新的 HEAD。
const EXTEND_RELOAD_MS = Number(process.env.NOUS_EXTEND_RELOAD_MS ?? 30_000);
let extendRepoHead = null;
let extendPullError = ""; // 热重载的 pull 失败要上页面，不能只躺在日志里

const installCore = async (version) => {
  const { default: createNous } = await import(`${EXTEND}/nous/index.js?v=${version}`);
  registry.install(createNous(api));
  // skills 与 prompts 都换整个 service 对象（路由里的闭包指着代理，不指着变量）。
  const skillsModule = await import(`${EXTEND}/skills/index.js?v=${version}`);
  skillsService.holder.current = await skillsModule.createSkills({
    dir: SKILLS_ROOT,
    registry,
    api,
    variants: await skillVariants.loadable(),
    upstream: await upstreamSkillRoots(),
  });
  registry.install(skillsService.holder.current.extension);
  await skillsService.holder.current.load();
  void skillsService.holder.current.backfill?.();

  const promptsModule = await import(`${EXTEND}/prompts/index.js?v=${version}`);
  promptsService.holder.current = await promptsModule.createPrompts({
    promptsDir: PROMPTS_ROOT,
    registry,
    api,
  });
  registry.install(promptsService.holder.current.extension);
  await promptsService.holder.current.load();

  await loadCoreVariants(version);
};

/** 只在本版变了才写 —— 写它会触发宿主重启块，没必要每次重试都重启一次。 */
const pinLock = async (head) => {
  const file = path.join(HERE, "extensions.lock.json");
  try {
    if (JSON.parse(await readFile(file, "utf8")).commit === head) return;
  } catch {}
  await writeFile(file, JSON.stringify({
    _about: ["钉住 Nous-extend 仓库的那个 commit —— 扩展源码在仓库里，版本钉在这儿。"],
    repo: "JoeWest9966/Nous-extend",
    path: EXTEND,
    commit: head,
  }, null, 2) + "\n");
};

const reloadExtensions = async () => {
  // 正在写仓库（改类别 / commit / push）时让一轮 —— 两个 git 进程会抢 index.lock。
  if (repoBusy(EXTEND)) return;
  let head;
  try {
    head = (await run("git", ["-C", EXTEND, "rev-parse", "HEAD"])).stdout.trim();
  } catch (error) {
    console.error("[extend] git 不在或仓库没了：", String(error?.message ?? error));
    return;
  }
  if (extendRepoHead === null) {
    // 初次：从 lock 读我们 booted 的版本 —— HEAD 可能已经比 lock 新（比如手动 pull 过），
    // 那样 extendRepoHead = HEAD 就永远追不上，重装永远不触发。
    try {
      extendRepoHead = JSON.parse(await readFile(path.join(HERE, "extensions.lock.json"), "utf8")).commit;
    } catch {
      extendRepoHead = head;
    }
    return;
  }
  if (head === extendRepoHead) return;

  try {
    await run("git", ["-C", EXTEND, "pull", "--ff-only"]);
  } catch (error) {
    // 本地删了某个扩展，别人又推了那个目录 → ff-only 会报「本地改动会被覆盖」。
    // 那之后热重载会一直停摆，页面必须看见这句话。
    extendPullError = (error.stderr || error.stdout || String(error?.message ?? error)).trim();
    console.error("[extend] git pull 失败，下一轮再试：", extendPullError);
    return;
  }
  extendPullError = "";
  head = (await run("git", ["-C", EXTEND, "rev-parse", "HEAD"])).stdout.trim();
  if (head === extendRepoHead) return;

  try {
    await installCore(head);
    await syncExtraExtensions(head);
    extensionLoad.delete("__reload__");

    extendRepoHead = head;
    await pinLock(head);
    console.error("[extend] 热重装：nous + skills + prompts + 清单扩展 @" + head.slice(0, 7));
  } catch (error) {
    // 核心扩展挂了：旧的那套还在跑。页面必须说真话，不能显示成绿卡。
    noteLoad("__reload__", "failed", { error: errorLine(error) });
    console.error("[extend] 重装失败：", errorLine(error));
  }
};
// 「重试装载」：不等 30 秒轮询，按磁盘现状立刻装一遍。失败不抛仓库状态。
const reloadExtensionsNow = async () => {
  try {
    await run("git", ["-C", EXTEND, "pull", "--ff-only"]);
    extendPullError = "";
  } catch (error) {
    // 拉不下来也能用磁盘上的代码重装 —— 只是得把这句话带到页面上。
    extendPullError = (error.stderr || error.stdout || String(error?.message ?? error)).trim();
  }
  const head = (await run("git", ["-C", EXTEND, "rev-parse", "HEAD"])).stdout.trim();
  // 重装前先重扫清单：`index.json` 是生成物，而「改 nous.json 加一个变体再点重装」
  // 不会走 publish —— 不重扫的话装载层读到的还是改之前那份清单，人会觉得变体没生效。
  // 扫完不 commit：清单变了就该显示成「待同步」，那才是实话。
  try {
    await runIndexGenerator();
  } catch (error) {
    // 清单扫不动（有目录缺字段之类）也照样按旧清单装：能跑比整齐重要。
    console.error("[extend] 重扫 index.json 失败：", errorLine(error));
  }
  try {
    await installCore(head);
    await syncExtraExtensions(`reload-${Date.now()}`);
    extensionLoad.delete("__reload__");
  } catch (error) {
    noteLoad("__reload__", "failed", { error: errorLine(error) });
    throw withStatus(new Error(`重装失败：${errorLine(error)}`), 400);
  }
  extendRepoHead = head;
  await pinLock(head);
  return { ok: true, head, pullError: extendPullError, load: Object.fromEntries(extensionLoad) };
};

setInterval(reloadExtensions, EXTEND_RELOAD_MS);
void reloadExtensions();

const harness = await Harness.open(
  storage,
  {
    models,
    registry,
    // 会话在哪台机器上跑：机器名从 Sessions 文档的 machines 边表来（AgentState 没有位置）。
    // 找不到 / 机器不在编目 → 退回本机，并记一行，别静默成「照常」。
    env: async ({ cwd, conversationId }) => {
      const machineName = (await machinesOf())[key(conversationId)];
      if (!machineName) return new NodeExecutionEnv({ cwd: cwd ?? HERE });
      const machine = await resolveMachine(machineName);
      if (!machine?.addr) {
        console.error(`[machine] 会话 ${conversationId} 指着机器 ${machineName}，但编目里没有它 —— 退回本机`);
        return new NodeExecutionEnv({ cwd: cwd ?? HERE });
      }
      // 名字不能叫 `key` —— 边表那个 `key()` 就在同一个作用域里，
      // `const key` 会把它遮住并把自己关进 TDZ（踩过：Cannot access 'key' before initialization）。
      const secretKey = await machineSecret(machineName);
      if (!secretKey) {
        console.error(`[machine] 会话 ${conversationId} 指着 ${machineName}，但拿不到密钥 —— 退回本机`);
        return new NodeExecutionEnv({ cwd: cwd ?? HERE });
      }
      return new SshExecutionEnv({ cwd: cwd ?? "~", host: machine, key: secretKey });
    },
    // 默认选择只放**真扩展**：空名牌（skill:<名>）装了但不默认选 —— 否则每条新会话
    // 一出生就吃 38 条描述，progressive disclosure 当场作废。
    settings: { extensions: [CodingTools, nousExtension, skills.extension, prompts.extension] },
    // 新会话一出生就带一张自己的名字卡片，省得后面到处判空。
    conversationCreated: async (tx, conversation) => {
      const doc = await tx.doc(Sessions);
      doc.names[key(conversation.id)] = `对话 ${conversation.id}`;
      // 未读水位也在这儿种（`unread` 是下面才建的 const，这个钩子只在新会话出生时跑，
      // 那时早就初始化完了）。种 0 而不是「当时最新」：出生时还没有 entry，等第一条消息
      // 进来再种就已经晚了 —— 那条会被吞成 0 条未读。老会话靠启动时那次 seed。
      unread.begin(conversation.id);
    },
    onReport: (error, info) => console.error("[durable]", info?.kind ?? "", String(error)),
  },
  ctx,
);

// ── 时间与未读 ───────────────────────────────────────────────────────────
//
// durable 里没有 wall-clock（EntryRecord 没时间，agent.sqlite 也没时间列），所以：
// - updatedAt 只能靠每条 entry 落库时顺手记一刻（activity.js）；
// - 未读只能靠「读到哪个 EntryId」比出来（unread.js）。
// 两张表都落 data/，跟这一盘一起被拷走 —— 不落仓库根，也不进 sqlite。

const activity = await openActivity(path.join(DATA, "activity.json"));

/** 这条会话最新的 entry id。没有会话 / 没有 entry = null。 */
const latestEntryId = async (id) => {
  const conversation = await harness.conversation(id, ctx);
  if (!conversation) return null;
  const page = await conversation.entries({}, 1, undefined, ctx);
  return page.items[0]?.id ?? null;
};

/** 水位之后还有几条。minEntryId 是**包含**的下界，所以从水位 + 1 开始扫。 */
const countEntriesAfter = async (id, afterId) => {
  const conversation = await harness.conversation(id, ctx);
  if (!conversation || !Number.isFinite(afterId)) return 0;
  let count = 0;
  let cursor;
  for (;;) {
    const page = await conversation.entries({ minEntryId: afterId + 1 }, 200, cursor, ctx);
    count += page.items.length;
    if (page.next === undefined) break;
    cursor = page.next;
  }
  return count;
};

const unread = await openUnread(path.join(DATA, "read.json"), {
  entries: { latest: latestEntryId, countAfter: countEntriesAfter },
});

// 每条 entry 落库 = 这条会话动过。监听器不许阻塞、不许调 Session API —— 这里只记一刻。
harness.subscribeCommits((publication) => {
  for (const change of publication.changes) {
    if (change.type === "entry") activity.note(change.value.conversationId);
  }
});

const names = async () => (await harness.snapshot(Sessions, ctx))?.names ?? {};

const machinesOf = async () => (await harness.snapshot(Sessions, ctx))?.machines ?? {};

const setName = (conversationId, name) =>
  harness.commit(async (tx) => {
    const doc = await tx.doc(Sessions);
    doc.names[key(conversationId)] = name;
  }, ctx);

/** 会话在哪台机器上跑：`null` / 不写 = 本机，字符串 = 机器名。跟 names 一样住 Sessions 文档。 */
const setMachine = (conversationId, machine) =>
  harness.commit(async (tx) => {
    const doc = await tx.doc(Sessions);
    // 老盘的 Sessions 文档里根本没这个键（`initial` 只对新文档生效）——
    // 直接赋值就是 `Cannot set properties of undefined`，而且只有「选机器开会话」会踩。
    doc.machines ??= {};
    if (machine === null || machine === undefined) delete doc.machines[key(conversationId)];
    else doc.machines[key(conversationId)] = machine;
  }, ctx);

/**
 * 归档墓碑（ADR 0001）：`{ at, gen, purged }` 或 `null` = 抹掉（打捞了）。
 * at = 归档时刻，gen = 复印进了哪个 gen 文件，purged = 原件撕了没。
 * 住 Sessions 文档跟 names/machines 一起 —— 它是「这盘的状态」，不是 durable 的数据。
 */
const setArchived = (conversationId, mark) =>
  harness.commit(async (tx) => {
    const doc = await tx.doc(Sessions);
    doc.archived ??= {}; // 老盘的文档里没这个键，跟 machines 同一个坑
    if (mark === null) delete doc.archived[key(conversationId)];
    else doc.archived[key(conversationId)] = mark;
  }, ctx);

const archivedMarks = async () => (await harness.snapshot(Sessions, ctx))?.archived ?? {};

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
  const [namesNow, machinesNow, busy, archivedNow] = await Promise.all([names(), machinesOf(), busyMap(), archivedMarks()]);
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
      cwd: agent.cwd ?? null,
      machine: machinesNow[key(record.id)] ?? null,
      parent: record.parent ? { id: record.parent.conversationId, at: record.parent.at } : null,
      owner: record.owner ? record.owner.taskId : null,
      model: agent.model ? `${agent.model.provider}/${agent.model.modelId}` : null,
      // 生效的档位（不是“发上去的那个值”）：agent 已经过模型档位表和缺省值。
      // 面板该拿这个去画，所以列表里就得有它 —— 少一个字段，前端就只能自己猜。
      thinkingLevel: agent.thinkingLevel ?? null,
      thinkingLevels: levelsOf(model),
      // 生效的技能（从 durable 存的扩展名里读回来，不是从我们的表里读）——
      // 面板该拿这个去画勾，所以列表里就得有它。
      skills: skillNamesOf(agent),
      template: templateOf(agent),
      readOnly: !agent.tools?.some?.((tool) => WRITE_NAMES.includes(tool.name)),
      busy: busy.get(key(record.id)) ?? null,
      // 归档墓碑（ADR 0001）：null = 活着的；{ at, gen, purged } = 归档了。客户端拿这个画「已归档」。
      archived: archivedNow[key(record.id)] ?? null,
      // 这条会话最后一次落库的时刻（epoch ms）。没记过就是 null —— 宁可空，也不拿文件的
      // mtime 之类冒充（拷一份盘就能让每条会话都变成「刚刚」）。
      updatedAt: activity.at(record.id),
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

const expandHome = (p) => (p === "~" ? HOME : p.startsWith("~/") ? path.join(HOME, p.slice(2)) : p);

/** 目录浏览器：`path` 空 = 列出 start dirs；否则列某个目录的子目录。 */
const browseDirs = async (p) => {
  if (!p) {
    const dirs = [];
    for (const root of START_DIRS) {
      try {
        dirs.push({ name: root, path: await realpath(root) });
      } catch {
        // start dir 没了就当它没有，别让整张列表 500。
      }
    }
    return { path: "", dirs };
  }
  const expanded = expandHome(p);
  let real;
  try {
    real = await realpath(path.resolve(expanded));
    if (!(await stat(real)).isDirectory()) throw Error();
  } catch {
    throw withStatus(new Error(`不是目录：${p}`), 400);
  }
  const dirs = [];
  for (const entry of await readdir(real, { withFileTypes: true })) {
    if (!entry.isDirectory() || entry.name.startsWith(".") || entry.name === "node_modules") continue;
    dirs.push({ name: entry.name, path: path.join(real, entry.name) });
  }
  dirs.sort((a, b) => a.name.localeCompare(b.name));
  return { path: real, parent: path.dirname(real) !== real ? path.dirname(real) : undefined, dirs: dirs.slice(0, 500) };
};

/** 校验会话的 cwd：展开 ~，realpath 后必须是个存在的目录。返回规范绝对路径。 */
const resolveCwd = async (p) => {
  const expanded = expandHome(String(p).trim());
  let real;
  try {
    real = await realpath(path.resolve(START_DIRS[0] ?? HERE, expanded));
    if (!(await stat(real)).isDirectory()) throw Error();
  } catch {
    throw withStatus(new Error(`cwd 必须是这台机器上一个存在的文件夹：${p}`), 400);
  }
  return real;
};

/** 校验远端机器的 cwd：realpath 后必须是个存在的目录。返回那台机器上的规范绝对路径。 */
const resolveRemoteCwd = async (machine, p) => {
  const env = await envForMachine(machine, "~");
  const canon = await env.canonicalPath(String(p).trim() || "~", ctx);
  if (!canon.ok) {
    throw withStatus(new Error(`cwd 必须是「${machine.name}」上一个存在的文件夹：${p}`), 400);
  }
  const info = await env.fileInfo(canon.value, ctx);
  if (!info.ok || info.value.kind !== "directory") {
    throw withStatus(new Error(`cwd 必须是「${machine.name}」上一个存在的文件夹：${p}`), 400);
  }
  return canon.value;
};

/** 远端目录浏览器：`path` 空 = 那台机器的 home；否则列那台机器某个目录的子目录。 */
const browseRemoteDirs = async (machine, p) => {
  const env = await envForMachine(machine, "~");
  const target = String(p).trim() || "~";
  const canon = await env.canonicalPath(target, ctx);
  if (!canon.ok) throw withStatus(new Error(`不是目录：${p || "~"}`), 400);
  const listed = await env.listDir(canon.value, ctx);
  if (!listed.ok) throw withStatus(new Error(`不是目录：${p || "~"}`), 400);
  const dirs = listed.value
    .filter((e) => e.kind === "directory" && !e.name.startsWith(".") && e.name !== "node_modules")
    .map((e) => ({ name: e.name, path: e.path }));
  dirs.sort((a, b) => a.name.localeCompare(b.name));
  const parent = path.posix.dirname(canon.value);
  return {
    path: canon.value,
    parent: parent !== canon.value ? parent : undefined,
    dirs: dirs.slice(0, 500),
  };
};

const device = async () => ({
  self: process.env.NOUS_SELF ?? "/",
  origin: process.env.NOUS_ORIGIN ?? null,
  node: process.version,
  startedAt,
  model: defaultModel,
  faux,
  data: DATA,
  startDirs: START_DIRS,
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

  if (req.method === "GET" && parts[0] === "v1" && parts[1] === "machines") {
    return json(res, 200, await machines(Boolean(url.searchParams.get("fresh"))));
  }

  if (req.method === "GET" && parts[0] === "v1" && parts[1] === "fs" && parts[2] === "dirs") {
    try {
      const machineName = String(url.searchParams.get("machine") ?? "").trim();
      const p = String(url.searchParams.get("path") ?? "");
      if (machineName) {
        const machine = await resolveMachine(machineName);
        if (!machine) return json(res, 400, { error: `没有这台机器：${machineName}` });
        return json(res, 200, await browseRemoteDirs(machine, p));
      }
      return json(res, 200, await browseDirs(p));
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

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

  // ── 技能 ────────────────────────────────────────────────────────────────
  //
  // 全套 `/v1/skills*` 在 `skills-api.js`。这里只有接线：`root` 定死，测试注不进来。
  // **必须放在 `parts[1] === "extensions"` 那一串路由之前** —— 老代码踩过：
  // 只看 `parts[1]` 的列表接口会把 `GET /v1/extensions/sync` 先接走。
  //
  // 老的三条（`GET /v1/skills` 编目版、`POST refresh|install|uninstall`）已删干净：
  // 仓库里有什么就是装着什么，没有「装」这个动作，也没有上游编目要刷新。
  if (parts[0] === "v1" && parts[1] === "skills") {
    try {
      return await handleSkills({
        req,
        res,
        url,
        parts,
        root: SKILLS_ROOT,
        skills,
        repo,
        data: DATA,
        // 变体那一层：绑定表 + worktree + 变体分支发布；改完让上层重建名册。
        variants: skillVariants,
        reload: rebuildSkills,
      });
    } catch (error) {
      // handleSkills 自己写响应；走到这儿说明是它没接住的意外（读 body 炸了之类）。
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  if (parts[0] === "v1" && parts[1] === "prompts") {
    try {
      return await handlePrompts({ req, res, url, parts, root: PROMPTS_ROOT, prompts, repo });
    } catch (error) {
      // handlePrompts 自己写响应；走到这儿说明是它没接住的意外。
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  if (parts[0] === "v1" && parts[1] === "presets") {
    try {
      return await handlePresets({
        req,
        res,
        url,
        parts,
        repoRoot: PRESETS_ROOT,
        sections: presetSections,
        presetsOf,
        repo,
        ctx: presetCtx,
        ultimateCtx,
      });
    } catch (error) {
      // handlePresets 自己写响应；走到这儿说明是它没接住的意外。
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  // 触发器目录（只读）：终极模板要引用触发器、要照它的声明校验参数、要照它画表单。
  // 真相在另一个仓（Nous-Trigger）的 index.json —— 这一层只是投影，不维护第二份名单。
  if (parts[0] === "v1" && parts[1] === "triggers" && (req.method ?? "GET") === "GET") {
    try {
      const catalog = await triggerCatalog();
      return json(res, 200, {
        triggers: [...catalog.values()],
        root: process.env.NOUS_TRIGGER ?? "/opt/Nous-Trigger",
      });
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  // 扩展类别：改 nous.json + 重扫 index.json（立刻生效）；commit/push 走 /sync。
  // 放在 GET /v1/extensions 前面 —— 否则 `GET /v1/extensions/sync` 会被下面那条
  // 只看 parts[1] 的列表接口先接走。
  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "category") {
    if (req.method !== "POST") return json(res, 405, { error: "类别只有 POST" });
    const body = JSON.parse((await readBody(req)) || "{}");
    try {
      const updated = await serializeRepoWrite(EXTEND, () =>
        setExtensionCategories(String(body.name ?? ""), body.category),
      );
      const catalog = JSON.parse(await readFile(path.join(EXTEND, "index.json"), "utf8")).extensions;
      return json(res, 200, {
        extension: updated,
        catalog,
        categories: await readCategories(EXTEND),
        protected: PROTECTED_CATEGORIES,
      });
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  // 类别词表：建一个还没挂到任何扩展上的类别 / 删一个（连同从扩展上摘掉）。
  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "categories") {
    if (req.method === "GET") {
      return json(res, 200, { categories: await readCategories(EXTEND), protected: PROTECTED_CATEGORIES });
    }
    if (req.method !== "POST") return json(res, 405, { error: "类别词表只有 GET/POST" });
    const body = JSON.parse((await readBody(req)) || "{}");
    try {
      const result = await serializeRepoWrite(EXTEND, () => mutateExtensionCategories(body));
      const catalog = JSON.parse(await readFile(path.join(EXTEND, "index.json"), "utf8")).extensions;
      return json(res, 200, { ...result, catalog, protected: PROTECTED_CATEGORIES });
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  // 扩展源码：列表 / 读 / 写（只落盘）。发布是单独一条路，见下面 publish。
  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "files") {
    if (req.method !== "GET") return json(res, 405, { error: "文件列表只有 GET" });
    try {
      const name = String(url.searchParams.get("name") ?? "");
      const variant = url.searchParams.get("variant") || null;
      return json(res, 200, { name, variant, files: await listExtensionFiles(name, variant) });
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "file") {
    if (req.method === "GET") {
      try {
        const name = String(url.searchParams.get("name") ?? "");
        const rel = String(url.searchParams.get("path") ?? "");
        const variant = url.searchParams.get("variant") || null;
        return json(res, 200, await readExtensionFile(name, rel, variant));
      } catch (error) {
        return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
      }
    }
    if (req.method === "PUT") {
      const body = JSON.parse((await readBody(req)) || "{}");
      try {
        // 写盘不占 git 锁（不碰 index），但和类别写共享一条链：index.mjs 与 commit 会抢。
        const saved = await serializeRepoWrite(EXTEND, () =>
          writeExtensionFile({
            name: body.name,
            rel: body.path,
            text: body.text,
            base: body.base,
            variant: body.variant ?? null,
          }),
        );
        return json(res, 200, saved);
      } catch (error) {
        return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
      }
    }
    return json(res, 405, { error: "文件只有 GET / PUT" });
  }

  // 发布 = 校验 + commit + push。跟类别那条 debounce 分开：这条只由人显式点。
  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "publish") {
    if (req.method !== "POST") return json(res, 405, { error: "发布只有 POST" });
    const body = JSON.parse((await readBody(req)) || "{}");
    try {
      // 带 variant 就是「发布变体」：在那棵树上 commit + push 到它自己的分支，主树不碰。
      const result = await serializeRepoWrite(EXTEND, () =>
        body.variant
          ? publishVariant({ name: body.name, variant: body.variant, message: body.message })
          : publishExtensions({ name: body.name, message: body.message }),
      );
      // 发布之后立刻重装：commit + push 只挪了 git 指针，进程里跑的仍是旧代码。
      // 不在这里装一次，页头会挂 30 秒「⚠ 不一致」，而改代码的人正要马上看效果。
      let reload = null;
      if (result.committed) {
        try {
          reload = await serializeRepoWrite(EXTEND, () => reloadExtensionsNow());
        } catch (error) {
          // 装不上不算发布失败：代码已经在仓库里了，页面会按「装载失败」红着报。
          reload = { ok: false, error: String(error?.message ?? error) };
        }
      }
      return json(res, 200, { ...result, reload });
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  // 重试装载：人点一下，立刻按磁盘现状重装（不等 30 秒轮询）。
  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "reload") {
    if (req.method !== "POST") return json(res, 405, { error: "重装只有 POST" });
    try {
      return json(res, 200, await serializeRepoWrite(EXTEND, () => reloadExtensionsNow()));
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  // 变体：绑目录 / 开分支树 / 解绑 / 拆树。**只动本机清单和本机工作树，不 commit 不 push**
  // （参数变体是另一回事：它写在 nous.json 里，跟着仓库走，用代码抽屉改，不走这个接口）。
  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "variant") {
    // 转正前先问一句「谁还在用这个变体名」—— 解绑会让那些会话装载失败，那种失败没人能自己诊断。
    if (parts[3] === "usage") {
      if (req.method !== "GET") return json(res, 405, { error: "usage 只有 GET" });
      try {
        const key = String(url.searchParams.get("key") ?? "");
        if (!key.includes(VARIANT_SEP)) return json(res, 400, { error: "key 得是 名称@变体" });
        return json(res, 200, { key, sessions: await sessionsUsing(key) });
      } catch (error) {
        return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
      }
    }
    if (req.method !== "POST") return json(res, 405, { error: "变体只有 POST" });
    const body = JSON.parse((await readBody(req)) || "{}");
    const family = String(body.name ?? "");
    const id = String(body.id ?? "");
    const action = String(body.action ?? "bind");
    try {
      const result = await serializeRepoWrite(EXTEND, async () => {
        const manifest = await variantManifestOf(family);
        let worktree = null;
        if (action === "bind") {
          await bindVariantDir({ family, manifest, id, dir: String(body.dir ?? ""), params: body.params });
        } else if (action === "unbind") {
          await unbindVariant(family, id);
        } else if (action === "branch") {
          worktree = await createVariantWorktree({ family, id, branch: body.branch });
          await bindVariantDir({ family, manifest, id, dir: worktree.dir, branch: worktree.branch });
        } else if (action === "drop") {
          // 删分支：树还在就先拆（代码在树里，分支删了树就是孤儿）。
          const branch = String(body.branch ?? "").trim();
          if (await stat(variantWorktreePath(family, id)).catch(() => null)) {
            throw withStatus(new Error("树还在 —— 先拆树再删分支"), 409);
          }
          worktree = await dropVariantBranch(branch);
        } else if (action === "prune") {
          worktree = await removeVariantWorktree(family, id);
          await unbindVariant(family, id);
        } else {
          throw withStatus(new Error(`不认识的变体操作：${action}`), 400);
        }
        // 绑定变了就立刻重装：不等 30 秒轮询（否则页面说「装上了」而进程里还没有）。
        const reload = await reloadExtensionsNow();
        const sources = await variantSources(family, manifest);
        return {
          ok: true,
          action,
          worktree,
          variants: sources.map((source) => {
            const key = variantKey(family, source.id);
            return { ...source, key, installed: extraLoaded.has(key), load: extensionLoad.get(key) ?? null };
          }),
          load: reload.load,
        };
      });
      return json(res, 200, result);
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  // 归档 / 恢复 / 彻底删除：这三个会真改仓库（commit + push），所以都带二次确认（前端）。
  if (parts[0] === "v1" && parts[1] === "extensions" && ["archive", "restore", "purge"].includes(parts[2])) {
    if (req.method !== "POST") return json(res, 405, { error: "归档只有 POST" });
    const body = JSON.parse((await readBody(req)) || "{}");
    const name = String(body.name ?? "");
    const task = { archive: archiveExtension, restore: restoreExtension, purge: purgeExtension }[parts[2]];
    try {
      return json(res, 200, await serializeRepoWrite(EXTEND, () => task(name)));
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  // 本地装卸：只动这一台的工作区 + data/ 里的本机清单，不 commit、不 push。
  if (parts[0] === "v1" && parts[1] === "extensions" && (parts[2] === "uninstall" || parts[2] === "install")) {
    if (req.method !== "POST") return json(res, 405, { error: "装卸只有 POST" });
    const body = JSON.parse((await readBody(req)) || "{}");
    const name = String(body.name ?? "");
    try {
      const result = await serializeRepoWrite(EXTEND, () =>
        parts[2] === "uninstall" ? localUninstall(name) : localInstall(name),
      );
      return json(res, 200, result);
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  // 新建扩展：建目录 + 空骨架，立刻重装（新扩展默认就算「这台装着」）。
  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "create") {
    if (req.method !== "POST") return json(res, 405, { error: "新建只有 POST" });
    const body = JSON.parse((await readBody(req)) || "{}");
    try {
      const made = await serializeRepoWrite(EXTEND, () => createExtension(body));
      // 装不上不算建失败：目录已经在磁盘上了，页面会把它标成「装载失败」并给出重试。
      let load = null;
      try {
        load = await serializeRepoWrite(EXTEND, () => reloadExtensionsNow());
      } catch (error) {
        load = { ok: false, error: String(error?.message ?? error) };
      }
      const catalog = JSON.parse(await readFile(path.join(EXTEND, "index.json"), "utf8")).extensions;
      return json(res, 200, { ...made, catalog, categories: await readCategories(EXTEND), load });
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "duplicate") {
    if (req.method !== "POST") return json(res, 405, { error: "复制只有 POST" });
    const body = JSON.parse((await readBody(req)) || "{}");
    try {
      const made = await serializeRepoWrite(EXTEND, () => duplicateExtension(body));
      // 和「新建」一样，造完就重装一次：目录已经在盘上了，装不上的话页面会标成装载失败并给重试。
      let load = null;
      try {
        load = await serializeRepoWrite(EXTEND, () => reloadExtensionsNow());
      } catch (error) {
        load = { ok: false, error: String(error?.message ?? error) };
      }
      const catalog = JSON.parse(await readFile(path.join(EXTEND, "index.json"), "utf8")).extensions;
      return json(res, 200, { ...made, catalog, categories: await readCategories(EXTEND), load });
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  // 仓库状态 / 手动同步。客户端 debounce 之后打 POST；刷新时先 GET 一次看有没有欠账。
  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "pending") {
    if (req.method !== "GET") return json(res, 405, { error: "待同步清单只有 GET" });
    try {
      if (parts[3] === "diff") {
        return json(res, 200, await pendingFileDiff(EXTEND, url.searchParams.get("path")));
      }
      if (parts[3] === "commit") {
        return json(res, 200, await pendingCommit(EXTEND, url.searchParams.get("hash")));
      }
      return json(res, 200, await pendingState(EXTEND));
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "sync") {
    if (req.method === "GET") {
      try {
        return json(res, 200, await repoState(EXTEND));
      } catch (error) {
        return json(res, 500, { error: String(error?.message ?? error) });
      }
    }
    if (req.method === "POST") {
      const body = JSON.parse((await readBody(req)) || "{}");
      try {
        // 两条路：
        //   · 带 paths = 人在弹窗里显式勾了哪几个改动单元 → 只提交这些（代码过语法闸）；
        //   · 不带 paths = 类别那 8 秒 debounce → 只提交词表 / 清单。
        const picked = Array.isArray(body.paths) ? body.paths : null;
        const result = await serializeRepoWrite(EXTEND, () =>
          picked ? syncRepo(EXTEND, body.message, { paths: picked }) : syncRepo(EXTEND, body.message, { only: "categories" }),
        );
        return json(res, 200, result);
      } catch (error) {
        return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
      }
    }
    return json(res, 405, { error: "同步只有 GET 和 POST" });
  }

  // ── B 路线的答案：不问仓库文件，问**正在跑的进程** ───────────────────────────
  //
  // 仓库里有什么（catalog）一并带上，因为块知道仓库在哪：于是页面一次就能看出
  // 「菜谱上有、厨房里没有」那种漂移 —— 仓库改了但没重启，别让页面汇报一个假的版本。
  if (req.method === "GET" && parts[0] === "v1" && parts[1] === "extensions") {
    // `tracked` = 这个目录在 HEAD 里。false 就是「只在本地，别人 pull 不到」（未发布）。
    const inHead = new Set();
    try {
      const out = (await run("git", ["-C", EXTEND, "ls-tree", "--name-only", "HEAD"], { timeout: 15_000 })).stdout;
      for (const line of out.split("\n")) if (line.trim()) inHead.add(line.trim());
    } catch {
      // git 不在：宁可说不准，也不要拿一个假 true 去撒谎。
    }
    const catalog = (await readExtendCatalog()).map((item) => ({ ...item, tracked: inHead.has(item.name) }));
    const installed = registry.snapshot().installed().map((extension) => extension.name);
    // 自带的（CodingTools 那四件套）单列：不是我们开发的，不该跟自研的混一个列表 ——
    // 它不在仓库里，页面也就不该给它「装卸」之类的按钮。
    const builtins = [CodingTools].map((extension) => extension?.name).filter(Boolean);
    // 变体在 registry 里是独立名字，但**卡片是一张**（按本体分组）——
    // 「装了哪些扩展」仍只列本体，变体在 `variants` 里按本体归位。
    const ours = installed.filter(
      (name) => !NAMEPLATE_PREFIXES.some((prefix) => name.startsWith(prefix)) && !builtins.includes(name) && !name.includes(VARIANT_SEP),
    );
    const variants = {};
    for (const item of catalog) {
      const sources = await variantSources(item.name, item);
      if (!sources.length) continue;
      variants[item.name] = await Promise.all(
        sources.map(async (source) => {
          const key = variantKey(item.name, source.id);
          // 拆树前要能问一句「这条分支上有几笔没并进 main」—— 有才值得拦一下。
          let unmerged = 0;
          let remoteBranch = false;
          if (source.branch) {
            unmerged = Number(
              (await run("git", ["-C", EXTEND, "rev-list", "--count", `main..${source.branch}`], { timeout: 15_000 })
                .then((out) => out.stdout.trim())
                .catch(() => 0)) || 0,
            );
            remoteBranch = await run("git", ["-C", EXTEND, "rev-parse", "--verify", `refs/remotes/origin/${source.branch}`], {
              timeout: 15_000,
            })
              .then(() => true)
              .catch(() => false);
          }
          return { ...source, key, unmerged, remoteBranch, installed: extraLoaded.has(key), load: extensionLoad.get(key) ?? null };
        }),
      );
    }
    const nameplates = installed.filter((name) => NAMEPLATE_PREFIXES.some((prefix) => name.startsWith(prefix)));
    let head = null;
    try {
      head = (await run("git", ["-C", EXTEND, "rev-parse", "HEAD"])).stdout.trim();
    } catch {
      // git 不在也照样回：版本那一栏就显示「不知道」，别让一个工具缺了整个接口 500。
    }
    const lock = JSON.parse(await readFile(path.join(HERE, "extensions.lock.json"), "utf8")).commit;
    const body = {
      repo: { path: EXTEND, lock, head, drift: head !== null && head !== lock, pullError: extendPullError },
      installed: ours,
      // 能装的名字全集（含变体 `名@变体`）—— 名字就是 API，别让每个块自己去变体表里拼 key。
      loadable: [
        ...ours,
        ...Object.values(variants)
          .flat()
          .filter((item) => item.installed)
          .map((item) => item.key),
      ],
      builtin: builtins,
      // 名牌单独列：那是 skills / prompts 运行时按仓里的条目生成的家族，不是独立扩展 ——
      // 混进 installed 会变成几十行里大半是生成物。
      nameplates: { prefixes: NAMEPLATE_PREFIXES, count: nameplates.length, names: nameplates },
      defaults: [nousExtension.name, skills.extension.name, prompts.extension.name],
      categories: await readCategories(EXTEND),
      protected: PROTECTED_CATEGORIES,
      catalog,
      variants,
      local: { ...(await readLocalState()), core: CORE_EXTENSIONS },
      archived: await archivedEntries(EXTEND),
      // 装载结果：页面上「装载失败」的唯一依据（空对象 = 一切正常）。
      load: Object.fromEntries(extensionLoad),
      // 对账：清单（nous.json 的 sections）说有几段，进程里**真装**了几段。
      // 只有这里能说清 —— 页面拿 index.json 的 sections 数「提示词 N 段」，
      // 而那份是手写的；代码里改了段落忘了回头改清单，页面就会照抄一个谎。
      live: (() => {
        const map = {};
        for (const item of registry.snapshot().sections()) (map[item.extension.name] ??= []).push(item.section.key);
        return map;
      })(),
    };
    // 谁在用 —— 管理页最有用的一栏，代价是每条会话读一次 agent。默认不算，?usage=1 才给。
    if (url.searchParams.get("usage") === "1") {
      const records = (await harness.commit((tx) => tx.scanConversations({}, 200, undefined), ctx)).items;
      const usage = {};
      for (const record of records) {
        const conversation = await harness.conversation(record.id, ctx);
        if (!conversation) continue;
        const agent = await conversation.agent(ctx);
        for (const { name } of agent.extensions ?? []) {
          if (!ours.includes(name)) continue;
          (usage[name] ??= []).push(record.id);
        }
      }
      body.usage = usage;
    }
    return json(res, 200, body);
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
    const items = await tree();
    // 默认藏归档的 —— 老客户端拿到的列表跟以前一个形状；?archived=1 才给全量（带墓碑，客户端自己画）。
    if (url.searchParams.get("archived") !== "1") {
      const marks = await archivedMarks();
      return json(res, 200, { items: items.filter((s) => !marks[key(s.id)]) });
    }
    return json(res, 200, { items });
  }

  // 未读：**平铺的 map**，一条会话一个键，不套 items（客户端照这个形状读）。
  // 数由引擎算 —— 客户端只显示，不自己数。
  if (req.method === "GET" && parts[0] === "v1" && parts[1] === "unread" && !parts[2]) {
    // 翻完所有页：一页 200 条，会话过了 200 条只看第一页就是静默漏掉未读。
    const records = await collectPages((cursor) =>
      harness.commit((tx) => tx.scanConversations({}, 200, cursor), ctx),
    );
    const body = {};
    for (const record of records) body[record.id] = await unread.unreadOf(record.id);
    return json(res, 200, body);
  }

  // 读到哪 = 把水位推到这条会话当前最新，回 { id, readAt, unread: 0 }。
  if (parts[0] === "v1" && parts[1] === "read") {
    if (req.method !== "POST") return json(res, 405, { error: "read 只有 POST" });
    const id = Number(parts[2]);
    if (!parts[2] || !Number.isFinite(id)) return json(res, 400, { error: "read 后面得跟会话号" });
    if (!(await harness.conversation(id, ctx))) return json(res, 404, { error: "没有这个会话", id });
    return json(res, 200, await unread.markRead(id));
  }

  // ── 归档（ADR 0001：复印 + 撕原件，agent.sqlite 永不关）───────────────────────────
  // 动词跟扩展那套同款：archive（复印+墓碑）/ purge（撕原件）/ restore（打捞）。
  if (parts[0] === "v1" && parts[1] === "archive") {
    // GET /v1/archive   浏览归档库（mcfpqm 的 storage-Archive 页吃这个）
    if (req.method === "GET" && !parts[2]) return json(res, 200, await archive.browse());

    const rid = Number(parts[2]);
    if (!parts[2] || !Number.isFinite(rid)) return json(res, 400, { error: "archive 后面得跟会话号" });
    if (req.method !== "POST") return json(res, 405, { error: "archive/:id 只有 POST" });

    // POST /v1/archive/:id/restore   {gen} 打捞：原样 INSERT 回活库 + 抹墓碑
    if (parts[3] === "restore") {
      const body = JSON.parse((await readBody(req)) || "{}");
      const gen = String(body.gen ?? "").replace(/\D/g, "");
      if (!gen) return json(res, 400, { error: "gen 得是日期（YYYYMMDD）" });
      const result = await archive.restore({ rootId: rid, gen });
      await setArchived(rid, null);
      return json(res, 200, result);
    }
    return json(res, 404, { error: "archive/:id 下只有 restore" });
  }

  if (req.method === "POST" && parts[0] === "v1" && parts[1] === "sessions" && !parts[2]) {
    const body = JSON.parse((await readBody(req)) || "{}");
    // ── 预设打底 ────────────────────────────────────────────────────────────
    // 三态，跟 template 一条纪律：字符串 = 用这条预设；null / 不传 = 手动创建（老路径，行为不变）。
    // 预设里的字段是**初值不是裁决**：body 里显式给了的字段一个个覆盖它（hb7k3j 的 picker
    // 就是靠这条「带着初值的手动创建」）。覆盖是个键一个键的 —— 整份替换会让
    // 「只想改模型」把技能也顺手改掉。
    const presetAsked = typeof body.preset === "string" ? body.preset.trim() : null;
    // 段名是预设名的一部分（`Session-Template/weekly-report`）：两段可以各有一条同名预设，
    // 光凭名字分不出是哪条。不带段名的按会话段处理 —— 那是这一页唯一的合法用途。
    const presetRef = presetAsked && presetAsked.includes("/")
      ? { section: presetAsked.slice(0, presetAsked.indexOf("/")), name: presetAsked.slice(presetAsked.indexOf("/") + 1) }
      : presetAsked
        ? { section: PRESET_SESSION_SECTION, name: presetAsked }
        : null;
    let presetBase = null;
    if (presetRef) {
      const roster = presetsOf(presetRef.section);
      const item = roster?.get(presetRef.name) ?? null;
      if (!item) return json(res, 400, { error: `没有这条预设：${presetRef.section}/${presetRef.name}` });
      if (item.specError) {
        return json(res, 400, { error: `预设「${presetRef.name}」自己写坏了：${item.specError}` });
      }
      presetBase = expandPresetSpec(item.spec);
    }
    const pick = (key) => (body[key] !== undefined ? body[key] : presetBase?.[key]);

    // machine：body 显式给 > 预设给的。`null` / 不传 = 本机；字符串 = 机器名。
    // 机器不在编目就 400 —— 跟「引用失效」一条纪律（预设可以钉一台别处没有的机器）。
    const askedMachine = pick("machine");
    let machine = null;
    if (askedMachine) {
      machine = await resolveMachine(String(askedMachine).trim());
      if (!machine) return json(res, 400, { error: `没有这台机器：${askedMachine}` });
    }

    const model = pick("model") ?? defaultModel;
    // 档位在建会话的**同一个 commit** 里落定（AgentChange 的 thinkingLevel），
    // 不是先建再 configure —— 后者中间那一下，第一条消息可能已经跑出去了。
    const thinkingLevel = fitLevel(models.getModel(model.provider, model.modelId), pick("thinkingLevel"));
    // 技能也在这个 commit 里落 —— 跟 thinkingLevel 一条纪律：先建再 configure，中间那一下
    // 第一条消息可能已经跑出去了。没装的技能名直接丢（选了也不生效，不存一个幽灵）。
    const askedSkills = pick("skills");
    const wanted = [
      ...new Set((Array.isArray(askedSkills) ? askedSkills : []).map(String)),
    ].filter((name) => skills.has(name));
    // 模板是**单值**，不是数组：一条会话最多一个 `prompt:<名>`。三态必须分清 ——
    //   字符串 = 选它；null = 显式不要；不传 = 用仓库 settings.json 的默认值。
    // 都归成「不传」的话，「不要」和「没说」会变成同一件事，页面上那句「无模板」就是假的。
    // 预设里的 template 原样搬过来（`presets.js` 的 `expandPresetSpec` 只写「键在不在」），
    // 所以 `template: null` 的预设照样是「明确不要」。
    const asked = pick("template");
    const wantedTemplate = asked === undefined ? await readDefaultTemplate() : asked === null ? null : String(asked).trim();
    const template = wantedTemplate && prompts.has(wantedTemplate) ? wantedTemplate : null;
    const askedExt = pick("extensions");
    const wantedExt = [...new Set((Array.isArray(askedExt) ? askedExt : []).map(String))].filter((name) =>
      registry.snapshot().installed().some((e) => e.name === name),
    );
    // cwd：body 显式给 > 预设给的 > 块自己的目录。预设可以带绝对路径，但那可能是
    // 别台机器上的 —— 这里不替它猜，不存在就 400 说清楚（跟「引用失效」一条纪律）。
    // 选了机器的话，cwd 是**那台机器**上的路径，校验也跑在那边（realpath + 是目录）。
    const askedCwd = pick("cwd");
    let cwd;
    try {
      cwd = machine
        ? await resolveRemoteCwd(machine, askedCwd ?? "~")
        : askedCwd ? await resolveCwd(askedCwd) : HERE;
    } catch (error) {
      return json(res, error.status ?? 400, { error: String(error?.message ?? error) });
    }
    // 清单里的 `default: true` = 新会话默认选它。README 一直这么写着，但从来没人读它 ——
    // 页面那个「默认」绿角标看的是下面这三个硬编码的名字，于是 nous.json 里那个字段
    // 只是个装饰品（改了没反应，比没有更糟）。这里把它接上：只补「不是那三个」的，
    // 重复 add 同一个扩展没意义。
    // **只在「没人说」时补**：预设写了 extensions 就是说了（`[]` 也是说了），别替它加东西。
    if (askedExt === undefined) {
      const baseDefaults = [CodingTools.name, nousExtension.name, skills.extension.name, prompts.extension.name];
      const installedNow = registry.snapshot().installed().map((e) => e.name);
      for (const manifest of await readExtendCatalog().catch(() => [])) {
        if (manifest.default !== true) continue;
        if (baseDefaults.includes(manifest.name) || !installedNow.includes(manifest.name)) continue;
        if (!wantedExt.includes(manifest.name)) wantedExt.push(manifest.name);
      }
    }
    const created = await harness.createConversation(
      {
        ownership: { kind: "ownerless" },
        agent: {
          cwd,
          model,
          ...(thinkingLevel ? { thinkingLevel } : {}),
          ...(() => {
          const adds = [
            ...(wanted.length ? wanted.map(skillExtension) : []),
            ...(template ? [promptExtension(template)] : []),
            ...(wantedExt.length ? wantedExt.map((n) => registry.snapshot().extension(n)) : []),
          ];
          return adds.length ? { extensions: { add: adds } } : {};
        })(),
        },
      },
      ctx,
    );
    if (body.name) await setName(created.id, String(body.name).slice(0, 120));
    if (machine) await setMachine(created.id, machine.name);
    // 只读：`createConversation` 没有这个字段（它只认 model / thinkingLevel / extensions），
    // 所以建完立刻 configure 一次。这不是「先建再配」那种坏味道 —— 那条纪律要防的是
    // 「第一条消息已经跑出去了配置还没落」，而这里第一条消息还没发生（调用方拿到 201 才发）。
    const askedReadOnly = pick("readOnly");
    if (askedReadOnly === true) {
      await created.configure({ modelTools: { remove: WRITE_TOOLS } }, ctx);
    }
    const agent = await created.agent(ctx);
    return json(res, 201, {
      id: created.id,
      name: (await names())[key(created.id)],
      cwd: agent.cwd ?? null,
      machine: machine?.name ?? null,
      model: agent.model ? `${agent.model.provider}/${agent.model.modelId}` : null,
      thinkingLevel: agent.thinkingLevel ?? null,
      skills: skillNamesOf(agent),
      template: templateOf(agent),
      extensions: extensionNamesOf(agent),
      // 只读也是**生效值**：上面那次 configure 到底成没成，回给前端别让人猜。
      readOnly: !agent.tools?.some?.((tool) => WRITE_NAMES.includes(tool.name)),
      // 预设回的出处：这次是哪条预设开的（`null` = 手动创建的）。前端拿它显一条小标签。
      preset: presetRef ? `${presetRef.section}/${presetRef.name}` : null,
      missing: skills.missing(wanted),
      // 勾了但没装 / 盘上刚删掉：说清楚，别静默降级成「没有扩展」。
      ...(() => {
        const asked = [...new Set((Array.isArray(askedExt) ? askedExt : []).map(String))];
        const gone = asked.filter((name) => !registry.snapshot().installed().some((e) => e.name === name));
        return gone.length ? { missingExtensions: gone } : {};
      })(),
      // 勾了但没装 / 盘上刚删掉：说清楚，别静默降级成「没有模板」。
      ...(wantedTemplate && !template ? { missingTemplates: [wantedTemplate] } : {}),
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

    // POST /v1/sessions/:id/archive   归档 = 复印进 gen + 写墓碑（活库的行不动，purge 才动）
    if (req.method === "POST" && rest === "archive") {
      const busy = await busyMap();
      if (busy.get(key(id))) return json(res, 409, { error: "会话正在忙，等它说完再归档", id, busy: busy.get(key(id)) });
      // 闭包纪律：有活后代不归 —— 爹的行搬走了，儿子的祖先链就断了。
      // archive.js 的复印侧不查这个（它只管闭包算得对），这里拦：归档后列表里不该再有指着爹的孩子。
      const records = await collectPages((cursor) => harness.commit((tx) => tx.scanConversations({}, 200, cursor), ctx));
      const liveKids = records.filter((r) => r.parent?.conversationId === id).map((r) => r.id);
      if (liveKids.length) {
        return json(res, 409, { error: `它还有 ${liveKids.length} 条活分支，先归档它们或改用整子树归档`, id, children: liveKids });
      }
      const result = await archive.archive({ rootId: id });
      await setArchived(id, { at: Date.now(), gen: result.gen, purged: false });
      return json(res, 200, result);
    }

    // POST /v1/sessions/:id/purge   真删：复印件完整才动手，事务 DELETE 九张表 + 墓碑记 purged
    if (req.method === "POST" && rest === "purge") {
      const busy = await busyMap();
      if (busy.get(key(id))) return json(res, 409, { error: "会话正在忙，等它说完再撕", id });
      const mark = (await archivedMarks())[key(id)];
      if (!mark) return json(res, 409, { error: "它还没归档过 —— 先归档再撕，别裸删", id });
      if (mark.purged) return json(res, 409, { error: "原件已经撕过了", id });
      const result = await archive.purge({ rootId: id });
      await setArchived(id, { ...mark, purged: true });
      return json(res, 200, result);
    }

    if (req.method === "POST" && rest === "config") {
      const body = JSON.parse((await readBody(req)) || "{}");
      if (typeof body.readOnly === "boolean") {
        // 只读不是「拦住」，是根本不把会写的工具递给模型。
        // 走 modelTools（按名字过滤），不走 tools.remove（那个要的是工具对象）。
        await conversation.configure({ modelTools: body.readOnly ? { remove: WRITE_TOOLS } : null }, ctx);
      }
      // 模型：`"provider/modelId"` 换掉这条会话的模型。
      // 建会话时 model 是直接塞进 createConversation 的，改已有会话只能走 configure ——
      // pi-durable 的 AgentChange 里有 model 这个字段（README 里 configure({ model }) 就是例子）。
      // 换完要按**新模型**重夹一次档位：有的模型没有 high，留着旧值会落一个用不了的档。
      if (body.model !== undefined) {
        const wanted = String(body.model ?? "").trim();
        const at = wanted.indexOf("/");
        if (at <= 0 || at === wanted.length - 1) {
          return json(res, 400, { error: `model 得是 "provider/modelId"：${wanted}` });
        }
        const provider = wanted.slice(0, at);
        const modelId = wanted.slice(at + 1);
        if (!models.getModel(provider, modelId)) {
          return json(res, 400, { error: `没有这个模型：${wanted}` });
        }
        await conversation.configure({ model: { provider, modelId } }, ctx);
        const now = await conversation.agent(ctx);
        const fit = fitLevel(models.getModel(provider, modelId), now.thinkingLevel);
        if (fit) await conversation.configure({ thinkingLevel: fit }, ctx);
      }
      // 档位改的是**这条对话**，不是全局：configure 落的 AgentState 跟着会话走，
      // 下一条消息就吃到新值（不用重建会话，也就不会丢历史）。
      if (body.thinkingLevel !== undefined) {
        const now = await conversation.agent(ctx);
        const fit = fitLevel(now.model ? models.getModel(now.model.provider, now.model.modelId) : undefined, body.thinkingLevel);
        // 夹不成（模型认不出、或发来的不是字符串）就当没收到，绝不把没验证的值落盘。
        if (fit) await conversation.configure({ thinkingLevel: fit }, ctx);
      }
      // 技能：**相对改法**，不是把完整数组写回去 —— 完整数组会把这条会话的扩展选择
      // 冻住，宿主以后往默认选择里加东西就再也进不来了。
      if (Array.isArray(body.skills)) {
        const current = skillNamesOf(await conversation.agent(ctx));
        const wanted = [...new Set(body.skills.map(String))].filter((name) => skills.has(name));
        await conversation.configure(
          {
            extensions: { remove: current.map(skillExtension), add: wanted.map(skillExtension) },
          },
          ctx,
        );
      }
      if (Array.isArray(body.extensions)) {
        // 只动**真扩展**：名牌（skill:/prompt:）不归这里管 —— 它们各有各的写入口，
        // 否则「换个扩展」会顺手把这条会话的技能和模板一起清掉。
        const current = extensionNamesOf(await conversation.agent(ctx));
        const wanted = [...new Set(body.extensions.map(String))].filter((name) =>
          registry.snapshot().installed().some((e) => e.name === name),
        );
        await conversation.configure(
          {
            extensions: {
              remove: current.map((n) => registry.snapshot().extension(n)).filter(Boolean),
              add: wanted.map((n) => registry.snapshot().extension(n)).filter(Boolean),
            },
          },
          ctx,
        );
      }
      // 模板：**单值**。`null` = 清掉，字符串 = 换掉。先移旧的再加新的 —— 一条会话永远只有一个。
      if (body.template !== undefined) {
        const now = await conversation.agent(ctx);
        const current = templateOf(now);
        const wanted =
          body.template === null ? null : String(body.template).trim() ? String(body.template).trim() : null;
        if (wanted !== null && !prompts.has(wanted)) {
          return json(res, 400, { error: `没有这条模板：${wanted}` });
        }
        if (wanted !== current) {
          await conversation.configure(
            {
              extensions: {
                remove: current ? [promptExtension(current)] : [],
                add: wanted ? [promptExtension(wanted)] : [],
              },
            },
            ctx,
          );
        }
      }
      // machine：`null` = 回本机，字符串 = 换机器（机器得在编目里）。改了机器，cwd 跟着重验。
      if (body.machine !== undefined) {
        if (body.machine === null) {
          await setMachine(id, null);
        } else {
          const wanted = String(body.machine).trim();
          if (!wanted) {
            await setMachine(id, null);
          } else {
            const found = await resolveMachine(wanted);
            if (!found) return json(res, 400, { error: `没有这台机器：${wanted}` });
            await setMachine(id, found.name);
          }
        }
      }
      // cwd 可以换（AgentChange 支持）：`null` = 清回缺省，字符串 = 换掉。
      // 换了机器之后，cwd 是**那台机器**上的路径，校验也跑在那边（realpath + 是目录）。
      if (body.cwd !== undefined) {
        try {
          const machineName = (await machinesOf())[key(id)] ?? null;
          const target = machineName ? await resolveMachine(machineName) : null;
          let wanted;
          if (body.cwd === null) {
            wanted = null;
          } else if (!String(body.cwd).trim()) {
            wanted = null;
          } else if (target) {
            wanted = await resolveRemoteCwd(target, body.cwd);
          } else {
            wanted = await resolveCwd(body.cwd);
          }
          await conversation.configure({ cwd: wanted }, ctx);
        } catch (error) {
          return json(res, error.status ?? 400, { error: String(error?.message ?? error) });
        }
      }
      // 回的是**生效值**，不是请求值：前端拿它重画，面板和真相就不会分叉。
      const agent = await conversation.agent(ctx);
      return json(res, 200, {
        id,
        model: agent.model ? `${agent.model.provider}/${agent.model.modelId}` : null,
        thinkingLevel: agent.thinkingLevel ?? null,
        skills: skillNamesOf(agent),
        template: templateOf(agent),
        extensions: extensionNamesOf(agent),
        cwd: agent.cwd ?? null,
        machine: (await machinesOf())[key(id)] ?? null,
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
      // pi.system 是工具目录 + 提示词增量：界面上不收它（一整屏噪音），但调试要看得见。
      // `?system=1` 给两样，别搞混：
      //   items[].delta —— 这一条**自己**带了什么（增量原文）
      //   system        —— 回放整条会话后**此刻生效**的提示词（人真正想看的是这个）
      const withSystem = url.searchParams.get("system") === "1";
      const system = withSystem ? await replayPrompt(conversation, ctx) : undefined;
      return json(res, 200, {
        items: page.items
          .filter((entry) => withSystem || entry.kind !== "pi.system")
          .map((entry) =>
            entry.kind === "pi.system"
              ? (() => {
                  // SystemEntry 的正文在 `model[0]`（一个 SystemMessage，content 是空的），
                  // 不在 entry 自己身上 —— 写成 entry.sections 会永远拿到 null。
                  const message = entry.model?.[0] ?? {};
                  return {
                    id: entry.id,
                    kind: entry.kind,
                    delta: {
                      sections: message.sections ?? null,
                      toolsAdded: (message.toolsAdded ?? []).map((tool) => tool.name),
                      toolsRemoved: (message.toolsRemoved ?? []).map((tool) => tool.name),
                    },
                  };
                })()
              : { id: entry.id, kind: entry.kind, text: textOfEntry(entry), parent: entry.head ?? null },
          ),
        ...(system ? { system } : {}),
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

// 老会话的未读水位补一次：种在「当时最新」，之后进来的才算未读。
// 不补也能用（没有水位的会话老实回 0 条未读），但先补好，客户端拿到的第一份 /v1/unread
// 就是全 0，而不是要等谁先发一条消息。放 listen 之后、不挡启动 —— 几百条会话要挨个问
// 一遍最新 id，而且是翻完所有页（一页 200 条）。
const seedWatermarks = async () => {
  const records = await collectPages((cursor) =>
    harness.commit((tx) => tx.scanConversations({}, 200, cursor), ctx),
  );
  let seeded = 0;
  for (const record of records) if (await unread.seed(record.id)) seeded += 1;
  if (seeded) console.log(`[qrd3wh] 未读水位补了 ${seeded} 条会话`);
};
void seedWatermarks().catch((error) => console.error("[unread] 补水位失败", error));

const shutdown = async () => {
  server.close();
  // 攒着没写的时刻/水位先落下，再关库。
  await Promise.allSettled([activity.flush(), unread.flush()]);
  await harness.close(ctx).catch(() => {});
  process.exit(0);
};
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);

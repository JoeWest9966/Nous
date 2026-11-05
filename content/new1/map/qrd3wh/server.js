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
//   POST /v1/sessions                 {name, model, thinkingLevel, skills} → 新对话
//   POST /v1/sessions/:id/rename      {name}
//   POST /v1/sessions/:id/fork        {entryId} 或 {} （空 = 从最后一条分叉）
//   POST /v1/sessions/:id/config      {readOnly, thinkingLevel, skills} 只读 = 不给会写的工具
//   GET  /v1/skills                   已装技能 + 编目里能装的（skills.js）
//   POST /v1/skills/install           {source, entry} → 拉下来落到 data/skills/
//   POST /v1/skills/uninstall         {name}
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

import { cp, mkdir, readFile, readdir, realpath, rename, rm, stat, unlink, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { createServer } from "node:http";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
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
// 自研扩展的**源码不在这棵树里** —— 在 Nous-extend 仓库的本地副本（`NOUS_EXTEND`）。
// 块里只留两件事：去哪儿拿（下面这两行）+ 钉着哪个 commit（extensions.lock.json）。
// 用动态 import 是因为路径得能配；静态 import 只能写字面量，一换机器就炸。
const EXTEND = process.env.NOUS_EXTEND ?? "/opt/Nous-extend";
// 扩展是造扩展的函数，宿主把 api 递进去 —— 它们不自己 import pi-durable（见仓库 README）。
const api = { defineExtension, section };
const { default: createNous } = await import(`${EXTEND}/nous/index.js`);
const { createSkills, skillNamesOf, SKILL_PREFIX } = await import(`${EXTEND}/skills/index.js`);
// 回放提示词增量 —— 自己在 `prompt-log.js`，纯读，能单验。
import { replayPrompt } from "./prompt-log.js";
import { NodeExecutionEnv } from "@earendil-works/pi-durable/env/node";
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";

const ctx = BACKGROUND_CONTEXT;
const HERE = path.dirname(fileURLToPath(import.meta.url));
const run = promisify(execFile);
const PORT = Number(process.env.PORT ?? 4780);
const DATA = process.env.NOUS_DATA ?? path.join(HERE, "data");
const startedAt = Date.now();

// ── 本地装卸：这一台的私事，跟仓库没关系 ───────────────────────────────────
//
// 状态全放 data/（宿主的 watcher 跳过 data/，写它不会把进程重启；也不在
// Nous-extend 工作区里，所以不会脏到 git）。两个真相：
//   · extensions.local.json —— 「这台不装谁」是**唯一真值**，不靠「目录在不在」。
//     别人一 push、git pull 一跑，文件可能自己回来；没有这份名单扩展就悄悄复活了。
//   · extend-trash/<name>-<时间>/ —— 删掉的源码本体，点安装就搬回来。
//
// 仓库（HEAD / origin）一个字节都不动：删除只表现为工作区里的 ` D`，而 publish
// 会把删除过滤掉（见 syncExtendRepo），绝不会把「本地删除」顺手推上 GitHub。
const CORE_EXTENSIONS = ["nous", "skills"]; // 引擎启动路径硬编码 import，删了就起不来
const ARCHIVE_DIR = ".archive"; // 仓库内的归档区：点开头，index.mjs 天然跳过
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

/** 搬目录：跨设备时 rename 会 EXDEV，退回复制再删。 */
const moveDir = async (src, dst) => {
  try {
    await rename(src, dst);
  } catch (error) {
    if (error?.code !== "EXDEV") throw error;
    await cp(src, dst, { recursive: true });
    await rm(src, { recursive: true, force: true });
  }
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
  const bag = { api, defineExtension, section, dir: path.join(DATA, "skills"), registry };
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

let extendWriteChain = Promise.resolve();
let extendBusy = false;

const serializeExtendWrite = (task) => {
  const runTask = async () => {
    extendBusy = true;
    try {
      return await task();
    } finally {
      extendBusy = false;
    }
  };
  const next = extendWriteChain.then(runTask, runTask);
  extendWriteChain = next.then(() => {}, () => {});
  return next;
};

const EXTENSION_NAME_RE = /^[a-z0-9][a-z0-9._-]*$/i;
const withStatus = (error, status) => Object.assign(error, { status });

const writeFileAtomic = async (file, text) => {
  const tmp = `${file}.tmp-${process.pid}-${Date.now()}`;
  try {
    await writeFile(tmp, text, "utf8");
    await rename(tmp, file);
  } catch (error) {
    // 半截临时文件别留在仓库里 —— 下一轮 `git add -A` 会把它当真账提交。
    await unlink(tmp).catch(() => {});
    throw error;
  }
};

// 类别词表：扩展目录的 category 是「哪些类别挂在这个扩展上」，
// categories.json 是「这个仓库认得哪些类别」—— 没有它，一个刚建、还没挂到任何
// 扩展上的类别就会当场消失（建了看不见）。两边取并集，词表里的零计数类别也摆着。
//
// 类别是路径：`工具/搜索` = `工具` 底下的 `搜索`。父子关系不存，靠 `/` 推。
// 祖先会自动进词表（打上 `工具/搜索`，`工具` 就得存在），但**不会**写进扩展的
// category —— 一个扩展属于 `工具` 是因为它的标签前缀是 `工具/`，不是因为它多存了一条。
const CATEGORIES_FILE = path.join(EXTEND, "categories.json");

// 内置类别：像「全部」一样永远在 —— 删不掉、改不了名。但可以往里挂扩展、建子类别。
// 想保护别的就往这个列表里加；客户端从 API 读这份列表，不在前端再抄一遍。
const PROTECTED_CATEGORIES = ["Basics"];

const CATEGORY_SEP = "/";
const MAX_CATEGORY_SEGMENT = 40;

const splitCategory = (value) =>
  String(value ?? "")
    .split(CATEGORY_SEP)
    .map((part) => part.trim())
    .filter(Boolean);

/** 规范化一个类别路径；空串表示空。控制字符 / 超长段直接 400。 */
const normalizeCategory = (value) => {
  const parts = splitCategory(value);
  if (!parts.length) return "";
  for (const part of parts) {
    if (/[\u0000-\u001f\u007f]/.test(part)) throw withStatus(new Error("类别名里不能有控制字符"), 400);
    if (part.length > MAX_CATEGORY_SEGMENT) {
      throw withStatus(new Error(`类别名太长（一段最多 ${MAX_CATEGORY_SEGMENT} 字）：${part}`), 400);
    }
  }
  return parts.join(CATEGORY_SEP);
};

const normalizeCategories = (list) =>
  Array.isArray(list) ? list.map((c) => normalizeCategory(c)).filter(Boolean) : [];

/** `a/b/c` → [`a`, `a/b`]。 */
const ancestorsOf = (value) => {
  const parts = value.split(CATEGORY_SEP);
  return parts.slice(0, -1).map((_, index) => parts.slice(0, index + 1).join(CATEGORY_SEP));
};

const isUnder = (value, root) => value === root || value.startsWith(root + CATEGORY_SEP);

// 父在子前、同级按段比 —— 直接 localeCompare 比全串会被 `/` 的位置坑到。
const compareCategories = (a, b) => {
  const left = a.split(CATEGORY_SEP);
  const right = b.split(CATEGORY_SEP);
  const common = Math.min(left.length, right.length);
  for (let i = 0; i < common; i++) {
    const cmp = left[i].localeCompare(right[i], "zh-CN");
    if (cmp) return cmp;
  }
  return left.length - right.length;
};

const sortCategories = (list) => [...new Set(list)].sort(compareCategories);

const readCategories = async () => {
  let list = [];
  try {
    const raw = JSON.parse(await readFile(CATEGORIES_FILE, "utf8"));
    list = normalizeCategories(Array.isArray(raw) ? raw : raw?.categories);
  } catch {
    // 文件没写 / 写坏了：内置类别照样在。
  }
  return [...new Set([...list, ...PROTECTED_CATEGORIES])];
};

const writeCategories = async (list) => {
  const expanded = [];
  // 内置类别永远写进去 —— 词表被人清空也丢不了。
  for (const item of normalizeCategories([...list, ...PROTECTED_CATEGORIES])) expanded.push(...ancestorsOf(item), item);
  const unique = sortCategories(expanded);
  await writeFileAtomic(CATEGORIES_FILE, JSON.stringify({ categories: unique }, null, 2) + "\n");
  return unique;
};

/** 改一个扩展的 category（字符串数组：去空、去重、排序）。返回新清单。 */
const setExtensionCategories = async (name, categories) => {
  if (!EXTENSION_NAME_RE.test(name)) throw withStatus(new Error(`扩展名不合法：${name}`), 400);
  const file = path.join(EXTEND, name, "nous.json");
  let before;
  try {
    before = await readFile(file, "utf8");
  } catch {
    throw withStatus(new Error(`仓库里没有这个扩展：${name}`), 404);
  }
  const manifest = JSON.parse(before);
  const next = sortCategories(normalizeCategories(categories));
  const updated = { ...manifest, category: next };
  await writeFileAtomic(file, JSON.stringify(updated, null, 2) + "\n");
  try {
    // index.json 是生成物：跑生成器，顺带校验（目录名==扩展名、字段齐全）。
    await runIndexGenerator();
  } catch (error) {
    await writeFileAtomic(file, before);
    const detail = (error.stderr || error.stdout || String(error?.message ?? error)).trim();
    throw withStatus(new Error(`index.mjs 校验失败，已回滚：${detail}`), 400);
  }
  // 词表跟着收益：任何被打上的类别都该在左栏看得见，哪怕以后从所有扩展上摘掉也还在。
  // 祖先由 writeCategories 补齐 —— 挂 `工具/搜索` 就得有 `工具` 这个节点。
  await writeCategories([...(await readCategories()), ...next]);
  return updated;
};

/** 仓库根下每个扩展目录（点开头的不是，node_modules / scripts 也不是）。 */
const listExtensionDirs = async () =>
  (await readdir(EXTEND, { withFileTypes: true }))
    .filter((e) => e.isDirectory() && !e.name.startsWith(".") && e.name !== "node_modules" && e.name !== "scripts")
    .map((e) => e.name);

const readManifest = async (dir) => {
  const file = path.join(EXTEND, dir, "nous.json");
  try {
    const text = await readFile(file, "utf8");
    return { file, text, manifest: JSON.parse(text) };
  } catch {
    return null; // 没 manifest 或 JSON 坏了：这里不顺手改它
  }
};

/** 落盘之后重扫 index.json；生成器不认就整体回滚（manifest + 词表）。 */
const regenOrRollback = async (snapshots, beforeVocab) => {
  try {
    await runIndexGenerator();
  } catch (error) {
    for (const [file, text] of snapshots) await writeFileAtomic(file, text);
    await writeCategories(beforeVocab);
    const detail = (error.stderr || error.stdout || String(error?.message ?? error)).trim();
    throw withStatus(new Error(`index.mjs 校验失败，已回滚：${detail}`), 400);
  }
};

/** 类别词表操作：add 建空类别；rename 改名（有子类别就整枝改）；remove 删整枝。 */
const mutateCategories = async ({ op, name, to }) => {
  const current = await readCategories();
  const target = normalizeCategory(name);
  if (!target) throw withStatus(new Error("类别名不能为空"), 400);

  if (op === "add") {
    return { categories: await writeCategories([...current, target]), touched: 0 };
  }

  if (op !== "remove" && op !== "rename") {
    throw withStatus(new Error(`未知操作：${op}（只认 add / rename / remove）`), 400);
  }

  // 内置类别（或它的祖先）不许删、不许改名 —— 改名等于绕开删除锁。
  if (PROTECTED_CATEGORIES.some((p) => isUnder(p, target))) {
    throw withStatus(new Error(`「${target}」是内置类别，不能${op === "rename" ? "改名" : "删除"}`), 403);
  }

  const dirs = await listExtensionDirs();

  if (op === "rename") {
    const next = normalizeCategory(to);
    if (!next) throw withStatus(new Error("新类别名不能为空"), 400);
    if (next === target) return { categories: current, touched: 0 };
    if (next.startsWith(target + CATEGORY_SEP)) {
      throw withStatus(new Error(`不能把「${target}」改到它自己的子类别里`), 400);
    }

    // 改名 = 整枝改前缀（所以跨父级改名同时就是“移动”）。先扫一遍：谁挂着这枝、
    // 新名字有没有撞车。撞车不合并 —— 合了就没法反悔。
    const plans = [];
    const known = new Set(current);
    for (const dir of dirs) {
      const found = await readManifest(dir);
      if (!found) continue;
      const cats = normalizeCategories(found.manifest.category);
      for (const c of cats) known.add(c);
      if (cats.some((c) => isUnder(c, target))) plans.push({ ...found, cats });
    }
    if ([...known].some((c) => isUnder(c, next))) {
      throw withStatus(new Error(`类别「${next}」已经存在`), 409);
    }
    if (!current.some((c) => isUnder(c, target)) && plans.length === 0) {
      throw withStatus(new Error(`没有这个类别：${target}`), 404);
    }

    const renamePath = (c) => (isUnder(c, target) ? next + c.slice(target.length) : c);
    const snapshots = new Map(plans.map((plan) => [plan.file, plan.text]));
    for (const plan of plans) {
      plan.manifest.category = sortCategories(plan.cats.map(renamePath));
      await writeFileAtomic(plan.file, JSON.stringify(plan.manifest, null, 2) + "\n");
    }
    await writeCategories([...current.map(renamePath), next]);
    await regenOrRollback(snapshots, current);
    return { categories: await readCategories(), touched: plans.length };
  }

  // remove：删的是整棵子树（`工具` 连 `工具/搜索` 一起），词表和所有 nous.json 同时摘。
  const snapshots = new Map();
  let touched = 0;
  for (const dir of dirs) {
    const found = await readManifest(dir);
    if (!found) continue;
    const cats = normalizeCategories(found.manifest.category);
    const kept = cats.filter((c) => !isUnder(c, target));
    if (kept.length === cats.length) continue;
    snapshots.set(found.file, found.text);
    found.manifest.category = kept;
    await writeFileAtomic(found.file, JSON.stringify(found.manifest, null, 2) + "\n");
    touched++;
  }
  await writeCategories(current.filter((c) => !isUnder(c, target)));
  await regenOrRollback(snapshots, current);
  return { categories: await readCategories(), touched };
};

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

const FILE_KINDS = new Map([
  [".js", "javascript"],
  [".mjs", "javascript"],
  [".json", "json"],
  [".md", "markdown"],
]);
const MAX_FILE_BYTES = 512 * 1024;
const etagOf = (text) => createHash("sha1").update(text, "utf8").digest("hex").slice(0, 16);

const extensionDir = (name) => {
  const clean = String(name ?? "");
  if (!EXTENSION_NAME_RE.test(clean)) throw withStatus(new Error(`扩展名不合法：${clean}`), 400);
  const dir = path.join(EXTEND, clean);
  // 名字过了正则也可能是个文件：路径下面必须是目录，不然 readdir 会给出奇怪的东西。
  return { name: clean, dir };
};

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

/** 文件所在的 git 仓库根：主树是 EXTEND，变体是它自己那棵 checkout。 */
const gitRootOf = async (dir) => {
  try {
    return (await run("git", ["-C", dir, "rev-parse", "--show-toplevel"], { timeout: 15_000 })).stdout.trim();
  } catch {
    return EXTEND;
  }
};

/** 相对路径 → 绝对路径，且 realpath 之后还得在这个扩展目录里（符号链接是逃逸口）。 */
const resolveInsideExtension = async (name, rel, variant = null) => {
  const { name: clean, dir: baseDir } = extensionDir(name);
  const dir = variant ? (await variantRootOf(clean, variant)).dir : baseDir;
  const raw = String(rel ?? "").replace(/\\/g, "/").replace(/^\/+/, "");
  if (!raw || raw.split("/").some((part) => !part || part === "." || part === "..")) {
    throw withStatus(new Error(`路径不合法：${rel}`), 400);
  }
  const target = path.resolve(dir, raw);
  if (!target.startsWith(dir + path.sep)) throw withStatus(new Error("路径跑出了扩展目录"), 403);
  let real;
  try {
    real = await realpath(target);
  } catch {
    throw withStatus(new Error(`没有这个文件：${clean}/${raw}`), 404);
  }
  const realDir = await realpath(dir).catch(() => dir);
  if (!real.startsWith(realDir + path.sep)) throw withStatus(new Error("路径跑出了扩展目录"), 403);
  return { name: clean, dir, file: real, rel: raw, variant: variant ?? null };
};

const kindOfPath = (file) => FILE_KINDS.get(path.extname(file).toLowerCase()) ?? null;

/** 这一个扩展目录里有哪些文件 —— 只列这一页能编辑的，node_modules / 点开头的都不算。 */
const listExtensionFiles = async (name, variant = null) => {
  const { name: clean, dir: baseDir } = extensionDir(name);
  const dir = variant ? (await variantRootOf(clean, variant)).dir : baseDir;
  const exists = await stat(dir).catch(() => null);
  if (!exists?.isDirectory()) throw withStatus(new Error(`仓库里没有这个扩展：${clean}`), 404);

  const files = [];
  const walk = async (base, relBase, depth) => {
    if (depth > 4) return;
    const entries = await readdir(base, { withFileTypes: true }).catch(() => []);
    entries.sort((a, b) => a.name.localeCompare(b.name));
    for (const entry of entries) {
      if (entry.name.startsWith(".") || entry.name === "node_modules") continue;
      const rel = relBase ? `${relBase}/${entry.name}` : entry.name;
      if (entry.isDirectory()) {
        await walk(path.join(base, entry.name), rel, depth + 1);
        continue;
      }
      const kind = kindOfPath(entry.name);
      if (!kind) continue;
      const info = await stat(path.join(base, entry.name)).catch(() => null);
      if (!info || info.size > MAX_FILE_BYTES) continue;
      files.push({ path: rel, kind, size: info.size, mtime: Math.round(info.mtimeMs) });
    }
  };
  await walk(dir, "", 0);
  return files;
};

const readExtensionFile = async (name, rel, variant = null) => {
  const { name: clean, file, rel: pathname, dir } = await resolveInsideExtension(name, rel, variant);
  const kind = kindOfPath(file);
  if (!kind) throw withStatus(new Error("这一页只编辑 .js / .mjs / .json / .md"), 415);
  const info = await stat(file);
  if (info.size > MAX_FILE_BYTES) throw withStatus(new Error(`文件太大：${info.size} 字节`), 413);
  const text = await readFile(file, "utf8");
  // HEAD 里那一版 —— 抽屉的 diff 就靠它与现在的文本比。未见过的文件（未发布）给 null。
  // 注意：比的是 HEAD，不是 origin。发布推的是工作区 → 本地 commit，
  // origin 落后是另一件事（顶栏 chip 已经在说）。
  // 基线取**文件自己那棵树**的 HEAD：变体在自己的分支上，主树的 HEAD 跟它无关
  // （而且主树那份文件内容都不一样 —— 拿它当基线，diff 会显示成整篇重写）。
  let base = null;
  try {
    const gitRoot = await gitRootOf(dir);
    const relToRoot = path.relative(gitRoot, file).split(path.sep).join("/");
    base = (await run("git", ["-C", gitRoot, "show", `HEAD:${relToRoot}`], { timeout: 15_000 })).stdout;
  } catch {
    base = null;
  }
  return {
    name: clean,
    variant: variant ?? null,
    path: pathname,
    kind,
    text,
    base,
    size: info.size,
    mtime: Math.round(info.mtimeMs),
    etag: etagOf(text),
  };
};

/** 语法闸：拷成 .mjs 让 node 按 ESM 解析。只解析，不执行 —— 进程里没有 api 可递。 */
const checkSyntax = async (label, text) => {
  const dir = path.join(tmpdir(), "nous-extend-check");
  await mkdir(dir, { recursive: true });
  const file = path.join(dir, `${process.pid}-${Date.now()}-${Math.random().toString(36).slice(2)}.mjs`);
  try {
    await writeFile(file, text, "utf8");
    await run(process.execPath, ["--check", file], { timeout: 15_000 });
  } catch (error) {
    const lines = (error.stderr || error.stdout || String(error?.message ?? error))
      .trim()
      // 报错里是 /tmp 那个临时路径，换回仓库里的路径，人才能对上号。
      .replaceAll(file, label)
      .split("\n");
    // 切到错误类型那一行为止：再往下是 checkSyntax 自己的调用栈，对改代码的人零信息量。
    const stop = lines.findIndex((line) => /^(SyntaxError|ReferenceError|TypeError)\b/.test(line.trim()));
    const detail = (stop === -1 ? lines : lines.slice(0, stop + 1)).slice(0, 8).join("\n");
    throw withStatus(new Error(`${label} 语法错误：\n${detail}`), 400);
  } finally {
    await unlink(file).catch(() => {});
  }
};

/** 写一个文件：立刻落盘，**不提交**。nous.json 额外过一遍 index.mjs（不过就回滚）。 */
const writeExtensionFile = async ({ name, rel, text, base, variant = null }) => {
  const { name: clean, file, rel: pathname } = await resolveInsideExtension(name, rel, variant);
  const kind = kindOfPath(file);
  if (!kind) throw withStatus(new Error("这一页只编辑 .js / .mjs / .json / .md"), 415);
  if (typeof text !== "string") throw withStatus(new Error("text 得是字符串（哪怕是空串）"), 400);
  if (Buffer.byteLength(text, "utf8") > MAX_FILE_BYTES) throw withStatus(new Error("文件太大"), 413);

  const before = await readFile(file, "utf8");
  // base 是「我改的是哪一版」：别人先改了，这里就 409，别把对方的字盖掉。
  if (base && etagOf(before) !== String(base)) {
    throw withStatus(new Error("这个文件在别处被改过了 —— 先刷新，再改"), 409);
  }

  if (kind === "json") {
    try {
      JSON.parse(text);
    } catch (error) {
      throw withStatus(new Error(`${clean}/${pathname} 不是合法 JSON：${error.message}`), 400);
    }
  }
  if (kind === "javascript") await checkSyntax(`${clean}/${pathname}`, text);

  await writeFileAtomic(file, text);

  // nous.json 不是「普通 JSON」：目录名 == 扩展名、字段齐全、file 真在 —— 只有 index.mjs 认。
  // 变体跳过：index.mjs 扫的是主树，拿变体的清单去重扫主树的生成物是错的。
  if (pathname === "nous.json" && !variant) {
    try {
      await run(process.execPath, ["index.mjs"], { cwd: EXTEND, timeout: 30_000 });
    } catch (error) {
      await writeFileAtomic(file, before);
      const detail = (error.stderr || error.stdout || String(error?.message ?? error)).trim();
      throw withStatus(new Error(`nous.json 没过 index.mjs，已回滚：${detail}`), 400);
    }
  }

  const info = await stat(file);
  const gitRoot = await gitRootOf(path.dirname(file));
  return {
    name: clean,
    variant: variant ?? null,
    path: pathname,
    kind,
    size: info.size,
    mtime: Math.round(info.mtimeMs),
    etag: etagOf(text),
    dirty: (await run("git", ["status", "--porcelain"], { cwd: gitRoot })).stdout.trim().length > 0,
  };
};

/** 工作区里改了哪些文件（porcelain v1：前两位是状态，改名只认 `->` 右边那个）。 */
const gitChanges = async () => {
  // 别对整份输出 .trim()：第一行是 ` M path`，那个前导空格是状态列的一部分 ——
  // trim 掉再 slice(3) 会把路径首字母一起切掉（`categories.json` → `ategories.json`）。
  const porcelain = (await run("git", ["status", "--porcelain"], { cwd: EXTEND, timeout: 15_000 })).stdout.replace(/\n+$/, "");
  if (!porcelain.trim()) return [];
  return porcelain
    .split("\n")
    .map((line) => {
      let rel = line.slice(3).trim().replace(/^"|"$/g, "");
      if (rel.includes(" -> ")) rel = rel.split(" -> ").pop();
      return { status: line.slice(0, 2), path: rel };
    })
    .filter((item) => item.path);
};

const changedPaths = async () => (await gitChanges()).map((item) => item.path);

/**
 * 语法闸（`checkSyntax`）：代码要过 `node --check` 才允许进仓库。
 *
 * 挂三处，口径一致：发布（`publishExtensions`，只查这次要提交的 js）、变体发布
 * （`publishVariant`，查那棵树里改过的 js）、未同步弹窗的显式提交（`syncExtendRepo` 的
 * `paths` 分支）。类别那条 debounce 碰不到代码，不用陪跑。
 */

/** 重扫 index.json；失败不提交（提交了就是一份红账）。 */
const regenIndex = async () => {
  try {
    await runIndexGenerator();
  } catch (error) {
    const detail = (error.stderr || error.stdout || String(error?.message ?? error)).trim();
    throw withStatus(new Error(`index.mjs 校验失败，没有提交：${detail}`), 400);
  }
};

/** 发布：校验工作区里改过的每个 .js，重扫 index.json，然后 commit + push。 */
const publishExtensions = async ({ name = null, message = null } = {}) => {
  // 发布**只发这一个扩展**（`name/` 底下的一切）+ 重扫出来的 index.json。
  // 以前是「把整份工作区推上去」：抽屉里点一次发布，别人未发布的目录、你正在试的副本
  // 会一起上 GitHub —— 踩过，测试副本就是这么上去的。
  const changes = await gitChanges();
  const scoped = name ? changes.filter((item) => item.path === name || item.path.startsWith(`${name}/`)) : changes;
  if (!scoped.length) {
    return { committed: false, ...(await extendRepoState()), pushed: null, checked: [], changed: [] };
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
  await regenIndex();
  const paths = [...new Set([...scoped.map((item) => item.path), "index.json"])];
  const result = await syncExtendRepo(String(message ?? "").trim() || null, { paths });
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
  const gitRoot = await gitRootOf(dir);
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
    if (kindOfPath(file) !== "javascript") continue;
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
// 所以都串进 serializeExtendWrite，并带上 includeDeleted。

const isTrackedDir = async (name) => {
  try {
    await run("git", ["-C", EXTEND, "ls-files", "--error-unmatch", name], { timeout: 15_000 });
    return true;
  } catch {
    return false; // 未发布的新扩展：还没有任何文件进 index
  }
};

/** 归档区里有什么（按归档时间新→旧）。仓库里没有 .archive/ 就返回空。 */
const archivedEntries = async () => {
  const root = path.join(EXTEND, ARCHIVE_DIR);
  let dirs = [];
  try {
    dirs = (await readdir(root, { withFileTypes: true })).filter((entry) => entry.isDirectory()).map((entry) => entry.name);
  } catch {
    return [];
  }
  const out = [];
  for (const name of dirs) {
    const info = await readManifest(path.join(ARCHIVE_DIR, name));
    let archivedAt = null;
    try {
      archivedAt = (await run("git", ["-C", EXTEND, "log", "-1", "--format=%cI", "--", `${ARCHIVE_DIR}/${name}`])).stdout.trim() || null;
    } catch {}
    out.push({
      name,
      version: info?.manifest?.version ?? null,
      summary: info?.manifest?.summary ?? null,
      category: info?.manifest?.category ?? [],
      archivedAt,
    });
  }
  return out.sort((a, b) => String(b.archivedAt ?? "").localeCompare(String(a.archivedAt ?? "")) || a.name.localeCompare(b.name));
};

/** 把一个目录搬进 / 搬出 .archive/；tracked 的走 git mv（保留历史），否则普搬 + git add。 */
const moveIntoArchive = async (name) => {
  const target = `${ARCHIVE_DIR}/${name}`;
  await mkdir(path.join(EXTEND, ARCHIVE_DIR), { recursive: true });
  if (await stat(path.join(EXTEND, target)).catch(() => null)) {
    throw withStatus(new Error(`${target} 已经存在 —— 先到「已删除」里处理掉那一条`), 409);
  }
  if (await isTrackedDir(name)) {
    await run("git", ["-C", EXTEND, "mv", name, target], { timeout: 30_000 });
  } else {
    await moveDir(path.join(EXTEND, name), path.join(EXTEND, target));
    await run("git", ["-C", EXTEND, "add", "--", target], { timeout: 30_000 });
  }
};

const moveOutOfArchive = async (name) => {
  const source = `${ARCHIVE_DIR}/${name}`;
  if (!(await stat(path.join(EXTEND, source)).catch(() => null))) {
    throw withStatus(new Error(`归档区里没有 ${name}`), 404);
  }
  if (await stat(path.join(EXTEND, name)).catch(() => null)) {
    throw withStatus(new Error(`工作区里已经有一个叫 ${name} 的目录了`), 409);
  }
  if (await isTrackedDir(source)) {
    await run("git", ["-C", EXTEND, "mv", source, name], { timeout: 30_000 });
  } else {
    await moveDir(path.join(EXTEND, source), path.join(EXTEND, name));
    await run("git", ["-C", EXTEND, "add", "--", name], { timeout: 30_000 });
  }
};

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
  if (cats.length) await writeCategories([...(await readCategories()), ...cats]);
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
  if (cats.length) await writeCategories([...(await readCategories()), ...cats]);
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
  if (!(await isTrackedDir(name))) {
    await rm(path.join(EXTEND, name), { recursive: true, force: true });
    await runIndexGenerator();
    await syncExtraExtensions(`drop-${Date.now()}`);
    return { name, archived: false, dropped: true, committed: false, pushed: false, head: null };
  }
  const indexFile = path.join(EXTEND, "index.json");
  const beforeIndex = await readFile(indexFile, "utf8").catch(() => null);
  try {
    await moveIntoArchive(name);
    await runIndexGenerator();
    // 先从库里撤下来（停装），再提交。顺序反过来会有一小段时间「清单里有、目录没了」。
    await syncExtraExtensions(`archive-${Date.now()}`);
    const result = await syncExtendRepo(`extend: 归档 ${name}`, { includeDeleted: true });
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
    await moveOutOfArchive(name);
    await runIndexGenerator();
    await syncExtraExtensions(`unarchive-${Date.now()}`);
    const result = await syncExtendRepo(`extend: 从归档恢复 ${name}`, { includeDeleted: true });
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
  const tracked = await isTrackedDir(`${ARCHIVE_DIR}/${name}`);
  try {
    if (exists && tracked) {
      await run("git", ["-C", EXTEND, "rm", "-r", "-f", "--", `${ARCHIVE_DIR}/${name}`], { timeout: 30_000 });
    } else if (exists) {
      await rm(source, { recursive: true, force: true });
    }
    const result = await syncExtendRepo(`extend: 彻底删除 ${name}`, { includeDeleted: true });
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

/** 仓库状态：有没有没提交的、领先/落后远端几步。 */
const extendRepoState = async () => {
  const opts = { cwd: EXTEND, timeout: 15_000 };
  const head = (await run("git", ["rev-parse", "HEAD"], opts)).stdout.trim();
  const dirty = (await run("git", ["status", "--porcelain"], opts)).stdout.trim().length > 0;
  let ahead = 0;
  let behind = 0;
  try {
    ahead = Number((await run("git", ["rev-list", "--count", "@{u}..HEAD"], opts)).stdout.trim()) || 0;
    behind = Number((await run("git", ["rev-list", "--count", "HEAD..@{u}"], opts)).stdout.trim()) || 0;
  } catch {
    // 没有上游 / 断网：当成 0 说，别让一个查询把整页卡死。
  }
  return { head, dirty, ahead, behind };
};

// index.json 是 categories.json 的生成物：只提交词表不提交索引，仓库当场自相矛盾。
const isCategoryFile = (rel) =>
  rel === "categories.json" || rel === "index.json" || rel.endsWith("nous.json");

// ── 待同步清单：把工作区的脏文件按「改动单元」分组 ─────────────────────────
//
// 单元是**扩展**，不是文件：人想的是「把 123 发出去」，不是「提交 index.js」。
// 文件级的勾选藏在展开里。另外「已提交没推」和「没提交」是两件事，这里分开给。

const EXT_CODE_RE = /\.(mjs|cjs|js)$/i;
const fileKindOf = (rel) => (EXT_CODE_RE.test(rel) ? "code" : isCategoryFile(rel) ? "manifest" : "other");

/** 一条改动属于哪个单元。这个仓库的顶层目录就是扩展，顶层文件是词表/索引/README。 */
const unitOfPath = (rel) => {
  const parts = rel.split("/");
  if (parts[0] === ".archive" && parts.length > 2) return { key: `.archive/${parts[1]}`, kind: "archive", name: parts[1] };
  if (parts.length > 1 && !parts[0].startsWith(".")) return { key: parts[0], kind: "extension", name: parts[0] };
  if (isCategoryFile(rel)) return { key: "__categories", kind: "categories", name: null };
  return { key: "__other", kind: "other", name: null };
};

const UNIT_ORDER = { extension: 0, categories: 1, archive: 2, other: 3 };

const trimRemote = (url) => {
  const raw = String(url ?? "").trim();
  const ssh = raw.match(/^git@([^:]+):(.+?)(?:\.git)?$/);
  if (ssh) return `https://${ssh[1]}/${ssh[2]}`;
  return raw.replace(/\.git$/, "");
};

const pendingState = async () => {
  const changes = await gitChanges();
  const state = await extendRepoState();

  // 「已发布/未发布」看 HEAD 里有没有这个目录 —— 和卡片上 tracked 是同一个口径。
  const trackedDirs = new Set(
    (await run("git", ["-C", EXTEND, "ls-tree", "--name-only", "-d", "HEAD"], { timeout: 20_000 })
      .then((out) => out.stdout)
      .catch(() => "")).split("\n").map((line) => line.trim()).filter(Boolean),
  );

  // 行数口径和 git 一致：对 HEAD 比（暂存 + 未暂存都算）。未跟踪的文件 git diff 看不见，单独数。
  const numstat = new Map();
  const trackedPaths = changes.filter((item) => !item.status.startsWith("??")).map((item) => item.path);
  if (trackedPaths.length) {
    const out = (await run("git", ["-C", EXTEND, "diff", "--numstat", "HEAD", "--", ...trackedPaths], { timeout: 30_000 }))
      .stdout;
    for (const line of out.split("\n").filter(Boolean)) {
      const [adds, dels, ...rest] = line.split("\t");
      numstat.set(rest.join("\t"), { added: adds === "-" ? 0 : Number(adds), removed: dels === "-" ? 0 : Number(dels) });
    }
  }

  const units = new Map();
  for (const change of changes) {
    const { key, kind, name } = unitOfPath(change.path);
    const deleted = change.status.includes("D");
    let lines = numstat.get(change.path) ?? { added: 0, removed: 0 };
    if (change.status.startsWith("??")) {
      // 整份文件都是新增。读不了 / 太大就记 0 —— 这个数只用来判断量级，不参与决策。
      const info = await stat(path.join(EXTEND, change.path)).catch(() => null);
      if (info && info.size <= MAX_FILE_BYTES) {
        const text = await readFile(path.join(EXTEND, change.path), "utf8").catch(() => "");
        lines = { added: text ? text.replace(/\n+$/, "").split("\n").length : 0, removed: 0 };
      }
    }
    const unit = units.get(key) ?? {
      key,
      kind,
      name,
      tracked: kind === "extension" ? trackedDirs.has(name) : true,
      files: [],
      added: 0,
      removed: 0,
      code: false,
      hasDeleted: false,
    };
    unit.files.push({ path: change.path, status: change.status.trim() || "??", kind: fileKindOf(change.path), deleted, ...lines });
    unit.added += lines.added;
    unit.removed += lines.removed;
    if (fileKindOf(change.path) === "code") unit.code = true;
    if (deleted) unit.hasDeleted = true;
    units.set(key, unit);
  }

  const list = [...units.values()].sort(
    (a, b) => UNIT_ORDER[a.kind] - UNIT_ORDER[b.kind] || String(a.name ?? a.key).localeCompare(String(b.name ?? b.key)),
  );

  let aheadCommits = [];
  if (state.ahead) {
    const log = await run("git", ["-C", EXTEND, "log", "--format=%h%x09%s", "-n", "30", "@{u}..HEAD"], { timeout: 20_000 })
      .then((out) => out.stdout)
      .catch(() => "");
    aheadCommits = log
      .split("\n")
      .filter(Boolean)
      .map((line) => {
        const [hash, ...rest] = line.split("\t");
        return { hash, subject: rest.join("\t") };
      });
  }

  const remote = await run("git", ["-C", EXTEND, "remote", "get-url", "origin"], { timeout: 10_000 })
    .then((out) => trimRemote(out.stdout))
    .catch(() => "");
  return { ...state, remote, units: list, aheadCommits };
};

/** 一个文件「对 HEAD」的前后内容 —— 给弹窗里的 diff 用。 */
const pendingFileDiff = async (rel) => {
  const clean = String(rel ?? "").trim();
  if (!clean || clean.includes("..") || path.isAbsolute(clean)) throw withStatus(new Error("路径不对"), 400);
  const file = path.join(EXTEND, clean);
  if (!file.startsWith(EXTEND + path.sep)) throw withStatus(new Error("路径跑到仓库外面了"), 400);
  const info = await stat(file).catch(() => null);
  if (info && info.size > MAX_FILE_BYTES * 4) throw withStatus(new Error("文件太大，不在面板里看 diff"), 413);
  const base = await run("git", ["-C", EXTEND, "show", `HEAD:${clean}`], { timeout: 20_000 })
    .then((out) => out.stdout)
    .catch(() => "");
  const text = info ? await readFile(file, "utf8").catch(() => "") : "";
  return { path: clean, base, text, missing: !text && !base };
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

/** 一条已提交但没推的 commit 改了哪些文件 —— 弹窗里点开看，人去不去推它。 */
const pendingCommit = async (hash) => {
  const sha = String(hash ?? "").trim();
  if (!/^[0-9a-f]{4,40}$/i.test(sha)) throw withStatus(new Error("提交号不对"), 400);
  const meta = (await run("git", ["-C", EXTEND, "show", "-s", "--format=%H%x09%an%x09%ad%x09%s", "--date=short", sha], {
    timeout: 20_000,
  })).stdout.trim();
  const [full, author, date, ...subject] = meta.split("\t");
  const numstat = (await run("git", ["-C", EXTEND, "show", "--numstat", "--format=", sha], { timeout: 60_000 })).stdout;
  const files = numstat
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const [adds, dels, ...rest] = line.split("\t");
      return { path: rest.join("\t"), added: adds === "-" ? 0 : Number(adds), removed: dels === "-" ? 0 : Number(dels) };
    });
  return { hash: full, short: full.slice(0, 7), author, date, subject: subject.join("\t"), files };
};

/**
 * index.json 里有哪些扩展**不在 HEAD 里** —— 也就是「未发布」：新建的、复制出来的、
 * 还没发布就躺在盘上的。它们合法地出现在工作区的 index.json 里，但**不该跟着自动提交走**。
 */
const untrackedIndexNames = async () => {
  let entries = [];
  try {
    entries = JSON.parse(await readFile(path.join(EXTEND, "index.json"), "utf8")).extensions ?? [];
  } catch {
    return [];
  }
  const tracked = new Set(
    (await run("git", ["-C", EXTEND, "ls-tree", "--name-only", "-d", "HEAD"], { timeout: 20_000 })
      .then((out) => out.stdout)
      .catch(() => ""))
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean),
  );
  return entries.filter((item) => !tracked.has(item.name)).map((item) => item.name);
};

/** 把未发布的扩展从 index.json 里摘掉（只动磁盘；提交完再重扫回去）。 */
const pruneIndex = async (names) => {
  const indexFile = path.join(EXTEND, "index.json");
  const doc = JSON.parse(await readFile(indexFile, "utf8"));
  doc.extensions = (doc.extensions ?? []).filter((item) => !names.includes(item.name));
  await writeFile(indexFile, JSON.stringify(doc, null, 2) + "\n", "utf8");
};

/**
 * 把待提交改动打成一条 commit，然后 push。push 失败要说实话，不能装成功。
 *
 * `only: "categories"` = 只提交词表和清单，**不碰代码**。类别那条 8 秒 debounce 走这个：
 * 否则「保存了但没发布」的代码会被下一次点标签顺手推上去 —— 保存和发布分开了，
 * 推的却是同一个工作区，等于白分。
 */
const syncExtendRepo = async (message, { only = null, includeDeleted = false, paths = null } = {}) => {
  const opts = { cwd: EXTEND, timeout: 60_000, env: { ...process.env, GIT_TERMINAL_PROMPT: "0" } };
  let committed = false;
  const changes = await gitChanges();
  // 「删本地」必然让 tracked 文件变成 ` D`。这条过滤是本地删除和发布之间的防火墙：
  // 发布推的是工作区，不挡住的话，你删了 demo、隔天改一行代码点发布，删除就跟着上 GitHub。
  const deleted = new Set(changes.filter((item) => item.status.includes("D")).map((item) => item.path));
  const changed = changes.map((item) => item.path);
  let targets = (only === "categories" ? changed.filter(isCategoryFile) : changed).filter(
    // 归档/恢复/彻底删除是显式动作，它们的删除必须能提交；其余路径一律不碰删除。
    (rel) => includeDeleted || !deleted.has(rel),
  );
  if (Array.isArray(paths)) {
    // 显式勾选：只碰这几条。没勾的一个字节都不提交 —— 这是「按扩展选」的全部实现。
    const want = new Set(paths.map((rel) => String(rel)));
    targets = targets.filter((rel) => want.has(rel));
    // 代码改动要过语法闸：不走抽屉也能提交代码，代价就是这道检查。
    for (const rel of targets) {
      if (!EXT_CODE_RE.test(rel)) continue;
      const text = await readFile(path.join(EXTEND, rel), "utf8").catch(() => null);
      if (text !== null) await checkSyntax(rel, text);
    }
  }
  // 类别那条 debounce 是**自动**提交，它不能替谁发布。未发布的目录也在 index.json 里
  // （新建的、复制出来的），整份推上去 = 「仓库说我有它，实际没有」。
  // 所以提交前把不在 HEAD 里的条目从索引里摘掉，提交完再重扫回来（它会重新变脏，正是该有的样子）。
  // 谁进了这次提交，谁就有资格留在索引里：index.json 里的未发布条目，只要「HEAD 里没有
  // 这个目录」且「这次的 paths 里也没有它」，就摘掉 —— 自动提交不能替谁发布。
  // （发布一个新扩展时它的目录也在 paths 里，于是照常进索引。）
  let skippedIndex = [];
  if (targets.includes("index.json")) {
    const incoming = new Set(targets.map((rel) => rel.split("/")[0]));
    const untracked = await untrackedIndexNames();
    const skip = untracked.filter((item) => !incoming.has(item));
    if (skip.length) {
      await pruneIndex(skip);
      skippedIndex = skip;
    }
  }

  try {
    if (targets.length) {
      // 已删掉的路径不能再 `git add`（“pathspec did not match any files”）；它们的删除
      // 已经由 `git rm` / `git mv` 暂存好了。只 add 还存在的文件。
      const addable = [];
      for (const rel of targets) {
        if (await stat(path.join(EXTEND, rel)).catch(() => null)) addable.push(rel);
      }
      if (addable.length) await run("git", ["add", "--", ...addable], opts);
      // 用暂存区判断“真有待提交的东西”：纯删除（git rm）不会有 addable，但也得提交。
      const staged = (await run("git", ["diff", "--cached", "--name-only"], opts)).stdout.trim();
      if (staged) {
        // 默认说明按**真改了的文件**生。混着代码改动却写成 `category:` 是骗人
        // —— 将来的 `git log` 靠这一行找人。
        const when = new Date().toISOString().slice(0, 19).replace("T", " ");
        const onlyCategories = targets.every(isCategoryFile);
        const fallback = onlyCategories
          ? `category: 更新扩展类别（${when}）`
          : `extend: 更新 ${targets.slice(0, 4).join("、")}${targets.length > 4 ? ` 等 ${targets.length} 个文件` : ""}`;
        const msg = String(message ?? "").trim() || fallback;
        await run("git", ["commit", "-m", msg], opts);
        committed = true;
      }
    }
    const hash = committed
      ? (await run("git", ["-C", EXTEND, "rev-parse", "--short", "HEAD"], opts)).stdout.trim()
      : null;
    const state = await extendRepoState();
    if (!state.ahead) return { committed, hash, files: targets, skippedIndex, ...state, pushed: null };
    try {
      await run("git", ["push"], opts);
    } catch (error) {
      const pushError = (error.stderr || error.stdout || String(error?.message ?? error)).trim();
      return { committed, hash, files: targets, skippedIndex, ...state, pushed: false, pushError };
    }
    return { committed, hash, files: targets, skippedIndex, ...state, ahead: 0, pushed: true };
  } finally {
    // 索引必须回到「盘上有什么就写什么」，否则页面和生成器看到的是被摘过的版本。
    if (skippedIndex.length) {
      await runIndexGenerator().catch((error) => console.error("[extend] 重扫 index.json 失败：", errorLine(error)));
    }
  }
};

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

// ── 提示词 ───────────────────────────────────────────────────────────────
//
// 提示词一类的自研扩展（`nous` = 自我介绍 + cwd）在 Nous-extend 仓库的 `nous/`，不写在这儿。
// 当初为什么先有它再有 skills：registry 里只有 CodingTools 时一条 section 都没有，
// 系统提示词是空的 —— 技能清单挂上去会像「一个不知道自己是谁的家伙手里拿着一张技能表」。

const nousExtension = createNous(api);
// skills 的 API 用间接引用：热重载时要换整个 service 对象。直接替换变量不够，
// 路由里的闭包还指着旧对象——所以所有调用都走代理，读当前那个 service。
const skillsService = { current: null };
const skills = new Proxy({}, {
  get(_, key) {
    const service = skillsService.current;
    const value = service[key];
    return typeof value === "function" ? value.bind(service) : value;
  },
});
skillsService.current = await createSkills({ dir: path.join(DATA, "skills"), registry, api });
const { skillExtension } = skills;
registry.install(nousExtension);
registry.install(skills.extension);
await skills.load();
// 老收据补齐版本：后台跑，别把启动卡在 clone 上。
void skills.backfill();
// 核心两条的变体：**启动时就得装**。只靠 installCore 的话，进程一重启它们就没了，
// 得等人点一次「重试装载」或者推一次代码才回来 —— 那是「页面说装了、进程里没有」的翻版。
await loadCoreVariants("boot");
// 核心两条之外，按 index.json 装其余扩展。放最后：一个坏扩展不该拖垮启动路径。
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
  const skillsModule = await import(`${EXTEND}/skills/index.js?v=${version}`);
  skillsService.current = await skillsModule.createSkills({ dir: path.join(DATA, "skills"), registry, api });
  registry.install(skillsService.current.extension);
  await skillsService.current.load();
  void skillsService.current.backfill();

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
  if (extendBusy) return;
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
    console.error("[extend] 热重装：nous + skills + 清单扩展 @" + head.slice(0, 7));
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
    env: ({ cwd }) => new NodeExecutionEnv({ cwd: cwd ?? HERE }),
    // 默认选择只放**真扩展**：空名牌（skill:<名>）装了但不默认选 —— 否则每条新会话
    // 一出生就吃 38 条描述，progressive disclosure 当场作废。
    settings: { extensions: [CodingTools, nousExtension, skills.extension] },
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
      // 生效的技能（从 durable 存的扩展名里读回来，不是从我们的表里读）——
      // 面板该拿这个去画勾，所以列表里就得有它。
      skills: skillNamesOf(agent),
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

  // 技能页：装 / 卸。链接写相对的（"skills"），反代前缀由宿主决定。
  if (req.method === "GET" && parts.length === 1 && parts[0] === "skills") {
    res.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" });
    return res.end(await readFile(new URL("./skills.html", import.meta.url), "utf8"));
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

  // ── 技能 ────────────────────────────────────────────────────────────────
  if (parts[0] === "v1" && parts[1] === "skills") {
    if (req.method === "GET" && !parts[2]) {
      // 编目没拉过就先拉一次：列表页第一次打开不该看见「什么都没有」。
      const cached = skills.catalog();
      const loaded = cached.at ? cached : await skills.refreshCatalog();
      // 还有人没记版本（老收据 / 刚装完断电）：后台补一次，这一轮先照实回 null。
      // 页面上就是「未标版本」，不会凭空猜一个数字。
      void skills.backfill();
      return json(res, 200, {
        dir: skills.dir,
        installed: skills.list(),
        items: loaded.items,
        // 编目通不通跟着清单一起报，跟 /v1/models 一个道理。
        catalog: { base: loaded.base, at: loaded.at, error: loaded.error },
      });
    }
    if (req.method === "POST" && parts[2] === "refresh") {
      const next = await skills.refreshCatalog();
      return json(res, 200, { ok: !next.error, ...next });
    }
    if (req.method === "POST" && parts[2] === "install") {
      const body = JSON.parse((await readBody(req)) || "{}");
      try {
        const installed = await skills.install({ source: body.source, entry: body.entry, repo: body.repo, path: body.path });
        return json(res, 200, { ok: true, installed });
      } catch (error) {
        // 装失败要说清楚到哪一步了（编目？版本号？tarball？目录里没这条路径？），
        // 不然页面上只有一个红条，人得自己猜。
        return json(res, 400, { error: String(error?.message ?? error) });
      }
    }
    if (req.method === "POST" && parts[2] === "uninstall") {
      const body = JSON.parse((await readBody(req)) || "{}");
      const name = String(body.name ?? "");
      if (!name) return json(res, 400, { error: "name 是空的" });
      return json(res, 200, { ok: true, ...(await skills.uninstall(name)) });
    }
    return json(res, 404, { error: "skills 没有这条路" });
  }

  // 扩展类别：改 nous.json + 重扫 index.json（立刻生效）；commit/push 走 /sync。
  // 放在 GET /v1/extensions 前面 —— 否则 `GET /v1/extensions/sync` 会被下面那条
  // 只看 parts[1] 的列表接口先接走。
  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "category") {
    if (req.method !== "POST") return json(res, 405, { error: "类别只有 POST" });
    const body = JSON.parse((await readBody(req)) || "{}");
    try {
      const updated = await serializeExtendWrite(() =>
        setExtensionCategories(String(body.name ?? ""), body.category),
      );
      const catalog = JSON.parse(await readFile(path.join(EXTEND, "index.json"), "utf8")).extensions;
      return json(res, 200, {
        extension: updated,
        catalog,
        categories: await readCategories(),
        protected: PROTECTED_CATEGORIES,
      });
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  // 类别词表：建一个还没挂到任何扩展上的类别 / 删一个（连同从扩展上摘掉）。
  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "categories") {
    if (req.method === "GET") {
      return json(res, 200, { categories: await readCategories(), protected: PROTECTED_CATEGORIES });
    }
    if (req.method !== "POST") return json(res, 405, { error: "类别词表只有 GET/POST" });
    const body = JSON.parse((await readBody(req)) || "{}");
    try {
      const result = await serializeExtendWrite(() => mutateCategories(body));
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
        const saved = await serializeExtendWrite(() =>
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
      const result = await serializeExtendWrite(() =>
        body.variant
          ? publishVariant({ name: body.name, variant: body.variant, message: body.message })
          : publishExtensions({ name: body.name, message: body.message }),
      );
      // 发布之后立刻重装：commit + push 只挪了 git 指针，进程里跑的仍是旧代码。
      // 不在这里装一次，页头会挂 30 秒「⚠ 不一致」，而改代码的人正要马上看效果。
      let reload = null;
      if (result.committed) {
        try {
          reload = await serializeExtendWrite(() => reloadExtensionsNow());
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
      return json(res, 200, await serializeExtendWrite(() => reloadExtensionsNow()));
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
      const result = await serializeExtendWrite(async () => {
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
      return json(res, 200, await serializeExtendWrite(() => task(name)));
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
      const result = await serializeExtendWrite(() =>
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
      const made = await serializeExtendWrite(() => createExtension(body));
      // 装不上不算建失败：目录已经在磁盘上了，页面会把它标成「装载失败」并给出重试。
      let load = null;
      try {
        load = await serializeExtendWrite(() => reloadExtensionsNow());
      } catch (error) {
        load = { ok: false, error: String(error?.message ?? error) };
      }
      const catalog = JSON.parse(await readFile(path.join(EXTEND, "index.json"), "utf8")).extensions;
      return json(res, 200, { ...made, catalog, categories: await readCategories(), load });
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "duplicate") {
    if (req.method !== "POST") return json(res, 405, { error: "复制只有 POST" });
    const body = JSON.parse((await readBody(req)) || "{}");
    try {
      const made = await serializeExtendWrite(() => duplicateExtension(body));
      // 和「新建」一样，造完就重装一次：目录已经在盘上了，装不上的话页面会标成装载失败并给重试。
      let load = null;
      try {
        load = await serializeExtendWrite(() => reloadExtensionsNow());
      } catch (error) {
        load = { ok: false, error: String(error?.message ?? error) };
      }
      const catalog = JSON.parse(await readFile(path.join(EXTEND, "index.json"), "utf8")).extensions;
      return json(res, 200, { ...made, catalog, categories: await readCategories(), load });
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  // 仓库状态 / 手动同步。客户端 debounce 之后打 POST；刷新时先 GET 一次看有没有欠账。
  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "pending") {
    if (req.method !== "GET") return json(res, 405, { error: "待同步清单只有 GET" });
    try {
      if (parts[3] === "diff") {
        return json(res, 200, await pendingFileDiff(url.searchParams.get("path")));
      }
      if (parts[3] === "commit") {
        return json(res, 200, await pendingCommit(url.searchParams.get("hash")));
      }
      return json(res, 200, await pendingState());
    } catch (error) {
      return json(res, error.status ?? 500, { error: String(error?.message ?? error) });
    }
  }

  if (parts[0] === "v1" && parts[1] === "extensions" && parts[2] === "sync") {
    if (req.method === "GET") {
      try {
        return json(res, 200, await extendRepoState());
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
        const result = await serializeExtendWrite(() =>
          picked ? syncExtendRepo(body.message, { paths: picked }) : syncExtendRepo(body.message, { only: "categories" }),
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
      (name) => !name.startsWith(SKILL_PREFIX) && !builtins.includes(name) && !name.includes(VARIANT_SEP),
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
    const nameplates = installed.filter((name) => name.startsWith(SKILL_PREFIX));
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
      // 名牌单独列：那是 skills 扩展运行时按技能生成的家族，不是独立扩展 ——
      // 混进 installed 会变成 40 行里 38 行是生成物。
      nameplates: { prefix: SKILL_PREFIX, count: nameplates.length, names: nameplates },
      defaults: [nousExtension.name, skills.extension.name],
      categories: await readCategories(),
      protected: PROTECTED_CATEGORIES,
      catalog,
      variants,
      local: { ...(await readLocalState()), core: CORE_EXTENSIONS },
      archived: await archivedEntries(),
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
    return json(res, 200, { items: await tree() });
  }

  if (req.method === "POST" && parts[0] === "v1" && parts[1] === "sessions" && !parts[2]) {
    const body = JSON.parse((await readBody(req)) || "{}");
    const model = body.model ?? defaultModel;
    // 档位在建会话的**同一个 commit** 里落定（AgentChange 的 thinkingLevel），
    // 不是先建再 configure —— 后者中间那一下，第一条消息可能已经跑出去了。
    const thinkingLevel = fitLevel(models.getModel(model.provider, model.modelId), body.thinkingLevel);
    // 技能也在这个 commit 里落 —— 跟 thinkingLevel 一条纪律：先建再 configure，中间那一下
    // 第一条消息可能已经跑出去了。没装的技能名直接丢（选了也不生效，不存一个幽灵）。
    const wanted = [...new Set((Array.isArray(body.skills) ? body.skills : []).map(String))].filter((name) => skills.has(name));
    const wantedExt = [...new Set((Array.isArray(body.extensions) ? body.extensions : []).map(String))].filter((name) =>
      registry.snapshot().installed().some((e) => e.name === name),
    );
    // 清单里的 `default: true` = 新会话默认选它。README 一直这么写着，但从来没人读它 ——
    // 页面那个「默认」绿角标看的是下面这三个硬编码的名字，于是 nous.json 里那个字段
    // 只是个装饰品（改了没反应，比没有更糟）。这里把它接上：只补「不是那三个」的，
    // 重复 add 同一个扩展没意义。
    if (!body.extensions) {
      const baseDefaults = [CodingTools.name, nousExtension.name, skills.extension.name];
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
          cwd: HERE,
          model,
          ...(thinkingLevel ? { thinkingLevel } : {}),
          ...(() => {
          const adds = [
            ...(wanted.length ? wanted.map(skillExtension) : []),
            ...(wantedExt.length ? wantedExt.map((n) => registry.snapshot().extension(n)) : []),
          ];
          return adds.length ? { extensions: { add: adds } } : {};
        })(),
        },
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
      skills: skillNamesOf(agent),
      extensions: (agent.extensions ?? []).filter((n) => !String(n).startsWith("skill:")),
      missing: skills.missing(wanted),
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
        const current = (await conversation.agent(ctx)).extensions ?? [];
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
      // 回的是**生效值**，不是请求值：前端拿它重画，面板和真相就不会分叉。
      const agent = await conversation.agent(ctx);
      return json(res, 200, {
        id,
        model: agent.model ? `${agent.model.provider}/${agent.model.modelId}` : null,
        thinkingLevel: agent.thinkingLevel ?? null,
        skills: skillNamesOf(agent),
        extensions: (agent.extensions ?? []).filter((n) => !String(n).startsWith("skill:")),
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

const shutdown = async () => {
  server.close();
  await harness.close(ctx).catch(() => {});
  process.exit(0);
};
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);

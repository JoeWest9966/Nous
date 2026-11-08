// presets —— 会话预设：名册 + 引用校验 + 展开。**纯模块，不建扩展。**
//
// 位置：`content/new1/map/qrd3wh/presets.js`。跟 `prompt-log.js` 同一个位置级别 ——
// 直接 import、纯逻辑 + 一个扫盘，没有 `createExtension`、没有 section、**没有 registry 副作用**。
// 所以它能裸跑 `node --test`（`server.js` 有顶层 await，测试永远不能 import 它）。
//
// 为什么不住 `Nous-extend`（skills / prompts 都住那儿）：那两家住那儿是因为它们是**扩展**
// —— 要 registry 认、要一个 `skill:` / `prompt:` 名牌、要每轮渲染 section。预设一个都不是：
// 它不建名牌、不进提示词、跑起来之后已经全部展开成别的东西了。塞进扩展仓只是多改一个仓，
// 换不来任何东西。
//
// 一条预设（`Nous-Template/Session-Template/<名>/preset.json`）= 一组**引用**。这一层干两件
// 别处干不了的事：
//
//   校验 —— 「引用的东西还活着吗」（技能还装着吗、模板还在吗、模型在编目里吗、档位在那台
//            模型的档位表里吗）。盘上的 `index.mjs` 做不了，它没有 registry。
//   展开 —— 把 spec 变成 `POST /v1/sessions` 认得的那几个字段，**原样保留「键在不在」**。
//
// 四条纪律：
//
//  1. **仓库是唯一真相，这里是投影。** 一个仓按**段**放（Session-Template /
//     Ultimate-Template），每段一个名册实例，各扫各的目录、各写各的索引。 盘上有 `<名>/preset.json` = 有这条预设；删掉目录
//     下次扫盘就没了。不联网、不维护第二份名单、不缓存。
//  2. **三态不许合并。** `template` 缺席 = 用仓库默认、`template: null` = 明确不要、
//     字符串 = 用它。`skills` 缺席 = 服务端按默认、`[]` = 一个都不要。展开函数里一个
//     `?? null` 就能把这三种语义糊成一种，所以 `expandPresetSpec` 只写「键在不在」，
//     不写默认值。
//  3. **「引用失效」不是「写坏了」。** 前者是现在开不了（技能被卸了），后者是 JSON / 形状坏了。
//     前者走 `resolve` 的红 chip、启动时进 `missing[]`；只有后者进 `specError`。
//  4. **形状规则跟 `Nous-Template/index.mjs` 是同一套，两处存在。** 那个仓要能单独 checkout、
//     单独跑（不许 import 引擎），所以要自己一份。跟 `Nous-prompts/index.mjs` ↔ `prompts-api.js`
//     那一对同一个形状。两边各持一份同样的测试表，漂了会红。

import { readFile, readdir } from "node:fs/promises";
import path from "node:path";

/**
 * 终极模板的键表。一条 = **触发器 + 触发器的参数 + 一条会话模板**。
 *
 * 跟会话预设不是一套形状：预设钉「开会话时用什么」，终极模板钉「什么时候起、起什么」。
 * 形状规则跟 `Nous-Template/index.mjs` 的 `checkUltimateShape` 是同一套 —— 两处各持一份。
 */
export const ULTIMATE_KEYS = [
  "v",
  "name",
  "summary",
  "category",
  "description",
  "trigger",
  "params",
  "session",
  "enabled",
];

/** 终极模板的形状校验（不看引用）。返回 `string[]`，空 = 通过。 */
export function ultimateShapeProblems(raw, { dirName = null, where = "ultimate.json" } = {}) {
  const problems = [];
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return [`${where} 得是一个对象`];

  for (const key of Object.keys(raw)) {
    if (!ULTIMATE_KEYS.includes(key)) {
      problems.push(`${where} 里有不认识的键「${key}」—— 不忽略它（拼错的键等于那条设置没生效）`);
    }
  }
  if (raw.v !== 1) problems.push(`${where} 的 v 得是 1，收到 ${JSON.stringify(raw.v)}`);

  const name = typeof raw.name === "string" ? raw.name.trim() : "";
  if (dirName) {
    if (name && name !== dirName) problems.push(`${where} 的 name 是「${name}」—— 必须等于目录名「${dirName}」`);
  } else if (!name) {
    problems.push(`${where} 缺 name`);
  }
  if (name) {
    if (!NAME_RE.test(name)) problems.push(`${where} 的 name「${name}」不合格：小写字母 / 数字开头，只留 . _ -`);
    if (name.length > MAX_NAME) problems.push(`${where} 的 name 太长（最多 ${MAX_NAME} 字）`);
  }

  const summary = typeof raw.summary === "string" ? raw.summary.trim() : "";
  if (!summary) problems.push(`${where} 的 summary 为空 —— 人靠这一行决定开不开它`);
  else if (summary.length > MAX_SUMMARY) problems.push(`${where} 的 summary 太长（最多 ${MAX_SUMMARY} 字）`);

  if (raw.description !== undefined) {
    if (typeof raw.description !== "string") problems.push(`${where} 的 description 得是字符串`);
    else if (raw.description.length > MAX_DESCRIPTION) {
      problems.push(`${where} 的 description 太长（最多 ${MAX_DESCRIPTION} 字）`);
    }
  }

  if (!Array.isArray(raw.category)) {
    problems.push(`${where} 缺 category 数组（没有就写 []）`);
  } else if (raw.category.some((entry) => typeof entry !== "string")) {
    problems.push(`${where} 的 category 得是字符串数组`);
  }

  if (!nonEmptyString(raw.trigger)) problems.push(`${where} 缺 trigger —— 这条得挂在某个触发器上`);
  if (!raw.params || typeof raw.params !== "object" || Array.isArray(raw.params)) {
    problems.push(`${where} 的 params 得是一个对象（键是参数名，值是参数值）`);
  } else {
    for (const [key, value] of Object.entries(raw.params)) {
      if (!/^[a-z0-9][a-z0-9_-]*$/.test(key)) {
        problems.push(`${where} 的参数名「${key}」不合格：小写字母 / 数字开头，只留 _ -`);
      }
      if (!["string", "number", "boolean"].includes(typeof value)) {
        problems.push(`${where} 的参数「${key}」的值得是字符串 / 数字 / 布尔`);
      }
    }
  }
  if (!nonEmptyString(raw.session)) {
    problems.push(`${where} 缺 session —— 得钉一条会话模板（Session-Template 段里的名字）`);
  }
  if (raw.enabled !== undefined && typeof raw.enabled !== "boolean") {
    problems.push(`${where} 的 enabled 得是 true / false`);
  }

  return problems;
}

/**
 * 触发器的**参数类型**。跟 `Nous-Trigger` 那边 `index.mjs` 校验的词汇表同一份 ——
 * 那边是写，这边是读，两处各持一份，漂了会红。
 */
export const PARAM_TYPES = ["epoch", "number", "string", "text", "boolean", "select"];

/** 一个参数值对这个声明合不合格。返回一句话（空 = 合格）。 */
export function paramValueProblem(param, value) {
  switch (param.type) {
    case "epoch":
      if (typeof value !== "number" || !Number.isInteger(value) || value < 0) return "得是 UTC 秒（正整数）";
      return "";
    case "number":
      if (typeof value !== "number" || !Number.isFinite(value)) return "得是数字";
      return "";
    case "boolean":
      return typeof value === "boolean" ? "" : "得是 true / false";
    case "select":
      if (typeof value !== "string") return "得是字符串";
      if (Array.isArray(param.options) && !param.options.includes(value)) {
        return `只能是 ${param.options.join(" / ")}`;
      }
      return "";
    case "text":
    case "string":
    default:
      return typeof value === "string" ? "" : "得是字符串";
  }
}

/**
 * 终极模板的**引用**校验：触发器还在吗、参数跟它的声明对得上吗、会话模板还在吗。
 *
 * 这一层**只有引擎做得了**：触发器的参数声明在另一个仓（Nous-Trigger）里，
 * 仓自己的 index.mjs 读不到 —— 它只查形状。
 *
 * @param {object} spec 形状已通过的一条 ultimate
 * @param {{ triggers?: Map<string, object>, sessionsHas?: Function }} ctx
 */
export function ultimateReferenceProblems(spec, ctx = {}) {
  const errors = [];
  const { triggers, sessionsHas } = ctx;

  const trigger = triggers?.get?.(String(spec.trigger ?? ""));
  if (triggers && !trigger) {
    errors.push(`触发器「${spec.trigger}」不在 Nous-Trigger 里`);
  } else if (trigger) {
    // 参数对不对得上那个触发器自己的声明 —— **值域在触发器那边定，不在页面定**。
    const schema = Array.isArray(trigger.params) ? trigger.params : [];
    const given = spec.params ?? {};
    for (const param of schema) {
      const value = given[param.name];
      if (value === undefined) {
        if (param.required) errors.push(`触发器「${spec.trigger}」要参数「${param.name}」，这条没给`);
        continue;
      }
      const problem = paramValueProblem(param, value);
      if (problem) errors.push(`参数「${param.name}」：${problem}`);
    }
    for (const key of Object.keys(given)) {
      if (!schema.some((param) => param.name === key)) {
        // 触发器改了参数名，这条还留着旧的 —— 报出来，让人去改**它**（不是这条）。
        errors.push(`触发器「${spec.trigger}」没有参数「${key}」了 —— 参数名变了，这条得改`);
      }
    }
  }

  if (sessionsHas && !sessionsHas(spec.session)) {
    errors.push(`会话模板「${spec.session}」不在 Session-Template 里`);
  }

  return errors;
}

/** 思考档的**顺序**（往上找、再往下找都靠它）。跟 pi-ai 的档位表同一份，别另立一套。 */
export const THINKING_LEVELS = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/** 顶层键白名单。未知键必须报错不能忽略：拼错的键等于那条设置静默没生效。 */
export const PRESET_KEYS = [
  "v",
  "name",
  "summary",
  "category",
  "description",
  "model",
  "thinkingLevel",
  "skills",
  "extensions",
  "template",
  "readOnly",
];

const NAME_RE = /^[a-z0-9][a-z0-9._-]*$/;
const MAX_NAME = 64;
const MAX_SUMMARY = 200;
const MAX_DESCRIPTION = 2000;

const nonEmptyString = (value) => typeof value === "string" && value.trim().length > 0;
export const oneLine = (error) => String(error?.message ?? error).split("\n")[0].slice(0, 400);

// ── 形状校验：跟 Nous-Template/index.mjs 的 checkSpecShape 同一套规则 ─────────

/**
 * 一条 spec 的**形状**校验（不看引用）。返回 `string[]`，空 = 通过。
 *
 * `dirName` 有值时要求 `name` 等于目录名；没有（编辑框草稿）时只要求 name 合法。
 */
export function shapeProblems(raw, { dirName = null, where = "preset.json" } = {}) {
  const problems = [];
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return [`${where} 得是一个对象`];

  for (const key of Object.keys(raw)) {
    if (!PRESET_KEYS.includes(key)) {
      problems.push(`${where} 里有不认识的键「${key}」—— 不忽略它（拼错的键等于那条设置没生效）`);
    }
  }
  if (raw.v !== 1) problems.push(`${where} 的 v 得是 1，收到 ${JSON.stringify(raw.v)}`);

  const name = typeof raw.name === "string" ? raw.name.trim() : "";
  if (dirName) {
    if (name && name !== dirName) problems.push(`${where} 的 name 是「${name}」—— 必须等于目录名「${dirName}」`);
  } else if (!name) {
    problems.push(`${where} 缺 name`);
  }
  if (name) {
    if (!NAME_RE.test(name)) problems.push(`${where} 的 name「${name}」不合格：小写字母 / 数字开头，只留 . _ -`);
    if (name.length > MAX_NAME) problems.push(`${where} 的 name 太长（最多 ${MAX_NAME} 字）`);
  }

  const summary = typeof raw.summary === "string" ? raw.summary.trim() : "";
  if (!summary) problems.push(`${where} 的 summary 为空 —— 人靠这一行决定开不开它`);
  else if (summary.length > MAX_SUMMARY) problems.push(`${where} 的 summary 太长（最多 ${MAX_SUMMARY} 字）`);

  if (raw.description !== undefined) {
    if (typeof raw.description !== "string") problems.push(`${where} 的 description 得是字符串`);
    else if (raw.description.length > MAX_DESCRIPTION) {
      problems.push(`${where} 的 description 太长（最多 ${MAX_DESCRIPTION} 字）`);
    }
  }

  if (!Array.isArray(raw.category)) {
    problems.push(`${where} 缺 category 数组（没有就写 []）`);
  } else if (raw.category.some((entry) => typeof entry !== "string")) {
    problems.push(`${where} 的 category 得是字符串数组`);
  }

  if (raw.model !== undefined) {
    if (!raw.model || typeof raw.model !== "object" || Array.isArray(raw.model)) {
      problems.push(`${where} 的 model 得是 {provider, modelId} 对象，或者整个键不写（= 跟随设备默认）`);
    } else {
      if (Object.keys(raw.model).sort().join(",") !== "modelId,provider") {
        problems.push(`${where} 的 model 只能有 provider 和 modelId 两个键`);
      }
      if (!nonEmptyString(raw.model.provider)) problems.push(`${where} 的 model.provider 为空`);
      if (!nonEmptyString(raw.model.modelId)) problems.push(`${where} 的 model.modelId 为空`);
    }
  }

  if (raw.thinkingLevel !== undefined && !THINKING_LEVELS.includes(raw.thinkingLevel)) {
    problems.push(`${where} 的 thinkingLevel「${raw.thinkingLevel}」不在 ${THINKING_LEVELS.join(" / ")} 里`);
  }

  for (const field of ["skills", "extensions"]) {
    if (raw[field] === undefined) continue;
    if (!Array.isArray(raw[field])) {
      problems.push(`${where} 的 ${field} 得是字符串数组（一个都不要写 []，跟随默认就整个键不写）`);
      continue;
    }
    if (raw[field].some((entry) => !nonEmptyString(entry))) {
      problems.push(`${where} 的 ${field} 里有空的 / 不是字符串的项`);
    }
  }

  // 三态：键在且是字符串 = 选它；键在且是 null = 显式不要；键不在 = 跟随仓库默认。
  if (raw.template !== undefined && raw.template !== null && !nonEmptyString(raw.template)) {
    problems.push(`${where} 的 template 得是模板名，或者 null（明确不要）`);
  }

  if (raw.readOnly !== undefined && typeof raw.readOnly !== "boolean") {
    problems.push(`${where} 的 readOnly 得是 true / false`);
  }

  return problems;
}

// ── 引用校验 + 展开 ─────────────────────────────────────────────────────────

/**
 * 引用还活着吗。`ctx` 全是问句函数，别传对象 —— 调用的那一层知道 registry 长什么样，
 * 这一层不知道（也正因如此它能裸跑测试）。
 *
 * @param {object} spec 形状已通过的一条 spec
 * @param {{ skillsHas?: Function, extensionsHas?: Function, promptsHas?: Function,
 *           catalogHas?: Function, levelsOf?: Function }} ctx
 * @returns {{ errors: string[], warnings: string[] }}
 */
export function referenceProblems(spec, ctx = {}) {
  const errors = [];
  const warnings = [];
  const { skillsHas, extensionsHas, promptsHas, catalogHas, levelsOf } = ctx;

  if (spec.skills !== undefined && skillsHas) {
    for (const name of spec.skills) if (!skillsHas(name)) errors.push(`技能「${name}」没装（Nous-skills 里没有它）`);
  }
  if (spec.extensions !== undefined && extensionsHas) {
    for (const name of spec.extensions) {
      // 名牌不是扩展：`prompt:<名>` / `skill:<名>` 有各自的字段，混进来一定是写错了。
      if (name.includes(":")) {
        errors.push(`「${name}」是名牌，不是扩展 —— 模板写 template，技能写 skills`);
        continue;
      }
      if (!extensionsHas(name)) errors.push(`扩展「${name}」没装（registry 里没有它）`);
    }
  }
  if (spec.template !== undefined && spec.template !== null && promptsHas && !promptsHas(spec.template)) {
    errors.push(`模板「${spec.template}」不在 Nous-prompts 里`);
  }

  if (spec.model && catalogHas && !catalogHas(spec.model)) {
    errors.push(`模型「${spec.model.provider}/${spec.model.modelId}」不在编目里`);
  }
  if (spec.thinkingLevel !== undefined && levelsOf) {
    if (!spec.model) {
      // 模型跟随设备默认 —— 档位到时候按**那台**模型重新夹。说出来，别偷偷改。
      warnings.push(`档位「${spec.thinkingLevel}」会按设备默认模型重新夹（这条没钉模型）`);
    } else {
      const levels = levelsOf(spec.model) ?? [];
      if (!levels.includes(spec.thinkingLevel)) {
        errors.push(
          `档位「${spec.thinkingLevel}」不在 ${spec.model.modelId} 支持的档位里（${levels.join(" / ") || "无"}）`,
        );
      }
    }
  }

  return { errors, warnings };
}

/**
 * 一条 spec → 建会话要发的字段。**只写「键在不在」，不写默认值** —— 默认值是服务端的事，
 * 这一层填了就等于把「没说」变成「说了」，三态当场没了。
 *
 * `template: "auto"` 是唯一一个被规范化掉的魔法值（留给 hb7k3j 的 picker）：它等于「跟随默认」，
 * 也就是键缺席。
 */
export function expandPresetSpec(spec) {
  const out = {};
  if (spec.model) out.model = { provider: spec.model.provider, modelId: spec.model.modelId };
  if (spec.thinkingLevel !== undefined) out.thinkingLevel = spec.thinkingLevel;
  if (spec.skills !== undefined) out.skills = [...spec.skills];
  if (spec.extensions !== undefined) out.extensions = [...spec.extensions];
  if ("template" in spec && spec.template !== "auto") out.template = spec.template;
  if (spec.readOnly !== undefined) out.readOnly = spec.readOnly;
  // cwd 可选：缺席 = 用块缺省；null = 显式不要（同 template 三态）。绝对路径在
  // 别台机器上可能不存在 —— 只在建会话那一刻校验，这里只透传。
  if (spec.cwd !== undefined) out.cwd = spec.cwd;
  return out;
}

/**
 * 终极模板的 resolve：这条现在还成立吗。**面板不许自己算** —— 它一算就有两份规则。
 *
 * 三样东西各自的状态：
 *   触发器 —— 在不在 Nous-Trigger 里（不在 = 引用失效）
 *   参数   —— 跟触发器的声明对不对得上（参数名变了 = 引用失效）
 *   会话   —— 在不在 Session-Template 里（不在 = 引用失效）
 *
 * 形状坏了的这条不 resolve（那是 failed，不是 invalid）。
 */
export function resolveUltimate(spec, ctx = {}) {
  const { triggers, sessionsHas } = ctx;
  const trigger = triggers?.get?.(String(spec.trigger ?? "")) ?? null;
  const schema = Array.isArray(trigger?.params) ? trigger.params : [];
  const given = spec.params ?? {};

  const params = schema.map((param) => {
    const value = given[param.name];
    if (value === undefined) {
      return { name: param.name, state: param.required ? "missing" : "default", value: null, label: param.label ?? param.name };
    }
    const problem = paramValueProblem(param, value);
    return {
      name: param.name,
      state: problem ? "invalid" : "ok",
      value,
      label: param.label ?? param.name,
      problem,
    };
  });
  // 参数名对不上了（触发器改了声明）—— 跟「值不合格」分开说：那是去改**触发器**。
  const unknown = Object.keys(given).filter((key) => !schema.some((param) => param.name === key));

  return {
    name: spec.name ?? null,
    trigger: {
      state: trigger ? "ok" : "invalid",
      ref: spec.trigger ?? null,
      value: trigger?.summary ?? null,
      version: trigger?.version ?? null,
      params,
      unknown,
    },
    session: {
      state: sessionsHas && !sessionsHas(spec.session) ? "invalid" : "ok",
      ref: spec.session ?? null,
    },
    enabled: spec.enabled !== false,
  };
}

/**
 * 面板要的那份「这条预设实际会跑成什么」。**面板不许自己算** —— 它一算就有两份规则。
 *
 * 状态只有三种：`ok` / `auto`（只有 model 有）/ `invalid`。
 */
export function resolveSpec(spec, ctx = {}) {
  const { skillsHas, extensionsHas, promptsHas, catalogHas, levelsOf, deviceModel } = ctx;

  const picked = spec.model ?? null;
  const model = picked
    ? {
        state: catalogHas && !catalogHas(picked) ? "invalid" : "ok",
        ref: `${picked.provider}/${picked.modelId}`,
        value: `${picked.provider}/${picked.modelId}`,
        thinkingLevels: levelsOf ? (levelsOf(picked) ?? []) : [],
      }
    : {
        state: "auto",
        ref: null,
        value: deviceModel ? `${deviceModel.provider}/${deviceModel.modelId}` : null,
        thinkingLevels: deviceModel && levelsOf ? (levelsOf(deviceModel) ?? []) : [],
      };

  const levels = model.thinkingLevels ?? [];
  const wanted = spec.thinkingLevel;
  const thinking = !wanted
    ? { state: "ok", ref: null, value: null }
    : !picked || levels.length === 0 || levels.includes(wanted)
      ? { state: "ok", ref: wanted, value: wanted }
      : // 钉了模型但档位不在它那几档：**不改值**，报红 —— 静默夹档正是要防的那种谎。
        { state: "invalid", ref: wanted, value: null };

  const skillRows = (spec.skills ?? []).map((ref) => ({
    state: skillsHas && !skillsHas(ref) ? "invalid" : "ok",
    ref,
  }));
  const extensionRows = (spec.extensions ?? []).map((ref) => ({
    state: ref.includes(":") || (extensionsHas && !extensionsHas(ref)) ? "invalid" : "ok",
    ref,
  }));
  // 三态在这儿展开成四种可读形状（default / none / picked / invalid），给面板画 chip 用。
  const template = !("template" in spec)
    ? { state: "ok", ref: null, value: null, mode: "default" }
    : spec.template === null
      ? { state: "ok", ref: null, value: null, mode: "none" }
      : spec.template === "auto"
        ? { state: "ok", ref: null, value: null, mode: "default" }
        : {
            state: promptsHas && !promptsHas(spec.template) ? "invalid" : "ok",
            ref: spec.template,
            value: spec.template,
            mode: "picked",
          };

  return {
    name: spec.name ?? null,
    model,
    thinking,
    skills: skillRows,
    extensions: extensionRows,
    template,
    readOnly: spec.readOnly === true,
    // 「键在不在」的五个事实，给面板画 chip 用（`默认` / `都不要` / `N 条`）。
    absence: {
      skills: spec.skills === undefined,
      extensions: spec.extensions === undefined,
      template: !("template" in spec),
      model: spec.model === undefined,
      thinkingLevel: spec.thinkingLevel === undefined,
      readOnly: spec.readOnly === undefined,
    },
  };
}

/**
 * `POST /v1/presets/validate` 和 `resolve` 共用：形状 + 引用，一次给全。
 *
 * @returns {{ ok: boolean, errors: string[], warnings: string[], spec: object|null, resolved: object|null }}
 */
export function validatePresetSpec(raw, ctx = {}, { dirName = null, where } = {}) {
  const problems = shapeProblems(raw, { dirName, ...(where ? { where } : {}) });
  if (problems.length) return { ok: false, errors: problems, warnings: [], spec: null, resolved: null };
  const { errors, warnings } = referenceProblems(raw, ctx);
  return { ok: errors.length === 0, errors, warnings, spec: raw, resolved: resolveSpec(raw, ctx) };
}

// ── 扫盘 → 内存名册 ────────────────────────────────────────────────────────

/** 一条 spec 里的类别，给面板分组用。没有 / 坏了都给 `[]`，不阻塞装载。 */
function categoryOf(spec) {
  if (!Array.isArray(spec?.category)) return [];
  return [...new Set(spec.category.map((entry) => String(entry ?? "").trim()).filter(Boolean))];
}

/**
 * 扫仓库根 → 内存。只在启动和每次写盘之后跑。
 *
 * 只认目录、跳过点开头（`.archive/` 天然不进）；没有 `preset.json` 的目录静默跳过
 * （可能是只写了一篇 PRESET.md 的笔记）；有 `preset.json` 但坏了 → `specError`，留着不扔。
 */
async function scan(dir, { manifest = "preset.json", check = shapeProblems } = {}) {
  let entries;
  try {
    entries = await readdir(dir, { withFileTypes: true });
  } catch {
    return []; // 仓库还没建：空名册，不是错误
  }

  const found = [];
  for (const entry of entries) {
    if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
    if (entry.name === "node_modules" || entry.name === "test") continue;

    const presetDir = path.join(dir, entry.name);
    let raw;
    try {
      raw = await readFile(path.join(presetDir, manifest), "utf8");
    } catch {
      continue; // 没有清单文件：不是这一段的一条
    }

    let spec = null;
    let specError = null;
    try {
      spec = JSON.parse(raw);
    } catch (error) {
      specError = `${manifest} 不是合法 JSON：${oneLine(error)}`;
    }
    if (!specError) {
      // 形状不过 = 这条不生效。**引用不过不算**：引用的东西可能只是这会儿没装，
      // 那是「现在开不了」不是「这条写坏了」—— 后者进 specError，前者由 resolve 报红。
      const problems = check(spec, { dirName: entry.name, where: `${entry.name}/${manifest}` });
      if (problems.length) specError = problems.join("；");
    }
    if (specError && !spec) spec = {};

    found.push({
      // 名字一律是目录名：`name === 目录名 === 会话里的键`（改名故意不做）。
      name: entry.name,
      dir: presetDir,
      spec,
      specError,
      summary: String(spec?.summary ?? "").trim(),
      description: String(spec?.description ?? "").trim(),
      category: categoryOf(spec),
      mtime: null,
    });
  }
  found.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
  return found;
}

/**
 * 仓里的段清单（`sections.json`）。一个仓按段放：`Session-Template` / `Ultimate-Template`。
 * **段 ID 就是文件夹名** —— 目录名 = git 路径 = URL 段 = 会话里的前缀，只有一份事实源。
 *
 * `checker: null` 的段（形状还没定）照样登记、照样出现在清单里：它只是现在装不了东西。
 */
export async function readSections(repoRoot) {
  let raw;
  try {
    raw = await readFile(path.join(repoRoot, "sections.json"), "utf8");
  } catch {
    // 没有 sections.json = 老的单段仓（预设直接躺在仓根）。当成只有一段，段名空串。
    return [{ id: "", title: "默认", manifest: "preset.json", checker: "preset" }];
  }
  const parsed = JSON.parse(raw);
  if (parsed?.v !== 1) throw new Error(`sections.json 的 v 不对：要 1，收到 ${JSON.stringify(parsed?.v)}`);
  if (!Array.isArray(parsed?.sections) || !parsed.sections.length) throw new Error("sections.json 缺 sections 数组");
  return parsed.sections.map((entry) => {
    const id = String(entry?.id ?? "").trim();
    if (!id || id !== path.basename(id) || id.startsWith(".")) {
      throw new Error(`sections.json 里的段 id「${id}」不合格：只能是文件夹名`);
    }
    return {
      id,
      title: String(entry.title ?? id),
      manifest: String(entry.manifest ?? "preset.json"),
      checker: entry.checker ?? null,
    };
  });
}

/**
 * 名册。**不是扩展** —— 没有 `createExtension`、没有 registry 副作用、没有 section。
 *
 * @param {{ dir: string }} options
 */
export async function createPresets({ dir, manifest = "preset.json", checker = "preset" }) {
  /** @type {Map<string, object>} */
  const roster = new Map();

  // 形状校验器按段选：会话预设用 shapeProblems，终极模板用 ultimateShapeProblems。
  const check = checker === "ultimate" ? ultimateShapeProblems : shapeProblems;

  const load = async () => {
    const found = await scan(dir, { manifest, check });
    roster.clear();
    for (const item of found) roster.set(item.name, item);
    return [...roster.values()];
  };

  await load();

  return {
    dir,
    load,
    list: () => [...roster.values()],
    has: (name) => roster.has(String(name ?? "")),
    get: (name) => roster.get(String(name ?? "")) ?? null,
  };
}

/**
 * 给 `resolve` / `validate` / 建会话造那个 ctx。**宿主侧的判定逻辑只此一处** ——
 * 面板的校验端点、建会话、列表页三处都走它，所以「面板说有效」和「启动能跑」是同一个判定。
 *
 * @param {{ skills?: object, prompts?: object, registry?: object, models?: object,
 *           levelsOf?: Function, defaultModel?: object }} deps
 */
export function presetContext({ skills, prompts, registry, models, levelsOf, defaultModel } = {}) {
  const installed = new Set(
    (() => {
      try {
        return (registry?.snapshot?.().installed?.() ?? []).map((item) => item.name);
      } catch {
        return [];
      }
    })(),
  );
  const modelOf = (ref) => {
    try {
      return models?.getModel?.(ref.provider, ref.modelId) ?? null;
    } catch {
      return null;
    }
  };
  return {
    skillsHas: (name) => Boolean(skills?.has?.(name)),
    promptsHas: (name) => Boolean(prompts?.has?.(name)),
    extensionsHas: (name) => installed.has(name),
    catalogHas: (ref) => Boolean(modelOf(ref)),
    levelsOf: (ref) => levelsOf?.(modelOf(ref)) ?? [],
    deviceModel: defaultModel ?? null,
  };
}

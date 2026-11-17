// 测试夹具：一台「什么都没装」的假机器。
//
// 内核启动和 `skills-api.js` 都会去 NOUS_EXTEND 找扩展仓；开发机上没有
// `/opt/Nous-extend` / `/opt/Nous-skills`，所以全套测试必须自己造最小桩。
// 这里用**同步 fs**，因为 ESM 的 import 提升会让 `skills-api.js` 的顶层
// `await import(NOUS_EXTEND/skills/index.js)` 在任何测试代码跑之前就求值。
//
// `test/index.js` 第一个 import 本文件；HTTP 测试也读这里设好的 env 起内核。
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

const root = mkdtempSync(path.join(tmpdir(), "nous-test-stubs-"));
const write = (rel, text) => {
  const file = path.join(root, rel);
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, text, "utf8");
};

// 真实 extend 的 defineExtension 是 identity；桩只给内核启动所需的最小形状。
// parseFrontmatter 必须是同款实现：skills-api.test.js 用真文件验它。
write(
  "extend/nous/index.js",
  `export default ({ defineExtension }) => defineExtension({ name: "nous" });\n`,
);
write(
  "extend/skills/index.js",
  [
    `export const SKILL_PREFIX = "skill:";`,
    `export const skillNamesOf = (agent) => (agent?.extensions ?? []).map((e) => (typeof e === "string" ? e : e?.name)).filter((n) => typeof n === "string" && n.startsWith(SKILL_PREFIX));`,
    String.raw`export function parseFrontmatter(text) {`,
    String.raw`  const match = /^\uFEFF?---[ \t]*\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/.exec(text);`,
    String.raw`  if (!match) return undefined;`,
    String.raw`  const lines = match[1].split(/\r?\n/);`,
    String.raw`  const out = {};`,
    String.raw`  for (let i = 0; i < lines.length; i++) {`,
    String.raw`    const kv = /^([A-Za-z][\w-]*):[ \t]*(.*)$/.exec(lines[i]);`,
    String.raw`    if (!kv) continue;`,
    String.raw`    let value = kv[2].trim();`,
    String.raw`    if (/^[>|][-+]?$/.test(value)) {`,
    String.raw`      const folded = value.startsWith(">");`,
    String.raw`      const block = [];`,
    String.raw`      while (i + 1 < lines.length && /^[ \t]+\S/.test(lines[i + 1])) block.push(lines[++i].trim());`,
    String.raw`      value = folded ? block.join(" ") : block.join("\n");`,
    String.raw`    } else if (value.length > 1 && ((value.startsWith(String.fromCharCode(34)) && value.endsWith(String.fromCharCode(34))) || (value.startsWith(String.fromCharCode(39)) && value.endsWith(String.fromCharCode(39))))) {`,
    String.raw`      value = value.slice(1, -1).replace(/\\(["\\])/g, "$1");`,
    String.raw`    }`,
    String.raw`    out[kv[1]] = value;`,
    String.raw`  }`,
    String.raw`  return out;`,
    String.raw`}`,
    `export async function createSkills({ api }) {`,
    `  const { defineExtension } = api;`,
    `  return {`,
    `    extension: defineExtension({ name: "skills" }),`,
    `    skillExtension: (name) => defineExtension({ name: SKILL_PREFIX + name }),`,
    `    list: () => [], load: async () => [], has: () => false, missing: (names) => names, render: () => undefined,`,
    `  };`,
    `}`,
  ].join("\n") + "\n",
);

write(
  "extend/prompts/index.js",
  [
    `export const PROMPT_PREFIX = "prompt:";`,
    `export const promptNamesOf = (agent) => (agent?.extensions ?? []).map((e) => (typeof e === "string" ? e : e?.name)).filter((n) => typeof n === "string" && n.startsWith(PROMPT_PREFIX));`,
    `export const templateOf = (agent) => promptNamesOf(agent)[0] ?? null;`,
    `export async function createPrompts({ api }) {`,
    `  const { defineExtension } = api;`,
    `  return {`,
    `    extension: defineExtension({ name: "prompts" }),`,
    `    promptExtension: (name) => defineExtension({ name: PROMPT_PREFIX + name }),`,
    `    list: () => [], load: async () => [], has: () => false, missing: (names) => names, render: () => undefined,`,
    `  };`,
    `}`,
  ].join("\n") + "\n",
);
for (const dir of ["skills", "prompts", "presets", "trigger"]) {
  mkdirSync(path.join(root, dir), { recursive: true });
}

/** 注入环境；已经设过（HTTP 测试自己造过桩）就不覆盖。 */
export const stubEnv = {
  NOUS_EXTEND: path.join(root, "extend"),
  NOUS_SKILLS: path.join(root, "skills"),
  NOUS_PROMPTS: path.join(root, "prompts"),
  NOUS_PRESETS: path.join(root, "presets"),
  NOUS_TRIGGER: path.join(root, "trigger"),
};
for (const [key, value] of Object.entries(stubEnv)) {
  process.env[key] ??= value;
}

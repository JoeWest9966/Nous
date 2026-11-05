// 测试夹具：临时 git 仓 + 假的 skills service + 假的 req/res。
//
// 全是 `node:` 内置模块，零依赖。每个测试自己造仓、自己删 —— 测试之间不共享状态。
// **不 import `server.js`**：那文件有顶层 await，import 一下就起服务、还要 sqlite。

import { execFile } from "node:child_process";
import { mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { promisify } from "node:util";
import path from "node:path";
import { Readable } from "node:stream";

export const run = promisify(execFile);

/** `git init` + 一条 commit + 用户身份（不写全局配置，测试不许污染这台机器）。 */
export const makeRepo = async ({ files = {}, commit = true } = {}) => {
  const root = await mkdtemp(path.join(tmpdir(), "nous-skills-test-"));
  await run("git", ["init", "-q"], { cwd: root });
  await run("git", ["symbolic-ref", "HEAD", "refs/heads/main"], { cwd: root });
  await run("git", ["config", "user.email", "test@example.com"], { cwd: root });
  await run("git", ["config", "user.name", "test"], { cwd: root });
  await run("git", ["config", "commit.gpgsign", "false"], { cwd: root });
  for (const [rel, text] of Object.entries(files)) await writeTree(root, rel, text);
  if (commit) await commitAll(root, "init");
  return root;
};

export const writeTree = async (root, rel, text) => {
  const file = path.join(root, rel);
  await mkdir(path.dirname(file), { recursive: true });
  await writeFile(file, text, "utf8");
};

export const commitAll = async (root, message) => {
  await run("git", ["add", "-A"], { cwd: root });
  // 空仓（一条文件都没有）也要有一个 HEAD：`rev-parse HEAD` 是每个查询的起点。
  await run("git", ["commit", "-q", "--allow-empty", "-m", message], { cwd: root });
};

export const head = async (root) => (await run("git", ["rev-parse", "HEAD"], { cwd: root })).stdout.trim();

export const status = async (root) => (await run("git", ["status", "--porcelain"], { cwd: root })).stdout;

export const showNames = async (root, rev = "HEAD") =>
  (await run("git", ["show", "--name-status", "--format=", rev], { cwd: root })).stdout
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);

export const exists = async (file) => Boolean(await stat(file).catch(() => null));

export const cleanup = (root) => rm(root, { recursive: true, force: true });

// ── 技能仓库的生成器 ────────────────────────────────────────────────────────
//
// 真仓库里 `index.mjs` 是票 A 的产物（`buildIndex(root)` + CLI）。这里放一个**同形状的
// 迷你版**：扫非点开头的目录、读 SKILL.md 的 frontmatter、校验失败就非 0 退出且不写文件。
// 测的是「写完之后有没有真的重扫」，不是那个文件本身 —— 所以它够用就行。
export const MINI_INDEX = `import { readdir, readFile, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(fileURLToPath(import.meta.url));
// 技能可以住在分组里（\`deprecated/<名字>/\`）。分组最多一层 —— 跟真生成器同一条规矩。
const findSkillRels = async (dir, rel = "", depth = 0) => {
  const out = [];
  const entries = await readdir(dir, { withFileTypes: true }).catch(() => []);
  for (const entry of entries.sort((a, b) => a.name.localeCompare(b.name))) {
    if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
    const childRel = rel ? rel + "/" + entry.name : entry.name;
    const childDir = path.join(dir, entry.name);
    if (await stat(path.join(childDir, "SKILL.md")).catch(() => null)) {
      out.push(childRel);
      continue;
    }
    if (depth < 1) out.push(...(await findSkillRels(childDir, childRel, depth + 1)));
  }
  return out;
};

const skills = [];
for (const rel of await findSkillRels(root)) {
  const leaf = rel.split("/").at(-1);
  const text = await readFile(path.join(root, rel, "SKILL.md"), "utf8");
  const front = /^\\uFEFF?---[ \\t]*\\r?\\n([\\s\\S]*?)\\r?\\n---/.exec(text);
  const fields = {};
  for (const line of (front?.[1] ?? "").split(/\\r?\\n/)) {
    const at = line.indexOf(":");
    if (at === -1) continue;
    fields[line.slice(0, at).trim()] = line.slice(at + 1).trim().replace(/^"|"$/g, "");
  }
  if (fields.name !== leaf) {
    console.error(rel + ": frontmatter 的 name 跟目录名对不上");
    process.exit(1);
  }
  if (!fields.description) {
    console.error(rel + ": description 是空的");
    process.exit(1);
  }
  const manifest = JSON.parse(await readFile(path.join(root, rel, "nous.json"), "utf8"));
  skills.push({ name: leaf, path: rel, description: fields.description, category: manifest.category ?? [] });
}
skills.sort((a, b) => a.name.localeCompare(b.name));
await writeFile(path.join(root, "index.json"), JSON.stringify({ v: 1, skills }, null, 2) + "\\n");
`;

export const CATEGORIES = JSON.stringify({ v: 1, categories: ["Basics"] }, null, 2) + "\n";

/**
 * 跟真 `Nous-skills/index.mjs` 同一道闸的生成器：**类别不在词表里就红**。
 *
 * `MINI_INDEX` 不查词表，于是「先写 nous.json 再补词表」那个顺序错误在它底下是绿的 ——
 * 真仓里却是永远 400（真生成器先拿词表校验）。这条测试专门钉死那个顺序。
 */
export const STRICT_INDEX = `import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(fileURLToPath(import.meta.url));
const vocab = new Set(JSON.parse(readFileSync(path.join(root, "categories.json"), "utf8")).categories ?? []);
const skills = [];
for (const entry of readdirSync(root, { withFileTypes: true })) {
  if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
  const manifest = JSON.parse(readFileSync(path.join(root, entry.name, "nous.json"), "utf8"));
  for (const category of manifest.category ?? []) {
    if (!vocab.has(category)) {
      console.error(entry.name + "/nous.json 的类别「" + category + "」不在 categories.json 词表里 —— 先登记类别");
      process.exit(1);
    }
  }
  skills.push({ name: entry.name, category: manifest.category ?? [] });
}
writeFileSync(path.join(root, "index.json"), JSON.stringify({ v: 1, skills }, null, 2) + "\\n");
`;

/**
 * 一个技能目录（两个文件），可以直接铺进仓里当 fixture。
 * `name` 可以是带分组的路径（`deprecated/demo`）—— 文件里写的名字永远是**最后一段**。
 */
export const skillFiles = (name, { description = `${name} 的说明`, category = [], summary = "" } = {}) => {
  const leaf = path.posix.basename(name);
  return {
    [`${name}/SKILL.md`]: `---\nname: ${leaf}\ndescription: ${JSON.stringify(description)}\n---\n\n# ${leaf}\n`,
    [`${name}/nous.json`]: JSON.stringify({ name: leaf, summary, category }, null, 2) + "\n",
  };
};

// ── 假的 skills service ─────────────────────────────────────────────────────
//
// 真扩展（`Nous-extend/skills/index.js`）的 `list()` 只做四件事：扫目录、读 frontmatter、
// 读 nous.json 的 category、判 failed。这里照那个形状做一份，好让 `skills.load()` 的
// 「写完之后内存名册有没有跟上」能被断言（`loads` 计数）。
export const makeSkills = (root) => {
  const state = { roster: [], loads: 0, failLoad: null };
  const scan = async () => {
    // 分组（`deprecated/<名字>/`）最多一层：跟真扩展的 findSkillRels 一个形状。
    const rels = [];
    const walk = async (base, rel, depth) => {
      let entries;
      try {
        entries = await readdir(base, { withFileTypes: true });
      } catch {
        return;
      }
      for (const entry of entries.filter((e) => e.isDirectory() && !e.name.startsWith("."))) {
        const childRel = rel ? `${rel}/${entry.name}` : entry.name;
        const childDir = path.join(base, entry.name);
        if (await stat(path.join(childDir, "SKILL.md")).catch(() => null)) {
          rels.push(childRel);
          continue;
        }
        if (depth < 1) await walk(childDir, childRel, depth + 1);
      }
    };
    await walk(root, "", 0);

    const found = [];
    for (const rel of rels) {
      const entry = { name: rel.split("/").at(-1) };
      const dir = path.join(root, rel);
      let text;
      try {
        text = await readFile(path.join(dir, "SKILL.md"), "utf8");
      } catch {
        continue;
      }
      const front = /^\uFEFF?---[ \t]*\r?\n([\s\S]*?)\r?\n---/.exec(text);
      const fields = {};
      for (const line of (front?.[1] ?? "").split(/\r?\n/)) {
        const at = line.indexOf(":");
        if (at === -1) continue;
        fields[line.slice(0, at).trim()] = line.slice(at + 1).trim().replace(/^"|"$/g, "");
      }
      let category = [];
      try {
        const manifest = JSON.parse(await readFile(path.join(dir, "nous.json"), "utf8"));
        category = Array.isArray(manifest.category) ? manifest.category : [];
      } catch {}
      const error =
        fields.name !== entry.name
          ? `name 跟目录名对不上`
          : !fields.description
            ? "description 是空的"
            : null;
      found.push({
        name: entry.name,
        path: rel,
        description: fields.description ?? "",
        dir,
        file: path.join(dir, "SKILL.md"),
        hidden: fields["disable-model-invocation"] === "true",
        category,
        load: error ? { state: "failed", error } : { state: "ok" },
      });
    }
    return found.sort((a, b) => a.name.localeCompare(b.name));
  };
  return {
    get loads() {
      return state.loads;
    },
    set failLoad(error) {
      state.failLoad = error;
    },
    dir: root,
    async load() {
      if (state.failLoad) throw new Error(state.failLoad);
      state.loads++;
      state.roster = await scan();
      return state.roster;
    },
    list: () => state.roster,
    has: (name) => state.roster.some((skill) => skill.name === name && skill.load.state === "ok"),
  };
};

// ── 假的 req / res ─────────────────────────────────────────────────────────

export const makeReq = (method, body) => {
  const payload = body === undefined ? [] : [Buffer.from(JSON.stringify(body), "utf8")];
  const stream = Readable.from(payload);
  stream.method = method;
  return stream;
};

export const makeRes = () => {
  const out = { status: null, headers: null, text: null, body: null };
  return {
    out,
    res: {
      writeHead(status, headers) {
        out.status = status;
        out.headers = headers;
      },
      end(text) {
        out.text = text ?? "";
        try {
          out.body = out.text ? JSON.parse(out.text) : null;
        } catch {
          out.body = null;
        }
      },
    },
  };
};

/** 一次完整的 `handleSkills` 调用；返回 { status, body }。 */
export const call = async (handleSkills, { method = "GET", path: target, body, root, skills, repo, data }) => {
  const url = new URL(`http://block.local${target}`);
  const parts = url.pathname.split("/").filter(Boolean);
  const req = makeReq(method, body);
  const { res, out } = makeRes();
  // `data` 只有上游镜像用得上（缓存落在块的 data/ 里）；不传就等于这台没有 data 目录。
  await handleSkills({ req, res, url, parts, root, skills, repo, data });
  return { status: out.status, body: out.body, raw: out.text };
};

// ── 提示词模板仓的夹具 ──────────────────────────────────────────────────────
//
// 跟技能仓同一套：临时 git 仓 + 一个同形状的迷你生成器 + 一个假 service。
// 生成器的校验比 `MINI_INDEX` 多两条（正文非空、settings.default 必须存在），
// 因为那正是模板仓和技能仓唯一不同的地方 —— 模板的正文就是提示词本身。
export const PROMPTS_MINI_INDEX = `import { readdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(fileURLToPath(import.meta.url));
const prompts = [];
for (const entry of await readdir(root, { withFileTypes: true })) {
  if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
  let body;
  try {
    body = await readFile(path.join(root, entry.name, "PROMPT.md"), "utf8");
  } catch {
    continue;
  }
  if (!body.trim()) {
    console.error(entry.name + ": PROMPT.md 是空的");
    process.exit(1);
  }
  const manifest = JSON.parse(await readFile(path.join(root, entry.name, "nous.json"), "utf8"));
  if (manifest.name !== entry.name) {
    console.error(entry.name + ": name 跟目录名对不上");
    process.exit(1);
  }
  if (typeof manifest.summary !== "string" || !manifest.summary.trim()) {
    console.error(entry.name + ": summary 是空的");
    process.exit(1);
  }
  prompts.push({ name: entry.name, summary: manifest.summary, category: manifest.category ?? [] });
}
prompts.sort((a, b) => a.name.localeCompare(b.name));
let settings = { default: null };
try {
  settings = JSON.parse(await readFile(path.join(root, "settings.json"), "utf8"));
} catch {}
if (settings.default && !prompts.some((prompt) => prompt.name === settings.default)) {
  console.error("settings.json 的 default 指向不存在的模板：" + settings.default);
  process.exit(1);
}
await writeFile(
  path.join(root, "index.json"),
  JSON.stringify({ v: 1, prompts, default: settings.default ?? null }, null, 2) + "\\n",
);
`;

/** 一条模板（两个文件），可以直接铺进仓里当 fixture。 */
export const promptFiles = (
  name,
  { summary = `${name} 的说明`, category = [], body = "Answer in one sentence.\n" } = {},
) => ({
  [`${name}/PROMPT.md`]: body,
  [`${name}/nous.json`]: JSON.stringify({ name, summary, category }, null, 2) + "\n",
});

// ── 假的 prompts service ────────────────────────────────────────────────────
//
// 真扩展（`Nous-extend/prompts/index.js`）的 `list()` 只做四件事：扫目录、读正文、
// 读 nous.json 的 category、判 failed。这里照那个形状做一份，好让「写完之后内存名册
// 有没有跟上」能被断言（`loads` 计数）。
export const makePrompts = (root) => {
  const state = { roster: [], loads: 0, failLoad: null };
  const scan = async () => {
    let entries;
    try {
      entries = await readdir(root, { withFileTypes: true });
    } catch {
      return [];
    }
    const found = [];
    for (const entry of entries) {
      if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
      const dir = path.join(root, entry.name);
      let body;
      try {
        body = await readFile(path.join(dir, "PROMPT.md"), "utf8");
      } catch {
        continue;
      }
      let manifest = {};
      let manifestError = null;
      try {
        manifest = JSON.parse(await readFile(path.join(dir, "nous.json"), "utf8"));
      } catch {
        manifestError = "缺 nous.json";
      }
      const error =
        manifestError ??
        (manifest.name !== entry.name
          ? "name 跟目录名对不上"
          : !String(manifest.summary ?? "").trim()
            ? "summary 是空的"
            : !body.trim()
              ? "PROMPT.md 是空的"
              : null);
      found.push({
        name: entry.name,
        summary: manifest.summary ?? "",
        dir,
        file: path.join(dir, "PROMPT.md"),
        bytes: Buffer.byteLength(body, "utf8"),
        category: Array.isArray(manifest.category) ? manifest.category : [],
        load: error ? { state: "failed", error } : { state: "ok" },
      });
    }
    return found.sort((a, b) => a.name.localeCompare(b.name));
  };
  return {
    get loads() {
      return state.loads;
    },
    set failLoad(error) {
      state.failLoad = error;
    },
    dir: root,
    async load() {
      if (state.failLoad) throw new Error(state.failLoad);
      state.loads++;
      state.roster = await scan();
      return state.roster;
    },
    list: () => state.roster,
    has: (name) => state.roster.some((prompt) => prompt.name === name && prompt.load.state === "ok"),
  };
};

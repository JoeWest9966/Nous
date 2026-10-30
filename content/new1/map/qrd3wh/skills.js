// skills.js —— 技能：装、名册、提示词。
//
// 一句话：**这里没有一行技能正文。** 技能是磁盘上的 md（`data/skills/<名>/SKILL.md`），
// 模型用 read 工具自己按需去读。本文件只干三件事：
//
//   装     —— 从 Nous Warehouse 编目挑一条，拉 tarball，落到 data/skills/<名>/
//   名册   —— 扫 frontmatter 进内存 Map（**不是每轮请求现扫盘**，那会把提示词缓存打死）
//   提示词 —— 一条 section，把「这条会话选中的技能」渲染成 <available_skills> XML
//
// 格式照抄 pi CLI（pi-coding-agent/dist/core/skills.js）：同一套 XML、同样让模型用
// read 取正文、同样让模型把相对路径按「SKILL.md 所在目录」展开。给 pi 写的 SKILL.md
// 直接能用，反过来也一样。
//
// 三条纪律：
//
//  1. **内容是数据，不是代码。** 一个技能 = 一个目录 + 一份 SKILL.md。它进提示词靠的是
//     `data/skills/` 里真有这个目录，不是靠哪个 ts 文件里写了它。
//  2. **「这条会话会什么」是配置，归 pi-durable 存。** 一个技能占一个空扩展名
//     （`skill:<名>`），durable 自己把它存进 `pi.agent.extensions` —— 于是 fork 自动继承、
//     重启自动活、加一条只追加一行 `pi.system` 增量。我们**不另建一张表**存这事。
//  3. **编目是唯一真相（nw），这里是投影。** 别读本机任何 skills.json：那台机器上已经
//     有两份会漂的账了（`/var/opt/Data-001/skills.json` 只有 1 条，8443 上有 38 条）。

import { execFile } from "node:child_process";
import { cp, mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { promisify } from "node:util";

import { defineExtension, section } from "@earendil-works/pi-durable";

const run = promisify(execFile);

/** git 不许交互：一次密码提示就能把整个装在后台挂到超时。 */
const GIT_ENV = { ...process.env, GIT_TERMINAL_PROMPT: "0", GIT_ASKPASS: "true", GIT_LFS_SKIP_SMUDGE: "1" };

// 编目在哪儿：跟 catalog.js 同一把 8443（免 token）。走 panel-gate（8080）才需要 NW_TOKEN。
const DEFAULT_BASE = "https://j1900.tail8d0018.ts.net:8443";

/** 一个技能在 `pi.agent.extensions` 里占的名字前缀。 */
export const SKILL_PREFIX = "skill:";

/**
 * 空名牌：**不含一个字的技能文本**，只为在 agent.extensions 里占一个名字。
 * 装 38 个技能，代码还是这一行。
 */
export const skillExtension = (name) => defineExtension({ name: SKILL_PREFIX + name });

/** 这条会话选了哪些技能（从 durable 存的扩展名里读，不是从我们的表里读）。 */
export const skillNamesOf = (agent) =>
  (agent?.extensions ?? [])
    .map((extension) => extension.name)
    .filter((name) => name.startsWith(SKILL_PREFIX))
    .map((name) => name.slice(SKILL_PREFIX.length));

const esc = (value) =>
  String(value).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

/**
 * SKILL.md 的 frontmatter。只认标量（`> ` / `|` 折叠块也收），不引 yaml 库：
 * 多一个依赖只为了读 4 个字段不值。嵌套映射读出来是字符串，反正我们也不用（metadata）。
 */
export function parseFrontmatter(text) {
  const match = /^\uFEFF?---[ \t]*\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/.exec(text);
  if (!match) return undefined;
  const lines = match[1].split(/\r?\n/);
  const out = {};
  for (let i = 0; i < lines.length; i++) {
    const kv = /^([A-Za-z][\w-]*):[ \t]*(.*)$/.exec(lines[i]);
    if (!kv) continue;
    let value = kv[2].trim();
    if (/^[>|][-+]?$/.test(value)) {
      const folded = value.startsWith(">");
      const block = [];
      while (i + 1 < lines.length && /^[ \t]+\S/.test(lines[i + 1])) block.push(lines[++i].trim());
      value = folded ? block.join(" ") : block.join("\n");
    } else if (value.length > 1 && ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'")))) {
      // 去壳 + 解转义：上游的 description 里常见 `\"review since X\"`，不解的话这串
      // 反斜杠会原样进提示词（模型读得懂，但那不是原文）。
      value = value.slice(1, -1).replace(/\\(["\\])/g, "$1");
    }
    out[kv[1]] = value;
  }
  return out;
}

/**
 * 技能目录。`dir` 是缓存盘（`data/skills`），`registry` 用来装卸空名牌。
 *
 * 注意 `render()` 是每轮请求都会跑的：它只查内存 Map，**不碰文件系统、不碰网络、
 * 不看时间**。任何一样漏进去，都会让每轮请求追加一条 pi.system 增量并打掉 provider
 * 的提示词缓存 —— 账单上看不出来，缓存命中率上看得很清楚。
 */
export async function createSkills({
  dir,
  registry,
  base = process.env.NW_BASE ?? DEFAULT_BASE,
  token = process.env.NW_TOKEN ?? "",
  fetchImpl = globalThis.fetch,
  githubToken = process.env.GH_TOKEN ?? process.env.GITHUB_TOKEN ?? "",
}) {
  const root = base.replace(/\/+$/, "");
  // 仓库缓存：只存 git 的提交图（--filter=blob:none），一条源几百 KB。
  // 放 data/.repos 而不是 data/skills：后者是名册，扫盘时不能看见一堆不是技能的目录。
  const cacheRoot = path.join(dir, "..", ".repos");
  const headers = token ? { authorization: `Bearer ${token}` } : {};
  const ghHeaders = { accept: "application/vnd.github+json", ...(githubToken ? { authorization: `Bearer ${githubToken}` } : {}) };

  /** 已装：name → { name, description, dir, file, hidden, receipt }。 */
  let installed = new Map();
  /** 可装：编目摊平后的条目。带 at / error —— 挂了要说清，别跟「编目里没有」长得一样。 */
  let catalog = { items: [], base: root, at: null, error: null };

  const sorted = () => [...installed.values()].sort((a, b) => a.name.localeCompare(b.name));

  const list = () =>
    sorted().map((skill) => ({
      name: skill.name,
      description: skill.description,
      dir: skill.dir,
      file: skill.file,
      hidden: skill.hidden,
      source: skill.receipt?.source ?? null,
      entry: skill.receipt?.entry ?? null,
      repo: skill.receipt?.repo ?? null,
      sha: skill.receipt?.sha ?? null,
      at: skill.receipt?.at ?? null,
      // 版本是**装下那个 sha 上的**事实，不是编目里的账面情报：
      // version = 仓库 package.json 声明的版本；commitsSince = 比 tag 晚几个提交（git describe）。
      version: skill.receipt?.version ?? null,
      tag: skill.receipt?.tag ?? null,
      commitsSince: skill.receipt?.commitsSince ?? null,
      describe: skill.receipt?.describe ?? null,
      commitAt: skill.receipt?.commitAt ?? null,
      skillChangedAt: skill.receipt?.skillChangedAt ?? null,
    }));

  const has = (name) => installed.has(name);

  /** 扫盘 → 内存。只在启动和装卸之后跑。 */
  const scan = async () => {
    let entries;
    try {
      entries = await readdir(dir, { withFileTypes: true });
    } catch {
      return [];
    }
    const found = [];
    for (const entry of entries) {
      if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
      const skillDir = path.join(dir, entry.name);
      const file = path.join(skillDir, "SKILL.md");
      let text;
      try {
        text = await readFile(file, "utf8");
      } catch {
        continue; // 没有 SKILL.md 的目录不是技能，静默跳过（可能是 .trash 之类）
      }
      const front = parseFrontmatter(text);
      const name = String(front?.name ?? entry.name).trim();
      const description = String(front?.description ?? "").trim();
      // 跟 pi CLI 一样：没描述的不进提示词 —— 模型只能靠描述判断「这事该不该用」，
      // 没有描述的技能摆在那儿只会白占 token。
      if (!name || !description) {
        console.warn(`[qrd3wh] 技能 ${entry.name} 缺 name/description，不加载`);
        continue;
      }
      let receipt = null;
      try {
        receipt = JSON.parse(await readFile(path.join(skillDir, ".nw.json"), "utf8"));
      } catch {}
      found.push({
        name,
        description,
        dir: skillDir,
        file,
        hidden: front?.["disable-model-invocation"] === "true",
        receipt,
      });
    }
    return found;
  };

  /** 把一份技能在注册表里占的名牌补上（装了但没名牌 = 选了也不生效）。 */
  const ensureStub = (name) => {
    const installedNow = registry.snapshot().extension(SKILL_PREFIX + name);
    if (!installedNow) registry.install(skillExtension(name));
  };

  const load = async () => {
    const found = await scan();
    installed = new Map(found.map((skill) => [skill.name, skill]));
    // 只补名牌，**不卸名牌**：盘上不见了但会话还选着的，让它静默失效，
    // 等用户自己卸 —— 免得一次扫盘把别人的选择悄悄改掉。
    for (const skill of found) ensureStub(skill.name);
    return list();
  };

  const render = (names) => {
    const picked = names
      .map((name) => installed.get(name))
      .filter((skill) => skill && !skill.hidden)
      .sort((a, b) => a.name.localeCompare(b.name));
    if (!picked.length) return undefined; // 一条都没选 = 整段不出现，别吐空的 <available_skills>
    return [
      "The following skills provide specialized instructions for specific tasks.",
      "Use the read tool to load a skill's file when the task matches its description.",
      "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.",
      "",
      "<available_skills>",
      ...picked.flatMap((skill) => [
        "  <skill>",
        `    <name>${esc(skill.name)}</name>`,
        `    <description>${esc(skill.description)}</description>`,
        `    <location>${esc(skill.file)}</location>`,
        "  </skill>",
      ]),
      "</available_skills>",
    ].join("\n");
  };

  const extension = defineExtension({
    name: "skills",
    // tag: false —— 跟 pi CLI 一样裸着进提示词，不加 <skills> 外壳，两边字节相同。
    sections: [section("skills", ({ agent }) => render(skillNamesOf(agent)), { tag: false })],
  });

  // ── 编目 ────────────────────────────────────────────────────────────────

  const refreshCatalog = async () => {
    const at = Date.now();
    try {
      const res = await fetchImpl(`${root}/api/source`, { headers });
      if (!res.ok) throw new Error(`编目回了 ${res.status}`);
      const body = await res.json();
      const sources = body?.data?.sources ?? [];
      const items = [];
      const seen = new Set();
      for (const source of sources) {
        if (source.kind && source.kind !== "skill") continue;
        for (const entry of source.entries ?? []) {
          // 同一个 repo 在编目里登记两遍是真实发生过的（matt / mattpocock）：
          // 按 repo + path 去重，别让用户看见两份一模一样的技能。
          const id = `${source.repo}#${entry.path}`;
          if (seen.has(id)) continue;
          seen.add(id);
          items.push({
            source: source.name,
            entry: entry.name,
            path: entry.path,
            repo: source.repo,
            category: source.category ?? [],
            // 编目里那个 version 是**账面情报**（注册那一刻上游最新 release），不是版本层，
            // 而且会过期（今天 matt 这条就写着 v1.3.1，实际已 v1.3.2）。只拿来对照着显示，
            // 绝不拿它冒充「你装的是哪一版」—— 那个只认收据。
            catalogVersion: source.version || null,
          });
        }
      }
      catalog = { items, base: root, at, error: null };
    } catch (error) {
      // 编目挂了要跟「编目里没有技能」分开说 —— 一个去修网络，一个去登记。
      catalog = { items: catalog.items, base: root, at, error: String(error?.message ?? error) };
    }
    return { items: catalog.items, base: catalog.base, at: catalog.at, error: catalog.error };
  };

  // ── 装 / 卸 ─────────────────────────────────────────────────────────────

  const repoSlug = (repo) => {
    const match = /github\.com[/:]([^/]+)\/([^/#?]+?)(?:\.git)?$/i.exec(String(repo ?? "").trim());
    return match ? { owner: match[1], name: match[2] } : null;
  };

  const headSha = async ({ owner, name }) => {
    const res = await fetchImpl(`https://api.github.com/repos/${owner}/${name}/commits?per_page=1`, { headers: ghHeaders });
    if (!res.ok) throw new Error(`取不到 ${owner}/${name} 的版本号（GitHub 回了 ${res.status}）`);
    const body = await res.json();
    const sha = Array.isArray(body) ? body[0]?.sha : undefined;
    if (!sha) throw new Error(`${owner}/${name} 没有提交记录`);
    return sha;
  };

  // ── 版本：只从「装下的那个 sha」上读，不猜、不借用编目 ────────────────────
  //
  // 一条外部源没有版本层（nw 自己的 ADR 就这么写的），技能目录里也没有 package.json。
  // 能拿到的最真的一句就是 `git describe`：v1.3.2-1-ga496f55 = 「v1.3.2 之后第 1 个提交」。
  // 所以这里**不碰 api.github.com**（匿名 60 次/小时，38 条就穿）：
  // blobless bare clone 走 git 协议，零配额，一条源 1-2 秒、几百 KB，之后复用。

  const git = (args) => run("git", args, { env: GIT_ENV, timeout: 180_000, maxBuffer: 8 << 20 });

  /** 源仓库的提交图缓存。返回目录，拿不到就 null。 */
  const repoCache = async (repo) => {
    const slug = repoSlug(repo);
    if (!slug) return null;
    const dest = path.join(cacheRoot, `${slug.owner}__${slug.name}.git`);
    const stamp = path.join(dest, ".nw-fetched");
    try {
      if (!(await stat(dest).catch(() => null))) {
        await mkdir(cacheRoot, { recursive: true });
        await git(["clone", "--bare", "--filter=blob:none", "--quiet", `https://github.com/${slug.owner}/${slug.name}`, dest]);
      } else {
        // tag 不是每分钟一个：十分钟内不重复 fetch，否则每装一条都在等网络。
        const last = Number(await readFile(stamp, "utf8").catch(() => "0")) || 0;
        if (Date.now() - last > 600_000) {
          await git(["-C", dest, "fetch", "--tags", "--force", "--quiet", "origin"]).catch(() => {});
        }
      }
      await writeFile(stamp, String(Date.now()));
      return dest;
    } catch {
      return null; // 断网、私有仓、git 版本太老：版本留空，装照装
    }
  };

  /**
   * 从缓存里读版本。四条，缺哪条留哪条空 —— 宁可显「未标版本」，不编。
   * `version` 是仓库声明的（package.json，拿不到时退回 tag 名去掉 v）。
   */
  const versionOf = async ({ repo, sha, path: entryPath }) => {
    const out = { version: null, tag: null, commitsSince: null, describe: null, commitAt: null, skillChangedAt: null };
    const cache = await repoCache(repo);
    if (!cache || !sha) return out;
    const g = (args) => git(["-C", cache, ...args]).then((r) => r.stdout.trim()).catch(() => "");

    // v1.3.2-1-ga496f55（--long 保证偶数个提交也带 -0-gxxx）
    out.describe = (await g(["describe", "--tags", "--long", "--abbrev=7", sha])) || null;
    const parsed = /^(.+)-(\d+)-g([0-9a-f]+)$/.exec(out.describe ?? "");
    if (parsed) {
      out.tag = parsed[1];
      out.commitsSince = Number(parsed[2]);
    }

    try {
      out.version = JSON.parse(await g(["show", `${sha}:package.json`]))?.version ?? null;
    } catch {}
    if (!out.version && out.tag) out.version = out.tag.replace(/^v/, "");

    out.commitAt = (await g(["log", "-1", "--format=%cI", sha])) || null;
    if (entryPath) out.skillChangedAt = (await g(["log", "-1", "--format=%cI", "--", entryPath])) || null;
    return out;
  };

  /**
   * 装一个技能：`{ source, entry }`（编目里的两个名字），或者直接 `{ repo, path }`。
   *
   * 内容是**文件**，落到 `data/skills/<名>/` —— 只存文本进数据库等于砍掉一半价值：
   * 技能里的 `./scripts/x.sh` 得能被 bash 真跑起来。
   */
  const install = async ({ source, entry, repo, path: entryPath }) => {
    let target = null;
    if (repo && entryPath) target = { source: source ?? null, entry: entry ?? path.basename(entryPath), path: entryPath, repo };
    else {
      const now = await refreshCatalog();
      if (now.error) throw new Error(`编目读不到：${now.error}`);
      target = now.items.find((item) => item.source === source && item.entry === entry);
      if (!target) throw new Error(`编目里没有 ${source}/${entry}`);
    }

    const slug = repoSlug(target.repo);
    if (!slug) throw new Error(`只支持 github 仓库，这个是：${target.repo}`);
    const sha = await headSha(slug);

    // 已经装过同一版 → 不重拉。
    const existing = installed.get(target.entry);
    if (existing && existing.receipt?.sha === sha && path.resolve(existing.dir) === path.resolve(dir, target.entry)) return list();

    const temp = await mkdtemp(path.join(tmpdir(), "nw-skill-"));
    try {
      const tarball = await fetchImpl(`https://codeload.github.com/${slug.owner}/${slug.name}/tar.gz/${sha}`, {
        headers: githubToken ? { authorization: `Bearer ${githubToken}` } : {},
      });
      if (!tarball.ok) throw new Error(`拉 ${slug.owner}/${slug.name}@${sha.slice(0, 7)} 失败（${tarball.status}）`);
      const file = path.join(temp, "src.tgz");
      await writeFile(file, Buffer.from(await tarball.arrayBuffer()));
      await run("tar", ["-xzf", file, "-C", temp], { timeout: 180_000 });
      const top = (await readdir(temp, { withFileTypes: true })).find((e) => e.isDirectory())?.name;
      if (!top) throw new Error("tarball 里没有目录");
      const from = path.join(temp, top, target.path);
      const info = await stat(from).catch(() => null);
      if (!info?.isDirectory()) throw new Error(`仓库里没有这个目录：${target.path}`);

      const dest = path.join(dir, target.entry);
      await rm(dest, { recursive: true, force: true });
      await mkdir(path.dirname(dest), { recursive: true });
      await cp(from, dest, { recursive: true });
      // 收据：编目**没有版本层**（nw 自己说的，编目不含 ref/@tag），所以「上次跑的是
      // 哪一版」只有我们记。跟模型密钥那条纪律同构：凭证只活内存，版本只活收据。
      //
      // 版本在现场算：装下的这个 sha 上，仓库自己写了什么、离最近的 tag 差几步。
      // 算不出来就是 null（断网 / 上游不打 tag / 私有仓）—— 页面上显「未标版本」，
      // 不让一个猜出来的 “1.0.0” 进收据。
      const version = await versionOf({ repo: target.repo, sha, path: target.path });
      await writeFile(
        path.join(dest, ".nw.json"),
        JSON.stringify(
          {
            source: target.source,
            entry: target.entry,
            repo: target.repo,
            path: target.path,
            sha,
            at: new Date().toISOString(),
            ...version,
          },
          null,
          2,
        ) + "\n",
      );
    } finally {
      await rm(temp, { recursive: true, force: true });
    }

    await load();
    if (!installed.has(target.entry)) throw new Error(`${target.entry} 的 SKILL.md 读不出 name/description`);
    return list();
  };

  const uninstall = async (name) => {
    const skill = installed.get(name);
    await rm(path.join(dir, name), { recursive: true, force: true });
    if (registry.snapshot().extension(SKILL_PREFIX + name)) registry.uninstall(skillExtension(name));
    await load();
    return { removed: Boolean(skill), installed: list() };
  };

  /**
   * 补齐老收据：这个字段是后加的，之前装的没有 version。装的时候不阻塞启动，
   * 装完也不阻塞页面 —— 由调用方 fire-and-forget，下一次 GET 就能看见。
   * 同一时刻只跑一份，重复触发直接返回空。
   */
  let backfilling = null;
  const backfill = () => {
    if (backfilling) return backfilling;
    const pending = sorted().filter((skill) => skill.receipt?.sha && skill.receipt.version === undefined);
    if (!pending.length) return Promise.resolve([]);
    backfilling = (async () => {
      const done = [];
      for (const skill of pending) {
        const version = await versionOf({ repo: skill.receipt.repo, sha: skill.receipt.sha, path: skill.receipt.path });
        await writeFile(
          path.join(skill.dir, ".nw.json"),
          JSON.stringify({ ...skill.receipt, ...version }, null, 2) + "\n",
        ).catch(() => {});
        done.push(skill.name);
      }
      await load();
      return done;
    })().finally(() => {
      backfilling = null;
    });
    return backfilling;
  };

  /** 选了但没装的技能名：给前端画「已失效」用，不静默吞掉。 */
  const missing = (names) => names.filter((name) => !installed.has(name));

  return { dir, list, has, load, refreshCatalog, install, uninstall, missing, backfill, extension, catalog: () => catalog };
}

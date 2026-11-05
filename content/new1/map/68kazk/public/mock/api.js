// D1 的假 qrd3wh。**只活在 D1 分支上，D2 必须连这个文件一起删掉。**
//
// app.js 里那个 `MOCK` 开关一开，所有 `v1/skills*` 请求都落在这儿：
// 读 public/mock/skills.json（spec §6.2 的响应原样），写进内存。所以不用等 qrd3wh 的端点
// 就能把「新建 / 改类别 / 改正文 / 发布 / 归档」整条 UI 流程点完 —— 刷新页面就回到原样。
//
// 接口形状照 spec §6.1。它不碰文件系统、不跑 git，语义够用就行：
// 它验证的是这一页，不是服务端。

const clone = (value) => JSON.parse(JSON.stringify(value));

const hash = (text) => {
  let h = 5381;
  for (let i = 0; i < text.length; i++) h = ((h * 33) ^ text.charCodeAt(i)) >>> 0;
  return h.toString(16).padStart(8, "0").slice(0, 8);
};

const normalize = (value) =>
  String(value ?? "")
    .split("/")
    .map((segment) => segment.trim())
    .filter(Boolean)
    .join("/");

const ancestors = (path) => {
  const parts = normalize(path).split("/").filter(Boolean);
  return parts.slice(0, -1).map((_, index) => parts.slice(0, index + 1).join("/"));
};

const under = (path, root) => path === root || path.startsWith(`${root}/`);

const failure = (status, error) => {
  const err = new Error(error);
  err.status = status;
  return err;
};

const skillSkeleton = (skill) =>
  [
    "---",
    `name: ${skill.name}`,
    `description: ${JSON.stringify(skill.description ?? "")}`,
    "---",
    "",
    `# ${skill.name}`,
    "",
    "正文。这一段进提示词：写清什么时候用、怎么用。",
    "",
  ].join("\n");

const manifestOf = (skill) =>
  `${JSON.stringify(
    { name: skill.name, summary: skill.description ?? "", category: skill.category ?? [] },
    null,
    2,
  )}\n`;

export function createMockTransport(mockData) {
  const data = clone(mockData);
  /** name → 相对路径 → 文本。第一次被读的时候造一份。 */
  const files = new Map();
  /** name → 相对路径 → HEAD 里那一版（没有 = 未跟踪）。 */
  const heads = new Map();
  let published = 0;

  const find = (name) => data.skills.find((skill) => skill.name === name);

  const filesOf = (skill) => {
    if (!files.has(skill.name)) {
      files.set(skill.name, {
        "SKILL.md": skillSkeleton(skill),
        "nous.json": manifestOf(skill),
      });
    }
    return files.get(skill.name);
  };

  const rewriteVocabulary = () => {
    const set = new Set();
    const add = (raw) => {
      const path = normalize(raw);
      if (!path) return;
      for (const parent of ancestors(path)) set.add(parent);
      set.add(path);
    };
    for (const raw of data.categories) add(raw);
    for (const skill of data.skills) for (const tag of skill.category ?? []) add(tag);
    data.categories = [...set];
  };

  const readSkillFile = (name, rel) => {
    const skill = find(name) ?? { name, description: "" };
    const map = filesOf(skill);
    if (!(rel in map)) throw failure(404, `${name}/${rel} 不存在`);
    return { name, path: rel, text: map[rel], etag: hash(map[rel]) };
  };

  /** `SKILL.md` 的 frontmatter 里 name 跟目录名不一致 = 装载失败（spec §2.1）。 */
  const refreshLoadState = (skill) => {
    const text = files.get(skill.name)?.["SKILL.md"] ?? skillSkeleton(skill);
    const said = /^---\n(?:[\s\S]*?\n)?name:\s*(.+)$/m.exec(text)?.[1]?.trim();
    const description = /^---\n(?:[\s\S]*?\n)?description:\s*(.+)$/m.exec(text)?.[1]?.trim() ?? "";
    if (!said || said !== skill.name) {
      skill.load = { state: "failed", error: `${skill.name}: SKILL.md 的 name 是「${said ?? "空"}」，跟目录名对不上` };
      return;
    }
    if (!description || description === '""' || description === "''") {
      skill.load = { state: "failed", error: `${skill.name}: description 是空的` };
      return;
    }
    skill.load = { state: "ok" };
    skill.description = description.replace(/^["']|["']$/g, "");
  };

  const routes = {
    "GET v1/skills": () => data,

    "POST v1/skills/categories": (body) => {
      const op = body?.op;
      const name = normalize(body?.name);
      const to = normalize(body?.to);
      if (!name) throw failure(400, "类别名不能为空");
      if (op === "add") {
        data.categories = [...new Set([...data.categories, ...ancestors(name), name])];
      } else if (op === "rename") {
        if (!to) throw failure(400, "改成什么？");
        const branch = (path) => (under(path, name) ? to + path.slice(name.length) : path);
        data.categories = [...new Set(data.categories.map(branch))];
        for (const skill of data.skills) skill.category = [...new Set((skill.category ?? []).map(branch))];
      } else if (op === "remove") {
        data.categories = data.categories.filter((path) => !under(path, name));
        for (const skill of data.skills) {
          skill.category = (skill.category ?? []).filter((path) => !under(path, name));
        }
      } else {
        throw failure(400, `没有这个操作：${op}`);
      }
      rewriteVocabulary();
      return { categories: data.categories, skills: data.skills };
    },

    "POST v1/skills/category": (body) => {
      const skill = find(body?.name);
      if (!skill) throw failure(404, `没有这条技能：${body?.name}`);
      skill.category = [...new Set((body?.category ?? []).map(normalize).filter(Boolean))];
      const map = files.get(skill.name);
      if (map) map["nous.json"] = manifestOf(skill);
      rewriteVocabulary();
      return { skill, skills: data.skills, categories: data.categories };
    },

    "POST v1/skills/create": (body) => {
      const name = String(body?.name ?? "").trim();
      if (!/^[a-z0-9][a-z0-9._-]*$/.test(name)) throw failure(400, "名字只能用字母、数字和 . _ -，且字母数字开头");
      if (find(name)) throw failure(400, `${name} 已经在仓库里了`);
      if (data.archived.includes(name)) throw failure(400, `.archive/${name} 还在，去「已归档」里恢复`);
      const skill = {
        name,
        description: String(body?.description ?? "").trim(),
        dir: `${data.root}/${name}`,
        file: `${data.root}/${name}/SKILL.md`,
        hidden: false,
        category: [...new Set((body?.category ?? []).map(normalize).filter(Boolean))],
        load: { state: "ok" },
        tracked: false,
      };
      data.skills = [...data.skills, skill];
      filesOf(skill);
      rewriteVocabulary();
      return { skill, skills: data.skills };
    },

    "POST v1/skills/publish": () => {
      published += 1;
      for (const skill of data.skills) {
        for (const [rel, text] of Object.entries(filesOf(skill))) {
          if (!heads.has(skill.name)) heads.set(skill.name, {});
          heads.get(skill.name)[rel] = text;
        }
        skill.tracked = true;
      }
      data.repo = { ...data.repo, dirty: false, ahead: 0 };
      data.repo.head = `9a${published}c0de`.slice(0, 7);
      return { ok: true, commit: { hash: data.repo.head, subject: `skills: 发布 ${published} 处改动` }, pushed: true, warnings: [] };
    },

    "POST v1/skills/archive": (body) => {
      const skill = find(body?.name);
      if (!skill) throw failure(404, `没有这条技能：${body?.name}`);
      data.skills = data.skills.filter((item) => item !== skill);
      data.archived = [...new Set([...data.archived, skill.name])].sort();
      return { ok: true, commit: { hash: "a4c41ve", subject: `skills: 归档 ${skill.name}` }, pushed: true };
    },

    "POST v1/skills/restore": (body) => {
      const name = String(body?.name ?? "");
      if (!data.archived.includes(name)) throw failure(404, `.archive/${name} 不在`);
      data.archived = data.archived.filter((item) => item !== name);
      const skill = {
        name,
        description: `${name}（从归档恢复）`,
        dir: `${data.root}/${name}`,
        file: `${data.root}/${name}/SKILL.md`,
        hidden: false,
        category: [],
        load: { state: "ok" },
        tracked: true,
      };
      data.skills = [...data.skills, skill];
      rewriteVocabulary();
      return { ok: true, commit: { hash: "be5t0re", subject: `skills: 恢复 ${name}` }, pushed: true };
    },

    "POST v1/skills/purge": (body) => {
      const name = String(body?.name ?? "");
      if (!data.archived.includes(name)) throw failure(404, `.archive/${name} 不在`);
      data.archived = data.archived.filter((item) => item !== name);
      files.delete(name);
      heads.delete(name);
      return { ok: true, commit: { hash: "pu2ge00", subject: `skills: 彻底删除 ${name}` }, pushed: true };
    },
  };

  const route = (url, method) => {
    const key = `${method} ${url.pathname.replace(/^\/+/, "")}`;
    return routes[key];
  };

  const run = (url, method, body) => {
    // 带查询串的那几个单独走，别把 URL 解析混进路由表。
    const pathname = url.pathname.replace(/^\/+/, "");
    if (method === "GET" && pathname === "v1/skills/files") {
      const name = url.searchParams.get("name") ?? "";
      const skill = find(name);
      if (!skill) throw failure(404, `没有这条技能：${name}`);
      const map = filesOf(skill);
      return { name, files: Object.entries(map).map(([path, text]) => ({ path, size: text.length })) };
    }
    if (method === "GET" && pathname === "v1/skills/file") {
      return readSkillFile(url.searchParams.get("name") ?? "", url.searchParams.get("path") ?? "");
    }
    if (method === "GET" && pathname === "v1/skills/pending/diff") {
      const path = url.searchParams.get("path") ?? "";
      const [name, ...rest] = path.split("/");
      const rel = rest.join("/");
      const current = readSkillFile(name, rel);
      return { path, before: heads.get(name)?.[rel] ?? null, after: current.text };
    }
    if (method === "PUT" && pathname === "v1/skills/file") {
      const name = body?.name ?? "";
      const rel = body?.path ?? "";
      const skill = find(name);
      if (!skill) throw failure(404, `没有这条技能：${name}`);
      const map = filesOf(skill);
      if (!(rel in map)) throw failure(404, `${name}/${rel} 不存在 —— 抽屉不建文件`);
      const current = hash(map[rel]);
      if (body?.base && body.base !== current) throw failure(409, `${rel} 在别处被改过了，先刷新再改`);
      map[rel] = String(body?.text ?? "");
      if (rel === "SKILL.md") refreshLoadState(skill);
      if (rel === "nous.json") {
        try {
          const manifest = JSON.parse(map[rel]);
          if (Array.isArray(manifest.category)) skill.category = manifest.category.map(normalize).filter(Boolean);
          if (typeof manifest.summary === "string") skill.summary = manifest.summary;
          rewriteVocabulary();
        } catch {
          throw failure(400, `${name}/nous.json 不是合法 JSON`);
        }
      }
      data.repo = { ...data.repo, dirty: true };
      return { etag: hash(map[rel]) };
    }
    const handler = route(url, method);
    if (!handler) throw failure(404, `mock 里没有 ${method} ${pathname}`);
    return handler(body);
  };

  const call = async (method, path, body) => {
    const url = new URL(path, "http://mock.local/");
    // 慢一点，让「读…」这类中间态真的出现一帧 —— 太快的话手点验收看不出加载态。
    await new Promise((resolve) => setTimeout(resolve, 60));
    return clone(run(url, method, body));
  };

  return {
    get: (path) => call("GET", path),
    post: (path, body) => call("POST", path, body),
    put: (path, body) => call("PUT", path, body),
  };
}

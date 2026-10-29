/**
 * 一个目录 = 一个项目。每个项目两份 json，各管一层：
 *
 * - `tree.json`   语义：块、组、父子。**唯一真相**，丢了就是真丢。
 * - `log.json`    最近 200 手：加、删、改名、撤销。丢了只是没历史。
 * - `map/<id>/`   每个节点一个文件夹。**派生 + 用户内容**：文件夹跟着树长，
 *                 里面的东西是自己的，程序不碰。对账在 `map.ts`。
 *
 * 语义没有第三份。布局（谁在哪、多大）是 `grid()` 从宽度算出来的派生数据，
 * 丢了重算一次就有 —— 写它等于把衍生物存成事实源。等真能手动调尺寸那天再议。
 *
 * 这个文件不 import 任何浏览器代码 —— 它能被 vite 插件用，也能被真后端用。
 * 它不知道 HTTP 长什么样，HTTP 在 `api.ts` 里。
 */
import { mkdir, readdir, readFile, rename, stat, writeFile } from "node:fs/promises";
import { createHash, randomBytes } from "node:crypto";
import path from "node:path";
import { adoptNode, freshId, hasNode, isNodeId, syncMap } from "./map.ts";
import { isDocName, normalizeDocName } from "../src/docname.ts";
import { audit, readLocks, restoreLocked, writeLocks } from "./locks.ts";

export type BoardSummary = {
  id: string;
  name: string;
  version: number;
  updatedAt: string;
};

export type StoredTree = {
  v: 1;
  name: string;
  version: number;
  updatedAt: string;
  root: unknown;
};

export type SaveResult =
  | { status: "ok"; version: number; root: unknown; restored: string[] }
  | { status: "conflict"; version: number }
  | { status: "missing" };

/** id 同时也是目录名，所以白名单同时防路径穿越和防跨平台怪字符。 */
const ID_OK = /^[a-z0-9][a-z0-9-]{0,63}$/;

const TREE = "tree.json";
const LOG = "log.json";
const LOG_LIMIT = 200;
/** 每个节点的文件夹里那份人写的东西。名字在 `map.ts` 里定过，这里同一个。 */
const MAP = "map";
/** coding agent 进目录时自动找的那一份。**它和别的文档一样，写了才有** —— 不预生成。 */
const DOC = "AGENTS.md";
/**
 * 判「是不是入口」一律不分大小写：macOS 上 `agents.md` 和 `AGENTS.md` 是同一个文件，
 * Linux 上是两个。让一个名字在两台机器上是两件事，是 bug 的温床。
 */
const isEntry = (name: string): boolean => name.toLowerCase() === DOC.toLowerCase();
/** 删掉的文档进这里。**不真删** —— 和节点的 `.gone/` 一个道理。 */
const TRASH = ".trash";

export const isBoardId = (id: string): boolean => ID_OK.test(id);


export type DocFile = {
  name: string;
  size: number;
  at: string;
  /** `AGENTS.md` —— 存在时能改，不能删、不能改名。不存在就是不存在。 */
  protected: boolean;
  trashed: boolean;
  /** 搜索命中时那一行的上下文。 */
  snippet?: string;
};

export type DocListResult =
  | { status: "ok"; files: DocFile[]; hasApp: boolean }
  | { status: "no-board" }
  | { status: "no-node" };

export type DocResult =
  | { status: "ok"; text: string; etag: string; hasApp: boolean }
  | { status: "no-board" }
  | { status: "no-node" }
  | { status: "no-file" };

export type DocSaveResult =
  | { status: "ok"; etag: string }
  | { status: "conflict"; etag: string; text: string }
  | { status: "no-board" }
  | { status: "no-node" };

export type DocEditResult =
  | { status: "ok"; name: string }
  | { status: "no-board" }
  | { status: "no-node" }
  | { status: "bad-name" }
  | { status: "missing" }
  /** 这份不许动：`AGENTS.md` 只能改内容。 */
  | { status: "protected" }
  | { status: "exists" };

/** 磁盘上没有那份文件时，写回的底就是这个。老客户端可能还带着它来 —— 认。 */
const MISSING_ETAG = "stub";

/** 内容哈希，取前 16 位就够认人了 —— 比 mtime 可靠，也不依赖时钟。 */
const etagOf = (text: string): string =>
  createHash("sha1").update(text, "utf8").digest("hex").slice(0, 16);

/** 原子写：写临时文件再 rename。断电只会留下一个孤儿 tmp，不会留下半截 json。 */
async function writeAtomic(file: string, body: string): Promise<void> {
  const tmp = `${file}.${randomBytes(4).toString("hex")}.tmp`;
  await writeFile(tmp, body, "utf8");
  await rename(tmp, file);
}

const writeJson = (file: string, value: unknown): Promise<void> =>
  writeAtomic(file, `${JSON.stringify(value, null, 2)}\n`);

const readJson = async (file: string): Promise<unknown | undefined> => {
  try {
    return JSON.parse(await readFile(file, "utf8")) as unknown;
  } catch {
    return undefined;
  }
};

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

export class BoardStore {
  /** content/ 的绝对路径由调用方给 —— 这里不许出现硬编码路径。 */
  constructor(readonly root: string) {}

  private dir(id: string): string {
    const p = path.resolve(this.root, id);
    // 双保险：正则已经挡了 `../`，但路径拼接这东西值得再确认一次。
    if (path.dirname(p) !== path.resolve(this.root)) throw new Error("bad board id");
    return p;
  }

  /** 一个块的文件夹。**id 就是文件夹名** —— 整套东西的魔力全在这一行。 */
  folder(id: string, nodeId: string): string | undefined {
    if (!isNodeId(nodeId)) return undefined;
    return path.join(this.dir(id), MAP, nodeId);
  }

  /** 块在磁盘上的家。给「终端路径」用的 —— 浏览器不知道 content/ 在哪儿，只有服务器知道。 */
  absDir(id: string): string {
    return this.dir(id);
  }

  async list(): Promise<BoardSummary[]> {
    let names: string[] = [];
    try {
      names = (await readdir(this.root, { withFileTypes: true }))
        .filter((e) => e.isDirectory() && isBoardId(e.name))
        .map((e) => e.name);
    } catch {
      return [];
    }

    const out: BoardSummary[] = [];
    for (const id of names) {
      const tree = await readJson(path.join(this.dir(id), TREE));
      const t = isRecord(tree) ? tree : {};
      const updatedAt =
        typeof t.updatedAt === "string"
          ? t.updatedAt
          : (await stat(path.join(this.dir(id), TREE)).catch(() => null))?.mtime.toISOString() ??
            new Date(0).toISOString();
      out.push({
        id,
        name: typeof t.name === "string" && t.name ? t.name : id,
        version: typeof t.version === "number" ? t.version : 0,
        updatedAt,
      });
    }
    // 最近动过的排前面，人找的是「刚才那一盘」。
    return out.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  }

  async tree(id: string): Promise<StoredTree | undefined> {
    const raw = await readJson(path.join(this.dir(id), TREE));
    if (!isRecord(raw) || !("root" in raw)) return undefined;
    return {
      v: 1,
      name: typeof raw.name === "string" ? raw.name : id,
      version: typeof raw.version === "number" ? raw.version : 0,
      updatedAt: typeof raw.updatedAt === "string" ? raw.updatedAt : new Date(0).toISOString(),
      root: raw.root,
    };
  }

  async log(id: string): Promise<unknown[]> {
    const raw = await readJson(path.join(this.dir(id), LOG));
    return isRecord(raw) && Array.isArray(raw.moves) ? raw.moves : [];
  }

  /**
   * 这块自己有没有代码。判定**只看文件夹**（`package.json` 的 `scripts.start`）——
   * 不往 `tree.json` 里写「这是个应用块」，那会变成第二份真相。
   */
  async startCmd(id: string, nodeId: string): Promise<string> {
    const folder = this.folder(id, nodeId);
    if (!folder) return "";
    try {
      const pkg = JSON.parse(await readFile(path.join(folder, "package.json"), "utf8")) as {
        scripts?: Record<string, unknown>;
      };
      const cmd = pkg.scripts?.["start"];
      return typeof cmd === "string" ? cmd.trim() : "";
    } catch {
      return "";
    }
  }

  /**
   * 这个块名下的文档：文件夹里的 `*.md`，加上 `.trash/` 里的。
   *
   * **文件夹就是清单。** 不存 index、不往 `tree.json` 写一行 —— 你 `touch` 一个
   * `ideas.md` 进去，下次打开它就在列表里；删掉文件它就不在。
   *
   * **没有例外。** `AGENTS.md` 也不凭空出现：文件夹空着，列表就空着。
   * 一个刚建出来的块，文档页上就该是一片空 —— 那是「还没写」，不是坏了。
   */
  async docs(id: string, nodeId: string, q = ""): Promise<DocListResult> {
    if (!isNodeId(nodeId)) return { status: "no-node" };
    const tree = await this.tree(id);
    if (!tree) return { status: "no-board" };
    const root = adoptNode(tree.root);
    if (!root || !hasNode(root, nodeId)) return { status: "no-node" };

    const folder = path.join(this.dir(id), MAP, nodeId);
    const hasApp = (await this.startCmd(id, nodeId)) !== "";
    const needle = q.trim().toLowerCase();

    const read = async (dir: string, trashed: boolean): Promise<DocFile[]> => {
      const entries = await readdir(dir, { withFileTypes: true }).catch(() => []);
      const out: DocFile[] = [];
      for (const e of entries) {
        if (!e.isFile() || !isDocName(e.name)) continue;
        const full = path.join(dir, e.name);
        const st = await stat(full).catch(() => null);
        if (!st) continue;
        const hit = needle ? await this.matchIn(full, needle) : undefined;
        if (needle && !e.name.toLowerCase().includes(needle) && !hit) continue;
        out.push({
          name: e.name,
          size: st.size,
          at: st.mtime.toISOString(),
          protected: isEntry(e.name),
          trashed,
          ...(hit ? { snippet: hit } : {}),
        });
      }
      return out;
    };

    const live = await read(folder, false);
    const gone = await read(path.join(folder, TRASH), true);

    // 入口只是排在前面，不是凭空多一行：它得先在磁盘上存在。
    live.sort((a, b) =>
      a.protected === b.protected ? a.name.localeCompare(b.name) : a.protected ? -1 : 1,
    );

    return { status: "ok", files: [...live, ...gone.sort((a, b) => a.name.localeCompare(b.name))], hasApp };
  }

  /** 命中就有上下文，没命中有名字也不算白搜。返回那一行，前后各截 40 字。 */
  private async matchIn(file: string, needle: string): Promise<string | undefined> {
    const text = await readFile(file, "utf8").catch(() => "");
    const at = text.toLowerCase().indexOf(needle);
    if (at < 0) return undefined;
    const from = Math.max(0, text.lastIndexOf("\n", at) + 1);
    const to = text.indexOf("\n", at);
    const line = text.slice(from, to < 0 ? text.length : to).trim();
    return line.length > 90 ? `${line.slice(0, 90)}…` : line;
  }

  /**
   * 一块的正文：`map/<nodeId>/<name>`（`name` 省略 = 入口那份）。
   *
   * 三件事值得写在这儿：
   * - **树里没有这节点就不给看。** 目录可以被手动删、可以躺在 `.gone/` 里，
   *   不能让一个旧书签顺着 id 把死节点读活。
   * - **文件不在就是不在。** 返回 `no-file`，不渲染任何说明书 —— 看一眼既不该
   *   长出一个文件，也不该看到一份没人写过的正文。
   * - **etag 是内容哈希，不是 mtime。** mtime 在 git checkout / rsync 下会乱。
   */
  async doc(id: string, nodeId: string, name = DOC): Promise<DocResult> {
    // 双保险：api.ts 已经验过，但拼路径之前再过一道 —— 一个坏 id 能读到盘外面去。
    if (!isNodeId(nodeId) || !isDocName(name)) return { status: "no-node" };
    const tree = await this.tree(id);
    if (!tree) return { status: "no-board" };
    const root = adoptNode(tree.root);
    if (!root || !hasNode(root, nodeId)) return { status: "no-node" };

    const hasApp = (await this.startCmd(id, nodeId)) !== "";
    try {
      const text = await readFile(path.join(this.dir(id), MAP, nodeId, name), "utf8");
      return { status: "ok", text, etag: etagOf(text), hasApp };
    } catch {
      return { status: "no-file" };
    }
  }

  /**
   * 写一块的正文。**只看自己那份文件的 etag，不看 board 的 version** ——
   * 删一块不该让一篇文章变 409，反过来也一样。
   *
   * 冲突不是失败：把磁盘上那份一起递回去，让人当面选，而不是默默盖掉。
   */
  async saveDoc(
    id: string,
    nodeId: string,
    text: string,
    baseEtag: string,
    name = DOC,
  ): Promise<DocSaveResult> {
    if (!isNodeId(nodeId)) return { status: "no-node" };
    const tree = await this.tree(id);
    if (!tree) return { status: "no-board" };
    const root = adoptNode(tree.root);
    // 树里没有的节点不写：`.gone/` 里躺着的不能被一个旧页面写活。
    if (!root || !hasNode(root, nodeId)) return { status: "no-node" };

    const file = path.join(this.dir(id), MAP, nodeId, name);
    let current = "";
    let currentEtag = MISSING_ETAG;
    try {
      current = await readFile(file, "utf8");
      currentEtag = etagOf(current);
    } catch {
      // 文件不在（文件夹被手动删过、或者从来就写）：认那个「没有」的 etag。
    }

    if (currentEtag !== baseEtag) {
      return { status: "conflict", etag: currentEtag, text: current };
    }
    if (current === text) return { status: "ok", etag: currentEtag };

    await mkdir(path.dirname(file), { recursive: true });
    await writeAtomic(file, text);
    return { status: "ok", etag: etagOf(text) };
  }

  /** 新建一份。写一行的正文（`# 名字`）—— 空文件在列表里是一行空白，看着像坏了。 */
  async createDoc(id: string, nodeId: string, raw: string): Promise<DocEditResult> {
    if (!isNodeId(nodeId)) return { status: "no-node" };
    // 规矩只有一份（`src/docname.ts`）：补 `.md`、压平空格、滤掉怪字符都在那里。
    const typed = normalizeDocName(raw);
    if (!typed || !isDocName(typed)) return { status: "bad-name" };
    // 入口名没有禁令：新块一开始就没有 `AGENTS.md`，想有就自己建一份。
    // 大小写一律归到正经写法 —— `agents.md` 和 `AGENTS.md` 在两台机器上不该是两件事。
    const name = isEntry(typed) ? DOC : typed;
    const tree = await this.tree(id);
    if (!tree) return { status: "no-board" };
    const root = adoptNode(tree.root);
    if (!root || !hasNode(root, nodeId)) return { status: "no-node" };

    const folder = path.join(this.dir(id), MAP, nodeId);
    const file = path.join(folder, name);
    // 同名文件已经在 → 不覆盖。重名是个正常的输入错误，不是冲突。
    if (await stat(file).then(() => true).catch(() => false)) return { status: "exists" };
    await mkdir(folder, { recursive: true });
    await writeAtomic(file, `# ${name.replace(/\.md$/i, "")}\n\n`);
    return { status: "ok", name };
  }

  /**
   * 删一份 → 挪进 `map/<id>/.trash/`。**不真删** —— 和节点的 `.gone/` 同一套逻辑：
   * 想恢复的时候，内容得还在。
   *
   * 回收站里重名就加 `-2` `-3`：文件名就是它的身份，**不藏时间戳、不做任何解析** ——
   * 恢复的时候原样搬回去，看到什么名字就是什么名字。
   */
  async trashDoc(id: string, nodeId: string, name: string): Promise<DocEditResult> {
    if (!isNodeId(nodeId)) return { status: "no-node" };
    if (!isDocName(name)) return { status: "bad-name" };
    // AGENTS.md 能改，不能删。这一层是硬的 —— UI 不给入口，服务端也不给过。
    if (isEntry(name)) return { status: "protected" };
    const tree = await this.tree(id);
    if (!tree) return { status: "no-board" };
    const root = adoptNode(tree.root);
    if (!root || !hasNode(root, nodeId)) return { status: "no-node" };

    const folder = path.join(this.dir(id), MAP, nodeId);
    const file = path.join(folder, name);
    if (!(await stat(file).then((s) => s.isFile()).catch(() => false))) return { status: "missing" };

    const bin = path.join(folder, TRASH);
    await mkdir(bin, { recursive: true });
    const stem = name.slice(0, -3);
    let target = path.join(bin, name);
    for (let n = 2; n < 1000; n++) {
      if (!(await stat(target).then(() => true).catch(() => false))) break;
      target = path.join(bin, `${stem}-${n}.md`);
    }
    await rename(file, target);
    return { status: "ok", name };
  }

  /** 从回收站搬回来，名字一个字符不改。同名还在就回不去 —— 宁可报错，也不默默改名。 */
  async restoreDoc(id: string, nodeId: string, name: string): Promise<DocEditResult> {
    if (!isNodeId(nodeId)) return { status: "no-node" };
    if (!isDocName(name)) return { status: "bad-name" };
    const tree = await this.tree(id);
    if (!tree) return { status: "no-board" };
    const root = adoptNode(tree.root);
    if (!root || !hasNode(root, nodeId)) return { status: "no-node" };

    const folder = path.join(this.dir(id), MAP, nodeId);
    const from = path.join(folder, TRASH, name);
    if (!(await stat(from).then((s) => s.isFile()).catch(() => false))) return { status: "missing" };

    const to = path.join(folder, name);
    if (await stat(to).then(() => true).catch(() => false)) return { status: "exists" };
    await rename(from, to);
    return { status: "ok", name };
  }

  /**
   * 改名 = 把文件挪到同一个文件夹里的新名字上。**不走回收站、不新写一份** ——
   * 名字就是身份，内容一个字节都不该因为改名而重新落盘（etag 也就不会变，打开着的页面不会 409）。
   *
   * 两头都拦：`AGENTS.md` 改不了名（它就是入口那个文件名），也改不成 `AGENTS.md`
   * （那是凭空造一个 agent 入口出来）。
   */
  async renameDoc(id: string, nodeId: string, name: string, raw: string): Promise<DocEditResult> {
    if (!isNodeId(nodeId)) return { status: "no-node" };
    if (!isDocName(name)) return { status: "bad-name" };
    if (isEntry(name)) return { status: "protected" };
    // 和服务端别处一样，规矩只有 `src/docname.ts` 那一份。
    const to = normalizeDocName(raw);
    if (!to || !isDocName(to)) return { status: "bad-name" };
    if (isEntry(to)) return { status: "protected" };
    const tree = await this.tree(id);
    if (!tree) return { status: "no-board" };
    const root = adoptNode(tree.root);
    if (!root || !hasNode(root, nodeId)) return { status: "no-node" };

    const folder = path.join(this.dir(id), MAP, nodeId);
    const from = path.join(folder, name);
    if (!(await stat(from).then((s) => s.isFile()).catch(() => false))) return { status: "missing" };
    if (to === name) return { status: "ok", name };

    const target = path.join(folder, to);
    // 只改大小写（`notes.md` → `Notes.md`）：Linux 上是两个文件，macOS 上是同一个。
    // 同一个就同一个，直接改；别的撞名才算撞名。
    const sameFile = to.toLowerCase() === name.toLowerCase();
    if (!sameFile && (await stat(target).then(() => true).catch(() => false))) {
      return { status: "exists" };
    }
    await rename(from, target);
    return { status: "ok", name: to };
  }

  /** 这一盘有哪些块被锁了。前端一次拿全，别一个一个问。 */
  async locks(id: string): Promise<string[]> {
    if (!(await this.tree(id))) return [];
    return readLocks(this.dir(id));
  }

  /**
   * 上锁 / 解锁。块和组都能锁 —— 锁的是**这个节点**：它不许从树里消失，
   * 它的孩子不受影响（锁住的组里新建的块照样能删）。
   *
   * **只有树里还有的节点才给上锁** —— 锁一个已经不存在的东西没有意义，
   * 而且会变成一把永远解不开的空头锁。解锁不管这个：解一把旧锁得随时都行。
   */
  async setLock(
    id: string,
    nodeId: string,
    locked: boolean,
    actor = "",
  ): Promise<"ok" | "no-board" | "no-node"> {
    const tree = await this.tree(id);
    if (!tree) return "no-board";
    if (!isNodeId(nodeId)) return "no-node";
    const ids = await readLocks(this.dir(id));
    const has = ids.includes(nodeId);
    if (locked) {
      const root = adoptNode(tree.root);
      if (!root || !hasNode(root, nodeId)) return "no-node";
      if (has) return "ok";
      await writeLocks(this.dir(id), [...ids, nodeId]);
      await audit(this.dir(id), `上锁 ${nodeId}${actor ? ` (by ${actor})` : ""}`);
      return "ok";
    }
    if (!has) return "ok";
    await writeLocks(
      this.dir(id),
      ids.filter((x) => x !== nodeId),
    );
    await audit(this.dir(id), `解锁 ${nodeId}${actor ? ` (by ${actor})` : ""}`);
    return "ok";
  }

  /**
   * 写之前先比对版本。文件是唯一真相，覆盖等于真丢数据 ——
   * 两个标签页同时开着的时候，这个检查就是唯一拦住静默损失的东西。
   */
  async save(
    id: string,
    input: { root: unknown; moves: unknown[]; baseVersion: number },
    from = "",
  ): Promise<SaveResult> {
    const current = await this.tree(id);
    if (!current) return { status: "missing" };
    if (current.version !== input.baseVersion) {
      return { status: "conflict", version: current.version };
    }

    // 锁定的节点不许从树里消失。客户端发什么都会被修正一遍再落盘 ——
    // 这是「防机器」的那一层：所有树写入都必经这里。
    //
    // 一把锁只管一个 id；它要是没了，`restoreLocked` 把它接回最近的存活祖先，
    // 整枝被删时连它的孩子一起还回来（那些孩子不是靠锁回来的，是靠「这一手根本没成」）。
    // 单独摘走一个没锁的孩子？照旧放行。
    const locked = new Set(await readLocks(this.dir(id)));
    let root = input.root;
    let restored: string[] = [];
    if (locked.size) {
      const prev = adoptNode(current.root);
      const wanted = adoptNode(input.root);
      if (prev && wanted) {
        const fixed = restoreLocked(wanted, prev, locked);
        root = fixed.root;
        restored = fixed.restored;
        if (restored.length) {
          await audit(
            this.dir(id),
            `被删的锁定节点被接回: ${restored.join(", ")}${from ? ` (from ${from})` : ""}`,
          );
        }
      }
    }

    const next: StoredTree = {
      v: 1,
      name: current.name,
      version: current.version + 1,
      updatedAt: new Date().toISOString(),
      root,
    };
    // 顺序要紧：先落语义，再落历史。历史落后了只是少一行，语义落后了是丢盘。
    await writeJson(path.join(this.dir(id), TREE), next);
    await writeJson(path.join(this.dir(id), LOG), {
      v: 1,
      moves: input.moves.slice(-LOG_LIMIT),
    });
    await this.syncMap(id, root);
    return { status: "ok", version: next.version, root, restored };
  }

  /**
   * 把 `map/` 校准成这棵树。**失败不许影响保存** —— tree.json 已经落盘了，
   * 让一个笔记目录把这次保存打成 500，等于告诉用户「刚才那手没存上」，那是谎话。
   */
  private async syncMap(id: string, root: unknown): Promise<void> {
    try {
      await syncMap(this.dir(id), root);
    } catch (e) {
      console.warn(`[map] ${id} 对账失败（tree.json 已存好）:`, e);
    }
  }

  /** 新项目：两份 json + map/ 一次建齐。 */
  async create(name: string, wantId?: string): Promise<BoardSummary> {
    await mkdir(this.root, { recursive: true });
    const slug =
      name
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "-")
        .replace(/^-+|-+$/g, "")
        .slice(0, 40);
    const base = wantId && isBoardId(wantId) ? wantId : slug || `board-${randomBytes(2).toString("hex")}`;

    const taken = new Set((await this.list()).map((b) => b.id));
    let id = base;
    for (let i = 2; taken.has(id); i++) id = `${base}-${i}`;
    if (!isBoardId(id)) throw new Error("bad board id");

    const now = new Date().toISOString();
    await mkdir(this.dir(id), { recursive: true });
    const root = { id: freshId(), name: name.trim() || id, children: [] };
    await writeJson(path.join(this.dir(id), TREE), {
      v: 1,
      name: name.trim() || id,
      version: 0,
      updatedAt: now,
      root,
    } satisfies StoredTree);
    await writeJson(path.join(this.dir(id), LOG), { v: 1, moves: [] });
    await this.syncMap(id, root);

    return { id, name: name.trim() || id, version: 0, updatedAt: now };
  }
}

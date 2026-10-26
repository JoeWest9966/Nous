/**
 * 一个目录 = 一个项目。每个项目两份 json，各管一层：
 *
 * - `tree.json`   语义：块、组、父子。**唯一真相**，丢了就是真丢。
 * - `log.json`    最近 200 手：加、删、改名、撤销。丢了只是没历史。
 * - `map/<id>/`   每个节点一个文件夹（agent.md）。**派生 + 用户内容**：文件夹跟着树长，
 *                 里面的东西是自己的，程序不碰。对账在 `map.ts`。
 *
 * 语义没有第三份。布局（谁在哪、多大）是 `squarify` 从值算出来的派生数据，
 * 丢了重算一次就有 —— 写它等于把衍生物存成事实源。等真能手动调尺寸那天再议。
 *
 * 这个文件不 import 任何浏览器代码 —— 它能被 vite 插件用，也能被真后端用。
 * 它不知道 HTTP 长什么样，HTTP 在 `api.ts` 里。
 */
import { mkdir, readdir, readFile, rename, stat, writeFile } from "node:fs/promises";
import { createHash, randomBytes } from "node:crypto";
import path from "node:path";
import { adoptNode, freshId, isNodeId, stub, syncMap } from "./map.ts";
import type { Node } from "../src/game.ts";

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
  | { status: "ok"; version: number }
  | { status: "conflict"; version: number }
  | { status: "missing" };

/** id 同时也是目录名，所以白名单同时防路径穿越和防跨平台怪字符。 */
const ID_OK = /^[a-z0-9][a-z0-9-]{0,63}$/;

const TREE = "tree.json";
const LOG = "log.json";
const LOG_LIMIT = 200;
/** 每个节点的文件夹里那份人写的东西。名字在 `map.ts` 里定过，这里同一个。 */
const MAP = "map";
const DOC = "agent.md";

export const isBoardId = (id: string): boolean => ID_OK.test(id);

export type DocResult =
  | { status: "ok"; text: string; etag: string; stub: boolean }
  | { status: "no-board" }
  | { status: "no-node" };

export type DocSaveResult =
  | { status: "ok"; etag: string }
  | { status: "conflict"; etag: string; text: string }
  | { status: "no-board" }
  | { status: "no-node" };

/** 文件还不存在时 GET 给的就是这个 etag —— 它得能被写回时认出来，不能是空串的哈希。 */
const STUB_ETAG = "stub";

/** 内容哈希，取前 16 位就够认人了 —— 比 mtime 可靠，也不依赖时钟。 */
const etagOf = (text: string): string =>
  createHash("sha1").update(text, "utf8").digest("hex").slice(0, 16);

const hasNode = (node: Node, id: string): boolean =>
  node.id === id || (node.children?.some((c) => hasNode(c, id)) ?? false);

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
   * 一块的正文：`map/<nodeId>/agent.md`。
   *
   * 三件事值得写在这儿：
   * - **树里没有这节点就不给看。** 目录可以被手动删、可以躺在 `.gone/` 里，
   *   不能让一个旧书签顺着 id 把死节点读活。
   * - **文件没了给 stub，但不落盘。** 看一眼不该长出一个文件；写是下一步的事。
   * - **etag 是内容哈希，不是 mtime。** mtime 在 git checkout / rsync 下会乱。
   */
  async doc(id: string, nodeId: string): Promise<DocResult> {
    // 双保险：api.ts 已经验过，但拼路径之前再过一道 —— 一个坏 id 能读到盘外面去。
    if (!isNodeId(nodeId)) return { status: "no-node" };
    const tree = await this.tree(id);
    if (!tree) return { status: "no-board" };
    const root = adoptNode(tree.root);
    if (!root || !hasNode(root, nodeId)) return { status: "no-node" };

    try {
      const text = await readFile(path.join(this.dir(id), MAP, nodeId, DOC), "utf8");
      return { status: "ok", text, etag: etagOf(text), stub: false };
    } catch {
      const text = stub(nodeId);
      return { status: "ok", text, etag: STUB_ETAG, stub: true };
    }
  }

  /**
   * 写一块的正文。**只看自己那份文件的 etag，不看 board 的 version** ——
   * 删一块不该让一篇文章变 409，反过来也一样。
   *
   * 冲突不是失败：把磁盘上那份一起递回去，让人当面选，而不是默默盖掉。
   */
  async saveDoc(id: string, nodeId: string, text: string, baseEtag: string): Promise<DocSaveResult> {
    if (!isNodeId(nodeId)) return { status: "no-node" };
    const tree = await this.tree(id);
    if (!tree) return { status: "no-board" };
    const root = adoptNode(tree.root);
    // 树里没有的节点不写：`.gone/` 里躺着的不能被一个旧页面写活。
    if (!root || !hasNode(root, nodeId)) return { status: "no-node" };

    const file = path.join(this.dir(id), MAP, nodeId, DOC);
    let current = "";
    let currentEtag = STUB_ETAG;
    try {
      current = await readFile(file, "utf8");
      currentEtag = etagOf(current);
    } catch {
      // 文件不在（文件夹被手动删过、或者从来就写）：stub 那把 etag 算匹配。
    }

    if (currentEtag !== baseEtag) {
      return { status: "conflict", etag: currentEtag, text: current };
    }
    if (current === text) return { status: "ok", etag: currentEtag };

    await mkdir(path.dirname(file), { recursive: true });
    await writeAtomic(file, text);
    return { status: "ok", etag: etagOf(text) };
  }

  /**
   * 写之前先比对版本。文件是唯一真相，覆盖等于真丢数据 ——
   * 两个标签页同时开着的时候，这个检查就是唯一拦住静默损失的东西。
   */
  async save(
    id: string,
    input: { root: unknown; moves: unknown[]; baseVersion: number },
  ): Promise<SaveResult> {
    const current = await this.tree(id);
    if (!current) return { status: "missing" };
    if (current.version !== input.baseVersion) {
      return { status: "conflict", version: current.version };
    }

    const next: StoredTree = {
      v: 1,
      name: current.name,
      version: current.version + 1,
      updatedAt: new Date().toISOString(),
      root: input.root,
    };
    // 顺序要紧：先落语义，再落历史。历史落后了只是少一行，语义落后了是丢盘。
    await writeJson(path.join(this.dir(id), TREE), next);
    await writeJson(path.join(this.dir(id), LOG), {
      v: 1,
      moves: input.moves.slice(-LOG_LIMIT),
    });
    await this.syncMap(id, input.root);
    return { status: "ok", version: next.version };
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

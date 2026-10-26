/**
 * 前端这一侧的文件接缝。三个动词：列、读、写。
 *
 * 换了引擎（localStorage → 文件 → 后端 → CRDT）只改这个文件，
 * 调用方一行不动 —— 这就是「一劳永逸」该落脚的地方。
 */
import type { Node } from "./game";

/** 一手棋。log.json 里存的就是这些，人能直接读，git diff 也能看。 */
export type Move =
  | { t: "addLeaf"; parent: string; id: string }
  | { t: "addSection"; parent: string; id: string }
  | { t: "remove"; id: string }
  | { t: "rename"; id: string; name: string }
  | { t: "reset" }
  | { t: "undo" };

export type BoardSummary = {
  id: string;
  name: string;
  version: number;
  updatedAt: string;
};

export type LoadedBoard = {
  name: string;
  version: number;
  root: Node;
  moves: Move[];
};

/** 磁盘上别人先写了。调用方必须停手，不能覆盖。 */
export class ConflictError extends Error {
  constructor(readonly version: number) {
    super("board changed on disk");
    this.name = "ConflictError";
  }
}

/**
 * 本地草稿：已经落下但还没发出去的一手。
 *
 * 写盘失败（服务重启、断网、合盖醒来）时，快照存在这里；页面一关，内存那份就没了，
 * 这一份还能活。下次启动先看它 —— 因为滑落的手不该靠「用户再点一下」才能救回来。
 *
 * `baseVersion` 是它当时的底：跟磁盘一样就说明只是没发出去，可以无感补写；
 * 不一样就说明磁盘被别人改过，必须问人，不能自动覆盖。
 */
export type Draft = {
  baseVersion: number;
  root: Node;
  moves: Move[];
  at: string;
};

const draftKey = (id: string): string => `n-draft:${id}`;

const isDraft = (v: unknown): v is Draft => {
  const d = v as Draft | null;
  return (
    typeof d === "object" &&
    d !== null &&
    typeof d.baseVersion === "number" &&
    typeof d.at === "string" &&
    Array.isArray(d.moves) &&
    typeof d.root === "object" &&
    d.root !== null &&
    typeof (d.root as Node).id === "string"
  );
};

export function readDraft(id: string): Draft | null {
  try {
    const raw = localStorage.getItem(draftKey(id));
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    return isDraft(parsed) ? parsed : null;
  } catch {
    // 存了半截 / 私隐模式禁写：当没草稿，不阻着页面起来。
    return null;
  }
}

export function writeDraft(id: string, root: Node, moves: Move[], baseVersion: number): void {
  try {
    const draft: Draft = { baseVersion, root, moves, at: new Date().toISOString() };
    localStorage.setItem(draftKey(id), JSON.stringify(draft));
  } catch {
    // 配额满了就算了：内存里的 pending 还在，本条路只是兼保险。
  }
}

export function clearDraft(id: string): void {
  try {
    localStorage.removeItem(draftKey(id));
  } catch {
    // 同上。
  }
}

const url = (id?: string): string => (id ? `/api/boards/${encodeURIComponent(id)}` : "/api/boards");

async function asJson<T>(res: Response): Promise<T> {
  if (res.ok) return (await res.json()) as T;
  if (res.status === 409) {
    const body = (await res.json().catch(() => ({}))) as { version?: number };
    throw new ConflictError(typeof body.version === "number" ? body.version : -1);
  }
  throw new Error(`${res.status} ${res.statusText}`);
}

export async function listBoards(): Promise<BoardSummary[]> {
  return asJson<BoardSummary[]>(await fetch(url(), { headers: { accept: "application/json" } }));
}

export async function loadBoard(id: string): Promise<LoadedBoard> {
  return asJson<LoadedBoard>(await fetch(url(id), { headers: { accept: "application/json" } }));
}

export async function createBoard(name: string): Promise<BoardSummary> {
  return asJson<BoardSummary>(
    await fetch(url(), {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name }),
    }),
  );
}

/** keepalive 版给关页面那一刀用：普通 fetch 会被浏览器连页面一起掐掉。 */
export async function saveBoard(
  id: string,
  root: Node,
  moves: Move[],
  baseVersion: number,
  keepalive = false,
): Promise<{ version: number }> {
  return asJson<{ version: number }>(
    await fetch(url(id), {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ root, moves, baseVersion }),
      keepalive,
    }),
  );
}

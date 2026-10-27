/**
 * 前端这一侧的文件接缝。三个动词：列、读、写。
 *
 * 换了引擎（localStorage → 文件 → 后端 → CRDT）只改这个文件，
 * 调用方一行不动 —— 这就是「一劳永逸」该落脚的地方。
 */
import { freshId, type Node } from "./game";

/** 一手棋。log.json 里存的就是这些，人能直接读，git diff 也能看。 */
export type Move =
  | { t: "addLeaf"; parent: string; id: string }
  | { t: "addSection"; parent: string; id: string }
  | { t: "remove"; id: string }
  | { t: "rename"; id: string; name: string }
  // 只记事实，不记结果：撤销靠 App 的 past 栈（快照），不靠重放，所以没有反向操作。
  | { t: "move"; id: string; to: string; before?: string }
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
  /** content/<board>/ 的绝对路径，服务器给的。块菜单的「终端路径」拼它。 */
  dir?: string;
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

const draftKey = (id: string): string => `nous-draft:${id}`;

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
    // 老草稿的根上还叫 id —— 认它，adoptTrees 会转过来。
    (typeof (d.root as Node).id === "string" || typeof (d.root as LegacyNode).tree === "string")
  );
};

export function readDraft(id: string): Draft | null {
  try {
    const raw = localStorage.getItem(draftKey(id));
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    // 草稿可能是改字段名之前存的 —— 同样接住一次。
    return isDraft(parsed) ? { ...parsed, root: adoptIds(parsed.root as LegacyNode) } : null;
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

/**
 * 这个字段搬过两次家：最早叫 `id`，中间改叫 `tree`，现在又叫回 `id`（值没变过 ——
 * 永远是新节点出生时发的那个随机六位）。载入时两个名字都认，写回去就是新格式。
 */
type LegacyNode = Omit<Node, "id" | "children"> & { id?: string; tree?: string; children?: LegacyNode[] };

const adoptIds = (n: LegacyNode): Node => {
  // 两个都没有（手改坏了）：发一个，别让空串顺着 findNode 到处走。
  const out: Node = { id: n.id ?? n.tree ?? freshId(), name: n.name };
  if (n.children) out.children = n.children.map(adoptIds);
  else out.value = n.value ?? 0;
  return out;
};

export async function listBoards(): Promise<BoardSummary[]> {
  return asJson<BoardSummary[]>(await fetch(url(), { headers: { accept: "application/json" } }));
}

export async function loadBoard(id: string): Promise<LoadedBoard> {
  const board = await asJson<LoadedBoard>(await fetch(url(id), { headers: { accept: "application/json" } }));
  return { ...board, root: adoptIds(board.root as LegacyNode) };
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
): Promise<{ version: number; restored: string[]; root?: Node }> {
  return asJson<{ version: number; restored: string[]; root?: Node }>(
    await fetch(url(id), {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ root, moves, baseVersion }),
      keepalive,
    }),
  );
}

/* ------------------------------------------------------------------ *
 * 文档：`map/<nodeId>/` 里那些 `.md`
 *
 * 文件夹就是清单。它和树是**两份互不相干的事实**：写一篇文章不该动 version，
 * 落一手棋也不该让文档变成 409。所以每份文档有自己的 etag（内容哈希），
 * 不走 board 的 baseVersion。
 *
 * 名字是身份，id 不是：`AGENTS.md` 永远在、永远第一、只能改不能删；
 * 其余的名字就是文件名，能建、能改、能删（进 `.trash/`）。
 * ------------------------------------------------------------------ */

export type DocFile = {
  name: string;
  size: number;
  at: string;
  /** `AGENTS.md` —— 入口，禁止删除。界面上那个删除入口根本不生成。 */
  protected: boolean;
  trashed: boolean;
  /** 搜索命中时那一行的上下文。 */
  snippet?: string;
};

export type DocList = {
  files: DocFile[];
  /** true = 这个文件夹里有 `scripts.start`。它的 URL 根归它自己，不归文档页。 */
  hasApp: boolean;
};

export type Doc = {
  text: string;
  /** 内容哈希。写回时带上它，磁盘被改过就是 409。 */
  etag: string;
  /** true = 磁盘上还没有这个文件，text 是程序给的 stub。 */
  stub: boolean;
  hasApp: boolean;
};

const docUrl = (board: string, node: string, name?: string): string =>
  `${url(board)}/nodes/${encodeURIComponent(node)}/doc${name ? `/${encodeURIComponent(name)}` : ""}`;

/**
 * 树里没有这节点、或者没这一盘，都返回 `null` ——
 * 调用方要画的是「查无此块」，不是一个错误页。
 * 文档被删了（404 + missing）也是 `null`：那是「这一份不在了」，不是错误。
 */
export async function loadDoc(board: string, node: string, name?: string): Promise<Doc | null> {
  const res = await fetch(docUrl(board, node, name), { headers: { accept: "application/json" } });
  if (res.status === 404) return null;
  return asJson<Doc>(res);
}

/** 这一块的文档清单。`q` 非空时按名字 + 正文筛（筛在服务端做，正文不上网）. */
export async function listDocs(board: string, node: string, q = ""): Promise<DocList | null> {
  const tail = q ? `?q=${encodeURIComponent(q)}` : "";
  const res = await fetch(`${url(board)}/nodes/${encodeURIComponent(node)}/docs${tail}`);
  if (res.status === 404) return null;
  return asJson<DocList>(res);
}

/** 磁盘上那份被别处改过了。带回来的是磁盘的全文 —— 让人当面选，别默默盖掉。 */
export class DocConflictError extends Error {
  constructor(
    readonly etag: string,
    readonly text: string,
  ) {
    super("doc changed on disk");
    this.name = "DocConflictError";
  }
}

/**
 * `keepalive` 给关页面那一刀用：普通 fetch 会被浏览器连页面一起掐掉。
 * 注意它有 64KB 上限 —— 只在真的要离开时用，普通的写走普通 fetch。
 */
export async function saveDoc(
  board: string,
  node: string,
  name: string,
  text: string,
  etag: string,
  keepalive = false,
): Promise<{ etag: string }> {
  const res = await fetch(docUrl(board, node, name), {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ text, etag }),
    keepalive,
  });
  // 409 得先自己接：`asJson` 那个 ConflictError 是 board 的，形状不一样。
  if (res.status === 409) {
    const body = (await res.json().catch(() => ({}))) as { etag?: string; text?: string };
    throw new DocConflictError(body.etag ?? "", body.text ?? "");
  }
  return asJson<{ etag: string }>(res);
}

/** 服务端说得出口的那几句。UI 直接拿去显示，不再翻译一遍。 */
export class DocEditError extends Error {
  constructor(
    readonly status: number,
    readonly reason: "bad-name" | "exists" | "protected" | "missing" | "other",
  ) {
    super(reason);
    this.name = "DocEditError";
  }
}

const editCall = async (path: string, init: RequestInit): Promise<{ name: string }> => {
  const res = await fetch(path, init);
  if (res.ok) return (await res.json()) as { name: string };
  const reason =
    res.status === 400
      ? "bad-name"
      : res.status === 409
        ? "exists"
        : res.status === 403
          ? "protected"
          : res.status === 404
            ? "missing"
            : "other";
  throw new DocEditError(res.status, reason);
};

/** 新建一份。名字 sanitize 在服务端也做，这里先做一遍只为「不用等一次往返」才知道重名。 */
export async function createDoc(board: string, node: string, name: string): Promise<{ name: string }> {
  return editCall(`${url(board)}/nodes/${encodeURIComponent(node)}/docs`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name }),
  });
}

/** 删一份 —— 实际是挪进 `.trash/`。`AGENTS.md` 会拿到 403。 */
export async function trashDoc(board: string, node: string, name: string): Promise<{ name: string }> {
  return editCall(docUrl(board, node, name), { method: "DELETE" });
}

/** 从 `.trash/` 搬回来，名字不改。 */
export async function restoreDoc(board: string, node: string, name: string): Promise<{ name: string }> {
  return editCall(`${docUrl(board, node, name)}/restore`, { method: "POST" });
}

/* —— 正文的本地草稿。和 board 的 Draft 一个道理：滑落的那几个字不该靠重敲才回来。 —— */

export type DocDraft = {
  text: string;
  /** 这份草稿是基于磁盘哪个版本写的。跟当前 etag 一样 = 只是没发出去，可以无感补写。 */
  etag: string;
  at: string;
};

/** 草稿按「盘 + 块 + 文件名」分。一份文档一个字都不能串到另一份上。 */
const docDraftKey = (board: string, node: string, name: string): string =>
  `nous-doc-draft:${board}:${node}:${name}`;

export function readDocDraft(board: string, node: string, name: string): DocDraft | null {
  try {
    const raw = localStorage.getItem(docDraftKey(board, node, name));
    const parsed: unknown = raw ? JSON.parse(raw) : null;
    if (
      parsed &&
      typeof parsed === "object" &&
      typeof (parsed as DocDraft).text === "string" &&
      typeof (parsed as DocDraft).etag === "string"
    ) {
      return parsed as DocDraft;
    }
    return null;
  } catch {
    return null;
  }
}

export function writeDocDraft(
  board: string,
  node: string,
  name: string,
  text: string,
  etag: string,
): void {
  try {
    const draft: DocDraft = { text, etag, at: new Date().toISOString() };
    localStorage.setItem(docDraftKey(board, node, name), JSON.stringify(draft));
  } catch {
    // 存不下就算了：字还在 textarea 里，下一次落键还会再试。
  }
}

export function clearDocDraft(board: string, node: string, name: string): void {
  try {
    localStorage.removeItem(docDraftKey(board, node, name));
  } catch {
    // 同上。
  }
}

/** 上次在看哪一份。打开文档页时从那篇接着看，不然每次都落回 AGENTS.md。 */
const lastDocKey = (board: string, node: string): string => `nous-doc-last:${board}:${node}`;

export function readLastDoc(board: string, node: string): string | null {
  try {
    return localStorage.getItem(lastDocKey(board, node));
  } catch {
    return null;
  }
}

export function writeLastDoc(board: string, node: string, name: string): void {
  try {
    localStorage.setItem(lastDocKey(board, node), name);
  } catch {
    // 记不住就记不住，回 AGENTS.md。
  }
}

/* ------------------------------------------------------------------ *
 * 锁：`content/<board>/.nous/locks.json`
 *
 * 锁是宿主的状态，不是块的内容 —— 所以它不住在块的文件夹里，也不进 `tree.json`。
 * 前端拿它只干一件事：锁定的块不给渲染删除按钮。真正的拦截在服务端（`save()` 会把
 * 被删的锁定块接回来），这里少画一颗按钮只是防手滑。
 * ------------------------------------------------------------------ */

export async function loadLocks(board: string): Promise<string[]> {
  const res = await fetch(`${url(board)}/locks`);
  if (!res.ok) return [];
  const body = (await res.json()) as { ids?: unknown };
  return Array.isArray(body.ids) ? body.ids.filter((x): x is string => typeof x === "string") : [];
}

export async function setLock(board: string, node: string, locked: boolean): Promise<string[]> {
  const res = await fetch(`${url(board)}/locks`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ id: node, locked }),
  });
  const body = await asJson<{ ids?: unknown }>(res);
  return Array.isArray(body.ids) ? body.ids.filter((x): x is string => typeof x === "string") : [];
}

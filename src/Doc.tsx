import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { type Node, findNode, isSection, pathTo, treeMap } from "./game";
import {
  DocConflictError,
  clearDocDraft,
  loadDoc,
  readDocDraft,
  saveDoc,
  writeDocDraft,
} from "./store";

/**
 * 一块的页面。**真实路径**：`/p/<board>/<nodeId>/` —— 冷启动能打开，粘到别处也能打开。
 *
 * 这里现在是一个编辑器：`map/<nodeId>/agent.md` 的原文，改了就自己叹口气存下去。
 * 三件事是刻意的：
 *
 * - **它盖在盘面上面，不卸载盘面。** 卸载会丢掉树的展开状态、滚动位置和撤销栈。
 * - **它有自己的版本号（etag），不碰 board 的 version。** 写一篇文章不该顶出 409。
 * - **「建文件夹时写一次，之后永不覆盖」这条承诺由这里兑现。** 只有真的敲了字才写盘：
 *   不自动加 frontmatter、不自动重排、不自动 trim。
 */
const SAVE_DELAY = 400;

type Phase = "loading" | "ready" | "missing" | "error";
type Status = "idle" | "saving" | "saved" | "conflict" | "error";

const STATUS_TEXT: Record<Status, string> = {
  idle: "",
  saving: "保存中",
  saved: "已保存",
  conflict: "冲突",
  error: "没连上",
};

export function DocPage({
  board,
  id,
  root,
  ready,
  onClose,
  onOpenGroup,
}: {
  board: string;
  id: string;
  root: Node;
  /** 树载完了没有。没载完就说「载入中」，别急着喊「查无此块」。 */
  ready: boolean;
  onClose: () => void;
  /** 回到它所在的组 —— 面包屑上那一段。 */
  onOpenGroup: (ids: string[]) => void;
}) {
  const node = findNode(root, id);
  const tree = useMemo(() => treeMap(root).get(id) ?? node?.name ?? id, [root, id, node]);

  const [phase, setPhase] = useState<Phase>("loading");
  const [text, setText] = useState("");
  const [status, setStatus] = useState<Status>("idle");
  /** 磁盘上那份（409 带回来的）。两边的字都还在，等人选。 */
  const [theirs, setTheirs] = useState<{ text: string; etag: string } | null>(null);
  /** 本地草稿，但它基于的是旧版本 —— 不能无感补写。 */
  const [orphan, setOrphan] = useState<{ text: string; etag: string } | null>(null);
  const [stubFile, setStubFile] = useState(false);
  const [msg, setMsg] = useState("");

  /** 还没发出去的那一版。`null` = 干净。 */
  const pending = useRef<string | null>(null);
  /** 写盘串行：两个 PUT 同时在飞，旧的后到就会把新敲的字盖掉。 */
  const writing = useRef(false);
  /** 我这一版建立在磁盘哪个 etag 上。 */
  const baseEtag = useRef("");
  /** 磁盘上确认过的最后一份。和它相同就不重复写。 */
  const lastSaved = useRef("");
  /** 撞了版本就停手，等人选 —— 继续自动写等于帮另一个标签页删字。 */
  const blocked = useRef(false);
  /** 最新的一份文本，给 flush 用（effect 闭包里拿不到 state）。 */
  const latest = useRef("");
  const timer = useRef(0);

  const drain = useCallback(async () => {
    if (writing.current || blocked.current) return;
    writing.current = true;
    try {
      while (pending.current !== null && !blocked.current) {
        const body = pending.current;
        pending.current = null;
        if (body === lastSaved.current) {
          clearDocDraft(board, id);
          setStatus("saved");
          continue;
        }
        try {
          const { etag } = await saveDoc(board, id, body, baseEtag.current);
          baseEtag.current = etag;
          lastSaved.current = body;
          clearDocDraft(board, id);
          setStubFile(false);
          setStatus("saved");
        } catch (err) {
          if (err instanceof DocConflictError) {
            blocked.current = true;
            pending.current = null;
            setTheirs({ text: err.text, etag: err.etag });
            setStatus("conflict");
          } else {
            // 网络抖一下：草稿已经落在本地了，下一次落键或离开再试。
            pending.current = body;
            setStatus("error");
            break;
          }
        }
      }
    } finally {
      writing.current = false;
    }
  }, [board, id]);

  /** 离开前把最后那几个字推出去。keepalive 只给真的要走的场景（它有 64KB 上限）。 */
  const flush = useCallback(
    (keepalive: boolean) => {
      const body = pending.current;
      if (body === null || body === lastSaved.current || blocked.current) return;
      writeDocDraft(board, id, body, baseEtag.current);
      void saveDoc(board, id, body, baseEtag.current, keepalive)
        .then(({ etag }) => {
          baseEtag.current = etag;
          lastSaved.current = body;
          clearDocDraft(board, id);
        })
        .catch(() => {
          // 失败就留在草稿里，下次进来先看它。
        });
    },
    [board, id],
  );

  /* —— 载入：磁盘那份 + 可能躺着的草稿 —— */
  useEffect(() => {
    if (!ready) return;
    if (!node) {
      setPhase("missing");
      return;
    }
    let alive = true;
    setPhase("loading");
    setStatus("idle");
    setTheirs(null);
    setOrphan(null);
    pending.current = null;
    blocked.current = false;
    void (async () => {
      try {
        const doc = await loadDoc(board, id);
        if (!alive) return;
        if (!doc) {
          setPhase("missing");
          return;
        }
        baseEtag.current = doc.etag;
        lastSaved.current = doc.text;
        latest.current = doc.text;
        setStubFile(doc.stub);
        setText(doc.text);
        setPhase("ready");
        setStatus("saved");

        const draft = readDocDraft(board, id);
        if (draft && draft.text !== doc.text) {
          if (draft.etag === doc.etag) {
            // 只是没发出去：无感补写，不麻烦用户。
            latest.current = draft.text;
            setText(draft.text);
            pending.current = draft.text;
            setStatus("saving");
            void drain();
          } else {
            // 底不一样：磁盘被别处改过。两边都留着，等人选。
            setOrphan({ text: draft.text, etag: draft.etag });
          }
        } else if (draft) {
          clearDocDraft(board, id);
        }
      } catch (e) {
        if (!alive) return;
        setMsg(e instanceof Error ? e.message : "读不到");
        setPhase("error");
      }
    })();
    return () => {
      alive = false;
    };
  }, [board, id, ready, node, drain]);

  /* —— 关页面 / 切后台 / 关掉这一页：最后那几个字不能丢 —— */
  useEffect(() => {
    const hide = () => {
      if (document.hidden) flush(true);
    };
    const leave = () => flush(true);
    window.addEventListener("pagehide", leave);
    document.addEventListener("visibilitychange", hide);
    return () => {
      window.removeEventListener("pagehide", leave);
      document.removeEventListener("visibilitychange", hide);
      window.clearTimeout(timer.current);
      // 从页面上离开（Esc / 面包屑 / 后退）不是页面卸载，普通 fetch 就够了。
      flush(false);
    };
  }, [flush]);

  /* —— Esc 走人，⌘S 立刻写。焦点在 textarea 里也得管用。 —— */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
        return;
      }
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        pending.current = latest.current;
        void drain();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose, drain]);

  const edit = (value: string) => {
    latest.current = value;
    setText(value);
    pending.current = value;
    setStatus("saving");
    writeDocDraft(board, id, value, baseEtag.current);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => void drain(), SAVE_DELAY);
  };

  /** 用磁盘的：我的这几笔不要了。 */
  const takeTheirs = () => {
    if (!theirs) return;
    blocked.current = false;
    pending.current = null;
    baseEtag.current = theirs.etag;
    lastSaved.current = theirs.text;
    latest.current = theirs.text;
    setText(theirs.text);
    setTheirs(null);
    clearDocDraft(board, id);
    setStatus("saved");
  };

  /** 用我的：认下磁盘的新版本，然后把我的字盖上去。 */
  const keepMine = () => {
    if (!theirs) return;
    blocked.current = false;
    baseEtag.current = theirs.etag;
    pending.current = latest.current;
    setTheirs(null);
    setStatus("saving");
    void drain();
  };

  const useOrphan = () => {
    if (!orphan) return;
    pending.current = orphan.text;
    latest.current = orphan.text;
    setText(orphan.text);
    setOrphan(null);
    setStatus("saving");
    void drain();
  };

  const dropOrphan = () => {
    clearDocDraft(board, id);
    setOrphan(null);
  };

  const crumbs = node ? tree.split("/") : [id];
  const rows = Math.max(14, text.split("\n").length + 2);

  return (
    <div
      role="dialog"
      aria-label={`${node?.name ?? id} 的页面`}
      className="absolute inset-0 z-40 flex flex-col bg-canvas text-text"
    >
      <header className="flex h-[52px] shrink-0 items-center gap-2 border-b border-border px-3">
        <button
          type="button"
          onClick={onClose}
          aria-label="返回盘面"
          title="返回盘面（Esc）"
          className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg text-muted transition-colors duration-150 hover:bg-surface hover:text-text"
        >
          <svg viewBox="0 0 16 16" className="h-4 w-4" aria-hidden="true">
            <path
              d="M9.5 3.5 5 8l4.5 4.5"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.4"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </button>

        <nav className="flex min-w-0 items-center gap-0.5 text-[13px]" aria-label="这个块在树里的位置">
          {crumbs.map((name, i) => {
            const last = i === crumbs.length - 1;
            return (
              <span key={`${name}-${i}`} className="flex min-w-0 items-center gap-0.5">
                {i > 0 && (
                  <span className="text-text/25" aria-hidden="true">
                    ›
                  </span>
                )}
                {last ? (
                  <span className="truncate text-text">{name}</span>
                ) : (
                  <button
                    type="button"
                    onClick={() => onOpenGroup(pathTo(root, id))}
                    className="truncate rounded-md px-1.5 py-1 text-muted transition-colors hover:text-text"
                  >
                    {name}
                  </button>
                )}
              </span>
            );
          })}
        </nav>

        <div className="ml-auto flex items-center gap-2">
          {node && (
            <span className="rounded-[3px] bg-surface px-1.5 py-0.5 text-[10.5px] text-muted">
              {isSection(node) ? "组" : "块"}
            </span>
          )}
          <span
            className="font-mono text-[10.5px] text-muted/70"
            title="这一块的 id 同时是它在 map/ 里的文件夹名"
          >
            {id}
          </span>
        </div>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[720px] px-6 py-6">
          <h1 className="mb-3 truncate text-[19px] font-semibold" style={{ letterSpacing: "-0.01em" }}>
            {node?.name ?? id}
          </h1>

          {phase === "loading" && <p className="text-[13px] text-muted">载入中…</p>}

          {phase === "missing" && (
            <p className="text-[13px] text-muted">
              树里没有这个节点。可能已经被删了 —— 它的文件夹还躺在 map/.gone/ 里，撤销回来就接得上。
            </p>
          )}

          {phase === "error" && (
            <p className="text-[13px] text-muted">
              读不到这份文件（<span className="text-danger">{msg}</span>）。
            </p>
          )}

          {phase === "ready" && (
            <>
              {theirs && (
                <div className="mb-3 flex flex-wrap items-center gap-2 rounded-lg border border-border bg-surface px-3 py-2 text-[12px] shadow-lg">
                  <span className="min-w-0 flex-1 text-muted">
                    磁盘上这份被别处改过了。两边都还在，你选。
                  </span>
                  <Banner onClick={takeTheirs}>用磁盘的</Banner>
                  <Banner onClick={keepMine} muted>
                    用我的覆盖
                  </Banner>
                </div>
              )}

              {orphan && (
                <div className="mb-3 flex flex-wrap items-center gap-2 rounded-lg border border-border bg-surface px-3 py-2 text-[12px] shadow-lg">
                  <span className="min-w-0 flex-1 text-muted">
                    本地有一份没发出去的草稿，但它基于的是旧版本。两边都还在，你选。
                  </span>
                  <Banner onClick={useOrphan}>用草稿的</Banner>
                  <Banner onClick={dropOrphan} muted>
                    丢掉草稿
                  </Banner>
                </div>
              )}

              {status === "error" && (
                <p className="mb-3 text-[11.5px] text-danger">
                  没连上。字已经存成本地草稿了，下次进来先看它。
                </p>
              )}

              {stubFile && (
                <p className="mb-3 text-[11.5px] text-muted/70">
                  磁盘上还没有这个文件 —— 开始打字才会建它。
                </p>
              )}

              <textarea
                value={text}
                onChange={(e) => edit(e.target.value)}
                rows={rows}
                autoFocus
                spellCheck={false}
                aria-label="正文"
                placeholder="写点什么。改了就自己存。"
                className="w-full resize-none rounded-[4px] bg-transparent font-mono text-[12.5px] leading-[1.7] text-text/90 outline-none placeholder:text-muted/50 focus-visible:ring-1 focus-visible:ring-border"
              />
            </>
          )}

          <div className="mt-6 flex items-center gap-3 border-t border-border pt-3">
            <span className="min-w-0 flex-1 truncate font-mono text-[10.5px] text-muted/60">
              content/{board}/map/{id}/agent.md
            </span>
            <span className="shrink-0 text-[10.5px] tabular-nums text-muted/60">
              {text.length} 字
            </span>
            {STATUS_TEXT[status] && (
              <span
                className={`shrink-0 text-[10.5px] ${status === "conflict" || status === "error" ? "text-danger" : "text-muted/60"}`}
              >
                {STATUS_TEXT[status]}
              </span>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function Banner({
  children,
  onClick,
  muted,
}: {
  children: React.ReactNode;
  onClick: () => void;
  muted?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`h-7 shrink-0 rounded-lg px-2.5 text-[12px] transition-colors duration-150 ${
        muted ? "text-muted hover:bg-raised hover:text-text" : "bg-surface text-text/90 hover:bg-raised"
      }`}
    >
      {children}
    </button>
  );
}

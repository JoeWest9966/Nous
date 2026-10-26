import { useEffect, useMemo, useState } from "react";
import { type Node, findNode, isSection, pathTo, treeMap } from "./game";
import { type Doc, loadDoc } from "./store";

/**
 * 一块的页面。**真实路径**：`/p/<board>/<nodeId>/` —— 冷启动能打开，粘到别处也能打开。
 *
 * 这一层现在只干一件事：把 `map/<nodeId>/agent.md` 的原文摆出来。写是下一步的事，
 * 所以这里是 `pre` 不是编辑器 —— 先让「点进去是一页」成立，再谈编辑。
 *
 * 它盖在盘面上面，**不卸载盘面**：卸载会丢掉树的展开状态、滚动位置和撤销栈。
 */
type Load =
  | { k: "loading" }
  | { k: "ready"; doc: Doc }
  | { k: "missing" } // 树里没有这节点（被删了、或者在 .gone/ 里）
  | { k: "error"; msg: string };

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
  const [load, setLoad] = useState<Load>({ k: "loading" });

  useEffect(() => {
    if (!ready) return;
    if (!node) {
      setLoad({ k: "missing" });
      return;
    }
    let alive = true;
    setLoad({ k: "loading" });
    void (async () => {
      try {
        const doc = await loadDoc(board, id);
        if (!alive) return;
        setLoad(doc ? { k: "ready", doc } : { k: "missing" });
      } catch (e) {
        if (!alive) return;
        setLoad({ k: "error", msg: e instanceof Error ? e.message : "读不到" });
      }
    })();
    return () => {
      alive = false;
    };
  }, [board, id, ready, node]);

  // Esc 走人。挂在 window 上是因为这一层刚挂出来时焦点还在盘面那边。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  const crumbs = node ? tree.split("/") : [id];

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
        <div className="mx-auto w-full max-w-[720px] px-6 py-7">
          <div className="mb-4 flex items-baseline gap-2">
            <h1 className="truncate text-[19px] font-semibold" style={{ letterSpacing: "-0.01em" }}>
              {node?.name ?? id}
            </h1>
          </div>

          <Body load={load} />

          <p className="mt-6 border-t border-border pt-3 font-mono text-[10.5px] text-muted/60">
            content/{board}/map/{id}/agent.md
          </p>
        </div>
      </div>
    </div>
  );
}

function Body({ load }: { load: Load }) {
  if (load.k === "loading") {
    return <p className="text-[13px] text-muted">载入中…</p>;
  }
  if (load.k === "missing") {
    return (
      <p className="text-[13px] text-muted">
        {`树里没有这个节点。可能已经被删了 —— 它的文件夹还躺在 map/.gone/ 里，撤销回来就接得上。`}
      </p>
    );
  }
  if (load.k === "error") {
    return (
      <p className="text-[13px] text-muted">
        读不到这份文件（<span className="text-danger">{load.msg}</span>）。
      </p>
    );
  }
  return (
    <>
      {load.doc.stub && (
        <p className="mb-3 text-[11.5px] text-muted/70">
          磁盘上还没有这个文件 —— 下面是程序给的 stub，还没落盘。
        </p>
      )}
      <pre className="whitespace-pre-wrap break-words font-mono text-[12.5px] leading-[1.7] text-text/85">
        {load.doc.text}
      </pre>
    </>
  );
}

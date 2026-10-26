import { useEffect, useMemo, useRef, useState } from "react";
import { type Node, canMove, isSection, moveNode, treeMap } from "./game";

/**
 * 「移动到…」的目标选择器。
 *
 * 一行一个落点，**一组一行**：人想的是"挪到哪个组"，不是"将来插在第几个"。
 * 落到某个东西前面这种需求，由第二颗小键在自己所在的爹底下插到它前面 —— 同一行就能办。
 *
 * 每行两件事，用同一个键表达：
 *
 *   · 点行 / 回车     → 放进这个组（最常见，所以它占整个键）
 *   · ⤒ / ⇧回车      → 排在这个东西前面
 *
 * 从哪儿来就从哪儿长出来（`transform-origin` 跟着触发它的那颗按钮走，Apple 的空间一致性），
 * 做完就缩回去。默认态就闪一下被搬的那块 —— **不拽视野**：你挪一块多半还要再挪下一块。
 */
export function MoveTo({
  source,
  root,
  anchor,
  onMove,
  onClose,
}: {
  source: string;
  root: Node;
  anchor: DOMRect | null;
  onMove: (to: string, before?: string) => void;
  onClose: () => void;
}) {
  const box = useRef<HTMLDivElement | null>(null);
  const [cursor, setCursor] = useState(0);
  const [q, setQ] = useState("");
  /** Esc 得先从来的地方缩回去，再让父层把它拆掉 —— 直接 unmount 等于半路闪断。 */
  const [leaving, setLeaving] = useState(false);
  const quit = useRef(false);

  const startQuit = () => {
    if (quit.current) return;
    quit.current = true;
    setLeaving(true);
    setTimeout(onClose, 150);
  };

  useEffect(() => {
    box.current?.focus();
  }, []);

  /** id → 爹。"排在它前面"要知道它爹是谁 —— 位置是爹的属性，不是它自己的。 */
  const parents = useMemo(() => {
    const map = new Map<string, string | undefined>();
    const walk = (n: Node, parent?: string): void => {
      map.set(n.id, parent);
      n.children?.forEach((kid) => walk(kid, n.id));
    };
    walk(root);
    return map;
  }, [root]);

  const trees = useMemo(() => treeMap(root), [root]);

  const rows = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const out: { node: Node; to: string; before?: string }[] = [];
    const walk = (n: Node): void => {
      if (n.id !== source) {
        const tree = trees.get(n.id) ?? n.name;
        if (!needle || tree.toLowerCase().includes(needle) || n.name.toLowerCase().includes(needle)) {
          out.push({
            node: n,
            to: isSection(n) ? n.id : parents.get(n.id)!,
            before: isSection(n) ? undefined : n.id,
          });
        }
      }
      n.children?.forEach(walk);
    };
    walk(root);
    // 不成立的落点压根不出现 —— 让人点了才发现什么都没发生，比不给他选还糟。
    // 「其实没动」的位子也在其中：同一个爹、同一个前一位，选它等于白点一下。
    // 自己的子树早被 canMove 划掉了 —— 那一整棵子树里随便哪一个都在自己底下。
    return out
      .filter((r) => canMove(root, source, r.to, r.before))
      // 「其实没动」的位子也划掉：爹还是那个爹、前一位还是那一位 —— 选它等于白点一下。
      // 判据直接拿 moveNode 的结果比引用，跟 play() 的早退用的是同一根尺子。
      .filter((r) => moveNode(root, source, r.to, r.before) !== root)
      .slice(0, 40);
  }, [root, trees, parents, source, q]);

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      startQuit();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setCursor((c) => Math.min(c + 1, rows.length - 1));
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      setCursor((c) => Math.max(c - 1, 0));
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      const hit = rows[cursor];
      if (hit) onMove(hit.to, hit.before);
    }
  };

  /** 贴着触发它的那颗按钮长出来：够不着就往下翻，太靠右就往左让。 */
  const place = (): { style: React.CSSProperties; origin: string } => {
    const w = Math.min(420, window.innerWidth - 24);
    if (!anchor) {
      return {
        style: { left: "50%", top: "12vh", width: w, transform: "translateX(-50%)" },
        origin: "top center",
      };
    }
    const left = Math.max(12, Math.min(anchor.left, window.innerWidth - w - 12));
    const flip = anchor.bottom + 8 + 320 > window.innerHeight;
    return {
      style: {
        left,
        top: flip ? undefined : anchor.bottom + 8,
        bottom: flip ? window.innerHeight - anchor.top + 8 : undefined,
        width: w,
      },
      origin: flip ? "bottom left" : "top left",
    };
  };

  const { style, origin } = place();

  return (
    <>
      <button
        type="button"
        aria-label="关闭移动面板"
        onClick={startQuit}
        data-leaving={leaving ? "1" : undefined}
        className="nm-move-scrim fixed inset-0 z-30 bg-black/45"
      />
      <div
        ref={box}
        tabIndex={-1}
        role="dialog"
        aria-label="移动到哪儿"
        onKeyDown={onKey}
        style={{ ...style, transformOrigin: origin }}
        data-leaving={leaving ? "1" : undefined}
        data-flip={style.bottom === undefined ? undefined : "up"}
        className="nm-move fixed z-40 flex max-h-[60vh] flex-col overflow-hidden rounded-xl border border-border bg-surface/95 shadow-[0_20px_60px_-12px_rgba(0,0,0,0.7)] outline-none backdrop-blur-xl"
      >
        <input
          value={q}
          onChange={(e) => {
            setQ(e.target.value);
            setCursor(0);
          }}
          placeholder="搜 tree 或名字：docs"
          spellCheck={false}
          autoComplete="off"
          className="h-10 shrink-0 border-b border-border bg-transparent px-3.5 font-mono text-[12.5px] text-text outline-none placeholder:text-muted/60"
        />
        <ul className="min-h-0 flex-1 overflow-y-auto p-1.5">
          {rows.length === 0 && (
            <li className="px-2.5 py-3 text-[12px] text-muted">没有能去的地方。</li>
          )}
          {rows.map(({ node, to, before }, i) => (
            <li key={node.id}>
              <div
                onMouseEnter={() => setCursor(i)}
                className={`flex items-center rounded-md transition-colors duration-100 ${
                  i === cursor ? "bg-raised" : ""
                }`}
              >
                <button
                  type="button"
                  tabIndex={-1}
                  onClick={() => onMove(to, before)}
                  className="flex min-w-0 flex-1 items-center gap-2 px-2.5 py-1.5 text-left"
                >
                  <span className="min-w-0 flex-1 truncate font-mono text-[11.5px]">
                    {trees.get(node.id) ?? node.name}
                  </span>
                  <span className="w-8 shrink-0 text-right font-mono text-[10.5px] text-muted/70">
                    {node.id}
                  </span>
                </button>
                <button
                  type="button"
                  tabIndex={-1}
                  aria-label={`排在 ${node.name} 前面`}
                  onClick={() => onMove(to, before ?? node.id)}
                  className="mr-1 flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted hover:bg-white/10 hover:text-text"
                >
                  <svg viewBox="0 0 16 16" className="h-3 w-3" aria-hidden="true">
                    <path
                      d="M4 7h8M6.5 4.5 4 7l2.5 2.5M10 4.5 12.5 7 10 9.5"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.2"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    />
                  </svg>
                </button>
              </div>
            </li>
          ))}
        </ul>
        <div className="flex shrink-0 items-center gap-2 border-t border-border px-3 py-1.5 text-[10.5px] text-muted">
          <span>↑↓ 选 · 回车放到这一行 · Esc 走人</span>
          <span className="ml-auto opacity-70">↕ 排在它前面</span>
        </div>
      </div>
    </>
  );
}

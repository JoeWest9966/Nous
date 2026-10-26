import { useEffect, useRef, useState } from "react";
import { type Node, isSection, valueOf } from "./game";

/**
 * ⌘K。一行 = 一个节点：handle 给人看，tree 给机器看。
 *
 * 它存在的理由只有一个：**盘面画不下的那些节点，这里是唯一看得到它们 handle 的地方**。
 * 回车跳过去（块就闪一下，组就钻进去），Esc 走人。不做复制 —— 复制在盘面上点 handle。
 */
export type Hit = { node: Node; handle: string };

export function Find({
  hits,
  onGo,
  onClose,
}: {
  hits: Hit[];
  onGo: (tree: string) => void;
  onClose: () => void;
}) {
  const [q, setQ] = useState("");
  const [cursor, setCursor] = useState(0);
  const input = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    input.current?.focus();
  }, []);

  const needle = q.trim().toLowerCase();
  const list = (
    needle
      ? hits.filter(
          (h) =>
            h.handle.toLowerCase().includes(needle) ||
            h.node.tree.toLowerCase().includes(needle) ||
            h.node.name.toLowerCase().includes(needle),
        )
      : hits
  ).slice(0, 40);

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setCursor((c) => Math.min(c + 1, list.length - 1));
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      setCursor((c) => Math.max(c - 1, 0));
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      const hit = list[cursor];
      if (hit) onGo(hit.node.tree);
    }
  };

  return (
    <div className="fixed inset-0 z-30 flex items-start justify-center pt-[12vh]">
      <button
        type="button"
        aria-label="关闭"
        onClick={onClose}
        className="absolute inset-0 bg-black/45"
      />
      <div
        role="dialog"
        aria-label="按 tree 或 handle 找一块"
        className="relative flex max-h-[60vh] w-[min(520px,92vw)] flex-col overflow-hidden rounded-xl border border-border bg-surface shadow-[0_20px_60px_-12px_rgba(0,0,0,0.7)]"
      >
        <input
          ref={input}
          value={q}
          onChange={(e) => {
            setQ(e.target.value);
            setCursor(0);
          }}
          onKeyDown={onKey}
          placeholder="src/App.tsx · n3 · App"
          spellCheck={false}
          autoComplete="off"
          className="h-11 shrink-0 border-b border-border bg-transparent px-3.5 font-mono text-[12.5px] text-text outline-none placeholder:text-muted/60"
        />
        <ul className="min-h-0 flex-1 overflow-y-auto p-1.5">
          {list.length === 0 && (
            <li className="px-2.5 py-3 text-[12px] text-muted">没有这一块。</li>
          )}
          {list.map((h, i) => (
            <li key={h.node.tree}>
              <button
                type="button"
                onMouseEnter={() => setCursor(i)}
                onClick={() => onGo(h.node.tree)}
                className={`flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left transition-colors duration-100 ${
                  i === cursor ? "bg-raised" : ""
                }`}
              >
                <span className={`shrink-0 text-[11px] ${isSection(h.node) ? "" : "text-muted"}`}>
                  {isSection(h.node) ? "组" : "块"}
                </span>
                <span className="min-w-0 flex-1 truncate font-mono text-[11.5px]">{h.handle}</span>
                <span className="shrink-0 font-mono text-[10.5px] tabular-nums text-muted">
                  {valueOf(h.node)}
                </span>
                <span className="w-8 shrink-0 text-right font-mono text-[10.5px] text-muted/70">
                  {h.node.tree}
                </span>
              </button>
            </li>
          ))}
        </ul>
        <div className="shrink-0 border-t border-border px-3 py-1.5 text-[10.5px] text-muted">
          ↑↓ 选 · 回车跳过去 · Esc 关
        </div>
      </div>
    </div>
  );
}

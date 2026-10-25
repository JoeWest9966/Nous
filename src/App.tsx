import { useEffect, useMemo, useRef, useState } from "react";
import { Tree } from "./Tree";
import {
  type Box,
  type Node,
  addLeaf,
  addSection,
  bump,
  findNode,
  initialState,
  isSection,
  layout,
  removeNode,
  valueOf,
} from "./game";

const EASE = "ease-[cubic-bezier(0.32,0.72,0,1)]";
const SIDE_KEY = "n-sidebar";

/**
 * pele 的 series 色板（`--pele-series-1..8`）。用它而不是自己调 HSL：
 * 这八个色是成对验算过对比度的，随机生成的花色一眼就假。
 */
const SERIES = [
  "#4c78a8",
  "#f58518",
  "#54a24b",
  "#e45756",
  "#72b7b2",
  "#eeca3b",
  "#b279a2",
  "#9d755d",
] as const;

/** pele treemap 的几何：块间 4px 缝（每边内缩 2）、子块左右各缩 6、底部缩 6、顶部让出标题带。 */
const GAP = 2;
const SIDE = 6;

const shrink = (b: Box, d: number): Box => ({
  x: b.x + d,
  y: b.y + d,
  w: Math.max(b.w - d * 2, 1),
  h: Math.max(b.h - d * 2, 1),
});

/** 标题带高度：够高才有，不然名字宁可不显示，也不要压着子块。 */
const hat = (b: Box, depth: number): number => {
  if (depth === 1) return b.h > 76 && b.w > 96 ? 24 : b.h > 40 && b.w > 60 ? 16 : 0;
  return b.h > 60 && b.w > 84 ? 18 : 0;
};

const tint = (hex: string, alpha: number): string => {
  const n = parseInt(hex.slice(1), 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`;
};

export function App() {
  const [root, setRoot] = useState<Node>(() => initialState());
  const [path, setPath] = useState<string[]>([]);
  /** 每一步落子前的一盘棋。撤销就是弹一个出来。 */
  const [past, setPast] = useState<Node[]>([]);
  const [side, setSide] = useState<boolean>(() => localStorage.getItem(SIDE_KEY) !== "0");
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [narrow, setNarrow] = useState(() => window.matchMedia("(max-width: 819px)").matches);
  const board = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const el = board.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) =>
      setSize({ w: e.contentRect.width, h: e.contentRect.height }),
    );
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    localStorage.setItem(SIDE_KEY, side ? "1" : "0");
  }, [side]);

  useEffect(() => {
    const mq = window.matchMedia("(max-width: 819px)");
    const sync = () => setNarrow(mq.matches);
    mq.addEventListener("change", sync);
    return () => mq.removeEventListener("change", sync);
  }, []);

  const here = (path.length ? findNode(root, path[path.length - 1]!) : root) ?? root;

  const play = (next: Node) => {
    setPast((p) => [...p.slice(-199), root]);
    setRoot(next);
  };

  const undo = () => {
    if (!past.length) return;
    const back = past[past.length - 1]!;
    setPast((p) => p.slice(0, -1));
    setRoot(back);
    setPath((ids) =>
      ids.filter((id) => {
        const n = findNode(back, id);
        return n !== undefined && isSection(n);
      }),
    );
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "z") {
        e.preventDefault();
        undo();
        return;
      }
      // 焦点在某片叶子上时，Delete / Backspace 删掉它 —— 悬停才出现的 × 不该是唯一入口。
      if (e.key === "Delete" || e.key === "Backspace") {
        const el = document.activeElement as HTMLElement | null;
        const id = el?.dataset.nodeId;
        if (!id) return;
        e.preventDefault();
        play(removeNode(root, id));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const placed = useMemo(() => {
    if (!size.w || !size.h) return [];

    const build = (parent: Node, area: Box, depth: number, color: string): Placed[] =>
      layout(parent, area, depth).map((kid, i) => {
        // 只有最外一层按分支分色；里面的块都继承自己那一支的颜色（pele 就是这么做的）。
        const kidColor = depth === 0 ? SERIES[i % SERIES.length]! : color;
        const box = shrink(kid.box, GAP);
        if (!kid.node.children) return { node: kid.node, box, depth: kid.depth, color: kidColor, kids: [] };
        const head = hat(box, kid.depth);
        return {
          node: kid.node,
          box,
          depth: kid.depth,
          color: kidColor,
          kids: build(
            kid.node,
            {
              x: box.x + SIDE,
              y: box.y + head,
              w: Math.max(box.w - SIDE * 2, 1),
              h: Math.max(box.h - head - SIDE, 1),
            },
            kid.depth,
            kidColor,
          ),
        };
      });

    return build(here, { x: 0, y: 0, w: size.w, h: size.h }, 0, SERIES[0]!);
  }, [here, size]);

  return (
    <main className="relative flex h-dvh w-full overflow-hidden bg-canvas text-text antialiased">
      <aside
        className={`shrink-0 overflow-hidden border-r border-white/[0.06] bg-canvas transition-[width] duration-300 ${EASE} ${
          narrow
            ? `absolute bottom-0 left-0 top-[52px] z-20 shadow-[0_0_60px_rgba(0,0,0,0.6)] ${side ? "w-[248px]" : "w-0 border-r-0"}`
            : `relative bg-white/[0.015] ${side ? "w-[248px]" : "w-0 border-r-0"}`
        }`}
        aria-hidden={!side}
      >
        <div
          className={`h-full w-[248px] overflow-y-auto ${side ? "opacity-100" : "opacity-0"} transition-opacity duration-200`}
        >
          <Tree root={root} path={path} onEnter={setPath} onRemove={(id) => play(removeNode(root, id))} />
        </div>
      </aside>

      {narrow && side && (
        <button
          type="button"
          aria-label="关闭左列浮层"
          onClick={() => setSide(false)}
          className="absolute bottom-0 left-0 top-[52px] z-10 w-full bg-black/40"
        />
      )}

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-[52px] shrink-0 items-center gap-2 border-b border-white/[0.06] px-3">
          <IconButton
            label={side ? "收起左列" : "展开左列"}
            onClick={() => setSide(!side)}
            className={side ? "text-text/80" : "text-muted"}
          >
            <svg viewBox="0 0 16 16" className="h-4 w-4" aria-hidden="true">
              <rect x="1.5" y="2.5" width="13" height="11" rx="2.5" fill="none" stroke="currentColor" strokeWidth="1.2" />
              <path d="M6 2.5v11" stroke="currentColor" strokeWidth="1.2" />
            </svg>
          </IconButton>

          <nav className="flex min-w-0 items-center gap-0.5 text-[13px]" aria-label="路径">
            <button
              type="button"
              onClick={() => setPath([])}
              className={`truncate rounded-md px-1.5 py-1 transition-colors ${path.length ? "text-muted hover:text-text" : "text-text"}`}
            >
              {root.name}
            </button>
            {path.map((id, i) => (
              <span key={id} className="flex min-w-0 items-center gap-0.5">
                <span className="text-text/25" aria-hidden="true">›</span>
                <button
                  type="button"
                  onClick={() => setPath(path.slice(0, i + 1))}
                  className={`truncate rounded-md px-1.5 py-1 transition-colors ${
                    i === path.length - 1 ? "text-text" : "text-muted hover:text-text"
                  }`}
                >
                  {findNode(root, id)?.name ?? "?"}
                </button>
              </span>
            ))}
          </nav>

          <div className="ml-auto flex items-center gap-1.5">
            <div className="mr-1 flex items-baseline gap-1.5">
              <span className="text-[11px] text-muted">总分</span>
              <span className="text-[17px] font-semibold tabular-nums" style={{ letterSpacing: "-0.02em" }}>
                {valueOf(root)}
              </span>
            </div>
            <Pill onClick={undo} disabled={!past.length} title="⌘Z / Ctrl+Z">
              撤销
            </Pill>
            <Pill onClick={() => play(addLeaf(root, here.id))}>＋ 块</Pill>
            <Pill onClick={() => play(addSection(root, here.id))}>＋ 组</Pill>
            <Pill
              onClick={() => {
                play(initialState());
                setPath([]);
              }}
              muted
            >
              重开
            </Pill>
          </div>
        </header>

        <div ref={board} className="relative min-h-0 flex-1 p-2" aria-label="盘面">
          {placed.length === 0 && (
            <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 text-[13px] text-muted">
              <span>盘上什么都没有了</span>
              <Pill onClick={() => play(addLeaf(root, root.id))}>＋ 块</Pill>
            </div>
          )}
          <div className="relative h-full w-full">
            {placed.map((p) => (
              <Piece
                key={p.node.id}
                piece={p}
                origin={{ x: 0, y: 0 }}
                isSection={isSection}
                onBump={(id) => play(bump(root, id))}
                onEnter={(id) => setPath([...path, id])}
                onRemove={(id) => play(removeNode(root, id))}
              />
            ))}
          </div>
        </div>
      </div>
    </main>
  );
}

type Placed = {
  node: Node;
  box: Box;
  depth: number;
  color: string;
  kids: Placed[];
};

/**
 * 一个格子。section 的子树渲染在它自己里面（不是平铺成兄弟节点）——
 * 这样鼠标停在里面的块上，section 的 `group-hover` 才成立，标题带上的按钮才出得来。
 */
function Piece({
  piece,
  origin,
  isSection,
  onBump,
  onEnter,
  onRemove,
}: {
  piece: Placed;
  origin: { x: number; y: number };
  isSection: (n: Node) => boolean;
  onBump: (id: string) => void;
  onEnter: (id: string) => void;
  onRemove: (id: string) => void;
}) {
  const { node, box, depth, color, kids } = piece;
  const place = { left: box.x - origin.x, top: box.y - origin.y, width: box.w, height: box.h };
  const section = isSection(node);
  const value = valueOf(node);

  if (section) {
    const strip = hat(box, depth);
    return (
      <div
        className={`group/sec absolute overflow-hidden rounded-[4px] transition-[left,top,width,height] duration-300 ${EASE}`}
        style={{
          ...place,
          background: tint(color, 0.12),
          // pele 的描边用的是 series 原色；深色底上降到 55% 才不刺眼。
          boxShadow: `inset 0 0 0 1px ${tint(color, 0.55)}`,
        }}
      >
        {strip > 0 && (
          <div
            className="absolute inset-x-0 top-0 flex items-center gap-1.5 px-2 text-[11px]"
            style={{ height: strip, color: tint(color, 0.92) }}
          >
            <span className="truncate" style={{ letterSpacing: "0.01em" }}>
              {node.name}
            </span>
            {strip >= 24 && <span className="shrink-0 opacity-45">{node.children!.length} 块</span>}
            <button
              type="button"
              onClick={() => onEnter(node.id)}
              title={`进入 ${node.name}`}
              className="ml-auto shrink-0 rounded-[3px] px-1.5 py-0.5 text-[11px] opacity-0 transition-opacity duration-150 hover:bg-white/10 group-hover/sec:opacity-100"
            >
              进入 ›
            </button>
            <button
              type="button"
              onClick={() => onRemove(node.id)}
              aria-label={`删除 ${node.name}`}
              title={`删除 ${node.name}（可撤销）`}
              className="shrink-0 rounded-[3px] px-1.5 py-0.5 text-[11px] leading-none opacity-0 transition-opacity duration-150 hover:bg-white/15 group-hover/sec:opacity-100"
            >
              ×
            </button>
          </div>
        )}
        {kids.map((k) => (
          <Piece
            key={k.node.id}
            piece={k}
            origin={box}
            isSection={isSection}
            onBump={onBump}
            onEnter={onEnter}
            onRemove={onRemove}
          />
        ))}
      </div>
    );
  }

  const big = Math.min(box.w, box.h);
  const showName = box.w > 56 && box.h > 30;
  const showValue = big > 24;
  const valueSize = Math.max(12, Math.min(40, Math.round(big / 3)));
  const roomForDelete = box.w > 34 && box.h > 26;

  return (
    <div
      className={`group/leaf absolute overflow-hidden rounded-[4px] transition-[left,top,width,height] duration-300 ${EASE}`}
      style={{ ...place, background: color, color: "#0a0c10" }}
    >
      <button
        type="button"
        onClick={() => onBump(node.id)}
        aria-label={`${node.name} ${value}`}
        data-node-id={node.id}
        className="flex h-full w-full flex-col items-center justify-center gap-1 transition-[filter] duration-150 hover:brightness-110 active:brightness-125"
      >
        {showName && (
          <span className="pointer-events-none max-w-full truncate px-1.5 text-[10px] leading-none opacity-65">
            {node.name}
          </span>
        )}
        {showValue && (
          <span
            className="pointer-events-none font-semibold tabular-nums leading-none"
            style={{ fontSize: valueSize, letterSpacing: "-0.02em" }}
          >
            {value}
          </span>
        )}
      </button>
      {roomForDelete && (
        <button
          type="button"
          onClick={() => onRemove(node.id)}
          aria-label={`删除 ${node.name}`}
          title={`删除 ${node.name}（可撤销）`}
          className="absolute right-1 top-1 flex h-4 w-4 items-center justify-center rounded-[3px] bg-black/25 text-[12px] leading-none text-white/85 opacity-0 transition-opacity duration-150 hover:bg-black/45 hover:text-white focus-visible:opacity-100 group-hover/leaf:opacity-100"
        >
          ×
        </button>
      )}
    </div>
  );
}

function Pill({
  children,
  onClick,
  disabled,
  muted,
  title,
}: {
  children: React.ReactNode;
  onClick: () => void;
  disabled?: boolean;
  muted?: boolean;
  title?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      title={title}
      className={`h-7 rounded-lg px-2.5 text-[12px] transition-colors duration-150 disabled:opacity-25 ${
        muted
          ? "text-muted hover:bg-white/[0.06] hover:text-text"
          : "bg-white/[0.07] text-text/90 hover:bg-white/[0.11]"
      }`}
    >
      {children}
    </button>
  );
}

function IconButton({
  children,
  onClick,
  label,
  className = "",
}: {
  children: React.ReactNode;
  onClick: () => void;
  label: string;
  className?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      title={label}
      className={`flex h-7 w-7 shrink-0 items-center justify-center rounded-lg transition-colors duration-150 hover:bg-white/[0.07] hover:text-text ${className}`}
    >
      {children}
    </button>
  );
}

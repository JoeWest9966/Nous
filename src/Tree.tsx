import { useEffect, useState } from "react";
import { NameInput } from "./Rename";
import { NodeMenuTrigger } from "./NodeMenu";
import { type Node, findNode, isSection, valueOf } from "./game";
import { TreeIcon, iconFor } from "./icons";

/**
 * 左列：pele 的 treeView-beta，原样搬到这盘棋上。
 *
 * 它是文件树（`src/`、`App.tsx`），不是开合面板 —— 所以：
 * - 连接线是 **弯角肘线**（pele 的 elbow corner），不是每行一根孤零零的竖线
 * - 文件夹名字带尾斜杠（`src/`），文件是完整文件名（`App.tsx`）
 * - 图标按 playground 那份 frontmatter 配（`.ts`→TS、`.tsx`→react、`.txt`→无）
 * - 分数在右缘；组能钻进去（点名字），转折点也能开合子树 —— 满行都是把手
 */

const INDENT = 16; // 每层缩进 = 肘线格宽
const STEM = 14; // 每层把手（chevron）中心在格内的 x：4 + 20/2
const STUB = 6; // 转折点拐进图标的横线长
const H = 32; // 一行的高（h-8），肘线按这个节距画

const Chevron = ({ open }: { open: boolean }) => (
  <svg
    viewBox="0 0 12 12"
    aria-hidden="true"
    className={`h-3 w-3 shrink-0 text-muted transition-transform duration-200 ease-out ${
      open ? "rotate-90" : ""
    }`}
  >
    <path
      d="M4.5 2.5 8 6l-3.5 3.5"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
    />
  </svg>
);

export function Tree({
  root,
  path,
  editingId,
  onEnter,
  onBeginRename,
  onCommitRename,
  onCancelRename,
}: {
  root: Node;
  path: string[];
  editingId: string | null;
  onEnter: (ids: string[]) => void;
  onBeginRename: (id: string, el: HTMLElement | null) => void;
  onCommitRename: (id: string, name: string) => void;
  onCancelRename: () => void;
}) {
  // 开局摊开前两层：文件树是要「扫一眼就找到文件」的，不是让人一层层点开找。
  const [open, setOpen] = useState<Set<string>>(
    () => new Set([root.id, ...(root.children ?? []).map((c) => c.id)]),
  );

  // 钻进去的时候，左列跟着展开到那一层 —— 不能让人在树里找不着自己。
  useEffect(() => {
    setOpen((prev) => {
      const next = new Set(prev);
      next.add(root.id);
      path.forEach((id) => next.add(id));
      return next;
    });
  }, [path, root.id]);

  const rows: { node: Node; depth: number; last: boolean }[] = [];
  const walk = (node: Node, depth: number, last: boolean, visible: boolean) => {
    if (!visible) return;
    rows.push({ node, depth, last });
    const kids = node.children ?? [];
    const isOpen = node.id === root.id || open.has(node.id);
    kids.forEach((kid, i) => walk(kid, depth + 1, i === kids.length - 1, isOpen));
  };
  walk(root, 0, true, true);

  const toggle = (id: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <ul className="flex flex-col px-2 py-2 text-[13px]" role="tree" aria-label="结构">
      {rows.map(({ node, depth, last }) => {
        const section = isSection(node);
        const isOpen = node.id === root.id || open.has(node.id);
        const isRoot = node.id === root.id;
        const here = path[path.length - 1] === node.id;
        const name = section && !isRoot ? `${node.name}/` : node.name;
        return (
          <li
            key={node.id}
            role="treeitem"
            aria-expanded={section ? isOpen : undefined}
            className="nm-host nm-host--row"
          >
            <div
              data-node-id={node.id}
              tabIndex={-1}
              onDoubleClick={(e) => {
                // 折叠箭头和 × 有自己一摊事，双击它们不该顺手开个输入框。
                if ((e.target as HTMLElement).closest("[data-no-rename]")) return;
                onBeginRename(node.id, e.currentTarget);
              }}
              className={`group relative flex h-8 items-center rounded-md pr-[30px] outline-none transition-colors duration-150 ${
                here ? "bg-accent/15" : "hover:bg-white/[0.045]"
              }`}
              style={{ paddingLeft: depth * INDENT + 4 }}
            >
              {depth > 0 && <Elbow depth={depth} last={last} />}
              {section ? (
                <button
                  type="button"
                  data-no-rename
                  onClick={() => toggle(node.id)}
                  aria-label={isOpen ? `收起 ${name}` : `展开 ${name}`}
                  className="relative z-10 flex h-5 w-5 shrink-0 items-center justify-center rounded text-muted hover:text-text"
                >
                  <Chevron open={isOpen} />
                </button>
              ) : (
                <span className="w-5 shrink-0" aria-hidden="true" />
              )}

              <TreeIcon name={iconFor(name, section)} open={isOpen} />

              {editingId === node.id ? (
                <NameInput
                  value={node.name}
                  onCommit={(next) => onCommitRename(node.id, next)}
                  onCancel={onCancelRename}
                  className="h-6 flex-1 px-1 text-[13px]"
                />
              ) : (
                <Row
                  section={section}
                  title={`${node.name} — 双击或 F2 改名`}
                  onEnter={() => onEnter(isRoot ? [] : [...path, node.id])}
                >
                  <span
                    className={`truncate leading-none ${section ? "text-text" : "text-text/75"}`}
                    style={{ letterSpacing: "-0.01em" }}
                  >
                    {name}
                  </span>
                  <span className="ml-auto shrink-0 pl-2 text-[11px] tabular-nums text-muted">
                    {valueOf(node)}
                  </span>
                </Row>
              )}

              {/* 原来那颗 × 让位给 •••：一行里挂两个删除没道理，删除进菜单。 */}
              {!isRoot && editingId !== node.id && (
                <NodeMenuTrigger nodeId={node.id} kind="row" />
              )}
            </div>
          </li>
        );
      })}
    </ul>
  );
}

/**
 * 弯角肘线：祖先的竖线从行顶贯穿到行底，本层的线在行中间拐 L 进图标。
 * 转折点 x = 父行把手（chevron）的中心，竖线从父行的把手正下方垂下来；
 * 每往上一层，导线就落在上上层把手的中心 —— 和 VS Code 文件树同一个节距。
 */
function Elbow({ depth, last }: { depth: number; last: boolean }) {
  const corner = (depth - 1) * INDENT + STEM;
  const w = corner + STUB;
  const ancestors = [...Array(depth - 1)].map((_, i) => i * INDENT + STEM);
  const mid = H / 2;
  const line = { stroke: "var(--n-color-diagram-line)", strokeOpacity: 0.28 } as const;
  return (
    <svg
      aria-hidden="true"
      className="pointer-events-none absolute inset-y-0 left-0"
      style={{ width: w }}
      viewBox={`0 0 ${w} ${H}`}
      preserveAspectRatio="none"
    >
      {ancestors.map((x) => (
        <line key={x} x1={x} y1={0} x2={x} y2={H} {...line} />
      ))}
      <path
        d={last ? `M${corner} 0 V${mid} H${w}` : `M${corner} 0 V${H}`}
        fill="none"
        {...line}
      />
      {!last && <line x1={corner} y1={mid} x2={w} y2={mid} {...line} />}
    </svg>
  );
}

export const pathNames = (root: Node, path: string[]): string[] =>
  path.map((id) => findNode(root, id)?.name ?? "?");

/** 组是按钮（点了钻进去），块只是行 —— 块自己没事情可做，就别假装能点。 */
function Row({
  section,
  title,
  onEnter,
  children,
}: {
  section: boolean;
  title: string;
  onEnter: () => void;
  children: React.ReactNode;
}) {
  const className = "flex min-w-0 flex-1 items-center gap-1.5 pl-1 text-left";
  return section ? (
    <button type="button" onClick={onEnter} className={className} title={title}>
      {children}
    </button>
  ) : (
    <div className={className} title={title}>
      {children}
    </div>
  );
}

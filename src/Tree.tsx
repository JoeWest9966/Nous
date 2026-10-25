import { useEffect, useState } from "react";
import { type Node, findNode, isSection, valueOf } from "./game";

/**
 * 左列：这盘棋的骨架。
 *
 * 树的形状就是数据的形状 —— 组能展开，行能点（点组＝钻进去）。
 * 参考 pele 的 treeView：缩进、连线、数值靠右。
 */

const Chevron = ({ open }: { open: boolean }) => (
  <svg
    viewBox="0 0 12 12"
    aria-hidden="true"
    className={`h-3 w-3 shrink-0 transition-transform duration-200 ease-out ${open ? "rotate-90" : ""}`}
  >
    <path d="M4.5 2.5 8 6l-3.5 3.5" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
  </svg>
);

export function Tree({
  root,
  path,
  onEnter,
  onRemove,
}: {
  root: Node;
  path: string[];
  onEnter: (ids: string[]) => void;
  onRemove: (id: string) => void;
}) {
  // 开局就把第一层摊开：左列是给人看结构的，不是给人点开找东西的。
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

  const toggle = (id: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });

  const rows: { node: Node; depth: number; last: boolean }[] = [];
  const walk = (node: Node, depth: number, last: boolean) => {
    rows.push({ node, depth, last });
    if (!open.has(node.id)) return;
    const kids = node.children ?? [];
    kids.forEach((kid, i) => walk(kid, depth + 1, i === kids.length - 1));
  };
  walk(root, 0, true);

  return (
    <ul className="flex flex-col gap-px px-2 py-2" role="tree" aria-label="结构">
      {rows.map(({ node, depth, last }) => {
        const section = isSection(node);
        const here = path[path.length - 1] === node.id;
        const isRoot = node.id === root.id;
        return (
          <li key={node.id} role="treeitem" aria-expanded={section ? open.has(node.id) : undefined}>
            <div
              className={`group flex h-8 items-center rounded-md pr-1.5 transition-colors duration-150 ${
                here ? "bg-accent/15 text-accent" : "text-text/85 hover:bg-white/[0.045]"
              }`}
              style={{ paddingLeft: depth * 13 + 4 }}
            >
              {/* 缩进导轨：一条极淡的竖线，让人看得出层级而不觉得被框住 */}
              {depth > 0 && (
                <span
                  aria-hidden="true"
                  className="mr-1 h-8 w-px shrink-0 bg-white/[0.07]"
                  style={{ marginLeft: -7 }}
                />
              )}
              {section ? (
                <button
                  type="button"
                  onClick={() => toggle(node.id)}
                  aria-label={open.has(node.id) ? `收起 ${node.name}` : `展开 ${node.name}`}
                  className="mr-1 flex h-5 w-5 shrink-0 items-center justify-center rounded text-muted hover:text-text"
                >
                  <Chevron open={open.has(node.id)} />
                </button>
              ) : (
                <span className="mr-1 h-5 w-5 shrink-0" aria-hidden="true" />
              )}

              <Row
                section={section}
                title={node.name}
                onEnter={() => onEnter(isRoot ? [] : [...path, node.id])}
              >
                <span
                  className={`truncate text-[13px] leading-none ${section ? "" : "text-text/70"}`}
                  style={{ letterSpacing: "-0.01em" }}
                >
                  {node.name}
                </span>
                <span className="ml-auto shrink-0 font-mono text-[11px] tabular-nums text-muted">
                  {valueOf(node)}
                </span>
                {section && !last && <span className="w-2" aria-hidden="true" />}
              </Row>
              {/* 盘面上没有标题带的窄 section 点不到 ×，所以在左边也给一个入口 */}
              {!isRoot && (
                <button
                  type="button"
                  onClick={() => onRemove(node.id)}
                  aria-label={`删除 ${node.name}`}
                  title={`删除 ${node.name}（可撤销）`}
                  className="ml-1 flex h-5 w-5 shrink-0 items-center justify-center rounded-[3px] text-[12px] leading-none text-muted opacity-0 transition-opacity duration-150 hover:bg-white/10 hover:text-text focus-visible:opacity-100 group-hover:opacity-100"
                >
                  ×
                </button>
              )}
            </div>
          </li>
        );
      })}
    </ul>
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
  const className = "flex min-w-0 flex-1 items-center gap-2 text-left";
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

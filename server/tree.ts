/**
 * 块/组树的最小模型 —— 面板前端删除后，服务端只保留壳真正需要的树读取。
 *
 * 两种节点，只有两种（中文只说这两个词：**块** 和 **组**）：
 * - **块**（代码里叫 leaf）—— 有值，面积就是它的值
 * - **组**（代码里叫 section）—— 没有值，值是子节点之和，负责把自己那片地方圈起来
 */
export type Node = {
  id: string;
  name: string;
  /** 只有块有值。组靠子节点算。 */
  value?: number;
  /** 只有组有 children。 */
  children?: Node[];
};

export const isSection = (n: Node): boolean => Array.isArray(n.children);

/**
 * 全树的 **tree**：每个节点在树里的地址（`N/src/App.tsx`），同名按出现顺序加 `-2`。
 *
 * tree 不是 id。改名、搬家它都会变 —— 这是允许的：**id 负责永不失效，tree 负责念得出来**。
 * 所以它不进 tree.json、不进 log.json，每次从树现算；重名编号跟遍历顺序有关，
 * 必须一次算全树，一个个算会算出行数不一致的结果。
 */
export function treeMap(root: Node): Map<string, string> {
  const out = new Map<string, string>();
  const used = new Map<string, number>();
  const walk = (node: Node, prefix: string): void => {
    const base = prefix ? `${prefix}/${node.name}` : node.name;
    const nth = used.get(base) ?? 0;
    used.set(base, nth + 1);
    const tree = nth === 0 ? base : `${base}-${nth + 1}`;
    out.set(node.id, tree);
    // 子节点挂在这一支 resolved 过的 tree 下面 —— 爹带 -2 的话，儿子也跟着带。
    node.children?.forEach((kid) => walk(kid, tree));
  };
  walk(root, "");
  return out;
}

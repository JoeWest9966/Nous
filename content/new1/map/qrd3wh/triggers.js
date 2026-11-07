// triggers —— 触发器的**只读投影**：Nous-Trigger 仓扫出来的 index.json。
//
// 为什么住在这儿：终极模板（Ultimate-Template）要引用触发器，还要**照触发器自己的声明
// 校验参数**（哪个参数必填、类型对不对）。那份声明在另一个仓里 —— 只有引擎读得到
// （仓自己的 index.mjs 也读不到，它管的是模板仓）。
//
// 所以这一层只做一件事：**把另一个仓的索引读成一张表**，别的什么都不管：
//   · 不跑 git、不写盘、不维护第二份名单 —— 盘上那份 index.json 就是真相。
//   · 读不到 = 空表（不是错误）：触发器仓还没建，或者还没扫过。
//
// ⚠️ 一条纪律：**这一层不认识「参数怎么填」**。参数的类型、必填、选项，全是触发器
// 自己 `nous.json` 里声明的（`params`）。这里只把那份声明原样带出去，让校验和页面
// 照它画 —— 页面不许认触发器的名字。

import { readFile } from "node:fs/promises";
import path from "node:path";

const TRIGGER_ROOT = process.env.NOUS_TRIGGER ?? "/opt/Nous-Trigger";

/**
 * 读触发器仓的索引，返回一张表：`name → { name, summary, category, params }`。
 *
 * @param {string} [root] 触发器仓的本地副本
 */
export async function readTriggerCatalog(root = TRIGGER_ROOT) {
  let raw;
  try {
    raw = await readFile(path.join(root, "index.json"), "utf8");
  } catch {
    return new Map(); // 仓还没建 / 还没扫：空表，不是错误
  }
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return new Map(); // 索引坏了：空表（页面会显示「触发器一个都没有」，比 500 诚实）
  }
  const items = Array.isArray(parsed?.triggers) ? parsed.triggers : [];
  const table = new Map();
  for (const item of items) {
    const name = String(item?.name ?? "").trim();
    if (!name) continue;
    table.set(name, {
      name,
      summary: String(item.summary ?? ""),
      category: Array.isArray(item.category) ? item.category : [],
      version: String(item.version ?? ""),
      // 参数声明原样带出去：这一层不解释它。
      params: Array.isArray(item.params) ? item.params : [],
    });
  }
  return table;
}

/** 触发器在不在。 */
export const triggerHas = (catalog, name) => catalog?.has?.(String(name ?? "")) ?? false;

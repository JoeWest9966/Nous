// 翻页 —— durable 的列表查询一页最多 200 条（`scanConversations({}, 200, cursor)`、
// `entries({...}, 200, cursor)`），`page.next` 还在就说明后面还有。只看第一页等于静默漏：
// 会话过了 200 条，未读和水位就开始少算，而接口照样回 200，没人看得出来。
//
// 单独一个文件是为了能被单独验：不 import server.js（那个有顶层 await + sqlite）。

/**
 * 一页一页取完，回所有记录（按取到的顺序）。
 *
 * @param fetchPage `(cursor) => { items, next? }`；第一次的 cursor 是 undefined，
 *                  之后用上一页给的 `next`，没有 `next` 就是最后一页。
 */
export const collectPages = async (fetchPage) => {
  const items = [];
  let cursor;
  for (;;) {
    const page = await fetchPage(cursor);
    items.push(...page.items);
    if (page.next === undefined || page.next === null) break;
    cursor = page.next;
  }
  return items;
};

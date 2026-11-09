// `pages.js` —— 「一页一页取到没有 next 为止」这件事只写一次。
//
// 为什么值得单独一条：durable 的 `scanConversations` / `entries` 一页最多 200 条。
// 只看第一页就是静默漏 —— 会话超过 200 条时少种水位、少报未读，而没人看得出来。
// 这里钉的是翻页本身：源是假的（一串固定页），不看真库。

import test from "node:test";
import assert from "node:assert/strict";

import { collectPages } from "../pages.js";

/** 假的分页源：一串页，每页 `{ items, next? }`，`next` 是下一页的下标。 */
const pager = (pages) => {
  const seen = [];
  return {
    seen,
    fetchPage: async (cursor) => {
      seen.push(cursor);
      const page = pages[cursor ?? 0];
      return page.next === undefined ? { items: page.items } : { items: page.items, next: page.next };
    },
  };
};

test("pages：三页跟到底，一条不落", async () => {
  const source = pager([
    { items: [1, 2], next: 1 },
    { items: [3], next: 2 },
    { items: [4, 5] },
  ]);

  assert.deepEqual(await collectPages(source.fetchPage), [1, 2, 3, 4, 5]);
  // 第一次没有 cursor，之后用上一页给的 next —— 别跳过任何一页。
  assert.deepEqual(source.seen, [undefined, 1, 2]);
});

test("pages：最后一页没有 next 就停 —— 一页也照样取一次", async () => {
  const source = pager([{ items: [401, 402] }]);

  assert.deepEqual(await collectPages(source.fetchPage), [401, 402]);
  assert.deepEqual(source.seen, [undefined]);
});

test("pages：最后一页是空的也没有 next —— 停下，不空转", async () => {
  const source = pager([{ items: [1], next: 1 }, { items: [] }]);

  assert.deepEqual(await collectPages(source.fetchPage), [1]);
  assert.deepEqual(source.seen, [undefined, 1]);
});

// `unread.js` 的行为 —— 每条会话读到哪了（EntryId 水位），未读就是「水位之后还有几条」。
//
// 为什么存水位不存条数：fork 和 compaction 会让「条数」变成另一件事（同一个 id 可能
// 出现在两条会话里、老条目可能被压掉），水位是单调的，不会因此错位。
//
// durable 那半边（最新 id 是多少、水位之后有几条）是注进来的：这个模块只管「水位记在哪、
// 什么时候该推进」，不 import server.js（那个有顶层 await + sqlite）。

import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { openUnread } from "../unread.js";

const tmp = () => mkdtemp(path.join(tmpdir(), "nous-unread-"));
const cleanup = (dir) => rm(dir, { recursive: true, force: true });

/** 假的 durable 那一半：每个会话一串 id（升序），跟真库一样是全局单调的。 */
const fakeEntries = (table) => ({
  latest: (id) => (table[id]?.length ? table[id][table[id].length - 1] : null),
  countAfter: (id, afterId) => (table[id] ?? []).filter((entryId) => entryId > afterId).length,
});

const clock = (start = 1_700_000_000_000) => {
  let t = start;
  return () => t++;
};

test("unread：没水位的会话是 0 条未读、readAt 是 null，并顺手种在「当时最新」上", async () => {
  const dir = await tmp();
  try {
    const entries = fakeEntries({ 14: [401, 402, 458] });
    const marks = await openUnread(path.join(dir, "read.json"), { now: clock(), entries });

    // 老会话不该一开张就 458 条未读。
    assert.deepEqual(await marks.unreadOf(14), { unread: 0, readAt: null });
    // 种下去的水位是「当时最新」，不是 0 —— 否则下一条消息进来会算出 459 条。
    assert.deepEqual(marks.watermark(14), { lastReadId: 458, readAt: null });
  } finally {
    await cleanup(dir);
  }
});

test("unread：新 entry 进来才算未读，markRead 之后归零并记下读的时刻", async () => {
  const dir = await tmp();
  try {
    const table = { 14: [401, 402, 458] };
    const entries = fakeEntries(table);
    const marks = await openUnread(path.join(dir, "read.json"), { now: clock(), entries });
    await marks.seed(14); // 水位 458

    table[14].push(459, 460);
    assert.deepEqual(await marks.unreadOf(14), { unread: 2, readAt: null });

    assert.deepEqual(await marks.markRead(14), { id: 14, readAt: 1_700_000_000_000, unread: 0 });
    assert.deepEqual(await marks.unreadOf(14), { unread: 0, readAt: 1_700_000_000_000 });
  } finally {
    await cleanup(dir);
  }
});

test("unread：水位不进位 —— 拿一个更小的最新 id 回来，也不把水位往回拨", async () => {
  const dir = await tmp();
  try {
    const table = { 14: [401, 458] };
    const entries = fakeEntries(table);
    const marks = await openUnread(path.join(dir, "read.json"), { now: clock(), entries });
    await marks.markRead(14);
    assert.deepEqual(marks.watermark(14), { lastReadId: 458, readAt: 1_700_000_000_000 });

    // 回滚 / fork：这条会话现在的最新 id 反而更小。
    table[14] = [401, 420];
    await marks.markRead(14);
    assert.equal(marks.watermark(14).lastReadId, 458);
  } finally {
    await cleanup(dir);
  }
});

test("unread：水位写下去之后重新开一个实例还在，已有的水位不会被再种一次", async () => {
  const dir = await tmp();
  const file = path.join(dir, "read.json");
  try {
    const table = { 14: [401, 402, 458] };
    const first = await openUnread(file, { now: clock(), entries: fakeEntries(table) });
    await first.markRead(14);
    await first.flush();

    // 重启，而且这条会话后面又长了几条：水位还是 458，未读是新的那几条。
    table[14].push(459);
    const second = await openUnread(file, { now: clock(1_800_000_000_000), entries: fakeEntries(table) });
    assert.deepEqual(second.watermark(14), { lastReadId: 458, readAt: 1_700_000_000_000 });
    // 再种一次不该把水位推到 459（那会吞掉一条真未读）。
    assert.equal(await second.seed(14), false);
    assert.deepEqual(second.watermark(14), { lastReadId: 458, readAt: 1_700_000_000_000 });
    assert.deepEqual(await second.unreadOf(14), { unread: 1, readAt: 1_700_000_000_000 });
  } finally {
    await cleanup(dir);
  }
});

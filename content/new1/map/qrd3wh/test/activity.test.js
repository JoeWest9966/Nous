// `activity.js` 的行为 —— 「这条会话最后一次写库是什么时候」。
//
// 为什么要我们自己记：durable 的 `EntryRecord` 没有 wall-clock，`agent.sqlite` 里也没有时间列。
// 所以这表只能引擎自己攒，攒的时候宁可空，也不要拿别的时间冒充（票里写死了这条）。
//
// 时钟是注进来的：断言用的是手写的字面量（1_700_000_000_000 这种），不是「跑一遍再看是多少」，
// 否则两个 note 挨得太近、毫秒一样，测试就成了同义反复。

import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { openActivity } from "../activity.js";

const tmp = () => mkdtemp(path.join(tmpdir(), "nous-activity-"));
const cleanup = (dir) => rm(dir, { recursive: true, force: true });

test("activity：没记过的会话是 null，记一次之后就是那个时刻", async () => {
  const dir = await tmp();
  try {
    let clock = 1_700_000_000_000;
    const activity = await openActivity(path.join(dir, "activity.json"), { now: () => clock, flushMs: 5 });

    assert.equal(activity.at(14), null);

    activity.note(14);
    assert.equal(activity.at(14), 1_700_000_000_000);

    clock = 1_700_000_060_000;
    activity.note(14);
    assert.equal(activity.at(14), 1_700_000_060_000);

    // 别人没写过，还是空 —— 别让一条会话的时间漏到另一条身上。
    assert.equal(activity.at(15), null);
  } finally {
    await cleanup(dir);
  }
});

test("activity：写下去的，重新开一个实例还读得回来", async () => {
  const dir = await tmp();
  const file = path.join(dir, "activity.json");
  try {
    const first = await openActivity(file, { now: () => 1_700_000_000_000, flushMs: 5 });
    first.note(14);
    await first.flush();

    // 换个实例 = 重启：内存里那张表没了，只剩盘上那份。
    const second = await openActivity(file, { now: () => 1_700_000_999_999, flushMs: 5 });
    assert.equal(second.at(14), 1_700_000_000_000);
    assert.equal(second.at(15), null);
  } finally {
    await cleanup(dir);
  }
});

test("activity：攒着写 —— 连着 note 不落盘，flush 之后才落，且落的是最后那个值", async () => {
  const dir = await tmp();
  const file = path.join(dir, "activity.json");
  try {
    let clock = 1_700_000_000_000;
    // flushMs 给很长：这个测试要自己控制落盘时机，不等计时器。
    const activity = await openActivity(file, { now: () => clock++, flushMs: 60_000 });
    activity.note(14);
    activity.note(14);
    activity.note(14);
    await new Promise((resolve) => setTimeout(resolve, 50));
    assert.equal(await stat(file).then(() => true, () => false), false);

    await activity.flush();
    const reread = await openActivity(file, { flushMs: 60_000 });
    assert.equal(reread.at(14), 1_700_000_000_002);
  } finally {
    await cleanup(dir);
  }
});

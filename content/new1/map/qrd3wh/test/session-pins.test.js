// session-pins.test.js —— 置顶进内核的契约（票 #15 / spec §3.7）。
// 断言真实响应形状 + 提交日志语义；不 import server.js。
import test from "node:test";
import assert from "node:assert/strict";

import { openSessionPins } from "../session-pins.js";

/** 假的 durable 的 commit 回调：请求数涨了就落一条提交，seq +1（像 pi-durable 的 publication.seq）。 */
const fakeCommit = (state) => async (change) => {
  const before = Object.keys(state.doc.pins?.requests ?? {}).length;
  const result = await change({ doc: async () => state.doc });
  const after = Object.keys(state.doc.pins?.requests ?? {}).length;
  if (after > before) state.commits.push(++state.seq);
  return result;
};

/** 假的 durable：读-改-写在一次 commit 内；复用 fakeCommit，新实例接同一份文档时锚不丢。 */
const commitLog = () => {
  const state = { seq: 100, commits: [], doc: {} };
  const pins = openSessionPins({
    commit: fakeCommit(state),
    peek: async (_token, anchor) => state.doc.pins?.requests?.[anchor],
  });
  return { state, pins };
};

const post = (pins, id, pinned, requestId) => pins.setPin({ id, pinned, requestId, token: "Sessions" });
const ids = (state) => state.doc.pins?.ids ?? [];

test("pin：置顶在前、其余保持原顺序；置顶顺序就是 pins.ids 的顺序", async () => {
  const { pins, state } = commitLog();
  await post(pins, 710, true, "r-710");
  await post(pins, 496, true, "r-496");
  await post(pins, 621, false, "r-621");

  const ordered = pins.decorate([{ id: 2 }, { id: 496 }, { id: 621 }, { id: 710 }, { id: 14 }], ids(state));
  assert.deepEqual(ordered.map((item) => item.id), [496, 710, 2, 621, 14]);
  assert.deepEqual(ordered.map((item) => item.pinned), [true, true, false, false, false]);
});

test("pin：取消置顶把它放回原来的队伍，不再插队", async () => {
  const { pins, state } = commitLog();
  await post(pins, 710, true, "a");
  await post(pins, 496, true, "b");
  await post(pins, 710, false, "c");

  const ordered = pins.decorate([{ id: 2 }, { id: 496 }, { id: 710 }], ids(state));
  assert.deepEqual(ordered.map((item) => item.id), [496, 2, 710]);
});

test("pin：同一个 requestId 重放十次只落一条提交，version 不涨", async () => {
  const { pins, state } = commitLog();
  const first = await post(pins, 14, true, "desk-pin-14");
  assert.deepEqual(first, { id: 14, pinned: true, changed: true, requestId: "desk-pin-14", version: 1 });
  assert.equal(state.commits.length, 1);

  for (let i = 0; i < 10; i++) {
    assert.deepEqual(await post(pins, 14, true, "desk-pin-14"), first);
  }
  assert.equal(state.commits.length, 1);
});

test("pin：重放锚是持久化状态 —— 新实例读同一份文档也不写第二条", async () => {
  const { pins, state } = commitLog();
  const first = await post(pins, 14, true, "restart-anchor");
  const again = openSessionPins({
    commit: fakeCommit(state),
    peek: async (_token, anchor) => state.doc.pins?.requests?.[anchor],
  });
  assert.deepEqual(await post(again, 14, true, "restart-anchor"), first);
  assert.equal(state.commits.length, 1);
});

test("pin：不同 requestId 重复置顶不写第二条（changed=false 但仍是一条新提交）", async () => {
  const { pins, state } = commitLog();
  const first = await post(pins, 14, true, "a");
  const second = await post(pins, 14, true, "b");
  assert.notEqual(second.version, first.version);
  assert.equal(first.changed, true);
  assert.equal(second.changed, false);
  assert.equal(state.commits.length, 2);
  assert.deepEqual(ids(state), ["14"]);
});

test("pin：同一个 requestId 换会话也不认 —— 锚是这次决定，不是会话 id", async () => {
  const { pins, state } = commitLog();
  const first = await post(pins, 14, true, "same-anchor");
  assert.deepEqual(await post(pins, 99, true, "same-anchor"), first);
  assert.equal(state.commits.length, 1);
  assert.deepEqual(ids(state), ["14"]);
});

test("pin：没有 requestId 直接 400，一个字节也不提交", async () => {
  const { pins, state } = commitLog();
  await assert.rejects(() => post(pins, 14, true, ""), (error) => error.status === 400);
  assert.equal(state.commits.length, 0);
  assert.deepEqual(ids(state), []);
});

test("pin：并发两次置顶不丢更新 —— 后一单看到前一单写下的 ids", async () => {
  const { pins, state } = commitLog();
  // 模拟 durable 的串行提交线：每个 change 在上一单落盘后才执行。
  await Promise.all([post(pins, 710, true, "c1"), post(pins, 496, true, "c2")]);
  const current = ids(state);
  assert.deepEqual([...current].sort(), ["496", "710"]);
});

test("pin：version 是全局提交序号 —— 跨会话单调，重放不涨", async () => {
  const { pins, state } = commitLog();
  assert.deepEqual(await post(pins, 1694, true, "g1"), {
    id: 1694,
    pinned: true,
    changed: true,
    requestId: "g1",
    version: 1,
  });
  assert.equal((await post(pins, 1694, true, "g2")).version, 2);
  assert.equal((await post(pins, 1694, true, "g3")).version, 3);
  assert.equal((await post(pins, 2, true, "g4")).version, 4); // 跨会话继续涨
  assert.deepEqual(await post(pins, 2, true, "g4"), {
    id: 2,
    pinned: true,
    changed: true,
    requestId: "g4",
    version: 4,
  });
});

test("pin：每会话只留最近 50 个锚 —— 文档体积有界，置顶状态不受淘汰影响", async () => {
  const { pins, state } = commitLog();
  // 先置顶，再打满 50 个锚，记录体积（后续锚都 pinned=true，置顶状态不该被改动）。
  await post(pins, 1694, true, "pin-on");
  for (let i = 1; i < 50; i++) await post(pins, 1694, true, `old-${i}`);
  const sizeAt50 = JSON.stringify(state.doc).length;

  // 同一会话再打 11 个不同锚（总数超过限额）。
  for (let i = 50; i < 59; i++) await post(pins, 1694, true, `old-${i}`);
  const newest = await post(pins, 1694, true, "newest");
  const anchors = Object.keys(state.doc.pins.requests);

  assert.ok(anchors.length <= 50, `该会话锚数 ${anchors.length} 应 ≤ 50`);
  assert.ok(anchors.includes("newest"), "最新那个锚必须在");
  assert.ok(!anchors.includes("pin-on"), "最旧的锚应已淘汰");
  assert.equal(state.doc.pins.requests["newest"].version, newest.version);

  // 淘汰只丢重放锚，不丢置顶状态：ids 里还有这个会话，列表仍排最前。
  assert.deepEqual(ids(state), ["1694"]);
  const ordered = pins.decorate([{ id: 2 }, { id: 1694 }], ids(state));
  assert.deepEqual(ordered.map((item) => item.id), [1694, 2]);
  assert.equal(ordered[0].pinned, true);

  // 文档体积不随锚数量无限增长：打满 50 后再打 11 个，体积应基本不动。
  const sizeAfter = JSON.stringify(state.doc).length;
  console.log(`[pin 保留策略] 50 锚时 ${sizeAt50}B → 61 锚后 ${sizeAfter}B（同一会话锚数 ${anchors.length}）`);
  assert.ok(sizeAfter < sizeAt50 * 1.15, `体积从 ${sizeAt50}B 涨到 ${sizeAfter}B，没被限额压住`);
});

test("pin：淘汰后 version 仍全局单调（不能拿 requests 数量当计数）", async () => {
  const { pins, state } = commitLog();
  for (let i = 0; i < 60; i++) await post(pins, 1694, true, `a${i}`);
  const afterPrune = await post(pins, 2, true, "b1");
  assert.equal(afterPrune.version, 61);
  assert.equal(Object.keys(state.doc.pins.requests).length, 51);
});

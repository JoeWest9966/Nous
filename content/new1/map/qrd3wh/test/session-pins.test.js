// session-pins.test.js —— 置顶进内核的契约（票 #15 / spec §3.7）。
// 断言真实响应形状 + 提交日志语义；不 import server.js。
import test from "node:test";
import assert from "node:assert/strict";

import { openSessionPins } from "../session-pins.js";

/** 假的 durable：读-改-写在一次 commit 内，提交后 seq +1，像 pi-durable 的 publication.seq。 */
const commitLog = () => {
  const state = { seq: 100, commits: [], doc: {} };
  const pins = openSessionPins({
    commit: async (change) => {
      const before = Object.keys(state.doc.pins?.requests ?? {}).length;
      const result = await change({ doc: async () => state.doc });
      const after = Object.keys(state.doc.pins?.requests ?? {}).length;
      if (after > before) state.commits.push(++state.seq);
      return result;
    },
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
    commit: async (change) => {
      const before = Object.keys(state.doc.pins?.requests ?? {}).length;
      const result = await change({ doc: async () => state.doc });
      const after = Object.keys(state.doc.pins?.requests ?? {}).length;
      if (after > before) state.commits.push(++state.seq);
      return result;
    },
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

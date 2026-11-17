// pin-http.test.js —— 票 #15 的真实请求 / 真实响应快照。
// 跑法：`node --test test/`（不加路径参数）。
// 这是唯一一个会起内核进程的测试：临时 data 目录、临时端口，绝不碰线上 agent.sqlite。
// **本测试不依赖机器上的扩展仓**：扩展 / 技能 / 提示词 / 预设 / 触发器全用临时桩，
// 所以开发机（没有 /opt/Nous-extend、/opt/Nous-skills）和 j1900 上跑法一样。
//
// 快照的形状归这个测试管；wire.md 只留标题。
import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { stubEnv } from "./bootstrap.js";

const BLOCK = path.resolve(import.meta.dirname, "..");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** 等 healthz 起来，最多 20 秒；起不来就把子进程 stderr 放进错误里。 */
const waitUp = async (port, proc, log) => {
  for (let i = 0; i < 200; i++) {
    if (proc.exitCode !== null) throw new Error(`内核退了（${proc.exitCode}）：${log.join("").slice(-800)}`);
    try {
      const r = await fetch(`http://127.0.0.1:${port}/healthz`);
      if (r.ok) return;
    } catch {}
    await sleep(100);
  }
  throw new Error(`内核没起来：${log.join("").slice(-800)}`);
};

const startKernel = async (dataDir, port, stubs) => {
  const log = [];
  const proc = spawn(process.execPath, ["server.js"], {
    cwd: BLOCK,
    env: {
      ...process.env,
      PORT: String(port),
      NOUS_DATA: dataDir,
      ...stubs,
      NOUS_VPS_BASE: "http://127.0.0.1:1", // 机器编目不可用，别连线上
      NOUS_START_DIRS: dataDir,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  proc.stdout.on("data", (b) => log.push(String(b)));
  proc.stderr.on("data", (b) => log.push(String(b)));
  await waitUp(port, proc, log);
  return {
    proc,
    log,
    stop: async () => {
      if (proc.exitCode === null) {
        proc.kill("SIGTERM");
        await new Promise((r) => proc.once("exit", r));
      }
    },
  };
};

const base = (port) => `http://127.0.0.1:${port}`;
const request = async (port, method, pathname, body) => {
  const resp = await fetch(`${base(port)}${pathname}`, {
    method,
    headers: body ? { "content-type": "application/json" } : {},
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await resp.text();
  return { status: resp.status, body: text ? JSON.parse(text) : null };
};

/** 选一个临时端口：bind 0 拿到编号再释放，够测试用（不并发跑内核）。 */
const freePort = async () => {
  const { createServer } = await import("node:net");
  const s = createServer();
  await new Promise((r) => s.listen(0, "127.0.0.1", r));
  const port = s.address().port;
  await new Promise((r) => s.close(r));
  return port;
};

test("pin HTTP：真实请求 + 真实响应，重放 / 排序 / 重启都在", { timeout: 120_000 }, async () => {
  const data = await mkdtemp(path.join(tmpdir(), "nous-pin-http-"));
  const stubs = stubEnv;
  const port = await freePort();
  let kernel = await startKernel(data, port, stubs);
  try {
    // 用内核真的会话（健康检查之后就能建，不需要模型）。
    const a = await request(port, "POST", "/v1/sessions", { name: "pin-A" });
    const b = await request(port, "POST", "/v1/sessions", { name: "pin-B" });
    assert.equal(a.status, 201, JSON.stringify(a.body));
    assert.equal(b.status, 201);
    const [idA, idB] = [a.body.id, b.body.id];

    // 快照：一个请求的真实响应形状（id / pinned / changed / requestId / version）。
    const first = await request(port, "POST", `/v1/sessions/${idA}/pin`, {
      pinned: true,
      requestId: "snap-pin-a",
    });
    assert.equal(first.status, 200, JSON.stringify(first.body));
    assert.deepEqual(Object.keys(first.body).sort(), ["changed", "id", "pinned", "requestId", "version"]);
    assert.equal(first.body.id, idA);
    assert.equal(first.body.pinned, true);
    assert.equal(first.body.changed, true);
    assert.equal(first.body.requestId, "snap-pin-a");
    assert.equal(typeof first.body.version, "number");
    const version = first.body.version;

    // 不同 requestId 的新决定 = 新提交，version 单调增长。
    const nextSeqProbe = await request(port, "POST", `/v1/sessions/${idB}/pin`, { pinned: true, requestId: "snap-pin-b" });
    assert.ok(nextSeqProbe.body.version > version);

    // 同一 requestId 重放 10 次：回包原样，version 不涨（重放回包一字不差）。
    for (let i = 0; i < 10; i++) {
      const replay = await request(port, "POST", `/v1/sessions/${idA}/pin`, {
        pinned: true,
        requestId: "snap-pin-a",
      });
      assert.equal(replay.status, 200);
      assert.deepEqual(replay.body, first.body);
    }
    const afterReplays = await request(port, "GET", "/v1/sessions");
    assert.equal(afterReplays.status, 200);
    const rowA = afterReplays.body.items.find((item) => item.id === idA);
    const rowB = afterReplays.body.items.find((item) => item.id === idB);
    assert.equal(rowA.pinned, true);
    assert.equal(rowB.pinned, true);
    // B 后置顶 → 排在最前；A 第二。列表顺序由提交重算，不是客户端本地。
    const posA = afterReplays.body.items.findIndex((item) => item.id === idA);
    const posB = afterReplays.body.items.findIndex((item) => item.id === idB);
    assert.ok(posB < posA, `后置顶的 B 应该在前：B=${posB}, A=${posA}`);
    // 其余会话跟在置顶后面（原顺序保持）。
    assert.equal(
      afterReplays.body.items.slice(2).some((item) => item.pinned),
      false,
    );

    // 重启（同一 data 目录）：置顶还在，重放锚还在（再重放一次不落新提交）。
    await kernel.stop();
    kernel = await startKernel(data, port, stubs);
    const restarted = await request(port, "GET", "/v1/sessions");
    assert.equal(restarted.body.items.find((item) => item.id === idA)?.pinned, true);
    // 重放拿回的是首轮那条提交（version 原样），不是新落一条。
    const replayAfterRestart = await request(port, "POST", `/v1/sessions/${idA}/pin`, {
      pinned: true,
      requestId: "snap-pin-a",
    });
    assert.deepEqual(replayAfterRestart.body, first.body);

    // 取消置顶也是一条提交。
    const unpinned = await request(port, "POST", `/v1/sessions/${idA}/pin`, {
      pinned: false,
      requestId: "snap-unpin-a",
    });
    assert.equal(unpinned.body.pinned, false);
    assert.equal(unpinned.body.changed, true);
    const final = await request(port, "GET", "/v1/sessions");
    assert.equal(final.body.items.find((item) => item.id === idA)?.pinned, false);
    assert.equal(final.body.items[0].id, idB);
  } finally {
    await kernel.stop();
    await rm(data, { recursive: true, force: true });
  }
});

test("pin HTTP：缺 requestId 回 400，pinned 不是布尔也回 400", { timeout: 120_000 }, async () => {
  const data = await mkdtemp(path.join(tmpdir(), "nous-pin-http-bad-"));
  const stubs = stubEnv;
  const port = await freePort();
  const kernel = await startKernel(data, port, stubs);
  try {
    const created = await request(port, "POST", "/v1/sessions", { name: "pin-bad" });
    const id = created.body.id;
    const noAnchor = await request(port, "POST", `/v1/sessions/${id}/pin`, { pinned: true });
    assert.equal(noAnchor.status, 400);
    const badFlag = await request(port, "POST", `/v1/sessions/${id}/pin`, {
      pinned: "true",
      requestId: "bad-flag",
    });
    assert.equal(badFlag.status, 400);
    const list = await request(port, "GET", "/v1/sessions");
    assert.equal(list.body.items.find((item) => item.id === id)?.pinned, false);
  } finally {
    await kernel.stop();
    await rm(data, { recursive: true, force: true });
  }
});

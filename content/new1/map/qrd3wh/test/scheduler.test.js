// scheduler.js 的行为 —— 全注入，不起真进程、不碰真仓。
//
// 测的是「节奏」这一层：fire 了 launch、不 fire 不 launch、出错报出来。
// 「该不该 fire」是触发器的事，这一层不测（那是 single-shot 自己的单元测试）。

import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile, chmod } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { createScheduler, runTriggerBinary } from "../scheduler.js";

test("调度器：fire 了就去 launch", async () => {
  const launched = [];
  const scheduler = createScheduler({
    listCombos: async () => [{ name: "morning", trigger: "single-shot", params: {}, session: "weekly-report" }],
    runTrigger: async () => ({ fire: true, reason: "到点了", data: {} }),
    launch: async (combo, result) => {
      launched.push([combo.name, result.reason]);
    },
  });
  await scheduler.tick();
  assert.deepEqual(launched, [["morning", "到点了"]]);
});

test("调度器：不 fire 就不 launch", async () => {
  const launched = [];
  const scheduler = createScheduler({
    listCombos: async () => [{ name: "morning", trigger: "single-shot", params: {}, session: "weekly-report" }],
    runTrigger: async () => ({ fire: false, reason: "还没到", data: {} }),
    launch: async () => {
      launched.push("不该来的");
    },
  });
  await scheduler.tick();
  assert.deepEqual(launched, []);
});

test("调度器：跑挂了走 onError，不吞", async () => {
  const errors = [];
  const scheduler = createScheduler({
    listCombos: async () => [{ name: "bad", trigger: "ghost", params: {}, session: "weekly-report" }],
    runTrigger: async () => ({ fire: false, error: "退出码 2：炸了" }),
    launch: async () => {
      throw new Error("不该 launch");
    },
    onError: (combo, message) => errors.push([combo.name, message]),
  });
  await scheduler.tick();
  assert.deepEqual(errors, [["bad", "退出码 2：炸了"]]);
});

test("调度器：listCombos 抛了也不崩（记 lastError）", async () => {
  const scheduler = createScheduler({
    listCombos: async () => {
      throw new Error("名册读不动");
    },
    runTrigger: async () => ({ fire: true }),
    launch: async () => {},
  });
  await scheduler.tick();
  assert.match(scheduler.lastError, /名册读不动/);
});

test("runTriggerBinary：真跑一个假触发器，fire + data 原样带出来", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "scheduler-bin-"));
  const script = path.join(dir, "fake-trigger");
  // 一个 shell 假触发器：读 stdin、回一行 JSON。跟真触发器同一个契约。
  await writeFile(
    script,
    `#!/bin/sh
read input
echo '{"fire":true,"reason":"到点了","data":{"at":1800000060}}'
`,
  );
  await chmod(script, 0o755);
  const result = await runTriggerBinary({
    binary: script,
    params: { epoch: 1800000060 },
    now: 1800000060,
    stateDir: dir,
  });
  assert.equal(result.fire, true);
  assert.equal(result.reason, "到点了");
  assert.deepEqual(result.data, { at: 1800000060 });
  assert.equal(result.error, "");
});

test("runTriggerBinary：非 0 退出 = 出错，不是「不 fire」", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "scheduler-bin-"));
  const script = path.join(dir, "fake-trigger");
  await writeFile(script, `#!/bin/sh\necho oops >&2\nexit 2\n`);
  await chmod(script, 0o755);
  const result = await runTriggerBinary({ binary: script, params: {}, now: 1, stateDir: dir });
  assert.equal(result.fire, false);
  assert.match(result.error, /退出码 2/);
});

test("runTriggerBinary：stdout 不是 JSON = 出错", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "scheduler-bin-"));
  const script = path.join(dir, "fake-trigger");
  await writeFile(script, `#!/bin/sh\necho "你好，不是 JSON"\n`);
  await chmod(script, 0o755);
  const result = await runTriggerBinary({ binary: script, params: {}, now: 1, stateDir: dir });
  assert.equal(result.fire, false);
  assert.match(result.error, /不是 JSON/);
});

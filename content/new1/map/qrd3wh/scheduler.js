// scheduler.js —— 触发器调度器。
//
// 干一件事：**按时跑触发器，fire 了就去起会话**。
//
// 触发器的契约（见 n8qkkj/AGENTS.md）：
//   stdin  一行 JSON：{"now": <epoch 秒>, "params": {…}}
//   stdout 一行 JSON：{"fire": true|false, "reason": "…", "data": {…}}
//   退出码 0 = 正常（fire 真假都算）；非 0 = 出错
//
// 这一层**不判断「该不该 fire」** —— 那是触发器自己的事。这一层只做：
//   1. 到点跑一次（listCombos 给谁跑，runTrigger 怎么跑，都是调用方注入的）
//   2. fire 了就 launch（起会话也是注入的 —— 「手动跑」和「自动跑」走同一条路）
//   3. 出错报出来（onError），别悄悄吞
//
// 全注入 = 能裸测：测试里给假的 listCombos / runTrigger / launch，一个真进程都不用起。

import { spawn } from "node:child_process";

/** 跑一个触发器二进制。返回 `{ fire, reason, data, error }` —— error 非空 = 这次跑挂了。 */
export const runTriggerBinary = ({ binary, params = {}, now, stateDir, timeout = 20_000 }) =>
  new Promise((resolve) => {
    const child = spawn(binary, [], {
      env: {
        ...process.env,
        // 状态统一落引擎的数据目录：触发器在哪台机器、被谁叫，写的位置都一致。
        NOUS_STATE_DIR: stateDir,
      },
      stdio: ["pipe", "pipe", "pipe"],
    });
    let out = "";
    let err = "";
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      resolve({ fire: false, reason: "", data: {}, error: "超时（20 秒没跑完）" });
    }, timeout);
    child.stdout.on("data", (chunk) => (out += chunk));
    child.stderr.on("data", (chunk) => (err += chunk));
    child.on("error", (error) => {
      clearTimeout(timer);
      resolve({ fire: false, reason: "", data: {}, error: `起不来：${error.message}` });
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      if (code !== 0) {
        // 非 0 = 出错，跟「不 fire」不是一回事 —— 这是契约里最要紧的一条。
        resolve({ fire: false, reason: "", data: {}, error: `退出码 ${code}：${(err || out).trim().slice(0, 300)}` });
        return;
      }
      try {
        const parsed = JSON.parse(out.trim());
        resolve({
          fire: parsed.fire === true,
          reason: String(parsed.reason ?? ""),
          data: parsed.data ?? {},
          error: "",
        });
      } catch {
        resolve({ fire: false, reason: "", data: {}, error: `stdout 不是 JSON：${out.trim().slice(0, 300)}` });
      }
    });
    // 程序可能没读 stdin 就退出了（比如一上来就 exit 2）—— 那时写 stdin 会 EPIPE，
    // 它是「程序已经不在了」的另一种说法，不是我们这边的错。
    child.stdin.on("error", () => {});
    child.stdin.end(`${JSON.stringify({ now, params })}\n`);
  });

/**
 * 造一个调度器。所有「干活」的动作都是注入的，这一层只掌握**节奏**：
 *   listCombos —— 现在有哪些开着、要跑的终极模板（[] 或 null 也行）
 *   runTrigger —— 跑一个触发器，回 { fire, error }
 *   launch     —— fire 了，去起会话
 *   onError    —— 出错了报出来（记日志 / 上页面，调用方定）
 *
 * 两条纪律：
 *   · 上一拍没跑完，这一拍**跳过**（setInterval 不会重入 —— 慢触发器会叠起来）。
 *   · fire 的裁决只信触发器，这里不二次判断「该不该跑」。
 */
export const createScheduler = ({ listCombos, runTrigger, launch, onError, tickMs = 60_000 }) => {
  let timer = null;
  let running = false;
  let lastTickAt = null;
  let lastError = "";

  const tick = async () => {
    if (running) return;
    running = true;
    try {
      const combos = (await listCombos()) ?? [];
      for (const combo of combos) {
        try {
          const result = await runTrigger(combo);
          if (result.fire) {
            await launch(combo, result);
          } else if (result.error) {
            lastError = result.error;
            onError?.(combo, result.error);
          }
        } catch (error) {
          lastError = String(error?.message ?? error);
          onError?.(combo, lastError);
        }
      }
      lastTickAt = Date.now();
    } catch (error) {
      // listCombos 自己抛了（名册读不动之类）：记下来，别让这一拍把调度器炸停。
      lastError = String(error?.message ?? error);
      onError?.(null, lastError);
    } finally {
      running = false;
    }
  };

  const start = () => {
    if (timer) return;
    timer = setInterval(() => void tick(), tickMs);
  };

  const stop = () => {
    if (timer) clearInterval(timer);
    timer = null;
  };

  return {
    start,
    stop,
    tick,
    get lastTickAt() {
      return lastTickAt;
    },
    get lastError() {
      return lastError;
    },
  };
};

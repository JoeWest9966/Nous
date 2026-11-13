// `node --test test/` 在这台机器的 Node（v24.10.0）上不展开目录参数 ——
// 它把 `test/` 当模块路径直接解析，没有落点就是 "Cannot find module"。
// 所以 test/ 需要一个入口把真正的测试引进来（qrd3wh/test/index.js 同一个约定）。
// 别在这里写断言：这个文件只是名单。
//
// 两个 CDP 冒烟是「import 时就开跑」的顶层脚本，还用 process.exit 报成败 ——
// 直接 import 会并行抢同一个 chromium 页面互踩、exit(1) 会把 runner 一起带走。
// 所以这里当子进程串行跑：输出原样透传，退出码非 0 就是这条 test 红。
import test from "node:test";
import { spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import "./archive-bff.test.js";

const HERE = path.dirname(fileURLToPath(import.meta.url));

const smoke = (file, env = {}) =>
  new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [path.join(HERE, file)], {
      stdio: "inherit",
      env: { ...process.env, ...env },
    });
    child.on("exit", (code) => (code === 0 ? resolve() : reject(new Error(`${file} 退出码 ${code}`))));
    child.on("error", reject);
  });

// 归档冒烟要打到**这份 worktree 的代码**，不能指望宿主机恰好也在这个分支上：
// 起一个本地 BFF（PORT=0），只借宿主的引擎地址。BASE 显式给了就照旧用宿主。
const runArchiveSmoke = async () => {
  if (process.env.BASE) return smoke("smoke-archive.mjs");

  const data = mkdtempSync(path.join(tmpdir(), "bjes6b-smoke-"));
  const server = spawn(process.execPath, [path.join(HERE, "..", "server.js")], {
    env: {
      ...process.env,
      PORT: "0",
      NOUS_DATA: data,
      NOUS_LOCAL_ORIGIN: process.env.NOUS_LOCAL_ORIGIN ?? "http://127.0.0.1:4790",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  server.stderr.on("data", (d) => process.stderr.write(`[bjes6b smoke server] ${d}`));

  let base;
  try {
    base = await new Promise((resolve, reject) => {
      let buf = "";
      const timer = setTimeout(() => reject(new Error("smoke server 没报端口")), 10_000);
      server.stdout.on("data", (chunk) => {
        buf += chunk.toString();
        const m = buf.match(/听 (\d+) /);
        if (m) {
          clearTimeout(timer);
          resolve(`http://127.0.0.1:${m[1]}/`);
        }
      });
      server.on("exit", () => {
        clearTimeout(timer);
        reject(new Error(`smoke server 退了：${buf.slice(0, 200)}`));
      });
    });
    await smoke("smoke-archive.mjs", { BASE: base });
  } finally {
    server.kill("SIGKILL");
  }
};

test("smoke-archive（CDP 真点一遍）", { timeout: 180_000 }, runArchiveSmoke);
test("smoke-templates（CDP 真点一遍）", { timeout: 180_000 }, () => smoke("smoke-templates.mjs"));

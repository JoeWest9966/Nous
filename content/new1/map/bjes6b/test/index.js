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
import path from "node:path";
import { fileURLToPath } from "node:url";

import "./archive-bff.test.js";

const HERE = path.dirname(fileURLToPath(import.meta.url));

const smoke = (file) =>
  new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [path.join(HERE, file)], { stdio: "inherit" });
    child.on("exit", (code) => (code === 0 ? resolve() : reject(new Error(`${file} 退出码 ${code}`))));
    child.on("error", reject);
  });

test("smoke-archive（CDP 真点一遍）", { timeout: 180_000 }, () => smoke("smoke-archive.mjs"));
test("smoke-templates（CDP 真点一遍）", { timeout: 180_000 }, () => smoke("smoke-templates.mjs"));

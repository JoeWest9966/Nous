// `node --test test/` 在这台机器的 Node（v22.23.2 定制构建）上不展开目录参数 ——
// 它把 `test/` 当模块路径直接解析，没有落点就是 "Cannot find module"。
// 所以 test/ 需要一个入口把真正的测试引进来（跟票 A 的 Nous-skills 同一个约定）。
// 别在这里写断言：这个文件只是名单。
// 第一个 import：造扩展桩 + 注入 NOUS_* 环境，必须早于 skills-api.js 的顶层动态 import。
import "./bootstrap.js";
import "./repo-root.test.js";
import "./skills-api.test.js";
import "./skills-variants.test.js";
import "./catalog.test.js";
import "./prompts-api.test.js";
import "./presets.test.js";
import "./presets-api.test.js";
import "./upstream.test.js";
import "./activity.test.js";
import "./pages.test.js";
import "./unread.test.js";
import "./session-pins.test.js";
import "./session-pins-http.test.js";

import "./scheduler.test.js";
import "./archive.test.mjs";
import "./maintenance.test.js";
import "./combos.test.js";

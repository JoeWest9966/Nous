// `node --test test/` 在这台机器的 Node（v22.23.2 定制构建）上不展开目录参数 ——
// 它把 `test/` 当模块路径直接解析，没有落点就是 "Cannot find module"。
// 所以 test/ 需要一个入口把真正的测试引进来（跟票 A 的 Nous-skills 同一个约定）。
// 别在这里写断言：这个文件只是名单。
import "./repo-root.test.js";
import "./skills-api.test.js";
import "./catalog.test.js";
import "./prompts-api.test.js";

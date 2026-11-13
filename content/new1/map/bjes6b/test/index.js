// `node --test test/` 在这台机器的 Node（v24.10.0）上不展开目录参数 ——
// 它把 `test/` 当模块路径直接解析，没有落点就是 "Cannot find module"。
// 所以 test/ 需要一个入口把真正的测试引进来（qrd3wh/test/index.js 同一个约定）。
// 别在这里写断言：这个文件只是名单。
import "./smoke-templates.mjs";
import "./smoke-archive.mjs";
import "./archive-bff.test.js";

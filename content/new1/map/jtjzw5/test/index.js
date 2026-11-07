// barrel：让 `node --test test/` 这条字面命令也能跑（node 22 把目录参数解析成 index.js）。
// 别在这里写断言：这个文件只是名单。
import "./model.test.js";
import "./api.test.js";

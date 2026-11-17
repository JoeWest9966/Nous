//! 样例扩展 `nameplate`：名牌由**扩展在应答里自报**（扫内容后才知道名字）。
//!
//! 名字来自句柄 `skills` 的内容里每个 `*.md` 的文件主干：`tdd.md` → `skill:tdd`。
//! 它没有 section、没有 tool —— 只声明具名槽位。

use nous_ext_sdk::{run, Extension};

fn main() {
    run(Extension::new("nameplate").nameplates("skills", "skill:"));
}

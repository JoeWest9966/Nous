//! `nous-fixture [路径]`：读一份 `agent.sqlite`，把 `wire.md` 的 `GET /v1/sessions` 形状打到 stdout。
//!
//! **只干这一件事**：没有子命令、没有 flag、不写库（源库用 `mode=ro` 打开）。
//! 不给路径时读 `fixtures/agent.sqlite`；那是 `./scripts/verify.sh` 第 2 段的默认对象，
//! 也是第 3 段「冷启动不 panic」跑的那一次。
//!
//! 退出码：0 = 打完了；2 = 读不出来或写不出去（stderr 说明原因）。

use std::process::ExitCode;

use nous_durable::storage::import::ImportedStore;
use nous_durable::storage::sessions::render_sessions;

/// 没给参数时读的那份。相对仓库根 —— `verify.sh` 就是在那儿跑的。
const DEFAULT_FIXTURE: &str = "fixtures/agent.sqlite";

fn main() -> ExitCode {
    let path = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_FIXTURE.to_string());
    match run(&path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("nous-fixture: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    // 只读打开：fixture 是真实用户数据，永远不写回去。
    let store = ImportedStore::read(path)?;
    let sessions = render_sessions(&store)?;
    // `wire.md` 的 `GET /v1/sessions`：`{ items: [...] }`，新的在前。
    let body = serde_json::json!({ "items": sessions });
    println!("{}", serde_json::to_string_pretty(&body)?);
    Ok(())
}

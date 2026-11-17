//! `nous-server`：把 `/v1/*` 接到一个真监听端口上。
//!
//! 两个环境变量，别的都走默认：
//!
//! | 变量 | 默认 | 说明 |
//! |---|---|---|
//! | `NOUS_LISTEN` | `127.0.0.1:4789` | 监听地址。端口给 0 时由内核选，实际地址打到 stdout |
//! | `NOUS_DB` | `./nous.db` | 库路径（**本库自己写的 v1 库**） |
//! | `NOUS_ADOPT_LEGACY` | 不设 | `1` = 把一份**上游 v2 副本**领养成本库可写库（见下） |
//!
//! ## 启动行是给脚本抓的
//!
//! 绑定成功后 stdout **只打一行** `listening on <addr>`，然后 flush —— 端口写 0 时脚本要
//! 靠这一行知道真实端口。其余日志走 stderr。
//!
//! ## 为什么需要 `NOUS_ADOPT_LEGACY`
//!
//! 上游 v2 库在写模式下是**明确拒绝**的（`docs/storage.md` §12.3）：两边的版本号不可比，
//! 认门靠 `durable_schema.writer`。所以拿 `fixtures/agent.sqlite` 的副本起服务，默认会
//! 报错并提示这个开关。它只做两件事：补 `writer` 列（值 `nous`）、版本号记成 1 —— 表结构
//! 已逐字相同，不动任何行。**只对副本用**：跑完那份文件在本库眼里就是 v1。

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use nous_durable::storage::{adopt_legacy_copy, SqliteStore};
use nous_durable::StorageError;
use nous_server::{router, AppState};

const DEFAULT_ADDR: &str = "127.0.0.1:4789";
const DEFAULT_DB: &str = "./nous.db";

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("nous-server: {error}");
            ExitCode::from(2)
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let db = std::env::var("NOUS_DB").unwrap_or_else(|_| DEFAULT_DB.to_string());
    if std::env::var("NOUS_ADOPT_LEGACY").is_ok_and(|value| value == "1") && Path::new(&db).exists() {
        match adopt_legacy_copy(&db) {
            Ok(()) => eprintln!("nous-server: adopted legacy copy at {db}"),
            // 已经是可写库（writer='nous'）：开关重复打开是无害意图，不报错。
            Err(StorageError::WriterMismatch { .. }) => {}
            Err(error) => return Err(format!("adopt {db}: {error}").into()),
        }
    }

    let state = match AppState::durable(&db) {
        Ok(state) => state,
        Err(error) => {
            if is_upstream_v2(&db) {
                return Err(format!(
                    "{db} is an upstream v2 library; the writer refuses it by design \
                     (docs/storage.md §12.3).\n\
                     Work on a throwaway copy and adopt it explicitly:\n  \
                     cp <legacy.sqlite> /var/tmp/nous-e2e.db\n  \
                     NOUS_ADOPT_LEGACY=1 NOUS_DB=/var/tmp/nous-e2e.db nous-server"
                )
                .into());
            }
            return Err(error.to_string().into());
        }
    };

    // spec §3.4：监听地址是本进程自己的配置，不是「服务器地址发现」。生产配置里
    // 不许出现 `NOUS_SERVER_ADDR`（spec ch1 判据 11：发现唯一，走 resolve），所以
    // 这个变量叫 `NOUS_LISTEN`。测试与 verify.sh 冷启动段用它给临时端口。
    let address = std::env::var("NOUS_LISTEN").unwrap_or_else(|_| DEFAULT_ADDR.to_string());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    let actual = listener.local_addr()?;
    // 脚本抓这一行：先写行、再 flush，之后的 serve 才可能阻塞。
    println!("listening on {actual}");
    std::io::stdout().flush()?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}

/// 只读地看一眼：这是不是一份上游 v2 库（没有认门列、版本 2）。用来给出可操作的报错。
fn is_upstream_v2(db: &str) -> bool {
    let Ok(store) = SqliteStore::open_read_only(db) else {
        return false;
    };
    matches!(store.writer(), Ok(None)) && store.schema_version().ok() == Some(2)
}

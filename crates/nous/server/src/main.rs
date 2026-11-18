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
//! 报错并提示这个开关。它只做两件事：补 `writer` 列（值 `nous`）、版本号记成本库当前版本（2）—— 表结构
//! 已逐字相同，不动任何行。**只对副本用**：跑完那份文件在本库眼里就是 v1。

use std::io::Write;
use std::sync::Arc;
use std::path::Path;
use std::process::ExitCode;

use nous_durable::storage::{adopt_legacy_copy, SqliteStore};
use nous_durable::StorageError;
use nous_server::{router, AppState, DurableCommands, SessionObserver};

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

    let registry = nous_durable::session::ObserverRegistry::new();
    let session = match nous_durable::session::Session::open(&db) {
        Ok(session) => Arc::new(session),
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
    let commands = DurableCommands::with_session(session.clone(), registry);
    let state = AppState::durable_with_commands(
        nous_server::ImportedSource::open(&db)?,
        commands.clone(),
    );
    commands.attach(SessionObserver::new(state.clone()));

    // spec §3.4：监听地址是本进程自己的配置，不是「服务器地址发现」。生产配置里
    // 不许出现 `NOUS_SERVER_ADDR`（spec ch1 判据 11：发现唯一，走 resolve），所以
    // 这个变量叫 `NOUS_LISTEN`。测试与 verify.sh 冷启动段用它给临时端口。
    // 整机崩 / 重启留下的孤儿：启动扫一次，补一条 `abandoned`。幂等（连重启三次只多一条），
    // 进程熔断不走这里（那是 `extension_crash`）。**走 `state` 已经持有的那个写者** ——
    // 另开 `Session` 就是第二条写通道（`#38` 收尾，2026-11-18）。
    state.reclaim_orphans().await?;

    // 组装根：这里构造 provider / tool 两张注册表，交给 sessions 的轮次驱动器。
    // 只装配、不绑定监听；真轮次由命令层构造 TurnRequest 时调用 driver.run。
    // 它和写命令**共用同一个 `Session`**（`session.clone()`）—— 另开一条就是第二个写者。
    let assembled = nous_server::runtime::assemble(
        std::env::current_dir()?,
        &nous_server::runtime::ProcessEnvValues,
        session,
    )?;
    let state = state.with_runtime(Arc::new(assembled));
    // #48：documents 端点与 sessions 共用同一条 `Session` 写者（同一把锁、同一份广播），
    // 不另开第二个 Storage。
    let state = state.with_documents_backend(
        std::path::PathBuf::from(&db),
        commands.session().clone(),
    );

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

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

use axum::Router;
use nous_durable::storage::{adopt_legacy_copy, SqliteStore};
use nous_durable::StorageError;
use nous_server::{auth, refusal, router, AppState, DurableCommands, SessionObserver};
use nous_server::refusal::{decide_refusal, EXIT_ERROR, EXIT_REFUSED};
#[allow(unused_imports)]
use refusal::REFUSAL_REASONS;

const DEFAULT_ADDR: &str = "127.0.0.1:4789";
const DEFAULT_DB: &str = "./nous.db";

/// 启动门槛的统一出口：一行 `refuse: <slug>` 到 stderr，退出码走理由表。
/// #50 的 token 门槛、#51 的 faux 门槛、D12 的 cwd 门槛共用这一个函数。
fn refuse_startup(code: &str, detail: Option<&str>) -> ! {
    match decide_refusal(code) {
        Some(reason) => {
            eprintln!("{}", reason.line());
            if let Some(detail) = detail {
                eprintln!("nous-server: {detail}");
            }
            std::process::exit(reason.exit as i32);
        }
        None => {
            eprintln!("nous-server: unknown refusal code: {code}");
            std::process::exit(EXIT_ERROR as i32);
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(RunError::Refused(code, detail)) => {
            refuse_startup(&code, Some(&detail))
        }
        Err(RunError::Failed(error)) => {
            eprintln!("nous-server: {error}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

/// 运行错误分两类：拒绝启动（78，理由表）与一般失败（2）。
enum RunError {
    Refused(&'static str, String),
    Failed(Box<dyn std::error::Error>),
}

impl From<Box<dyn std::error::Error>> for RunError {
    fn from(error: Box<dyn std::error::Error>) -> Self {
        Self::Failed(error)
    }
}

impl From<String> for RunError {
    fn from(message: String) -> Self {
        Self::Failed(message.into())
    }
}

impl From<&str> for RunError {
    fn from(message: &str) -> Self {
        Self::Failed(message.to_string().into())
    }
}

impl From<std::io::Error> for RunError {
    fn from(error: std::io::Error) -> Self {
        Self::Failed(Box::new(error))
    }
}

impl From<std::net::AddrParseError> for RunError {
    fn from(error: std::net::AddrParseError) -> Self {
        Self::Failed(Box::new(error))
    }
}

macro_rules! run_error_from {
    ($($ty:ty),* $(,)?) => {
        $(impl From<$ty> for RunError {
            fn from(error: $ty) -> Self {
                Self::Failed(Box::new(error))
            }
        })*
    };
}

run_error_from!(
    axum::Error,
    nous_server::ServerError,
);

impl From<nous_durable::StorageError> for RunError {
    fn from(error: nous_durable::StorageError) -> Self {
        if error.is_schema_too_new() {
            return Self::Refused(
                "schema-too-new",
                format!("database schema is newer than this binary: {error}"),
            );
        }
        Self::Failed(error.into())
    }
}

// verify.sh 2b 读 EXIT_REFUSED；保证常量与本文件同一处定义。
const _: u8 = EXIT_REFUSED;

async fn run() -> Result<(), RunError> {
    // D13 运维动作：手写 args 分发，不引 clap。动作本身不启服务、不进 Session。
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!("{}", nous_server::version::version_line());
            return Ok(());
        }
        Some("--print-token") => {
            // `--print-token`：只打印 token 本身，不启服务、不碰库。
            let Some((token, _)) = auth::load_or_generate(&auth::default_token_path(), true)? else {
                return Err("generate=true 必有 token".into());
            };
            println!("{token}");
            return Ok(());
        }
        Some("--backup") => {
            let path = args.get(1).ok_or("--backup 需要 <path>")?;
            let db = std::env::var("NOUS_DB").unwrap_or_else(|_| DEFAULT_DB.to_string());
            nous_durable::ops::backup(&db, std::path::Path::new(path))?;
            return Ok(());
        }
        Some("--check") => {
            let path = args.get(1).ok_or("--check 需要 <path>")?;
            match nous_durable::ops::check(std::path::Path::new(path)) {
                Ok(schema) => {
                    println!("quick_check=ok");
                    println!("schema={schema}");
                    return Ok(());
                }
                Err(error) => {
                    eprintln!("nous-server: {error}");
                    return Err(RunError::Failed("quick_check 未通过".into()));
                }
            }
        }
        Some("--active-tasks") => {
            let db = std::env::var("NOUS_DB").unwrap_or_else(|_| DEFAULT_DB.to_string());
            println!("{}", nous_durable::ops::active_tasks(&db)?);
            return Ok(());
        }
        _ => {}
    }

    // D12：工作目录断言只在真启动服务时做；失败 = refuse: workdir-missing。
    if let Err(detail) = nous_server::workdir::assert_workdir() {
        refuse_startup("workdir-missing", Some(&detail));
    }

    let token_path = auth::default_token_path();
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
    // 写者与观察者必须共用同一张 `ObserverRegistry`：`Session::open` 会另造一张，
    // 于是 `commands.attach(...)` 挂到的是一张空名单，真实提交永远播不出来（#51 合并时发现）。
    let session = match nous_durable::Storage::open(&db) {
        Ok(storage) => Arc::new(nous_durable::session::Session::with_observers(
            storage,
            registry.clone(),
        )),
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
    let address = std::env::var("NOUS_LISTEN").unwrap_or_else(|_| DEFAULT_ADDR.to_string());
    let faux = nous_server::runtime::provider_kind(&nous_server::runtime::ProcessEnvValues) == "faux";

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

    // 认证：token 文件存在就启用，不存在时只有绑回环才放行（且不启用中间件，
    // 判据 4「无 token 照常可用」）。绑非回环必须有 token，否则拒绝启动。
    let loaded = auth::load_or_generate(&token_path, false)?;
    let socket: std::net::SocketAddr = address.parse()?;
    let loopback = socket.ip().is_loopback();

    // 门槛理由集中在**这一个**数组里：#51 的 faux 检查与 #50 的 token 检查共用
    // refuse_startup。不许再写第二个 if + refuse_startup —— 那会让两条理由的日志口径分叉。
    if faux && !loopback {
        refuse_startup(
            "faux-on-non-loopback",
            Some(&format!(
                "faux provider 只允许绑回环（127.0.0.1），当前监听 {address}"
            )),
        );
    }
    if loaded.is_none() && !loopback {
        refuse_startup(
            "no-token-on-non-loopback",
            Some(&format!(
                "NOUS_LISTEN={address} 绑非回环但没有可用 token；请先用 `nous-app --print-token` 生成/抄取 token（文件：{}）",
                token_path.display()
            )),
        );
    }

    let token = match loaded {
        Some((token, _)) => {
            if let Ok(false) = auth::has_secured_permissions(&token_path) {
                eprintln!(
                    "nous-server: warning: token 文件 {} 权限不是 0600（容器里常见，不拦启动）",
                    token_path.display()
                );
            }
            Some(token)
        }
        None => None, // 只有回环能走到这里
    };

    let listener = tokio::net::TcpListener::bind(&address).await?;
    let actual = listener.local_addr()?;
    // 脚本抓这一行：先写行、再 flush，之后的 serve 才可能阻塞。
    println!("listening on {actual}");
    std::io::stdout().flush()?;
    // 端口给 0 时实际地址只有绑定后才知道；`NOUS_ORIGIN` 显式给出则优先。
    let state = if std::env::var("NOUS_ORIGIN").is_ok_and(|value| !value.is_empty()) {
        state
    } else {
        state.with_origin(format!("http://{actual}"))
    };
    let app: Router = match token {
        Some(token) => router(state).layer(axum::middleware::from_fn(move |req, next| {
            auth::require_bearer(token.clone(), req, next)
        })),
        None => router(state),
    };
    axum::serve(listener, app).await?;
    Ok(())
}

/// 只读地看一眼：这是不是一份上游 v2 库（没有认门列、版本 2）。用来给出可操作的报错。
fn is_upstream_v2(db: &str) -> bool {
    let Ok(store) = SqliteStore::open_read_only(db) else {
        return false;
    };
    matches!(store.writer(), Ok(None)) && store.schema_version().ok() == Some(2)
}

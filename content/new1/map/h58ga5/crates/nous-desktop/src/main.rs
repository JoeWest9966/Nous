//! Nous-Desktop 入口。三栏布局、会话、输入、置顶都在 app.rs 里。
//
// 正式包不要控制台黑框：不加这句，Windows 会给进程配一个 console，
// GPUI 的 INFO 日志（字体/GPU/D3D feature level）就打在那个黑窗里。
// debug 构建保留控制台，方便本地看日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod live;
mod config;

use std::io::Write as _;
use std::sync::OnceLock;

use gpui_kit::*;

/// GPUI 的 executor 不是 tokio，而 nous-engine 的 HTTP 全在 tokio 上。
/// 所以起一个 tokio runtime，把 Handle 挂全局，UI 里要用 HTTP 就从这里 spawn。
pub static TOKIO: OnceLock<tokio::runtime::Handle> = OnceLock::new();

/// 日志：**落盘**，不再只怼控制台。
///
/// release 包是 GUI 子系统（`windows_subsystem = "windows"`），进程没有 console，
/// `eprintln!` 写进去等于丢进 null —— 真出「Failed to create D3D device」这种致命
/// 问题，界面上看不到、事后也没证据。所以统一写文件：
/// `%LOCALAPPDATA%\nous\desktop.log`（Linux 是 `~/.local/share/nous/`）。
///
/// 超过 2 MiB 就轮转一次（desktop.log → desktop.log.1）：留最近两次启动的日志，
/// 够查「刚才那次为什么没起来」。debug 构建照旧同时打控制台，本地开发看得见。
fn init_log() {
    struct FileLog {
        file: std::sync::Mutex<Option<std::fs::File>>,
        also_console: bool,
    }

    impl log::Log for FileLog {
        fn enabled(&self, meta: &log::Metadata) -> bool {
            meta.level() <= log::Level::Info
        }

        fn log(&self, record: &log::Record) {
            if !self.enabled(record.metadata()) {
                return;
            }
            let line = format!("[{}] {} ({})\n", record.level(), record.args(), record.target());
            if let Ok(mut slot) = self.file.lock()
                && let Some(file) = slot.as_mut()
            {
                let _ = file.write_all(line.as_bytes());
            }
            if self.also_console {
                let _ = std::io::stderr().write_all(line.as_bytes());
            }
        }

        fn flush(&self) {
            if let Ok(mut slot) = self.file.lock()
                && let Some(file) = slot.as_mut()
            {
                let _ = file.flush();
            }
        }
    }

    let _ = log::set_boxed_logger(Box::new(FileLog {
        file: std::sync::Mutex::new(open_log_file()),
        also_console: cfg!(debug_assertions),
    }));
    log::set_max_level(log::LevelFilter::Info);
}

/// 打开日志文件。拿不到目录、没权限 → None：日志丢了也不能拦住启动。
fn open_log_file() -> Option<std::fs::File> {
    const MAX_BYTES: u64 = 2 * 1024 * 1024;

    let dir = dirs::data_local_dir()?.join("nous");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("desktop.log");
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_BYTES {
        let _ = std::fs::rename(&path, dir.join("desktop.log.1"));
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()
}

fn main() {
    init_log();

    let rt = tokio::runtime::Runtime::new().expect("起 tokio");
    TOKIO.set(rt.handle().clone()).ok();
    // enter 让 main 线程也在 runtime 上下文里；gpui run 会阻塞在这里，rt 一直活着。
    let _guard = rt.enter();

    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        // 窗口：标题 Nous，1280x800 居中。桌面聊天应用的第一屏。
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1280.), px(800.)), cx)),
            titlebar: Some(TitlebarOptions {
                title: Some("Nous".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| app::Desktop::new(window, cx))
        })
        .expect("Failed to open window");
    });
}

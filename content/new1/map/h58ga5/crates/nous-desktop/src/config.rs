//! 配置：`~/.config/nous/desktop.toml`（或 `NOUS_DESKTOP_CONFIG` 指的文件）。
//!
//! 只有两个键。引擎地址**不**写这里 —— 它从宿主的 resolve 现拿。
//!
//! **文件不存在时不报错，用默认值。** 装好就能开，别让用户先去手写 toml。
//! （默认值是「这台机器上的 Nous」，换机器就改那一份文件。）

use std::path::PathBuf;

/// 没有配置文件时用的默认宿主。这是**宿主**地址，不是引擎地址。
///
/// 默认指向本机 —— 对任何跑着 Nous 的机器都成立。走 tailnet / 反代 / 域名的
/// 写进 `desktop.toml`，别改这里（这里是所有人共用的默认值）。
pub const DEFAULT_HOST: &str = "http://127.0.0.1:4790";

/// 盘名。你自己那盘的 id，换盘就改。
pub const DEFAULT_BOARD: &str = "new1";

/// 自己的块 id 和引擎块 id：`resolve` 的路径里要用。
/// 换一块部署就改这两个（它们是这一份安装的坐标，不是协议的一部分）。
pub const SELF_BLOCK: &str = "h58ga5";
pub const ENGINE_BLOCK: &str = "qrd3wh";

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Config {
    /// 宿主地址，可带路径前缀（如 `https://…/nous`）。
    pub host: String,
    /// 盘名。
    pub board: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            board: DEFAULT_BOARD.to_string(),
        }
    }
}

impl Config {
    /// 读配置。一个都不存在 → 用默认值（不是错误）。存在但写坏了 → 报错，别静默用默认。
    ///
    /// 查找顺序（先到先得）：
    /// 1. `NOUS_DESKTOP_CONFIG` —— 调试用
    /// 2. **exe 旁边**的 `desktop.toml` —— 便携式。发布包里就带一份，解压即用
    /// 3. `%APPDATA%\nous\desktop.toml`（Linux 是 `~/.config`）—— 装机后的个人配置
    pub fn load() -> Result<Self, String> {
        let Some(path) = Self::path() else {
            return Ok(Self::default());
        };

        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("读不了 {}：{e}", path.display())),
        };

        let cfg: Config =
            toml::from_str(&text).map_err(|e| format!("{} 不是合法 toml：{e}", path.display()))?;
        if cfg.host.trim().is_empty() {
            return Err(format!("{} 里的 host 是空的", path.display()));
        }
        if cfg.board.trim().is_empty() {
            return Err(format!("{} 里的 board 是空的", path.display()));
        }
        Ok(cfg)
    }

    /// 实际用的那一个配置文件（不存在就是 None，会用默认值）。
    pub fn path() -> Option<PathBuf> {
        Self::candidates().into_iter().find(|p| p.is_file())
    }

    /// 所有候选路径，按优先级。exe 旁边那份排在用户配置之前 —— 便携包要能覆盖掉旧配置。
    pub fn candidates() -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(explicit) = std::env::var("NOUS_DESKTOP_CONFIG") {
            out.push(PathBuf::from(explicit));
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            out.push(dir.join("desktop.toml"));
        }
        if let Some(config) = dirs::config_dir() {
            out.push(config.join("nous").join("desktop.toml"));
        }
        out
    }
}

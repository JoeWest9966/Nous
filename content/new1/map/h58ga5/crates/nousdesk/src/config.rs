//! 配置：`~/.config/nousdesk/desktop.toml`（或 `NOUS_DESKTOP_CONFIG` 指的文件）。
//!
//! 只有两个键。引擎地址**不**写这里 —— 它从宿主的 resolve 现拿。
//!
//! **文件不存在时不报错，用默认值。** 装好就能开，别让用户先去手写 toml。
//! （默认值是「这台机器上的 Nous」，换机器就改那一份文件。）

use std::path::PathBuf;

/// 用户配置 / 日志的目录名（`%APPDATA%\nousdesk\`、`%LOCALAPPDATA%\nousdesk\`）。
pub const APP_DIR: &str = "nousdesk";

/// 改名前的目录名（`nous`）。**只读不写**：老用户的 `desktop.toml` 还得能读到。
pub const LEGACY_APP_DIR: &str = "nous";

/// 没有配置文件时用的默认宿主。这是**宿主**地址，不是引擎地址。
///
/// 默认指向本机 —— 对任何跑着 Nous 的机器都成立。走 tailnet / 反代 / 域名的
/// 写进 `desktop.toml`，别改这里（这里是所有人共用的默认值）。
pub const DEFAULT_HOST: &str = "http://192.168.1.7:4790";

/// 盘名。你自己那盘的 id，换盘就改。
pub const DEFAULT_BOARD: &str = "new1";

/// 自己的块 id 和引擎块 id：`resolve` 的路径里要用。
/// 换一块部署就改这两个（它们是这一份安装的坐标，不是协议的一部分）。
pub const SELF_BLOCK: &str = "h58ga5";
pub const ENGINE_BLOCK: &str = "qrd3wh";
/// 手机端那块。**只用来读写置顶**（`pins.json` 的单源在它那儿，见 `decisions.md`）。
pub const MOBILE_BLOCK: &str = "bjes6b";

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
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
    /// 3. `%APPDATA%\nousdesk\desktop.toml`（Linux 是 `~/.config`）—— 装机后的个人配置
    ///    4. `%APPDATA%\nous\desktop.toml` —— 改名前的旧目录，只用来接住老用户
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

    /// 保存会写到哪个文件：**当前生效的那份**；一份都没有就新建用户配置那一级。
    ///
    /// 写「生效的那份」而不是永远写用户配置：便携包的 `desktop.toml` 排在用户配置之前，
    /// 写死 `%APPDATA%` 的话用户改完保存会发现毫无变化。
    pub fn save_target() -> Result<PathBuf, String> {
        if let Some(path) = Self::path() {
            return Ok(path);
        }
        dirs::config_dir()
            .map(|dir| dir.join(APP_DIR).join("desktop.toml"))
            .ok_or_else(|| "找不到配置目录（%APPDATA%）".to_string())
    }

    /// 把这两个键写回去。**不会**顺手拉别的键进来 —— 这个文件就只有这两行。
    pub fn save(&self) -> Result<PathBuf, String> {
        let path = Self::save_target()?;
        self.save_to(&path)?;
        Ok(path)
    }

    /// 写到指定文件（父目录不存在就建）。分成两步是为了能直接测，不用动环境变量。
    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("建不了目录 {}：{e}", dir.display()))?;
        }
        let text = toml::to_string(self).map_err(|e| format!("写不出来：{e}"))?;
        std::fs::write(path, text).map_err(|e| format!("写不了 {}：{e}", path.display()))
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
            out.push(config.join(APP_DIR).join("desktop.toml"));
            // 改名前的目录：排在后面，新的那份一存在就盖过它。
            out.push(config.join(LEGACY_APP_DIR).join("desktop.toml"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 写回去的文件必须**只有**这两行 —— 多出来的键会让「配置文件就两个键」这句话变成假的。
    #[test]
    fn save_writes_only_host_and_board() {
        let dir = std::env::temp_dir().join(format!("nousdesk-cfg-{}", std::process::id()));
        let path = dir.join("nested").join("desktop.toml");
        let cfg = Config {
            host: "http://example.test:4790".into(),
            board: "probe".into(),
        };

        cfg.save_to(&path).expect("写得进去（父目录会自己建）");
        let text = std::fs::read_to_string(&path).expect("读得回来");

        assert!(
            text.contains("host = \"http://example.test:4790\""),
            "host 没写对：{text}"
        );
        assert!(text.contains("board = \"probe\""), "board 没写对：{text}");
        assert_eq!(
            text.lines().filter(|l| !l.trim().is_empty()).count(),
            2,
            "不该有第三个键：{text}"
        );

        let back: Config = toml::from_str(&text).expect("还是合法的 toml");
        assert_eq!(back.host, cfg.host);
        assert_eq!(back.board, cfg.board);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! 供应商密钥的写入面：元素库只存引用（`$NAME`），值落到宿主的一份环境文件里。
//!
//! 为什么是文件而不是元素库：**元素库照旧只存引用**，值不进 git、不进投影、不进日志。
//! 引擎读这份文件的方式就是 `EnvironmentFile=`（见 `deploy/nous-app.service`），
//! 所以写进去的每一行都是 `NAME=value`。
//!
//! 三条不越过的线：
//!
//! - **只改得起环境变量引用**：`$NAME` 能写；`!cmd` / `secret:<id>` / 明文是别人家的东西，
//!   直说改不了（409），不偷偷往别处塞。
//! - **写文件是原子的**：同目录临时文件 + `rename`，中途断电不会留下半份密钥文件；
//!   目录 `0700`、文件 `0600`。
//! - **回执不含值**：响应只说写了哪个变量、写到哪、要不要重启。

use std::fs;
use std::path::{Path, PathBuf};

use nous_elements::ElementItem;

use crate::{parse_key_ref, KeyRef};
use crate::GROUP_ID;

/// 写成功后的回执。**不含 value**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    /// 供应商键（`wb2api`）。
    pub provider: String,
    /// 供应商包 id（`provider-wb2api`）。
    pub package: String,
    /// 环境变量名（`WB2API_API_KEY`）。
    pub env: String,
    /// 写到哪个文件。
    pub path: PathBuf,
    /// 这个变量本来就在文件里（`true`）= 覆盖，不是新增。
    pub replaced: bool,
}

/// 拒绝写的原因。每种都有自己的状态码，别混成一个 500。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// 组里没有这个供应商。
    UnknownProvider(String),
    /// 组里没有这个供应商包（引用了但盘上没有）。
    UnknownPackage(String),
    /// 这条密钥不是环境变量引用，Desk 改不了。
    NotEnv { provider: String, shape: &'static str },
    /// 元素包里根本没写 `apiKey`。
    NoKey(String),
    /// 元素包里写了明文（元素库不收明文）。
    Plaintext(String),
    /// 请求本身不合法（空值、换行、变量名不合法）。
    Bad(String),
    /// 宿主没开这个能力 / 密钥文件不可写。
    Unavailable(String),
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Self::UnknownProvider(provider) => {
                format!("组里没有 {provider} 这个供应商")
            }
            Self::UnknownPackage(package) => {
                format!("{GROUP_ID}/{package} 这个包不在元素库里")
            }
            Self::NotEnv { provider, shape } => format!(
                "{provider} 的密钥是 {shape} 引用，Desk 改不了 —— 改元素包，或改那台机器上的{}",
                match *shape {
                    "cmd" => "命令",
                    "secret" => "密钥库",
                    _ => "来源",
                }
            ),
            Self::NoKey(package) => format!(
                "{package} 里没写 apiKey —— 先给它加一条形如 \"$WB2API_API_KEY\" 的引用，再来这里填值"
            ),
            Self::Plaintext(package) => format!(
                "{package} 的 apiKey 写的是明文 —— 元素库不收明文，先改成 \"$WB2API_API_KEY\" 再填值"
            ),
            Self::Bad(detail) => detail.clone(),
            Self::Unavailable(detail) => detail.clone(),
        }
    }
}

/// 供应商键 → 包里那条 `apiKey` 引用 + 包 id。没引用到的包不参与（组是唯一的门）。
pub fn binding(group: &ElementItem, packages: &[ElementItem], provider: &str) -> Option<(String, KeyRef)> {
    for id in crate::referenced(group) {
        let Some(item) = packages.iter().find(|item| item.id == id) else {
            continue;
        };
        if !crate::is_provider(item) {
            continue;
        }
        if crate::provider_key_of(item) != provider {
            continue;
        }
        return Some((
            item.id.clone(),
            parse_key_ref(item.fields.get("apiKey").and_then(|value| value.as_str())),
        ));
    }
    None
}

/// 把一条密钥写进 `path` 那份环境文件。
///
/// 只接受 `$NAME` 引用；值不得为空、不得含换行；其余行（注释、别的变量）原样保留。
pub fn store(
    group: &ElementItem,
    packages: &[ElementItem],
    provider: &str,
    value: &str,
    path: &Path,
) -> Result<Stored, Refusal> {
    let value = value.trim();
    if value.is_empty() {
        return Err(Refusal::Bad("密钥是空的 —— 填个值再存".into()));
    }
    if value.contains('\n') || value.contains('\r') {
        return Err(Refusal::Bad("密钥里不能有换行".into()));
    }

    let (package, key) = match binding(group, packages, provider) {
        Some(found) => found,
        // 组里引用了这个供应商的包，但包本身在盘上找不到：说清楚是包缺了。
        None => {
            let missing = crate::referenced(group)
                .into_iter()
                .find(|id| id == &format!("{}{}", crate::PROVIDER_PREFIX, provider))
                .or_else(|| {
                    crate::referenced(group)
                        .into_iter()
                        .find(|id| id.contains(provider))
                });
            return Err(match missing {
                Some(id) if !packages.iter().any(|item| item.id == id) => {
                    Refusal::UnknownPackage(id)
                }
                _ => Refusal::UnknownProvider(provider.to_string()),
            });
        }
    };

    let env = match key {
        KeyRef::Env(name) => name,
        KeyRef::Cmd(_) => {
            return Err(Refusal::NotEnv { provider: provider.to_string(), shape: "cmd" })
        }
        KeyRef::Secret(_) => {
            return Err(Refusal::NotEnv { provider: provider.to_string(), shape: "secret" })
        }
        KeyRef::Missing => return Err(Refusal::NoKey(package)),
        KeyRef::Plaintext => return Err(Refusal::Plaintext(package)),
    };
    if !valid_env_name(&env) {
        return Err(Refusal::Bad(format!(
            "包里的引用名 {env} 不是合法的环境变量名（字母数字下划线，不能以数字开头）"
        )));
    }

    let replaced = write_env(path, &env, value).map_err(Refusal::Unavailable)?;
    Ok(Stored { provider: provider.to_string(), package, env, path: path.to_path_buf(), replaced })
}

/// 环境变量名：`[A-Za-z_][A-Za-z0-9_]*`。
pub fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 往环境文件里写一个变量，返回「原来就有」。原子替换 + `0600`。
fn write_env(path: &Path, name: &str, value: &str) -> Result<bool, String> {
    let existing = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("读不了密钥文件 {}：{error}", path.display())),
    };

    let mut replaced = false;
    let mut lines: Vec<String> = Vec::new();
    for line in existing.lines() {
        if key_of(line) == Some(name) {
            // 同一变量出现多次时收敛成一行，避免 systemd 取到最后一行是个旧值。
            if !replaced {
                lines.push(env_line(name, value));
                replaced = true;
            }
            continue;
        }
        lines.push(line.to_string());
    }
    if !replaced {
        lines.push(env_line(name, value));
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("建不了密钥目录 {}：{error}", parent.display()))?;
            restrict_dir(parent);
        }
    }

    let mut body = lines.join("\n");
    body.push('\n');
    let tmp = tmp_path(path);
    fs::write(&tmp, body).map_err(|error| format!("写不了临时文件 {}：{error}", tmp.display()))?;
    restrict_file(&tmp);
    fs::rename(&tmp, path)
        .map_err(|error| format!("替换不了密钥文件 {}：{error}", path.display()))?;
    restrict_file(path);
    Ok(replaced)
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|name| name.to_os_string()).unwrap_or_default();
    name.push(format!(".tmp-{}", std::process::id()));
    path.with_file_name(name)
}

/// 一行 `NAME=value`。含空白 / 引号 / `#` 时按 systemd 的规矩加引号。
fn env_line(name: &str, value: &str) -> String {
    let needs_quote = value.is_empty()
        || value.chars().any(|c| c.is_whitespace())
        || value.starts_with(|c: char| c == '"' || c == '\'' || c == '#');
    if !needs_quote {
        return format!("{name}={value}");
    }
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("{name}=\"{escaped}\"")
}

/// 取一行里的变量名（`#` 开头的注释不算）。
fn key_of(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return None;
    }
    let (key, _) = trimmed.split_once('=')?;
    let key = key.trim();
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

#[cfg(unix)]
fn restrict_file(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) {}

#[cfg(unix)]
fn restrict_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_dir(_path: &Path) {}

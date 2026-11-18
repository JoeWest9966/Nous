//! `/v1/*` 的 Bearer token 认证与启动门槛。
//!
//! 一个 token 一把钥匙：文件在 `~/.config/nous/token`（0600），首次启动生成 32 字节
//! base64url。绑非回环地址时没有可用 token 就拒绝启动；绑回环时没有 token 也照常起，
//! 但只要 token 文件存在，`/v1/*` 全量保护（含 SSE 与 capabilities，零豁免）。
//!
//! 这里只加认证，不改任何路由形状。日志只打 token 文件路径，不打内容。

use std::fs;
use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

use axum::extract::Request;
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

/// 默认 token 文件路径：`~/.config/nous/token`。
pub fn default_token_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".config/nous/token")
}

/// token 文件的权限是否为 0600（只判权限位，不判文件是否在）。
pub fn has_secured_permissions(path: &std::path::Path) -> io::Result<bool> {
    Ok(path.metadata()?.permissions().mode() & 0o777 == 0o600)
}

/// 启动时装载 token：有文件就读取（trim 掉尾部空白），没有就按 `generate` 决定是否创建。
///
/// `generate=true` 时创建父目录、写 0600、返回 (token, 是否新建)。
/// `generate=false` 时只读；没有文件返回 Ok(None)。
pub fn load_or_generate(
    path: &std::path::Path,
    generate: bool,
) -> Result<Option<(String, bool)>, Box<dyn std::error::Error>> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let token = text.trim().to_string();
            if token.is_empty() {
                if generate {
                    eprintln!("nous-server: token 文件为空，重新生成：{}", path.display());
                    let generated = generate_file(path)?;
                    Ok(Some((generated, true)))
                } else {
                    Ok(None)
                }
            } else {
                Ok(Some((token, false)))
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if generate {
                let generated = generate_file(path)?;
                Ok(Some((generated, true)))
            } else {
                Ok(None)
            }
        }
        Err(error) => Err(format!("读取 token 文件 {}: {error}", path.display()).into()),
    }
}

fn generate_file(path: &std::path::Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut bytes = [0u8; 32];
    // /dev/urandom 是生成密钥的独立熵源；token 不参与任何协议版本。
    use std::io::Read;
    let mut urandom = fs::File::open("/dev/urandom")?;
    urandom.read_exact(&mut bytes)?;
    let token = URL_SAFE_NO_PAD.encode(bytes);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true).mode(0o600);
    let mut file = options.open(path)?;
    use std::io::Write;
    file.write_all(token.as_bytes())?;
    file.flush()?;
    // 已存在但权限不对的文件，mode() 只在创建时生效：显式 chmod 保证 0600。
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(token)
}

/// 常量时间比较：逐字节异或累计，不看长度是否相等就提前返回。
/// 长度差也走相同轮数（用 0 补短的那侧），避免 token 长度泄漏。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let max = a.len().max(b.len());
    let length_diff = a.len() ^ b.len();
    let mut diff = 0u8;
    for i in 0..max {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    // 长度差折叠成字节（逐个丢弃高位），不能 (len^len) as u8 —— 256 的倍数会被截断成 0。
    for shift in [0, 8, 16, 24, 32, 40, 48, 56] {
        diff |= ((length_diff >> shift) & 0xff) as u8;
    }
    diff == 0
}

/// 从 `Authorization` 头里取 Bearer token。只看 `Bearer ` 前缀；大小写不敏感。
fn bearer_value(header: Option<&str>) -> Option<&str> {
    let header = header?;
    let (scheme, value) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(value)
}

/// `/v1/*` 全量 Bearer 中间件：缺头 / 错 token / 空 token 一律 401 + `WWW-Authenticate: Bearer`。
pub async fn require_bearer(
    expected: String,
    request: Request,
    next: Next,
) -> Response {
    let supplied = bearer_value(request.headers().get(AUTHORIZATION).and_then(|v| v.to_str().ok()));
    let matched = supplied.is_some_and(|value| {
        constant_time_eq(value.as_bytes(), expected.as_bytes())
    });
    if !matched {
        return (
            StatusCode::UNAUTHORIZED,
            [(WWW_AUTHENTICATE, "Bearer")],
            axum::Json(serde_json::json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_accepts_identical_and_rejects_different_or_wrong_length() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
        // 长度差是 256 的倍数时不能因 u8 截断被误判相等。
        let token = b"abcdefghijklmnopqrstuvwxyz0123456789ab"; // 43 字节
        let mut longer = token.to_vec();
        longer.extend(std::iter::repeat_n(b'x', 256));
        assert!(!constant_time_eq(token, &longer));
    }

    #[test]
    fn bearer_value_rejects_empty_and_non_bearer_but_accepts_case_insensitive() {
        assert_eq!(bearer_value(Some("Bearer abc")), Some("abc"));
        assert_eq!(bearer_value(Some("bearer abc")), Some("abc"));
        assert_eq!(bearer_value(Some("Bearer ")), None);
        assert_eq!(bearer_value(Some("Basic abc")), None);
        assert_eq!(bearer_value(None), None);
    }

    #[test]
    fn generated_token_is_32_bytes_base64url_with_0600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("token");
        let (token, fresh) = load_or_generate(&path, true).unwrap().unwrap();
        assert!(fresh);
        assert_eq!(token.len(), 43); // 32 字节 = 43 个 base64url 字符（无 padding）
        assert!(token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert!(has_secured_permissions(&path).unwrap());
        // 第二次装载不改内容
        let (again, fresh2) = load_or_generate(&path, false).unwrap().unwrap();
        assert_eq!(again, token);
        assert!(!fresh2);
    }

    #[test]
    fn readonly_load_of_missing_token_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_or_generate(&dir.path().join("missing"), false).unwrap().is_none());
    }
}

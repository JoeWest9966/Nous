//! `GET /v1/fs/dirs?machine=` 的远端分支（#78）。
//!
//! 只列子目录（不跟随 symlink），沿用 #68 的 `BrowseDirs` 形状与 500 上限；
//! 不存在 / 权限不足 / 路径不是目录分别映射，连不上是 `Unreachable`（HTTP 502）。

use nous_env::{Context, FileErrorCode, FileSystem, FileKind};

use crate::config::{machine, MachineError};
use crate::env::SshExecutionEnv;

/// 远端目录浏览的失败分类，交给 server 映射 HTTP 状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteDirsError {
    /// 编目里没有这台机器 → 404（不是 501/500）。
    MachineNotFound,
    /// 连不上（SSH 握手 / 10s 超时）→ 502，带可读错误。
    Unreachable(String),
    /// 相对路径 / `~` → 400。
    RelativePath,
    NotFound,
    PermissionDenied,
    NotDirectory,
    Other,
}

/// 按机器名浏览一个远端绝对目录。`path` 为空 = 远端 `cwd`（通常 `~`）。
pub async fn remote_subdirs(
    machine_name: &str,
    path: &str,
    include_hidden: bool,
    limit: usize,
) -> Result<nous_env::BrowseDirs, RemoteDirsError> {
    if !path.is_empty() && !path.starts_with('/') {
        return Err(RemoteDirsError::RelativePath);
    }
    let config = machine(machine_name).map_err(|error| match error {
        MachineError::Unknown => RemoteDirsError::MachineNotFound,
        MachineError::Invalid => RemoteDirsError::Other,
    })?;

    // 密钥内容只从环境读，不进机器编目/日志/库；这里只在内存里过一手。
    let pem = std::env::var("NOUS_SSH_KEY").ok().filter(|text| !text.trim().is_empty());
    let env = SshExecutionEnv::new(config, pem.as_deref())
        .map_err(|error| RemoteDirsError::Unreachable(format!("ssh 环境不可用：{error}")))?;
    let address = if path.is_empty() { env.cwd().to_string_lossy().into_owned() } else { path.to_string() };
    let context = Context::new();

    // `realpath` 先做远端 cwd 校验：不存在/不是目录的错误分类与 ls 一致。
    let canonical = env.canonical_path(&address, &context).await.map_err(map_file_error)?;
    let entries = env.list_dir(&canonical, &context).await.map_err(map_file_error)?;
    let mut dirs = Vec::new();
    let mut truncated = false;
    // 远端返回项按名字排序，保持与 #68 本地浏览一致的稳定顺序。
    let mut entries = entries;
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in entries {
        if entry.kind != FileKind::Directory {
            continue;
        }
        let hidden = entry.name.starts_with('.');
        if hidden && !include_hidden {
            continue;
        }
        if dirs.len() == limit {
            truncated = true;
            continue;
        }
        dirs.push(nous_env::BrowseDir {
            name: entry.name,
            path: entry.path,
            hidden,
        });
    }
    Ok(nous_env::BrowseDirs { path: canonical, dirs, truncated })
}

fn map_file_error(error: nous_env::FileError) -> RemoteDirsError {
    match error.code {
        FileErrorCode::NotFound => RemoteDirsError::NotFound,
        FileErrorCode::PermissionDenied => RemoteDirsError::PermissionDenied,
        FileErrorCode::NotDirectory | FileErrorCode::IsDirectory => RemoteDirsError::NotDirectory,
        FileErrorCode::Aborted => RemoteDirsError::Unreachable("操作被取消".to_string()),
        _ => {
            // ssh 连不上的特征：255 / timed out / 启动失败。不要把它说成目录不存在。
            let text = error.message.to_ascii_lowercase();
            if text.contains("timed out") || text.contains("连不上") || text.contains("255") {
                RemoteDirsError::Unreachable(error.message)
            } else {
                RemoteDirsError::Other
            }
        }
    }
}

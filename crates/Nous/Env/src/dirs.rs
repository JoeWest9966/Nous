//! `GET /v1/fs/dirs` 的只读目录浏览能力（#68）。
//!
//! 环境变量只在这里读一次：缺省起点取 `NOUS_START_DIRS` 的第一段，没有就取进程 cwd。
//! `~` 不在这里展开 —— 客户端负责展开，引擎只认绝对路径。
//!
//! 本地后端只列目录项，用 `std::fs::read_dir` + `DirEntry::file_type`：符号链接不算
//! 目录（与 `FileSystem::list_dir` 的 `lstat` 语义一致）。失败按稳定分类返回，调用方
//! 把 `FileErrorCode` 映射成 HTTP 状态，不让 IO 错误变成 500。

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{io_to_file_code, FileError, FileErrorCode};

/// 一条子目录。字段名就是 wire 契约的形状。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowseDir {
    pub name: String,
    pub path: String,
    pub hidden: bool,
}

/// 一次目录浏览的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowseDirs {
    pub path: String,
    pub dirs: Vec<BrowseDir>,
    pub truncated: bool,
}

/// `NOUS_START_DIRS`：冒号分隔，取第一条非空项。没有就用 cwd。
///
/// 相对路径也原样返回：校验交给调用方，免得两处各判一次。
pub fn start_dir() -> PathBuf {
    std::env::var("NOUS_START_DIRS")
        .ok()
        .and_then(|value| {
            value
                .split(':')
                .map(str::trim)
                .find(|part| !part.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")))
}

/// 本机名：`HOSTNAME` 优先，回退 `/etc/hostname`。两边都不读就是「不知道自己叫什么」，
/// 此时任何显式 `machine` 都不认作本机（宁可 501，不许静默退回本机目录）。
pub fn local_machine_name() -> Option<String> {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|text| text.trim().to_string())
                .filter(|value| !value.is_empty())
        })
}

/// `machine` 是本机名：空 / 省略 / 等于本机名就是本机。
pub fn is_local_machine(machine: Option<&str>) -> bool {
    match machine {
        None => true,
        Some("") => true,
        Some(name) => local_machine_name().as_deref() == Some(name),
    }
}

/// 目录浏览的失败原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseError {
    /// 相对路径（或 `~` 开头）→ 400。
    RelativePath,
    NotFound,
    PermissionDenied,
    NotDirectory,
    Other,
}

impl From<&FileError> for BrowseError {
    fn from(error: &FileError) -> Self {
        match error.code {
            FileErrorCode::NotFound => Self::NotFound,
            FileErrorCode::PermissionDenied => Self::PermissionDenied,
            FileErrorCode::NotDirectory => Self::NotDirectory,
            FileErrorCode::IsDirectory => Self::NotDirectory,
            _ => Self::Other,
        }
    }
}

/// 只读浏览一个绝对目录。
///
/// - `path` 为空 = [`start_dir`]；
/// - 只列 `file_type().is_dir()` 的项，符号链接不算；
/// - 点开头的项默认跳过，`include_hidden` 时收录并标 `hidden: true`；
/// - 名字字节序排序（同名不可能，稳定即确定性）；
/// - 最多 `limit` 条，超过置 `truncated`。
pub fn list_subdirs(
    path: &str,
    include_hidden: bool,
    limit: usize,
) -> Result<BrowseDirs, BrowseError> {
    let addressed = if path.is_empty() {
        start_dir()
    } else {
        PathBuf::from(path)
    };
    // 不做 home 展开：`~`（无论来自参数还是 NOUS_START_DIRS）一律 400。
    if !addressed.is_absolute()
        || addressed.to_string_lossy().starts_with('~')
    {
        return Err(BrowseError::RelativePath);
    }

    let mut entries = std::fs::read_dir(&addressed)
        .map_err(|error| classify_io(&error, &addressed))?;
    let mut dirs = Vec::new();
    let mut truncated = false;
    while let Some(entry) = entries.next() {
        let entry = entry.map_err(|error| classify_io(&error, &addressed))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let hidden = name.starts_with('.');
        if hidden && !include_hidden {
            continue;
        }
        if !entry.file_type().map_err(|error| classify_io(&error, &addressed))?.is_dir() {
            continue;
        }
        if dirs.len() == limit {
            truncated = true;
            continue;
        }
        dirs.push(BrowseDir {
            name: name.clone(),
            path: addressed.join(&name).to_string_lossy().into_owned(),
            hidden,
        });
    }
    dirs.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(BrowseDirs {
        path: addressed.to_string_lossy().into_owned(),
        dirs,
        truncated,
    })
}

fn classify_io(error: &std::io::Error, path: &Path) -> BrowseError {
    let code = io_to_file_code(error);
    let file_error = FileError::new(code, error.to_string())
        .with_path(path.to_string_lossy().into_owned());
    BrowseError::from(&file_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_dir_is_nowhere_else_read() {
        // 环境变量只在 start_dir 里读一次；这里只断言它是绝对路径，不假设 NOUS_START_DIRS 的值。
        assert!(start_dir().is_absolute());
    }

    #[test]
    fn local_machine_accepts_absent_and_empty() {
        assert!(is_local_machine(None));
        assert!(is_local_machine(Some("")));
        assert!(!is_local_machine(Some("some-other-box")));
    }
}

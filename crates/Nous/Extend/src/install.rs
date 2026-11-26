//! 安装 / 升级：**临时目录 → fsync → 原子 rename → 切配置**（spec 8.3）。
//!
//! 任何时刻 `kill -9`，重启后要么是完整旧版、要么是完整新版。目录形状：
//!
//! ```text
//! <artifacts>/<file>/
//!   current -> versions/<version>        # 原子切换点（符号链接 rename）
//!   versions/<version>/<file> + meta.json
//! ```
//!
//! 直接写目标目录（A12）在这里是不可能的：写只发生在 `tmp-*` 里，出现在 `current` 之前
//! 一定已经完整。

use std::path::{Path, PathBuf};

use nous_ext_sdk::manifest::Manifest;
use serde_json::{json, Value};

use crate::error::{ExtError, ExtResult};
use crate::manifest::ArtifactMeta;
use crate::sha256;

/// 一个产物仓库。
#[derive(Debug, Clone)]
pub struct Installer {
    pub root: PathBuf,
}

impl Installer {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 装一份产物，返回 `current` 指向的版本目录。
    pub fn install(
        &self,
        manifest: &Manifest,
        binary: &[u8],
        meta: &ArtifactMeta,
    ) -> ExtResult<PathBuf> {
        let base = self.root.join(&manifest.file);
        let versions = base.join("versions");
        let version_dir = versions.join(&manifest.version);
        std::fs::create_dir_all(&versions)
            .map_err(|error| ExtError::Spawn(format!("mkdir {}: {error}", versions.display())))?;

        if !version_dir.exists() {
            let tmp = base.join(format!(
                "tmp-{}-{}",
                std::process::id(),
                crate::runtime::now_ms()
            ));
            std::fs::create_dir_all(&tmp)
                .map_err(|error| ExtError::Spawn(format!("mkdir {}: {error}", tmp.display())))?;
            let binary_path = tmp.join(&manifest.file);
            write_synced(&binary_path, binary)?;
            // 产物要能 exec（别人的 uid 也要能）。
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755));
            let mut meta_json = json!({
                "hash": meta.hash,
                "builtFrom": meta.built_from,
                "buildMachine": meta.build_machine,
                "protocol": meta.protocol,
                "triple": meta.triple,
            });
            meta_json["manifestHash"] = Value::String(sha256::hex(
                serde_json::to_string(manifest).unwrap_or_default().as_bytes(),
            ));
            write_synced(
                &tmp.join("meta.json"),
                serde_json::to_string_pretty(&meta_json)
                    .unwrap_or_default()
                    .as_bytes(),
            )?;
            sync_dir(&tmp);
            // 原子换到版本目录：rename 之后这个目录要么不在、要么完整。
            std::fs::rename(&tmp, &version_dir).map_err(|error| {
                let _ = std::fs::remove_dir_all(&tmp);
                ExtError::Spawn(format!("rename {}: {error}", tmp.display()))
            })?;
        }

        // 切配置：先建临时符号链接，再原子 rename 到 current。
        let link = base.join("current");
        let tmp_link = base.join(format!("current.tmp-{}", std::process::id()));
        let _ = std::fs::remove_file(&tmp_link);
        let target = PathBuf::from("versions").join(&manifest.version);
        std::os::unix::fs::symlink(&target, &tmp_link)
            .map_err(|error| ExtError::Spawn(format!("symlink: {error}")))?;
        std::fs::rename(&tmp_link, &link)
            .map_err(|error| ExtError::Spawn(format!("rename current: {error}")))?;
        sync_dir(&base);
        Ok(version_dir)
    }

    /// 装完之后清掉没被 `current` 引用的残留（半装/旧版），返回清掉的目录数。
    pub fn cleanup(&self, file: &str) -> usize {
        let base = self.root.join(file);
        let Ok(current) = std::fs::read_link(base.join("current")) else {
            return 0;
        };
        let keep = base.join(current);
        let mut removed = 0;
        let Ok(entries) = std::fs::read_dir(&base) else {
            return 0;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("tmp-") || name.starts_with("current.tmp-") {
                let _ = std::fs::remove_dir_all(&path);
                let _ = std::fs::remove_file(&path);
                removed += 1;
            }
        }
        if let Ok(entries) = std::fs::read_dir(base.join("versions")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path != keep && path.is_dir() {
                    if std::fs::remove_dir_all(&path).is_ok() {
                        removed += 1;
                    }
                }
            }
        }
        removed
    }

    /// `current` 指向的版本目录 + 它是否完整（二进制与 meta 都在）。
    pub fn current(&self, file: &str) -> Option<(PathBuf, bool)> {
        let base = self.root.join(file);
        let current = std::fs::read_link(base.join("current")).ok()?;
        let dir = if current.is_absolute() {
            current
        } else {
            base.join(current)
        };
        let complete = dir.join(file).is_file() && dir.join("meta.json").is_file();
        Some((dir, complete))
    }
}

fn write_synced(path: &Path, bytes: &[u8]) -> ExtResult<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)
        .map_err(|error| ExtError::Spawn(format!("create {}: {error}", path.display())))?;
    file.write_all(bytes)
        .map_err(|error| ExtError::Spawn(format!("write {}: {error}", path.display())))?;
    file.sync_all()
        .map_err(|error| ExtError::Spawn(format!("fsync {}: {error}", path.display())))?;
    Ok(())
}

fn sync_dir(path: &Path) {
    if let Ok(dir) = std::fs::File::open(path) {
        let _ = dir.sync_all();
    }
}

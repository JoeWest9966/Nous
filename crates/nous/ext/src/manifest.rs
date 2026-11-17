//! 清单装载与产物校验（spec ch8）。
//!
//! 装载前四项一次跑完：**哈希**（改一字节即拒，EX-48）、**target triple**（在兼容列表里即通过，
//! 不是相等，EX-50）、**protocol**（握手时对，EX-06）、**清单字段**（EX-02 / EX-03 / EX-05）。

use std::path::{Path, PathBuf};

use nous_ext_sdk::manifest::Manifest;
use serde_json::Value;

use crate::config::RuntimeConfig;
use crate::error::{ExtError, ExtResult};
use crate::sha256;

/// 一份加载好的清单。
#[derive(Debug, Clone)]
pub struct LoadedManifest {
    pub manifest: Manifest,
    pub raw: Value,
    pub dir: PathBuf,
    /// 清单文件的 SHA-256（审计里的 `manifestHash`）。
    pub hash: String,
}

/// 产物的来源三项 + 装载校验要的字段。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactMeta {
    pub hash: String,
    pub built_from: String,
    pub build_machine: String,
    pub protocol: u32,
    pub triple: String,
}

impl ArtifactMeta {
    pub fn parse(raw: &Value) -> Option<Self> {
        Some(Self {
            hash: raw.get("hash")?.as_str()?.to_string(),
            built_from: raw
                .get("builtFrom")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            build_machine: raw
                .get("buildMachine")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            protocol: raw.get("protocol").and_then(Value::as_u64).unwrap_or(0) as u32,
            triple: raw
                .get("triple")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        })
    }
}

impl LoadedManifest {
    /// 从扩展目录读 `nous.json` 并校验。
    pub fn load(dir: &Path) -> ExtResult<Self> {
        let path = dir.join("nous.json");
        let text = std::fs::read_to_string(&path)
            .map_err(|error| ExtError::Spawn(format!("read {}: {error}", path.display())))?;
        let raw: Value = serde_json::from_str(&text)
            .map_err(|error| ExtError::Spawn(format!("parse {}: {error}", path.display())))?;
        let manifest = Manifest::parse(&raw).map_err(ExtError::Manifest)?;
        if manifest.is_empty() {
            return Err(ExtError::EmptyExtension);
        }
        Ok(Self {
            manifest,
            raw,
            dir: dir.to_path_buf(),
            hash: sha256::hex(text.as_bytes()),
        })
    }
}

/// 扫一个根目录：一个子目录一个扩展。顺序按目录名（确定性）。
pub fn scan(root: &Path) -> Vec<(PathBuf, ExtResult<LoadedManifest>)> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|item| item.path()))
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .map(|dir| {
            let loaded = LoadedManifest::load(&dir);
            (dir, loaded)
        })
        .collect()
}

/// 装载后的产物：二进制路径 + 元数据。
#[derive(Debug, Clone)]
pub struct Artifact {
    pub path: PathBuf,
    pub meta: ArtifactMeta,
}

/// 按清单里的产物名解析并校验产物。
///
/// 目录形状两种都认：
/// - `<artifacts>/<file>` —— 直接把二进制放在那里（样例/开发）；
/// - `<artifacts>/<file>/current -> versions/<version>` —— 原子切换装出来的（spec 8.3）。
pub fn resolve_artifact(config: &RuntimeConfig, manifest: &Manifest) -> ExtResult<Artifact> {
    let base = config.artifacts_dir.join(&manifest.file);
    let (path, meta_path) = if base.is_dir() {
        let current = base.join("current");
        let resolved = std::fs::read_link(&current).unwrap_or_else(|_| PathBuf::from("current"));
        let path = if resolved.is_absolute() {
            resolved
        } else {
            base.join(resolved)
        };
        (path.join(&manifest.file), path.join("meta.json"))
    } else {
        (base, config.artifacts_dir.join(format!("{}.json", manifest.file)))
    };

    let meta_text = std::fs::read_to_string(&meta_path)
        .map_err(|error| ExtError::Spawn(format!("read {}: {error}", meta_path.display())))?;
    let meta_value: Value = serde_json::from_str(&meta_text)
        .map_err(|error| ExtError::Spawn(format!("parse {}: {error}", meta_path.display())))?;
    let meta = ArtifactMeta::parse(&meta_value)
        .ok_or_else(|| ExtError::Spawn(format!("{} 缺 hash", meta_path.display())))?;

    // ① 哈希：改一字节就拒，不重试、不降级。
    let bytes = std::fs::read(&path)
        .map_err(|error| ExtError::Spawn(format!("read {}: {error}", path.display())))?;
    let actual = sha256::hex(&bytes);
    if actual != meta.hash {
        return Err(ExtError::HashMismatch {
            expected: meta.hash.clone(),
            actual,
        });
    }

    // ② target triple：在兼容列表里即通过（不是相等）。
    if !meta.triple.is_empty()
        && !config
            .host_triples
            .iter()
            .any(|triple| triple == &meta.triple)
    {
        return Err(ExtError::TripleNotCompatible {
            triple: meta.triple.clone(),
            accepted: config.host_triples.clone(),
        });
    }

    Ok(Artifact { path, meta })
}

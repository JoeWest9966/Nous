//! 文件层：路径安全检查、原子写、etag、If-Match、列目录。
//! 所有路径先 realpath 再确认在仓根内；拒绝二进制，单文件 512KB。

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use crate::error::{RepoError, Result};

pub const MAX_FILE_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone)]
pub struct WriteFile<'a> {
    pub path: &'a str,
    pub content: &'a [u8],
    /// Some = If-Match；None = 新建/覆盖。
    pub base_etag: Option<&'a str>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct WrittenFile {
    pub path: String,
    pub etag: String,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub dir: bool,
}

/// 路径必须是相对路径，且 realpath 之后仍在仓根内。
pub fn safe_path(root: &Path, rel: &str) -> Result<PathBuf> {
    if rel.is_empty() || rel.starts_with('/') {
        return Err(RepoError::BadRequest(format!("路径必须是仓内相对路径：{rel}")));
    }
    let candidate = Path::new(rel);
    for part in candidate.components() {
        match part {
            Component::Normal(_) => {}
            _ => return Err(RepoError::BadRequest(format!("路径含非法片段：{rel}"))),
        }
    }
    let real_root = fs::canonicalize(root).map_err(|e| RepoError::Io(e.to_string()))?;
    // 找最长已存在的祖先并 realpath；剩余组件逐段追加，遇符号链接则再次 realpath。
    let mut ancestor = root.join(candidate);
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !ancestor.exists() {
        // 记录被剥掉的组件再 pop；root 本身就存在时循环不进来。
        if let Some(name) = ancestor.file_name() { tail.push(name.to_os_string()); }
        if !ancestor.pop() || ancestor == Path::new("/") {
            return Err(RepoError::BadRequest("路径不在仓根内".into()));
        }
    }
    let mut resolved = fs::canonicalize(&ancestor).map_err(|e| RepoError::Io(e.to_string()))?;
    if !resolved.starts_with(&real_root) {
        return Err(RepoError::BadRequest(format!("路径逃出仓根：{rel}")));
    }
    for name in tail.into_iter().rev() {
        resolved.push(name);
        if resolved.is_symlink() {
            resolved = fs::canonicalize(&resolved).map_err(|e| RepoError::Io(e.to_string()))?;
            if !resolved.starts_with(&real_root) {
                return Err(RepoError::BadRequest(format!("路径逃出仓根：{rel}")));
            }
        }
    }
    Ok(resolved)
}

fn check_text(path: &str, content: &[u8]) -> Result<()> {
    if content.len() > MAX_FILE_BYTES {
        return Err(RepoError::BadRequest(format!("文件超过 {}KB：{path}", MAX_FILE_BYTES / 1024)));
    }
    if content.contains(&0) {
        return Err(RepoError::BadRequest(format!("拒绝二进制文件：{path}")));
    }
    std::str::from_utf8(content)
        .map_err(|_| RepoError::BadRequest(format!("拒绝非 UTF-8 文件：{path}")))?;
    Ok(())
}

pub fn etag(content: &[u8]) -> String {
    // sha256（hex），与 git blob sha 区别开：etag 是内容指纹，不是 git 对象。
    use std::fmt::Write as _;
    let digest = simple_sha256(content);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 纯 Rust 的 sha256（FIPS 180-4）。`etag` 与需要内容指纹的插件共用这一份 ——
/// 两处各抄一遍实现，抄错过一次（`presets` 的修订 sha 与 etag 必须逐字节相等）。
pub fn sha256(data: &[u8]) -> [u8; 32] {
    simple_sha256(data)
}

fn simple_sha256(data: &[u8]) -> [u8; 32] {
    // 纯 Rust 紧凑实现（不引入依赖）：FIPS 180-4。
    const K: [u32; 64] = [
        0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
        0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
        0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
        0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
        0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
        0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
        0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
        0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0); }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i*4],chunk[i*4+1],chunk[i*4+2],chunk[i*4+3]]);
        }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
            let s1 = w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ ((!v[4]) & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v[7]=v[6]; v[6]=v[5]; v[5]=v[4]; v[4]=v[3].wrapping_add(t1);
            v[3]=v[2]; v[2]=v[1]; v[1]=v[0]; v[0]=t1.wrapping_add(t2);
        }
        for i in 0..8 { h[i] = h[i].wrapping_add(v[i]); }
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() { out[i*4..i*4+4].copy_from_slice(&word.to_be_bytes()); }
    out
}

pub fn read(root: &Path, rel: &str) -> Result<WrittenFile> {
    let path = safe_path(root, rel)?;
    if !path.is_file() {
        return Err(RepoError::NotFound(format!("文件不存在：{rel}")));
    }
    let content = fs::read(&path).map_err(|e| RepoError::Io(e.to_string()))?;
    Ok(WrittenFile { path: rel.to_string(), etag: etag(&content), content })
}

/// 写文件：临时文件 + rename；If-Match 不符回 409 + 磁盘当前正文。
pub fn write(root: &Path, input: WriteFile<'_>) -> Result<WrittenFile> {
    check_text(input.path, input.content)?;
    let path = safe_path(root, input.path)?;
    let current = if path.exists() {
        Some(fs::read(&path).map_err(|e| RepoError::Io(e.to_string()))?)
    } else {
        None
    };
    let current_etag = current.as_deref().map(etag);
    if let Some(base) = input.base_etag {
        if current_etag.as_deref() != Some(base) {
            return Err(RepoError::Conflict {
                message: format!("If-Match 不符：{} 已被改过", input.path),
                current,
                etag: current_etag,
            });
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| RepoError::Io(e.to_string()))?;
    }
    let tmp = path.with_extension(format!("nous-tmp-{}", std::process::id()));
    {
        let mut file = fs::File::create(&tmp).map_err(|e| RepoError::Io(e.to_string()))?;
        file.write_all(input.content).map_err(|e| RepoError::Io(e.to_string()))?;
        file.sync_all().map_err(|e| RepoError::Io(e.to_string()))?;
    }
    fs::rename(&tmp, &path).map_err(|e| RepoError::Io(e.to_string()))?;
    Ok(WrittenFile { path: input.path.to_string(), etag: etag(input.content), content: input.content.to_vec() })
}

pub fn remove(root: &Path, rel: &str, base_etag: Option<&str>) -> Result<()> {
    let path = safe_path(root, rel)?;
    if !path.is_file() {
        return Err(RepoError::NotFound(format!("文件不存在：{rel}")));
    }
    if let Some(base) = base_etag {
        let current = fs::read(&path).map_err(|e| RepoError::Io(e.to_string()))?;
        let current_etag = etag(&current);
        if current_etag != base {
            return Err(RepoError::conflict(
                format!("If-Match 不符：{rel} 已被改过"),
                current,
                current_etag,
            ));
        }
    }
    fs::remove_file(&path).map_err(|e| RepoError::Io(e.to_string()))
}

pub fn list(root: &Path, rel: &str) -> Result<Vec<DirEntry>> {
    let path = safe_path(root, if rel.is_empty() { "." } else { rel })?;
    if !path.is_dir() {
        return Err(RepoError::NotFound(format!("目录不存在：{rel}")));
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&path).map_err(|e| RepoError::Io(e.to_string()))? {
        let entry = entry.map_err(|e| RepoError::Io(e.to_string()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" || name.starts_with(".nous-tmp-") { continue; }
        let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        entries.push(DirEntry { name, path: child, dir: entry.path().is_dir() });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

pub fn create_dir(root: &Path, rel: &str) -> Result<()> {
    let path = safe_path(root, rel)?;
    if path.exists() {
        return Err(RepoError::Conflict { message: format!("目录已存在：{rel}"), current: None, etag: None });
    }
    fs::create_dir_all(&path).map_err(|e| RepoError::Io(e.to_string()))
}

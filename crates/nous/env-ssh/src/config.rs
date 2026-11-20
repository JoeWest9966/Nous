//! 机器清单驱动的连接参数（`device` 的 `machines`）。
//!
//! 只读 `NOUS_MACHINES_FILE` 指的那份 JSON；`{machines:[...]}` 或直接数组都收。
//! 凭据不在这里：私钥内容只从环境读，config 只认 `keyPath`（宿主管密钥）。

use std::path::Path;

use serde::Deserialize;

pub const MACHINES_FILE_ENV: &str = "NOUS_MACHINES_FILE";

/// 连接一台机器需要的参数（其余编目字段用 `Value` 原样收着透传）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    pub name: String,
    pub user: String,
    pub addr: String,
    pub port: u16,
    /// 可选：已有私钥路径（宿主自己管权限）。内嵌密钥另走环境 → 临时文件。
    pub key_path: Option<String>,
    /// 远端执行环境的工作目录，缺省 `~`。
    pub cwd: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineError {
    /// 编目里没有这个名字 —— HTTP 映射 404，不是 501/500。
    Unknown,
    /// 有这个名字但没有可用的连接参数。
    Invalid,
}

#[derive(Debug, Deserialize)]
struct Catalog {
    #[serde(default)]
    machines: Vec<MachineEntry>,
}

#[derive(Debug, Default, Deserialize)]
struct MachineEntry {
    name: Option<String>,
    user: Option<String>,
    addr: Option<String>,
    hostname: Option<String>,
    #[serde(rename = "ip")]
    ip: Option<String>,
    port: Option<u16>,
    key_path: Option<String>,
    #[serde(rename = "keyPath")]
    key_path_camel: Option<String>,
    cwd: Option<String>,
    ssh: Option<SshEntry>,
}

#[derive(Debug, Default, Deserialize)]
struct SshEntry {
    user: Option<String>,
    addr: Option<String>,
    hostname: Option<String>,
    port: Option<u16>,
    key_path: Option<String>,
    #[serde(rename = "keyPath")]
    key_path_camel: Option<String>,
    cwd: Option<String>,
}

fn pick<T>(primary: Option<T>, fallback: Option<T>) -> Option<T> {
    primary.or(fallback)
}

impl MachineEntry {
    fn into_machine(self) -> Option<Machine> {
        let ssh = self.ssh.unwrap_or_default();
        let name = self.name?;
        let user = pick(self.user, ssh.user).unwrap_or_else(|| "root".to_string());
        let addr = pick(self.addr, ssh.addr)
            .or(self.hostname)
            .or(ssh.hostname)
            .or(self.ip)?;
        let port = pick(self.port, ssh.port).unwrap_or(22);
        let key_path = pick(self.key_path, ssh.key_path)
            .or(pick(self.key_path_camel, ssh.key_path_camel));
        let cwd = pick(self.cwd, ssh.cwd).unwrap_or_else(|| "~".to_string());
        if addr.is_empty() || name.is_empty() {
            return None;
        }
        Some(Machine { name, user, addr, port, key_path, cwd })
    }
}

/// 从编目文件里按名字找一台机器。文件缺失 / 解析失败 = 没有这台机器（404）。
pub fn machine(name: &str) -> Result<Machine, MachineError> {
    let path = std::env::var(MACHINES_FILE_ENV)
        .ok()
        .filter(|text| !text.trim().is_empty())
        .map(std::path::PathBuf::from)
        .ok_or(MachineError::Unknown)?;
    machine_from_file(&path, name)
}

pub fn machine_from_file(path: &Path, name: &str) -> Result<Machine, MachineError> {
    let text = std::fs::read_to_string(path).map_err(|_| MachineError::Unknown)?;
    let entries: Vec<MachineEntry> = match serde_json::from_str::<Catalog>(&text) {
        Ok(catalog) => catalog.machines,
        Err(_) => serde_json::from_str::<Vec<MachineEntry>>(&text).unwrap_or_default(),
    };
    let entry = entries
        .into_iter()
        .find(|entry| entry.name.as_deref() == Some(name))
        .ok_or(MachineError::Unknown)?;
    entry.into_machine().ok_or(MachineError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_entry_accepts_hostname_from_the_device_catalog() {
        let entry: MachineEntry = serde_json::from_str(
            r#"{"name":"j1900","hostname":"10.0.0.9","user":"deploy","port":2222}"#,
        )
        .unwrap();
        let machine = entry.into_machine().unwrap();
        assert_eq!(machine.addr, "10.0.0.9");
        assert_eq!(machine.user, "deploy");
        assert_eq!(machine.port, 2222);
        assert_eq!(machine.cwd, "~");
    }

    #[test]
    fn missing_name_is_unknown_not_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("machines.json");
        std::fs::write(&path, r#"{"machines":[]}"#).unwrap();
        assert_eq!(machine_from_file(&path, "nope"), Err(MachineError::Unknown));
    }
}

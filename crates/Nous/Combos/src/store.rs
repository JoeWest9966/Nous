//! 组合（终极模板里「触发器 + 会话模板」那一条）的盘面读写。
//!
//! 这一层是 app 层的文件系统逻辑：名册由扫 `<root>/<section>/<name>/preset.json` 得出，
//! 与 presets 共用数据布局，但**没有**进程内常驻名单。每次请求都重扫盘面 —— 人手改文件
//! 也算数。`enabled` 由引擎算：`spec.enabled !== false` 且引用的 trigger / session 都在；
//! 客户端只显示 + 写穿，不自己拼规则。
//!
//! 写穿只动 `spec.enabled` 一位。幂等回执存在 `<root>/.combos-receipts/`（点开头，扫名册
//! 时会跳过），不另开一份「开关名单」。
//!
//! 搬家前这一层叫 `server::combos`，只读盘、不认宿主；现在仍是 —— 两个根
//! （模板私库 / 触发器名册）由宿主装配一次传进来（[`Host::combo_roots`]），
//! 路径解析仍然只有 `env` 插件一处。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use nous_routes::HttpRefusal;

use crate::precondition_refusal;

/// 组合只活在终极模板那一段。`Session-Template` 的条目是「开一条会话用什么」，
/// 没有触发器可言 —— 以前两段都扫，结果会话模板全带「这条没钉触发器」的假 `problem`。
/// 段白名单只有这一个值，别的段一律 400（不静默返回空：客户端会以为「这一段没组合」）。
pub const SECTIONS: &[&str] = &["Ultimate-Template"];

/// 目录名（不是 section 名）：扫名册时跳过，不是一条组合。
const RECEIPTS_DIR: &str = ".combos-receipts";

/// 一份组合的盘面原文（不缓存）。
#[derive(Debug, Clone)]
pub(crate) struct ComboFile {
    pub(crate) name: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) body: Value,
}

impl ComboFile {
    fn version(&self) -> String {
        format!("v{}", &crate::sha256_hex(&self.bytes)[..12])
    }
}

/// 一条组合的对外形状。`unknown` 里收着引擎以后加的字段，原样透传。
pub(crate) fn view(file: &ComboFile, triggers: &BTreeSet<String>, sessions: &BTreeSet<String>) -> Value {
    let spec = file.body.get("spec").cloned().unwrap_or(Value::Null);
    let mut problems = Vec::new();
    let trigger = spec.get("trigger").and_then(Value::as_str).unwrap_or("").to_string();
    let session = spec.get("session").and_then(Value::as_str).unwrap_or("").to_string();
    let params = spec.get("params").cloned().unwrap_or_else(|| json!({}));
    if trigger.is_empty() {
        problems.push("这条没钉触发器".to_string());
    } else if !triggers.contains(&trigger) {
        problems.push(format!("引用的 trigger `{trigger}` 不在了"));
    }
    if session.is_empty() {
        problems.push("这条没钉会话模板".to_string());
    } else if !sessions.contains(&session) {
        problems.push(format!("引用的会话模板 `{session}` 不在了"));
    }
    let explicit = spec.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    let mut out = json!({
        "name": file.name,
        "trigger": if trigger.is_empty() { Value::Null } else { Value::String(trigger.clone()) },
        "params": params,
        "session": if session.is_empty() { Value::Null } else { Value::String(session.clone()) },
        "enabled": explicit && problems.is_empty(),
        "problem": problems.join("；"),
        "version": file.version(),
    });
    // 未知字段原样透传：引擎加字段不该让我们编译失败，也不该在这里被吞掉。
    if let Some(object) = out.as_object_mut() {
        if let Some(spec_object) = spec.as_object() {
            for (key, value) in spec_object {
                if !object.contains_key(key) {
                    object.insert(key.clone(), value.clone());
                }
            }
        }
    }
    out
}

/// 扫描连接器：两个根都由宿主装配（生产从 `NOUS_PRESETS` / `NOUS_TRIGGER` 读，测试直接注）。
#[derive(Debug, Clone)]
pub struct ComboStore {
    root: PathBuf,
    triggers: PathBuf,
}

impl ComboStore {
    pub fn new(root: impl Into<PathBuf>, triggers: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), triggers: triggers.into() }
    }

    /// 模板私库根（组合段所在的那棵树）。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 触发器名册根（`<root>/<code>/` 下有目录就算存在）。
    pub fn triggers_root(&self) -> &Path {
        &self.triggers
    }

    fn section_path(&self, section: &str) -> Result<PathBuf, HttpRefusal> {
        if !SECTIONS.contains(&section) {
            return Err(HttpRefusal::bad_request(format!(
                "不认识的 section: {section}（只有 {}）", SECTIONS.join(", ")
            )));
        }
        Ok(self.root.join(section))
    }

    fn file_path(&self, section: &str, name: &str) -> Result<PathBuf, HttpRefusal> {
        if name.is_empty()
            || Path::new(name).file_name().and_then(|n| n.to_str()) != Some(name)
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
        {
            return Err(HttpRefusal::bad_request(format!("非法的 name: {name:?}")));
        }
        let section_path = self.section_path(section)?;
        let dir = section_path.join(name);
        let real_root = fs::canonicalize(&self.root)
            .map_err(|error| HttpRefusal::internal(format!("realpath 数据根失败: {error}")))?;
        // 词法检查只挡 `../`；目录是仓内指向仓外的符号链接时只有 realpath 挡得住。
        // 写法照 crates/Nous/Repo/src/files.rs::safe_path：解析最长已存在的祖先，
        // 不存在的尾段逐段追加，逐段验符号链接。
        let mut ancestor = dir.clone();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        while !ancestor.exists() {
            if let Some(part) = ancestor.file_name() { tail.push(part.to_os_string()); }
            if !ancestor.pop() || ancestor == Path::new("/") {
                return Err(HttpRefusal::bad_request(format!("路径逃出私库根: {section}/{name}")));
            }
        }
        let mut resolved = fs::canonicalize(&ancestor)
            .map_err(|error| HttpRefusal::internal(format!("realpath 失败: {error}")))?;
        if !resolved.starts_with(&real_root) {
            return Err(HttpRefusal::bad_request(format!("路径逃出私库根: {section}/{name}")));
        }
        for part in tail.into_iter().rev() {
            resolved.push(part);
            if resolved.is_symlink() {
                resolved = fs::canonicalize(&resolved)
                    .map_err(|error| HttpRefusal::internal(format!("realpath 失败: {error}")))?;
                if !resolved.starts_with(&real_root) {
                    return Err(HttpRefusal::bad_request(format!("路径逃出私库根: {section}/{name}")));
                }
            }
        }
        Ok(dir.join("preset.json"))
    }

    fn scan(&self, section: &str) -> Result<Vec<ComboFile>, HttpRefusal> {
        let dir = self.section_path(section)?;
        self.scan_dir(&dir, section)
    }

    /// 扫一个目录（不管它是不是组合段）：`Session-Template` 作为**引用目标**也要扫。
    fn scan_dir(&self, dir: &Path, label: &str) -> Result<Vec<ComboFile>, HttpRefusal> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(error) => return Err(HttpRefusal::internal(format!("扫 {label} 失败: {error}"))),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            if !entry.path().is_dir() || entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let bytes = match fs::read(entry.path().join("preset.json")) {
                Ok(bytes) => bytes,
                Err(_) => continue, // 没有 preset.json 的目录不是一条组合
            };
            let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            out.push(ComboFile {
                name: entry.file_name().to_string_lossy().into_owned(),
                bytes,
                body,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub(crate) fn one(&self, section: &str, name: &str) -> Result<ComboFile, HttpRefusal> {
        let path = self.file_path(section, name)?;
        let bytes = fs::read(&path)
            .map_err(|_| HttpRefusal::not_found(format!("组合不存在: {section}/{name}")))?;
        let body = serde_json::from_slice(&bytes)
            .map_err(|error| HttpRefusal::bad_request(format!("preset.json 坏了: {error}")))?;
        Ok(ComboFile { name: name.to_string(), bytes, body })
    }

    /// 触发器名册：`<NOUS_TRIGGER>/<code>/` 下有目录就算存在。
    pub(crate) fn trigger_names(&self) -> BTreeSet<String> {
        fs::read_dir(&self.triggers)
            .ok()
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.path().is_dir())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 会话模板名册：与 presets 共用数据根，从盘现扫（不读 durable 库）。
    ///
    /// **不经过 `section_path`**：`Session-Template` 不是组合段，但它是组合的引用目标，
    /// 这里只需要把那一堆目录名字扫出来。
    pub(crate) fn session_names(&self) -> Result<BTreeSet<String>, HttpRefusal> {
        let dir = self.root.join("Session-Template");
        Ok(self
            .scan_dir(&dir, "Session-Template")?
            .into_iter()
            .map(|file| file.name)
            .collect())
    }

    pub(crate) fn roster(&self, section: &str) -> Result<Value, HttpRefusal> {
        let triggers = self.trigger_names();
        let sessions = self.session_names()?;
        let items = self.scan(section)?
            .iter()
            .map(|file| view(file, &triggers, &sessions))
            .collect::<Vec<_>>();
        Ok(json!({ "combos": items }))
    }

    /// 写穿 `spec.enabled` 一位；只动这一条组合的这一位。
    pub(crate) fn patch(&self, section: &str, name: &str, enabled: bool, if_match: &str, request_id: &str) -> Result<Value, HttpRefusal> {
        // 幂等优先于版本号：客户端拿第一次的 If-Match 重放同一 requestId 时，
        // 盘面已经变了也不能再回 412 —— 那等于让幂等锚失效（坑 99）。
        if let Some(previous) = self.receipt(section, name, request_id)? {
            return Ok(previous);
        }
        let file = self.one(section, name)?;
        if file.version() != if_match {
            return Err(precondition_refusal(format!("版本不匹配：期望 {if_match}，盘上是 {}", file.version())));
        }
        let mut body = file.body.clone();
        // `spec` 是引擎的扩展位：旧条目（0.1.7 之前写下的）没有这个键。
        // 缺了就补一个空对象再写 —— 不要回 400 把人挡在门外（那 5 条老模板就这样）。
        let object = body.as_object_mut().ok_or_else(|| {
            HttpRefusal::bad_request("preset.json 必须是 JSON 对象".to_string())
        })?;
        let spec = object
            .entry("spec".to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| {
                HttpRefusal::bad_request("preset.json 的 spec 不是对象".to_string())
            })?;
        spec.insert("enabled".to_string(), json!(enabled));
        let bytes = serde_json::to_vec_pretty(&body)
            .map_err(|error| HttpRefusal::internal(format!("序列化 preset.json: {error}")))?;
        let written = ComboFile { name: name.to_string(), bytes: bytes.clone(), body };
        let triggers = self.trigger_names();
        let sessions = self.session_names()?;
        let mut receipt = view(&written, &triggers, &sessions);
        receipt["requestId"] = json!(request_id);
        // review 10（盘上）：回执先写（原子 rename），再改 preset；任一步失败就整体回滚
        // （删掉刚写的回执），重放必回到「变更未发生」的原点，不会拿到半截回执。
        self.remember(section, name, request_id, &receipt)?;
        let path = self.file_path(section, name)?;
        let dir = path.parent().ok_or_else(|| HttpRefusal::internal("没有父目录"))?;
        let temp = dir.join(".preset.json.tmp");
        let write_result = (|| -> Result<(), HttpRefusal> {
            if std::env::var("NOUS_TEST_COMBOS_WRITE_FAIL").as_deref() == Ok("1") {
                return Err(HttpRefusal::internal("injected preset write failure"));
            }
            fs::write(&temp, &bytes)
                .map_err(|error| HttpRefusal::internal(format!("写临时文件失败: {error}")))?;
            fs::rename(&temp, &path)
                .map_err(|error| HttpRefusal::internal(format!("原子替换失败: {error}")))?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = std::fs::remove_file(&temp);
            let _ = std::fs::remove_file(self.receipt_dir(section, name).join(format!("{request_id}.json")));
            return Err(error);
        }
        Ok(receipt)
    }

    /// #82：动作（fire / launch）的幂等回执，与 PATCH 共用同一份盘面目录。
    pub(crate) fn action_receipt(&self, name: &str, request_id: &str) -> Result<Option<Value>, HttpRefusal> {
        self.receipt("Ultimate-Template", name, request_id)
    }

    pub(crate) fn remember_action(&self, name: &str, request_id: &str, receipt: &Value) -> Result<(), HttpRefusal> {
        self.remember("Ultimate-Template", name, request_id, receipt)
    }

    fn receipt_dir(&self, section: &str, name: &str) -> PathBuf {
        self.root.join(RECEIPTS_DIR).join(section).join(name)
    }

    fn receipt(&self, section: &str, name: &str, request_id: &str) -> Result<Option<Value>, HttpRefusal> {
        let path = self.receipt_dir(section, name).join(format!("{request_id}.json"));
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(HttpRefusal::internal(format!("读回执失败: {error}"))),
        };
        let value = serde_json::from_slice(&bytes)
            .map_err(|error| HttpRefusal::internal(format!("回执坏了: {error}")))?;
        Ok(Some(value))
    }

    fn remember(&self, section: &str, name: &str, request_id: &str, receipt: &Value) -> Result<(), HttpRefusal> {
        let dir = self.receipt_dir(section, name);
        fs::create_dir_all(&dir)
            .map_err(|error| HttpRefusal::internal(format!("建回执目录失败: {error}")))?;
        fs::write(dir.join(format!("{request_id}.json")), serde_json::to_vec(receipt).unwrap())
            .map_err(|error| HttpRefusal::internal(format!("记回执失败: {error}")))?;
        Ok(())
    }
}

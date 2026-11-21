//! #84：扩展源码的管理动作。
//!
//! 这里只操作 `NOUS_EXTEND` 那份私库的源码目录（平台能力在 `nous-repo`，进程在
//! `runtime`）：写文件是原子写，垃圾站移动是 rename，删除名单是随私库发布的 JSON。
//! 本模块不 spawn git、不读取环境变量、不持有运行时状态。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use crate::error::{ExtError, ExtResult};
use crate::runtime::now_ms;

/// 私库根的删除名单（防 git pull 把本地删掉的扩展带回来）。
pub const REPO_DELETED_FILE: &str = "deleted-extensions.json";
/// 类别词表（在私库里，随发布进历史）。
pub const REPO_CATEGORIES_FILE: &str = "categories.json";
/// 三个核心扩展：引擎自己需要，不能本地删。
pub const CORE_EXTENSIONS: &[&str] = &["nous", "skills", "prompts"];

/// 扫描扩展目录时跳过删除名单里的名字（名单住在私库根，随发布走）。
pub fn scan_active(root: &Path) -> Vec<(PathBuf, ExtResult<crate::manifest::LoadedManifest>)> {
    let deleted = deleted_names(root);
    crate::manifest::scan(root)
        .into_iter()
        .filter(|(dir, _)| {
            let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            !deleted.iter().any(|item| item == &name)
        })
        .collect()
}
/// 新建扩展落下的三个文件（名字与顺序稳定，判据逐个数）。
pub const SCAFFOLD_FILES: &[&str] = &["nous.json", "README.md", "main.rs"];

#[derive(Debug, Clone)]
pub struct CreateOptions {
    pub name: String,
    pub category: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DuplicateOptions {
    pub name: String,
    pub to: String,
    /// 解绑变体：清单里的 file / entry 也改成新名字。
    pub unbind: bool,
}

/// 扩展源码根 + 垃圾站根。
#[derive(Debug, Clone)]
pub struct AdminStore {
    pub root: PathBuf,
    /// `data/extend-trash/`，不在私库里（不随发布走）。
    pub trash: PathBuf,
}

impl AdminStore {
    pub fn new(root: impl Into<PathBuf>, trash: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), trash: trash.into() }
    }

    fn dir(&self, name: &str) -> ExtResult<PathBuf> {
        Ok(self.root.join(valid_name(name)?))
    }

    fn deleted_path(&self) -> PathBuf {
        self.root.join(REPO_DELETED_FILE)
    }

    fn categories_path(&self) -> PathBuf {
        self.root.join(REPO_CATEGORIES_FILE)
    }

    pub fn deleted(&self) -> Vec<String> {
        deleted_names(&self.root)
    }

    pub fn categories(&self) -> Vec<String> {
        read_string_array(&self.categories_path())
    }

    /// 新建扩展骨架：目录 + 三个文件，不做 git 提交。
    pub fn create(&self, input: &CreateOptions) -> ExtResult<Value> {
        let dir = self.dir(&input.name)?;
        if dir.exists() {
            return Err(ExtError::Spawn(format!("扩展 {} 已存在", input.name)));
        }
        fs::create_dir_all(&dir)
            .map_err(|error| ExtError::Spawn(format!("mkdir {}: {error}", dir.display())))?;
        let mut manifest = json!({
            "name": input.name,
            "version": "0.1.0",
            "protocol": 3,
            "file": format!("exts-{}", input.name),
            "entry": format!("exts-{}", input.name),
            "sections": [],
            "handles": [],
            "summary": format!("{}（#84 新建）", input.name),
        });
        if let Some(category) = &input.category {
            if !category.trim().is_empty() {
                manifest["category"] = json!(category);
            }
        }
        let readme = format!("# {}\n\n由 `POST /v1/extensions/create` 生成。\n", input.name);
        let main = format!("// {} 的入口骨架。编译产物放在 artifacts/ 下。\nfn main() {{}}\n", input.name);
        write_file(&dir.join(SCAFFOLD_FILES[0]), manifest.to_string().as_bytes())?;
        write_file(&dir.join(SCAFFOLD_FILES[1]), readme.as_bytes())?;
        write_file(&dir.join(SCAFFOLD_FILES[2]), main.as_bytes())?;
        Ok(json!({
            "name": input.name,
            "dir": dir.to_string_lossy().into_owned(),
            "files": SCAFFOLD_FILES,
        }))
    }

    /// 整目录复制：除 `nous.json` 外逐个文件字节不变。
    pub fn duplicate(&self, input: &DuplicateOptions) -> ExtResult<Value> {
        let source = self.dir(&input.name)?;
        let target = self.dir(&input.to)?;
        if !source.is_dir() {
            return Err(ExtError::Spawn(format!("扩展 {} 不存在", input.name)));
        }
        if target.exists() {
            return Err(ExtError::Spawn(format!("扩展 {} 已存在", input.to)));
        }
        let mut files = Vec::new();
        for entry in fs::read_dir(&source)
            .map_err(|error| ExtError::Spawn(format!("read {}: {error}", source.display())))?
        {
            let entry = entry.map_err(|error| ExtError::Spawn(error.to_string()))?;
            if !entry.path().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let content = fs::read(entry.path())
                .map_err(|error| ExtError::Spawn(format!("read {}: {error}", entry.path().display())))?;
            if name == "nous.json" {
                let mut manifest: Value = serde_json::from_slice(&content)
                    .map_err(|error| ExtError::Spawn(format!("解析清单：{error}")))?;
                manifest["name"] = json!(input.to);
                if input.unbind {
                    manifest["file"] = json!(format!("exts-{}", input.to));
                    manifest["entry"] = json!(format!("exts-{}", input.to));
                }
                let bytes = serde_json::to_vec_pretty(&manifest).unwrap_or_default();
                write_file(&target.join(&name), &bytes)?;
            } else {
                write_file(&target.join(&name), &content)?;
            }
            files.push(name);
        }
        files.sort();
        Ok(json!({ "name": input.name, "to": input.to, "files": files }))
    }

    /// 卸载：源码移到垃圾站 + 记删除名单。核心扩展直接拒。
    pub fn uninstall(&self, name: &str) -> ExtResult<Value> {
        if CORE_EXTENSIONS.contains(&name) {
            return Err(ExtError::Spawn(format!("核心扩展 {name} 不许本地删")));
        }
        let dir = self.dir(name)?;
        if !dir.is_dir() {
            return Err(ExtError::Spawn(format!("扩展 {name} 不存在")));
        }
        fs::create_dir_all(&self.trash)
            .map_err(|error| ExtError::Spawn(format!("mkdir {}: {error}", self.trash.display())))?;
        let trashed = self.trash.join(format!("{name}-{}", now_ms()));
        fs::rename(&dir, &trashed)
            .map_err(|error| ExtError::Spawn(format!("移到垃圾站失败：{error}")))?;
        let mut deleted = self.deleted();
        if !deleted.iter().any(|item| item == name) {
            deleted.push(name.to_string());
            deleted.sort();
            write_file(&self.deleted_path(), serde_json::to_vec_pretty(&deleted).unwrap_or_default().as_slice())?;
        }
        Ok(json!({
            "name": name,
            "trashed": trashed.to_string_lossy().into_owned(),
            "deleted": deleted,
        }))
    }

    /// 安装：优先垃圾站最新一份，没有就读 HEAD（由调用方先 `git restore`）。
    pub fn install(&self, name: &str) -> ExtResult<Value> {
        let dir = self.dir(name)?;
        let listed = self.deleted().iter().any(|item| item == name);
        if dir.is_dir() && !listed {
            return Err(ExtError::Spawn(format!("扩展 {name} 已在源码根")));
        }
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.trash) {
            for entry in entries.flatten() {
                let path = entry.path();
                let file = entry.file_name().to_string_lossy().into_owned();
                if path.is_dir() && file.starts_with(&format!("{name}-")) {
                    candidates.push(path);
                }
            }
        }
        candidates.sort();
        let (from, source) = if let Some(path) = candidates.last() {
            ("trash", path.clone())
        } else {
            let head = self.root.join(valid_name(name)?);
            if !head.is_dir() {
                return Err(ExtError::Spawn(format!("垃圾站和 HEAD 里都没有扩展 {name}")));
            }
            ("head", head)
        };
        if from == "trash" {
            // git pull 可能已带回来一个残目录：先用垃圾站的完整内容替换它。
            if dir.is_dir() {
                fs::remove_dir_all(&dir)
                    .map_err(|error| ExtError::Spawn(format!("清残目录失败：{error}")))?;
            }
            fs::rename(&source, &dir)
                .map_err(|error| ExtError::Spawn(format!("从垃圾站装回失败：{error}")))?;
        }
        let deleted: Vec<String> = self.deleted().into_iter().filter(|item| item != name).collect();
        write_file(&self.deleted_path(), serde_json::to_vec_pretty(&deleted).unwrap_or_default().as_slice())?;
        Ok(json!({ "name": name, "from": from }))
    }

    /// 类别词表：增 / 改名 / 删。改名同时改写所有挂它的 `nous.json.category`。
    pub fn edit_categories(&self, op: &str, name: &str, to: Option<&str>) -> ExtResult<Value> {
        let mut categories = self.categories();
        let mut affected = Vec::new();
        match op {
            "add" => {
                if !categories.iter().any(|item| item == name) {
                    categories.push(name.to_string());
                }
            }
            "rename" => {
                let to = to.ok_or_else(|| ExtError::Spawn("rename 需要 to".into()))?;
                if !categories.iter().any(|item| item == name) {
                    return Err(ExtError::Spawn(format!("词表里没有类别 {name}")));
                }
                for item in &mut categories {
                    if item == name { *item = to.to_string(); }
                }
                affected = self.rewrite_category(name, Some(to))?;
            }
            "remove" => {
                categories.retain(|item| item != name);
                affected = self.rewrite_category(name, None)?;
            }
            other => return Err(ExtError::Spawn(format!("未知词表操作：{other}"))),
        }
        categories.sort();
        categories.dedup();
        write_file(&self.categories_path(), serde_json::to_vec_pretty(&categories).unwrap_or_default().as_slice())?;
        Ok(json!({ "categories": categories, "affected": affected }))
    }

    fn rewrite_category(&self, from: &str, to: Option<&str>) -> ExtResult<Vec<String>> {
        let mut affected = Vec::new();
        let Ok(entries) = fs::read_dir(&self.root) else { return Ok(affected) };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || path.file_name().is_some_and(|n| n == ".git") { continue; }
            let manifest_path = path.join("nous.json");
            if !manifest_path.is_file() { continue; }
            let text = fs::read_to_string(&manifest_path)
                .map_err(|error| ExtError::Spawn(format!("read {}: {error}", manifest_path.display())))?;
            let mut manifest: Value = serde_json::from_str(&text)
                .map_err(|error| ExtError::Spawn(format!("解析清单：{error}")))?;
            if manifest.get("category").and_then(Value::as_str) != Some(from) { continue; }
            if let Some(value) = to {
                manifest["category"] = json!(value);
            } else if let Some(map) = manifest.as_object_mut() {
                map.remove("category");
            }
            write_file(&manifest_path, serde_json::to_vec_pretty(&manifest).unwrap_or_default().as_slice())?;
            affected.push(entry.file_name().to_string_lossy().into_owned());
        }
        affected.sort();
        Ok(affected)
    }

    /// 给一个扩展挂/改类别；`None` 清除。
    pub fn set_category(&self, name: &str, category: Option<&str>) -> ExtResult<Value> {
        let dir = self.dir(name)?;
        let path = dir.join("nous.json");
        if !path.is_file() {
            return Err(ExtError::Spawn(format!("扩展 {name} 的清单不存在")));
        }
        let text = fs::read_to_string(&path)
            .map_err(|error| ExtError::Spawn(format!("read {}: {error}", path.display())))?;
        let mut manifest: Value = serde_json::from_str(&text)
            .map_err(|error| ExtError::Spawn(format!("解析清单：{error}")))?;
        if let Some(value) = category {
            manifest["category"] = json!(value);
        } else if let Some(map) = manifest.as_object_mut() {
            map.remove("category");
        }
        write_file(&path, serde_json::to_vec_pretty(&manifest).unwrap_or_default().as_slice())?;
        Ok(json!({ "name": name, "category": category }))
    }
}

/// 私库根的删除名单（纯函数，运行时扫描也用它 —— 防 git pull 复活）。
pub fn deleted_names(root: &Path) -> Vec<String> {
    read_string_array(&root.join(REPO_DELETED_FILE))
}

/// 名字必须是单一普通路径段：防 `../` 逃出私库根。
fn valid_name(name: &str) -> ExtResult<&str> {
    let path = Path::new(name);
    let mut components = path.components();
    match components.next() {
        Some(Component::Normal(_)) if components.next().is_none() && !name.is_empty() => Ok(name),
        _ => Err(ExtError::Spawn(format!("非法扩展名：{name}"))),
    }
}

fn read_string_array(path: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(path) else { return Vec::new() };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Array(items)) => {
            let mut seen = BTreeSet::new();
            items.into_iter().filter_map(|item| item.as_str().map(str::to_string))
                .filter(|item| seen.insert(item.clone())).collect()
        }
        _ => Vec::new(),
    }
}

fn write_file(path: &Path, content: &[u8]) -> ExtResult<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| ExtError::Spawn(format!("mkdir {}: {error}", parent.display())))?;
    }
    let mut file = fs::File::create(path)
        .map_err(|error| ExtError::Spawn(format!("create {}: {error}", path.display())))?;
    file.write_all(content)
        .map_err(|error| ExtError::Spawn(format!("write {}: {error}", path.display())))?;
    file.sync_all()
        .map_err(|error| ExtError::Spawn(format!("fsync {}: {error}", path.display())))?;
    Ok(())
}

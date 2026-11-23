//! elements 插件：Nous 元素库（组 / 包）的盘上真相。
//!
//! 元素就是 JSON 文件，一个文件一个元素，私库根下两个目录：
//!
//! ```text
//! groups/<id>.json     组：一组包的引用
//! packages/<id>.json   包：一份可复用的东西（提示词 / 规则 / 清单 / 文件）
//! ```
//!
//! 四条不越过的线（`k3skws` 块的行为移植，见 `wire.md`「元素库」）：
//!
//! - **未知字段原样透传**：只校验 `id` / `name` 两个必填，其余字段进来什么样、出去什么样，
//!   一个键都不删、不补默认值。
//! - **写一次提交一次**：落盘 → `git commit` → 尽力 push。push 失败不回滚本地提交，
//!   响应里如实说 `pushed: false`。
//! - **幂等**：同一个 `requestId` 只提交一次，重放回 `replay: true`（`nous-repo` 的
//!   `Request-Id` trailer 就是回执）。
//! - **id 就是文件名**：只收 `[a-z0-9-]`、字母数字开头、最长 64。改名 = 换 id，
//!   引用会断 —— 所以没有 rename，改名是「新建一份 + 删掉旧的」。
//!
//! 这一层不 spawn 进程、不读环境变量：路径由 `nous-env` 解析，git 只在 `nous-repo` 里。

#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use nous_repo::{CommitResult, Pushed, Repo, RepoError, Result, WriteCommit};
use serde_json::{json, Map, Value};

/// id 最长 64（与旧面板 `ID_RE` 同一个数）。
pub const ID_MAX: usize = 64;

/// 元素的两类。URL 段与盘上目录用复数（`groups` / `packages`），
/// 提交信息与响应里的 `kind` 用单数（`group` / `package`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Groups,
    Packages,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::Groups, Kind::Packages];

    /// 盘上的目录名，也是 URL 里的那一段。
    pub fn dir(self) -> &'static str {
        match self {
            Self::Groups => "groups",
            Self::Packages => "packages",
        }
    }

    /// 提交信息里的那一截：`feat(group): <id>`。
    pub fn singular(self) -> &'static str {
        match self {
            Self::Groups => "group",
            Self::Packages => "package",
        }
    }

    /// 给人看的中文名（错误信息用）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Groups => "组",
            Self::Packages => "包",
        }
    }

    /// 只认复数目录名与单数名两种写法；别的一律不猜。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "groups" | "group" => Some(Self::Groups),
            "packages" | "package" => Some(Self::Packages),
            _ => None,
        }
    }
}

/// id 规则：字母数字开头，其余字母数字加连字符，最长 [`ID_MAX`]。
pub fn check_id(raw: &str) -> std::result::Result<String, String> {
    let id = raw.trim();
    if id.is_empty() {
        return Err("id 不能为空".into());
    }
    if id.chars().count() > ID_MAX {
        return Err(format!("id 太长（最多 {ID_MAX} 个字）：{id}"));
    }
    let mut chars = id.chars();
    let first = chars.next().unwrap_or_default();
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(format!("id 必须以小写字母或数字开头：{id}"));
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(format!("id 只认小写字母、数字、连字符：{id}"));
    }
    Ok(id.to_string())
}

fn file_path(kind: Kind, id: &str) -> String {
    format!("{}/{id}.json", kind.dir())
}

/// 一份读得出来的元素：`fields` 是整份 JSON（含未知字段），`etag` 是内容指纹。
#[derive(Debug, Clone, PartialEq)]
pub struct ElementItem {
    pub id: String,
    pub fields: Value,
    pub etag: String,
}

/// 一份读不出来的元素：坏 JSON / 顶层不是对象 / id 与文件名对不上。
/// 只标红这一份，不影响同一目录里的其他元素（旧面板同一条行为）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokenFile {
    pub id: String,
    pub path: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scan {
    pub items: Vec<ElementItem>,
    pub broken: Vec<BrokenFile>,
}

/// 扫一个类别：`<dir>/*.json`，按 id 排序。
///
/// 目录不存在 = 空库（不是错误）；坏 JSON 不进 `items`，进 `broken`。
pub fn scan(repo: &Repo, kind: Kind) -> Result<Scan> {
    let dir = kind.dir();
    if !repo.root().join(dir).is_dir() {
        return Ok(Scan::default());
    }
    let mut scan = Scan::default();
    for entry in repo.list_dir(dir)? {
        if entry.dir || entry.name.starts_with('.') {
            continue;
        }
        let Some(id) = entry.name.strip_suffix(".json") else { continue };
        let path = format!("{dir}/{}", entry.name);
        if let Err(problem) = check_id(id) {
            scan.broken.push(BrokenFile { id: id.to_string(), path, detail: problem });
            continue;
        }
        let read = match repo.read_file(&path) {
            Ok(read) => read,
            Err(error) => {
                scan.broken.push(BrokenFile { id: id.to_string(), path, detail: error.to_string() });
                continue;
            }
        };
        match parse_object(&read.content) {
            Ok(fields) => scan.items.push(ElementItem { id: id.to_string(), fields, etag: read.etag }),
            Err(problem) => scan.broken.push(BrokenFile { id: id.to_string(), path, detail: problem }),
        }
    }
    scan.items.sort_by(|a, b| a.id.cmp(&b.id));
    scan.broken.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(scan)
}

/// 读一份元素。id 不合法 = 400；文件不存在 = 404。
pub fn get(repo: &Repo, kind: Kind, id: &str) -> Result<ElementItem> {
    let id = check_id(id).map_err(RepoError::BadRequest)?;
    let read = repo.read_file(&file_path(kind, &id)).map_err(|error| match error {
        RepoError::NotFound(_) => {
            RepoError::NotFound(format!("没有这份{}：{}/{}", kind.label(), kind.dir(), id))
        }
        other => other,
    })?;
    let fields = parse_object(&read.content).map_err(RepoError::Io)?;
    Ok(ElementItem { id, fields, etag: read.etag })
}

#[derive(Debug, Clone)]
pub struct PutOutcome {
    pub id: String,
    pub created: bool,
    /// 写后的内容指纹；重放时为 None（这次没写盘）。
    pub etag: Option<String>,
    /// 真的产生了提交才有；重放或内容没变时为 None。
    pub commit: Option<CommitResult>,
    /// 同一 `requestId` 之前已提交过：一个字节都没碰。
    pub replay: bool,
}

/// 新建或覆盖一份元素：落盘 → 提交 → 尽力推。
///
/// `base` 给了就是 If-Match（内容指纹不符回 409）；`request_id` 给了就幂等。
pub fn put(
    repo: &Repo,
    kind: Kind,
    id: &str,
    fields: Value,
    base: Option<&str>,
    request_id: Option<&str>,
) -> Result<PutOutcome> {
    let id = check_id(id).map_err(RepoError::BadRequest)?;
    validate_fields(kind, &id, &fields)?;
    let path = file_path(kind, &id);
    let existed = repo.read_file(&path).is_ok();
    let body = serialize(&fields);
    let message = format!("feat({}): {id}", kind.singular());
    let result = repo.write_and_commit(WriteCommit {
        path: &path,
        content: Some(body.as_bytes()),
        base_etag: base,
        message: &message,
        request_id,
    })?;
    Ok(PutOutcome {
        id,
        created: !existed,
        etag: result.etag,
        commit: result.commit,
        replay: result.replay,
    })
}

#[derive(Debug, Clone)]
pub struct DeleteOutcome {
    pub commit: Option<CommitResult>,
    pub replay: bool,
}

/// 删掉一份元素并提交（`chore(package): rm <id>`）。
pub fn remove(
    repo: &Repo,
    kind: Kind,
    id: &str,
    base: Option<&str>,
    request_id: Option<&str>,
) -> Result<DeleteOutcome> {
    let id = check_id(id).map_err(RepoError::BadRequest)?;
    let path = file_path(kind, &id);
    if repo.read_file(&path).is_err() {
        return Err(RepoError::NotFound(format!(
            "没有这份{}：{}/{}",
            kind.label(),
            kind.dir(),
            id
        )));
    }
    let message = format!("chore({}): rm {id}", kind.singular());
    let result = repo.write_and_commit(WriteCommit {
        path: &path,
        content: None,
        base_etag: base,
        message: &message,
        request_id,
    })?;
    Ok(DeleteOutcome { commit: result.commit, replay: result.replay })
}

/// 手动补推（网络恢复后页面上的「重试推送」）。
pub fn push(repo: &Repo) -> Result<Pushed> {
    repo.sync()
}

/// 组 → 包的引用解析：`{ "groups": { <组 id>: { <包 id>: { state } } }, "missing": [...] }`。
///
/// 只判「包在不在盘上」：`ok` / `missing`。不替调用方补空包、不改数据。
pub fn resolve(groups: &[ElementItem], packages: &[ElementItem]) -> Value {
    let known: BTreeSet<&str> = packages.iter().map(|item| item.id.as_str()).collect();
    let mut listed = Map::new();
    let mut missing = Vec::new();
    for group in groups {
        let mut references = Map::new();
        if let Some(items) = group.fields.get("packages").and_then(Value::as_array) {
            for item in items {
                let Some(id) = item.as_str() else { continue };
                let ok = known.contains(id);
                references.insert(
                    id.to_string(),
                    json!({ "state": if ok { "ok" } else { "missing" } }),
                );
                if !ok {
                    missing.push(format!("{}/{}", group.id, id));
                }
            }
        }
        listed.insert(group.id.clone(), Value::Object(references));
    }
    json!({ "groups": listed, "missing": missing })
}

/// 规范落盘格式：两空格缩进，末尾一个换行（旧面板约定的一部分）。
pub fn serialize(fields: &Value) -> String {
    let mut text = serde_json::to_string_pretty(fields).unwrap_or_else(|_| "{}".to_string());
    text.push('\n');
    text
}

fn parse_object(bytes: &[u8]) -> std::result::Result<Value, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("这份 JSON 读不出来：{error}"))?;
    if !value.is_object() {
        return Err("顶层必须是一个 JSON 对象".into());
    }
    Ok(value)
}

/// 只校验认识的那几个字段，别的一概不碰（未知字段原样透传）。
fn validate_fields(kind: Kind, id: &str, fields: &Value) -> Result<()> {
    let Some(object) = fields.as_object() else {
        return Err(RepoError::Validation {
            field: "body".into(),
            problem: "body 必须是一个 JSON 对象".into(),
        });
    };
    match object.get("id").and_then(Value::as_str) {
        Some(body_id) if body_id == id => {}
        Some(body_id) => {
            return Err(RepoError::Validation {
                field: "id".into(),
                problem: format!("body.id（{body_id}）与路径里的 id（{id}）不一致"),
            })
        }
        None => {
            return Err(RepoError::Validation {
                field: "id".into(),
                problem: "id 必填，且必须是字符串".into(),
            })
        }
    }
    if object.get("name").and_then(Value::as_str).is_none_or(|name| name.trim().is_empty()) {
        return Err(RepoError::Validation {
            field: "name".into(),
            problem: "name 必填，且必须是非空字符串".into(),
        });
    }

    match kind {
        Kind::Groups => {
            if let Some(items) = object.get("packages").filter(|value| !value.is_null()) {
                let Some(items) = items.as_array() else {
                    return Err(RepoError::Validation {
                        field: "packages".into(),
                        problem: "packages 必须是数组（元素是包的 id 字符串）".into(),
                    });
                };
                for item in items {
                    let Some(reference) = item.as_str() else {
                        return Err(RepoError::Validation {
                            field: "packages".into(),
                            problem: format!("packages 里有非字符串引用：{item}"),
                        });
                    };
                    if let Err(problem) = check_id(reference) {
                        return Err(RepoError::Validation {
                            field: "packages".into(),
                            problem: format!("packages 里有不合法的引用：{problem}"),
                        });
                    }
                }
            }
        }
        Kind::Packages => {
            if let Some(tags) = object.get("tags").filter(|value| !value.is_null()) {
                let valid = tags
                    .as_array()
                    .is_some_and(|items| items.iter().all(|item| item.is_string()));
                if !valid {
                    return Err(RepoError::Validation {
                        field: "tags".into(),
                        problem: "tags 必须是字符串数组".into(),
                    });
                }
            }
            if let Some(body) = object.get("body").filter(|value| !value.is_null()) {
                if !body.is_string() {
                    return Err(RepoError::Validation {
                        field: "body".into(),
                        problem: "body 字段必须是字符串（正文内联）".into(),
                    });
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: &str, packages: &[&str]) -> ElementItem {
        ElementItem {
            id: id.into(),
            fields: json!({ "id": id, "name": "组", "packages": packages }),
            etag: "x".into(),
        }
    }

    fn package(id: &str) -> ElementItem {
        ElementItem {
            id: id.into(),
            fields: json!({ "id": id, "name": "包" }),
            etag: "x".into(),
        }
    }

    #[test]
    fn ids_follow_the_old_panel_rule() {
        for good in ["a", "frontend", "react-rules", "a1", "x-9"] {
            assert_eq!(check_id(good).unwrap(), good);
        }
        for bad in ["", "-a", "A", "a_b", "a/b", "a b", "é"] {
            assert!(check_id(bad).is_err(), "{bad} 应该被拒");
        }
        assert!(check_id(&"a".repeat(ID_MAX)).is_ok());
        assert!(check_id(&"a".repeat(ID_MAX + 1)).is_err());
    }

    #[test]
    fn kind_parses_singular_and_plural_only() {
        assert_eq!(Kind::parse("groups"), Some(Kind::Groups));
        assert_eq!(Kind::parse("group"), Some(Kind::Groups));
        assert_eq!(Kind::parse("packages"), Some(Kind::Packages));
        assert_eq!(Kind::parse("package"), Some(Kind::Packages));
        assert_eq!(Kind::parse("groups "), Some(Kind::Groups));
        assert_eq!(Kind::parse("g"), None);
        assert_eq!(Kind::parse("skills"), None);
    }

    #[test]
    fn validation_only_touches_id_name_and_known_types() {
        let ok = json!({ "id": "a", "name": "组", "whatever": { "deep": [1, 2] } });
        assert!(validate_fields(Kind::Groups, "a", &ok).is_ok(), "未知字段不许被拒");

        let missing_id = json!({ "name": "组" });
        assert!(matches!(
            validate_fields(Kind::Groups, "a", &missing_id),
            Err(RepoError::Validation { field, .. }) if field == "id"
        ));

        let mismatched = json!({ "id": "b", "name": "组" });
        assert!(validate_fields(Kind::Groups, "a", &mismatched).is_err());

        let blank = json!({ "id": "a", "name": "   " });
        assert!(validate_fields(Kind::Groups, "a", &blank).is_err());

        let not_array = json!({ "id": "a", "name": "组", "packages": "react" });
        assert!(validate_fields(Kind::Groups, "a", &not_array).is_err());

        let bad_ref = json!({ "id": "a", "name": "组", "packages": ["React_Rules"] });
        assert!(validate_fields(Kind::Groups, "a", &bad_ref).is_err());

        let bad_tags = json!({ "id": "a", "name": "包", "tags": [1] });
        assert!(validate_fields(Kind::Packages, "a", &bad_tags).is_err());

        let bad_body = json!({ "id": "a", "name": "包", "body": 3 });
        assert!(validate_fields(Kind::Packages, "a", &bad_body).is_err());

        // 包不看 packages、组不看 tags —— 不认识的那一段原样留着。
        let cross = json!({ "id": "a", "name": "包", "packages": "随便" });
        assert!(validate_fields(Kind::Packages, "a", &cross).is_ok());
    }

    #[test]
    fn serialize_is_two_spaces_and_one_trailing_newline() {
        let text = serialize(&json!({ "id": "a", "name": "组" }));
        assert_eq!(text, "{\n  \"id\": \"a\",\n  \"name\": \"组\"\n}\n");
        // 键序原样（serde_json 开了 preserve_order）。
        let ordered = serialize(&json!({ "zeta": 1, "alpha": 2 }));
        assert!(ordered.find("zeta").unwrap() < ordered.find("alpha").unwrap());
    }

    #[test]
    fn resolve_marks_missing_packages_without_touching_data() {
        let groups = vec![group("frontend", &["react-rules", "gone"])];
        let packages = vec![package("react-rules")];
        let value = resolve(&groups, &packages);
        assert_eq!(value["groups"]["frontend"]["react-rules"]["state"], "ok");
        assert_eq!(value["groups"]["frontend"]["gone"]["state"], "missing");
        assert_eq!(value["missing"], json!(["frontend/gone"]));
        // 原数据一个键都没动。
        assert_eq!(groups[0].fields["packages"], json!(["react-rules", "gone"]));
    }
}

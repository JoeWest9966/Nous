//! #47：能连上的端点 + 能力声明的 wire 形状快照。
//!
//! 权威：`wire.md`（端点与约束）+ `NousDesk/crates/nous-engine/src/types.rs`（客户端真实期望）。
//! 断言只钉「字段在、类型对」，不钉「恰好这些字段」：`ReadReceipt` 等类型带 `#[serde(flatten)]`，
//! 多回字段是合法的。快照文件让 camelCase 漂移一眼可见。

mod common;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

static CONTENT_ENV_LOCK: Mutex<()> = Mutex::new(());

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

fn snapshot(test: &str, value: &Value) {
    let path = snapshots_dir().join(format!("{test}.json"));
    let actual = serde_json::to_string_pretty(value).unwrap() + "\n";
    match std::fs::read_to_string(&path) {
        Ok(expected) if expected == actual => {}
        Ok(expected) => panic!(
            "字段形状快照漂移：{}\n--- expected ---\n{expected}\n--- actual ---\n{actual}",
            path.display()
        ),
        Err(_) => panic!(
            "快照不存在：{}（先按下面的 actual 建立文件）\n--- actual ---\n{actual}",
            path.display()
        ),
    }
}

/// 一个**一定不存在**的路径：给「默认路径不存在」这类判据用。
///
/// 别拿真路径当判据 —— `/opt/Nous-skills` 在有些机器上正好有（开发机就有一份真私库），
/// 判据会随机器变红/变绿。判据要的是语义（「指到不存在的目录时怎么答」），不是这台机器。
fn missing_repo() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "nous-skills-missing-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}

async fn get_json(app: &Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

/// 对象里必须有这些键，且每个键的值都过 `check`。
fn fields(value: &Value, where_: &str, expected: &[(&str, fn(&Value) -> bool)]) {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("{where_} 必须是对象：{value}"));
    for (key, check) in expected {
        let actual = object
            .get(*key)
            .unwrap_or_else(|| panic!("{where_} 缺字段 {key}：{value}"));
        assert!(check(actual), "{where_}.{key} 类型不对：{actual}");
    }
}

fn is_string(v: &Value) -> bool {
    v.is_string()
}
fn is_string_or_null(v: &Value) -> bool {
    v.is_string() || v.is_null()
}
fn is_array(v: &Value) -> bool {
    v.is_array()
}
fn is_bool(v: &Value) -> bool {
    v.as_bool().is_some()
}

/// GET /v1/device —— `types.rs::Device`。`available` 一定是字符串数组，其余可空。
#[tokio::test]
async fn device_response_pins_client_shape() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/device").await;
    assert_eq!(status, StatusCode::OK, "GET /v1/device 应 200");
    fields(
        &body,
        "Nous_Device",
        &[
            ("self", is_string_or_null),
            ("origin", is_string_or_null),
            ("node", is_string_or_null),
            ("version", is_string),
            ("build", is_string),
            ("target", is_string),
            ("startedAt", |v| v.is_string() || v.is_number() || v.is_null()),
            ("model", |_| true), // types.rs: Option<Value> —— 空目录就是 null
            ("faux", is_bool),
            ("data", is_string_or_null),
            ("catalog", |v| v.is_object() || v.is_null()),
            ("available", is_array),
        ],
    );
    assert!(
        body["available"].as_array().unwrap().iter().all(Value::is_string),
        "available 是 string[]"
    );
    // build 是正在跑的这份测试二进制的哈希，target 是编译目标；两者都单独断言过，
    // 快照前归一，避免每换一次编译产物就假红。启动时刻每次进程也不同。
    assert!(body["startedAt"].is_number());
    assert_eq!(body["build"].as_str().unwrap().len(), 8);
    let mut stable = body.clone();
    stable["startedAt"] = json!(0);
    stable["build"] = json!("00000000");
    stable["target"] = json!("test-target");
    snapshot("device_response_pins_client_shape", &stable);
}

/// GET /v1/selection —— 客户端当 `Value` 收，`{id}` / `{id:null}`。
#[tokio::test]
async fn selection_response_pins_id_field() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/selection").await;
    assert_eq!(status, StatusCode::OK, "GET /v1/selection 应 200");
    fields(&body, "selection", &[("id", |v| v.is_null() || v.is_number() || v.is_string())]);
    snapshot("selection_response_pins_id_field", &body);
}

/// 四条私库路径的环境变量入口。测试同时持有进程级 env 锁，避免互相踩。
struct ContentEnv {
    previous: Vec<(&'static str, Option<OsString>)>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl ContentEnv {
    /// 清空本票涉及的变量；其余环境（HOME 等）原样保留 —— 路径解析不读 HOME。
    fn cleared() -> Self {
        let guard = CONTENT_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names = [
            "NOUS_SKILLS",
            "NOUS_PROMPTS",
            "NOUS_PRESETS",
            "NOUS_EXTEND",
        ];
        let previous = names
            .iter()
            .map(|name| {
                let prev = std::env::var_os(name);
                std::env::remove_var(name);
                (*name, prev)
            })
            .collect();
        Self { previous, _guard: guard }
    }

    fn set(&mut self, name: &'static str, value: &Path) {
        std::env::set_var(name, value);
    }
}

impl Drop for ContentEnv {
    fn drop(&mut self) {
        for (name, prev) in self.previous.drain(..) {
            match prev {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

/// 私有内容根的测试替身：真目录、真条目；本测试夹具负责写入，产品代码只读。
struct ContentRepo {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl ContentRepo {
    fn with_two_skills() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        for name in ["alpha", "beta"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(name).join("SKILL.md"), format!("# {name}\n")).unwrap();
        }
        Self { _dir: dir, root }
    }

    /// (路径, mtime 纳秒, sha256) —— 用独立入口 `sha256sum` 算摘要，不看产品代码。
    fn fingerprint(&self) -> Vec<(String, Option<u128>, String)> {
        let mut files: Vec<_> = walk(&self.root);
        files.sort();
        files
            .into_iter()
            .map(|path| {
                let meta = std::fs::metadata(&path).unwrap();
                let modified = meta.modified().ok().and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_nanos())
                });
                let output = std::process::Command::new("sha256sum")
                    .arg(&path)
                    .output()
                    .unwrap();
                assert!(output.status.success(), "sha256sum 失败");
                let sha = String::from_utf8(output.stdout).unwrap();
                let sha = sha.split_whitespace().next().unwrap().to_string();
                (path.display().to_string(), modified, sha)
            })
            .collect()
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// GET /v1/skills —— `types.rs::Skills` / `Skill`。
#[tokio::test]
async fn skills_response_pins_client_shape() {
    // 显式指到一个不存在的目录：`present:false` 才是确定性形状（机器上可能有真私库）。
    let missing = missing_repo();
    let mut env = ContentEnv::cleared();
    env.set("NOUS_SKILLS", &missing);
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/skills").await;
    drop(env);
    assert_eq!(status, StatusCode::OK, "GET /v1/skills 应 200");
    fields(
        &body,
        "skills",
        &[
            ("root", is_string),
            ("present", is_bool),
            ("missing", |v| v.is_string() || v.is_null()),
            ("categories", is_array),
            ("skills", is_array),
            ("repo", |v| v.is_object()),
        ],
    );
    assert!(body["categories"].as_array().unwrap().iter().all(Value::is_string));
    // #71：repo 块只给 root + present（#63 形状），没有 git 状态字段。
    let mut repo_keys: Vec<_> = body["repo"].as_object().unwrap().keys().cloned().collect();
    repo_keys.sort();
    assert_eq!(repo_keys, ["present", "root"]);
    let mut stable = body.clone();
    stable["root"] = json!("<root>");
    stable["missing"] = json!("<missing>");
    stable["repo"]["root"] = json!("<root>");
    snapshot("skills_response_pins_client_shape", &stable);
}

/// 判据 2：不设任何环境变量，技能数据根回 TS 版的默认值（不是 null）。
#[tokio::test]
async fn skills_default_root_is_the_private_repo_path() {
    let env = ContentEnv::cleared();
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/skills").await;
    drop(env);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["root"], json!("/opt/Nous-skills"));
}

/// 判据 3：默认路径不存在时，root 照旧是路径，present=false，列表空。
#[tokio::test]
async fn missing_private_repo_says_where_it_looked() {
    // 用「一定不存在的覆盖值」代替「默认路径碰巧不存在」：判据要的是语义，不是这台机器。
    let missing = missing_repo();
    assert!(!missing.exists(), "测试自己的临时路径不该存在");
    let mut env = ContentEnv::cleared();
    env.set("NOUS_SKILLS", &missing);
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (_, body) = get_json(&app, "/v1/skills").await;
    drop(env);
    assert_eq!(body["root"], json!(missing.display().to_string()));
    assert_eq!(body["present"], json!(false));
    assert!(body["missing"].is_string(), "missing 要给一句人话");
    assert_eq!(body["skills"], json!([]));
}

/// 判据 4：NOUS_SKILLS 指向临时目录时，真条目可见，root 就是覆盖值。
#[tokio::test]
async fn skills_from_an_overridden_private_repo_are_visible() {
    let mut env = ContentEnv::cleared();
    let repo = ContentRepo::with_two_skills();
    let before = repo.fingerprint();
    env.set("NOUS_SKILLS", &repo.root);
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (_, body) = get_json(&app, "/v1/skills").await;

    assert_eq!(body["root"], json!(repo.root.display().to_string()));
    assert_eq!(body["present"], json!(true));
    assert!(body["missing"].is_null(), "present=true 时不该有 missing");
    let skills = body["skills"].as_array().expect("skills 是数组");
    assert_eq!(skills.len(), 2);
    let mut names: Vec<_> = skills.iter().map(|s| s["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(names, ["alpha", "beta"]);

    // 判据 5：只读 —— 引擎看过之后，临时目录里每个文件的 mtime 与 sha256 都没变。
    assert_eq!(repo.fingerprint(), before, "GET /v1/skills 改了私库里的文件");
    drop(env);
}

/// GET /v1/prompts —— `types.rs::Prompts` / `Prompt`。
#[tokio::test]
async fn prompts_response_pins_client_shape() {
    let env = ContentEnv::cleared();
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/prompts").await;
    drop(env);
    assert_eq!(status, StatusCode::OK, "GET /v1/prompts 应 200");
    fields(
        &body,
        "prompts",
        &[
            ("default", is_string_or_null),
            ("categories", is_array),
            ("prompts", is_array),
            ("root", is_string),
            ("present", is_bool),
            ("missing", |v| v.is_string() || v.is_null()),
        ],
    );
    let mut stable = body.clone();
    stable["root"] = json!("<root>");
    stable["missing"] = json!("<missing>");
    snapshot("prompts_response_pins_client_shape", &stable);
}

/// 提示词与技能走同一条解析规则：默认值回来，缺目录用 present 说清楚。
#[tokio::test]
async fn prompts_report_the_default_root_and_its_presence() {
    let env = ContentEnv::cleared();
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (_, body) = get_json(&app, "/v1/prompts").await;
    drop(env);
    assert_eq!(body["root"], json!("/opt/Nous-prompts"));
    assert_eq!(body["present"], json!(false));
    assert!(body["missing"].is_string());
    assert_eq!(body["prompts"], json!([]));
}

/// GET /v1/triggers —— 没有装配的测试路由也诚实返回空表，不是 404（#49 空表契约保留）。
#[tokio::test]
async fn triggers_response_is_an_empty_list_even_without_a_store() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/triggers").await;
    assert_eq!(status, StatusCode::OK, "GET /v1/triggers 应 200，不是 404");
    fields(&body, "triggers", &[("triggers", is_array), ("root", |v| v.is_null())]);
    assert!(body["triggers"].as_array().unwrap().is_empty());
}

/// GET /v1/extensions —— #81：没装配扩展时诚实回空清单，但形状钉住真清单字段。
#[tokio::test]
async fn extensions_response_pins_empty_items() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/extensions").await;
    assert_eq!(status, StatusCode::OK, "GET /v1/extensions 应 200");
    fields(&body, "extensions", &[("items", is_array)]);
    assert!(body["items"].as_array().unwrap().is_empty(), "没装配扩展 → items 空");
    assert_eq!(body["items"], json!([]), "冻结时代已过：空就是空，不许塞假条目");
    snapshot("extensions_response_pins_empty_items", &body);
}

/// #81：ext 运行时装配后，GET 回真清单：每个 item 的字段集合、类型与值来自装载结果。
#[tokio::test]
async fn extensions_response_reports_loaded_runtime_items() {
    use nous_extend::{ExtState, Runtime, RuntimeConfig};
    let dir = tempfile::tempdir().unwrap();
    // 只装配一个最小运行时：没有扩展目录也要能读出空清单（不伪造内容）。
    std::fs::create_dir_all(dir.path().join("exts")).unwrap();
    std::fs::create_dir_all(dir.path().join("artifacts")).unwrap();
    let config = RuntimeConfig::new(dir.path().join("exts"), dir.path().join("artifacts"));
    let runtime = Runtime::new(config);
    runtime.start_all();
    let expected = runtime.items();
    let _ = ExtState::Normal; // 状态枚举是投影的权威来源，防止测试与实现漂移
    for item in &expected {
        fields(
            item,
            "extensions.items[]",
            &[
                ("name", is_string),
                ("version", is_string),
                ("kind", is_string),
                ("loaded", is_bool),
                ("pid", |v| v.is_null() || v.is_u64()),
                ("state", is_string),
                ("error", |v| v.is_null() || v.is_string()),
            ],
        );
    }
}

/// GET /v1/capabilities —— 拷问结果 §1–§7：contract/server/11 模块/reason 对象/runtime。
#[tokio::test]
async fn capabilities_declares_modules_and_reasons() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/capabilities").await;
    assert_eq!(status, StatusCode::OK, "GET /v1/capabilities 应 200");
    fields(
        &body,
        "capabilities",
        &[
            ("contract", |v| v.is_i64()),
            ("server", |v| v.is_object()),
            ("modules", is_array),
            ("runtime", |v| v.is_object()),
            ("services", is_array),
        ],
    );
    let services = body["services"].as_array().unwrap();
    let engine = services
        .iter()
        .find(|service| service["name"] == json!("engine"))
        .expect("services 必须声明 engine");
    fields(engine, "services[engine]", &[("name", is_string), ("base", is_string)]);
    assert_eq!(engine["base"], json!("http://127.0.0.1:4789"));
    let triggers = services
        .iter()
        .find(|service| service["name"] == json!("triggers"))
        .expect("services 必须声明 triggers");
    fields(triggers, "services[triggers]", &[("name", is_string), ("enabled", is_bool)]);
    assert_eq!(triggers["enabled"], json!(true), "#57 后 triggers 已可用");
    assert_eq!(body["contract"], json!(1));
    fields(
        &body["server"],
        "server",
        &[("name", is_string), ("version", is_string)],
    );
    assert_eq!(body["server"]["name"], json!("nous"));
    fields(
        &body["runtime"]["provider"],
        "runtime.provider",
        &[("configured", is_bool), ("message", is_string)],
    );

    let modules = body["modules"].as_array().unwrap();
    // #88：每个 module（含未启用的）都带六列，且 enum 闭集。
    assert_eq!(modules.len(), 15, "#90 下线 documents、#60 补 presets-admin、元素库 elements + Element_Models 后 capabilities 应吐出 15 个 module");
    for module in modules {
        let name = module["name"].as_str().unwrap();
        let where_ = format!("capabilities.modules[{name}]");
        fields(
            module,
            &where_,
            &[
                ("version", is_string),
                ("kind", is_string),
                ("layer", is_string),
                ("required", is_bool),
                ("crate", is_string),
                ("description", is_string),
            ],
        );
        let kind = module["kind"].as_str().unwrap();
        assert!(
            matches!(kind, "编译期" | "声明式" | "运行时外挂"),
            "{where_}.kind 非法：{kind}"
        );
        let layer = module["layer"].as_str().unwrap();
        assert!(
            matches!(layer, "kernel" | "platform" | "app" | "dev"),
            "{where_}.layer 非法：{layer}"
        );
    }
    let by_name: std::collections::BTreeMap<&str, &Value> = modules
        .iter()
        .map(|m| (m["name"].as_str().expect("name 是字符串"), m))
        .collect();
    // 名字 == docs/plugins.md 的 module 列（评论拍板：ext，不是 extensions）。
    for name in [
        "sessions",
        "env",
        "repo",
        "presets",
        "presets-admin",
        "archive",
        "combos",
        "triggers",
        "extensions",
        "skills",
        "Nous_Prompts",
        "Nous_Device",
        "Nous_Extend",
    ] {
        if name == "extensions" {
            continue; // 旧名不许出现；下面的断言单独钉死
        }
        let module = by_name
            .get(name)
            .unwrap_or_else(|| panic!("capabilities 缺模块 {name}"));
        fields(module, "module", &[("name", is_string), ("enabled", is_bool)]);
        if module["enabled"].as_bool() == Some(true) {
            let endpoints = module["endpoints"].as_array().unwrap_or_else(|| {
                panic!("enabled:true 的 {name} 必须有 endpoints 数组")
            });
            assert!(
                endpoints.iter().all(|e| e.is_string()),
                "{name}.endpoints 元素是字符串"
            );
            assert!(
                module.get("reason").is_none(),
                "enabled:true 不该带 reason"
            );
        } else {
            let reason = module["reason"]
                .as_object()
                .unwrap_or_else(|| panic!("enabled:false 的 {name} 必须有 reason 对象"));
            let code = reason["code"].as_str().unwrap();
            assert!(
                matches!(
                    code,
                    "not_implemented"
                        | "disabled_by_manifest"
                        | "frozen"
                        | "needs_credentials"
                ),
                "非法 reason.code: {code}"
            );
            assert!(reason["message"].is_string(), "reason.message 是人话");
            assert!(
                module.get("endpoints").is_none(),
                "enabled:false 不该声明 endpoints"
            );
        }
    }
    assert!(
        !by_name.contains_key("extensions") && !by_name.contains_key("ext"),
        "模块全名是 Nous_Extend（规则 12），URL 段才是 extensions"
    );
    assert_eq!(by_name["sessions"]["enabled"], json!(true));
    // #60：presets 不再是 not_implemented，Desk「模板」页的 gate 才能打开。
    assert_eq!(by_name["presets"]["enabled"], json!(true));
    assert!(by_name["presets"].get("reason").is_none(), "enabled:true 不该带 reason");
    // #81：扩展运行时解冻；15 条端点已归 `Nous_Extend` 插件自带（搬家票）。
    assert_eq!(by_name["Nous_Extend"]["enabled"], json!(true));
    let ext_endpoints = by_name["Nous_Extend"]["endpoints"]
        .as_array()
        .expect("Nous_Extend 解冻后有 endpoints");
    assert_eq!(ext_endpoints.len(), 15, "扩展 15 条端点（编辑 4 + 管理 9 + 清单/重载 2）");
    assert!(ext_endpoints.iter().any(|e| e == "GET /v1/extensions"));
    assert!(ext_endpoints.iter().any(|e| e == "POST /v1/extensions/reload"));
    assert!(ext_endpoints.iter().any(|e| e == "PUT /v1/extensions/file"));
    assert!(by_name["Nous_Extend"].get("reason").is_none(), "enabled:true 不带 reason");
    // #61：combos 也不再是 not_implemented，Desk「终极模板」页的 gate 才能打开。
    assert_eq!(by_name["combos"]["enabled"], json!(true));
    assert!(by_name["combos"].get("reason").is_none(), "enabled:true 不该带 reason");
    let combos_endpoints = by_name["combos"]["endpoints"].as_array().unwrap();
    assert_eq!(combos_endpoints.len(), 4, "combos 现有列 + 开关 + 试跑 + launch 四条");
    assert!(combos_endpoints.iter().any(|e| e == "GET /v1/combos/:section"));
    assert!(combos_endpoints.iter().any(|e| e == "PATCH /v1/combos/:section/:name"));
    assert!(combos_endpoints.iter().any(|e| e == "POST /v1/combos/:section/:name/fire"));
    assert!(combos_endpoints.iter().any(|e| e == "POST /v1/presets/Ultimate-Template/launch"));
    snapshot("capabilities_declares_modules_and_reasons", &body);
}

/// #68：目录浏览作为已启用端点出现在 capabilities 声明里（从 ROUTES 自动生成）。
#[tokio::test]
async fn capabilities_declares_the_directory_browsing_endpoint() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/capabilities").await;
    assert_eq!(status, StatusCode::OK);
    let endpoint = "GET /v1/fs/dirs";
    let owners: Vec<&str> = body["modules"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|module| {
            module["endpoints"]
                .as_array()
                .is_some_and(|list| list.iter().any(|value| value.as_str() == Some(endpoint)))
        })
        .map(|module| module["name"].as_str().unwrap())
        .collect();
    assert_eq!(owners, vec!["env"], "端点必须只由 env 模块声明一次");
}

/// 拷问结果 §8：_routes ↔ enabled 模块 endpoints 多重集相等（双向）。
/// 两侧都来自 `meta` 的同一注册表；这里从 HTTP 响应分别取值，不让手抄数组蒙混。
#[tokio::test]
async fn route_registry_and_enabled_endpoints_are_the_same_multiset() {
    use std::collections::BTreeMap;
    fn tally(items: Vec<String>) -> BTreeMap<String, usize> {
        let mut map = BTreeMap::new();
        for item in items {
            *map.entry(item).or_insert(0) += 1;
        }
        map
    }

    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, capabilities) = get_json(&app, "/v1/capabilities").await;
    assert_eq!(status, StatusCode::OK);
    let declared: Vec<String> = capabilities["modules"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|module| module["enabled"].as_bool() == Some(true))
        .flat_map(|module| {
            module["endpoints"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_string())
        })
        .collect();
    // _routes 的 routes 视图（从同一张注册表吐出），两边都走 HTTP/公共视图取值。
    let (status, routes_body) = get_json(&app, "/v1/_routes").await;
    assert_eq!(status, StatusCode::OK);
    let registry: Vec<String> = routes_body["routes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|route| {
            format!(
                "{} {}",
                route["method"].as_str().unwrap(),
                route["path"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        tally(declared),
        tally(registry),
        "_routes 与 endpoints 必须是同一个多重集"
    );
    // 负向哨兵：注册表里每条声明都得真能在 Router 上打到 handler。
    // 405/500 才说明路由没注册或 handler 炸了；404 可能是资源的「不存在」语义。
    for route in nous_server::meta::ROUTES {
        if !nous_server::meta::route_enabled(route.module) {
            continue;
        }
        let uri = route.path
            .replace(":id", "2")
            .replace("{kind}", "presets")
            .replace("{section}", "Session-Template")
            .replace("{name}", "missing-name");
        let request = Request::builder()
            .method(route.method)
            .uri(&uri)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert!(
            response.status() != StatusCode::METHOD_NOT_ALLOWED
                && response.status() != StatusCode::INTERNAL_SERVER_ERROR,
            "注册表声明的 {} {uri} 没打到 handler：{}",
            route.method, response.status()
        );
    }
}

#[tokio::test]
async fn machines_items_carry_the_wire_fields() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/machines").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().expect("items 是数组");
    assert!(!items.is_empty());
    for item in items {
        for key in ["at", "user", "port", "ips", "note", "tags"] {
            assert!(item.get(key).is_some(), "机器缺 wire 字段 {key}：{item}");
        }
        // 旧字段作为附加字段保留一版。
        for key in ["name", "hostname", "os", "arch", "triples", "rustc", "roles", "annotations"] {
            assert!(item.get(key).is_some(), "旧字段 {key} 不该在这一版突然消失：{item}");
        }
    }
}

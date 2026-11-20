//! #68：`GET /v1/fs/dirs` 的只读目录浏览判据。
//!
//! 打真 Router + 真临时目录：测试的是客户端看得见的端点，不碰任何内部函数。
//! 目录里的东西由测试自己造，期望值写死在断言里（不拿实现同一套算法再算一遍）。

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_server::{router, AppState};
use serde_json::Value;
use tower::ServiceExt;

fn temp_dir(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-issue68-dirs-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

async fn get_dirs(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn make_fixture(root: &Path) {
    for name in ["zeta", "alpha", "middle"] {
        std::fs::create_dir(root.join(name)).unwrap();
    }
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::write(root.join("loose.txt"), "not a directory").unwrap();
    std::fs::write(root.join("another.md"), "still not").unwrap();
}

#[tokio::test]
async fn directory_listing_shows_only_visible_subdirectories() {
    let root = temp_dir("visible");
    make_fixture(&root);
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));

    let (status, body) = get_dirs(&app, &format!("/v1/fs/dirs?path={}", root.display())).await;

    assert_eq!(status, StatusCode::OK, "列出存在的绝对目录应 200：{body}");
    assert_eq!(body["path"], Value::String(root.to_string_lossy().into_owned()));
    assert_eq!(body["truncated"], Value::Bool(false));
    let dirs = body["dirs"].as_array().expect("dirs 是数组");
    let names: Vec<&str> = dirs.iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["alpha", "middle", "zeta"], "按名字稳定排序，只有 3 个可见子目录");
    for dir in dirs {
        let path = PathBuf::from(dir["path"].as_str().unwrap());
        assert!(path.is_dir(), "{} 必须是目录", path.display());
        assert_eq!(dir["hidden"], Value::Bool(false));
    }

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn hidden_directories_are_opt_in() {
    let root = temp_dir("hidden");
    make_fixture(&root);
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));

    let (_, body) = get_dirs(
        &app,
        &format!("/v1/fs/dirs?path={}&hidden=1", root.display()),
    )
    .await;
    let dirs = body["dirs"].as_array().expect("dirs 是数组");
    let names: Vec<&str> = dirs.iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec![".git", "alpha", "middle", "zeta"]);
    assert_eq!(dirs[0]["hidden"], Value::Bool(true));

    std::fs::remove_dir_all(root).unwrap();
}

/// 目标目录的完整指纹：mtime + 内容 sha256。只读端点不许动这两样。
fn fingerprint(root: &Path) -> (String, String) {
    let output = Command::new("find")
        .arg(root)
        .args(["-mindepth", "1", "-printf", "%P %T@\\n"])
        .output()
        .unwrap();
    let listing = String::from_utf8(output.stdout).unwrap();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    let mut digest = String::new();
    for path in paths {
        digest.push_str(&path.file_name().unwrap().to_string_lossy());
        digest.push(':');
        if path.is_dir() {
            for child in std::fs::read_dir(&path).unwrap() {
                digest.push_str(&child.unwrap().file_name().to_string_lossy());
            }
        } else {
            digest.push_str(&std::fs::read_to_string(&path).unwrap());
        }
        digest.push('\n');
    }
    (listing, digest)
}

#[tokio::test]
async fn browsing_a_directory_does_not_modify_it() {
    let root = temp_dir("readonly");
    make_fixture(&root);
    let before = fingerprint(&root);
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));

    let (status, _) = get_dirs(&app, &format!("/v1/fs/dirs?path={}&hidden=1", root.display())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fingerprint(&root), before, "浏览是只读的：mtime 与内容都不变");

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn relative_paths_and_missing_directories_are_distinct_errors() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));

    let (status, body) = get_dirs(&app, "/v1/fs/dirs?path=foo").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "相对路径 400：{body}");
    assert!(body["error"].is_string());

    let missing = temp_dir("missing").join("definitely-not-here");
    let (status, body) =
        get_dirs(&app, &format!("/v1/fs/dirs?path={}", missing.display())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "绝对路径不存在 404：{body}");
    assert!(body["error"].is_string(), "不许拿空数组冒充不存在");
}

/// 给非 root 子进程跑的探针测试：只有 NOUS_FS_DIRS_PROBE_PATH 设了才动作。
#[cfg(unix)]
#[test]
fn permission_probe_test() {
    let path = std::env::var("NOUS_FS_DIRS_PROBE_PATH").unwrap_or_default();
    permission_probe_main(&path);
}

/// 打真 Router 并把状态码打到 stdout（libtest 在 --nocapture 下能看见）。
/// NOUS_FS_DIRS_PROBE_PATH 不设时不带 `path=` 参数，走 NOUS_START_DIRS / cwd 缺省。
#[cfg(unix)]
fn permission_probe_main(path: &str) {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let uri = if path.is_empty() {
        "/v1/fs/dirs".to_string()
    } else {
        format!("/v1/fs/dirs?path={path}")
    };
    let (status, body) = rt.block_on(async move {
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    });
    println!("status={status} body={body}");
}

/// 权限判据用非 root 子进程实测：root 会穿透 000 目录，SKIP 等于没验。
#[cfg(unix)]
#[test]
fn unreadable_directory_is_forbidden_not_a_server_error() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_dir("denied");
    let secret = root.join("secret");
    std::fs::create_dir(&secret).unwrap();
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();

    // root 会穿透权限位，所以必须让 setpriv 把测试二进制降成 nobody 再跑请求。
    let exe = std::env::current_exe().unwrap();
    let result = Command::new("/usr/bin/setpriv")
        .args([
            "--reuid=65534",
            "--regid=65534",
            "--clear-groups",
        ])
        .arg(&exe)
        .arg("--exact")
        .arg("permission_probe_test")
        .arg("--nocapture")
        .env("NOUS_FS_DIRS_PROBE_PATH", &secret)
        .output()
        .unwrap();
    let output = format!(
        "rc={:?} stdout={} stderr={}",
        result.status.code(),
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.status.success(), "降权探针跑通：{output}");
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("status=403"),
        "权限不足必须 403 而不是 500：{output}"
    );

    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn more_than_five_hundred_subdirectories_are_truncated() {
    let root = temp_dir("many");
    for i in 0..501 {
        std::fs::create_dir(root.join(format!("d-{i:03}"))).unwrap();
    }
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));

    let (status, body) = get_dirs(&app, &format!("/v1/fs/dirs?path={}", root.display())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truncated"], Value::Bool(true), "501 个子目录必须报截断");
    let dirs = body["dirs"].as_array().expect("dirs 是数组");
    assert_eq!(dirs.len(), 500, "最多回 500 条");

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn the_local_machine_name_is_accepted() {
    let root = temp_dir("local-machine");
    make_fixture(&root);
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    if let Some(name) = nous_env::local_machine_name() {
        let (status, body) = get_dirs(
            &app,
            &format!("/v1/fs/dirs?path={}&machine={name}", root.display()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "本机 machine 应放行：{body}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn unknown_machine_is_404_not_501_or_local_fallback() {
    let root = temp_dir("unknown-machine");
    make_fixture(&root);
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));

    let (status, body) = get_dirs(
        &app,
        &format!("/v1/fs/dirs?path={}&machine=some-other-box", root.display()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "编目里没有的机器必须 404：{body}");
    assert!(body["error"].is_string(), "错误体是 {{error}}：{body}");

    std::fs::remove_dir_all(root).unwrap();
}

/// 缺省路径来自 `NOUS_START_DIRS` 的第一条；子进程隔离避免环境变量竞争。
#[cfg(unix)]
#[test]
fn default_start_dir_comes_from_nous_start_dirs() {
    let first = temp_dir("start-first");
    let second = temp_dir("start-second");
    make_fixture(&first);
    let exe = std::env::current_exe().unwrap();
    let result = Command::new(&exe)
        .arg("--exact")
        .arg("permission_probe_test")
        .arg("--nocapture")
        .env(
            "NOUS_START_DIRS",
            format!("{}:{}", first.display(), second.display()),
        )
        .env_remove("NOUS_FS_DIRS_PROBE_PATH")
        .output()
        .unwrap();
    let output = String::from_utf8_lossy(&result.stdout).into_owned();
    assert!(result.status.success(), "探针跑通：{output}");
    assert!(
        output.contains(&format!("\"path\":\"{}\"", first.display())),
        "缺省起点应是 NOUS_START_DIRS 的第一条：{output}"
    );
    assert!(
        output.contains("\"name\":\"alpha\""),
        "起点目录里的子目录要列出来：{output}"
    );

    std::fs::remove_dir_all(first).unwrap();
    std::fs::remove_dir_all(second).unwrap();
}

// 判据 6（只读）已在 browsing_a_directory_does_not_modify_it 里。

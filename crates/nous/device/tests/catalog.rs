//! #65 判据 3/4/5/6：模型编目与默认模型的公共接缝。
//!
//! 接缝：`nous_device::{catalog, load_models, save_default}` 与
//! `nous_device::ModelRef` / `ModelsResponse`。测试只通过公开 API 与真文件系统
//! （tempdir + 真 rename）观察行为，不碰内部辅助函数。

use std::path::Path;

use nous_device::{load_models, save_default, ModelRef};
use serde_json::json;

fn catalog_environment() -> nous_device::CatalogEnvironment {
    nous_device::CatalogEnvironment {
        inline: Some(
            json!({
                "default": { "provider": "env", "modelId": "env/model-a" },
                "items": [
                    { "provider": "env", "modelId": "env/model-a", "name": "A",
                      "thinkingLevels": ["off", "high"], "contextWindow": 100, "maxTokens": 10 },
                    { "provider": "env", "modelId": "env/model-b", "name": "B",
                      "thinkingLevels": ["off"], "contextWindow": null, "maxTokens": null }
                ]
            })
            .to_string(),
        ),
        file: None,
        file_path: "/var/lib/nous/models.json".into(),
    }
}

#[test]
fn inline_models_catalog_reports_every_item_and_ok_state() {
    let response = load_models(&catalog_environment()).expect("内联 JSON 可读");
    assert_eq!(response.items.len(), 2, "items 条数与内联 JSON 逐条相等");
    assert_eq!(response.items[0].model_id, "env/model-a");
    assert_eq!(response.items[1].name, "B");
    assert_eq!(response.catalog.state, "ok");
    assert_eq!(response.catalog.source.as_deref(), Some("env"));
}

#[test]
fn missing_models_catalog_is_honest_and_points_at_default_path() {
    let env = nous_device::CatalogEnvironment {
        inline: None,
        file: None,
        file_path: "/var/lib/nous/models.json".into(),
    };
    let response = load_models(&env).expect("没有编目不是错误");
    assert!(response.items.is_empty(), "诚实空表");
    assert_eq!(response.catalog.state, "missing");
    assert!(response.catalog.source.is_none());
    assert_eq!(response.catalog.path, Path::new("/var/lib/nous/models.json"));
}

#[test]
fn file_catalog_is_used_when_inline_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(
        &path,
        json!({
            "default": null,
            "items": [{
                "provider": "file", "modelId": "file/one", "name": "One",
                "thinkingLevels": [], "contextWindow": null, "maxTokens": null
            }]
        })
        .to_string(),
    )
    .unwrap();
    let env = nous_device::CatalogEnvironment {
        inline: None,
        file: Some(path.clone()),
        file_path: path.clone(),
    };
    let response = load_models(&env).unwrap();
    assert_eq!(response.items.len(), 1);
    assert_eq!(response.items[0].provider, "file");
    assert_eq!(response.catalog.state, "ok");
    assert_eq!(response.catalog.source.as_deref(), Some("file"));
    assert_eq!(response.catalog.path, path);
}

#[test]
fn writing_an_existing_default_survives_a_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    let body = json!({
        "default": null,
        "items": [{
            "provider": "file", "modelId": "file/one", "name": "One",
            "thinkingLevels": [], "contextWindow": null, "maxTokens": null
        }]
    })
    .to_string();
    std::fs::write(&path, &body).unwrap();

    let chosen = ModelRef { provider: "file".into(), model_id: "file/one".into() };
    let written = save_default(&path, &chosen).expect("写存在的模型应成功");
    assert_eq!(written, chosen);

    let env = nous_device::CatalogEnvironment {
        inline: None,
        file: Some(path),
        file_path: dir.path().join("models.json"),
    };
    let reloaded = load_models(&env).unwrap();
    assert_eq!(reloaded.default, Some(chosen));
}

#[test]
fn writing_a_default_outside_the_catalog_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    let body = json!({
        "default": null,
        "items": [{
            "provider": "file", "modelId": "file/one", "name": "One",
            "thinkingLevels": [], "contextWindow": null, "maxTokens": null
        }]
    })
    .to_string();
    std::fs::write(&path, &body).unwrap();

    let result = save_default(
        &path,
        &ModelRef { provider: "ghost".into(), model_id: "ghost/nope".into() },
    );
    assert!(result.is_err(), "不许写一个不在 items 里的默认模型");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), body, "拒绝时文件不变");
}

#[test]
fn atomic_write_keeps_original_bytes_and_mtime_without_rename() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    let original = json!({
        "default": null,
        "items": [{
            "provider": "file", "modelId": "file/one", "name": "One",
            "thinkingLevels": [], "contextWindow": null, "maxTokens": null
        }]
    })
    .to_string();
    std::fs::write(&path, &original).unwrap();
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();

    let staged = nous_device::stage_default(
        &path,
        &ModelRef { provider: "file".into(), model_id: "file/one".into() },
    )
    .unwrap();
    assert!(staged.exists(), "临时文件应写在目标旁边");
    assert_ne!(staged, path, "临时文件不是目标文件");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original, "rename 前目标不动");
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), before, "mtime 不变");

    // 模拟进程在 rename 前被杀：临时文件留着，目标依然是可读的原文。
    let env = nous_device::CatalogEnvironment {
        inline: None,
        file: Some(path.clone()),
        file_path: path,
    };
    let response = load_models(&env).unwrap();
    assert!(response.items[0].model_id == "file/one");
    assert!(response.default.is_none(), "没 rename 就不该看见新 default");
}

#[test]
fn machines_catalog_uses_the_file_when_given_and_builtin_otherwise() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("machines.json");
    std::fs::write(
        &path,
        json!({ "items": [{ "name": "from-file", "hostname": "box", "triples": ["x"] }] })
            .to_string(),
    )
    .unwrap();
    let from_file = nous_device::machines(Some(&path));
    assert_eq!(from_file["items"][0]["name"], json!("from-file"));

    let builtin = nous_device::machines(Some(&dir.path().join("absent.json")));
    let names = builtin["items"].as_array().unwrap();
    assert_eq!(names.len(), 2);
    assert_eq!(names[0]["name"], json!("runtime-1"));
    assert_eq!(names[1]["name"], json!("asus"));
}

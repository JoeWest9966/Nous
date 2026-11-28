//! `GET /v1/modules/updates`：把「这个二进制里各模块的版本」与「仓库里各模块的版本」
//! 并排放出来，**让客户端自己决定怎么说「落后」**。
//!
//! ## 这条端点回答什么，不回答什么
//!
//! - `current`：**编译期内嵌**（`build.rs` 把每份 `plugin.toml` 的 `version` 抄进
//!   `PLUGIN_VERSIONS`）。它说的是「这个产物装着什么」，不是「盘上此刻有什么」——
//!   和 `/v1/modules/graph` 同一个理由（运行期读盘会多出第二份真相）。
//!   清单里没写版本 → `current: null` + `currentSource: "release"`，**不拿发行版号冒充**：
//!   那会把「某个模块落后」说成「引擎落后」。
//! - `latest`：仓库 `main` 上那份 `plugin-versions.json`（生成物，`scripts/plugin-versions.py`
//!   产出、`verify.sh` 校验不漂）。**一次请求拿全**：per-module 打 GitHub API 是 28 次请求，
//!   未认证限流 60 次/小时，进一次页面就打满。
//!
//! **这里不做 semver 比较、不出现 `outdated` 之类的状态位。** 比较是展示层的纯函数
//! （预发布、build metadata、非 semver 字符串都得处理），同一件事两个算法迟早分叉。
//!
//! ## 失败怎么说话
//!
//! 上游挂了**永远 200**（5xx 会让客户端当成「端点坏了」），靠 `available` / `stale` /
//! `error` 三个字段说清楚：
//!
//! | 情形 | `available` | `stale` | `error` |
//! |---|---|---|---|
//! | 刚拉到的新数据 | true | false | null |
//! | 有缓存但这次没拉到（超时 / 5xx / 解析失败） | true | true | 一句人话 |
//! | 从没拉到过 | false | false | 一句人话 |
//!
//! `available: false` **不等于**「全都是最新」——客户端必须显示「检查不可用」。
//!
//! 缓存：内存 + 盘（`NOUS_DATA/updates/plugin-versions.json`，没配 `NOUS_DATA` 就只在内存），
//! TTL 6 小时，`?refresh=1` 穿透；带 ETag，重编 304 不重新解析。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::Value;

use crate::{PLUGIN_VERSIONS, RELEASE};

/// 上游清单的默认地址（仓库 `main` 上的生成物）。可用 `NOUS_UPDATES_SOURCE` 覆盖
/// （内网镜像 / 自建清单 / 测试假源）。
pub const DEFAULT_SOURCE: &str =
    "https://raw.githubusercontent.com/JoeWest9966/Nous/main/plugin-versions.json";

/// 缓存多久算新鲜。`?refresh=1` 不看它。
pub const DEFAULT_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// 上游超时。**3 秒是硬上限**：这是挂在设置页上的交互，不能让一次外网往返把它拖住。
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(3);

/// 一次调用的配置（全部来自环境，进来可注入 —— 测试不打真网）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub source: String,
    pub token: Option<String>,
    /// 盘上缓存的位置；`None` = 只在内存里缓存（没配 `NOUS_DATA` 的开发机）。
    pub cache_path: Option<PathBuf>,
    pub ttl: Duration,
}

impl Config {
    pub fn from_env() -> Self {
        let source = std::env::var("NOUS_UPDATES_SOURCE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_SOURCE.to_string());
        let token = std::env::var("NOUS_UPDATES_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let cache_path = std::env::var_os("NOUS_DATA")
            .map(PathBuf::from)
            .map(|root| root.join("updates").join("plugin-versions.json"));
        Self {
            source,
            token,
            cache_path,
            ttl: DEFAULT_TTL,
        }
    }
}

/// 上游清单里的一条。只取用得上的两个字段，其余（`crate` 等）原样不读也不改。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ManifestModule {
    pub name: String,
    #[serde(default)]
    pub version: String,
}

/// 上游清单（`plugin-versions.json`）。`modules` 缺失 = 空表（不 panic、不猜）。
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Manifest {
    #[serde(default)]
    pub release: String,
    #[serde(default)]
    pub modules: Vec<ManifestModule>,
}

impl Manifest {
    fn version_of(&self, module: &str) -> Option<&str> {
        self.modules
            .iter()
            .find(|item| item.name == module)
            .map(|item| item.version.as_str())
            .filter(|version| !version.is_empty())
    }

    fn names(&self) -> Vec<&str> {
        self.modules.iter().map(|item| item.name.as_str()).collect()
    }
}

/// 一次成功拉取的结果 + 它的元信息。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Cached {
    /// `source` 变了就作废：缓存不能跨源用。
    source: String,
    manifest: Manifest,
    etag: Option<String>,
    fetched_at_ms: i64,
}

#[derive(Debug, Default)]
struct State {
    cached: Option<Cached>,
    /// 盘上缓存只读一次；读不到也读过了。
    loaded: bool,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .build()
            .unwrap_or_default()
    })
}

/// `?refresh=1` 算；别的都不算（不猜）。
pub fn wants_refresh(raw: Option<&str>) -> bool {
    matches!(raw.map(str::trim), Some("1"))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// 这条端点的 THIN handler：读环境、拿全局客户端、打 [`handle_with`]。
pub async fn handle(refresh: bool) -> Value {
    handle_with(&Config::from_env(), client(), refresh).await
}

/// 可注入配置与客户端的入口（测试用本地假源 / 自定义 TTL）。
pub async fn handle_with(config: &Config, http: &reqwest::Client, refresh: bool) -> Value {
    load_disk(config);

    let cached = {
        let guard = lock();
        guard
            .cached
            .as_ref()
            .filter(|cached| cached.source == config.source)
            .cloned()
    };

    if let Some(cached) = &cached {
        if !refresh && is_fresh(cached.fetched_at_ms, config.ttl, now_ms()) {
            return compose(Some(cached), None, false, config);
        }
    }

    match fetch(config, http, cached.as_ref()).await {
        Ok(Fetched::NotModified) => {
            let mut guard = lock();
            let Some(entry) = guard.cached.as_mut() else {
                // 304 但没有缓存可续 = 上游说「没变」，我们却说「没见过」：
                // 只能当失败处理（不编数据）。
                return compose(
                    None,
                    Some("上游回 304，但没有可用的缓存".to_string()),
                    false,
                    config,
                );
            };
            entry.fetched_at_ms = now_ms();
            let entry = entry.clone();
            drop(guard);
            write_disk(config, &entry);
            compose(Some(&entry), None, false, config)
        }
        Ok(Fetched::Body(manifest, etag)) => {
            let entry = Cached {
                source: config.source.clone(),
                manifest,
                etag,
                fetched_at_ms: now_ms(),
            };
            {
                let mut guard = lock();
                guard.cached = Some(entry.clone());
            }
            write_disk(config, &entry);
            compose(Some(&entry), None, false, config)
        }
        Err(error) => {
            // 有旧数据就戴着「旧」的标签继续给（stale），没有就老实说不可用。
            let stale = cached.is_some();
            compose(cached.as_ref(), Some(error), stale, config)
        }
    }
}

enum Fetched {
    /// 304：缓存仍有效。
    NotModified,
    Body(Manifest, Option<String>),
}

async fn fetch(
    config: &Config,
    http: &reqwest::Client,
    cached: Option<&Cached>,
) -> Result<Fetched, String> {
    let mut request = http.get(&config.source);
    if let Some(etag) = cached
        .filter(|cached| cached.source == config.source)
        .and_then(|cached| cached.etag.as_deref())
    {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    if let Some(token) = config.token.as_deref() {
        request = request.bearer_auth(token);
    }
    // 错误文案只给人看「为什么没拿到」：地址在 `source` 字段里已经有了，不要把 URL
    // 与 reqwest 的英文原文塞进界面（真机上那一行会折成两行还读不懂）。
    let response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            "连不上上游：超时".to_string()
        } else if error.is_connect() {
            "连不上上游：连接被拒或域名解析不了".to_string()
        } else {
            "连不上上游".to_string()
        }
    })?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    if !status.is_success() {
        return Err(match status.as_u16() {
            404 => "上游没有这份清单（HTTP 404）".to_string(),
            401 | 403 => format!("上游拒绝了这次请求（HTTP {}）", status.as_u16()),
            code => format!("上游回了 HTTP {code}"),
        });
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let text = response
        .text()
        .await
        .map_err(|_| "上游的清单没读完（连接断了）".to_string())?;
    let manifest: Manifest = serde_json::from_str(&text)
        .map_err(|_| "上游回的不是清单形状（JSON 解析不了）".to_string())?;
    Ok(Fetched::Body(manifest, etag))
}

fn is_fresh(fetched_at_ms: i64, ttl: Duration, now_ms: i64) -> bool {
    // 时钟往回跳（NTP / 手工改时间）时宁可重拉，也不要「一直新鲜」。
    now_ms >= fetched_at_ms && (now_ms - fetched_at_ms) < ttl.as_millis() as i64
}

/// 组装回包。**纯函数**：同样的入参永远给同样的 JSON（键序也稳定，便于快照比对）。
fn compose(cached: Option<&Cached>, error: Option<String>, stale: bool, config: &Config) -> Value {
    let manifest = cached.map(|cached| &cached.manifest);
    let names: Vec<&str> = manifest.map(Manifest::names).unwrap_or_default();

    let mut modules: Vec<Value> = Vec::with_capacity(PLUGIN_VERSIONS.len());
    let mut missing: Vec<&str> = Vec::new();
    for (name, version) in PLUGIN_VERSIONS {
        let own = (!version.is_empty()).then_some(*version);
        let latest = manifest.and_then(|manifest| manifest.version_of(name));
        // 上游整份都没拿到时 `missing` 必须空：「清单里没有它」只有在**有清单**时才有意义，
        // 否则一次没网就会把 31 个模块全说成「不在清单里」（真机验出来的）。
        if manifest.is_some() && latest.is_none() && !version.is_empty() {
            missing.push(name);
        }
        modules.push(json!({
            "name": name,
            "current": own,
            "currentSource": if own.is_some() { "module" } else { "release" },
            "latest": latest,
        }));
    }

    // 仓库里有、这个产物没有的模块：说明引擎比仓库旧（比「某模块落后」更严重）。
    let only_in_manifest: Vec<&str> = names
        .into_iter()
        .filter(|name| !PLUGIN_VERSIONS.iter().any(|(ours, _)| ours == name))
        .collect();

    json!({
        "available": manifest.is_some(),
        "checkedAt": cached.map(|cached| cached.fetched_at_ms),
        "source": config.source,
        "stale": stale,
        "server": {
            "current": RELEASE,
            "latest": manifest
                .map(|manifest| manifest.release.clone())
                .filter(|release| !release.is_empty()),
        },
        "modules": modules,
        "missing": missing,
        "onlyInManifest": only_in_manifest,
        "error": error,
    })
}

/// 盘上缓存的位置：`cache_path` 的兄弟文件 `meta.json` 存 `source` / `etag` / `fetchedAt`。
fn meta_path(cache_path: &std::path::Path) -> PathBuf {
    cache_path.with_file_name("meta.json")
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
struct DiskMeta {
    #[serde(default)]
    source: String,
    #[serde(default)]
    etag: Option<String>,
    #[serde(default, rename = "fetchedAtMs")]
    fetched_at_ms: i64,
}

/// 测试用：把内存缓存清空（盘上那份不动）。生产路径不需要它 —— 换源会自动作废。
pub fn reset_cache() {
    let mut guard = lock();
    guard.cached = None;
    guard.loaded = false;
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 第一次调用时把盘上缓存读进内存。**读不到不是错误**：只是「没有缓存」。
fn load_disk(config: &Config) {
    {
        let guard = lock();
        if guard.loaded {
            return;
        }
    }
    let loaded = config.cache_path.as_ref().and_then(|path| {
        let manifest: Manifest = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        let meta: DiskMeta =
            serde_json::from_str(&std::fs::read_to_string(meta_path(path)).ok()?).ok()?;
        // 源对不上等于没缓存（缓存不能跨源用）。
        (meta.source == config.source).then_some(Cached {
            source: meta.source,
            manifest,
            etag: meta.etag,
            fetched_at_ms: meta.fetched_at_ms,
        })
    });
    let mut guard = lock();
    guard.loaded = true;
    if let Some(cached) = loaded {
        if guard.cached.is_none() {
            guard.cached = Some(cached);
        }
    }
}

/// 落盘缓存。**失败只说明**，不影响这次的回包（缓存是加速，不是真相）。
fn write_disk(config: &Config, cached: &Cached) {
    let Some(path) = config.cache_path.as_ref() else {
        return;
    };
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(&cached.manifest).map_err(std::io::Error::other)?;
        std::fs::write(path, body + "\n")?;
        let meta = json!({
            "source": cached.source,
            "etag": cached.etag,
            "fetchedAtMs": cached.fetched_at_ms,
        });
        std::fs::write(
            meta_path(path),
            serde_json::to_string_pretty(&meta).map_err(std::io::Error::other)? + "\n",
        )
    };
    if let Err(error) = write() {
        eprintln!(
            "nous-relationship-network: 写插件版本缓存 {} 失败：{error}（下次仍然会重新拉）",
            path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(pairs: &[(&str, &str)]) -> Manifest {
        Manifest {
            release: "9.9.9".into(),
            modules: pairs
                .iter()
                .map(|(name, version)| ManifestModule {
                    name: name.to_string(),
                    version: version.to_string(),
                })
                .collect(),
        }
    }

    fn config() -> Config {
        Config {
            source: "https://example.invalid/plugin-versions.json".into(),
            token: None,
            cache_path: None,
            ttl: DEFAULT_TTL,
        }
    }

    fn cached(manifest: Manifest) -> Cached {
        Cached {
            source: config().source,
            manifest,
            etag: None,
            fetched_at_ms: 1_700_000_000_000,
        }
    }

    fn module<'a>(body: &'a Value, name: &str) -> &'a Value {
        body["modules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == name)
            .unwrap_or_else(|| panic!("{name} 不在 modules 里"))
    }

    /// 有上游数据：current 用自己的版本，latest 用清单的；两列都说得清楚。
    #[test]
    fn compose_pairs_current_with_latest() {
        let one = cached(manifest(&[(crate::MODULE, "0.2.0")]));
        let body = compose(Some(&one), None, false, &config());
        assert_eq!(body["available"], json!(true));
        assert_eq!(body["stale"], json!(false));
        assert_eq!(body["error"], Value::Null);
        assert_eq!(body["checkedAt"], json!(1_700_000_000_000u64));
        assert_eq!(body["server"]["latest"], json!("9.9.9"));
        assert_eq!(body["server"]["current"], json!(RELEASE));
        let ours = module(&body, crate::MODULE);
        let declared = PLUGIN_VERSIONS
            .iter()
            .find(|(name, _)| *name == crate::MODULE)
            .map(|(_, version)| *version)
            .expect("自己在版本表里");
        assert_eq!(ours["current"], json!(declared));
        assert_eq!(ours["currentSource"], json!("module"));
        assert_eq!(ours["latest"], json!("0.2.0"));
    }

    /// 没拉到过上游：`available:false` —— 不是「全都是最新」。
    #[test]
    fn nothing_fetched_is_unavailable_not_uptodate() {
        let body = compose(None, Some("超时".into()), false, &config());
        assert_eq!(body["available"], json!(false));
        assert_eq!(body["checkedAt"], Value::Null);
        assert_eq!(body["error"], json!("超时"));
        assert_eq!(
            body["modules"].as_array().unwrap().len(),
            PLUGIN_VERSIONS.len()
        );
        for item in body["modules"].as_array().unwrap() {
            assert_eq!(item["latest"], Value::Null, "没有上游就没有 latest");
        }
        assert_eq!(
            body["missing"],
            json!([]),
            "上游整份没拿到时不许把每个模块都说成「清单里没有」"
        );
        assert_eq!(body["onlyInManifest"], json!([]));
    }

    /// 有缓存但这次失败：`stale:true`，数据照给（旧总比没有强，但必须标出来）。
    #[test]
    fn stale_cache_is_served_but_marked() {
        let one = cached(manifest(&[(crate::MODULE, "0.2.0")]));
        let body = compose(Some(&one), Some("HTTP 500".into()), true, &config());
        assert_eq!(body["available"], json!(true));
        assert_eq!(body["stale"], json!(true));
        assert_eq!(body["error"], json!("HTTP 500"));
        assert_eq!(body["checkedAt"], json!(1_700_000_000_000u64));
    }

    /// 清单里少一个模块：`latest:null` 且进 `missing`，不许静默当「最新」。
    #[test]
    fn missing_modules_are_listed() {
        let one = cached(manifest(&[(crate::MODULE, "0.2.0")]));
        let body = compose(Some(&one), None, false, &config());
        let missing: Vec<&str> = body["missing"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item.as_str().unwrap())
            .collect();
        assert!(!missing.contains(&crate::MODULE), "清单里有它");
        assert_eq!(
            missing.len(),
            PLUGIN_VERSIONS
                .iter()
                .filter(|(_, v)| !v.is_empty())
                .count()
                - 1,
            "除了唯一对上的那个，其余都得进 missing"
        );
    }

    /// 仓库里有、产物里没有的模块 → `onlyInManifest`（引擎比仓库旧）。
    #[test]
    fn modules_only_in_manifest_are_named() {
        let one = cached(manifest(&[
            (crate::MODULE, "0.2.0"),
            ("Brand_New", "0.1.0"),
        ]));
        let body = compose(Some(&one), None, false, &config());
        assert_eq!(body["onlyInManifest"], json!(["Brand_New"]));
    }

    /// 版本为空的模块：`current:null` + `currentSource:"release"`，**不拿发行版号冒充**。
    #[test]
    fn modules_without_own_version_do_not_fake_one() {
        let one = cached(Manifest::default());
        let body = compose(Some(&one), None, false, &config());
        for item in body["modules"].as_array().unwrap() {
            if item["current"].is_null() {
                assert_eq!(item["currentSource"], json!("release"));
            }
        }
    }

    #[test]
    fn freshness_window_and_clock_rollback() {
        let ttl = Duration::from_secs(60);
        assert!(is_fresh(1_000, ttl, 30_000));
        assert!(!is_fresh(1_000, ttl, 61_001));
        assert!(!is_fresh(1_000, ttl, 0), "时钟往回跳要重拉");
    }

    #[test]
    fn refresh_flag_is_explicit() {
        assert!(wants_refresh(Some("1")));
        assert!(wants_refresh(Some(" 1 ")));
        assert!(!wants_refresh(Some("true")));
        assert!(!wants_refresh(Some("yes")));
        assert!(!wants_refresh(Some("0")));
        assert!(!wants_refresh(Some("no")));
        assert!(!wants_refresh(None));
    }

    /// 盘上缓存：写得进、读得回；`source` 对不上就当没有。
    #[test]
    fn disk_cache_roundtrip_checks_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("updates").join("plugin-versions.json");
        let mut config = config();
        config.cache_path = Some(path.clone());
        let entry = Cached {
            source: config.source.clone(),
            manifest: manifest(&[(crate::MODULE, "0.2.0")]),
            etag: Some("\"abc\"".into()),
            fetched_at_ms: 42,
        };
        write_disk(&config, &entry);
        assert!(path.is_file(), "缓存文件写出来了");
        assert!(path.with_file_name("meta.json").is_file(), "元信息也写了");

        // 换个源：`load_disk` 必须丢掉它。
        {
            let mut guard = lock();
            guard.cached = None;
            guard.loaded = false;
        }
        let mut other = config.clone();
        other.source = "https://example.invalid/other.json".into();
        load_disk(&other);
        assert!(lock().cached.is_none(), "缓存不许跨源");

        // 同一个源：读得回来。
        {
            let mut guard = lock();
            guard.cached = None;
            guard.loaded = false;
        }
        load_disk(&config);
        let guard = lock();
        let got = guard.cached.as_ref().expect("读回来了");
        assert_eq!(got.manifest, entry.manifest);
        assert_eq!(got.etag, entry.etag);
        assert_eq!(got.fetched_at_ms, 42);
    }

    /// 没有 `cache_path` 时不写盘、不报错（开发机 / 测试）。
    #[test]
    fn no_cache_path_is_silent() {
        let config = Config {
            cache_path: None,
            ..config()
        };
        write_disk(&config, &cached(Manifest::default()));
    }

    /// 清单形状：`modules` 缺失 = 空表；未知字段不挡解析。
    #[test]
    fn manifest_tolerates_missing_and_unknown_fields() {
        let empty: Manifest = serde_json::from_str(r#"{"release":"0.1.0"}"#).unwrap();
        assert!(empty.modules.is_empty());
        let full: Manifest = serde_json::from_str(
            r#"{"release":"0.1.0","generatedAt":"x","modules":[
                 {"name":"Device","crate":"nous-device","version":"0.11.0","extra":true}]}"#,
        )
        .unwrap();
        assert_eq!(full.version_of("Device"), Some("0.11.0"));
        assert_eq!(full.version_of("Skill"), None);
    }
}

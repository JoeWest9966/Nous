//! 置顶的**写穿**客户端：真相在 bjes6b（手机端）的 `data/pins.json`。
//!
//! 桌面端**不存一份**置顶表（`decisions.md`：宁可多一次 HTTP，也不要两份互不相干的
//! 置顶表）。所以这里只做两件事：
//!
//! ```text
//! GET  /api/sessions           → items[] 里每条带 pinned（服务端已排好：置顶在前）
//! POST /api/sessions/:id/pin   → {pinned: true|false} → {id, pinned, changed}
//! ```
//!
//! 跟 `EngineClient` 的分工：那个连 qrd3wh（会话/历史/流），这个连 bjes6b（只有置顶）。
//! 两个地址都来自宿主的 `__nous/resolve`，不写进配置、不写进常量。

use std::time::Duration;

use serde::Deserialize;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum PinError {
    #[error("网络：{0}")]
    Http(#[from] reqwest::Error),
    #[error("URL 拼不起来：{0}")]
    Url(#[from] url::ParseError),
    #[error("手机端那块回 {status}：{message}")]
    Engine { status: u16, message: String },
}

/// `GET /api/sessions` 的整份清单。我们只用 `id` + `pinned`，别的字段原样透传。
#[derive(Debug, Clone, Deserialize)]
pub struct PinSessions {
    pub items: Vec<PinSession>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PinSession {
    pub id: i64,
    /// 缺了当 `false`：手机端哪天不发这个字段，也不该整份清单解析失败。
    #[serde(default)]
    pub pinned: bool,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct PinClient {
    http: reqwest::Client,
    /// bjes6b 的地址，保证以 `/` 结尾。来源只能是 `__nous/resolve`。
    base: Url,
}

impl PinClient {
    pub fn new(base: &str, timeout: Duration) -> Result<Self, PinError> {
        let mut url = Url::parse(base)?;
        if !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        let http = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self { http, base: url })
    }

    fn url(&self, path: &str) -> Result<Url, PinError> {
        Ok(self.base.join(path.trim_start_matches('/'))?)
    }

    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        resp: reqwest::Response,
    ) -> Result<T, PinError> {
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(PinError::Engine {
                status: status.as_u16(),
                message: text.chars().take(200).collect(),
            });
        }
        serde_json::from_str(&text).map_err(|e| PinError::Engine {
            status: status.as_u16(),
            message: format!("JSON 不像话：{e}"),
        })
    }

    /// 置顶的会话 id，**数组顺序就是显示顺序**（服务端已经按 pins.json 排好了）。
    pub async fn pinned_ids(&self) -> Result<Vec<i64>, PinError> {
        let resp = self.http.get(self.url("api/sessions")?).send().await?;
        let parsed: PinSessions = self.json(resp).await?;
        Ok(parsed
            .items
            .into_iter()
            .filter(|s| s.pinned)
            .map(|s| s.id)
            .collect())
    }

    /// 写穿：置顶 / 取消置顶。服务端回 `{id, pinned, changed}`，我们不依赖 body。
    pub async fn set_pin(&self, id: i64, pinned: bool) -> Result<(), PinError> {
        #[derive(serde::Serialize)]
        struct Body {
            pinned: bool,
        }
        let resp = self
            .http
            .post(self.url(&format!("api/sessions/{id}/pin"))?)
            .json(&Body { pinned })
            .send()
            .await?;
        let _: serde_json::Value = self.json(resp).await?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn 只挑出置顶的_并保持服务端顺序() {
        let raw = r#"{"items":[
            {"id":710,"pinned":true},
            {"id":496,"pinned":true},
            {"id":621,"pinned":false}
        ]}"#;
        let parsed: PinSessions = serde_json::from_str(raw).unwrap();
        let ids: Vec<i64> = parsed
            .items
            .into_iter()
            .filter(|s| s.pinned)
            .map(|s| s.id)
            .collect();
        // 顺序 = pins.json 里的顺序（新的在最前），不是按 id 大小。
        assert_eq!(ids, vec![710, 496]);
    }

    #[test]
    fn 少字段的条目也能解析() {
        // 手机端加字段不该让我们编译失败；反过来，`pinned` 缺了当 false。
        let raw = r#"{"items":[{"id":1,"pinned":true},{"id":2}]}"#;
        let parsed: PinSessions = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.items.len(), 2);
        assert!(!parsed.items[1].pinned);
    }
}

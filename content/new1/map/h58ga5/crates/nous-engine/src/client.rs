//! `EngineClient`：qrd3wh 的哑 HTTP 客户端。
//!
//! 这一层**不加业务字段、不算状态**。引擎回什么就递什么，错误原样带出来。

use std::time::Duration;

use futures_util::Stream;
use serde::Serialize;
use url::Url;

use crate::sse::{SseError, SseEvent};
use crate::types::{
    ConfigPatch, CreateSession, CreatedSession, Device, Dirs, Entries, Forked, Machines, Models,
    Presets, Prompts, ReadReceipt, SessionConfig, Sessions, Skills, Submission, Unread,
};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("网络：{0}")]
    Http(#[from] reqwest::Error),
    #[error("URL 拼不起来：{0}")]
    Url(#[from] url::ParseError),
    #[error("引擎 {status}：{message}")]
    Engine { status: u16, message: String },
}

#[derive(Debug, Clone)]
pub struct EngineClient {
    http: reqwest::Client,
    /// **SSE 专用**：长连接绝不能挂「总超时」。
    ///
    /// 踩过：SSE 和普通请求共用一个 `timeout(10s)` 的 client，回答超过 10 秒时 reqwest
    /// 直接把响应体掐了，客户端看到的是 `HTTP 流断了：error decoding response body` ——
    /// 长回答必死，而且看起来像「引擎挂了」。这条流只掐「连不上」和「一直不来数据」。
    sse: reqwest::Client,
    /// 引擎地址，保证以 `/` 结尾。来源只能是宿主的 `__nous/resolve`，别自己拼。
    base: Url,
}

impl EngineClient {
    pub fn new(base: &str, timeout: Duration) -> Result<Self, EngineError> {
        let mut url = Url::parse(base)?;
        if !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        let http = reqwest::Client::builder().timeout(timeout).build()?;
        let sse = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            // 空闲读超时：只有「连着但一直没数据」才算坏。有心跳就永远不会触发。
            .read_timeout(Duration::from_secs(180))
            .build()?;
        Ok(Self {
            http,
            sse,
            base: url,
        })
    }

    fn url(&self, path: &str) -> Result<Url, EngineError> {
        Ok(self.base.join(path.trim_start_matches('/'))?)
    }

    /// 引擎那句 `{error}` 原样带出来，不包、不美化。
    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        resp: reqwest::Response,
    ) -> Result<T, EngineError> {
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            let message = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_owned))
                .unwrap_or_else(|| text.chars().take(200).collect());
            return Err(EngineError::Engine {
                status: status.as_u16(),
                message,
            });
        }
        serde_json::from_str(&text).map_err(|e| EngineError::Engine {
            status: status.as_u16(),
            message: format!("JSON 不像话：{e}"),
        })
    }

    pub async fn device(&self) -> Result<Device, EngineError> {
        let resp = self.http.get(self.url("v1/device")?).send().await?;
        self.json(resp).await
    }

    // ── 建会话那一页要的几份清单 ────────────────────────────────────────
    // 一个端点一个小方法，不聚合：哪一份挂了不该把其它几份一起拖下水。

    /// 模型清单 + 盘上"那只眼"当前指着哪个（`default`）。
    pub async fn models(&self) -> Result<Models, EngineError> {
        let resp = self.http.get(self.url("v1/models")?).send().await?;
        self.json(resp).await
    }

    /// 已装技能（仓库版，带 `tracked` / `load.state`）。
    pub async fn skills(&self) -> Result<Skills, EngineError> {
        let resp = self.http.get(self.url("v1/skills")?).send().await?;
        self.json(resp).await
    }

    /// 提示词模板（每轮都带的那份规则）+ 仓库默认值。
    pub async fn prompts(&self) -> Result<Prompts, EngineError> {
        let resp = self.http.get(self.url("v1/prompts")?).send().await?;
        self.json(resp).await
    }

    /// 会话能跑在哪几台机器上。`None` = 本机。
    pub async fn machines(&self) -> Result<Machines, EngineError> {
        let resp = self.http.get(self.url("v1/machines")?).send().await?;
        self.json(resp).await
    }

    /// 列目录。`path` 为 `None` = 用引擎给的起点（board 的 map 根）。
    ///
    /// 浏览发生在**引擎进程**里，这边只收发路径字符串 —— 以后引擎跑在远程机器上，
    /// 这一页列的就是那台机器的目录。
    pub async fn dirs(&self, path: Option<&str>) -> Result<Dirs, EngineError> {
        let mut url = self.url("v1/fs/dirs")?;
        if let Some(path) = path.filter(|p| !p.is_empty()) {
            url.query_pairs_mut().append_pair("path", path);
        }
        let resp = self.http.get(url).send().await?;
        self.json(resp).await
    }

    /// 会话树。`parent != null` 的是 fork，显示不显示由 UI 决定，这里全给。
    pub async fn sessions(&self) -> Result<Sessions, EngineError> {
        let resp = self.http.get(self.url("v1/sessions")?).send().await?;
        self.json(resp).await
    }

    /// 历史。**新的在前**，调用方自己翻正。
    pub async fn entries(&self, id: i64) -> Result<Entries, EngineError> {
        let resp = self
            .http
            .get(self.url(&format!("v1/sessions/{id}/entries"))?)
            .send()
            .await?;
        self.json(resp).await
    }

    /// 每条会话的未读数，平铺的 map。跟 [`Self::sessions`] 一起拉，别一会拉一个。
    pub async fn unread(&self) -> Result<Unread, EngineError> {
        let resp = self.http.get(self.url("v1/unread")?).send().await?;
        self.json(resp).await
    }

    /// 把这条会话标成读到最新。水位只进不退，引擎那边钉住了。
    ///
    /// 别自己推水位、也别只把「已读」写本地了事 —— 重装客户端不该丢未读。
    pub async fn mark_read(&self, id: i64) -> Result<ReadReceipt, EngineError> {
        let resp = self
            .http
            .post(self.url(&format!("v1/read/{id}"))?)
            .send()
            .await?;
        self.json(resp).await
    }

    /// 从这条会话分叉一条新的。`entry_id` 为 `None` = 从最后一条分叉。
    pub async fn fork(
        &self,
        id: i64,
        entry_id: Option<i64>,
        name: Option<&str>,
    ) -> Result<Forked, EngineError> {
        #[derive(Serialize)]
        struct Body<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            entry_id: Option<i64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            name: Option<&'a str>,
        }
        let resp = self
            .http
            .post(self.url(&format!("v1/sessions/{id}/fork"))?)
            .json(&Body { entry_id, name })
            .send()
            .await?;
        self.json(resp).await
    }

    /// 发一条。`request_id` 是幂等锚，桌面端**必须传**（重发不会答两遍）。
    pub async fn send_message(
        &self,
        id: i64,
        content: &str,
        request_id: &str,
    ) -> Result<Submission, EngineError> {
        #[derive(Serialize)]
        struct Body<'a> {
            content: &'a str,
            request_id: &'a str,
        }
        let resp = self
            .http
            .post(self.url(&format!("v1/sessions/{id}/messages"))?)
            .json(&Body {
                content,
                request_id,
            })
            .send()
            .await?;
        self.json(resp).await
    }

    pub async fn abort(&self, id: i64) -> Result<serde_json::Value, EngineError> {
        let resp = self
            .http
            .post(self.url(&format!("v1/sessions/{id}/abort"))?)
            .send()
            .await?;
        self.json(resp).await
    }

    pub async fn create(&self, body: &CreateSession) -> Result<CreatedSession, EngineError> {
        let resp = self
            .http
            .post(self.url("v1/sessions")?)
            .json(body)
            .send()
            .await?;
        self.json(resp).await
    }

    /// 改**这条会话**的配置。`skills` 传的是完整目标数组（服务端自己算相对改法：
    /// 读当前 extension 名牌 → remove 旧的 / add 新的），不要在本地模拟增减。
    ///
    /// 回包是生效值，调用方拿它覆盖本地会话的 `skills`，别自己乐观地假定写成功了。
    /// 改这条会话的配置。回的是**生效值**，不是请求值 —— 调用方要拿它覆盖本地，
    /// 而不是自己把改的值写上去（换了模型，服务端会把档位按新模型重夹）。
    pub async fn config(&self, id: i64, patch: &ConfigPatch) -> Result<SessionConfig, EngineError> {
        let resp = self
            .http
            .post(self.url(&format!("v1/sessions/{id}/config"))?)
            .json(patch)
            .send()
            .await?;
        self.json(resp).await
    }

    pub async fn presets(&self, section: &str) -> Result<Presets, EngineError> {
        let resp = self
            .http
            .get(self.url(&format!("v1/presets/{section}"))?)
            .send()
            .await?;
        self.json(resp).await
    }

    pub async fn selection(&self) -> Result<serde_json::Value, EngineError> {
        let resp = self.http.get(self.url("v1/selection")?).send().await?;
        self.json(resp).await
    }

    pub async fn set_selection(&self, id: Option<i64>) -> Result<serde_json::Value, EngineError> {
        #[derive(Serialize)]
        struct Body {
            id: Option<i64>,
        }
        let resp = self
            .http
            .post(self.url("v1/selection")?)
            .json(&Body { id })
            .send()
            .await?;
        self.json(resp).await
    }

    /// SSE。拿到的是解析好的事件流，合帧是 UI 层的事。
    pub async fn events(
        &self,
        id: i64,
    ) -> Result<impl Stream<Item = Result<SseEvent, SseError>>, EngineError> {
        let resp = self
            .sse
            .get(self.url(&format!("v1/sessions/{id}/events"))?)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await?;
            return Err(EngineError::Engine {
                status,
                message: text.chars().take(200).collect(),
            });
        }
        Ok(crate::sse::parse(resp.bytes_stream()))
    }
}

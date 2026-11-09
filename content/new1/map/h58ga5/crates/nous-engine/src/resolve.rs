//! 从宿主 `__nous/resolve` 拿引擎地址。
//!
//! 这是「地址只有一个来源」的唯一实现：引擎地址永远不写进配置 / 常量，
//! 只从宿主的 resolve 接口现拿。搬走、换域名、进容器，这里一行不改。

use url::Url;

use crate::client::EngineError;

#[derive(Debug, serde::Deserialize)]
struct ResolveResp {
    url: String,
}

/// `host` 是宿主地址（可带路径前缀，如 `https://…/nous`），
/// `board` 是盘名，`from_block` 是我们自己（h58ga5），`engine_block` 是引擎（qrd3wh）。
pub async fn resolve_engine(
    host: &str,
    board: &str,
    from_block: &str,
    engine_block: &str,
) -> Result<Url, EngineError> {
    let http = reqwest::Client::new();
    let mut base = Url::parse(host)?;
    let base_path = base.path().trim_end_matches('/');
    base.set_path(&format!("{base_path}/p/{board}/{from_block}/__nous/resolve/{engine_block}"));

    let resp = http.get(base).send().await?;
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        return Err(EngineError::Engine {
            status: status.as_u16(),
            message: text.chars().take(200).collect(),
        });
    }
    let parsed: ResolveResp = serde_json::from_str(&text).map_err(|e| EngineError::Engine {
        status: status.as_u16(),
        message: format!("resolve 回的不是话：{e}"),
    })?;
    Ok(Url::parse(&parsed.url)?)
}

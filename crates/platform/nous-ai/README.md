# nous-ai

The unified, provider-agnostic LLM layer: message and content types, the
`Provider` trait, streaming events, and concrete provider implementations.

Every other crate in this library talks in these types. Nothing above this layer
knows whether it is talking to Anthropic, an OpenAI-compatible gateway, or the
deterministic in-process `faux` provider.

**来源**：`bigfish1913/pi-rust@v0.24.2` 的 `crates/rpi-ai`（`src/**` + `tests/**` 原样，
只把包名换成 `nous-ai`、内部依赖换成 `nous-telemetry`）。归属见仓库根
`LICENSE.rpi`，提货记录见 `SOURCES.md`。**冻结**：不 pull、不 merge、不追上游新 tag。

## Cargo

```toml
[dependencies]
# 类型 + faux provider。没有 HTTP 栈，构建期不碰网络。
nous-ai = { path = "../nous-ai" }

# 真 HTTP provider（anthropic）
nous-ai = { path = "../nous-ai", features = ["providers"] }
```

## 上游原文（保留，讲的是这一层负责什么）

The crate is deliberately split so the core stays network-free:

- **Core** (default): types, schema validation, the event stream, the `faux`
  scripted provider.
- **`providers`**: real HTTP providers (`reqwest` + `eventsource-stream`).

Anything provider-specific lives behind the `Provider` trait, so the layer above
never branches on dialect.

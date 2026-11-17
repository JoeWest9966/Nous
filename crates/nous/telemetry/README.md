# nous-telemetry

Minimal span and event contracts, with a no-op default.

**来源**：`bigfish1913/pi-rust@v0.24.2` 的 `crates/rpi-telemetry`（`src/**` 原样，
只有包名从 `rpi-telemetry` 改成 `nous-telemetry`）。归属见仓库根 `LICENSE.rpi`，
提货记录见 `SOURCES.md`。**冻结**：不 pull、不 merge、不追上游新 tag。

The default `NoopTelemetryContext` does nothing; consumers stay monomorphic by
holding a `&dyn TelemetryContext` defaulting to `NOOP_TELEMETRY_CONTEXT`.

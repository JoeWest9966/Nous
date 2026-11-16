# Nous-Package

Nous 的会话内核：Rust，私有，对外只出 `/v1/*`。

**先读 [`AGENTS.md`](AGENTS.md)** —— 它是这个库唯一的维护纲领。
契约在 [`wire.md`](wire.md)（权威版本，Nous 侧只放只读镜像）。

| 文件 | 是什么 |
|---|---|
| `AGENTS.md` | 维护纲领：四个词、来源与规矩、crate 契约图、四类任务、守卫、验收 |
| `wire.md` | 对外契约（宪法） |
| `SOURCES.md` | 提货台账：每一行都能用 `git show` 对上 |
| `docs/storage.md` | 表结构 / 版本号 / 迁移链（自己定，不继承上游） |
| `docs/spec-from-pi-durable.md` | 从上游采纳什么、不采纳什么 |
| `docs/pitfalls-from-pi.md` | 上游 `fix` 的逐条判定（只追加） |
| `scripts/verify.sh` | 唯一的"做完了"定义 |

```bash
./scripts/verify.sh        # 全绿 = 这一版做完了
```

来源：`bigfish1913/pi-rust@v0.24.2`、`earendil-works/pi@v1.5.2`，都是 MIT，归属见 `LICENSE.rpi` / `LICENSE.pi`。

# spec 的唯一真相在 `/opt/Nous-spec`

spec 不放进编码仓。内核 spec、扩展运行时 spec 及其附录 / 交接 / 接缝记录，全部住在
`/opt/Nous-spec` 的 `docs/spec/`（分支 `spec/kernel-rust`），本仓只**引用**、不复制。

- 读法：形状听 `wire.md`，语义听 `/opt/Nous-spec/docs/spec/kernel-rust.md`。
- 判据：`scripts/verify.sh` 第 5 段逐条实现 spec 第 1 章的 15 条；要改判据，改的是 spec 票。
- `scripts/judge.sh` 从 `$NOUS_SPEC/docs/spec/ext-runtime.md`（默认 `/opt/Nous-spec`）读附录 A 的 EX 编号。

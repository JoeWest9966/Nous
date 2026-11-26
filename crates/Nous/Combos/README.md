# 组合

终极模板里每条组合（触发器 + 会话模板）的名册投影与开关写穿。

**什么时候用**：面板要列出「哪些组合现在能用」、要拨一个开关、要试跑一次、要手动起一条。

**不负责**：不判定触发器该不该点火（`nous-dispatcher` 的纯函数），不起会话（`nous-durable`
的命令面），不写 git。盘面只有 `<root>/<section>/<name>/preset.json` 和一份幂等回执目录。

## 四条端点

- `GET /v1/combos/:section` —— 每次请求都重扫盘面，`enabled` 由引擎算（`spec.enabled !== false`
  且引用的 trigger / session 都在）。
- `PATCH /v1/combos/:section/:name` —— 只写穿 `spec.enabled` 一位，`If-Match` 是内容版本号，
  `requestId` 是幂等锚。
- `POST /v1/combos/:section/:name/fire` —— 先问一次 `evaluate`，判定不 fire 就不建会话。
- `POST /v1/presets/Ultimate-Template/launch` —— 跳过判定直接起一条。

后两条共用同一条 launch 路径（建会话 → 写血缘 → 记回执），只在 `forced` 一位上分叉。

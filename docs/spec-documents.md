# documents 模块的形状权威（#48）

> 权威顺序：ticket #48 > `/opt/NousDesk/crates/nous-engine/src/types.rs`（desk 真实期望）>
> `wire.md`（语义参考）。`presets/*` 从来不在 `wire.md` 里，形状一律从 desk 类型抄。
> 本文件是**实现前**定稿，防止 documents 长出 per-kind 分支（ticket 末尾的反悔点）。

## 1. 通用信封（所有 kind 共用，不带 kind 分支）

端点用 `:kind` 路径参数 + 编译期注册表选择 kind；所有响应体都是同一个泛型信封：

| 端点 | 响应（泛型字段） | desk 侧适配 |
|---|---|---|
| `GET /v1/documents/:kind` | `{kind, section, items[], resolved}` | `items[] → Presets{presets, section, categories, repo:null, archived:[]}`；`resolved → Preset.resolved` |
| `GET /v1/documents/:kind/:section/:name` | `{kind, section, name, fields, etag}` | `fields → Preset`，`etag` 用作 `PresetFile.etag` |
| `PUT /v1/documents/:kind/:section/:name` | `{kind, section, name, etag}` | 映射成写文件回包，desk 随后重拉 |
| `POST /v1/documents/:kind/:section` | `{kind, section, name, etag}` | `NewPreset` 的返回体由 `Value` 承接 |
| `POST …/:name/rename` / `publish` / `reload` | `{kind, section, name, ok, refs[], etag}` | rename → `RenamedPreset`（`commit: {}`）；publish/reload 原样由 `Value` 收 |
| `GET …/:name/history` | `{kind, section, name, commits[{hash, short, at, author, message}]}` | → `PresetHistory` |
| `GET …/:name/history/diff` | `{kind, section, name, path, before, after}` | → `FileDiff`（`path` = 文档名） |
| `GET …/:name/pending` | `{kind, section, units[{key, kind, name, tracked, files[], added, removed, code, hasDeleted}], head:null, dirty, ahead, behind, remote:null, aheadCommits:[]}` | → `Pending`（`PendingUnit` / `PendingFile` 字段同名 camelCase） |
| `GET …/:name/pending/diff` | 同 `history/diff`，只比较 base 与之后的 delta | → `FileDiff` |
| `GET …/:name/validate` | `{ok, errors[], warnings[], resolved}` | → `PresetValidation`（本票快照） |

### validate 的 desk 形状（`PresetValidation`，types.rs:511）

```json
{
  "ok": false,
  "errors": [{"path": "summary", "message": "summary is required"}],
  "warnings": [],
  "resolved": {"references": {}}
}
```

- `errors` / `warnings` 的条目是 `{path, message}`（desk 侧按 `Value` 收，UI 自己挑一句话显示）；
- `resolved` 是对象；本实现只做「名字存在性」：`resolved.references.<name>.state = "invalid"` 表示找不到，
  不做 git / 远端 / 跨文件事务那类假装；
- `ok = errors.is_empty()`。

### 16 条 desk 调用 → documents 端点（详见 handoff note）

desk 的 `files` / `file` / `category` / `order` / `categories` / `archive|restore|purge`
落在 documents 的 `PUT`（整份 typed fields）与通用 `POST` 动作上；不需要服务端按 kind 分支。
类别、顺序、归档都作为 typed `Preset` 字段（`category: Vec<String>` / `order` 字段）存在，
由客户端整份写回；`archive|restore|purge` 映射成通用 `POST …/:name/action`，action 原样写进
`fields` 的状态字段，服务端不解释产品语义。

## 2. 结构化字段（compile-time，不是 `Value`）

- `presets`：`PresetFields { name, summary, description, category: Vec<String>, spec: Option<Value>, body: Option<String> }`
- `prompts`：`PromptFields { name, text, summary: Option<String>, tags: Vec<String> }`

`#[serde(deny_unknown_fields)]` 让写错字段名在运行时 400，Rust 侧字段改名/类型改错在编译期红。
未知字段按 desk 契约要透传，但**输入** typed 结构体不接受未知字段：desk 发的就是已知字段，
读出的未知字段放 `extra` 里原样回。

## 3. 存储与发布

- 文档地址：`kind = "<KIND>.<section>"`，`scope = {kind:"session"}`，`key = <name>`；
  用 `session` 作用域只为地址唯一，历史端点的数据来自 `document_revisions`（base + delta），
  不是 git 历史 —— 响应里的 `hash` 就是 revision 的 seq 字符串，不假装有 git sha。
- 写：`nous_durable::session::commands::write_document(session, writes)`，内部同一把写锁、
  同一个事务、同一个 `ObserverRegistry`；模块 `documents` 本身不持有第二个 `Storage`。
- 读：`ImportedSource` 风格的只读 `Storage::open_read_only`（上层 `AppState` 已有数据源），
  `documents` crate 只认 trait `DocumentStore`，不认路由。

## 4. 反悔点

一旦某个 kind 需要流式 / 二进制 / 跨文件事务（git 推拉、归档还原的真实目录语义），
**不许给 documents 加 `if kind == …`**：按 ticket 末尾的规矩把它拆成单独模块。
fields 里的 `spec: Value` 是既成事实（desk 的 `NewPreset.spec` 就是 `Value`），
它是**字段内容**，不是 documents 自己拿 `Value` 当文档形状。

# Trigger_Core

触发器插件。一个触发器在**两个地方**活着，这里两边都管：

- **库里的行**（定义面）：`nous-store` 的 `triggers` / `fires`。点火固定六步
  （许可 → 记账 → 串行 → 会话解析 → 提交 → 血缘），该跳过的跳过，不假装点着了。
- **盘上的目录**（工程面）：`<root>/<name>/{nous.json,Cargo.toml,src/main.rs}`。
  一个触发器 = 一个独立 cargo 项目，`publish` 才编译、写 `index.json`、提交。

两者共用一个私库（`triggers`，默认 `/opt/Nous-Trigger`），但不是同一层东西 ——
所以是**两条宿主能力**，不是一条。

## 它要宿主给什么

```rust
trait Host {
    fn triggers_runtime(&self) -> Option<&TriggerRuntime>;   // 库里的行 + 点火账
    fn triggers_repo(&self) -> Option<Arc<Repo>>;            // 盘上的目录 + git 写锁
}
```

`None` = 没装配，端点回 **409 + 一句人话**，不是 500。只有 `GET /v1/triggers` 除外：
空表本身就是它的契约（#49），空库也得能读。

## 二十四条端点

| 面 | 条数 | 干什么 |
|---|---|---|
| 定义 | 11 | 清单 / 点数 / 账 / 建 / 改 / 归档三代 / 类别词表 |
| 工程 | 13 | 清单 / 文件读写 / 骨架 / 发布 / 归档三代 / 词表 / sync |

## 别忘了

- **点火只有一条写通道**：`nous_durable::session::commands`。
- **`--triggers` 与 `GET /v1/triggers` 是同一个函数**（#58 判据 4）：`runtime::observation_report`。
- **起进程只走 `nous_tools::os_env::run_capture_async`**，`publish` 跑 cargo 也不例外。

# 上游踩过的坑 —— 我们的判定

**只追加，不改旧条目。** 每一轮读上游 changelog 时，在末尾加一节，逐条给判定。

判定只有三个值：

- `中` —— 我们会有同一个毛病 → 转成 `AGENTS.md` 4.4
- `不中` —— 结构不同，不会中
- `未知` —— 还没有对应代码，**写下"等哪一步必须回答"**

---

## 2026-11-16 · 首次采样（`pi@v1.5.2` 的 changelog，近 40 天 94 条 `fix` 里的代表）

| 上游条目 | 现象 | 判定 | 说明 / 等哪一步回答 |
|---|---|---|---|
| `fix(durable): find the conversations above a changed task without walking its owner chain` | 创建 n 层任务链是 O(n²)：找"哪些会话的闲置状态受影响"要逐级往上走 | 未知 | 我们第一版**不做多层任务**，天然不中；做任务图时重新回答 |
| `fix(durable): mark wait promises handled before they reach the caller` | 等待被取消 / Harness 关闭后，promise rejection 无人处理 | 未知 | `nous-durable` 的 abort 落地时必须回答：每个 await 点谁负责收尸 |
| `fix(durable,env): skip entries removed during listDir enumeration` | 枚举目录后 `lstat` 前文件被删 → 整个 listing 失败 | 未知 | 写 `nous-env` 的 `list_dir` 时必须回答：ENOENT 是跳过还是失败 |
| `fix(durable): keep read's non-vision image warning out of what the model gets` | 诊断信息被写进工具结果，换视觉模型后仍然残留 | 不中 | 我们暂不做图片 |
| `perf: SqliteStorage.commit() 每条 8 次语句 → 多行批量` | 一次提交按批问查，改多行语句 | **中** | `nous-durable/src/storage` 的第一次实现就按"批量语句"写，别先写逐条再优化 |
| `feat(durable)!: 部分答案与运行中的工具输出延后落库` | 流式中间态合并成"下一次读 `pi.live` 的提交"，崩溃丢约 1 秒 | 未知 | 见 `docs/spec-from-pi-durable.md`「进度延后落库」；取值写进 `docs/storage.md` |
| `feat(durable)!: DocumentWatch 跟随地址而不是被观察的那一代` | 文档换代（retire → 重建）时监听要跟着地址走，否则监听静默死掉 | 未知 | 我们的归档/捞回（`restore`）就是换代。写 `nous-chord/src/watch` 时回答 |
| `feat(durable)!: 一次回调里合并多个提交，超过 10,000 条操作降为根替换` | 监听回调内的积压合并 | 未知 | 长会话 + SSE 重连时会遇到；写 `nous-wire` 的第一版回答 |

## 格式（下一轮照这个加）

```markdown
## YYYY-MM-DD · 采样 <tag>

| 上游条目 | 现象 | 判定 | 说明 / 等哪一步回答 |
|---|---|---|---|
```

判成 `中` 的，写完这一行就去开分支改代码，别只记账。

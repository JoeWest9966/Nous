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

---

## 2026-11-16 · 移植坑（不是上游 `fix`，但同等级别）

这节记的不是 changelog 条目，是**读上游源码时发现的、Rust 版一定会再踩一次**的坑。放在这份文件里的理由是：三个月后没人记得 `chars().count()` 为什么不行，而这里只追加、会被下一轮读到。来源与行号见 `docs/chord-delta-notes.md`。

| 上游条目 | 现象 | 判定 | 说明 / 等哪一步回答 |
|---|---|---|---|
| `packages/chord/src/delta` 的 `t` op（`tracker.ts`、`apply-immutable-trusted.ts`、`index.ts` 三处一致） | `t` 是「从头截掉 count 个 **UTF-16 码元**」，不是 Unicode 标量，也不是字节。Rust 里顺手写 `chars().count()` 或按字节切，会在含 emoji / 代理对的文本上**静默**让副本与权威分叉 —— 没有 TS 那套字符串语义兜底 | **中** | chord delta 落地那张票（#10）第一次实现 `T` 时必须做两件事：① 按 `encode_utf16` 计数再切；② 在 `apply` 那个分支上写一条注释，明说为什么**不能**用 `chars()`/字节下标，并引这一行。测试必须含 emoji 与代理对用例 |
| `tracker.ts` 的 `appendText` 多 chunk 合并（`tracker.ts:363-388`） | 逐 chunk 追加若退回「比较新旧字符串前缀」，流式写入从 O(N×chunk) 变成 O(N×length)；800KB / 2 万次从 ~1ms 变 ~2.4s | **中** | Nous 的 SSE 合帧铁律就是逐 token 流。Rust 版必须提供 `append_text` 等价 API，emit 时直接发 `a` op；同一条注释里写清「前缀检测只在没有 append 记录时才走」 |

## 2026-11-16 · `nous_ai::Message` 比引擎写进库里的数据窄

**判定：中（已中，不是预判）**。这条比「某个字段暂时用 `Value` 凑合」严重，因为它会被下一次「顺手类型化」再踩一遍。

**症状**：拿 `nous_ai::Message` 解析 `entries.record.model` 里的 message，两种坏法：

1. `pi.system` 的 message **整条解析失败** —— `unknown variant "system"`。`rpi-ai` 的 `Message` 只有
   `user` / `assistant` / `toolResult` 三种，而 `pi` 引擎往同一张表里还写 `role: "system"`。
2. `pi.assistant` / `pi.tool-result` 的 message 能解析，但 **`durationMs` 静默消失** —— serde 默认忽略
   未知键，解析成功、字段没了，没有报错、没有日志。

**根因**：`nous-ai` 是 `rpi-ai@v0.24.2` 的冻结拷贝，它的消息类型是 **rpi 的字段集**，不是 `pi` 引擎的。
`pi` 引擎往同一个 `model` 列里写它自己扩展过的 message（`pi.system` 多一个 `role: system` 变体，
assistant 多 `durationMs`，tool-result 多 `durationMs`）。冻结的那份不知道这些扩展，于是：
认不出的变体 → 拒绝；认得出的变体里多出来的键 → 吞掉。

**解**（三条，按顺序）：

- **存储层不动**：`EntryRecord.model` 保持 `Vec<serde_json::Value>`，原样存原样取（#13 已落地，
  `crates/nous-durable/src/types/records.rs`）。
- **#14 harness 读 message 时不许** `serde_json::from_value::<nous_ai::Message>`。要么继续 `Value`
  透传（渲染、拼请求体时再按需取字段），要么先给 `nous_ai` 的消息类型加 `#[serde(flatten)] extra`
  兜底，**并证明未知键往返不丢**（判据：解析 → 序列化 → 与源 JSON 相等，`toolsAdded` 与
  `durationMs` 都在）。没做到第二条之前，一律走透传。
- **#17 接 google 方言的输入侧同理**：请求体由 `Message` 生成，`Message` 丢键就等于把用户库里的扩展
  信息丢掉再发出去 —— 丢的是数据，不是格式。

**守卫（机器判据）**：`crates/nous-durable/tests/records_fixture.rs::nous_ai_message_is_narrower_than_what_the_engine_stored`
**故意断言「解析不了 / 会丢键」**。哪天 `nous-ai` 补上了 system 变体和未知键兜底，这条会红 —— 那时是
好事：该删的是这条断言和这一节，不是改断言让它继续绿。

## 2026-11-16 · 中途 system 的两档处理（#23 的范围刀）

**起因**：`#23` 票面最初把「anthropic → 顶层 `system` 参数」当成 system 的全部通路，并写了「`toolsAdded`
渲成工具声明文本」。两处都被上游源码推翻，用户核过后确认。这一节记源码事实，票面照它写。

**第一条：首条与中途走的是两个函数**（`packages/ai/src/utils/text.ts`）

| 场景 | 上游函数 | 渲染成什么 |
|---|---|---|
| 首条（`normalizeContext()` 折出来的那条） | `getSystemMessageText` | `content` 文本 + `sections` 的**值**，`"\n\n"` 连接，空串跳过 —— 不加任何框 |
| 中途（模型原生吃中途 system 时） | `renderSystemMessageUpdate` | `content` 文本 + 每个 section **带名字框起来**：`Updated system prompt section "x":\n\n<值>` / 值为 `null` → `Removed system prompt section "x".` |

所以「anthropic 把 system 放顶层 `system` 参数」**只对首条成立**；中途 system 按
`anthropic-messages.ts:1638-1667` held back 到下一个 assistant 之前，落成 messages 里的
`role:"system"` 项 —— 注释写了原因：夹在 `tool_use` 与其结果之间会被服务端拒。

**第二条：工具集不进 prompt 文本。** `toolsAdded` / `toolsRemoved` 的 replay 目标是**请求的工具列表**
（`transcript.ts::getCurrentTools`：首次声明顺序、同名替换在原位、`toolsRemoved` 删除），
`renderSystemMessageUpdate` 也不渲工具。任何「把工具声明拼进 system 文本」的实现都与上游不同序。

**第三条：`sections` 是打补丁，不是全量覆盖。** `getCurrentSystemMessage` 用 `Map.set` 同名就地替换
（保位置）、`null` 删除；被删掉的名字再出现时排到最后。渲染时按 Map 的插入序出值。

**第四条：fixture 里没有 compaction entry。** 机器查过（`sqlite3` 只读）：`pi.system` **17 条**、
`pi.compaction` **0 条**；17 条全是 `model[0]` 一条 system、`content` 全为空串、15 条带 `toolsAdded`
（各 4 个工具）、14 条带 `sections`（`preamble` / `cwd` / `prompts`）、`toolsRemoved` 从未出现。
真数据里的「中途 system」只有两处：会话 2（entry 12 → 410）、会话 14（entry 23 → 457）。
**要验「content 按序追加」只能用构造数据**，测试里必须标出是构造的，别假装 fixture 有。

**由此定的范围（用户刀）**：`#23` 只做「表示 + collapse」—— 原生 flush 分支（`renderSystemMessageUpdate`
+ anthropic 的 `pendingSystemMessages`）**不做**，因为我们手上没有「吃原生中途 system」的模型能对照，
写出来没有任何判据能红。

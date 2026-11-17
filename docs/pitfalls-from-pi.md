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

**坑已填（2026-11-16，票 #23 落地）**：`nous-ai` 补上了 `Message::System`（含 `ToolReference`），
原来那节「`nous_ai::Message` 比引擎写进库里的数据窄」和它的守卫测试按我们自己的规矩**删掉**
（不是改断言）：17 条 `pi.system` 现在逐字段读得进、写回去与源 JSON 相等。

**由此定的范围（用户刀）**：`#23` 只做「表示 + collapse」—— 原生 flush 分支（`renderSystemMessageUpdate`
+ anthropic 的 `pendingSystemMessages`）**不做**，因为我们手上没有「吃原生中途 system」的模型能对照，
写出来没有任何判据能红。

---

## 人工哨兵（没有机器判据，只能连真引擎看）

机器判据只能看本仓的代码，看不到「真引擎现在发什么帧」。所以有几件事**故意不做成测试** ——
做成测试必然假红（`grep` 我们自己的源码永远能过），而假绿比没有更坏。这几件事靠人，写在下面，
每条都写清「为什么会变」和「变了之后动哪里」。

### S1 · 每轮动 `#22` 之前：连一次真引擎，确认 `message_update.changes` 还在

**看什么**：真引擎的 SSE 帧里，`message_update` 是否仍然带 `changes[]`（8 个变体的数组），
而不是带整条 `message`（snapshot-diff）。

**为什么会消失**：上游 `pi@v1.5.1` 已经把它整条事件流砍了 —— `packages/durable/CHANGELOG.md:76`：
*Removed `watchEvents()` and its types `AgentEvent`, `AgentEventStream`, `SnapshotEvent`,
`MessageChange`, and `ToolEventCall` … A renderer renders state instead of assembling it from
events*（earendil-works/pi#11327）。`grep -rn MessageChange packages/` 全仓零命中。
上游的理由是「渲染器渲状态、不拿事件拼状态」。

**我们的理由（不是抄它）**：`wire.md` 的 8 个变体是从**真流量**（NousDesk ↔ 真引擎）抄的，
不是从 pi 源码抄的。所以 pi 删不删**不影响合同** —— 它删的是它自己的渲染路径。

**变了怎么办**：`docs/decisions.md` 里那条「`message_update.changes[]` 的 8 个变体」写了完整
反悔动作：`wire.md` 的 `changes[]` 那段整段作废、`#22` 的投影换成状态差分、`MessageChange`
8 个变体退役。

**为什么是人工**：真引擎不在本仓。要机器判它，得先有一个「连真引擎抓一帧比对」的集成测试，
而那需要线上凭据与网络 —— 那属于部署面，不属于 `verify.sh`（`verify.sh` 必须离线可跑）。

### S2 · 每次提货上游之后：`rg 'rpi[_-]' crates/` 必须 0

代码里不许出现 `rpi_` / `rpi-` 前缀（改了名的提货文件也不能留旧名）。这条**曾经**有机器判据，
后来改成人工报数：报告里报今天多少处（2026-11-17 是 **19**），代码引用必须 0。理由同 S1 ——
上游名字会漂，机器判据只会维护一堆例外。

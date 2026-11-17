# 提交代价实测（`#14` 的前置门：immediate 顶不顶得掉 deferred）

上游流式 partial 是**延迟落库**的（`packages/durable/src/harness/live.ts:236`
`PROGRESS_COMMIT = { persistence: { kind: "deferred" } }`），而本库 `#13` 的 `Session::commit`
明确拒绝 deferred（`SessionError::DeferredUnsupported`）。所以 `#14` 必须先回答：不补 deferred，
1,000 次 immediate 提交要多久。

结论（用户 2026-11-17 拍板）：**deferred 永久砍掉，不留「以后再说」**。判据是下面那张表里
**真实节流下**的数字（160 次 = 110 ms），不是 1,000 次的极限值。

**预算是 3,000 ms，不是 2,000 ms**：2,000 是按「最坏值 = 红线」写的 —— 实测最坏一次 1,989 ms，
离红线 11 ms；五次运行的散布 1,805-1,989 ms。那是假红制造机，不是判据。改成留 50% 余量后，
这个数才是在说「数量级对不对」。

## 环境（数字只在同一环境内可比）

| 项 | 值 |
|---|---|
| CPU | AMD Ryzen 5 5500U with Radeon Graphics（12 线程） |
| 内存 | 15 GiB |
| 文件系统 | ext4，`/dev/nvme0n1p2`，`/var/tmp`（**不是 tmpfs**，见下面那条坑） |
| 内核/发行 | Linux 6.x（asus 本机） |
| rustc | 1.99.1 (58cf8f9c3 2026-10-13) |
| profile | `--release` |
| runtime | `#[tokio::test(flavor = "current_thread")]`，单进程 |
| 每批 | 1 条 delta（`a` 追加 32 字节到 `pi.live` 的 `generation.message.content[0].text`） |
| 预热 | 无（首次打开含迁移） |

## 跑法

```bash
cargo test --release -p nous-durable --test commit_throughput -- --ignored --nocapture
NOUS_BENCH_DIR=/some/ext4/dir cargo test ...        # 改落点
```

`crates/nous-durable/tests/commit_throughput.rs` 里三条：

| 测试 | 量什么 |
|---|---|
| `the_writer_connection_runs_in_wal_with_normal_synchronous`（**不是** ignored） | 前提条件：WAL + `synchronous=NORMAL` |
| `one_thousand_immediate_partial_commits_stay_within_the_budget` | 1,000 次提交、**一条不断长的 delta 链** |
| `one_thousand_commits_with_a_checkpoint_every_hundred` | 同样 1,000 次，但每 100 次打一次 base |

## 数字

每次运行都是独立新库（`temp_db` 会删旧文件）。

| 场景 | 落点 / FS | `synchronous` | 1,000 次提交 | 每次 |
|---|---|---|---|---|
| 一条不断长的链 | /tmp（**tmpfs**） | FULL（默认） | 1,813 ms | 1.8 ms |
| 同上 | tmpfs | NORMAL | 1,770 ms | 1.8 ms |
| 同上 | tmpfs | OFF | 1,726 ms | 1.7 ms |
| 同上 | **ext4（NVMe）** | FULL | 11,854 / 10,701 / 11,226 ms | 11 ms |
| 同上 | ext4 | **NORMAL** | **散布 1,805-1,989 ms**（五次：1,805 / 1,868 / 1,886 / 1,916 / 1,989） | 1.8-2.0 ms |
| 每 100 次打一次 base | ext4 | FULL | 11,888 / 10,747 ms | 11 ms |
| 同上 | ext4 | **NORMAL** | **散布 448-517 ms**（用户复跑 460 ms） | 0.5 ms |
| **160 次**（真实节流：100 ms 一次 = 16 秒流式输出） | ext4 | NORMAL | **110 ms** | 0.7 ms |

链长扫描（tmpfs，FULL；同一进程内改 `COMMITS`）：200 → 89 ms、400 → 329 ms、800 → 1,089 ms。
**总耗时近似平方**，不是线性。

## 结论一：fsync 是真原因，`synchronous=NORMAL` 就是那个「先改同步级别」

- tmpfs 上 FULL / NORMAL / OFF 三者无差别（1,726-1,813 ms）—— 因为 tmpfs 的 `fsync` 是空操作。
  在 tmpfs 上测这件事会得出「fsync 不是瓶颈」的错误结论。
- ext4 上 FULL 11,200 ms、NORMAL 1,890 ms：**82% 的时间在等 fsync**，每次提交 ~11 ms。
- 于是按 `#14` 定的门：`synchronous=NORMAL` 落进 `SqliteStore::open_writer`
  （`crates/nous-durable/src/storage/mod.rs`），代价写清楚 —— **掉电会丢最近若干个提交，库不会坏**
  （WAL 下的 NORMAL 只保证 checkpoint 落盘，SQLite 文档明确这条取舍）。判据
  `the_writer_connection_runs_in_wal_with_normal_synchronous` 钉住这两条 pragma。

## 结论二：deferred **砍掉**，不留「以后再说」

真实节流是 **100 ms** 一次（`settings.progress.partialIntervalMs` 的默认值），不是 16 ms ——
16 ms 是 `wire.md` 里 **SSE 合帧**的节拍（`#22`，客户端那一侧），两件事别混。
按 100 ms 算，16 秒流式输出 = 160 次提交 = **110 ms**（ext4 + NORMAL）。

deferred 能省的只是「把这 110 ms 摊薄」，代价是补 `session/transaction.rs` 的 `plans`/`holds`、
连带改 `#13` 的关闭语义（关会话时 hold 里的东西要落）。**不值。**
决策记录见 `docs/decisions.md` 的「deferred 持久化：不做」。

## 结论三：剩下的 1.9 s 是另一件事，别混进 deferred 的理由里

「1,000 次 ≤ 3s」在 ext4 + NORMAL 下是 **1,805-1,989 ms** —— 过，但**这是极限值，不是我们的节流**。
原因不是 fsync，是**每个 delta 提交都要 `materialize(Current)` 重放整条修订链**
（`crates/nous-durable/src/storage/commit.rs:526` `stored_batch` → `:585` `materialize`），
这是 `#13` 定下的语义（「delta 的 publication `value` 必须等于直接读回该文档当前值」）。

证据：同样 1,000 次提交，链被 base 切断后是 **448 ms**，不切断是 **1,805 ms**；链长 200 → 800
时每次提交从 0.45 ms 涨到 1.36 ms。O(n²)。

- 按真实节流（160 次/16 秒）这件事**不影响 `#14`**：110 ms。
- 长回答（60 秒 = 600 次提交）会把这条曲线走上去；`#22` 一旦按 16 ms 提交就会撞上。
- **处理方式：`#25`**（把 materialize 的结果在 `Storage` 里按文档缓存，base/版本变化时失效），
  排期在 `#14`/`#22` 之后。**先写「外部连接改了同一份文档，缓存必须失效」那条测试、先红，再优化** ——
  加缓存就必然有失效问题，先优化后补测试等于给自己装一个「读到旧值」的静默 bug。

## 为什么这条基准是 `#[ignore]` 而不是判据

墙上时间在共享机器上有噪声：同一个二进制两次跑能差 10%（上面 1,805 vs 1,989 就是同一天量的）。
钉进 `verify.sh` 的后果只有两种：要么把预算放到大到没有意义，要么必然假红。

所以分工是：**测试产出数字，人来判，判据留在这份文件里**（带环境、带散布）。真正进
`verify.sh` 的是**前提条件**——`journal_mode=WAL` + `synchronous=NORMAL`（这两条是机器判的，
设错了整个基准量的是另一个系统）。

## 陷阱（写下来是因为已经踩过一次）

1. **`std::env::temp_dir()` 常常是 tmpfs。** 第一版基准落在 `/tmp`，那里 `fsync` 是空操作 ——
   FULL / NORMAL / OFF 三者差在噪声内（1,813 / 1,770 / 1,726 ms），于是得出「fsync 不是瓶颈」
   的错误结论。基准库默认改到 `/var/tmp`（ext4），可用 `NOUS_BENCH_DIR` 覆盖，**测试会打印落点**。
   这条已升级成人工哨兵 `S3`（`docs/pitfalls-from-pi.md`）：**基准必须打印落点**。
2. **两种节流别混**：存储 100 ms（本文件）/ SSE 合帧 16 ms（`wire.md`、`#22`）。
3. **`CommitResult.stored` 数的是文档修订条数**，不是「写了几条」—— 一批里带会话行时它仍然是 1。

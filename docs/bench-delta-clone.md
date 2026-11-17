# chord delta 的克隆成本（票 #18）

一句话：**不过线**。`Draft::append_text` 每次追加整棵 `Value` 拷贝一次，最差档 p99 **8.07 ms**
（闸 2 ms），内存 **31 MiB**（闸 100 MB）。所以 #10b「不做」的结论被推翻：数据结构要动，
而且按这份数据**动 overlay 树、不动 piece tree**。

## 环境

| 项 | 值 |
|---|---|
| 机器 | AMD Ryzen 5 5500U（12 逻辑核）/ 15,377 MiB 内存 |
| 内存上限 | `systemd-run --scope -p MemoryMax=8G`，检查过实际值：`memory.max = 8589934592` |
| 系统 | Linux（cgroup v2）、rustc 1.99.1（`58cf8f9c3` 2026-10-13） |
| 构建 | `--release`；debug 下 `serde_json` 的克隆慢一个数量级，量出来的不是部署时的数 |
| 负载 | 机器上还有另外两条线在跑（跑完测的 `load average` 5.46 / 6.11 / 7.98），下面第 3 次跑的离群值就是它 |
| 落点 | **无**。这条基准不写盘（不碰 SQLite），所以没有 `docs/bench-commit.md` 那条「tmpfs 上 fsync 是空操作」的坑 |

`AGENTS.md` 口径：j1900 是线上机，不跑 `cargo build`。所以基准在 asus 上跑，用 cgroup 限到 8G
——这正是部署机的量级。

## 跑法

```bash
systemd-run --scope -p MemoryMax=8G -- \
  cargo test -j 2 -p nous-chord --release --test delta_clone_cost -- \
  --ignored --nocapture --test-threads=1 delta_clone_cost
```

`--test-threads=1` 不是可选项：这条基准要读 `/proc/self/status` 的 `VmHWM`，同一个进程里并行跑
别的测试会污染它。两个测试都会被 `delta_clone_cost` 这个过滤词选中（`delta_clone_cost` 与
`delta_clone_cost_shape`）。

调档（慢机器上别硬扛）：`NOUS_DELTA_BENCH_MESSAGES=100,1000`、`NOUS_DELTA_BENCH_APPENDS=2000`、
`NOUS_DELTA_BENCH_SHAPE_APPENDS=200`。三个变量都会打进表头，改了就写在这段下面。

## 票面那条：20,000 次追加，三档

```
root = { messages: [ {role, content}, ... N 条 ] }
操作 = 对最后一条的 content 追加 20,000 次，每次 5–40 字符（确定性 LCG，ASCII）
```

三次独立运行（同一条命令，同一天）：

| N | base 字节 | p50（每次） | p99（每次） | 最慢一次 | `prepare` 总耗时 | ops | 峰值 VmHWM |
|---|---|---|---|---|---|---|---|
| 100 | 10 KiB | 43.5 / 41.6 / 42.3 µs | 358.0 / **312.0** / 310.5 µs | 1,067 / 838 / 687 µs | 0.16 / 0.42 / 0.49 ms | 1 | 6 MiB |
| 1,000 | 104 KiB | 312.4 / 298.9 / 306.0 µs | 559.1 / **470.4** / 613.5 µs | 3,705 / 1,707 / 1,553 µs | 0.80 / 1.27 / 1.05 ms | 1 | 9 MiB |
| 10,000 | 1,058 KiB | 4,402 / 4,224 / 4,593 µs | 8,677 / **8,074** / 16,060 µs | 19,361 / 20,105 / **768,473** µs | 10.33 / 9.23 / 9.19 ms | 1 | 30 / 31 / 31 MiB |

（三格读数是三次跑的顺序；`p99` 加粗的是与 `docs/` 里判定用的那一次。第 3 次 `N=10,000` 的
768 ms 离群值是机器上另外两条线抢 CPU，不是数据结构 —— 见下面「陷阱」第 2 条。）

**判定**：

| 闸 | 票面阈值 | 实测 | 结论 |
|---|---|---|---|
| 每次 `append_text` 的 p99 | < 2,000 µs | 最差档 **8,074–16,060 µs**（3 次跑全超） | **不过** |
| 峰值内存（`VmHWM`） | < 100 MB | **31 MiB** | 过 |

内存之所以这么低：每次克隆出来的整棵树在 `apply_once` 里立刻顶掉旧的 `self.value`，瞬时峰值
只有两份树。**这条不是「内存没问题所以可以不动」** —— 它说明代价是 CPU（分配 + 拷贝），不是驻留。

顺带一个能换算成人话的数：`N=10,000` 那一档 20,000 次追加总共 104 秒。也就是**一次两万 token
的流式回答，光克隆就烧掉 100 秒 CPU**。这跟「op 数只有 1 条」并不矛盾 —— `815873` 量的是 op 数，
从来没人量过这里。

## 过线点在哪（补充探针，5,000 次追加）

```bash
NOUS_DELTA_BENCH_MESSAGES=1000,1500,2000,2500,3000 NOUS_DELTA_BENCH_APPENDS=5000 \
  systemd-run --scope -p MemoryMax=8G -- cargo test -j 2 -p nous-chord --release \
  --test delta_clone_cost -- --ignored --nocapture --test-threads=1 delta_clone_cost
```

| N | base 字节 | p50 | p99 |
|---|---|---|---|
| 1,000 | 104 KiB | 314.1 µs | 714.6 µs |
| 1,500 | 157 KiB | 428.3 µs | 619.0 µs |
| 2,000 | 210 KiB | 575.0 µs | 746.6 µs |
| 2,500 | 263 KiB | 723.8 µs | **1,958.8 µs** |
| 3,000 | 316 KiB | 891.5 µs | **2,399.3 µs** |

**在大约 2,300 条消息 / 240 KiB 处过 2 ms 线**。说清楚这个数是为了别把结论读成「所有会话都慢」：
1,000 条以内的 800 KB 会话（p99 0.47 ms）现在还安全；出事的是**长会话**（1 万条消息 ≈ 1 MB）。

## 过线之后选哪个结构：`overlay` 树，不是 `piece tree`

票面说「二选一，不是两个都做」。选哪个得看成本跟着什么走。于是有第二条基准：**同一份字节数，
两种摆法**（各 2,000 次追加）：

| 摆法 | 结构 | base 字节 | p50 | p99 | `prepare` 总耗时 |
|---|---|---|---|---|---|
| 宽 | 10,000 条消息，各 ~108 B | 1,058 KiB | 3,985 / 4,205 µs | 8,572 / 8,838 µs | 8.14 / 10.29 ms |
| 胖 | 1 条消息，content 1,058 KiB | 1,058 KiB | **338 / 317 µs** | 800 / 811 µs | 0.23 / 0.19 ms |

**同样 1 MB，胖摆法比宽摆法便宜 12–13 倍**（p50）。所以成本由**节点数**决定（每个节点一次分配 +
一次拷贝），不是由被追加的那段字节决定：

- 1 MB 的字符串整段 `memcpy` 只要 ~340 µs —— **本身就在闸线内**；
- 10,000 个节点的树要 4 ms —— 这才是超标的来源。

`piece tree`（rope）解决的正是前者：把字符串切块，追加变成 O(1) 挂一片。它对这份数据里的
**瓶颈没有作用**（瓶颈是节点数），所以现在做它是选错了解药。`overlay` 树（节点延迟拷贝，只有被
写到的路径复制）直接对着瓶颈。

**决定：开 `overlay` 树。`piece tree` 留给「字符串本身大到 `memcpy` 也过线」的时候再重开** ——
即某一天会话里出现单段 >5 MB 的正文，或者 `overlay` 树落地后 L 档 p99 仍 > 2 ms 且形状判据翻到
胖摆法那边。那时这条基准的 `delta_clone_cost_shape` 会给出新的比值，不用重新设计量法。

## 陷阱

1. **`Draft::prepare` 不是大头。** 20,000 次追加里它只有 9–10 ms（一次性），4.2 ms × 20,000 次
   的 `append_text` 才是。只看 `prepare` 的耗时会把这件事看反 —— 这也是票面把两列并列的原因。

2. **机器上别的活会污染基准。** 第 3 次跑 `N=10,000` 的最慢一次 768 ms、p99 16 ms，而同一个二进制
   同一份数据在前两次是 8.1 / 8.7 ms。同一台机器另一次形状基准的 p50 是 7.90 ms（当时有别的编译在跑），而两次正式跑是 3.99 / 4.21 ms。
   **贴散布、贴跑法、别用单次数字做结论** —— 与 `docs/bench-commit.md` 那条同源。

3. **一次 200 次追加量不出这个成本。** 早期探针用 200 次追加跑 `N=10,000` 得到 p99 4.4 ms，用
   20,000 次是 8.7 ms。原因不是预热：正文在长（20,000 次 × ~22 B ≈ 450 KB），每次克隆的字节数
   跟着长。所以「p50 vs p99 的差距」本身就带着「越跑越贵」的信息，别把它当噪声抹平。

4. **`VmHWM` 是单调的。** 它记的是进程高水位，不是当前 RSS。表里「峰值」取的是跑完那一刻的
   `VmHWM`，所以三档只能一路涨（6 → 9 → 31 MiB），中间档不可能自己降下去。要判「这一档用了多少」
   得看差值，要判「会不会撑爆部署机」看绝对值 —— 票面问的是后者。

## 这条基准为什么 `#[ignore]`

20,000 × 3 档在 release 下 100 秒级，是 `cargo test --workspace` 的几百倍。放进默认路径会让
`verify.sh` 第 1 段变得没人愿意跑，然后它就会被人加 `--exclude`。数字已经落在这里，测试在仓库里
可复算，这就是「慢要慢得可解释」的兑现方式。

## 复算

```bash
# 票面那条（≈100 s）
systemd-run --scope -p MemoryMax=8G -- \
  cargo test -j 2 -p nous-chord --release --test delta_clone_cost -- \
  --ignored --nocapture --test-threads=1

# 只看形状判据（≈5 s）
NOUS_DELTA_BENCH_SHAPE_APPENDS=300 \
  cargo test -j 2 -p nous-chord --release --test delta_clone_cost -- \
  --ignored --nocapture --test-threads=1 delta_clone_cost_shape
```

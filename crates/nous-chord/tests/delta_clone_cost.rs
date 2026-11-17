//! 基准：`Draft` 的克隆成本（票 #18）。
//!
//! 这一条**只出数，不优化**。判据在票面：p99 < 2 ms/次 **且** 峰值 < 100 MB ⇒ 不做 overlay 树 /
//! piece tree（`tracker.ts` 那 2,319 行的活不干）；任一超标才动数据结构。
//!
//! ## 为什么需要这条
//!
//! `815873` 那条「2 万次追加 = 1 条 op」测的是 **op 数**，不是**克隆成本**：
//! `Draft::append_text` 每次调 `apply_immutable_trusted(&self.value, ..)`，`serde_json::Value`
//! 没有结构共享，于是**每次追加都整棵拷贝一次**。op 数很漂亮，代价没量过。
//!
//! ## 两条基准，各答一个问题
//!
//! 1. `delta_clone_cost` —— **票面那条**：过不过线？
//!    ```text
//!    root = { messages: [ {role, content}, ... N 条 ] }
//!    操作 = 对最后一条的 content 追加 20,000 次，每次 5–40 字符
//!    N    = 100 / 1000 / 10000
//!    ```
//!    每档三列：`append_text` 的 p50 / p99（每次）、`Draft::prepare` 的总耗时（`diff_revisions`
//!    是成本最集中的地方）、`VmHWM`。
//! 2. `delta_clone_cost_shape` —— **过线之后选哪个结构**：同样是 ~1 MB 的文档，一种摆成 1 万条
//!    小消息（宽），一种摆成 1 条大消息（胖）。成本若随**节点数**走，`overlay` 树是解；若随
//!    **被追加的那段字节**走，`piece tree` 是解。这条不写进票面判据，只用来决定「二选一选哪个」。
//!
//! ## 跑法
//!
//! ```bash
//! systemd-run --scope -p MemoryMax=8G -- \
//!   cargo test -j 2 -p nous-chord --release --test delta_clone_cost -- \
//!   --ignored --nocapture --test-threads=1 delta_clone_cost
//! ```
//!
//! `--test-threads=1` 不是可选项：这条基准要读 `VmHWM`，同一个进程里并行跑别的测试会污染它。
//! `--release` 也不是可选项：debug 下 `serde_json` 的克隆慢一个数量级，量出来的不是部署时的数。
//!
//! 调档（慢机器上别硬扛）：`NOUS_DELTA_BENCH_MESSAGES=100,1000`、`NOUS_DELTA_BENCH_APPENDS=2000`、
//! `NOUS_DELTA_BENCH_SHAPE_APPENDS=200`。三个变量都会打进表头，改了要写进
//! `docs/bench-delta-clone.md` 的跑法里。
//!
//! ## 为什么 `#[ignore]`
//!
//! 20,000 × 3 档在 release 下是 100 秒级，不该进 `cargo test --workspace` 的默认路径
//! （`verify.sh` 第 1 段）。但它**在仓库里、可复算** —— 不是一次性脚本。

use std::time::{Duration, Instant};

use nous_chord::delta::draft::Draft;
use nous_chord::delta::{Revision, Seg};
use serde_json::{json, Value};

/// 一次「追加 N 次再 `prepare`」的读数。
struct Samples {
    p50: Duration,
    p99: Duration,
    worst: Duration,
    prepare: Duration,
    ops: usize,
}

/// 读 `/proc/self/status` 的 `VmHWM`（KiB）。非 Linux 返回 `None` —— 判据就那一列，
/// 拿不到要如实说「没量到」，不能拿 RSS 冒充峰值。
fn vm_hwm_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest.trim().split_whitespace().next()?.parse().ok();
        }
    }
    None
}

/// 确定性的伪随机：基准里不要引 `rand`，也不许结果每次不一样（两次跑要同量级，
/// 波动只能来自机器不能来自输入）。
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }
}

fn chunk(lcg: &mut Lcg) -> String {
    // 5–40 字符（票面的区间），ASCII 字母 —— 不碰中文，免得把「字符数 / 字节数」混进耗时。
    let len = 5 + (lcg.next() % 36) as usize;
    (0..len)
        .map(|_| (b'a' + (lcg.next() % 26) as u8) as char)
        .collect()
}

fn base_value(messages: usize) -> Value {
    let items: Vec<Value> = (0..messages)
        .map(|index| {
            json!({
                "role": if index % 2 == 0 { "user" } else { "assistant" },
                "content": format!("message {index} {}", "x".repeat(64)),
            })
        })
        .collect();
    json!({ "messages": items })
}

fn percentile(sorted: &[Duration], percent: f64) -> Duration {
    // 最近秩法：p99 是「第 ceil(0.99·n) 个」，不是插值 —— 插值在小样本上会给出一个
    // 谁也没观察到的数。
    let rank = ((percent / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

fn micros(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000_000.0
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn kib(bytes: usize) -> usize {
    bytes / 1024
}

/// 对 `path` 追加 `appends` 次，取分位，再把草稿结算成 op。
fn append_run(base: &Value, path: &[Seg], appends: usize) -> Samples {
    let mut draft = Draft::new(base, Revision::ZERO);
    let mut lcg = Lcg(0x5EED_1234_ABCD_0001);
    let mut samples = Vec::with_capacity(appends);
    for _ in 0..appends {
        let text = chunk(&mut lcg);
        let started = Instant::now();
        draft.append_text(path, text.as_str()).unwrap();
        samples.push(started.elapsed());
    }
    samples.sort();

    let started = Instant::now();
    let ops = draft.prepare(Revision::ZERO).unwrap().into_ops().len();
    let prepare = started.elapsed();

    Samples {
        p50: percentile(&samples, 50.0),
        p99: percentile(&samples, 99.0),
        worst: *samples.last().unwrap(),
        prepare,
        ops,
    }
}

fn serialized_bytes(value: &Value) -> usize {
    serde_json::to_vec(value).map(|bytes| bytes.len()).unwrap_or(0)
}

fn last_message_content(messages: usize) -> [Seg; 3] {
    [
        Seg::key("messages"),
        Seg::index(messages - 1),
        Seg::key("content"),
    ]
}

fn env_list(name: &str, fallback: &[usize]) -> Vec<usize> {
    match std::env::var(name) {
        Ok(text) => text
            .split(',')
            .filter_map(|part| part.trim().parse().ok())
            .collect(),
        Err(_) => fallback.to_vec(),
    }
}

fn header(appends: usize) {
    println!();
    println!("| 项 | 值 |");
    println!("|---|---|");
    println!(
        "| 每档追加次数 | {appends} |"
    );
    println!("| 每块长度 | 5–40 字符（确定性 LCG，ASCII） |");
    println!(
        "| 机器 | {} 逻辑核 / {} MiB 内存 |",
        std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(0),
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|line| line.starts_with("MemTotal:"))
                    .and_then(|line| line.split_whitespace().nth(1).map(str::to_string))
            })
            .and_then(|kib| kib.parse::<u64>().ok())
            .map(|kib| kib / 1024)
            .unwrap_or(0),
    );
    println!("| 落点 | 无（不碰 SQLite，没有 fsync 陷阱；数全是内存里的） |");
    println!("| VmHWM 可读 | {} |", vm_hwm_kib().is_some());
    println!();
}

fn hwm(value: Option<u64>) -> String {
    value
        .map(|kib| format!("{} MiB", kib / 1024))
        .unwrap_or_else(|| "n/a".to_string())
}

#[test]
#[ignore = "基准：20,000 次追加 × 3 档，release 下也要 100 秒级"]
fn delta_clone_cost() {
    let message_tiers = env_list("NOUS_DELTA_BENCH_MESSAGES", &[100, 1_000, 10_000]);
    let appends = env_list("NOUS_DELTA_BENCH_APPENDS", &[20_000])
        .first()
        .copied()
        .unwrap_or(20_000);

    header(appends);
    println!("条数档：{message_tiers:?}（票面场景：对最后一条消息的 `content` 追加）");
    println!();
    println!(
        "| N 条消息 | base 字节 | 追加次数 | p50 | p99 | 最慢一次 | `prepare` 总耗时 | ops | VmHWM 前 | VmHWM 后 |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");

    let mut readings = Vec::new();
    for messages in message_tiers {
        let base = base_value(messages);
        let bytes = serialized_bytes(&base);
        let before = vm_hwm_kib();
        let samples = append_run(&base, &last_message_content(messages), appends);
        let after = vm_hwm_kib();
        println!(
            "| {messages} | {} KiB | {} | {:.1} µs | **{:.1} µs** | {:.1} µs | {:.2} ms | {} | {} | {} |",
            kib(bytes),
            appends,
            micros(samples.p50),
            micros(samples.p99),
            micros(samples.worst),
            millis(samples.prepare),
            samples.ops,
            hwm(before),
            hwm(after),
        );
        readings.push((messages, samples));
    }

    let worst_p99 = readings
        .iter()
        .map(|(_, samples)| samples.p99)
        .max()
        .unwrap_or_default();
    let peak_kib = vm_hwm_kib();
    println!();
    println!(
        "最差档 p99 = **{:.1} µs**（闸 2 ms = 2000 µs）；进程峰值 VmHWM = {}（闸 100 MB）",
        micros(worst_p99),
        hwm(peak_kib)
    );
    println!();

    // 判据不是断言「必须过线」——过线的动作是「不做」（关票），超线是「开 overlay/piece tree」。
    // 两者都不该让这条测试红；这里只守住「读到的数不是空转」：每次追加真的产生了 op，
    // 且 `prepare` 把 20,000 次追加收成了一条。
    for (messages, samples) in &readings {
        assert_eq!(
            samples.ops, 1,
            "N={messages} 的 {appends} 次追加应当收成 1 条 op（这也是 op 数不是成本的那个旧结论）"
        );
        assert!(
            samples.p99 > Duration::ZERO,
            "N={messages} 的 p99 是 0 —— 计时没接上，量出来的东西不能拿去判定"
        );
    }
}

#[test]
#[ignore = "基准：过线之后用来选结构（宽摆法 vs 胖摆法）"]
fn delta_clone_cost_shape() {
    let appends = env_list("NOUS_DELTA_BENCH_SHAPE_APPENDS", &[2_000])
        .first()
        .copied()
        .unwrap_or(2_000);
    let wide_messages = 10_000;

    header(appends);
    println!(
        "同一份字节数，两种摆法。成本随**节点数**走 ⇒ `overlay` 树是解；随**被追加的那段字节**走 ⇒ `piece tree` 是解。"
    );
    println!();
    println!("| 摆法 | 结构 | base 字节 | 追加次数 | p50 | p99 | `prepare` 总耗时 | ops |");
    println!("|---|---|---|---|---|---|---|---|");

    let wide = base_value(wide_messages);
    let wide_bytes = serialized_bytes(&wide);
    let wide_samples = append_run(&wide, &last_message_content(wide_messages), appends);

    // 胖摆法：一条消息，content 补到跟宽摆法差不多总字节。补的字数按宽摆法里所有
    // `content` 之外的开销反推不出来，所以直接迭代到接近为止 —— 差几十字节不影响量级判断，
    // 但差一个数量级就会把结论说反，故打印实际字节。
    let mut pad = wide_bytes.saturating_sub(200);
    let mut fat = Value::Null;
    for _ in 0..8 {
        fat = json!({ "messages": [ { "role": "user", "content": "y".repeat(pad) } ] });
        let bytes = serialized_bytes(&fat);
        if bytes >= wide_bytes {
            break;
        }
        pad += wide_bytes - bytes;
    }
    let fat_bytes = serialized_bytes(&fat);
    let fat_path = [Seg::key("messages"), Seg::index(0), Seg::key("content")];
    let fat_samples = append_run(&fat, &fat_path, appends);

    println!(
        "| 宽 | {} 条消息，各 ~{} B | {} KiB | {} | {:.1} µs | **{:.1} µs** | {:.2} ms | {} |",
        wide_messages,
        wide_bytes / wide_messages,
        kib(wide_bytes),
        appends,
        micros(wide_samples.p50),
        micros(wide_samples.p99),
        millis(wide_samples.prepare),
        wide_samples.ops,
    );
    println!(
        "| 胖 | 1 条消息，content {} KiB | {} KiB | {} | {:.1} µs | **{:.1} µs** | {:.2} ms | {} |",
        pad / 1024,
        kib(fat_bytes),
        appends,
        micros(fat_samples.p50),
        micros(fat_samples.p99),
        millis(fat_samples.prepare),
        fat_samples.ops,
    );
    println!();
    println!(
        "胖 / 宽（p50 之比）= **{:.2}×** —— {}",
        micros(fat_samples.p50) / micros(wide_samples.p50),
        if fat_samples.p50 > wide_samples.p50 {
            "同样字节数下胖摆法更贵 ⇒ 成本主要跟着被追加的那段字节走"
        } else {
            "同样字节数下宽摆法更贵 ⇒ 成本主要跟着节点数走"
        }
    );
    println!();

    assert_eq!(wide_samples.ops, 1);
    assert_eq!(fat_samples.ops, 1);
    // 两种摆法的总字节要在一个量级，否则这条对比说明不了任何事。
    let ratio = fat_bytes as f64 / wide_bytes.max(1) as f64;
    assert!(
        (0.8..=1.25).contains(&ratio),
        "两种摆法的字节数差太多（{fat_bytes} vs {wide_bytes}），这条对比不成立"
    );
}

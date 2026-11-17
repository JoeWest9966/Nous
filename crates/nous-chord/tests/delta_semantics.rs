//! `docs/chord-delta-notes.md` 的三条不变式，各一条测试；外加 UTF-16 `t`、恢复点、随机往返。
//!
//! 判据纪律（上游 README:198–200「batch is exact but not canonical」）：**只断言重放后的值**，
//! 不断言 op 元组。随机往返那一万次重放就是这条纪律的实证。

use std::collections::HashSet;
use std::sync::Arc;

use nous_chord::delta::apply::{apply_immutable_trusted, TrustedOps};
use nous_chord::delta::codec::{Decoder, Encoder};
use nous_chord::delta::diff::diff_revisions;
use nous_chord::delta::pending::{PendingOps, PendingTake, MAX_PENDING_OPS};
use nous_chord::delta::{path, Op, Seg};
use serde_json::{json, Map, Value};

// ─────────────────────── 不变式 1：批次精确但不规范 ───────────────────────

#[test]
fn batches_are_exact_but_not_canonical() {
    let base = json!({"xs": [1, 2, 3], "t": "hello world"});
    let after = json!({"xs": [1, 9, 3], "t": "hello there"});

    // 路 A：diff 算出来的那批。
    let a = diff_revisions(&base, &after);
    let from_a = apply_immutable_trusted(&base, &a).unwrap();
    assert_eq!(from_a, after);

    // 同一变化还能折成整根替换：另一种形状，同一个值。
    let whole = TrustedOps::from_decoded(vec![Op::Replace(after.clone())]).unwrap();
    assert_eq!(apply_immutable_trusted(&base, &whole).unwrap(), after);

    // 删一个键：diff 发 `d`（细），也可以发父对象的整块 `s`（粗）。
    let fine_base = json!({"o": {"a": 1, "b": 2}});
    let fine_after = json!({"o": {"a": 1}});
    let fine = diff_revisions(&fine_base, &fine_after);
    let coarse = TrustedOps::from_decoded(vec![Op::Set(
        path(&["o"]),
        json!({"a": 1}),
    )])
    .unwrap();
    assert_eq!(apply_immutable_trusted(&fine_base, &fine).unwrap(), fine_after);
    assert_eq!(
        apply_immutable_trusted(&fine_base, &coarse).unwrap(),
        fine_after
    );
    // 两种形状都能重放到同一个值 —— 而 op 元组不同，所以拿元组做断言会随实现漂。
    assert_ne!(fine.as_ops(), coarse.as_ops());
}

// ───────────────── 不变式 2：PendingOps 叶折叠 + 溢出走整根替换 ─────────────────

#[test]
fn pending_ops_folds_leaves_and_overflows_into_replace() {
    // 叶折叠：同路径上的 a + a + a 合成一条；s 覆盖 s；其他 op 清空叶索引。
    let mut pending = PendingOps::new();
    pending.push(Arc::new(vec![Op::Append(path(&["t"]), "a".into())]));
    pending.push(Arc::new(vec![
        Op::Append(path(&["t"]), "b".into()),
        Op::Append(path(&["t"]), "c".into()),
    ]));
    assert_eq!(pending.size(), 1, "三条追加折成一条");
    match pending.take() {
        PendingTake::Ops(ops) => {
            assert_eq!(ops.as_slice(), &[Op::Append(path(&["t"]), "abc".into())])
        }
        PendingTake::Replace => panic!("不该溢出"),
    }

    // 溢出：折不动的一堆（不同路径）到 MAX_PENDING_OPS 之后只剩「整根替换」。
    let mut pending = PendingOps::new();
    pending.push(Arc::new(vec![Op::Set(path(&["seed"]), json!(0))]));
    pending.push(Arc::new(
        (0..=MAX_PENDING_OPS)
            .map(|index| Op::Set(vec![Seg::Key(format!("k{index}"))], json!(index)))
            .collect(),
    ));
    assert_eq!(pending.size(), 1);
    assert_eq!(pending.take(), PendingTake::Replace);

    // 溢出态被一个以 r 开头的批次打断：积压全丢，该批原样保留（它是恢复点）。
    let mut pending = PendingOps::new();
    pending.push(Arc::new(vec![Op::Set(path(&["seed"]), json!(0))]));
    pending.push(Arc::new(
        (0..=MAX_PENDING_OPS)
            .map(|index| Op::Set(vec![Seg::Key(format!("k{index}"))], json!(index)))
            .collect(),
    ));
    pending.push(Arc::new(vec![Op::Replace(json!({"fresh": true}))]));
    match pending.take() {
        PendingTake::Ops(ops) => {
            assert_eq!(ops.as_slice(), &[Op::Replace(json!({"fresh": true}))])
        }
        PendingTake::Replace => panic!("r 应该打断溢出态"),
    }
}

// ────────── 不变式 3：apply-immutable-trusted 的信任边界 ──────────

#[test]
fn trusted_apply_is_a_type_level_boundary() {
    let base = json!({"a": {"b": 1}});

    // 外部（wire / 模型输出）的 op 必须先过校验才能变成 TrustedOps。
    let from_wire = vec![Op::Set(path(&["__proto__", "isAdmin"]), json!(true))];
    let rejected = TrustedOps::from_decoded(from_wire).unwrap_err();
    assert_eq!(
        rejected,
        nous_chord::delta::OpError::UnsafePath("__proto__".into())
    );

    // 校验通过之后，可信路径不再做形状校验（上游语义：只接受自己产生的 op）：
    // 空路径的 `s` 在 apply 里表现为路径解析失败，不是「被接受」。
    let trusted = TrustedOps::from_decoded(vec![Op::Set(path(&["a", "b"]), json!(2))]).unwrap();
    assert_eq!(
        apply_immutable_trusted(&base, &trusted).unwrap(),
        json!({"a": {"b": 2}})
    );

    // 不可信的一侧（解码器）在边界上就拒绝保留段。
    let error = Decoder::new()
        .decode(&[json!(["d", ["constructor"]])])
        .unwrap_err();
    assert_eq!(
        error,
        nous_chord::delta::OpError::UnsafePath("constructor".into())
    );

    // base 永远不被修改（函数签名就挡住了对 base 的 &mut）。
    assert_eq!(base, json!({"a": {"b": 1}}));
}

// ─────────── 陈旧草稿的判据在 `Draft`，不在 `apply`（边界写清楚） ───────────

#[test]
fn a_stale_batch_is_rejected_by_the_draft_not_by_apply() {
    // 判据只在 `Draft::prepare` 里（它记着出生时的修订号）。`apply_immutable_trusted`
    // 收的是「相对 base 的一批 op」，手里没有修订号 —— 所以它能重放到任何 base 上。
    // 这不是漏洞，是分层：陈旧检测属于持有 base 的那一层（durable 的 storage/tracker）。
    let base = json!({"a": 1});
    let ops = diff_revisions(&base, &json!({"a": 2}));
    let other_base = json!({"a": 100, "b": true});
    let replayed = apply_immutable_trusted(&other_base, &ops).unwrap();
    assert_eq!(replayed, json!({"a": 2, "b": true}), "op 是相对的，apply 无从判断");

    // 而 `Draft` 这一层明确拒绝（细节见 draft.rs 的单测）。
    use nous_chord::delta::draft::Draft;
    use nous_chord::delta::Revision;
    let mut draft = Draft::new(&base, Revision::ZERO);
    draft.set(&[Seg::Key("a".into())], json!(2)).unwrap();
    assert!(draft.prepare(Revision::ZERO.next()).is_err());
}

// ─────────────────────── UTF-16：`t` op 的唯一单位 ───────────────────────

#[test]
fn truncate_counts_utf16_code_units_so_emoji_and_surrogate_pairs_survive() {
    // 1 个 char / 2 个 UTF-16 码元 / 4 个 UTF-8 字节。
    let base = json!({ "s": "😀😀" });
    let two_units = TrustedOps::from_decoded(vec![Op::Truncate(path(&["s"]), 2)]).unwrap();
    assert_eq!(
        apply_immutable_trusted(&base, &two_units).unwrap(),
        json!({ "s": "😀" }),
        "截 2 个码元 = 一个 emoji；用 chars() 会得到空串，用字节会切烂 UTF-8"
    );

    // 落在代理对中间：整个字符丢弃，绝不产生孤立代理项（&str 也就不可能非法）。
    let split = TrustedOps::from_decoded(vec![Op::Truncate(path(&["s"]), 1)]).unwrap();
    assert_eq!(
        apply_immutable_trusted(&base, &split).unwrap(),
        json!({ "s": "😀" })
    );

    // 组合字符（é = e + U+0301）按码元算，不按字素簇。
    let combining = json!({ "s": "e\u{301}x" });
    let one = TrustedOps::from_decoded(vec![Op::Truncate(path(&["s"]), 1)]).unwrap();
    assert_eq!(
        apply_immutable_trusted(&combining, &one).unwrap(),
        json!({ "s": "\u{301}x" })
    );

    // 代理对家族（ZWJ 序列）逐码元截断，结果仍是合法字符串。
    let family = json!({ "s": "👨‍👩‍👧x" });
    let units = "👨‍👩‍👧".encode_utf16().count();
    let cut = TrustedOps::from_decoded(vec![Op::Truncate(path(&["s"]), units)]).unwrap();
    assert_eq!(
        apply_immutable_trusted(&family, &cut).unwrap(),
        json!({ "s": "x" })
    );
}

// ─────────────────────── 恢复点：codec 必须在 `r` 处清字典 ───────────────────────

#[test]
fn a_replay_can_start_at_the_last_recovery_point() {
    let state_v1 = json!({"t": "hello"});
    let state_v2 = json!({"t": "hello world"});

    let mut encoder = Encoder::new();
    let first = encoder.encode(diff_revisions(&json!({}), &state_v1).as_ops());
    let second = encoder.encode(diff_revisions(&state_v1, &state_v2).as_ops());
    // 让字典里先攒几个 id：重复写同一路径。
    let chatter = encoder.encode(&[
        Op::Set(path(&["t"]), json!("hello world!")),
        Op::Set(path(&["u"]), json!(1)),
        Op::Set(path(&["t"]), json!("hello world!!")),
    ]);

    // 恢复点批（以 r 开头）之后的批次必须自给自足。
    let base_batch = encoder.encode(&[Op::Replace(state_v2.clone())]);
    let after_base = encoder.encode(&[Op::Append(path(&["t"]), " again".into())]);

    // 一个**全新的**解码器只从恢复点开始读：如果编码器在 r 之后还引用旧 id，这里会失败。
    let mut recovery_decoder = Decoder::new();
    assert_eq!(
        recovery_decoder.decode(&base_batch).unwrap(),
        vec![Op::Replace(state_v2.clone())]
    );
    let tail = recovery_decoder.decode(&after_base).unwrap();
    assert_eq!(tail, vec![Op::Append(path(&["t"]), " again".into())]);
    assert_eq!(
        apply_immutable_trusted(&state_v2, &TrustedOps::from_decoded(tail).unwrap()).unwrap(),
        json!({"t": "hello world again"})
    );

    // 而完整流（同一个解码器从头读）也能解出全部批次。
    let mut full = Decoder::new();
    for batch in [&first, &second, &chatter, &base_batch, &after_base] {
        full.decode(batch).expect("full stream decodes");
    }
}

// ─────────────────────── 随机往返：1,000 例 ───────────────────────

/// 确定性 PRNG（xorshift64*）：不引 rand 依赖，跑起来每次都一样。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            0
        } else {
            self.next() % bound
        }
    }
}

fn random_value(rng: &mut Rng, depth: usize) -> Value {
    let choice = rng.below(if depth >= 3 { 5 } else { 7 });
    match choice {
        0 => Value::Null,
        1 => Value::Bool(rng.below(2) == 0),
        2 => json!(rng.below(100) as i64),
        3 => json!(format!("s{}", rng.below(10))),
        4 => json!(format!("😀{}é", rng.below(3))),
        5 => Value::Array(
            (0..rng.below(4))
                .map(|_| random_value(rng, depth + 1))
                .collect(),
        ),
        _ => {
            let mut map = Map::new();
            for _ in 0..rng.below(4) {
                map.insert(format!("k{}", rng.below(5)), random_value(rng, depth + 1));
            }
            Value::Object(map)
        }
    }
}

#[test]
fn apply_of_diff_equals_target_for_a_thousand_random_pairs() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut non_trivial = 0usize;
    for round in 0..1_000 {
        let before = random_value(&mut rng, 0);
        let after = random_value(&mut rng, 0);

        let ops = diff_revisions(&before, &after);
        let replayed =
            apply_immutable_trusted(&before, &ops).expect("diff 产出的 op 必然可应用");
        assert_eq!(replayed, after, "round {round}: {before} → {after}");

        // 同一批也要能过 wire（encode → decode）再重放。
        let wire = Encoder::new().encode(ops.as_ops());
        let decoded = Decoder::new().decode(&wire).expect("自身产出的 wire 必然可解");
        assert_eq!(
            apply_immutable_trusted(&before, &TrustedOps::from_decoded(decoded).unwrap()).unwrap(),
            after,
            "round {round}: wire 往返后重放必须相同"
        );

        if before != after {
            non_trivial += 1;
        }
    }
    // 这一万次里绝大多数是真变化；否则这个测试等于什么都没测。
    assert!(non_trivial > 500, "随机对里真变化太少：{non_trivial}");
}

#[test]
fn random_codec_roundtrips_are_lossless() {
    let mut rng = Rng(0xDEAD_BEEF_CAFE_1234);
    for _ in 0..1_000 {
        let segments: Vec<Seg> = (0..rng.below(3))
            .map(|_| {
                if rng.below(2) == 0 {
                    Seg::Key(format!("k{}", rng.below(3)))
                } else {
                    Seg::Index(rng.below(3))
                }
            })
            .collect();
        // `s`/`d`/`a`/`t` 不许指向根。
        let target = if segments.is_empty() {
            path(&["p"])
        } else {
            segments
        };
        let op = match rng.below(4) {
            0 => Op::Set(target, random_value(&mut rng, 2)),
            1 => Op::Append(target, format!("x{}", rng.below(3))),
            2 => Op::Truncate(target, rng.below(4) as usize),
            _ => Op::Remove(target),
        };
        let wire = Encoder::new().encode(std::slice::from_ref(&op));
        let decoded = Decoder::new().decode(&wire).unwrap();
        assert_eq!(decoded, vec![op.clone()], "wire: {wire:?}");
    }

    // 一批里重复路径也不能错（id 定义/引用）。
    let ops: Vec<Op> = (0..50)
        .map(|index| {
            let key = ["a", "b", "c"][index % 3];
            Op::Set(vec![Seg::Key(key.to_string())], json!(format!("v{index}")))
        })
        .collect();
    let wire = Encoder::new().encode(&ops);
    assert!(wire.len() > ops.len(), "重复路径应当定义 id");
    assert_eq!(Decoder::new().decode(&wire).unwrap(), ops);
}

#[test]
fn distinct_segments_do_not_collide_in_one_batch() {
    // ["a.b"] 与 ["a","b"] 是不同路径：同一批里都必须活着。
    let ops = vec![
        Op::Set(path(&["a", "b"]), json!(2)),
        Op::Set(path(&["a.b"]), json!(1)),
    ];
    let wire = Encoder::new().encode(&ops);
    assert_eq!(Decoder::new().decode(&wire).unwrap(), ops);
    let base = json!({"a": {"b": 0}, "a.b": 0});
    let applied = apply_immutable_trusted(&base, &TrustedOps::from_decoded(ops).unwrap()).unwrap();
    assert_eq!(applied, json!({"a": {"b": 2}, "a.b": 1}));
    // 集合里两条键确实不同。
    let keys: HashSet<_> = applied.as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 2);
}

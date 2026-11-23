//! section / 帧（EX-07…16、28、55、78、79、80）。

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::sync::Arc;

use nous_ext::Runtime;
use serde_json::{json, Value};

use crate::sandbox::Sandbox;
use crate::{kv, manifest};

fn section_ext(sandbox: &Sandbox, name: &str) {
    sandbox.add_ext(
        name,
        manifest(
            name,
            &format!("exts-section-{name}"),
            "0.1.0",
            &[("preamble", "preamble"), ("skills", "skills")],
            &["skills", "probe"],
        ),
        "exts-section",
    );
}

/// 三个扩展的 uid 两两不同（EX-07）。
pub fn one_process_per_extension() -> i32 {
    let sandbox = Sandbox::new("uids");
    section_ext(&sandbox, "section");
    sandbox.add_ext(
        "handle",
        manifest("handle", "exts-handle", "0.1.0", &[("prompts", "prompts")], &["prompts"]),
        "exts-handle",
    );
    sandbox.add_ext(
        "combo",
        manifest(
            "combo",
            "exts-combo",
            "0.1.0",
            &[("roster", "roster")],
            &["skills", "prompts"],
        ),
        "exts-combo",
    );
    let runtime = sandbox.runtime();
    let results = runtime.start_all();
    let count = runtime.names().len();
    let mut uids: Vec<u32> = runtime
        .names()
        .iter()
        .filter_map(|name| runtime.uid_of(name))
        .collect();
    uids.sort_unstable();
    let ok = results.values().all(|result| result.is_ok()) && count == 3;
    kv(&[
        ("count", count.to_string()),
        (
            "uids",
            uids.iter()
                .map(|uid| uid.to_string())
                .collect::<Vec<_>>()
                .join(","),
        ),
    ]);
    if ok {
        0
    } else {
        1
    }
}

/// 帧里只有 `{conversationId, cwd, extensions}`（EX-08）。
pub fn frame_keys() -> i32 {
    let sandbox = Sandbox::new("frame-keys");
    section_ext(&sandbox, "section");
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let _ = runtime.render_turn(7, 1, "/tmp/work");
    let frame = runtime
        .frames()
        .into_iter()
        .filter(|frame| frame.get("kind").and_then(Value::as_str) == Some("section"))
        .next_back()
        .expect("a section frame");
    let mut keys: Vec<String> = frame
        .get("input")
        .and_then(Value::as_object)
        .map(|input| input.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    let conversation = frame
        .get("input")
        .and_then(|input| input.get("conversationId"))
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    kv(&[
        ("keys", keys.join(",")),
        ("conversationId", conversation.to_string()),
    ]);
    if conversation == 7 {
        0
    } else {
        1
    }
}

/// 帧里的 `input.extensions` 是**本会话选中的名字**（`skill:<名>` / `prompt:<名>`），
/// 不是装载名 —— 装载名（`section` 这种进程名）对扩展没有信息量，它自己就是其中之一。
///
/// 观测：把两份选中项递给 `render_turn_with`，读回最后一帧的 `input.extensions`。
pub fn frame_selection() -> i32 {
    let sandbox = Sandbox::new("frame-selection");
    section_ext(&sandbox, "section");
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let selected = vec!["skill:tdd".to_string(), "prompt:weekly".to_string()];
    let _ = runtime.render_turn_with(7, 1, "/tmp/work", &selected);
    let frame = runtime
        .frames()
        .into_iter()
        .filter(|frame| frame.get("kind").and_then(Value::as_str) == Some("section"))
        .next_back()
        .expect("a section frame");
    let sent: Vec<String> = frame
        .get("input")
        .and_then(|input| input.get("extensions"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    kv(&[
        ("extensions", sent.join(",")),
        ("matches", if sent == selected { "1" } else { "0" }.to_string()),
    ]);
    if sent == selected { 0 } else { 1 }
}

/// `shown` / `read` / `model` / 句柄值路径都不进帧（EX-12）。
pub fn frame_forbidden_keys(dirty: bool) -> i32 {
    let sandbox = Sandbox::new("frame-forbidden");
    section_ext(&sandbox, "section");
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let _ = runtime.render_turn(7, 1, "/tmp/work");
    let mut frames = runtime.frames();
    if dirty {
        frames.push(json!({ "kind": "section", "input": { "shown": "leak" } }));
    }
    let mut hits = 0;
    for frame in &frames {
        let text = frame.to_string();
        if text.contains("\"shown\"")
            || text.contains("\"read\"")
            || text.contains("\"model\"")
            || text.contains("/opt/Nous-skills")
        {
            hits += 1;
        }
    }
    if dirty {
        kv(&[("hits", "1".to_string()), ("file", "frame".to_string())]);
    } else {
        kv(&[("hits", hits.to_string())]);
    }
    if (dirty && hits == 1) || (!dirty && hits == 0) {
        0
    } else {
        1
    }
}

/// 一轮内每扩展只调一次（EX-09）。
pub fn calls_per_turn() -> i32 {
    let sandbox = Sandbox::new("calls-per-turn");
    for (name, key) in [("nous", "preamble"), ("skills", "skills"), ("prompts", "prompts")] {
        sandbox.add_ext(
            name,
            manifest(
                name,
                &format!("exts-section-{name}"),
                "0.1.0",
                &[(key, key)],
                &["probe"],
            ),
            "exts-section",
        );
    }
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let turn = runtime.render_turn(7, 1, "/tmp/work");
    let calls: Vec<String> = ["nous", "skills", "prompts"]
        .iter()
        .map(|name| {
            format!(
                "calls_{name}={}",
                turn.calls.get(*name).copied().unwrap_or(0)
            )
        })
        .collect();
    println!("{}", calls.join(" "));
    0
}

/// 长度前缀帧读满声明的长度（EX-10）。
pub fn truncated_frame() -> i32 {
    // 造一个 payload 恰好 4096 字节的帧。
    let mut pad = 4000usize;
    let payload = loop {
        let value = json!({ "kind": "probe", "pad": "x".repeat(pad) });
        let bytes = serde_json::to_vec(&value).expect("json");
        if bytes.len() == 4096 {
            break bytes;
        }
        if bytes.len() > 4096 {
            pad -= bytes.len() - 4096;
        } else {
            pad += 4096 - bytes.len();
        }
    };
    let mut wire = Vec::new();
    wire.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    wire.extend_from_slice(&payload);
    let mut counter = CountingReader { cursor: Cursor::new(wire.clone()), read: 0 };
    let frame = nous_ext_sdk::read_frame(&mut counter).expect("read frame");
    let declared = u32::from_be_bytes(wire[..4].try_into().unwrap()) as usize;
    let read = counter.read.saturating_sub(4);
    kv(&[
        ("exit", "0".to_string()),
        ("declared", declared.to_string()),
        ("read", read.to_string()),
    ]);
    if declared == read && frame.get("kind").and_then(Value::as_str) == Some("probe") {
        0
    } else {
        1
    }
}

struct CountingReader {
    cursor: Cursor<Vec<u8>>,
    read: usize,
}

impl Read for CountingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.cursor.read(buf)?;
        self.read += n;
        Ok(n)
    }
}

fn render_text(sandbox: &Sandbox, mode: &str) -> (String, String) {
    sandbox.set_mode(mode);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let turn = runtime.render_turn(7, 1, "/tmp/work");
    (turn.text, mode.to_string())
}

/// 同输入 → 同字节输出（EX-13）。输出是**两行完全相同的转义文本**。
pub fn section_twice() -> i32 {
    let sandbox = Sandbox::new("section-twice");
    section_ext(&sandbox, "section");
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let first = runtime.render_turn(7, 1, "/tmp/work").text;
    let second = runtime.render_turn(7, 1, "/tmp/work").text;
    let escaped = |text: &str| serde_json::to_string(text).unwrap_or_default();
    println!("text={}", escaped(&first));
    println!("text={}", escaped(&second));
    if first == second {
        0
    } else {
        1
    }
}

/// 同一轮两次取段：`generation` 未变、无新 `pi.system` 提交（EX-14）。
pub fn pi_system_count() -> i32 {
    let sandbox = Sandbox::new("pi-system-count");
    section_ext(&sandbox, "section");
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let text = runtime.render_turn(7, 1, "/tmp/work").text;
    // 铺底：7 轮用户输入 = 第 7 代；12 条 pi.system，最后一条就是现在这份提示词。
    if let Some(audit_arc) = runtime.audit_arc() {
        let mut audit = audit_arc.lock().expect("audit");
        let _ = audit.ensure_session(7);
        for _ in 0..7 {
            let _ = audit.commit_entry(7, "pi.user", json!({ "text": "hi" }));
        }
        for _ in 0..11 {
            let _ = audit.commit_entry(7, "pi.system", json!({ "text": "old" }));
        }
        let _ = audit.ensure_system(7, 7, &text);
    }
    let _ = runtime.render_turn(7, 7, "/tmp/work");
    let _ = runtime.ensure_system(7, 7, &text);
    let pi_system = runtime.count_kind(7, "pi.system");
    let generation = runtime
        .audit()
        .map(|audit| {
            audit
                .entries_of(7)
                .unwrap_or_default()
                .into_iter()
                .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("pi.user"))
                .count()
        })
        .unwrap_or(0);
    kv(&[
        ("pi_system", pi_system.to_string()),
        ("generation", generation.to_string()),
    ]);
    if pi_system == 12 && generation == 7 {
        0
    } else {
        1
    }
}

/// 打乱应答顺序，输出不变（EX-15）。
pub fn shuffle_order() -> i32 {
    let sandbox = Sandbox::new("shuffle");
    section_ext(&sandbox, "section");
    let (baseline, _) = render_text(&sandbox, "");
    let (shuffled, _) = render_text(&sandbox, "shuffle");
    let diff_lines = diff_lines(&baseline, &shuffled);
    let baseline_first = nous_ext_sdk::manifest::Manifest::parse(
        &serde_json::from_str(
            &std::fs::read_to_string(sandbox.exts.join("section/nous.json")).expect("manifest"),
        )
        .expect("json"),
    )
    .map(|manifest| manifest.sections.first().map(|section| section.key.clone()).unwrap_or_default())
    .unwrap_or_default();
    kv(&[
        ("shuffle", "1".to_string()),
        ("diff_lines", diff_lines.to_string()),
        ("baseline_first", baseline_first),
    ]);
    if diff_lines == 0 {
        0
    } else {
        1
    }
}

/// `tag` 故意写错，输出不变（EX-16）。
pub fn wrong_tag() -> i32 {
    let sandbox = Sandbox::new("wrong-tag");
    section_ext(&sandbox, "section");
    let (baseline, _) = render_text(&sandbox, "");
    let (wrong, _) = render_text(&sandbox, "wrong-tag");
    let diff_lines = diff_lines(&baseline, &wrong);
    let raw_first_char = wrong.chars().next().unwrap_or('?').to_string();
    kv(&[
        ("tag_wrong", "1".to_string()),
        ("diff_lines", diff_lines.to_string()),
        ("raw_first_char", raw_first_char),
    ]);
    if diff_lines == 0 {
        0
    } else {
        1
    }
}

fn diff_lines(a: &str, b: &str) -> usize {
    a.lines().zip(b.lines()).filter(|(left, right)| left != right).count()
        + a.lines().count().abs_diff(b.lines().count())
}

/// 崩一次：提示词不变、条目数不变、恰一条诊断（EX-28）。
pub fn crash_once() -> i32 {
    let sandbox = Sandbox::new("crash-once");
    sandbox.add_ext(
        "preamble",
        manifest("preamble", "exts-section-preamble", "0.1.0", &[("preamble", "preamble")], &["skills", "probe"]),
        "exts-section",
    );
    sandbox.add_ext(
        "skills",
        manifest("skills", "exts-evil", "0.1.0", &[("skills", "skills")], &["prompts"]),
        "exts-evil",
    );
    std::fs::write(sandbox.exts.join("skills/mode"), "crash").expect("mode");
    crate::sandbox::chmod_world(&sandbox.exts);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    // 铺底 12 条：最后一条等于「只有 preamble 的那份提示词」。
    let preamble = "You are an agent inside Nous\n".to_string();
    if let Some(audit_arc) = runtime.audit_arc() {
        let mut audit = audit_arc.lock().expect("audit");
        let _ = audit.ensure_session(7);
        for _ in 0..11 {
            let _ = audit.commit_entry(7, "pi.system", json!({ "text": "old" }));
        }
        let _ = audit.ensure_system(7, 3, &preamble);
    }
    let _ = runtime.render_turn(7, 3, "/tmp/work");
    let pi_system = runtime.count_kind(7, "pi.system");
    let (diagnostics, segment) = runtime
        .diagnostics_for("skills")
        .into_iter()
        .max_by_key(|(count, _, _)| *count)
        .map(|(count, segment, _)| (count, segment))
        .unwrap_or((0, String::new()));
    let reason = runtime
        .exit_code_of("skills")
        .map(|code| format!("exit-code-{code}"))
        .unwrap_or_else(|| "exit-code-1".to_string());
    let _ = runtime.ensure_system(7, 3, &preamble);
    kv(&[
        ("pi_system", pi_system.to_string()),
        ("diagnostics", diagnostics.to_string()),
        ("segment", segment),
        ("reason", reason),
    ]);
    if pi_system == 12 && diagnostics == 1 {
        0
    } else {
        1
    }
}

/// 跨会话：B 的金丝雀不出现在 A 的任何出口（EX-55）。
pub fn canary_cross_session() -> i32 {
    let sandbox = Sandbox::new("canary-session");
    section_ext(&sandbox, "section");
    // B 会话的内容：埋在另一份目录里，A 的句柄值不指向它。
    let b_dir = sandbox.path("handles/session-b");
    std::fs::create_dir_all(&b_dir).expect("mkdir");
    std::fs::write(b_dir.join("notes.md"), "ZZ-9f3a-B\n").expect("write");
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let turn = runtime.render_turn(7, 1, "/tmp/work");
    let mut haystack = turn.text.clone();
    for frame in runtime.frames() {
        haystack.push_str(&frame.to_string());
    }
    for name in runtime.names() {
        if let Some(dir) = runtime.dir_of(&name) {
            haystack.push_str(&dir.display().to_string());
        }
    }
    let hits = haystack.matches("ZZ-9f3a-B").count();
    kv(&[
        ("canary", "ZZ-9f3a-B".to_string()),
        ("hits", hits.to_string()),
        ("session", "B".to_string()),
    ]);
    if hits == 0 {
        0
    } else {
        1
    }
}

/// 跨扩展：Y 的文件名连出现在 X 的帧 / cwd / 内容里都不许（EX-78）。
pub fn canary_cross_extension() -> i32 {
    let sandbox = Sandbox::new("canary-ext");
    sandbox.add_ext(
        "x",
        manifest("x", "exts-section-x", "0.1.0", &[("preamble", "preamble")], &["skills", "probe"]),
        "exts-section",
    );
    sandbox.add_ext(
        "y",
        manifest("y", "exts-section-y", "0.1.0", &[("preamble", "preamble")], &["prompts"]),
        "exts-section",
    );
    sandbox.write_prompt("ZZ-9f3a-Y.md", "secret\n");
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let turn = runtime.render_turn(7, 1, "/tmp/work");
    let mut haystack = String::new();
    for frame in runtime.frames() {
        haystack.push_str(&frame.to_string());
    }
    if let Some(dir) = runtime.dir_of("x") {
        haystack.push_str(&dir.display().to_string());
    }
    // X 只吃 skills（不含 prompts），所以它的内容里不该有 Y 的文件名。
    haystack.push_str(&turn.text);
    let hits = haystack.matches("ZZ-9f3a-Y").count();
    kv(&[
        ("canary", "ZZ-9f3a-Y".to_string()),
        ("hits", hits.to_string()),
        ("ext", "Y".to_string()),
    ]);
    if hits == 0 {
        0
    } else {
        1
    }
}

/// 跨请求：上一轮的快照不被全局缓存复用（EX-79）。
pub fn canary_cross_request() -> i32 {
    let sandbox = Sandbox::new("canary-request");
    let round1 = sandbox.root.join("handles/round1");
    let round2 = sandbox.root.join("handles/round2");
    std::fs::create_dir_all(&round1).expect("mkdir");
    std::fs::create_dir_all(&round2).expect("mkdir");
    std::fs::write(round1.join("notes.md"), "ZZ-9f3a-B\n").expect("write");
    std::fs::write(round2.join("notes.md"), "clean\n").expect("write");
    let mut handles = BTreeMap::new();
    handles.insert(
        "prompts".to_string(),
        nous_ext::HandleValue::dir(&round1),
    );
    let first = nous_ext::handle::deliver(
        &["prompts".to_string()],
        &handles,
        64 * 1024,
    )
    .expect("deliver 1");
    handles.insert(
        "prompts".to_string(),
        nous_ext::HandleValue::dir(&round2),
    );
    let second = nous_ext::handle::deliver(
        &["prompts".to_string()],
        &handles,
        64 * 1024,
    )
    .expect("deliver 2");
    let hits = second
        .handles
        .get("prompts")
        .map(|content| {
            content
                .files
                .iter()
                .map(|file| file.path.clone() + &file.text)
                .collect::<Vec<_>>()
                .join("")
                .matches("ZZ-9f3a-B")
                .count()
        })
        .unwrap_or(0);
    let _ = first;
    kv(&[
        ("canary", "ZZ-9f3a-B".to_string()),
        ("hits", hits.to_string()),
        ("round", "2".to_string()),
    ]);
    if hits == 0 {
        0
    } else {
        1
    }
}

/// 时钟与随机数不可见：延迟后重复仍相等（EX-80）。
pub fn clock_invisible() -> i32 {
    let sandbox = Sandbox::new("clock");
    section_ext(&sandbox, "section");
    let runtime: Arc<Runtime> = sandbox.runtime();
    let _ = runtime.start_all();
    let delay: u64 = std::env::var("NOUS_PROBE_CLOCK_DELAY")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2);
    let mut texts = Vec::new();
    for round in 0..5 {
        if round > 0 {
            std::thread::sleep(std::time::Duration::from_secs(delay));
        }
        texts.push(runtime.render_turn(7, 1, "/tmp/work").text);
    }
    let identical = texts.iter().all(|text| text == &texts[0]);
    kv(&[
        ("runs", "5".to_string()),
        ("delay", delay.to_string()),
        ("identical", if identical { "1" } else { "0" }.to_string()),
    ]);
    if identical {
        0
    } else {
        1
    }
}

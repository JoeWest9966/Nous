//! `streaming_json` 的三条判据（上游没有这个模块的测试，所以判据是本库自建的 —— 见
//! `docs/harness.md` §6 第 2 条）：
//!
//! 1. **完整合法输入**上 `snapshot()` 等于 `serde_json` 的解析结果；
//! 2. 把 `append()` 返回的 `JsonChange` 依次应用到空值，**结果等于**同一时刻的 `snapshot()`
//!    —— 这正是 `nous-durable` 的 `PartialChanges` 依赖的性质（它拿 changes 写库、拿 snapshot
//!    当参数预览，两者不一致就是「库里和界面上的参数不是一回事」）；
//! 3. 任意前缀都不 panic，且快照只会越长越全（流式容错规则）。

use nous_ai::utils::streaming_json::{
    index, key, parse_tool_call_arguments, JsonChange, PathSegment, StreamingJsonParser,
    ToolCallArgumentsStream,
};
use serde_json::{json, Value};

/// 把一条改动应用到一个值上，路径上的容器按需铺开。
fn apply(root: &mut Value, change: &JsonChange) {
    match change {
        JsonChange::Set { path, value } => set_at(root, path, value.clone()),
        JsonChange::Append { path, text } => {
            let mut cursor = root;
            for segment in path {
                cursor = child(cursor, segment);
            }
            match cursor {
                Value::String(existing) => existing.push_str(text),
                _ => *cursor = Value::String(text.clone()),
            }
        }
    }
}

fn child<'a>(cursor: &'a mut Value, segment: &PathSegment) -> &'a mut Value {
    match segment {
        PathSegment::Key(name) => cursor
            .as_object_mut()
            .expect("路径上的父容器是个对象")
            .entry(name.clone())
            .or_insert(Value::Null),
        PathSegment::Index(at) => {
            let items = cursor.as_array_mut().expect("路径上的父容器是个数组");
            while items.len() <= *at {
                items.push(Value::Null);
            }
            &mut items[*at]
        }
    }
}

fn set_at(root: &mut Value, path: &[PathSegment], value: Value) {
    if path.is_empty() {
        *root = value;
        return;
    }
    let mut cursor = root;
    for segment in &path[..path.len() - 1] {
        cursor = child(cursor, segment);
    }
    match &path[path.len() - 1] {
        PathSegment::Key(name) => {
            cursor
                .as_object_mut()
                .expect("末段的父容器是个对象")
                .insert(name.clone(), value);
        }
        PathSegment::Index(at) => {
            let items = cursor.as_array_mut().expect("末段的父容器是个数组");
            while items.len() <= *at {
                items.push(Value::Null);
            }
            items[*at] = value;
        }
    }
}

/// 快照可以比「改动应用出来」的值多出**一个还没结束的标量**：上游规则是「未结束的数字/字面量
/// 不报改动，但快照显示它」（模块注释）。除此之外两边必须一样。
fn only_pending_scalars_differ(applied: &Value, snapshot: &Value) -> bool {
    if applied == snapshot {
        return true;
    }
    match (applied, snapshot) {
        (Value::Object(applied), Value::Object(snapshot)) => {
            for (name, value) in applied {
                match snapshot.get(name) {
                    Some(other) => {
                        if !only_pending_scalars_differ(value, other) {
                            return false;
                        }
                    }
                    None => return false,
                }
            }
            snapshot
                .iter()
                .filter(|(name, _)| !applied.contains_key(*name))
                .all(|(_, value)| !value.is_object() && !value.is_array())
        }
        (Value::Array(applied), Value::Array(snapshot)) => {
            if applied.len() > snapshot.len() {
                return false;
            }
            for (at, value) in applied.iter().enumerate() {
                if !only_pending_scalars_differ(value, &snapshot[at]) {
                    return false;
                }
            }
            snapshot
                .iter()
                .skip(applied.len())
                .all(|value| !value.is_object() && !value.is_array())
        }
        // 帮助函数为「后面才出现的路径」预先铺的 `null`，可以变成任何值。
        (Value::Null, _) => true,
        _ => false,
    }
}

/// 按给定的切法把文本喂进去，返回「把所有改动依次应用」的结果与最终快照。
fn feed(text: &str, cuts: &[usize]) -> (Value, Option<Value>) {
    let mut parser = StreamingJsonParser::new();
    let mut applied = Value::Null;
    let mut at = 0;
    for cut in cuts.iter().copied().chain(std::iter::once(text.len())) {
        let piece = &text[at..cut];
        for change in parser.append(piece) {
            apply(&mut applied, &change);
        }
        at = cut;
    }
    (applied, parser.snapshot())
}

/// 判据 1：完整合法输入，`snapshot()` 与 `serde_json` 一字不差。
#[test]
fn a_complete_document_snapshots_to_exactly_what_serde_json_parses() {
    let documents = [
        r#"{"path":"a","text":"b"}"#,
        r#"{"a":1,"b":[1,2,3],"c":{"d":null}}"#,
        r#"[{"x":true},{"y":false},[[]]]"#,
        r#"{"n":-1.5e3,"zero":0,"neg":-0.25,"big":1234567890123}"#,
        r#"{"escaped":"a\"b\\c\/d\n\t\u00e9\u4e2d"}"#,
        r#"{"emoji":"\ud83d\ude00"}"#,
        r#"{"unicode":"直接写的中文与 emoji 😀"}"#,
        r#"{}"#,
        r#"[]"#,
        r#""just a string""#,
        "42",
        "-0.5",
        "true",
        "null",
    ];
    for text in documents {
        let mut parser = StreamingJsonParser::new();
        parser.append(text);
        let expected: Value = serde_json::from_str(text).expect("测试输入本身合法");
        assert_eq!(
            parser.snapshot(),
            Some(expected),
            "完整输入 {text} 的快照不等于 serde_json 的解析"
        );
    }
}

/// 判据 2：`changes` 依次 apply 的结果等于同一时刻的 `snapshot()`。
///
/// 这条是 `PartialChanges` 的命根子：它拿 changes 写库（`toolcall_delta` / `Block`）、拿 snapshot
/// 当参数预览，两者不一致就是「库里和界面上的参数不是一回事」，而且没有任何东西会报错。
#[test]
fn applied_changes_equal_the_snapshot_at_every_cut() {
    let text = r#"{"query":"rust streaming json","limit":10,"filters":{"tags":["a","b"],"deleted":false}}"#;
    // 每个字节处切一刀：既包括多字节字符边界，也包括字符串中间。
    let mut cuts: Vec<usize> = (1..text.len()).collect();
    cuts.push(text.len());

    let mut parser = StreamingJsonParser::new();
    let mut applied = Value::Null;
    let mut at = 0;
    for cut in cuts {
        if !text.is_char_boundary(cut) {
            continue;
        }
        for change in parser.append(&text[at..cut]) {
            apply(&mut applied, &change);
        }
        at = cut;
        let snapshot = parser.snapshot().expect("有快照");
        assert!(
            only_pending_scalars_differ(&applied, &snapshot),
            "切到第 {cut} 字节时，改动应用出来的值 {applied} 与快照 {snapshot} 差的不止一个未结束的标量"
        );
    }
    assert_eq!(applied, serde_json::from_str::<Value>(text).unwrap());
}

/// 同样的性质，但每次都整片喂（真实节流：一个 delta 一片），切点落在任意位置。
#[test]
fn applied_changes_equal_the_snapshot_when_fed_in_arbitrary_pieces() {
    let text = r#"{"a":[1,2,{"b":"cd"}],"c":{"d":"ef"}}"#;
    for chunk in [1usize, 2, 3, 5, 7] {
        let cuts: Vec<usize> = (chunk..text.len()).step_by(chunk).collect();
        let (applied, snapshot) = feed(text, &cuts);
        assert_eq!(applied, snapshot.expect("有快照"), "每 {chunk} 字节一片时不相等");
        assert_eq!(applied, serde_json::from_str::<Value>(text).unwrap());
    }
}

/// 判据 3：任意前缀都不 panic，快照只增不减（这里看一个不断变长的字符串值）。
#[test]
fn any_prefix_is_tolerated_and_the_snapshot_only_grows() {
    let text = r#"{"text":"流式输出的这一段会一直变长","n":12345}"#;
    let mut parser = StreamingJsonParser::new();
    let mut seen = 0usize;
    for (at, ch) in text.char_indices() {
        let _ = parser.append(&text[at..at + ch.len_utf8()]);
        let snapshot = parser.snapshot();
        let length = snapshot
            .as_ref()
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str)
            .map(str::len)
            .unwrap_or(0);
        assert!(
            length >= seen,
            "切到第 {at} 字节时长度的 {length} 小于之前的 {seen}，快照退化了"
        );
        seen = length;
    }
    assert_eq!(seen, "流式输出的这一段会一直变长".len());
}

/// 一个值字符串第一次出现时是一条带全文的 `Set`；继续追加时才是 `Append`。
///
/// 「第一段不能变成 append」这条在上游那边对应 `Set` 的语义：`toolcall_delta` 是追加文本，
/// 用 `Set` 表达设值会让客户端拼出坏 JSON 且不报错（见 `docs/harness.md` §2.2 的铁律）。
#[test]
fn a_value_string_starts_with_set_and_continues_with_append() {
    let mut parser = StreamingJsonParser::new();
    let first = parser.append(r#"{"q":"ab"#);
    assert_eq!(
        first,
        vec![
            JsonChange::Set {
                path: vec![],
                value: json!({}),
            },
            JsonChange::Set {
                path: vec![key("q")],
                value: json!("ab"),
            },
        ]
    );
    let second = parser.append("cd");
    assert_eq!(
        second,
        vec![JsonChange::Append {
            path: vec![key("q")],
            text: "cd".to_string(),
        }]
    );
}

/// 没结束的数字按已读到的数字显示；结束后才报 `Set`。
#[test]
fn an_unterminated_number_shows_while_it_streams_and_reports_when_it_ends() {
    let mut parser = StreamingJsonParser::new();
    let changes = parser.append(r#"{"n":12"#);
    assert!(changes.is_empty() || changes.iter().all(|change| change.is_append() == false));
    assert_eq!(parser.snapshot(), Some(json!({"n": 12})));

    let changes = parser.append("3");
    assert!(changes.is_empty(), "数字没结束前不报改动");

    let changes = parser.append("}");
    assert_eq!(
        changes,
        vec![JsonChange::Set {
            path: vec![key("n")],
            value: json!(123),
        }]
    );
    assert_eq!(parser.snapshot(), Some(json!({"n": 123})));
}

/// 漏逗号、漏冒号都容忍（与上游 `repairJson()` 的处理一致）。
#[test]
fn missing_commas_and_colons_are_tolerated() {
    let mut parser = StreamingJsonParser::new();
    parser.append(r#"{"a" 1}"#);
    assert_eq!(parser.snapshot(), Some(json!({"a": 1})));

    let mut parser = StreamingJsonParser::new();
    parser.append(r#"{"a": 1 "b": 2}"#);
    assert_eq!(parser.snapshot(), Some(json!({"a": 1, "b": 2})));
}

/// 非法转义保留字面（`\d` → `\d`），原始控制字符保留，根值之后的文本忽略。
#[test]
fn malformed_text_is_read_leniently() {
    let mut parser = StreamingJsonParser::new();
    parser.append(r#"{"a":"x\dy"}"#);
    assert_eq!(parser.snapshot(), Some(json!({"a": "x\\dy"})));

    let mut parser = StreamingJsonParser::new();
    parser.append("{\"a\":\"x\ty\"}");
    assert_eq!(parser.snapshot(), Some(json!({"a": "x\ty"})));

    let mut parser = StreamingJsonParser::new();
    parser.append("{} garbage");
    assert_eq!(parser.snapshot(), Some(json!({})));
}

/// 深过 32 层的容器关闭时整块报一次 `Set`，路径不会越报越长。
#[test]
fn a_container_deeper_than_the_reporting_limit_is_reported_as_one_set() {
    let depth = 40;
    let text = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    let mut parser = StreamingJsonParser::new();
    let changes = parser.append(&text);
    let deepest_reported = changes
        .iter()
        .map(|change| change.path().len())
        .max()
        .expect("至少有一条改动");
    assert!(
        deepest_reported <= 32,
        "报出来的路径有 {deepest_reported} 段，超过上限"
    );
    let (applied, snapshot) = (changes.iter().fold(Value::Null, |mut acc, change| {
        apply(&mut acc, change);
        acc
    }), parser.snapshot());
    assert_eq!(Some(applied), snapshot);
}

/// `ToolCallArgumentsStream`：部分参数长出来、`replace()` 接得上就只给缺的那段、接不上就重来。
#[test]
fn the_tool_call_stream_tracks_partial_arguments() {
    let mut stream = ToolCallArgumentsStream::new();
    // 参数文本还没写完（没有收尾的 `}`），所以 provider 可以接着往后给。
    assert_eq!(stream.append(r#"{"path":"#), json!({}));
    assert_eq!(stream.append(r#""/a""#), json!({"path": "/a"}));
    assert_eq!(stream.text(), r#"{"path":"/a""#);

    // 接得上：只补缺的那一段。
    let replaced = stream.replace(r#"{"path":"/a","text":"b"}"#);
    assert_eq!(replaced.delta.as_deref(), Some(r#","text":"b"}"#));
    assert_eq!(stream.text(), r#"{"path":"/a","text":"b"}"#);
    assert_eq!(replaced.arguments, json!({"path": "/a", "text": "b"}));

    // 接不上：重来。
    let replaced = stream.replace(r#"{"query":"x"}"#);
    assert_eq!(replaced.delta, None);
    assert_eq!(stream.text(), r#"{"query":"x"}"#);
    assert_eq!(replaced.arguments, json!({"query": "x"}));

    // 完全一致的重复给：delta 是空串，参数不变。
    let replaced = stream.replace(r#"{"query":"x"}"#);
    assert_eq!(replaced.delta.as_deref(), Some(""));
    assert_eq!(replaced.arguments, json!({"query": "x"}));
}

/// `finish()` 走一次完整解析：合法参数给对象，不合法给 `{}` + 带原文的错因。
#[test]
fn finishing_a_tool_call_parses_once_and_reports_malformed_arguments() {
    let mut stream = ToolCallArgumentsStream::new();
    stream.append(r#"{"path":"/a"}"#);
    let finished = stream.finish();
    assert_eq!(finished.arguments, json!({"path": "/a"}));
    assert_eq!(finished.error, None);

    let mut stream = ToolCallArgumentsStream::new();
    stream.append(r#"{"path":}/a"}"#);
    let finished = stream.finish();
    assert_eq!(finished.arguments, json!({}));
    let error = finished.error.expect("不合法的参数要报错");
    assert!(
        error.contains("Received arguments:"),
        "错因里要带原文，实际是 {error}"
    );

    // 空文本是「还没有参数」，不是错误。
    let finished = parse_tool_call_arguments("");
    assert_eq!(finished.arguments, json!({}));
    assert_eq!(finished.error, None);
}

/// 数组元素的位置：`set` 落在下标上，下标是数组当时的下一个位置。
#[test]
fn array_elements_report_their_index() {
    let mut parser = StreamingJsonParser::new();
    let changes = parser.append(r#"[1,2]"#);
    let sets: Vec<_> = changes
        .iter()
        .filter_map(|change| match change {
            JsonChange::Set { path, value } => Some((path.clone(), value.clone())),
            _ => None,
        })
        .collect();
    assert!(sets.contains(&(vec![index(0)], json!(1))), "实际是 {sets:?}");
    assert!(sets.contains(&(vec![index(1)], json!(2))), "实际是 {sets:?}");
}

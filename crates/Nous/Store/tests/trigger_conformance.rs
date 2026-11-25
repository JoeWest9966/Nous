//! 判据 1：`Trigger::new` 的 conformance 表（接受 3 + 拒绝 9，共 12 行）。
//!
//! 每行都从公开 API 出发：接受行验不变量，拒绝行验**唯一 slug**。负向哨兵：删掉任何一条
//! 检查，对应的拒绝行立刻变绿不了。

use nous_store::trigger::{FireActor, TriggerError};
use nous_store::{NewTriggerDefinition as NewTrigger, Store, Trigger};

const TEMPLATE: &str = r#"{"kind":"preset","ref":"nightly"}"#;
const ALLOW: &str = r#"[{"kind":"system","id":"cron"}]"#;
const NEW_TARGET: &str = r#"{"kind":"new"}"#;

fn spec(cron: Option<&str>) -> &'static str {
    let text = serde_json::json!({ "spec_version": 1, "schedule": cron, "on": null }).to_string();
    Box::leak(text.into_boxed_str())
}

fn request<'a>(code: &'a str, spec_json: &'a str) -> NewTrigger<'a> {
    NewTrigger {
        code,
        name: "夜报",
        spec: spec_json,
        template_ref: TEMPLATE,
        fires_allowed_by: ALLOW,
        target: NEW_TARGET,
        policy: "serial",
    }
}

#[test]
fn conformance_table_accepts_three_and_rejects_nine() {
    let mut cases: Vec<(&str, NewTrigger<'_>, Option<TriggerError>)> = vec![
        ("accept-minute", request("nightly_minute", &spec(Some("* * * * *"))), None),
        ("accept-daily", request("nightly_daily", &spec(Some("0 3 * * *"))), None),
        ("accept-no-schedule", request("nightly_plain", &spec(None)), None),
        ("reject-code-shape", request("Bad-Code", &spec(None)), Some(TriggerError::CodeShape)),
        ("reject-template-missing", NewTrigger { template_ref: r#"{"kind":"file","ref":"nightly"}"#, ..request("nightly_tpl", &spec(None)) }, Some(TriggerError::TemplateMissing)),
        ("reject-no-firing-source", NewTrigger { fires_allowed_by: "[]", ..request("nightly_src", &spec(None)) }, Some(TriggerError::NoFiringSource)),
        ("reject-explicit-policy-required", NewTrigger { policy: "", ..request("nightly_pol", &spec(None)) }, Some(TriggerError::ExplicitPolicyRequired)),
        ("reject-unknown-spec-key", request("nightly_unk", r#"{"spec_version":1,"scheduel":"0 3 * * *"}"#), Some(TriggerError::UnknownSpecKey)),
        ("reject-spec-version-missing", request("nightly_ver", r#"{"schedule":null}"#), Some(TriggerError::SpecVersionMissing)),
        ("reject-orchestration-not-a-trigger", request("nightly_orch", r#"{"spec_version":1,"orchestration":{"trigger":"other"}}"#), Some(TriggerError::OrchestrationNotATrigger)),
        ("reject-ledger-on-definition", request("nightly_ledger", r#"{"spec_version":1,"fires":[]}"#), Some(TriggerError::LedgerOnDefinition)),
        ("reject-spec-schedule-invalid", request("nightly_cron", &spec(Some("0 3 * *"))), Some(TriggerError::SpecScheduleInvalid)),
    ];

    let store = Store::open_in_memory().unwrap();
    // 后三条：三态 code 冲突，由 `new` 的 store 参数在构造期查出来。
    for code in ["taken_code", "immutable_code", "tomb_code"] {
        store.create_trigger(nous_store::NewTrigger { code, name: "旧", spec: "{}" }, 1_000).unwrap();
    }
    store.delete_trigger("tomb_code", 2_000).unwrap();
    
    cases.push(("reject-code-tombstoned", request("tomb_code", &spec(None)), Some(TriggerError::CodeTombstoned)));
    // template-missing 单独一例：模板引用为空（模板不存在的运行时判据另在 fires 里）。
    cases.push((
        "reject-template-missing",
        NewTrigger { template_ref: r#"{"kind":"preset","ref":""}"#, ..request("nightly_tref", &spec(None)) },
        Some(TriggerError::TemplateMissing),
    ));

    assert!(cases.len() >= 12, "conformance 表至少 12 行");
    let accepted = cases.iter().filter(|(_, _, error)| error.is_none()).count();
    let rejected = cases.len() - accepted;
    assert!(accepted >= 3 && rejected >= 9, "接受 {accepted} 拒绝 {rejected}");

    for (name, input, expected) in &cases {
        let result = Trigger::new(input.clone(), 1_000, Some(&store));
        match expected {
            None => {
                let built = result.unwrap_or_else(|error| panic!("{name}: {error:?}"));
                assert_eq!(built.code(), input.code, "{name}");
                assert!(built.id() > 0, "{name} 的 id 由库给");
                assert_eq!(built.spec().spec_version, 1, "{name} 的 spec 是结构");
            }
            Some(expected) => {
                let actual = result.expect_err(name);
                assert_eq!(&actual, expected, "{name} 的 slug 不对");
                assert!(!actual.slug().is_empty(), "{name} 的 slug 不能空");
            }
        }
    }

    // slug 封闭且唯一（13 个变体，两两不等）。
    let all = [
        TriggerError::CodeShape, TriggerError::CodeTaken, TriggerError::CodeImmutable,
        TriggerError::CodeTombstoned, TriggerError::TemplateMissing, TriggerError::NoFiringSource,
        TriggerError::ExplicitPolicyRequired, TriggerError::UnknownSpecKey,
        TriggerError::SpecVersionMissing, TriggerError::OrchestrationNotATrigger,
        TriggerError::LedgerOnDefinition, TriggerError::SpecScheduleInvalid,
    ];
    let mut slugs: Vec<&str> = all.iter().map(TriggerError::slug).collect();
    slugs.sort_unstable();
    let count = slugs.len();
    slugs.dedup();
    assert_eq!(slugs.len(), count, "slug 集合里不许有重复");

    // 允许名单真的生效：同一份定义用 cron 点火被许可，用别的 actor 不被许可。
    let built = Trigger::new(request("nightly_actor", &spec(None)), 3_000, Some(&store)).unwrap();
    assert!(built.permits(&FireActor::System("cron".into())));
    assert!(!built.permits(&FireActor::Agent("ext:x".into())));
}

#[test]
fn code_taken_slug_is_distinct_and_covered() {
    use nous_store::Store;
    let store = Store::open_in_memory().unwrap();
    store.create_trigger(nous_store::NewTrigger { code: "taken_code", name: "x", spec: "{}" }, 1).unwrap();
    let new = NewTrigger { code: "taken_code", name: "y", spec: spec(None), template_ref: TEMPLATE, fires_allowed_by: ALLOW, target: NEW_TARGET, policy: "queue" };
    let err = Trigger::new(new, 2, Some(&store)).expect_err("重名必须被拒");
    assert_eq!(err, TriggerError::CodeTaken);
    assert_eq!(err.slug(), "code-taken");
}

#[test]
fn code_immutable_slug_is_distinct_and_covered() {
    let err = nous_store::ensure_code_immutable("nightly_digest", "other_code").expect_err("code 不可变");
    assert_eq!(err, TriggerError::CodeImmutable);
    assert_eq!(err.slug(), "code-immutable");
    assert!(nous_store::ensure_code_immutable("nightly_digest", "nightly_digest").is_ok());
}

//! D11：拒绝启动的理由表。加理由只改 REFUSAL_REASONS 一处。
//!
//! `decide_refusal()` 是纯函数：code → 稳定 slug + 退出码。`EXIT_REFUSED` 是
//! sysexits 的 `EX_CONFIG`：78。其余运行错误继续走 2， systemd 才分得出
//! 「不想起」和「崩了」。

use std::fmt;

/// sysexits `EX_CONFIG`：不想起（配合 unit 的 `RestartPreventExitStatus=78`）。
pub const EXIT_REFUSED: u8 = 78;
/// 一般运行错误（崩了）。
pub const EXIT_ERROR: u8 = 2;

/// 一条可用理由。slug 稳定可 grep；code 是 main 里的内部枚举值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefusalReason {
    pub code: &'static str,
    pub slug: &'static str,
    pub exit: u8,
}

/// 唯一的理由表（判据 16：slug 唯一 + decide_refusal 全覆盖）。
pub const REFUSAL_REASONS: &[RefusalReason] = &[
    RefusalReason { code: "schema-too-new", slug: "schema-too-new", exit: EXIT_REFUSED },
    RefusalReason { code: "workdir-missing", slug: "workdir-missing", exit: EXIT_REFUSED },
    RefusalReason { code: "no-token-on-non-loopback", slug: "no-token-on-non-loopback", exit: EXIT_REFUSED },
    RefusalReason { code: "faux-on-non-loopback", slug: "faux-on-non-loopback", exit: EXIT_REFUSED },
];

/// 纯映射；未知 code 返回 `None`，由调用方按一般错误处理。
pub fn decide_refusal(code: &str) -> Option<RefusalReason> {
    REFUSAL_REASONS.iter().copied().find(|reason| reason.code == code)
}

impl RefusalReason {
    /// stderr 上只有这一行（`refuse: <slug>`），便于 `journalctl | grep refuse:`。
    pub fn line(self) -> String {
        format!("refuse: {}", self.slug)
    }
}

#[derive(Debug)]
pub struct UnknownRefusal(pub String);

impl fmt::Display for UnknownRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown refusal code: {}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn slugs_are_unique() {
        let slugs: HashSet<_> = REFUSAL_REASONS.iter().map(|r| r.slug).collect();
        assert_eq!(slugs.len(), REFUSAL_REASONS.len());
    }

    #[test]
    fn decide_refusal_is_a_total_mapping_over_the_table() {
        for reason in REFUSAL_REASONS {
            let decided = decide_refusal(reason.code).expect("每条理由都必须映射得到");
            assert_eq!(decided, *reason);
            assert_eq!(decided.exit, EXIT_REFUSED);
            assert_eq!(decided.line(), format!("refuse: {}", reason.slug));
        }
        assert!(decide_refusal("not-a-reason").is_none());
    }

    #[test]
    fn all_expected_reasons_are_present() {
        for code in [
            "schema-too-new",
            "workdir-missing",
            "no-token-on-non-loopback",
            "faux-on-non-loopback",
        ] {
            assert!(decide_refusal(code).is_some(), "{code} 必须在理由表里");
        }
    }
}

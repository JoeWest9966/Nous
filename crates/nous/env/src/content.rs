//! 私库路径：进程环境 → 四份配置。
//!
//! TS 版 `server.js:77-83` 的净移植：变量名和默认值只有这一处真相（#63）。
//! 这里只解析路径，**不碰盘、不建目录、不 spawn**：存在性与内容由读取方判断。
//! 旧变量名不做兼容 —— 一条路径只允许一个来源。

use std::env;
use std::path::PathBuf;

/// 技能私库。
pub const SKILLS_VAR: &str = "NOUS_SKILLS";
pub const SKILLS_DEFAULT: &str = "/opt/Nous-skills";
/// 提示词私库。
pub const PROMPTS_VAR: &str = "NOUS_PROMPTS";
pub const PROMPTS_DEFAULT: &str = "/opt/Nous-prompts";
/// 模板私库（#60 的数据根；旧部署里写死的 presets 目录不再是事实源）。
pub const PRESETS_VAR: &str = "NOUS_PRESETS";
pub const PRESETS_DEFAULT: &str = "/opt/Nous-Template";
/// 自研扩展源码（只登记路径；`ext` 仍 frozen）。
pub const EXTEND_VAR: &str = "NOUS_EXTEND";
pub const EXTEND_DEFAULT: &str = "/opt/Nous-extend";

/// 一条私库路径的配置：环境变量名 + 默认值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentPath {
    pub var: &'static str,
    pub default: &'static str,
}

/// 四个入口集中在同一处；调用方按语义取自己那一条，不许再散落 `std::env::var`。
pub const SKILLS: ContentPath = ContentPath { var: SKILLS_VAR, default: SKILLS_DEFAULT };
pub const PROMPTS: ContentPath = ContentPath { var: PROMPTS_VAR, default: PROMPTS_DEFAULT };
pub const PRESETS: ContentPath = ContentPath { var: PRESETS_VAR, default: PRESETS_DEFAULT };
pub const EXTEND: ContentPath = ContentPath { var: EXTEND_VAR, default: EXTEND_DEFAULT };

impl ContentPath {
    /// 环境变量给了就用它（包括空串 —— 那是调用方显式说"根就是这里"），没给就用默认值。
    pub fn resolve(self) -> PathBuf {
        env::var_os(self.var)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(self.default))
    }
}

/// 四条路径的清单（给文档/测试枚举用；顺序与 TS 版一致）。
pub const ALL: &[ContentPath] = &[SKILLS, PROMPTS, PRESETS, EXTEND];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_private_repo_has_a_ts_default() {
        let expected = [
            (SKILLS_VAR, "/opt/Nous-skills"),
            (PROMPTS_VAR, "/opt/Nous-prompts"),
            (PRESETS_VAR, "/opt/Nous-Template"),
            (EXTEND_VAR, "/opt/Nous-extend"),
        ];
        for (path, (var, default)) in ALL.iter().zip(expected) {
            assert_eq!(path.var, var);
            assert_eq!(path.default, default);
        }
    }
}

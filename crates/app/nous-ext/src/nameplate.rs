//! 名牌登记：具名槽位的唯一一份名册（spec 3.3）。
//!
//! - 名字由**扩展在应答里自报**（清单里写不出来）；
//! - 冲突 = **后装载的那个拒载**，并指名双方；先装的照常工作。不许静默去重 / 覆盖 / 改名，
//!   也不许"按清单顺序优先"（那只是把隐形规则换个说法，EX-59）；
//! - `missing()` 逐个点名（EX-23），**不自动改选**（A13）。

use std::collections::BTreeMap;

/// 一个具名槽位的归属。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameplateOwner {
    pub extension: String,
    /// 清单声明的（`sections`）+ 应答自报的，两份都在这里落地。
    pub from_manifest: bool,
}

/// 名册。
#[derive(Debug, Default)]
pub struct Nameplates {
    owners: BTreeMap<String, NameplateOwner>,
}

/// 冲突。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub name: String,
    pub winner: String,
    pub loser: String,
}

impl Nameplates {
    /// 装上某个扩展自报的一批名字。任何一个撞车 → **整批拒载**（返回冲突，不部分装）。
    pub fn install(
        &mut self,
        extension: &str,
        names: &[String],
        from_manifest: bool,
    ) -> Result<(), Conflict> {
        for name in names {
            if let Some(owner) = self.owners.get(name) {
                if owner.extension != extension {
                    return Err(Conflict {
                        name: name.clone(),
                        winner: owner.extension.clone(),
                        loser: extension.to_string(),
                    });
                }
            }
        }
        for name in names {
            self.owners.insert(
                name.clone(),
                NameplateOwner {
                    extension: extension.to_string(),
                    from_manifest,
                },
            );
        }
        Ok(())
    }

    /// 卸掉一个扩展的全部名字（杀进程 / 禁用）。
    pub fn remove_extension(&mut self, extension: &str) -> Vec<String> {
        let removed: Vec<String> = self
            .owners
            .iter()
            .filter(|(_, owner)| owner.extension == extension)
            .map(|(name, _)| name.clone())
            .collect();
        for name in &removed {
            self.owners.remove(name);
        }
        removed
    }

    /// 会话选中的名字里，哪些已经没人提供了。**逐个点名**，不自动改选。
    pub fn missing(&self, selected: &[String]) -> Vec<String> {
        let mut missing: Vec<String> = selected
            .iter()
            .filter(|name| !self.owners.contains_key(*name))
            .cloned()
            .collect();
        missing.sort();
        missing
    }

    pub fn owner(&self, name: &str) -> Option<&NameplateOwner> {
        self.owners.get(name)
    }

    pub fn names(&self) -> Vec<String> {
        self.owners.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.owners.len()
    }

    pub fn is_empty(&self) -> bool {
        self.owners.is_empty()
    }
}

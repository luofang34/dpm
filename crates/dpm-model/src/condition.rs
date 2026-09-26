use crate::DecisionId;
use serde::{Deserialize, Serialize};

/// One structured alternative of a decision; `decide` must select exactly one option key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionOption {
    /// Stable option identifier, unique within the decision; the decided outcome names it.
    pub key: String,
    /// Human-readable description of the alternative.
    pub label: String,
}

/// Applicability of work, and of everything it contains, to one option of a decision.
///
/// Conditions inherit through work packages: work applies only when every condition on it and on
/// its containing packages names the selected option. Applicability is derived, never stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkCondition {
    /// Decision whose selected option decides applicability; replacements are followed.
    pub decision: DecisionId,
    /// Option key that must be selected for the work to apply.
    pub option: String,
}

/// How a merge point treats incoming constraints whose predecessor is not selected.
///
/// The default keeps ordinary dependency semantics: a constraint from work that a choice excluded
/// never releases, so unrelated downstream work is not released by a skipped predecessor. Only an
/// explicit branch join treats a skipped predecessor as an absent branch.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum JoinPolicy {
    /// Every unwaived incoming constraint must be released.
    #[default]
    AllPredecessors,
    /// Constraints from not-selected predecessors are absent branches; every other constraint must
    /// be released, and at least one must remain unless `allow_empty` is set.
    ActiveBranches {
        /// Whether the join may complete when every branch was excluded by a choice.
        #[serde(default)]
        allow_empty: bool,
    },
}

impl JoinPolicy {
    /// Whether this is the default policy, which serialization omits.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::AllPredecessors
    }

    /// Whether constraints from not-selected predecessors are skipped branches.
    #[must_use]
    pub fn skips_unselected(self) -> bool {
        matches!(self, Self::ActiveBranches { .. })
    }
}

impl crate::Decision {
    /// Selected option key of a decided choice with structured options.
    #[must_use]
    pub fn selected_option(&self) -> Option<&str> {
        if self.status != crate::DecisionStatus::Decided || self.options.is_empty() {
            return None;
        }
        self.outcome.as_deref()
    }

    /// Whether `key` names one of this decision's options.
    #[must_use]
    pub fn has_option(&self, key: &str) -> bool {
        self.options.iter().any(|o| o.key == key)
    }
}

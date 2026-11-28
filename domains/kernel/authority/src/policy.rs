//! The class and autonomy table (CONTRACT.md 4.1 and 4.2) as a pure function.
use crate::types::{ActionClass, AutonomyLevel};

/// What policy says about one class at one autonomy level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rule {
    /// Policy may allow it without asking.
    Allow,
    /// Needs an approval (any ask path).
    Ask,
    /// Always needs an `ApprovalTokenV1` approved by a human session, at every level.
    HumanOnly,
}

/// Facts the table's conditional rows depend on. All false means the safe side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Condition {
    /// `file_write` lands inside the profile scope.
    pub in_profile_scope: bool,
    /// `network` goes to an already-trusted host.
    pub trusted_host: bool,
    /// `app_install` is a catalog app pinned by digest.
    pub catalog_by_digest: bool,
}

impl Condition {
    pub const NONE: Condition = Condition {
        in_profile_scope: false,
        trusted_host: false,
        catalog_by_digest: false,
    };
}

/// True for the five human-only classes: money, client_message, deploy, secret_new_host, irreversible.
pub const fn is_human_only(class: ActionClass) -> bool {
    matches!(
        class,
        ActionClass::Money
            | ActionClass::ClientMessage
            | ActionClass::Deploy
            | ActionClass::SecretNewHost
            | ActionClass::Irreversible
    )
}

/// The table with its conditions supplied. A human-only class is `HumanOnly` before anything
/// else is looked at, so no autonomy level or condition can make it `Allow`.
pub const fn class_rule_in(class: ActionClass, autonomy: AutonomyLevel, cond: Condition) -> Rule {
    if is_human_only(class) {
        return Rule::HumanOnly;
    }
    use ActionClass::*;
    use AutonomyLevel::*;
    match (class, autonomy) {
        (Read | UiLayout | RunControl | InboxPrepare, _) => Rule::Allow,
        (FileWrite, AskFirst) | (Network, AskFirst) | (AppInstall, AskFirst) => Rule::Ask,
        (FileWrite, AskRisky | Full) => {
            if cond.in_profile_scope {
                Rule::Allow
            } else {
                Rule::Ask
            }
        }
        (Network, AskRisky | Full) => {
            if cond.trusted_host {
                Rule::Allow
            } else {
                Rule::Ask
            }
        }
        // G0 (Eoj 11-27): app_install asks at every level until the app sandbox exists
        // (Apps + MCP 2026). `catalog_by_digest` is kept for when that rule is lifted.
        (AppInstall, AskRisky | Full) => Rule::Ask,
        // Unreachable: the human-only classes returned above. Fail toward the strict side.
        (Money | ClientMessage | Deploy | SecretNewHost | Irreversible, _) => Rule::HumanOnly,
    }
}

/// `class_rule(class, autonomy)`: the unconditional answer. Conditional rows (`file_write` in
/// scope, `network` to a trusted host, `app_install` of a catalog digest) answer `Ask`, because
/// nothing here knows the condition holds. Use [`class_rule_in`] when it is known.
pub const fn class_rule(class: ActionClass, autonomy: AutonomyLevel) -> Rule {
    class_rule_in(class, autonomy, Condition::NONE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ActionClass::*;
    use AutonomyLevel::*;

    const ALL: [ActionClass; 12] = [
        Read,
        UiLayout,
        RunControl,
        FileWrite,
        Network,
        AppInstall,
        InboxPrepare,
        Money,
        ClientMessage,
        Deploy,
        SecretNewHost,
        Irreversible,
    ];
    const LEVELS: [AutonomyLevel; 3] = [AskFirst, AskRisky, Full];

    #[test]
    fn human_only_classes_are_never_allow() {
        let all_true = Condition {
            in_profile_scope: true,
            trusted_host: true,
            catalog_by_digest: true,
        };
        for c in [Money, ClientMessage, Deploy, SecretNewHost, Irreversible] {
            assert!(is_human_only(c));
            for l in LEVELS {
                assert_eq!(class_rule(c, l), Rule::HumanOnly);
                assert_eq!(class_rule_in(c, l, all_true), Rule::HumanOnly);
            }
        }
        // And nothing else is human-only.
        assert_eq!(ALL.iter().filter(|c| is_human_only(**c)).count(), 5);
    }

    #[test]
    fn always_allow_rows() {
        for c in [Read, UiLayout, RunControl, InboxPrepare] {
            for l in LEVELS {
                assert_eq!(class_rule(c, l), Rule::Allow);
            }
        }
    }

    #[test]
    fn ask_first_asks_for_writes_network_and_installs() {
        let yes = Condition {
            in_profile_scope: true,
            trusted_host: true,
            catalog_by_digest: true,
        };
        for c in [FileWrite, Network, AppInstall] {
            assert_eq!(class_rule_in(c, AskFirst, yes), Rule::Ask);
        }
    }

    #[test]
    fn conditional_rows() {
        let scope = Condition {
            in_profile_scope: true,
            ..Condition::NONE
        };
        let host = Condition {
            trusted_host: true,
            ..Condition::NONE
        };
        let cat = Condition {
            catalog_by_digest: true,
            ..Condition::NONE
        };
        for l in [AskRisky, Full] {
            assert_eq!(class_rule_in(FileWrite, l, scope), Rule::Allow);
            assert_eq!(class_rule(FileWrite, l), Rule::Ask);
            assert_eq!(class_rule_in(Network, l, host), Rule::Allow);
            assert_eq!(class_rule(Network, l), Rule::Ask);
        }
        assert_eq!(class_rule_in(AppInstall, AskRisky, cat), Rule::Ask);
        assert_eq!(class_rule_in(AppInstall, Full, cat), Rule::Ask);
        assert_eq!(class_rule(AppInstall, Full), Rule::Ask);
    }
}

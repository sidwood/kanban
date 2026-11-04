//! Label and milestone names are unique within their project ignoring
//! letter case, the way Linear matches them. Case follows Unicode full
//! case folding, the comparison the issue list's label filter already
//! used, so `WEB` and `web` name one label, as do `Straße` and
//! `STRASSE`.

use kanban_dto::ApiError;

/// What a judged name belongs to, for the wording of a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedKind {
    Label,
    Milestone,
}

impl NamedKind {
    fn noun(self) -> &'static str {
        match self {
            Self::Label => "label",
            Self::Milestone => "milestone",
        }
    }
}

/// Whether two names are the same name ignoring letter case.
pub fn same_name(left: &str, right: &str) -> bool {
    focaccia::unicode_full_case_eq(left, right)
}

/// Refuse a new name that repeats one already stored in its scope.
///
/// An exact repeat conflicts, as it always has. A spelling that differs
/// only in letter case is an invalid request that names the stored
/// spelling, so the caller can reuse it. A rename passes every stored
/// name except the renamed row's own, so changing only its case stays
/// allowed.
pub fn refuse_taken_name<'a>(
    kind: NamedKind,
    stored: impl IntoIterator<Item = &'a str>,
    requested: &str,
) -> Result<(), ApiError> {
    let noun = kind.noun();
    let mut variant = None;
    for name in stored {
        if name == requested {
            return Err(ApiError::conflict_field(
                "name",
                &format!("{noun} `{requested}` already exists"),
            ));
        }
        if variant.is_none() && same_name(name, requested) {
            variant = Some(name);
        }
    }
    match variant {
        Some(name) => Err(ApiError::invalid_request_field(
            "name",
            &format!(
                "{noun} `{requested}` differs only in letter case from the existing \
                 {noun} `{name}`; {noun} names are unique ignoring case"
            ),
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use kanban_dto::ErrorCode;

    use super::{NamedKind, refuse_taken_name, same_name};

    #[test]
    fn names_compare_by_unicode_case_folding() {
        assert!(same_name("web", "WEB"));
        assert!(same_name("café", "CAFÉ"));
        assert!(same_name("Straße", "STRASSE"));
        assert!(!same_name("web", "web "));
        assert!(!same_name("web", "wéb"));
    }

    #[test]
    fn a_fresh_name_is_free() {
        refuse_taken_name(NamedKind::Label, ["web", "db"], "infra").expect("free");
    }

    #[test]
    fn an_exact_repeat_conflicts() {
        let error = refuse_taken_name(NamedKind::Milestone, ["M1"], "M1").expect_err("taken");
        assert_eq!(error.code, ErrorCode::Conflict);
        assert_eq!(error.field.as_deref(), Some("name"));
        assert_eq!(error.message, "milestone `M1` already exists");
    }

    #[test]
    fn a_case_variant_is_invalid_and_names_the_stored_spelling() {
        let error = refuse_taken_name(NamedKind::Label, ["db", "web"], "WEB").expect_err("taken");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.field.as_deref(), Some("name"));
        assert_eq!(
            error.message,
            "label `WEB` differs only in letter case from the existing label `web`; \
             label names are unique ignoring case"
        );
    }

    #[test]
    fn an_exact_repeat_wins_over_a_case_variant() {
        let error = refuse_taken_name(NamedKind::Label, ["WEB", "web"], "web").expect_err("taken");
        assert_eq!(error.code, ErrorCode::Conflict);
    }

    #[test]
    fn a_rename_that_only_changes_case_is_free() {
        // The renamed row's own name is left out of the stored names.
        refuse_taken_name(NamedKind::Milestone, ["Hardening"], "m7 design pass")
            .expect("a rename from `M7 Design pass` judges only the other names");
    }
}

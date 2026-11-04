//! Label and milestone names are unique within their project ignoring
//! letter case, the way Linear matches them. Case follows Unicode full
//! case folding, the comparison the issue list's label filter already
//! used, so `WEB` and `web` name one label, as do `Straße` and
//! `STRASSE`.
//!
//! A database written before this rule may hold names that differ only
//! in letter case. Those stay readable and assignable: a lookup prefers
//! the exact spelling, falls back to the one stored name that matches
//! ignoring case, and refuses to guess between two.

use kanban_dto::ApiError;

/// What a judged name belongs to, for the wording of a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedKind {
    Label,
    LabelGroup,
    Milestone,
}

impl NamedKind {
    fn noun(self) -> &'static str {
        match self {
            Self::Label => "label",
            Self::LabelGroup => "label group",
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

/// Resolve a requested name to the stored spelling it names: the exact
/// spelling when one is stored, otherwise the one stored name that
/// matches ignoring case, otherwise nothing. Two or more stored names
/// that match only ignoring case leave no single answer, so the lookup
/// is refused against `field` rather than guessed.
pub fn resolve_stored_name<'a>(
    kind: NamedKind,
    field: &str,
    stored: impl IntoIterator<Item = &'a str>,
    requested: &str,
) -> Result<Option<&'a str>, ApiError> {
    let mut variants = Vec::new();
    for name in stored {
        if name == requested {
            return Ok(Some(name));
        }
        if same_name(name, requested) {
            variants.push(name);
        }
    }
    match variants.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(only)),
        several => Err(ApiError::invalid_request_field(
            field,
            &format!(
                "{noun} `{requested}` matches {}, which differ only in letter case; \
                 name one of them exactly",
                quoted_list(several),
                noun = kind.noun(),
            ),
        )),
    }
}

/// `a`, `b` and `c`, quoted the way refusals quote names.
fn quoted_list(names: &[&str]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    match quoted.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => quoted.concat(),
    }
}

#[cfg(test)]
mod tests {
    use kanban_dto::ErrorCode;

    use super::{NamedKind, refuse_taken_name, resolve_stored_name, same_name};

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

    #[test]
    fn a_lookup_prefers_the_exact_spelling() {
        let stored = ["WEB", "web"];
        assert_eq!(
            resolve_stored_name(NamedKind::Label, "add_labels", stored, "web").unwrap(),
            Some("web")
        );
        assert_eq!(
            resolve_stored_name(NamedKind::Label, "add_labels", stored, "WEB").unwrap(),
            Some("WEB")
        );
    }

    #[test]
    fn a_lookup_finds_the_one_stored_spelling_ignoring_case() {
        assert_eq!(
            resolve_stored_name(NamedKind::Label, "add_labels", ["db", "web"], "Web").unwrap(),
            Some("web")
        );
    }

    #[test]
    fn a_lookup_with_no_match_finds_nothing() {
        assert_eq!(
            resolve_stored_name(NamedKind::Label, "add_labels", ["web"], "db").unwrap(),
            None
        );
    }

    #[test]
    fn a_lookup_refuses_to_guess_between_case_variants() {
        let error = resolve_stored_name(NamedKind::Label, "remove_labels", ["WEB", "web"], "Web")
            .expect_err("ambiguous");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.field.as_deref(), Some("remove_labels"));
        assert_eq!(
            error.message,
            "label `Web` matches `WEB` and `web`, which differ only in letter case; \
             name one of them exactly"
        );

        let three = resolve_stored_name(NamedKind::Label, "name", ["WEB", "Web", "web"], "wEB")
            .expect_err("ambiguous");
        assert!(
            three.message.contains("`WEB`, `Web` and `web`"),
            "{}",
            three.message
        );
    }
}

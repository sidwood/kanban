//! The session-selection arguments every Herdr command carries.

use kanban_domain::HerdrSession;

/// The `--session` arguments for one Herdr command or connector: a
/// named session passes `--session NAME`, and the default session
/// passes nothing at all, so Herdr selects its own default (DR-HB-20).
pub fn session_arguments(session: &HerdrSession) -> Vec<String> {
    match session.as_name() {
        Some(name) => vec!["--session".to_owned(), name.to_owned()],
        None => Vec::new(),
    }
}

#[cfg(test)]
mod session_arguments {
    use kanban_domain::HerdrSession;

    use super::session_arguments;

    #[test]
    fn a_named_session_passes_the_session_flag() {
        let session = HerdrSession::named("kanban-main").expect("the name validates");

        assert_eq!(
            session_arguments(&session),
            vec!["--session".to_owned(), "kanban-main".to_owned()]
        );
    }

    #[test]
    fn the_default_session_omits_the_session_flag() {
        assert!(
            session_arguments(&HerdrSession::Default).is_empty(),
            "an unnamed session must pass no `--session` argument"
        );
    }
}

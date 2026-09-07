//! Global search matching and ordering (DR-BP-17): one read-only
//! query over Initiatives, Projects, Plans, Specs, and Tickets by
//! identifier and text. The domain owns what matches and the order
//! hits return in; clients never re-rank.

/// The kind of entity one search hit names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SearchHitKind {
    Initiative,
    Project,
    Plan,
    Spec,
    Ticket,
}

/// One row the search query may return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub kind: SearchHitKind,
    pub id: u64,
    pub identifier: String,
    pub label: String,
    pub project_id: Option<u64>,
}

/// One searchable entity before the query is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchCandidate {
    pub kind: SearchHitKind,
    pub id: u64,
    pub identifier: String,
    pub label: String,
    pub project_id: Option<u64>,
    /// Extra text fields the operator might type: names, titles,
    /// slices, and the like.
    pub texts: Vec<String>,
}

/// Whether `candidate` matches a trimmed, case-insensitive `query`
/// against its identifier or any text field. A blank query matches
/// nothing.
pub fn matches(query: &str, candidate: &SearchCandidate) -> bool {
    let needle = query.trim();
    if needle.is_empty() {
        return false;
    }
    let lower = needle.to_lowercase();
    if candidate.identifier.to_lowercase().contains(&lower) {
        return true;
    }
    if candidate.label.to_lowercase().contains(&lower) {
        return true;
    }
    candidate
        .texts
        .iter()
        .any(|text| text.to_lowercase().contains(&lower))
}

/// Filter candidates into hits and sort them deterministically: kind
/// first, then label, then id.
pub fn search(query: &str, candidates: &[SearchCandidate]) -> Vec<SearchHit> {
    let mut hits = candidates
        .iter()
        .filter(|candidate| matches(query, candidate))
        .map(|candidate| SearchHit {
            kind: candidate.kind,
            id: candidate.id,
            identifier: candidate.identifier.clone(),
            label: candidate.label.clone(),
            project_id: candidate.project_id,
        })
        .collect::<Vec<_>>();
    sort_hits(&mut hits);
    hits
}

/// The deterministic order every client receives.
pub fn sort_hits(hits: &mut [SearchHit]) {
    hits.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.label.to_lowercase().cmp(&right.label.to_lowercase()))
            .then_with(|| left.id.cmp(&right.id))
    });
}

#[cfg(test)]
mod tests {
    use super::{SearchCandidate, SearchHit, SearchHitKind, matches, search, sort_hits};

    fn candidate(
        kind: SearchHitKind,
        id: u64,
        identifier: &str,
        label: &str,
        texts: &[&str],
    ) -> SearchCandidate {
        SearchCandidate {
            kind,
            id,
            identifier: identifier.to_owned(),
            label: label.to_owned(),
            project_id: None,
            texts: texts.iter().map(|text| (*text).to_owned()).collect(),
        }
    }

    #[test]
    fn blank_query_matches_nothing() {
        let row = candidate(
            SearchHitKind::Ticket,
            1,
            "CORE-T12",
            "Archive the register",
            &[],
        );
        assert!(!matches("", &row));
        assert!(!matches("   ", &row));
        assert!(search("  ", &[row]).is_empty());
    }

    #[test]
    fn identifier_match_is_case_insensitive() {
        let row = candidate(
            SearchHitKind::Ticket,
            1,
            "CORE-T12",
            "Archive the register",
            &[],
        );
        assert!(matches("core-t12", &row));
        assert!(matches("CORE-T", &row));
    }

    #[test]
    fn text_match_is_case_insensitive() {
        let row = candidate(
            SearchHitKind::Spec,
            2,
            "CORE-S4",
            "Board presentation",
            &["surface reference"],
        );
        assert!(matches("surface", &row));
        assert!(matches("presentation", &row));
        assert!(!matches("canvas", &row));
    }

    #[test]
    fn search_sorts_matches_deterministically() {
        let rows = [
            candidate(SearchHitKind::Ticket, 2, "CORE-T2", "Second ticket", &[]),
            candidate(
                SearchHitKind::Initiative,
                1,
                "Personal tooling",
                "Personal tooling",
                &[],
            ),
            candidate(
                SearchHitKind::Project,
                1,
                "CORE",
                "Control plane",
                &["CORE"],
            ),
            candidate(SearchHitKind::Plan, 1, "CORE-P1", "First plan", &[]),
            candidate(SearchHitKind::Spec, 1, "CORE-S1", "Board presentation", &[]),
            candidate(SearchHitKind::Ticket, 1, "CORE-T1", "First ticket", &[]),
        ];
        let hits = search("core", &rows);
        assert_eq!(
            hits.iter().map(|hit| hit.kind).collect::<Vec<_>>(),
            vec![
                SearchHitKind::Project,
                SearchHitKind::Plan,
                SearchHitKind::Spec,
                SearchHitKind::Ticket,
                SearchHitKind::Ticket,
            ]
        );
        assert_eq!(hits[4].identifier, "CORE-T2");
    }

    #[test]
    fn sort_hits_breaks_ties_by_label_then_id() {
        let mut hits = vec![
            SearchHit {
                kind: SearchHitKind::Ticket,
                id: 2,
                identifier: "CORE-T2".to_owned(),
                label: "Beta".to_owned(),
                project_id: Some(1),
            },
            SearchHit {
                kind: SearchHitKind::Ticket,
                id: 1,
                identifier: "CORE-T1".to_owned(),
                label: "Alpha".to_owned(),
                project_id: Some(1),
            },
            SearchHit {
                kind: SearchHitKind::Ticket,
                id: 3,
                identifier: "CORE-T3".to_owned(),
                label: "Alpha".to_owned(),
                project_id: Some(1),
            },
        ];
        sort_hits(&mut hits);
        assert_eq!(hits[0].id, 1);
        assert_eq!(hits[1].id, 3);
        assert_eq!(hits[2].id, 2);
    }
}

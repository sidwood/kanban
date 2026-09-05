//! Timeline query time-bound rules: RFC 3339 parsing, UTC
//! normalisation, and window ordering.

use std::fmt;

use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

/// The UTC shape SQLite stores for `recorded_at`: microsecond
/// precision with a `Z` suffix and no offset.
const STORED_FORMAT: &[time::format_description::FormatItem<'static>] = time::macros::format_description!(
    "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z"
);

/// Why a timeline time window was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineTimeError {
    /// One bound was not valid RFC 3339.
    MalformedBound { label: &'static str, value: String },
    /// `since` sorts after `until` once both are normalised.
    ReversedWindow { since: String, until: String },
}

impl fmt::Display for TimelineTimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedBound { label, value } => {
                write!(f, "timeline {label} bound is not valid RFC 3339: `{value}`")
            }
            Self::ReversedWindow { since, until } => {
                write!(
                    f,
                    "timeline since bound `{since}` must not be after until bound `{until}`"
                )
            }
        }
    }
}

/// Parse `raw` as RFC 3339 and normalise it to the stored UTC shape.
pub fn normalise_timeline_bound(
    label: &'static str,
    raw: &str,
) -> Result<String, TimelineTimeError> {
    let parsed =
        OffsetDateTime::parse(raw, &Rfc3339).map_err(|_| TimelineTimeError::MalformedBound {
            label,
            value: raw.to_owned(),
        })?;
    let utc = parsed.to_offset(UtcOffset::UTC);
    utc.format(STORED_FORMAT)
        .map_err(|_| TimelineTimeError::MalformedBound {
            label,
            value: raw.to_owned(),
        })
}

/// Normalise optional bounds and refuse a reversed window.
pub fn validate_timeline_time_window(
    since: Option<&str>,
    until: Option<&str>,
) -> Result<(Option<String>, Option<String>), TimelineTimeError> {
    let since = since
        .map(|value| normalise_timeline_bound("since", value))
        .transpose()?;
    let until = until
        .map(|value| normalise_timeline_bound("until", value))
        .transpose()?;
    if let (Some(since), Some(until)) = (&since, &until) {
        if since > until {
            return Err(TimelineTimeError::ReversedWindow {
                since: since.clone(),
                until: until.clone(),
            });
        }
    }
    Ok((since, until))
}

#[cfg(test)]
mod tests {
    use super::{TimelineTimeError, normalise_timeline_bound, validate_timeline_time_window};

    #[test]
    fn utc_bounds_normalise_to_the_stored_shape() {
        assert_eq!(
            normalise_timeline_bound("since", "2026-09-04T12:00:01Z").expect("parses"),
            "2026-09-04T12:00:01.000000Z"
        );
        assert_eq!(
            normalise_timeline_bound("until", "2026-03-01T00:00:00.000000Z").expect("parses"),
            "2026-03-01T00:00:00.000000Z"
        );
    }

    #[test]
    fn offset_bounds_normalise_to_utc_before_comparison() {
        assert_eq!(
            normalise_timeline_bound("since", "2026-09-04T13:00:00+01:00").expect("parses"),
            "2026-09-04T12:00:00.000000Z"
        );
        assert_eq!(
            normalise_timeline_bound("until", "2026-09-04T11:00:00-05:00").expect("parses"),
            "2026-09-04T16:00:00.000000Z"
        );
    }

    #[test]
    fn malformed_bounds_are_refused() {
        let error = normalise_timeline_bound("since", "not-a-timestamp").expect_err("malformed");
        assert_eq!(
            error,
            TimelineTimeError::MalformedBound {
                label: "since",
                value: "not-a-timestamp".to_owned(),
            }
        );
    }

    #[test]
    fn reversed_windows_are_refused_after_normalisation() {
        let error = validate_timeline_time_window(
            Some("2026-09-05T00:00:00Z"),
            Some("2026-09-04T00:00:00Z"),
        )
        .expect_err("reversed");

        assert_eq!(
            error,
            TimelineTimeError::ReversedWindow {
                since: "2026-09-05T00:00:00.000000Z".to_owned(),
                until: "2026-09-04T00:00:00.000000Z".to_owned(),
            }
        );
    }

    #[test]
    fn a_single_bound_passes_through() {
        let (since, until) =
            validate_timeline_time_window(Some("2026-09-04T12:00:00Z"), None).expect("valid");
        assert_eq!(since.as_deref(), Some("2026-09-04T12:00:00.000000Z"));
        assert!(until.is_none());
    }
}

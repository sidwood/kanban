//! Timeline query time-bound rules: RFC 3339 parsing, UTC
//! normalisation, and window ordering.

use std::fmt;

use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime, UtcOffset};

/// The UTC shape SQLite stores for `recorded_at`: `strftime('%f')`
/// writes exactly three fractional digits, with a `Z` suffix and no
/// offset. Bounds rendered to this shape compare with stored rows as
/// plain text.
const STORED_FORMAT: &[time::format_description::FormatItem<'static>] = time::macros::format_description!(
    "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
);

/// One stored millisecond, in nanoseconds.
const STORED_MILLISECOND_NANOS: u32 = 1_000_000;

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
///
/// Rendering truncates anything finer than the stored millisecond.
/// Window validation aligns `since` upward instead, so a bound this
/// function truncates is only the exact window end when it is an
/// `until`.
pub fn normalise_timeline_bound(
    label: &'static str,
    raw: &str,
) -> Result<String, TimelineTimeError> {
    let parsed = parse_bound(label, raw)?;
    render_stored(label, raw, parsed)
}

/// Parse `raw` as RFC 3339 and move it to UTC.
fn parse_bound(label: &'static str, raw: &str) -> Result<OffsetDateTime, TimelineTimeError> {
    OffsetDateTime::parse(raw, &Rfc3339)
        .map(|parsed| parsed.to_offset(UtcOffset::UTC))
        .map_err(|_| TimelineTimeError::MalformedBound {
            label,
            value: raw.to_owned(),
        })
}

/// Render `instant` in the stored shape. Truncation is exact for
/// `until`: storage only records whole milliseconds, and a whole
/// millisecond never sorts after an instant it falls inside.
fn render_stored(
    label: &'static str,
    raw: &str,
    instant: OffsetDateTime,
) -> Result<String, TimelineTimeError> {
    instant
        .format(STORED_FORMAT)
        .map_err(|_| TimelineTimeError::MalformedBound {
            label,
            value: raw.to_owned(),
        })
}

/// Align a `since` instant up to the next whole stored millisecond:
/// storage only records whole milliseconds, so a finer bound must not
/// admit the millisecond it falls inside. A bound inside the last
/// representable millisecond cannot move up and is refused instead.
fn align_since_up(
    label: &'static str,
    raw: &str,
    instant: OffsetDateTime,
) -> Result<String, TimelineTimeError> {
    let beyond_millisecond = instant.nanosecond() % STORED_MILLISECOND_NANOS;
    let aligned = if beyond_millisecond == 0 {
        instant
    } else {
        instant
            .checked_add(Duration::nanoseconds(i64::from(
                STORED_MILLISECOND_NANOS - beyond_millisecond,
            )))
            .ok_or_else(|| TimelineTimeError::MalformedBound {
                label,
                value: raw.to_owned(),
            })?
    };
    render_stored(label, raw, aligned)
}

/// Normalise optional bounds and refuse a reversed window.
///
/// Ordering is judged on the parsed instants, before `since` is
/// aligned up to the stored millisecond. A window that falls entirely
/// inside one stored millisecond therefore validates, and normalises
/// to a pair that matches no stored row instead of drawing a false
/// refusal.
pub fn validate_timeline_time_window(
    since: Option<&str>,
    until: Option<&str>,
) -> Result<(Option<String>, Option<String>), TimelineTimeError> {
    let since_utc = since
        .map(|raw| parse_bound("since", raw).map(|instant| (raw, instant)))
        .transpose()?;
    let until_utc = until
        .map(|raw| parse_bound("until", raw).map(|instant| (raw, instant)))
        .transpose()?;
    if let (Some((since_raw, since_instant)), Some((until_raw, until_instant))) =
        (&since_utc, &until_utc)
    {
        if since_instant > until_instant {
            return Err(TimelineTimeError::ReversedWindow {
                since: render_stored("since", since_raw, *since_instant)?,
                until: render_stored("until", until_raw, *until_instant)?,
            });
        }
    }
    let since = since_utc
        .map(|(raw, instant)| align_since_up("since", raw, instant))
        .transpose()?;
    let until = until_utc
        .map(|(raw, instant)| render_stored("until", raw, instant))
        .transpose()?;
    Ok((since, until))
}

#[cfg(test)]
mod tests {
    use super::{TimelineTimeError, normalise_timeline_bound, validate_timeline_time_window};

    #[test]
    fn utc_bounds_normalise_to_the_stored_shape() {
        assert_eq!(
            normalise_timeline_bound("since", "2026-09-04T12:00:01Z").expect("parses"),
            "2026-09-04T12:00:01.000Z"
        );
        assert_eq!(
            normalise_timeline_bound("until", "2026-03-01T00:00:00.000Z").expect("parses"),
            "2026-03-01T00:00:00.000Z"
        );
    }

    #[test]
    fn offset_bounds_normalise_to_utc_before_comparison() {
        assert_eq!(
            normalise_timeline_bound("since", "2026-09-04T13:00:00+01:00").expect("parses"),
            "2026-09-04T12:00:00.000Z"
        );
        assert_eq!(
            normalise_timeline_bound("until", "2026-09-04T11:00:00-05:00").expect("parses"),
            "2026-09-04T16:00:00.000Z"
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
                since: "2026-09-05T00:00:00.000Z".to_owned(),
                until: "2026-09-04T00:00:00.000Z".to_owned(),
            }
        );
    }

    #[test]
    fn a_single_bound_passes_through() {
        let (since, until) =
            validate_timeline_time_window(Some("2026-09-04T12:00:00Z"), None).expect("valid");
        assert_eq!(since.as_deref(), Some("2026-09-04T12:00:00.000Z"));
        assert!(until.is_none());
    }

    #[test]
    fn normalised_bounds_match_the_stored_millisecond_shape() {
        assert_eq!(
            normalise_timeline_bound("since", "2026-09-04T12:00:01Z").expect("parses"),
            "2026-09-04T12:00:01.000Z",
            "SQLite stores three fractional digits via strftime %f"
        );
        assert_eq!(
            normalise_timeline_bound("until", "2026-03-01T00:00:00.123Z").expect("parses"),
            "2026-03-01T00:00:00.123Z",
            "a stored millisecond must normalise to the identical text"
        );
    }

    #[test]
    fn a_whole_millisecond_since_bound_keeps_its_instant() {
        let (since, _) = validate_timeline_time_window(Some("2026-03-01T00:00:00.123Z"), None)
            .expect("whole milliseconds are valid bounds");
        assert_eq!(since.as_deref(), Some("2026-03-01T00:00:00.123Z"));
    }

    #[test]
    fn sub_millisecond_since_rounds_up_to_the_stored_millisecond() {
        let (since, _) = validate_timeline_time_window(Some("2026-03-01T00:00:00.1239Z"), None)
            .expect("sub-millisecond bounds are valid RFC 3339");
        assert_eq!(
            since.as_deref(),
            Some("2026-03-01T00:00:00.124Z"),
            "a since finer than the stored millisecond must not admit the earlier millisecond"
        );
    }

    #[test]
    fn sub_millisecond_until_truncates_to_the_stored_millisecond() {
        let (_, until) = validate_timeline_time_window(None, Some("2026-03-01T00:00:00.1239Z"))
            .expect("sub-millisecond bounds are valid RFC 3339");
        assert_eq!(
            until.as_deref(),
            Some("2026-03-01T00:00:00.123Z"),
            "an until finer than the stored millisecond still holds its own millisecond"
        );
    }

    #[test]
    fn a_window_inside_one_stored_millisecond_matches_nothing() {
        let (since, until) = validate_timeline_time_window(
            Some("2026-03-01T00:00:00.1235Z"),
            Some("2026-03-01T00:00:00.1239Z"),
        )
        .expect("a window that holds no stored instant is still a valid window");

        assert_eq!(since.as_deref(), Some("2026-03-01T00:00:00.124Z"));
        assert_eq!(until.as_deref(), Some("2026-03-01T00:00:00.123Z"));
    }
}

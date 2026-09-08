//! Pure calendar calculations shared by recurring execution and its preview.

use chrono::{DateTime, SecondsFormat, Utc};
use chrono_tz::Tz;
use croner::Cron;

use crate::schedule::{CronExpression, ScheduleError};

/// Compute the first UTC activation strictly after `after`, without reading a clock.
pub fn next_activation(
    expression: &str,
    timezone: &str,
    after: &str,
) -> Result<String, ScheduleError> {
    let expression = CronExpression::new(expression)?;
    let cron: Cron = expression
        .as_str()
        .parse()
        .map_err(|_| ScheduleError::InvalidCron {
            value: expression.as_str().to_owned(),
        })?;
    let timezone: Tz = timezone
        .parse()
        .map_err(|_| ScheduleError::InvalidTimezone {
            value: timezone.to_owned(),
        })?;
    let after = DateTime::parse_from_rfc3339(after)
        .map_err(|_| ScheduleError::MalformedInstant {
            field: "after",
            value: after.to_owned(),
        })?
        .with_timezone(&timezone);
    let next =
        cron.find_next_occurrence(&after, false)
            .map_err(|_| ScheduleError::InvalidCron {
                value: expression.as_str().to_owned(),
            })?;
    Ok(next
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// Missed windows are skipped unless a Project opts into exactly one catch-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CatchUpPolicy {
    #[default]
    Skip,
    One,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkippedWindowReason {
    MissedWindow,
    Overlap,
    Blocked,
}
impl SkippedWindowReason {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::MissedWindow => "missed_window",
            Self::Overlap => "overlap",
            Self::Blocked => "blocked",
        }
    }
}

/// The bounded span of history a decision did not execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedWindows {
    pub from: String,
    pub through: String,
    pub reason: SkippedWindowReason,
}

/// One atomic advance of a recurring Schedule; never an unbounded replay queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceAdvance {
    pub occurrence_window: Option<String>,
    pub skipped: Option<SkippedWindows>,
    pub next_activation: String,
}

pub fn advance_recurring(
    schedule: &crate::Schedule,
    now: &str,
    has_open_occurrence: bool,
    catch_up: CatchUpPolicy,
) -> Result<Option<RecurrenceAdvance>, ScheduleError> {
    let crate::ScheduleTrigger::Recurring { expression } = schedule.trigger() else {
        return Ok(None);
    };
    if instant("now", now)? < instant("next activation", schedule.next_activation())? {
        return Ok(None);
    }
    let latest = previous_activation(expression.as_str(), schedule.timezone().as_str(), now, true)?;
    let current = instant("now", now)?;
    let is_current_minute = latest.timestamp().div_euclid(60) == current.timestamp().div_euclid(60);
    let would_mint = is_current_minute || catch_up == CatchUpPolicy::One;
    let mint = would_mint && !has_open_occurrence;
    let latest_text = latest.to_rfc3339_opts(SecondsFormat::Millis, true);
    let skipped_through = if mint {
        previous_activation(
            expression.as_str(),
            schedule.timezone().as_str(),
            &latest_text,
            false,
        )?
    } else {
        latest
    };
    Ok(Some(RecurrenceAdvance {
        occurrence_window: mint.then_some(latest_text),
        skipped: (skipped_through >= instant("next activation", schedule.next_activation())?).then(
            || SkippedWindows {
                from: schedule.next_activation().to_owned(),
                through: skipped_through.to_rfc3339_opts(SecondsFormat::Millis, true),
                reason: if would_mint && has_open_occurrence {
                    SkippedWindowReason::Overlap
                } else {
                    SkippedWindowReason::MissedWindow
                },
            },
        ),
        next_activation: next_activation(expression.as_str(), schedule.timezone().as_str(), now)?,
    }))
}

fn previous_activation(
    expression: &str,
    timezone: &str,
    before: &str,
    inclusive: bool,
) -> Result<DateTime<Utc>, ScheduleError> {
    let cron: Cron = expression.parse().map_err(|_| ScheduleError::InvalidCron {
        value: expression.to_owned(),
    })?;
    let timezone: Tz = timezone
        .parse()
        .map_err(|_| ScheduleError::InvalidTimezone {
            value: timezone.to_owned(),
        })?;
    cron.find_previous_occurrence(
        &instant("before", before)?.with_timezone(&timezone),
        inclusive,
    )
    .map(|value| value.with_timezone(&Utc))
    .map_err(|_| ScheduleError::InvalidCron {
        value: expression.to_owned(),
    })
}

fn instant(field: &'static str, text: &str) -> Result<DateTime<Utc>, ScheduleError> {
    DateTime::parse_from_rfc3339(text)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| ScheduleError::MalformedInstant {
            field,
            value: text.to_owned(),
        })
}

/// Done or terminal occurrences no longer reserve their recurring window.
pub fn occurrence_is_open(state: crate::TicketState) -> bool {
    !matches!(
        state,
        crate::TicketState::Done | crate::TicketState::Cancelled | crate::TicketState::Superseded
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecurringDispatchRefusal {
    Template,
    OpenOccurrence(crate::TicketId),
}
impl std::fmt::Display for RecurringDispatchRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Template => {
                f.write_str("a recurring template cannot execute; dispatch a fresh occurrence")
            }
            Self::OpenOccurrence(id) => write!(
                f,
                "recurring occurrence {id} remains open; overlapping dispatch is refused"
            ),
        }
    }
}
impl std::error::Error for RecurringDispatchRefusal {}

pub fn guard_recurring_dispatch(
    is_template: bool,
    peers: &[(crate::TicketId, crate::TicketState)],
) -> Result<(), RecurringDispatchRefusal> {
    if is_template {
        return Err(RecurringDispatchRefusal::Template);
    }
    if let Some((id, _)) = peers.iter().find(|(_, state)| occurrence_is_open(*state)) {
        return Err(RecurringDispatchRefusal::OpenOccurrence(*id));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Schedule, TicketId};

    fn template() -> Schedule {
        Schedule::recurring(
            TicketId::new(7),
            "*/15 * * * *",
            "UTC",
            "standard",
            "2026-09-08T09:15:00Z",
        )
        .unwrap()
    }

    #[test]
    fn on_time_window_mints_one_occurrence_and_advances() {
        let decision = advance_recurring(
            &template(),
            "2026-09-08T09:15:20Z",
            false,
            CatchUpPolicy::Skip,
        )
        .unwrap()
        .expect("the current window is due");
        assert_eq!(
            decision.occurrence_window.as_deref(),
            Some("2026-09-08T09:15:00.000Z")
        );
        assert_eq!(decision.next_activation, "2026-09-08T09:30:00.000Z");
        assert_eq!(decision.skipped, None);
    }

    #[test]
    fn skipped_history_advances_even_while_an_occurrence_is_open() {
        for open in [false, true] {
            let decision = advance_recurring(
                &template(),
                "2026-09-08T09:46:00Z",
                open,
                CatchUpPolicy::Skip,
            )
            .unwrap()
            .unwrap();
            assert_eq!(decision.occurrence_window, None);
            assert_eq!(decision.next_activation, "2026-09-08T10:00:00.000Z");
            assert_eq!(
                decision.skipped,
                Some(SkippedWindows {
                    from: "2026-09-08T09:15:00.000Z".to_owned(),
                    through: "2026-09-08T09:45:00.000Z".to_owned(),
                    reason: SkippedWindowReason::MissedWindow,
                })
            );
        }
    }

    #[test]
    fn catch_up_mints_only_the_latest_missed_window() {
        let decision = advance_recurring(
            &template(),
            "2026-09-08T12:07:00Z",
            false,
            CatchUpPolicy::One,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            decision.occurrence_window.as_deref(),
            Some("2026-09-08T12:00:00.000Z")
        );
        assert_eq!(decision.next_activation, "2026-09-08T12:15:00.000Z");
        assert_eq!(
            decision.skipped,
            Some(SkippedWindows {
                from: "2026-09-08T09:15:00.000Z".to_owned(),
                through: "2026-09-08T11:45:00.000Z".to_owned(),
                reason: SkippedWindowReason::MissedWindow,
            })
        );
    }

    #[test]
    fn overlap_prevents_both_current_and_catch_up_occurrences() {
        for (policy, now) in [
            (CatchUpPolicy::Skip, "2026-09-08T09:15:20Z"),
            (CatchUpPolicy::One, "2026-09-08T09:15:20Z"),
            (CatchUpPolicy::One, "2026-09-08T09:16:20Z"),
        ] {
            let decision = advance_recurring(&template(), now, true, policy)
                .unwrap()
                .unwrap();
            assert_eq!(decision.occurrence_window, None);
            assert_eq!(decision.next_activation, "2026-09-08T09:30:00.000Z");
            assert_eq!(
                decision.skipped,
                Some(SkippedWindows {
                    from: "2026-09-08T09:15:00.000Z".to_owned(),
                    through: "2026-09-08T09:15:00.000Z".to_owned(),
                    reason: SkippedWindowReason::Overlap,
                })
            );
        }
    }

    #[test]
    fn future_window_is_not_due_even_when_now_uses_an_offset() {
        for now in ["2026-09-08T09:14:59Z", "2026-09-08T10:14:59+01:00"] {
            assert_eq!(
                advance_recurring(&template(), now, false, CatchUpPolicy::Skip).unwrap(),
                None
            );
        }
    }
}

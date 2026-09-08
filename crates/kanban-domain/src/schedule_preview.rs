//! Calendar preview values, calculated without a clock or database.
use crate::schedule::{CronExpression, ScheduleError};
use chrono::{DateTime, SecondsFormat, Utc};
use chrono_tz::Tz;
use croner::{Cron, JobType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DstKind {
    FixedInstant,
    FixedTime,
    IntervalWildcard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewActivation {
    pub utc: String,
    pub local: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulePreview {
    pub activations: Vec<PreviewActivation>,
    pub kind: DstKind,
    pub spring_forward: &'static str,
    pub fall_back: &'static str,
}

pub fn preview_recurring(
    expression: &str,
    timezone: &str,
    after: &str,
    count: u8,
) -> Result<SchedulePreview, ScheduleError> {
    let expression = CronExpression::new(expression)?;
    let cron: Cron = expression
        .as_str()
        .parse()
        .map_err(|_| ScheduleError::InvalidCron {
            value: expression.as_str().to_owned(),
        })?;
    let zone: Tz = timezone
        .parse()
        .map_err(|_| ScheduleError::InvalidTimezone {
            value: timezone.to_owned(),
        })?;
    let (kind, spring_forward, fall_back) = match cron.determine_job_type() {
        JobType::FixedTime => (
            DstKind::FixedTime,
            "A fixed clock time in a DST gap runs at the first valid time after the jump.",
            "A fixed clock time in a repeated hour runs once, at its first occurrence.",
        ),
        JobType::IntervalWildcard => (
            DstKind::IntervalWildcard,
            "Intervals use real instants; clock times missing in a DST gap do not run.",
            "Intervals can run in both copies of a repeated hour, at distinct UTC instants.",
        ),
    };
    let mut cursor = after.to_owned();
    let mut activations = Vec::new();
    for _ in 0..count {
        let utc = crate::recurrence::next_activation(expression.as_str(), timezone, &cursor)?;
        let instant =
            DateTime::parse_from_rfc3339(&utc).map_err(|_| ScheduleError::MalformedInstant {
                field: "activation",
                value: utc.clone(),
            })?;
        activations.push(PreviewActivation {
            utc: instant
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true),
            local: instant
                .with_timezone(&zone)
                .to_rfc3339_opts(SecondsFormat::Millis, false),
        });
        cursor = utc;
    }
    Ok(SchedulePreview {
        activations,
        kind,
        spring_forward,
        fall_back,
    })
}

pub fn preview_one_time(
    activation: &str,
    timezone: &str,
) -> Result<SchedulePreview, ScheduleError> {
    let zone: Tz = timezone
        .parse()
        .map_err(|_| ScheduleError::InvalidTimezone {
            value: timezone.to_owned(),
        })?;
    let instant =
        DateTime::parse_from_rfc3339(activation).map_err(|_| ScheduleError::MalformedInstant {
            field: "activation",
            value: activation.to_owned(),
        })?;
    Ok(SchedulePreview {
        activations: vec![PreviewActivation {
            utc: instant
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true),
            local: instant
                .with_timezone(&zone)
                .to_rfc3339_opts(SecondsFormat::Millis, false),
        }],
        kind: DstKind::FixedInstant,
        spring_forward: "A one-time activation is a fixed instant; DST does not move it.",
        fall_back: "An explicit UTC offset selects one instant, even in a repeated hour.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recurrence::{CatchUpPolicy, advance_recurring};
    use crate::{Schedule, TicketId};

    #[test]
    fn next_activation_dst_preview_matches_firing_on_both_clock_changes() {
        for (cron, after, expected_utc, expected_local) in [
            (
                "30 1 * * *",
                "2026-03-28T23:00:00Z",
                "2026-03-29T01:00:00.000Z",
                "2026-03-29T02:00:00.000+01:00",
            ),
            (
                "30 1 * * *",
                "2026-10-25T00:00:00Z",
                "2026-10-25T00:30:00.000Z",
                "2026-10-25T01:30:00.000+01:00",
            ),
            (
                "*/30 * * * *",
                "2026-03-29T00:30:00Z",
                "2026-03-29T01:00:00.000Z",
                "2026-03-29T02:00:00.000+01:00",
            ),
            (
                "*/30 * * * *",
                "2026-10-25T00:30:00Z",
                "2026-10-25T01:00:00.000Z",
                "2026-10-25T01:00:00.000+00:00",
            ),
        ] {
            let preview = preview_recurring(cron, "Europe/London", after, 2).unwrap();
            assert_eq!(
                preview.activations[0].utc, expected_utc,
                "{cron} after {after}"
            );
            assert_eq!(
                preview.activations[0].local, expected_local,
                "{cron} after {after}"
            );
            let schedule = Schedule::recurring(
                TicketId::new(1),
                cron,
                "Europe/London",
                "standard",
                expected_utc,
            )
            .unwrap();
            let fired = advance_recurring(&schedule, expected_utc, false, CatchUpPolicy::Skip)
                .unwrap()
                .unwrap();
            assert_eq!(
                fired.occurrence_window.as_deref(),
                Some(expected_utc),
                "preview and firing disagree for {cron} after {after}"
            );
            assert_eq!(fired.next_activation, preview.activations[1].utc);
        }
    }
}

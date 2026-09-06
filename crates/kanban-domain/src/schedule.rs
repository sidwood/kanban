//! The Schedule rules (CONTEXT.md, DR-SA-01 to DR-SA-06): a Schedule
//! holds one-time activation or cron, the timezone it lives in, the
//! Execution Profile eligible once it fires, and its next activation
//! (DR-SA-01). Scheduled means qualified but unavailable until
//! activation (DR-SA-02); a one-time activation makes the existing
//! Ticket it holds ready through the lifecycle's own gates (DR-SA-03),
//! and Implementation and Bug Tickets accept one-time schedules only
//! (DR-SA-05). Recurring occurrence minting, missed-run policy, and
//! DST visibility are KAN-T54's and KAN-T55's; this module owns the
//! carried shape and the one-time firing rule the core service's
//! scheduler drives — including the overdue pass after a restart
//! (DR-SA-06). Time arrives as values: the domain owns no clock, and
//! `now` reaches these rules as a stored-shape instant.

use std::fmt;

use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use crate::dependency::Readiness;
use crate::lifecycle::{Actor, LifecycleError, apply_drag};
use crate::profile::{ProfileError, ProfileName};
use crate::ticket::{Ticket, TicketId, TicketKind, TicketState};
use crate::timeline_time::stored_format;

/// The identity of one Schedule. Assigned once by storage and
/// immutable afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScheduleId(u64);

impl ScheduleId {
    /// Wrap a storage-assigned identity.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying identity value.
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ScheduleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The closed Schedule state vocabulary: a Schedule waits for its
/// activation, and a one-time Schedule that fired is spent — it never
/// fires again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScheduleState {
    /// Qualified work held until its activation.
    Waiting,
    /// The activation fired; a one-time Schedule holds nothing more.
    Fired,
}

impl ScheduleState {
    /// Every state, in vocabulary order.
    pub const ALL: &'static [Self] = &[Self::Waiting, Self::Fired];

    /// The stored and wire name of this state.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Fired => "fired",
        }
    }

    /// The state a stored row names, or `None` outside the closed set.
    pub fn parse(stored: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|state| state.wire_name() == stored)
    }
}

/// The IANA zone name a Schedule lives in (DR-SA-01): `UTC`,
/// `Europe/Amsterdam`, `America/Argentina/Buenos_Aires`. The domain
/// validates the zone's name shape; resolving it to offsets and DST
/// rules is the visibility slice KAN-T55 owns, so no offset lookup
/// happens here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Timezone(String);

impl Timezone {
    /// Accept an IANA zone name by its shape: non-empty, slash
    /// separated with no empty segment, and every segment starting
    /// with a capital letter and holding only letters, digits, `-`,
    /// `_`, `.`, or `+` (as `Etc/GMT+14` does).
    pub fn new(raw: &str) -> Result<Self, ScheduleError> {
        let trimmed = raw.trim();
        let well_formed = !trimmed.is_empty()
            && trimmed.split('/').all(|segment| {
                !segment.is_empty()
                    && segment.len() <= 64
                    && segment
                        .chars()
                        .next()
                        .is_some_and(|first| first.is_ascii_uppercase())
                    && segment.chars().all(|character| {
                        character.is_ascii_alphanumeric()
                            || matches!(character, '-' | '_' | '.' | '+')
                    })
            });
        if !well_formed {
            return Err(ScheduleError::InvalidTimezone {
                value: trimmed.to_owned(),
            });
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The zone name as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Timezone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The five-field cron expression a recurring Schedule carries
/// (DR-SA-01): minute, hour, day of month, month, day of week. This
/// slice validates the expression's shape alone; evaluating it — the
/// next-activation computation and the occurrence minting — is
/// KAN-T54's and KAN-T55's.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CronExpression(String);

impl CronExpression {
    /// Accept exactly five non-empty fields of cron's character
    /// vocabulary, normalised to single spaces.
    pub fn new(raw: &str) -> Result<Self, ScheduleError> {
        let fields: Vec<&str> = raw.split_whitespace().collect();
        let well_formed = fields.len() == 5
            && fields.iter().all(|field| {
                !field.is_empty()
                    && field.chars().all(|character| {
                        character.is_ascii_alphanumeric()
                            || matches!(character, '*' | '/' | ',' | '-')
                    })
            });
        if !well_formed {
            return Err(ScheduleError::InvalidCron {
                value: raw.to_owned(),
            });
        }
        Ok(Self(fields.join(" ")))
    }

    /// The expression as stored, single-spaced.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CronExpression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What a Schedule waits on (DR-SA-01): one absolute activation
/// instant, or a cron expression whose occurrences KAN-T54 mints.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ScheduleTrigger {
    /// Fire once at the stored UTC activation instant.
    OneTime {
        /// The activation instant, RFC 3339 in the stored UTC shape.
        activation: String,
    },
    /// Fire at each occurrence the expression names.
    Recurring {
        /// The five-field cron expression.
        expression: CronExpression,
    },
}

/// What one fired one-time activation did to its Ticket (DR-SA-03).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    /// The Ticket moved to ready.
    BecameReady,
    /// The Ticket already circulated — ready or beyond — so the
    /// activation spent the Schedule without moving anything.
    AlreadyCirculating,
}

/// One Schedule: the existing Ticket it holds, its trigger, its
/// timezone, its eligible Execution Profile, and its next activation
/// (DR-SA-01). For a one-time Schedule the next activation is the
/// activation itself; for a recurring one it is the computed instant
/// the caller supplies, because the computation is KAN-T55's pure
/// function, not the constructor's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    id: Option<ScheduleId>,
    ticket: TicketId,
    trigger: ScheduleTrigger,
    timezone: Timezone,
    profile: ProfileName,
    next_activation: String,
    state: ScheduleState,
}

impl Schedule {
    /// Assemble one waiting one-time Schedule, normalising the
    /// activation instant to the stored UTC shape. Its next activation
    /// is its activation (DR-SA-01).
    pub fn one_time(
        ticket: TicketId,
        activation: impl Into<String>,
        timezone: &str,
        profile: &str,
    ) -> Result<Self, ScheduleError> {
        let activation = instant("activation", activation.into())?;
        let timezone = Timezone::new(timezone)?;
        let profile = profile_named(profile)?;
        Ok(Self {
            id: None,
            ticket,
            trigger: ScheduleTrigger::OneTime {
                activation: activation.clone(),
            },
            timezone,
            profile,
            next_activation: activation,
            state: ScheduleState::Waiting,
        })
    }

    /// Assemble one waiting recurring Schedule around a five-field
    /// cron expression, carrying the next activation the caller
    /// computed for it. Occurrence minting is KAN-T54's; the shape is
    /// rule-valid now.
    pub fn recurring(
        ticket: TicketId,
        expression: &str,
        timezone: &str,
        profile: &str,
        next_activation: impl Into<String>,
    ) -> Result<Self, ScheduleError> {
        let expression = CronExpression::new(expression)?;
        let timezone = Timezone::new(timezone)?;
        let profile = profile_named(profile)?;
        Ok(Self {
            id: None,
            ticket,
            trigger: ScheduleTrigger::Recurring { expression },
            timezone,
            profile,
            next_activation: instant("next activation", next_activation.into())?,
            state: ScheduleState::Waiting,
        })
    }

    /// Rehydrate a stored Schedule exactly as it was recorded.
    pub fn restore(
        id: ScheduleId,
        ticket: TicketId,
        trigger: ScheduleTrigger,
        timezone: Timezone,
        profile: ProfileName,
        next_activation: String,
        state: ScheduleState,
    ) -> Self {
        Self {
            id: Some(id),
            ticket,
            trigger,
            timezone,
            profile,
            next_activation,
            state,
        }
    }

    /// The storage-assigned identity, absent until storage lands the
    /// row.
    pub fn id(&self) -> Option<ScheduleId> {
        self.id
    }

    /// The existing Ticket this Schedule holds (DR-SA-03).
    pub fn ticket(&self) -> TicketId {
        self.ticket
    }

    /// What the Schedule waits on.
    pub fn trigger(&self) -> &ScheduleTrigger {
        &self.trigger
    }

    /// The timezone the Schedule lives in.
    pub fn timezone(&self) -> &Timezone {
        &self.timezone
    }

    /// The Execution Profile eligible once the Schedule fires.
    pub fn profile(&self) -> &ProfileName {
        &self.profile
    }

    /// The next activation, RFC 3339 in the stored UTC shape.
    pub fn next_activation(&self) -> &str {
        &self.next_activation
    }

    /// The Schedule's state.
    pub fn state(&self) -> ScheduleState {
        self.state
    }

    /// This Schedule with its activation spent: a fired one-time
    /// Schedule is never due again.
    pub fn fired(&self) -> Self {
        let mut spent = self.clone();
        spent.state = ScheduleState::Fired;
        spent
    }

    /// Whether the Schedule's activation has come due at `now`
    /// (DR-SA-02, DR-SA-06): a waiting Schedule is due from its
    /// activation instant onward — an overdue Schedule stays due until
    /// it fires — and a fired one is due never again.
    pub fn is_due(&self, now: &str) -> bool {
        self.state == ScheduleState::Waiting
            && instant_of(&self.next_activation)
                .zip(instant_of(now))
                .is_some_and(|(activation, now)| activation <= now)
    }

    /// Fire the one-time activation: make the Ticket it holds ready
    /// through the lifecycle's own transition table and gates
    /// (DR-SA-03), the way the core service's scheduler does
    /// (DR-SA-06). Scheduled work becomes ready; work that already
    /// circulated spends the Schedule without moving; work a readiness
    /// still holds back refuses and waits for a later tick; recurring
    /// Schedules mint occurrences in KAN-T54, never here.
    pub fn activate(
        &self,
        ticket: &mut Ticket,
        readiness: &Readiness,
    ) -> Result<Activation, ScheduleError> {
        if !matches!(self.trigger, ScheduleTrigger::OneTime { .. }) {
            return Err(ScheduleError::NotOneTime);
        }
        if ticket.state().is_terminal() {
            return Err(ScheduleError::TerminalTicket);
        }
        match ticket.state() {
            TicketState::Ready
            | TicketState::Active
            | TicketState::InReview
            | TicketState::Approved
            | TicketState::Landing
            | TicketState::Done => Ok(Activation::AlreadyCirculating),
            _ => apply_drag(ticket, TicketState::Ready, Actor::Agent, readiness)
                .map(|()| Activation::BecameReady)
                .map_err(|cause| ScheduleError::Activation { cause }),
        }
    }
}

/// Whether `kind` of Ticket accepts `trigger` (DR-SA-05): every kind
/// accepts one-time schedules; only a Task carries a recurring one,
/// because recurring activations mint fresh Task occurrences.
pub fn accepts(kind: TicketKind, trigger: &ScheduleTrigger) -> Result<(), ScheduleError> {
    if matches!(
        (kind, trigger),
        (
            TicketKind::Implementation | TicketKind::Bug,
            ScheduleTrigger::Recurring { .. }
        )
    ) {
        return Err(ScheduleError::RecurringNotSupported { kind });
    }
    Ok(())
}

/// Why a Schedule was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    /// A trigger instant was not valid RFC 3339. The value names the
    /// field and the raw text refused.
    MalformedInstant {
        /// The field the refused instant belongs to.
        field: &'static str,
        /// The raw value that named no instant.
        value: String,
    },
    /// The timezone named no IANA zone.
    InvalidTimezone {
        /// The raw name that named no zone.
        value: String,
    },
    /// The cron expression stated no five fields.
    InvalidCron {
        /// The raw expression refused.
        value: String,
    },
    /// The eligible profile named no catalogue name.
    InvalidProfile {
        /// The profile refusal.
        source: ProfileError,
    },
    /// A recurring Schedule serves Task Tickets alone (DR-SA-05).
    RecurringNotSupported {
        /// The kind that accepts one-time schedules only.
        kind: TicketKind,
    },
    /// A recurring Schedule was asked for one-time activation;
    /// occurrence minting is KAN-T54's.
    NotOneTime,
    /// A terminal Ticket accepts no activation.
    TerminalTicket,
    /// The lifecycle refused the activation the Schedule drove. The
    /// value carries its refusal.
    Activation {
        /// Why the lifecycle refused the move.
        cause: LifecycleError,
    },
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedInstant { field, value } => {
                write!(
                    f,
                    "a Schedule {field} must be an RFC 3339 instant: `{value}`"
                )
            }
            Self::InvalidTimezone { .. } => {
                write!(
                    f,
                    "a Schedule timezone names an IANA zone, like `Europe/Amsterdam`"
                )
            }
            Self::InvalidCron { value } => {
                write!(f, "a cron expression states five fields: `{value}`")
            }
            Self::InvalidProfile { source } => write!(f, "{source}"),
            Self::RecurringNotSupported { kind } => write!(
                f,
                "{} {} Ticket accepts one-time schedules only",
                article(kind.wire_name()),
                kind.wire_name()
            ),
            Self::NotOneTime => write!(
                f,
                "a recurring Schedule mints fresh Task occurrences, which KAN-T54 lands"
            ),
            Self::TerminalTicket => write!(f, "a terminal Ticket accepts no activation"),
            Self::Activation { cause } => write!(f, "{cause}"),
        }
    }
}

impl std::error::Error for ScheduleError {}

/// The article a kind's wire name takes.
fn article(wire_name: &str) -> &str {
    if wire_name.starts_with('i') {
        "an"
    } else {
        "a"
    }
}

/// Parse `raw` as RFC 3339 and render it in the stored UTC shape,
/// naming the field a refusal reports.
fn instant(field: &'static str, raw: String) -> Result<String, ScheduleError> {
    let parsed = OffsetDateTime::parse(&raw, &Rfc3339)
        .map(|parsed| parsed.to_offset(UtcOffset::UTC))
        .map_err(|_| ScheduleError::MalformedInstant {
            field,
            value: raw.clone(),
        })?;
    parsed
        .format(stored_format())
        .map_err(|_| ScheduleError::MalformedInstant { field, value: raw })
}

/// The instant a stored-shape text names.
fn instant_of(stored: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(stored, &Rfc3339)
        .ok()
        .map(|parsed| parsed.to_offset(UtcOffset::UTC))
}

/// The eligible profile a Schedule names, as a trimmed catalogue name.
fn profile_named(raw: &str) -> Result<ProfileName, ScheduleError> {
    ProfileName::new(raw).map_err(|source| ScheduleError::InvalidProfile { source })
}

/// Render `unix_nanos` in the stored UTC shape: the form every
/// schedule instant and every scheduler tick compare through. The
/// scheduler owns the clock and hands its reading here as a value.
pub fn stored_instant_of(unix_nanos: i128) -> Option<String> {
    OffsetDateTime::from_unix_timestamp_nanos(unix_nanos)
        .ok()?
        .to_offset(UtcOffset::UTC)
        .format(stored_format())
        .ok()
}

#[cfg(test)]
mod schedule_rules {
    use super::{
        Activation, CronExpression, Schedule, ScheduleError, ScheduleState, ScheduleTrigger,
        Timezone, accepts, stored_instant_of,
    };
    use crate::coverage::{AcceptanceCriterion, UserStoryRef, VerificationStep};
    use crate::dependency::{Readiness, ReadinessInputs, compute_readiness};
    use crate::plan::SpecNumber;
    use crate::profile::ProfileName;
    use crate::project::ProjectId;
    use crate::ticket::{
        BugQualification, Priority, Severity, TaskMode, TaskSubtype, TaskTiming, Ticket,
        TicketBody, TicketId, TicketKind, TicketNumber, TicketState,
    };
    use crate::timeline_time::stored_format;
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;

    /// One rule-valid one-time activation instant, in the stored UTC
    /// shape.
    const ACTIVATION: &str = "2026-09-10T09:00:00.000Z";

    fn number(value: u64) -> TicketNumber {
        TicketNumber::new(value).expect("the fixture number is positive")
    }

    /// The clear readiness: nothing holds the Ticket back.
    fn clear() -> Readiness {
        compute_readiness(ReadinessInputs {
            dependencies: &[],
            blockers: &[],
        })
    }

    /// A Task body, the bounded kind a human may schedule and drag.
    fn task_body() -> TicketBody {
        TicketBody::task(
            "Archive the old register",
            None,
            Some(TaskSubtype::Operational),
            Some(TaskMode::Human),
            vec![
                crate::ticket::CompletionCriterion::new("The register is archived.")
                    .expect("the fixture outcome binds"),
            ],
            TaskTiming::none(),
        )
        .expect("the fixture body validates")
    }

    /// One complete Bug qualification, so a Bug fixture may leave draft.
    fn qualified() -> BugQualification {
        let story = UserStoryRef::new(
            SpecNumber::new(1).expect("the fixture number is positive"),
            3,
        )
        .expect("the fixture ordinal is positive");
        BugQualification::new(
            "The integration branch survives every landing.",
            "Re land a reviewed change; the branch list still names it.",
            "macOS 26, Kanban 0.1.0, SQLite 3.50.",
            Severity::High,
            "Every landing so far.",
            "All landing reviews of every Project.",
            "Duplicate landings and lost review state.",
            vec![
                AcceptanceCriterion::new("The integration branch survives a landing.", vec![story])
                    .expect("the fixture criterion links"),
            ],
            vec![
                VerificationStep::new("cargo test -p kanban-domain schedule_rules")
                    .expect("the fixture step carries its command"),
            ],
        )
        .expect("the fixture qualification is complete")
    }

    /// A quick-captured Bug body, qualified so it may schedule.
    fn qualified_bug_body() -> TicketBody {
        let captured = TicketBody::bug(
            "Landing drops the integration branch",
            None,
            "The integration branch is dropped after a review lands.",
            "The landing log names the drop immediately after the merge.",
        )
        .expect("the fixture body validates");
        let TicketBody::Bug(boxed) = captured else {
            unreachable!("the fixture body is a Bug's");
        };
        TicketBody::Bug(Box::new(crate::ticket::BugTicket::restore(
            boxed.title().to_owned(),
            boxed.spec(),
            boxed.actual_behaviour().to_owned(),
            boxed.reporter_evidence().to_owned(),
            Some(qualified()),
            crate::ticket::BugFacts::empty(),
        )))
    }

    /// One Ticket in the state a test chooses, at version 1.
    fn ticket_of(body: TicketBody, state: TicketState) -> Ticket {
        Ticket::restore(
            TicketId::new(1),
            ProjectId::new(1),
            number(4),
            Priority::Normal,
            state,
            body,
            None,
            1,
        )
    }

    /// One waiting one-time Schedule bound to the fixture Ticket.
    fn one_time() -> Schedule {
        Schedule::one_time(
            TicketId::new(1),
            "2026-09-10T11:00:00+02:00",
            "Europe/Amsterdam",
            " standard ",
        )
        .expect("the fixture schedule validates")
    }

    #[test]
    fn a_one_time_schedule_carries_trigger_timezone_profile_and_next_activation() {
        let schedule = one_time();

        assert_eq!(
            schedule.trigger(),
            &ScheduleTrigger::OneTime {
                activation: ACTIVATION.to_owned()
            },
            "an offset activation instant normalises to the stored UTC shape"
        );
        assert_eq!(schedule.timezone().as_str(), "Europe/Amsterdam");
        assert_eq!(
            schedule.profile(),
            &ProfileName::new("standard").expect("the fixture name validates"),
            "the eligible profile keeps its trimmed catalogue name"
        );
        assert_eq!(
            schedule.next_activation(),
            ACTIVATION,
            "a one-time schedule's next activation is its activation (DR-SA-01)"
        );
        assert_eq!(schedule.state(), ScheduleState::Waiting);
        assert_eq!(schedule.ticket(), TicketId::new(1));
        assert_eq!(schedule.id(), None, "storage assigns the identity");
    }

    #[test]
    fn a_recurring_schedule_carries_a_cron_expression_and_its_own_next_activation() {
        let schedule = Schedule::recurring(
            TicketId::new(1),
            "*/15 * * * *",
            "UTC",
            "standard",
            "2026-09-10T09:15:00Z",
        )
        .expect("the fixture schedule validates");

        assert_eq!(
            schedule.trigger(),
            &ScheduleTrigger::Recurring {
                expression: CronExpression::new("*/15 * * * *")
                    .expect("the fixture expression validates")
            }
        );
        assert_eq!(schedule.timezone().as_str(), "UTC");
        assert_eq!(
            schedule.next_activation(),
            "2026-09-10T09:15:00.000Z",
            "a recurring schedule's next activation is the computed instant it carries"
        );
        assert_eq!(schedule.state(), ScheduleState::Waiting);
    }

    #[test]
    fn malformed_schedule_facts_are_refused_with_the_field_named() {
        assert_eq!(
            Schedule::one_time(TicketId::new(1), "September", "UTC", "standard").unwrap_err(),
            ScheduleError::MalformedInstant {
                field: "activation",
                value: "September".to_owned(),
            }
        );
        assert_eq!(
            Schedule::recurring(
                TicketId::new(1),
                "*/15 * * * *",
                "UTC",
                "standard",
                "tomorrow",
            )
            .unwrap_err(),
            ScheduleError::MalformedInstant {
                field: "next activation",
                value: "tomorrow".to_owned(),
            }
        );
        assert_eq!(
            ScheduleError::MalformedInstant {
                field: "activation",
                value: "September".to_owned(),
            }
            .to_string(),
            "a Schedule activation must be an RFC 3339 instant: `September`"
        );
    }

    #[test]
    fn a_timezone_names_an_iana_zone() {
        for named in [
            "Europe/Amsterdam",
            "America/Argentina/Buenos_Aires",
            "UTC",
            "Etc/GMT+14",
        ] {
            assert_eq!(
                Timezone::new(named)
                    .expect("the zone name validates")
                    .as_str(),
                named,
                "`{named}` names an IANA zone"
            );
        }
        for refused in [
            "",
            "  ",
            "europe/Amsterdam",
            "/Europe/Amsterdam",
            "Europe/",
            "Europe//Amsterdam",
            "Europe/Amsterdam/",
            "Europe /Amsterdam",
        ] {
            assert_eq!(
                Timezone::new(refused).unwrap_err(),
                ScheduleError::InvalidTimezone {
                    value: refused.trim().to_owned(),
                },
                "`{refused}` names no IANA zone"
            );
        }
        assert_eq!(
            ScheduleError::InvalidTimezone {
                value: "europe/Amsterdam".to_owned(),
            }
            .to_string(),
            "a Schedule timezone names an IANA zone, like `Europe/Amsterdam`"
        );
    }

    #[test]
    fn a_cron_expression_states_five_fields() {
        assert_eq!(
            CronExpression::new("*/15 0-6 * * 1-5")
                .expect("five fields shape a cron expression")
                .as_str(),
            "*/15 0-6 * * 1-5"
        );
        for refused in [
            "",
            "* * * *",
            "* * * * * *",
            "every fifteen minutes",
            "*/15 * * * *!",
        ] {
            assert_eq!(
                CronExpression::new(refused).unwrap_err(),
                ScheduleError::InvalidCron {
                    value: refused.to_owned(),
                },
                "`{refused}` states no five-field cron expression"
            );
        }
        assert_eq!(
            ScheduleError::InvalidCron {
                value: "* * * *".to_owned(),
            }
            .to_string(),
            "a cron expression states five fields: `* * * *`"
        );
    }

    #[test]
    fn a_schedule_names_an_eligible_profile() {
        assert_eq!(
            Schedule::one_time(TicketId::new(1), ACTIVATION, "UTC", "  ").unwrap_err(),
            ScheduleError::InvalidProfile {
                source: crate::profile::ProfileError::Blank("name"),
            },
            "the eligible profile names a catalogue entry"
        );
        assert_eq!(
            ScheduleError::InvalidProfile {
                source: crate::profile::ProfileError::Blank("name"),
            }
            .to_string(),
            crate::profile::ProfileError::Blank("name").to_string()
        );
    }

    #[test]
    fn a_waiting_schedule_becomes_due_at_its_activation_and_only_once() {
        let schedule = one_time();

        assert!(
            !schedule.is_due("2026-09-10T08:59:59.999Z"),
            "before its activation the Ticket stays unavailable (DR-SA-02)"
        );
        assert!(
            schedule.is_due(ACTIVATION),
            "the activation moment itself is due"
        );
        assert!(
            schedule.is_due("2026-09-11T00:00:00.000Z"),
            "an overdue schedule stays due until it fires (DR-SA-06)"
        );

        let fired = schedule.fired();
        assert!(
            !fired.is_due("2026-09-11T00:00:00.000Z"),
            "a fired one-time schedule is never due again"
        );
        assert_eq!(fired.state(), ScheduleState::Fired);
    }

    #[test]
    fn schedule_states_round_trip_through_their_wire_names() {
        assert_eq!(ScheduleState::Waiting.wire_name(), "waiting");
        assert_eq!(ScheduleState::Fired.wire_name(), "fired");
        assert_eq!(
            ScheduleState::parse("waiting"),
            Some(ScheduleState::Waiting)
        );
        assert_eq!(ScheduleState::parse("fired"), Some(ScheduleState::Fired));
        assert_eq!(ScheduleState::parse("ghost"), None);
    }

    #[test]
    fn one_time_activation_makes_a_scheduled_ticket_ready() {
        let mut chore = ticket_of(task_body(), TicketState::Scheduled);
        let schedule = one_time();

        let outcome = schedule
            .activate(&mut chore, &clear())
            .expect("a due one-time activation makes the Ticket ready");

        assert_eq!(outcome, Activation::BecameReady);
        assert_eq!(chore.state(), TicketState::Ready, "DR-SA-03");
        assert_eq!(chore.version(), 2, "the applied move counts");

        // One-time activation serves every kind: a qualified Bug the
        // schedule command held (DR-SA-05) activates the same way.
        let mut defect = ticket_of(qualified_bug_body(), TicketState::Scheduled);
        let outcome = schedule
            .activate(&mut defect, &clear())
            .expect("a scheduled qualified Bug activates");
        assert_eq!(outcome, Activation::BecameReady);
        assert_eq!(defect.state(), TicketState::Ready);
    }

    #[test]
    fn activation_answers_the_readiness_gate() {
        use crate::dependency::{DependencyState, TicketDependency, TicketDependencyGraph};
        let mut chore = ticket_of(task_body(), TicketState::Scheduled);
        let held = compute_readiness(ReadinessInputs {
            dependencies: &[DependencyState {
                dependency: TicketDependency::new(TicketId::new(9), TicketId::new(1)),
                state: TicketState::Active,
            }],
            blockers: &[],
        });
        let _ = TicketDependencyGraph::restore(Vec::new());

        let refused = one_time()
            .activate(&mut chore, &held)
            .expect_err("activation never bypasses readiness");

        assert_eq!(
            refused,
            ScheduleError::Activation {
                cause: crate::lifecycle::LifecycleError::NotReady { waiting_on: 1 },
            }
        );
        assert_eq!(
            refused.to_string(),
            "the Ticket is held back by 1 unresolved dependencies or external blockers"
        );
        assert_eq!(
            chore.state(),
            TicketState::Scheduled,
            "the refusal moved nothing; a later tick retries"
        );
        assert_eq!(chore.version(), 1);
    }

    #[test]
    fn activation_of_a_ticket_already_circulating_spends_the_schedule() {
        for state in [
            TicketState::Ready,
            TicketState::Active,
            TicketState::InReview,
            TicketState::Done,
        ] {
            let mut moved = ticket_of(task_body(), state);
            let outcome = one_time()
                .activate(&mut moved, &clear())
                .expect("a Ticket that moved on by other means needs no activation");
            assert_eq!(outcome, Activation::AlreadyCirculating, "{state:?}");
            assert_eq!(moved.state(), state, "the schedule moves nothing");
            assert_eq!(moved.version(), 1, "no change counted");
        }
    }

    #[test]
    fn implementation_and_bug_tickets_accept_one_time_schedules_only() {
        let one_time = ScheduleTrigger::OneTime {
            activation: ACTIVATION.to_owned(),
        };
        let recurring = ScheduleTrigger::Recurring {
            expression: CronExpression::new("*/15 * * * *")
                .expect("the fixture expression validates"),
        };

        for kind in TicketKind::ALL {
            accepts(*kind, &one_time)
                .unwrap_or_else(|error| panic!("every kind accepts one time: {error}"));
        }
        accepts(TicketKind::Task, &recurring)
            .expect("a Task Ticket may carry a recurring Schedule");
        for kind in [TicketKind::Implementation, TicketKind::Bug] {
            assert_eq!(
                accepts(kind, &recurring).unwrap_err(),
                ScheduleError::RecurringNotSupported { kind },
                "DR-SA-05"
            );
        }
        assert_eq!(
            ScheduleError::RecurringNotSupported {
                kind: TicketKind::Implementation,
            }
            .to_string(),
            "an implementation Ticket accepts one-time schedules only"
        );
    }

    #[test]
    fn recurring_schedules_and_terminal_tickets_activate_nothing_here() {
        let recurring = Schedule::recurring(
            TicketId::new(1),
            "*/15 * * * *",
            "UTC",
            "standard",
            "2026-09-10T09:15:00Z",
        )
        .expect("the fixture schedule validates");
        let mut chore = ticket_of(task_body(), TicketState::Scheduled);
        assert_eq!(
            recurring.activate(&mut chore, &clear()).unwrap_err(),
            ScheduleError::NotOneTime,
            "recurring activations mint occurrences in KAN-T54"
        );
        assert_eq!(
            ScheduleError::NotOneTime.to_string(),
            "a recurring Schedule mints fresh Task occurrences, which KAN-T54 lands"
        );

        let mut cancelled = ticket_of(task_body(), TicketState::Cancelled);
        assert_eq!(
            one_time().activate(&mut cancelled, &clear()).unwrap_err(),
            ScheduleError::TerminalTicket,
            "a terminal Ticket accepts no activation"
        );
        assert_eq!(cancelled.state(), TicketState::Cancelled);
    }

    #[test]
    fn restore_rehydrates_every_recorded_schedule_fact() {
        let restored = Schedule::restore(
            super::ScheduleId::new(3),
            TicketId::new(1),
            ScheduleTrigger::OneTime {
                activation: ACTIVATION.to_owned(),
            },
            Timezone::new("Europe/Amsterdam").expect("the fixture zone validates"),
            ProfileName::new("standard").expect("the fixture name validates"),
            ACTIVATION.to_owned(),
            ScheduleState::Waiting,
        );

        assert_eq!(restored.id(), Some(super::ScheduleId::new(3)));
        assert_eq!(restored.ticket(), TicketId::new(1));
        assert_eq!(restored.timezone().as_str(), "Europe/Amsterdam");
        assert_eq!(restored.profile().as_str(), "standard");
        assert_eq!(restored.next_activation(), ACTIVATION);
        assert_eq!(restored.state(), ScheduleState::Waiting);
        assert!(restored.is_due(ACTIVATION));
    }

    #[test]
    fn stored_instant_of_renders_unix_nanoseconds_in_the_stored_shape() {
        let nanos = OffsetDateTime::parse("2026-09-10T09:00:00Z", &Rfc3339)
            .expect("the fixture instant parses")
            .unix_timestamp_nanos();

        assert_eq!(
            stored_instant_of(nanos).expect("the epoch instant renders"),
            "2026-09-10T09:00:00.000Z"
        );
        assert_eq!(
            stored_instant_of(nanos + 1_000_000).expect("the finer instant renders"),
            "2026-09-10T09:00:00.001Z"
        );
        let _ = stored_format();
    }
}

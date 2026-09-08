//! Failed or expired gates cannot ride their last verdict. A new
//! attempt must revalidate while the earlier history remains.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateStatus {
    Open,
    Failed,
    Expired,
    Approved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevalidationError {
    AttemptRequired,
    WrongTip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateAttempt {
    number: u32,
    tip: String,
    outcome: Option<&'static str>,
    verdicts: Vec<&'static str>,
    invalidations: Vec<&'static str>,
}

impl GateAttempt {
    pub fn number(&self) -> u32 {
        self.number
    }

    pub fn outcome(&self) -> Option<&'static str> {
        self.outcome
    }

    pub fn verdicts(&self) -> &[&'static str] {
        &self.verdicts
    }

    pub fn invalidations(&self) -> &[&'static str] {
        &self.invalidations
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateHistory {
    tip: String,
    status: GateStatus,
    attempts: Vec<GateAttempt>,
}

impl GateHistory {
    pub fn open(tip: impl Into<String>) -> Self {
        let tip = tip.into();
        Self {
            tip: tip.clone(),
            status: GateStatus::Open,
            attempts: vec![GateAttempt {
                number: 1,
                tip,
                outcome: None,
                verdicts: Vec::new(),
                invalidations: Vec::new(),
            }],
        }
    }

    pub fn status(&self) -> GateStatus {
        self.status
    }

    pub fn attempts(&self) -> &[GateAttempt] {
        &self.attempts
    }

    pub fn begin_attempt(&mut self, tip: &str) -> Result<&GateAttempt, RevalidationError> {
        if tip != self.tip {
            return Err(RevalidationError::WrongTip);
        }
        let number = self.attempts.len() as u32 + 1;
        self.status = GateStatus::Open;
        self.attempts.push(GateAttempt {
            number,
            tip: tip.to_owned(),
            outcome: None,
            verdicts: Vec::new(),
            invalidations: Vec::new(),
        });
        Ok(self.attempts.last().expect("the new attempt is stored"))
    }

    pub fn record_verdict(&mut self, number: u32, verdict: &'static str) {
        if let Some(attempt) = self
            .attempts
            .iter_mut()
            .find(|attempt| attempt.number == number)
        {
            attempt.verdicts.push(verdict);
        }
    }

    pub fn record_invalidation(&mut self, number: u32, reason: &'static str) {
        if let Some(attempt) = self
            .attempts
            .iter_mut()
            .find(|attempt| attempt.number == number)
        {
            attempt.invalidations.push(reason);
        }
    }
}

pub fn fail_gate(history: &mut GateHistory, _reason: &str) {
    if let Some(attempt) = history.attempts.last_mut() {
        attempt.outcome = Some("failed");
    }
    history.status = GateStatus::Failed;
}

pub fn expire_gate(history: &mut GateHistory) {
    if let Some(attempt) = history.attempts.last_mut() {
        attempt.outcome = Some("expired");
    }
    history.status = GateStatus::Expired;
}

pub fn revalidate_gate(history: &mut GateHistory, tip: &str) -> Result<(), RevalidationError> {
    if tip != history.tip {
        return Err(RevalidationError::WrongTip);
    }
    match history.status {
        GateStatus::Failed | GateStatus::Expired => Err(RevalidationError::AttemptRequired),
        GateStatus::Open | GateStatus::Approved => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GateHistory, GateStatus, RevalidationError, expire_gate, fail_gate, revalidate_gate,
    };

    fn tip() -> String {
        "a".repeat(40)
    }

    #[test]
    fn gate_revalidation_refuses_approval_after_failure_until_a_new_attempt() {
        let mut history = GateHistory::open(tip());
        fail_gate(&mut history, "blocking finding");
        assert_eq!(history.status(), GateStatus::Failed);
        assert_eq!(
            revalidate_gate(&mut history, &tip()).unwrap_err(),
            RevalidationError::AttemptRequired
        );
        let attempt = history.begin_attempt(&tip()).expect("a new attempt opens");
        assert_eq!(attempt.number(), 2);
        assert_eq!(history.status(), GateStatus::Open);
        assert_eq!(history.attempts().len(), 2);
        assert_eq!(history.attempts()[0].outcome(), Some("failed"));
    }

    #[test]
    fn gate_revalidation_refuses_approval_after_expiry_until_a_new_attempt() {
        let mut history = GateHistory::open(tip());
        expire_gate(&mut history);
        assert_eq!(history.status(), GateStatus::Expired);
        assert_eq!(
            revalidate_gate(&mut history, &tip()).unwrap_err(),
            RevalidationError::AttemptRequired
        );
        history.begin_attempt(&tip()).expect("a new attempt opens");
        assert_eq!(history.status(), GateStatus::Open);
        assert_eq!(history.attempts()[0].outcome(), Some("expired"));
    }

    #[test]
    fn gate_revalidation_preserves_prior_attempts_verdicts_and_invalidations() {
        let mut history = GateHistory::open(tip());
        fail_gate(&mut history, "blocking finding");
        history.record_verdict(1, "rejected");
        history.record_invalidation(1, "content changed");
        history.begin_attempt(&tip()).expect("a new attempt opens");
        let prior = &history.attempts()[0];
        assert_eq!(prior.outcome(), Some("failed"));
        assert_eq!(prior.verdicts(), &["rejected"]);
        assert_eq!(prior.invalidations(), &["content changed"]);
        assert_eq!(history.attempts().len(), 2);
    }
}

use kanban_domain::RunStatus;

#[test]
fn no_verdict_rule_keeps_observations_out_of_execution_status() {
    assert!(
        RunStatus::parse("superseded").is_some(),
        "an explicit retry has its own execution status"
    );
    for observation in [
        "exit_zero",
        "exit_failure",
        "disconnected",
        "stale_deadline",
        "missing_result",
        "approved",
        "rejected",
    ] {
        assert!(
            RunStatus::parse(observation).is_none(),
            "an observation is not an execution verdict: {observation}"
        );
    }
}

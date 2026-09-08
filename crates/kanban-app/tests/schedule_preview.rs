//! Read-only previews use the scheduler's actual calendar semantics.
use kanban_app::{Core, NoopEventSink, exposed_operations};
use kanban_storage::{AllowAllMigrations, Database, RetentionPolicy, SqliteIdempotencyStore};
use serde_json::json;
use std::num::NonZeroU32;
use std::sync::Arc;

fn preview_core() -> (Core, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().unwrap();
    let mut database = Database::open(&dir.path().join("preview.sqlite")).unwrap();
    database.migrate(&AllowAllMigrations).unwrap();
    let mut core = Core::new(
        exposed_operations(),
        Arc::new(SqliteIdempotencyStore::new(
            &database,
            RetentionPolicy::keep_most_recent(NonZeroU32::new(20).unwrap()),
        )),
        Arc::new(NoopEventSink),
    );
    core.register_schedule_preview().unwrap();
    (core, dir)
}

#[test]
fn schedule_preview_shows_spring_dst_and_real_next_activations() {
    let (core, _dir) = preview_core();
    let preview = core
        .query(
            "schedule.preview",
            &json!({
                "cron":"30 1 * * *","timezone":"Europe/London",
                "after":"2026-03-28T23:00:00Z","count":2,
            }),
        )
        .expect("the operator can preview a recurring schedule before saving");
    assert_eq!(preview["dst_behaviour"]["kind"], "fixed_time");
    assert_eq!(
        preview["activations"],
        json!([
            {"utc":"2026-03-29T01:00:00.000Z","local":"2026-03-29T02:00:00.000+01:00"},
            {"utc":"2026-03-30T00:30:00.000Z","local":"2026-03-30T01:30:00.000+01:00"},
        ])
    );
    assert!(
        preview["dst_behaviour"]["spring_forward"]
            .as_str()
            .unwrap()
            .contains("first valid")
    );
}

#[test]
fn schedule_preview_refuses_empty_or_excessive_windows() {
    let (core, _dir) = preview_core();
    for count in [0, 21, 255] {
        let error=core.query("schedule.preview",&json!({
            "cron":"* * * * *","timezone":"UTC","after":"2026-03-28T23:00:00Z","count":count,
        })).expect_err("previews must have a small non-empty result window");
        assert!(error.message.contains("count"), "{error:?}");
    }
}

#[test]
fn one_time_preview_keeps_the_explicit_instant_in_a_dst_fold() {
    let (core, _dir) = preview_core();
    let preview = core
        .query(
            "schedule.preview",
            &json!({
                "activation":"2026-10-25T01:30:00+00:00","timezone":"Europe/London",
                "after":"2026-10-25T00:00:00Z","count":2,
            }),
        )
        .expect("one-time schedules can be previewed too");
    assert_eq!(preview["dst_behaviour"]["kind"], "fixed_instant");
    assert_eq!(
        preview["activations"],
        json!([
            {"utc":"2026-10-25T01:30:00.000Z","local":"2026-10-25T01:30:00.000+00:00"},
        ])
    );
}

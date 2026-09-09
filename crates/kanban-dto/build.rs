//! Build provenance is supplied by the packaging hook, never read at runtime.
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=KANBAN_SOURCE_REVISION");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let (revision, epoch) = match (
        env::var("KANBAN_SOURCE_REVISION").ok(),
        env::var("SOURCE_DATE_EPOCH").ok(),
    ) {
        (None, None) => ("development".to_owned(), 0),
        (Some(revision), Some(epoch)) => {
            assert!(
                revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "KANBAN_SOURCE_REVISION must be a full Git commit identifier"
            );
            let epoch = epoch
                .parse::<u64>()
                .expect("SOURCE_DATE_EPOCH is unsigned seconds");
            (revision, epoch)
        }
        _ => panic!("packaging must supply both source revision and source epoch"),
    };
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    fs::write(
        output.join("build_identity.rs"),
        format!(
            "pub const SOURCE_REVISION: &str = {revision:?};\npub const SOURCE_EPOCH: u64 = {epoch};\n"
        ),
    )
    .expect("build identity is written into Cargo's output directory");
}

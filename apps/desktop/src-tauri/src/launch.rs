//! What the shell's launch arguments ask of it. Linux starts a link's
//! handler with the link as its one argument, so a `kanban:` argument
//! starts the shell holding that argument's text exactly as the
//! operating system passed it: nothing parses or tidies it before the
//! domain judges it (see [`crate::issue_link`]).

use std::ffi::OsString;
use std::path::PathBuf;

use crate::issue_link;

/// One launch, as its arguments ask for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// Start the shell, holding the link a link launch carried.
    Shell { link: Option<String> },
    /// Print the build identity.
    Version,
    /// Probe an installed package against a new data directory.
    PackageSmoke(PathBuf),
    /// Arguments the shell does not know: refuse them with usage.
    Usage,
}

/// Read the arguments a launch was given, the executable's own name
/// left out.
pub fn read(arguments: &[OsString]) -> Launch {
    match arguments {
        [] => Launch::Shell { link: None },
        [link] if issue_link::is_link_argument(link) => Launch::Shell {
            link: link.to_str().map(str::to_owned),
        },
        [flag] if flag == "--version" => Launch::Version,
        [flag, directory] if flag == "--package-smoke" => {
            Launch::PackageSmoke(PathBuf::from(directory))
        }
        _ => Launch::Usage,
    }
}

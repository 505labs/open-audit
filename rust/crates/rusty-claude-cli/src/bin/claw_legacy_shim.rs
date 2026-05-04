//! Backward-compat shim for the legacy `claw` binary name.
//!
//! Prints a single-line deprecation warning to stderr (TTY only) and then
//! delegates to the sibling `openaudit` binary, forwarding all argv and
//! the exit code. Retained for one release after the OpenAudit rebrand so
//! existing scripts and integration tests bound on `CARGO_BIN_EXE_claw`
//! keep working.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::Command;

const LEGACY_NAME_DEPRECATION_NOTICE: &str =
    "warning: the `claw` binary name is deprecated and will be removed in a \
     future release of OpenAudit. Run `openaudit` instead.";

const SUPPRESS_ENV: &str = "OPENAUDIT_SUPPRESS_LEGACY_NAME_WARNING";

fn main() {
    emit_deprecation_warning_if_appropriate();

    let openaudit_path = match locate_sibling_openaudit() {
        Some(path) => path,
        None => {
            eprintln!(
                "error: could not locate the `openaudit` binary alongside `claw`. \
                 Reinstall OpenAudit or run `openaudit` directly."
            );
            std::process::exit(127);
        }
    };

    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let status = Command::new(&openaudit_path).args(args).status();

    match status {
        Ok(exit) => std::process::exit(exit.code().unwrap_or(1)),
        Err(error) => {
            eprintln!(
                "error: failed to invoke `openaudit` at {}: {error}",
                openaudit_path.display()
            );
            std::process::exit(127);
        }
    }
}

fn emit_deprecation_warning_if_appropriate() {
    if std::env::var_os(SUPPRESS_ENV).is_some() {
        return;
    }
    if !std::io::stderr().is_terminal() {
        return;
    }
    eprintln!("{LEGACY_NAME_DEPRECATION_NOTICE}");
}

/// Returns the path to the `openaudit` binary that lives alongside `claw`.
/// Cargo installs put both binaries in the same directory; this resolution
/// also works for `cargo run` and `cargo test` since both bins land in
/// `target/<profile>/`.
fn locate_sibling_openaudit() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    let dir = current.parent()?;
    let candidate = dir.join(if cfg!(windows) {
        "openaudit.exe"
    } else {
        "openaudit"
    });
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

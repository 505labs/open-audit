//! Phase-3 dual-agent acceptance tests.
//!
//! These run the `openaudit audit-demo` subcommand end-to-end and assert that
//! the rendered status panel reflects the expected dual-agent outcome:
//! - The planted SQLi fixture (`fixtures/vuln-sql-injection/`) yields a
//!   Confirmed finding.
//! - The board lists exactly one finding tagged `[HIGH]` with `CWE-89` in
//!   the human-readable summary.
//!
//! The Phase-3 demo subcommand drives a scripted in-process loop, so these
//! tests don't need network access or live API keys. Phase 5 will wire the
//! same DualAgentLoop to live providers + playbook resolution and add a
//! second test pair that runs against fixtures/safe-parameterized/.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR points at this crate's directory:
    // <repo>/rust/crates/rusty-claude-cli/. Walk up three levels to land
    // at the repository root that owns `fixtures/`.
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/<crate>/  -> crates/
    path.pop(); // crates/         -> rust/
    path.pop(); // rust/           -> repo root
    path
}

#[test]
fn audit_demo_runs_against_vuln_sql_fixture_and_publishes_one_finding() {
    let root = repo_root();
    let fixture = root.join("fixtures").join("vuln-sql-injection").join("app.py");
    assert!(
        fixture.exists(),
        "vulnerable SQLi fixture must exist at {}; the dual-agent demo \
         pretends to audit it. Did the fixture file get moved?",
        fixture.display()
    );

    let output = Command::new(env!("CARGO_BIN_EXE_claw"))
        .current_dir(&root)
        .arg("audit-demo")
        .output()
        .expect("openaudit audit-demo should launch");

    assert!(
        output.status.success(),
        "audit-demo exited non-zero.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");

    // Splash banner is printed before the panels.
    assert!(stdout.contains("OPENAUDIT") || stdout.contains("OpenAudit"));

    // Panel headers + lane rows for the auditor and reviewer must appear.
    assert!(stdout.contains("auditor"), "auditor lane missing in:\n{stdout}");
    assert!(stdout.contains("reviewer"), "reviewer lane missing in:\n{stdout}");
    assert!(stdout.contains("planner"), "planner lane missing in:\n{stdout}");

    // The demo's draft is a CWE-89 high-severity finding; assert the panel
    // lists it as Confirmed.
    assert!(stdout.contains("F-0001"), "finding id missing in:\n{stdout}");
    assert!(stdout.contains("[HIGH]"), "high severity tag missing in:\n{stdout}");
    assert!(
        stdout.contains("SQL injection"),
        "demo's planted finding title missing in:\n{stdout}"
    );
    assert!(
        stdout.contains("confirmed"),
        "Confirmed status missing in:\n{stdout}"
    );

    // Final summary line.
    assert!(
        stdout.contains("Dual-agent demo complete"),
        "summary line missing in:\n{stdout}"
    );
    assert!(
        stdout.contains("1 finding(s) confirmed"),
        "confirmed count missing in:\n{stdout}"
    );
    assert!(
        stdout.contains("0 refuted"),
        "refuted count missing in:\n{stdout}"
    );
    assert!(
        stdout.contains("1 fresh reviewer runtime(s) built"),
        "fresh-reviewer counter missing in:\n{stdout}"
    );
}

#[test]
fn safe_parameterized_fixture_is_present_for_phase5_followup() {
    // Documents the negative-control fixture's existence so it doesn't get
    // accidentally deleted before Phase 5 wires up a live provider that
    // exercises it. The actual auditor-drafts-then-reviewer-refutes assertion
    // lives in the Phase-3 unit tests in runtime::audit::tests today; the
    // fixture file is here so future integration work can drive it.
    let path = repo_root()
        .join("fixtures")
        .join("safe-parameterized")
        .join("app.py");
    assert!(
        path.exists(),
        "safe-parameterized fixture must exist at {}",
        path.display()
    );
    let body = std::fs::read_to_string(&path).expect("read fixture");
    // Sanity: each query in the fixture must use the placeholder form.
    assert!(
        body.contains("?"),
        "fixture should use parameterized SQL placeholders"
    );
    assert!(
        !body.contains("WHERE id = {user_id}"),
        "fixture must NOT contain interpolated SQL — it would defeat the purpose"
    );
}

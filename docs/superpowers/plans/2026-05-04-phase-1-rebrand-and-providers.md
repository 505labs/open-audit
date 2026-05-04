# Phase 1 — Rebrand + Provider Abstraction (sub-plan)

> Drafted against `ORIENTATION.md` at HEAD `49f876a`. Target binary: rename `claw` → `openaudit`. Target provider extension: direct Moonshot API at `api.moonshot.ai/v1` with model `kimi-2.6`.

**Goal:** Land a Phase-1 PR that (a) renames the binary, (b) adds direct Moonshot routing, (c) introduces `~/.openaudit/config.toml` with `[providers.*]` and `[roles]`, (d) adds `--dry-run`, while preserving backward compatibility with the existing `claw` binary name and `<cwd>/.claw/` session dir for one release.

**Acceptance:**
- `cargo test --workspace` baseline holds at 512+ passing, 1 pre-existing red, no new failures.
- `cargo clippy --workspace --all-targets -- -D warnings` passes.
- `scripts/fmt.sh --check` passes.
- `openaudit --dry-run` prints a `role | provider | model | base_url | auth_env` table and exits 0.
- `claw` binary still runs but emits a one-line deprecation warning to stderr.

**Working agreement (applies to every task below):**
- Edit ↔ test ↔ commit cycle. One concept per commit.
- Don't restructure files — extend in place. The 13,705-line `main.rs` is not getting decomposed in this phase.
- Preserve all existing test names and assertions; if a test must change, change it minimally and explain why in the commit body.

---

## Task 1.1 — Add Moonshot OpenAI-compat config

**Files:**
- Modify: `rust/crates/api/src/providers/openai_compat.rs` (around lines 19-21 for constants; around 52-78 for factories).

- [ ] **Step 1:** Add a const for the Moonshot base URL near `DEFAULT_DASHSCOPE_BASE_URL`:

```rust
pub const DEFAULT_MOONSHOT_BASE_URL: &str = "https://api.moonshot.ai/v1";
```

- [ ] **Step 2:** Add a `moonshot()` factory next to `dashscope()`:

```rust
pub const fn moonshot() -> Self {
    Self {
        auth_env_var: "MOONSHOT_API_KEY",
        base_url_env_var: "MOONSHOT_BASE_URL",
        default_base_url: DEFAULT_MOONSHOT_BASE_URL,
        provider_name: "moonshot",
    }
}
```

(Field names and exact shape: read `dashscope()` first and mirror it byte-for-byte. The `provider_name` and field set may differ from the sketch above.)

- [ ] **Step 3:** Update `credential_env_vars` to include `MOONSHOT_API_KEY` if that helper enumerates known vars (read line 89 to confirm).

- [ ] **Step 4:** Add a unit test that calls `OpenAiCompatConfig::moonshot()` and asserts the base URL and auth env. Place it next to existing config tests in the same module.

- [ ] **Step 5:** Run `cargo test -p api --lib` and confirm green.

- [ ] **Step 6:** Commit `feat(api): add Moonshot OpenAI-compat config`.

---

## Task 1.2 — Register kimi-2.6 routing

**Files:**
- Modify: `rust/crates/api/src/providers/mod.rs`
- Modify: `rust/crates/api/src/client.rs`

- [ ] **Step 1:** In `mod.rs`, add a `MODEL_REGISTRY` entry near the existing kimi alias (line 126):

```rust
(
    "kimi-2.6",
    ProviderMetadata {
        provider: ProviderKind::OpenAi,
        auth_env: "MOONSHOT_API_KEY",
        base_url_env: "MOONSHOT_BASE_URL",
        default_base_url: openai_compat::DEFAULT_MOONSHOT_BASE_URL,
    },
),
```

Keep the existing `kimi` -> DashScope alias unchanged for backward compatibility. Add a new explicit `kimi-dashscope` alias if needed for tests that bound on DashScope behavior; otherwise leave the existing `kimi` route alone.

- [ ] **Step 2:** Update `metadata_for_model` (mod.rs:166) so that any model whose canonical name matches `kimi-2.*` (or any 2.x variant we want routed via Moonshot direct) returns the Moonshot metadata. Keep the older `kimi-k1.5` / `kimi-k2.5` returning DashScope metadata.

- [ ] **Step 3:** Update `model_token_limit` (mod.rs:294) with a `kimi-2.6` entry. Use **256_000 context, 16_384 max output** as a starting estimate (matches the existing `kimi-k2.5` numbers); refine after consulting Moonshot's published docs.

- [ ] **Step 4:** Update `ProviderClient::from_model` (`client.rs:34-45`) so that when `metadata_for_model` returns a Moonshot route (`auth_env == "MOONSHOT_API_KEY"`), the OpenAI-compat client is built with `OpenAiCompatConfig::moonshot()` rather than `dashscope()` or default `openai()`.

- [ ] **Step 5:** Add a unit test in `mod.rs` that asserts `metadata_for_model("kimi-2.6")` returns Moonshot metadata, and `metadata_for_model("kimi-k2.5")` still returns DashScope metadata.

- [ ] **Step 6:** Add an integration-style test in `rust/crates/api/tests/provider_client_integration.rs` that sets `MOONSHOT_API_KEY=test-key` (no `DASHSCOPE_API_KEY`), calls `ProviderClient::from_model("kimi-2.6")`, and asserts `provider_kind() == ProviderKind::OpenAi` and the configured base URL contains `moonshot.ai`. Use the existing test as a template.

- [ ] **Step 7:** Run `cargo test -p api`. Expected: existing kimi-DashScope tests still pass; new kimi-2.6 tests pass.

- [ ] **Step 8:** Commit `feat(api): route kimi-2.6 through direct Moonshot API`.

---

## Task 1.3 — Rename binary claw -> openaudit (with alias shim)

**Files:**
- Modify: `rust/crates/rusty-claude-cli/Cargo.toml` (lines 8-10 currently declare `[[bin]] name = "claw"`).

- [ ] **Step 1:** Replace the single `[[bin]]` block with two:

```toml
[[bin]]
name = "openaudit"
path = "src/main.rs"

[[bin]]
name = "claw"
path = "src/bin/claw_alias.rs"
```

- [ ] **Step 2:** Create `rust/crates/rusty-claude-cli/src/bin/claw_alias.rs`:

```rust
fn main() {
    eprintln!(
        "warning: the `claw` binary is deprecated and will be removed in a future release. \
         Run `openaudit` instead."
    );
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|s| s.into_string().unwrap_or_default())
        .collect();
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("openaudit")))
        .expect("could not locate sibling openaudit binary");
    let status = std::process::Command::new(exe)
        .args(args)
        .status()
        .expect("failed to invoke openaudit binary");
    std::process::exit(status.code().unwrap_or(1));
}
```

(Resolve sibling-binary lookup using whichever pattern the workspace already uses if one exists; otherwise the `current_exe`-relative fallback above is fine for cargo-installed binaries.)

- [ ] **Step 3:** `cd rust && cargo build -p rusty-claude-cli` — both `target/debug/openaudit` and `target/debug/claw` should now exist.

- [ ] **Step 4:** Run `target/debug/openaudit --help` and `target/debug/claw --help`. The latter should print the deprecation stderr and then the same help text.

- [ ] **Step 5:** Commit `feat(cli): rename binary to openaudit with claw deprecation shim`.

---

## Task 1.4 — Config-dir migration `.claw` → `.openaudit`

**Files:**
- Modify: `rust/crates/runtime/src/session_control.rs:40` (the `.join(".claw")` literal).

- [ ] **Step 1:** Change `SessionStore::from_cwd` (line 32-50) to:
  1. Compute candidate `.openaudit` path under cwd.
  2. If `.openaudit/` exists, use it.
  3. Else if `.claw/` exists, log a one-line stderr migration notice and copy `.claw/sessions/` → `.openaudit/sessions/` (or rename atomically). After migration, use `.openaudit/`.
  4. Else create `.openaudit/` fresh.

- [ ] **Step 2:** Add a regression test that creates a tempdir with a `.claw/sessions/<hash>/x.jsonl` file, calls `SessionStore::from_cwd(tempdir)`, and asserts:
  - `.openaudit/sessions/<hash>/x.jsonl` exists.
  - The original session JSONL bytes are preserved.

- [ ] **Step 3:** Add a second regression test where `.openaudit/` already exists alongside `.claw/`. Assert that `.openaudit/` is used as-is and `.claw/` is left untouched (no double-migration).

- [ ] **Step 4:** Run `cargo test -p runtime --lib session_control`. Expected: green, including the new tests.

- [ ] **Step 5:** Commit `feat(runtime): migrate .claw -> .openaudit session dir`.

---

## Task 1.5 — `~/.openaudit/config.toml` with `[providers.*]` and `[roles]`

**Files:**
- Create: `rust/crates/runtime/src/openaudit_config.rs`
- Modify: `rust/crates/runtime/src/lib.rs` (add `pub mod openaudit_config;` and re-exports).

- [ ] **Step 1:** Module skeleton:

```rust
//! ~/.openaudit/config.toml loader for OpenAudit role -> provider mapping.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct OpenAuditConfig {
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderEntry>,
    #[serde(default)]
    pub roles: RoleAssignments,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProviderEntry {
    pub model: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct RoleAssignments {
    #[serde(default)]
    pub auditor: Option<String>,
    #[serde(default)]
    pub reviewer: Option<String>,
    #[serde(default)]
    pub planner: Option<String>,
}

#[derive(Debug)]
pub enum ConfigError {
    NotFound(PathBuf),
    Io(std::io::Error),
    Parse(toml::de::Error),
    UnknownProvider(String),
}

impl OpenAuditConfig {
    pub fn load_default() -> Result<Self, ConfigError> {
        let path = default_config_path().ok_or_else(|| ConfigError::NotFound(PathBuf::new()))?;
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let bytes = std::fs::read(path).map_err(ConfigError::Io)?;
        let text = String::from_utf8_lossy(&bytes);
        toml::from_str(&text).map_err(ConfigError::Parse)
    }

    pub fn resolve_role(&self, role: &str) -> Option<(&str, &ProviderEntry)> {
        let provider_name = match role {
            "auditor" => self.roles.auditor.as_deref(),
            "reviewer" => self.roles.reviewer.as_deref(),
            "planner" => self.roles.planner.as_deref(),
            _ => None,
        }?;
        let entry = self.providers.get(provider_name)?;
        Some((provider_name, entry))
    }
}

pub fn default_config_path() -> Option<PathBuf> {
    let home = std::env::var_os("OPENAUDIT_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".openaudit")))?;
    Some(home.join("config.toml"))
}
```

- [ ] **Step 2:** Add `toml = "0.8"` to `rust/crates/runtime/Cargo.toml` if not already present (check first; the workspace may already pull it).

- [ ] **Step 3:** Re-export from `runtime/src/lib.rs`:

```rust
pub mod openaudit_config;
pub use openaudit_config::{
    default_config_path as openaudit_config_path, ConfigError as OpenAuditConfigError,
    OpenAuditConfig, ProviderEntry as OpenAuditProviderEntry,
    RoleAssignments as OpenAuditRoleAssignments,
};
```

- [ ] **Step 4:** Add unit tests in the same module covering:
  - Parse the example from `docs/OPENAUDIT.md` (auditor=moonshot, reviewer=anthropic).
  - `resolve_role("auditor")` returns the moonshot entry.
  - `resolve_role("unknown")` returns `None`.
  - Empty `[providers]` plus `auditor = "missing"` returns `None` (graceful, not panic).

- [ ] **Step 5:** Run `cargo test -p runtime --lib openaudit_config`.

- [ ] **Step 6:** Commit `feat(runtime): add ~/.openaudit/config.toml loader with role->provider mapping`.

---

## Task 1.6 — `--dry-run` flag

**Files:**
- Modify: `rust/crates/rusty-claude-cli/src/main.rs` near the early arg-parsing dispatch (search for an existing `--help` short-circuit and add `--dry-run` adjacent).

- [ ] **Step 1:** Detect `--dry-run` early in `run()`, before any provider/network dispatch. If set, call a new helper:

```rust
fn print_dry_run_table() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = match runtime::OpenAuditConfig::load_default() {
        Ok(c) => c,
        Err(_) => {
            eprintln!("warning: ~/.openaudit/config.toml not found; showing defaults only");
            runtime::OpenAuditConfig::default()
        }
    };
    println!("{:<10} {:<12} {:<24} {:<40} {}", "ROLE", "PROVIDER", "MODEL", "BASE_URL", "AUTH_ENV");
    for role in ["auditor", "reviewer", "planner"] {
        match cfg.resolve_role(role) {
            Some((provider, entry)) => {
                println!(
                    "{:<10} {:<12} {:<24} {:<40} {}",
                    role,
                    provider,
                    entry.model,
                    entry.base_url.as_deref().unwrap_or("(provider default)"),
                    entry.api_key_env.as_deref().unwrap_or("(provider default)"),
                );
            }
            None => {
                println!("{:<10} {:<12} {:<24} {:<40} {}", role, "(unset)", "-", "-", "-");
            }
        }
    }
    Ok(())
}
```

(`runtime::OpenAuditConfig::default()` requires `#[derive(Default)]` on the struct — add it in Task 1.5.)

- [ ] **Step 2:** Plumb the call from `run()`:

```rust
if std::env::args().any(|a| a == "--dry-run") {
    return print_dry_run_table().map_err(|e| e.to_string());
}
```

- [ ] **Step 3:** Add an integration test in `rust/crates/rusty-claude-cli/tests/cli_flags_and_config_defaults.rs` (mirror an existing test) that:
  1. Writes a fixture `config.toml` to a tempdir with `[providers.moonshot]` + `[roles] auditor = "moonshot"`.
  2. Runs the binary with `OPENAUDIT_HOME=<tempdir>` and arg `--dry-run`.
  3. Asserts stdout contains `auditor`, `moonshot`, and `kimi-2.6`.
  4. Asserts exit code 0.

- [ ] **Step 4:** Commit `feat(cli): add --dry-run that prints role->provider->model resolution`.

---

## Task 1.7 — Verify regression baseline

- [ ] **Step 1:** From repo root run `scripts/fmt.sh --check`. Expected: exit 0.
- [ ] **Step 2:** From `rust/`: `cargo clippy --workspace --all-targets -- -D warnings`. Expected: exit 0.
- [ ] **Step 3:** From `rust/`: `cargo test --workspace 2>&1 | tail -50`. Expected: same baseline (Phase 0 §6) — only the pre-existing `runtime::hooks::tests::malformed_nonempty_hook_output...` red, all other green.
- [ ] **Step 4:** If a new failure appears, do not paper over it — fix the underlying cause or revert the offending commit. Do not mark this task complete until baseline holds.

---

## Task 1.8 — CHANGELOG + user-facing string sweep

- [ ] **Step 1:** Update `CHANGELOG.md` `[Unreleased]` section. Add under `### Added`:
  - Direct Moonshot routing for `kimi-2.6` (`MOONSHOT_API_KEY`, `MOONSHOT_BASE_URL`).
  - `~/.openaudit/config.toml` with `[providers.*]` and `[roles]` keys.
  - `--dry-run` flag.

  Add under `### Changed`:
  - Binary renamed `claw` → `openaudit`. `claw` retained as deprecated shim.
  - Config dir `<cwd>/.claw/` → `<cwd>/.openaudit/` with one-shot copy migration.

- [ ] **Step 2:** Sweep the top-level user-facing surface (`README.md`, `USAGE.md`, `install.sh`, the help string in `main.rs`) for instances of `claw` that should read `openaudit`. **Do not** rename internal symbols (`AnthropicClient`, `claw_settings_*`, struct names, internal docs about the coordination methodology). Only user-facing CLI invocations and copy.

- [ ] **Step 3:** Re-run `scripts/fmt.sh --check` and `cargo test --workspace`.

- [ ] **Step 4:** Commit `docs(phase-1): CHANGELOG + user-facing rebrand sweep`.

---

## Out of scope (Phase 2+)

Explicitly **not** included in this phase, by design:
- New `Provider` trait — the existing enum is fine.
- Audit-specific tools (Phase 2).
- Reviewer agent isolation (Phase 3).
- Evidence store (Phase 4).
- Playbooks (Phase 5).
- SARIF / GitHub Action (Phase 6).
- Hypothesis-board TUI (Phase 7).
- Decomposition of `main.rs` — defer to a post-MVP cleanup PR.
- Fixing the pre-existing `runtime::hooks` red — separate ticket; not OpenAudit work.

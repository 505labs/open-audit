# Phase 2 — Audit-specific tools (sub-plan)

> Drafted against `ORIENTATION.md` §2 (tool registry) and `ORIENTATION.md`
> §7 (risks). Phase 1 closed at `cargo test --workspace --no-fail-fast`
> baseline 1105 passed, 1 pre-existing red (`runtime::hooks::tests::malformed_nonempty_hook_output...`).
> Phase 2 work must be additive — no new failures introduced.

**Goal:** Land Phase-2 commits that (a) add an `AuditReadOnly` permission
level that is read-only AND networkless by default, (b) add an
`audit_mode` flag on `GlobalToolRegistry` that subtractively filters the
visible tool list down to the audit-safe subset, and (c) introduce three
structured-output audit tools (`note_append`, `finding_draft`,
`finding_finalize`) that write to an in-memory accumulator (the SQLite
evidence store lands in Phase 4). External-tool wrappers (`ast_grep`,
`tree_sitter_query`, `semgrep_run`, `slither_run`, `osv_scan`,
`sandbox_exec`) are sketched at milestone level only — full TDD
detail lands in a future Phase-2 close-out re-draft once the upstream
CLIs are in CI/Docker.

**Acceptance:**

- `cargo test --workspace --no-fail-fast` baseline holds: 1105+ passed,
  the same single pre-existing red, no new failures.
- `cargo clippy --workspace --all-targets -- -D warnings` passes.
- `scripts/fmt.sh --check` passes.
- `runtime::PermissionMode::AuditReadOnly` exists, has `as_str()
  == "audit-read-only"`, and is strictly less permissive than
  `ReadOnly` for ordering.
- `GlobalToolRegistry::builtin_audit_mode()` (or equivalent) returns a
  registry whose `definitions(None)` excludes every built-in tool whose
  `required_permission` is `WorkspaceWrite` or `DangerFullAccess`, and
  excludes `WebFetch` / `WebSearch` (network-touching ReadOnly tools)
  unless an explicit network override is set.
- `mvp_tool_specs()` includes `note_append`, `finding_draft`, and
  `finding_finalize` with `required_permission: PermissionMode::WorkspaceWrite`,
  schemas as specified below, and round-trip through
  `execute_tool` with valid inputs.

**Working agreement (applies to every task below):**

- TDD discipline: write the failing test, run it red, implement, run
  green, commit. One concept per commit.
- Reuse the existing `PermissionMode` taxonomy. Do not introduce a
  parallel one.
- The audit-mode filter is **purely subtractive**: existing tools keep
  their current `required_permission` values; the registry just hides
  them from the agent's tool list.
- The structured-output tools write to an in-memory accumulator only.
  Phase 4 will replace the in-memory shim with SQLite + blob storage.
- No new external dependencies (no `semgrep`, `slither`, `osv`, etc.).
  Tasks 2.1-2.3 are purely in-tree.

---

## Task 2.1 — `AuditReadOnly` permission level

**Files:**
- Modify: `rust/crates/runtime/src/permissions.rs` (add variant, update
  `as_str()`, preserve existing `Ord` derivation so the new variant
  sorts strictly below `ReadOnly`).
- Modify (only if needed): `rust/crates/runtime/src/config.rs`
  `parse_permission_mode_label` to recognize the literal
  `"audit-read-only"` (without changing existing labels).

The runtime already exposes a `PermissionMode` enum with
`ReadOnly`, `WorkspaceWrite`, `DangerFullAccess`, `Prompt`, and
`Allow`. The order is significant: `PermissionPolicy::authorize` uses
`current_mode >= required_mode` to decide whether the active session
mode covers the tool's requirement. Adding `AuditReadOnly` as the
**lowest** variant is the right shape — sessions running in audit mode
will fail to satisfy any `>= ReadOnly` requirement that touches the
network, but tools whose required mode is also `AuditReadOnly` (file
reads, glob, grep, LSP) will still authorize.

A separate `ResolvedPermissionMode` enum exists in
`runtime/src/config.rs` and reflects the **config-decoded** mode. Audit
mode is a runtime / CLI-level choice rather than a config-file label,
so for Phase 2 we leave `ResolvedPermissionMode` alone and add the
literal label to `parse_permission_mode_label` only if the config
parser test surface demands it. The Phase-2 baseline is "round-trip
the new variant through `as_str()` and equality" — config decoding can
follow in Phase 4 when audit runs need to be config-frozen.

### Steps

- [ ] **Step 1 (red):** Add a new test next to the existing prompter
  tests in `permissions.rs` `mod tests`:

  ```rust
  #[test]
  fn audit_read_only_label_round_trips_and_orders_below_read_only() {
      let mode = PermissionMode::AuditReadOnly;
      assert_eq!(mode.as_str(), "audit-read-only");
      assert!(mode < PermissionMode::ReadOnly);
      assert!(mode < PermissionMode::WorkspaceWrite);
      assert!(mode < PermissionMode::DangerFullAccess);
  }

  #[test]
  fn audit_read_only_session_denies_workspace_write_tool_without_prompt() {
      let policy = PermissionPolicy::new(PermissionMode::AuditReadOnly)
          .with_tool_requirement("write_file", PermissionMode::WorkspaceWrite)
          .with_tool_requirement("read_file", PermissionMode::ReadOnly);

      assert!(matches!(
          policy.authorize("write_file", "{}", None),
          PermissionOutcome::Deny { .. }
      ));
      // Read-only tools (no network) still authorize.
      assert_eq!(
          policy.authorize("read_file", "{}", None),
          PermissionOutcome::Allow
      );
  }
  ```

- [ ] **Step 2 (red):** Run `cd rust && cargo test -p runtime --lib
  permissions::tests::audit_read_only_label_round_trips_and_orders_below_read_only`.
  Expected: compile error / test failure because the variant does not
  exist.

- [ ] **Step 3 (implement):** Add `AuditReadOnly` as the **first**
  variant of `PermissionMode` (so the derived `Ord` orders it lowest):

  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
  pub enum PermissionMode {
      AuditReadOnly,
      ReadOnly,
      WorkspaceWrite,
      DangerFullAccess,
      Prompt,
      Allow,
  }
  ```

  Extend `as_str()`:

  ```rust
  Self::AuditReadOnly => "audit-read-only",
  ```

  Note: `Prompt` and `Allow` are not "stronger" modes in any meaningful
  sense — they are policy-shape labels. The derived `Ord` already places
  them above `DangerFullAccess`, which the existing tests rely on
  (e.g., `current_mode == PermissionMode::Prompt`). Inserting
  `AuditReadOnly` at the front is safe because existing tests assert
  inequalities involving `ReadOnly` and stronger modes — they remain
  true after the insertion.

- [ ] **Step 4 (verify):** Re-read the existing tests in
  `permissions.rs` to confirm none of them assert `mode == ReadOnly`
  via integer equality or any other machinery that would change. Spot
  check `bash_validation.rs` `validate_read_only(_, mode)` — that
  function explicitly compares against `PermissionMode::ReadOnly` and
  is unaffected by inserting a new variant.

- [ ] **Step 5 (green):** Run
  `cd rust && cargo test -p runtime --lib permissions`. Expected:
  all permission-mode tests, including the two new ones, pass.

- [ ] **Step 6 (clippy):** Run `cd rust && cargo clippy -p runtime
  --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 7 (commit):**

  ```
  feat(runtime): introduce PermissionMode::AuditReadOnly

  Adds a strictly-lowest permission variant intended for OpenAudit's
  audit mode. AuditReadOnly is below ReadOnly in the derived Ord,
  meaning a session running at AuditReadOnly fails to authorize any
  tool whose required_permission is ReadOnly or higher unless an
  explicit allow rule is in place. Network-touching ReadOnly tools
  (WebFetch, WebSearch) thereby become unreachable by default in
  audit mode; the registry-level filter in Task 2.2 enforces this
  visibility.

  Phase 2 task 2.1 of the OpenAudit MVP plan.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  ```

---

## Task 2.2 — Audit-mode tool filter on `GlobalToolRegistry`

**Files:**
- Modify: `rust/crates/tools/src/lib.rs`.

The registry already filters by `allowed_tools: Option<&BTreeSet<String>>`.
Audit mode is a second, orthogonal filter: even when no `allowedTools`
is supplied, audit mode should subtract the dangerous and
network-touching tools from `definitions(None)` and `permission_specs(None)`.

The filter is purely subtractive at the registry layer — tool
permission classifications stay as they are. The `audit_mode` flag
stored on the registry is consulted at `definitions` /
`permission_specs` / `searchable_tool_specs` time, and:

- Drops every built-in spec whose `required_permission` is
  `WorkspaceWrite` or `DangerFullAccess`.
- Drops `WebFetch` and `WebSearch` (network-touching `ReadOnly` tools)
  unless `allow_network` is also `true`.
- Drops every plugin tool whose declared `required_permission` (the
  `&str` form passed to `permission_mode_from_plugin`) decodes to
  `WorkspaceWrite` or `DangerFullAccess`.
- Drops every runtime tool whose `RuntimeToolDefinition::required_permission`
  is `WorkspaceWrite` or `DangerFullAccess`.

The audit-only finding-output tools (Task 2.3) are technically
`WorkspaceWrite` (they mutate the in-memory accumulator), so by
default they are also filtered out. An explicit
`enable_finding_tools: bool` flag — also stored on the registry —
re-includes them by name when set.

### Steps

- [ ] **Step 1 (red):** Add tests in the `tests` module of
  `rust/crates/tools/src/lib.rs`:

  ```rust
  #[test]
  fn audit_mode_registry_hides_workspace_write_and_network_tools() {
      let registry = GlobalToolRegistry::builtin().with_audit_mode(true);
      let names: BTreeSet<String> = registry
          .definitions(None)
          .into_iter()
          .map(|def| def.name)
          .collect();
      // Must include core read-only file tools.
      assert!(names.contains("read_file"), "read_file should remain visible");
      assert!(names.contains("glob_search"));
      assert!(names.contains("grep_search"));
      // Must NOT include workspace-write tools.
      assert!(!names.contains("write_file"));
      assert!(!names.contains("edit_file"));
      assert!(!names.contains("TodoWrite"));
      assert!(!names.contains("NotebookEdit"));
      // Must NOT include danger-full-access tools.
      assert!(!names.contains("bash"));
      assert!(!names.contains("Agent"));
      assert!(!names.contains("REPL"));
      assert!(!names.contains("PowerShell"));
      // Must NOT include network-touching read-only tools by default.
      assert!(!names.contains("WebFetch"));
      assert!(!names.contains("WebSearch"));
  }

  #[test]
  fn audit_mode_disabled_keeps_full_tool_visibility() {
      let registry = GlobalToolRegistry::builtin();
      let names: BTreeSet<String> = registry
          .definitions(None)
          .into_iter()
          .map(|def| def.name)
          .collect();
      assert!(names.contains("bash"), "bash should remain visible without audit mode");
      assert!(names.contains("write_file"));
      assert!(names.contains("WebFetch"));
  }

  #[test]
  fn audit_mode_with_allow_network_re_includes_web_tools() {
      let registry = GlobalToolRegistry::builtin()
          .with_audit_mode(true)
          .with_allow_network(true);
      let names: BTreeSet<String> = registry
          .definitions(None)
          .into_iter()
          .map(|def| def.name)
          .collect();
      assert!(names.contains("WebFetch"));
      assert!(names.contains("WebSearch"));
      // Workspace-write and danger remain hidden even with network override.
      assert!(!names.contains("bash"));
      assert!(!names.contains("write_file"));
  }
  ```

  (The `BTreeSet<String>` collection must own its strings — the
  `definitions` API returns `String` names, so collect into
  `BTreeSet<String>` and use `.contains("name")` against `&str`.)

- [ ] **Step 2 (red):** Run
  `cd rust && cargo test -p tools --lib audit_mode`. Expected:
  compile error because `with_audit_mode` / `with_allow_network` do
  not exist.

- [ ] **Step 3 (implement):** Add the flags to the registry struct:

  ```rust
  #[derive(Debug, Clone)]
  pub struct GlobalToolRegistry {
      plugin_tools: Vec<PluginTool>,
      runtime_tools: Vec<RuntimeToolDefinition>,
      enforcer: Option<PermissionEnforcer>,
      audit_mode: bool,
      allow_network: bool,
      enable_finding_tools: bool,
  }
  ```

  Default the new flags to `false` in `builtin()`, `with_plugin_tools`,
  and `with_runtime_tools` constructors. Add builder-style setters:

  ```rust
  #[must_use]
  pub fn with_audit_mode(mut self, audit_mode: bool) -> Self {
      self.audit_mode = audit_mode;
      self
  }

  #[must_use]
  pub fn with_allow_network(mut self, allow_network: bool) -> Self {
      self.allow_network = allow_network;
      self
  }

  #[must_use]
  pub fn with_finding_tools(mut self, enable: bool) -> Self {
      self.enable_finding_tools = enable;
      self
  }
  ```

  Add an internal helper:

  ```rust
  const NETWORK_TOUCHING_TOOLS: &[&str] = &["WebFetch", "WebSearch"];
  const FINDING_TOOL_NAMES: &[&str] =
      &["note_append", "finding_draft", "finding_finalize"];

  fn audit_filter_keeps(
      &self,
      name: &str,
      required: PermissionMode,
  ) -> bool {
      if !self.audit_mode {
          return true;
      }
      if FINDING_TOOL_NAMES.contains(&name) {
          return self.enable_finding_tools;
      }
      if NETWORK_TOUCHING_TOOLS.contains(&name) && !self.allow_network {
          return false;
      }
      matches!(
          required,
          PermissionMode::AuditReadOnly | PermissionMode::ReadOnly
      )
  }
  ```

  Update `definitions(...)`, `permission_specs(...)`, and
  `searchable_tool_specs(...)` to thread `audit_filter_keeps` over each
  source iterator (built-in, runtime, plugin) before the existing
  `allowed_tools` filter. For plugin tools, use
  `permission_mode_from_plugin(tool.required_permission())` to decode
  to a `PermissionMode`; if decoding fails, treat as
  `DangerFullAccess` (i.e., drop in audit mode — fail closed).

- [ ] **Step 4 (green):** Run
  `cd rust && cargo test -p tools --lib audit_mode`. Expected: all
  three new tests pass.

- [ ] **Step 5 (regression):** Run
  `cd rust && cargo test -p tools --lib`. Expected: existing
  `read_only_registry`, `exposes_mvp_tools`, and other registry tests
  still pass — the audit-mode filter is gated behind the flag so the
  default surface is unchanged.

- [ ] **Step 6 (clippy):** `cd rust && cargo clippy -p tools
  --all-targets -- -D warnings`.

- [ ] **Step 7 (commit):**

  ```
  feat(tools): add audit-mode subtractive filter on GlobalToolRegistry

  Introduces with_audit_mode / with_allow_network /
  with_finding_tools builder setters. When audit_mode is true the
  registry hides every tool whose required_permission is
  WorkspaceWrite or DangerFullAccess, plus the network-touching
  ReadOnly tools (WebFetch, WebSearch) unless allow_network is also
  set. The filter is purely subtractive — tool classifications
  themselves are unchanged.

  Phase 2 task 2.2 of the OpenAudit MVP plan.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  ```

---

## Task 2.3 — Structured-output audit tools (`note_append`, `finding_draft`, `finding_finalize`)

**Files:**
- Modify: `rust/crates/tools/src/lib.rs` (add three `ToolSpec` entries
  to `mvp_tool_specs()`, three executor functions, three input
  structs, and a global accumulator).

These three tools are the seam Phase 4 will replace with SQLite. For
Phase 2 the accumulator is a single
`OnceLock<Mutex<FindingAccumulator>>`. Its contents are visible only
to in-process tests; no JSON file is written under the workspace, no
network call leaves the binary. The runtime caller (CLI / future audit
driver) will read the accumulator after each turn.

### Tool schemas

`note_append`:
```json
{
  "type": "object",
  "properties": {
    "kind": { "type": "string" },
    "body": { "type": "string" }
  },
  "required": ["kind", "body"],
  "additionalProperties": false
}
```
Returns `{"status": "ok"}`. No finding ID — notes are auxiliary
breadcrumbs, not findings.

`finding_draft`:
```json
{
  "type": "object",
  "properties": {
    "title": { "type": "string" },
    "severity": {
      "type": "string",
      "enum": ["low", "medium", "high", "critical"]
    },
    "cwe": { "type": "string" },
    "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
    "description": { "type": "string" },
    "evidence": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "kind": {
            "type": "string",
            "enum": ["file_slice", "tool_output"]
          },
          "ref": { "type": "string" }
        },
        "required": ["kind", "ref"],
        "additionalProperties": false
      }
    }
  },
  "required": ["title", "severity", "confidence", "description", "evidence"],
  "additionalProperties": false
}
```
Returns `{"finding_id": "F-<ulid-or-uuid>"}`.

`finding_finalize`:
```json
{
  "type": "object",
  "properties": {
    "finding_id": { "type": "string" },
    "reviewer_verdict": {
      "type": "string",
      "enum": ["confirm", "refute", "needs_more_evidence"]
    },
    "reviewer_rationale": { "type": "string" }
  },
  "required": ["finding_id", "reviewer_verdict", "reviewer_rationale"],
  "additionalProperties": false
}
```
Returns `{"status": "ok"}`.

All three are `required_permission: PermissionMode::WorkspaceWrite`,
which means they are filtered out by audit mode unless
`with_finding_tools(true)` is set on the registry — exactly the
gating behavior Task 2.2's tests assert.

### Steps

- [ ] **Step 1 (red):** Add tests in the `tests` module of
  `rust/crates/tools/src/lib.rs`:

  ```rust
  #[test]
  fn finding_tools_are_registered_with_workspace_write_permission() {
      let specs = mvp_tool_specs();
      for name in ["note_append", "finding_draft", "finding_finalize"] {
          let spec = specs
              .iter()
              .find(|s| s.name == name)
              .unwrap_or_else(|| panic!("{name} should be registered"));
          assert_eq!(spec.required_permission, PermissionMode::WorkspaceWrite);
      }
  }

  #[test]
  fn note_append_round_trips_through_execute_tool() {
      let result = execute_tool(
          "note_append",
          &json!({ "kind": "scoping", "body": "looked at AuthController.java" }),
      )
      .expect("note_append should succeed");
      let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");
      assert_eq!(parsed["status"], "ok");
  }

  #[test]
  fn finding_draft_returns_an_id() {
      let result = execute_tool(
          "finding_draft",
          &json!({
              "title": "SQLi in /api/v1/users",
              "severity": "high",
              "cwe": "CWE-89",
              "confidence": 0.8,
              "description": "User input concatenated into SQL string.",
              "evidence": [
                  { "kind": "file_slice", "ref": "src/api.py:42-58" }
              ]
          }),
      )
      .expect("finding_draft should succeed");
      let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");
      let id = parsed["finding_id"].as_str().expect("finding_id present");
      assert!(id.starts_with("F-"));
  }

  #[test]
  fn finding_finalize_round_trips_against_a_drafted_finding() {
      let drafted: serde_json::Value = serde_json::from_str(
          &execute_tool(
              "finding_draft",
              &json!({
                  "title": "X",
                  "severity": "low",
                  "confidence": 0.5,
                  "description": "Y",
                  "evidence": []
              }),
          )
          .expect("draft"),
      )
      .expect("json");
      let id = drafted["finding_id"].as_str().expect("id").to_string();
      let result = execute_tool(
          "finding_finalize",
          &json!({
              "finding_id": id,
              "reviewer_verdict": "refute",
              "reviewer_rationale": "input is parameterized via prepared statement"
          }),
      )
      .expect("finalize");
      let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");
      assert_eq!(parsed["status"], "ok");
  }

  #[test]
  fn finding_draft_rejects_invalid_severity() {
      let result = execute_tool(
          "finding_draft",
          &json!({
              "title": "X",
              "severity": "spicy",
              "confidence": 0.5,
              "description": "Y",
              "evidence": []
          }),
      );
      assert!(result.is_err(), "invalid severity should error: {result:?}");
  }

  #[test]
  fn finding_finalize_rejects_unknown_id() {
      let result = execute_tool(
          "finding_finalize",
          &json!({
              "finding_id": "F-does-not-exist",
              "reviewer_verdict": "confirm",
              "reviewer_rationale": "n/a"
          }),
      );
      assert!(result.is_err(), "unknown finding id should error: {result:?}");
  }
  ```

- [ ] **Step 2 (red):** Run `cd rust && cargo test -p tools --lib
  finding`. Expected: tests fail because the three tools and the
  accumulator do not exist.

- [ ] **Step 3 (implement):** Inside `rust/crates/tools/src/lib.rs`:

  1. Add three `ToolSpec` entries to the vec returned by
     `mvp_tool_specs()` (anywhere after `StructuredOutput` is fine; keep
     them adjacent for readability):

     ```rust
     ToolSpec {
         name: "note_append",
         description:
             "Append a free-form note to the audit notebook. \
              Body is opaque text; kind is a short tag.",
         input_schema: json!({
             "type": "object",
             "properties": {
                 "kind": { "type": "string" },
                 "body": { "type": "string" }
             },
             "required": ["kind", "body"],
             "additionalProperties": false
         }),
         required_permission: PermissionMode::WorkspaceWrite,
     },
     ToolSpec {
         name: "finding_draft",
         description:
             "Draft a security finding with severity, evidence, and \
              confidence. Returns a finding_id.",
         input_schema: json!({
             "type": "object",
             "properties": {
                 "title": { "type": "string" },
                 "severity": {
                     "type": "string",
                     "enum": ["low", "medium", "high", "critical"]
                 },
                 "cwe": { "type": "string" },
                 "confidence": {
                     "type": "number",
                     "minimum": 0.0,
                     "maximum": 1.0
                 },
                 "description": { "type": "string" },
                 "evidence": {
                     "type": "array",
                     "items": {
                         "type": "object",
                         "properties": {
                             "kind": {
                                 "type": "string",
                                 "enum": ["file_slice", "tool_output"]
                             },
                             "ref": { "type": "string" }
                         },
                         "required": ["kind", "ref"],
                         "additionalProperties": false
                     }
                 }
             },
             "required": [
                 "title", "severity", "confidence", "description", "evidence"
             ],
             "additionalProperties": false
         }),
         required_permission: PermissionMode::WorkspaceWrite,
     },
     ToolSpec {
         name: "finding_finalize",
         description:
             "Finalize a previously drafted finding with a reviewer verdict.",
         input_schema: json!({
             "type": "object",
             "properties": {
                 "finding_id": { "type": "string" },
                 "reviewer_verdict": {
                     "type": "string",
                     "enum": ["confirm", "refute", "needs_more_evidence"]
                 },
                 "reviewer_rationale": { "type": "string" }
             },
             "required": [
                 "finding_id", "reviewer_verdict", "reviewer_rationale"
             ],
             "additionalProperties": false
         }),
         required_permission: PermissionMode::WorkspaceWrite,
     },
     ```

  2. Add the input structs:

     ```rust
     #[derive(Debug, Deserialize)]
     struct NoteAppendInput {
         kind: String,
         body: String,
     }

     #[derive(Debug, Deserialize)]
     #[serde(rename_all = "lowercase")]
     enum FindingSeverity {
         Low,
         Medium,
         High,
         Critical,
     }

     #[derive(Debug, Deserialize)]
     #[serde(rename_all = "snake_case")]
     enum FindingEvidenceKind {
         FileSlice,
         ToolOutput,
     }

     #[derive(Debug, Deserialize)]
     struct FindingEvidence {
         kind: FindingEvidenceKind,
         #[serde(rename = "ref")]
         reference: String,
     }

     #[derive(Debug, Deserialize)]
     struct FindingDraftInput {
         title: String,
         severity: FindingSeverity,
         #[serde(default)]
         cwe: Option<String>,
         confidence: f64,
         description: String,
         evidence: Vec<FindingEvidence>,
     }

     #[derive(Debug, Deserialize)]
     #[serde(rename_all = "snake_case")]
     enum ReviewerVerdict {
         Confirm,
         Refute,
         NeedsMoreEvidence,
     }

     #[derive(Debug, Deserialize)]
     struct FindingFinalizeInput {
         finding_id: String,
         reviewer_verdict: ReviewerVerdict,
         reviewer_rationale: String,
     }
     ```

  3. Add the in-memory accumulator. Keep it private to this module —
     Phase 4 will swap it out for a real evidence store and re-export
     a public reader API at that point.

     ```rust
     #[derive(Debug, Default)]
     struct FindingAccumulator {
         notes: Vec<(String, String)>,
         drafts: BTreeMap<String, serde_json::Value>,
         finalizations: BTreeMap<String, serde_json::Value>,
         next_id: u64,
     }

     fn finding_accumulator() -> &'static std::sync::Mutex<FindingAccumulator> {
         use std::sync::OnceLock;
         static ACCUM: OnceLock<std::sync::Mutex<FindingAccumulator>> =
             OnceLock::new();
         ACCUM.get_or_init(|| std::sync::Mutex::new(FindingAccumulator::default()))
     }
     ```

  4. Add the executor functions and wire them into
     `execute_tool_with_enforcer`:

     ```rust
     // dispatcher additions:
     "note_append" => {
         maybe_enforce_permission_check(enforcer, name, input)?;
         from_value::<NoteAppendInput>(input).and_then(run_note_append)
     }
     "finding_draft" => {
         maybe_enforce_permission_check(enforcer, name, input)?;
         from_value::<FindingDraftInput>(input).and_then(run_finding_draft)
     }
     "finding_finalize" => {
         maybe_enforce_permission_check(enforcer, name, input)?;
         from_value::<FindingFinalizeInput>(input).and_then(run_finding_finalize)
     }
     ```

     ```rust
     fn run_note_append(input: NoteAppendInput) -> Result<String, String> {
         let mut accum = finding_accumulator()
             .lock()
             .unwrap_or_else(std::sync::PoisonError::into_inner);
         accum.notes.push((input.kind, input.body));
         to_pretty_json(json!({ "status": "ok" }))
     }

     fn run_finding_draft(input: FindingDraftInput) -> Result<String, String> {
         let severity = match input.severity {
             FindingSeverity::Low => "low",
             FindingSeverity::Medium => "medium",
             FindingSeverity::High => "high",
             FindingSeverity::Critical => "critical",
         };
         let evidence = input
             .evidence
             .into_iter()
             .map(|e| {
                 let kind = match e.kind {
                     FindingEvidenceKind::FileSlice => "file_slice",
                     FindingEvidenceKind::ToolOutput => "tool_output",
                 };
                 json!({ "kind": kind, "ref": e.reference })
             })
             .collect::<Vec<_>>();
         let mut accum = finding_accumulator()
             .lock()
             .unwrap_or_else(std::sync::PoisonError::into_inner);
         accum.next_id = accum.next_id.saturating_add(1);
         let id = format!("F-{:06}", accum.next_id);
         let payload = json!({
             "title": input.title,
             "severity": severity,
             "cwe": input.cwe,
             "confidence": input.confidence,
             "description": input.description,
             "evidence": evidence,
         });
         accum.drafts.insert(id.clone(), payload);
         to_pretty_json(json!({ "finding_id": id }))
     }

     fn run_finding_finalize(input: FindingFinalizeInput) -> Result<String, String> {
         let verdict = match input.reviewer_verdict {
             ReviewerVerdict::Confirm => "confirm",
             ReviewerVerdict::Refute => "refute",
             ReviewerVerdict::NeedsMoreEvidence => "needs_more_evidence",
         };
         let mut accum = finding_accumulator()
             .lock()
             .unwrap_or_else(std::sync::PoisonError::into_inner);
         if !accum.drafts.contains_key(&input.finding_id) {
             return Err(format!(
                 "unknown finding_id `{}`; did you call finding_draft first?",
                 input.finding_id
             ));
         }
         accum.finalizations.insert(
             input.finding_id,
             json!({
                 "reviewer_verdict": verdict,
                 "reviewer_rationale": input.reviewer_rationale,
             }),
         );
         to_pretty_json(json!({ "status": "ok" }))
     }
     ```

- [ ] **Step 4 (green):** Run `cd rust && cargo test -p tools --lib
  finding note_append`. Expected: all six finding-related tests pass.

  > Note on test isolation: the accumulator is a process-global
  > singleton, so the tests must not bind on `next_id` being a
  > particular value (they only assert `id.starts_with("F-")` and
  > round-trips finalize against the id they got back, which is
  > order-independent).

- [ ] **Step 5 (regression):** `cd rust && cargo test -p tools --lib`
  to confirm the existing `exposes_mvp_tools` test (and friends) still
  pass. The new `mvp_tool_specs()` entries are additive.

- [ ] **Step 6 (clippy):** `cd rust && cargo clippy -p tools
  --all-targets -- -D warnings`.

- [ ] **Step 7 (commit):**

  ```
  feat(tools): add note_append, finding_draft, finding_finalize

  Three structured-output audit tools backed by an in-process
  accumulator. Schemas are validated by serde at the dispatcher
  boundary; finalize fails closed if asked to operate on an unknown
  finding_id. All three are required_permission WorkspaceWrite, so
  Task 2.2's audit-mode filter hides them from auditor sessions
  unless GlobalToolRegistry::with_finding_tools(true) is set.

  Phase 4 will replace the in-memory shim with the SQLite evidence
  store. The seam is intentional — the dispatcher calls a single
  accumulator() helper that Phase 4 can swap implementations on.

  Phase 2 task 2.3 of the OpenAudit MVP plan.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  ```

---

## Task 2.4 — `ast_grep` MCP wrapper (stub)

**Wraps:** the upstream `ast-grep` CLI (`https://ast-grep.github.io/`).
**Schema sketch:** `{ pattern: string, language: string, path?:
string, glob?: string }` → JSON array of matches with file path, span,
captured groups.
**Network requirement:** none (operates on local files).
**Sandbox requirement:** read-only file access; respect workspace
boundary.
**Disposition:** `required_permission: PermissionMode::AuditReadOnly`
(no network, no writes) once Task 2.1 lands.
**Status:** full implementation deferred until `ast-grep` is in CI
(Linux + macOS workflows). Implementation will live in a new
`crates/audit-tools/` crate or under `tools/src/audit/ast_grep.rs`
depending on whether MCP-server pluggability is preferred.

---

## Task 2.5 — `tree_sitter_query` wrapper (stub)

**Wraps:** `tree-sitter` query CLI / Rust bindings.
**Schema sketch:** `{ query: string, language: string, path?: string }`
→ JSON array of matches.
**Network requirement:** none.
**Sandbox requirement:** read-only file access.
**Disposition:** `AuditReadOnly`.
**Status:** deferred. Tree-sitter grammars per language are a
non-trivial dependency; vendoring the few we care about (Solidity,
Python, JS/TS, Java, Go, Rust) needs build-time decisions.

---

## Task 2.6 — `semgrep_run` wrapper (stub)

**Wraps:** `semgrep --config <ruleset> --json` CLI.
**Schema sketch:** `{ ruleset: string|array, path: string,
languages?: array, timeout_seconds?: integer }` → Semgrep's native
JSON output (or a normalized subset).
**Network requirement:** none for local rules; **yes** if `--config
p/ci` is used to fetch the registry. Default to local rules only.
**Sandbox requirement:** read-only file access; CPU/time budget.
**Disposition:** `AuditReadOnly` when run with local rules; gated
behind `--allow-network` when fetching from the registry.
**Status:** deferred until semgrep is in CI Docker.

---

## Task 2.7 — `slither_run` wrapper (stub)

**Wraps:** `slither <target> --json -` for Solidity static analysis.
**Schema sketch:** `{ path: string, solc_version?: string,
exclude_optimizations?: bool }` → Slither's native JSON output.
**Network requirement:** none for analysis itself; `solc-select` will
download solc binaries unless pre-installed.
**Sandbox requirement:** read-only file access on `path`; write
access to a scratch dir for compilation artifacts.
**Disposition:** `WorkspaceWrite` (because of compilation scratch);
gated behind the two-key audit rule.
**Status:** deferred until slither + solc are in CI Docker.

---

## Task 2.8 — `osv_scan` wrapper (stub)

**Wraps:** `osv-scanner --json --lockfile <path>` for SBOM-driven
vulnerability scanning.
**Schema sketch:** `{ lockfile_path: string, ecosystem?: string }` →
OSV-Scanner's native JSON output.
**Network requirement:** **yes** (queries `osv.dev`). Must be gated
behind `--allow-network` and a deterministic version pin of the OSV
database snapshot for replay.
**Sandbox requirement:** read-only file access on lockfile.
**Disposition:** `ReadOnly` once we accept the network requirement;
hidden by audit mode unless `--allow-network` is set.
**Status:** deferred. Replay determinism is the open design question.

---

## Task 2.9 — `sandbox_exec` wrapper (stub)

**Wraps:** `runtime/src/sandbox.rs` (existing Linux `unshare`-based
sandbox) plus a `docker run --network none --read-only` fallback for
macOS dev machines.
**Schema sketch:** `{ command: string, timeout_seconds?: integer,
allowed_mounts?: array, isolate_network?: bool }` → `{ stdout, stderr,
exit_code }`.
**Network requirement:** the sandbox itself runs networkless by
default; the auditor cannot toggle network from within the tool.
**Sandbox requirement:** this **is** the sandbox.
**Disposition:** `DangerFullAccess` underneath, gated behind the
two-key rule (`--enable-sandbox-exec` + interactive confirmation) at
the audit-mode filter layer.
**Status:** deferred until R6 (Linux-only sandbox) is resolved with a
Docker fallback path.

---

## Out of scope (Phase 3+)

Explicitly **not** included in this phase, by design:

- The reviewer agent (`fork_session`-vs-fresh-`ConversationRuntime`
  decision; Phase 3).
- Dual-agent integration tests in `mock_parity_harness.rs` (Phase 3).
- The evidence store (SQLite + content-addressed blobs; Phase 4). The
  in-memory accumulator from Task 2.3 is the deliberate shim.
- A `~/.openaudit/config.toml` audit-mode profile block.
  (`load_default()` will gain an `[audit]` table in Phase 4 when the
  evidence store needs config keys.)
- An `--audit` CLI flag that wires the new permission level + filter
  into the agent loop. The flag is a one-line CLI plumbing change but
  it requires `rusty-claude-cli/src/main.rs` edits that are out of
  scope for the registry-and-permission groundwork. Phase 3 / Phase 5
  will add it once the reviewer and playbooks need it.
- Playbooks (Phase 5).
- SARIF / GitHub Action (Phase 6).
- Hypothesis-board TUI (Phase 7).
- Fixing the pre-existing `runtime::hooks` red — separate ticket; not
  OpenAudit MVP work.

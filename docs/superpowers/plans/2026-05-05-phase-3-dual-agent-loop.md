# Phase 3 — Dual-Agent Loop (auditor + reviewer + planner) sub-plan

> Drafted 2026-05-05 against HEAD `3d3a223`. Phase 1 closed; Phase 2 is partially landed (sub-plan + `PermissionMode::AuditReadOnly`); Phase 3 jumps ahead per the user's request and reads from Phase 2's foundation.

**Goal:** Replace the single-agent loop with a dual-role driver. The **auditor** explores the codebase and drafts findings; the **reviewer** instantiates **fresh per finding** (new `ConversationRuntime`, new system prompt, no cached state from the auditor's session), sees only the finding draft + cited evidence, and votes one of `confirm` / `refute(rationale)` / `needs_more_evidence(request)`. A live **status panel** displays auditor / reviewer / planner state, current tool, and token + dollar counters during the run.

**Acceptance:**
- `openaudit audit ./fixtures/vuln-sql-injection --playbook generic` finds the planted SQLi, the reviewer confirms it, and the finding lands as published with confidence ≥ 0.7.
- `openaudit audit ./fixtures/safe-parameterized --playbook generic` produces an auditor draft, the reviewer refutes it with a rationale citing the parameterized binding, and the finding lands as **refuted** (kept in the report but tagged refuted, not published).
- The status panel renders three lanes (auditor / reviewer / planner) + a "current tool" footer + a live token/$ counter; snapshot test for each state combination.
- Reviewer's `ConversationRuntime` is constructed fresh per finding and shares **no** session state with the auditor.

**Working agreement:**
- TDD: failing test → red → implement → green → commit.
- One concept per commit; reviewable diffs.
- Don't fight the existing `ConversationRuntime` / `ApiClient` / `ToolExecutor` traits — wrap, compose, extend.
- Status panel is **inline text output**, not a full-screen TUI takeover. Phase 7 will lift it into the hypothesis-board renderer; Phase 3 just needs honest progress visibility.
- Do not ship audit-internal tools that write to the workspace filesystem. Findings live in an in-memory `HypothesisBoard` and (later, Phase 4) the evidence store.

---

## Task 3.1 — Finding domain types (`runtime::audit` module)

**Files:**
- Create: `rust/crates/runtime/src/audit.rs`
- Modify: `rust/crates/runtime/src/lib.rs` (add `pub mod audit;` and re-exports)

**Step 1.** Define the data types. They must be `serde` round-trippable so Phase 4's evidence store and Phase 6's renderers can read them without re-defining the schema.

```rust
use serde::{Deserialize, Serialize};

/// Severity tier surfaced to reports. Lower-case-string serialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingSeverity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    /// e.g. "file_slice", "tool_output", "turn".
    pub kind: String,
    /// Free-form locator: a `path:start-end` slice, a tool-call id, etc.
    pub r#ref: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindingDraft {
    pub title: String,
    pub severity: FindingSeverity,
    /// Optional CWE identifier (e.g. "CWE-89").
    pub cwe: Option<String>,
    /// 0.0..=1.0 confidence claimed by the auditor.
    pub confidence: f32,
    pub description: String,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum ReviewVerdict {
    Confirm,
    Refute { rationale: String },
    NeedsMoreEvidence { request: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus {
    Investigating,
    Drafted,
    Reviewing,
    Confirmed,
    Refuted,
    NeedsMoreEvidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub draft: FindingDraft,
    pub status: FindingStatus,
    pub review: Option<ReviewVerdict>,
    /// Number of `needs_more_evidence` round trips the auditor has answered
    /// for this finding (capped to prevent infinite loops).
    pub revision_count: u32,
}

impl Finding {
    pub fn new_drafted(id: impl Into<String>, draft: FindingDraft) -> Self {
        Self {
            id: id.into(),
            draft,
            status: FindingStatus::Drafted,
            review: None,
            revision_count: 0,
        }
    }

    pub fn apply_verdict(&mut self, verdict: ReviewVerdict) {
        self.status = match &verdict {
            ReviewVerdict::Confirm => FindingStatus::Confirmed,
            ReviewVerdict::Refute { .. } => FindingStatus::Refuted,
            ReviewVerdict::NeedsMoreEvidence { .. } => FindingStatus::NeedsMoreEvidence,
        };
        self.review = Some(verdict);
    }
}

/// Public hypothesis-board snapshot consumed by the status panel and (Phase 4) the evidence store.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HypothesisBoard {
    pub findings: Vec<Finding>,
}

impl HypothesisBoard {
    pub fn push_drafted(&mut self, draft: FindingDraft) -> &Finding {
        let id = format!("F-{:04}", self.findings.len() + 1);
        self.findings.push(Finding::new_drafted(id, draft));
        self.findings.last().expect("just pushed")
    }

    pub fn apply_verdict(&mut self, finding_id: &str, verdict: ReviewVerdict) -> bool {
        if let Some(finding) = self.findings.iter_mut().find(|f| f.id == finding_id) {
            finding.apply_verdict(verdict);
            true
        } else {
            false
        }
    }

    pub fn published(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| f.status == FindingStatus::Confirmed)
    }
}
```

**Step 2.** Tests — failing first, then green. Cover:
- `Finding::apply_verdict(Confirm)` flips status to `Confirmed`.
- `apply_verdict(Refute { rationale })` flips status to `Refuted` and stores rationale.
- `HypothesisBoard::push_drafted` assigns sequential `F-0001`, `F-0002` ids.
- `HypothesisBoard::apply_verdict` returns false for unknown id, leaves board untouched.
- Serde round-trip on `Finding` (JSON).

**Step 3.** Re-export from `runtime/src/lib.rs`:

```rust
pub mod audit;
pub use audit::{
    EvidenceRef, Finding, FindingDraft, FindingSeverity, FindingStatus, HypothesisBoard,
    ReviewVerdict,
};
```

**Step 4.** `cd rust && cargo test -p runtime --lib audit` → green; `cargo clippy -p runtime --all-targets -- -D warnings` → clean.

**Step 5.** Commit `feat(runtime): finding domain types and HypothesisBoard`.

---

## Task 3.2 — `DualAgentLoop` driver

**Files:**
- Modify: `rust/crates/runtime/src/audit.rs`

The driver owns the **auditor** `ConversationRuntime`, plus a **factory closure** for fresh reviewer runtimes. It does **not** own a long-lived reviewer — each finding gets a brand-new `ConversationRuntime`.

**Step 1.** Define the trait + struct:

```rust
use crate::conversation::{ApiClient, ConversationRuntime, ToolExecutor};

/// Factory that produces a fresh reviewer runtime for a single finding.
/// The driver invokes this once per `finding_draft` it intercepts; the
/// returned runtime sees only the draft + cited evidence (constructed by
/// the driver, not the auditor) and is dropped after the verdict is captured.
pub trait ReviewerRuntimeFactory {
    fn build(&mut self, finding: &Finding) -> Box<dyn ReviewerSession>;
}

/// Reviewer-side abstraction. Reviewer sees its draft and produces a verdict
/// in a single turn (no tool calls). Real impl uses ConversationRuntime;
/// tests use a scripted stub.
pub trait ReviewerSession {
    fn vote(&mut self, draft: &FindingDraft) -> Result<ReviewVerdict, ReviewerError>;
}

#[derive(Debug)]
pub enum ReviewerError {
    Provider(String),
    UnparseableVerdict(String),
}

pub struct DualAgentLoop<C: ApiClient, T: ToolExecutor, R: ReviewerRuntimeFactory> {
    auditor: ConversationRuntime<C, T>,
    reviewer_factory: R,
    board: HypothesisBoard,
    max_revisions_per_finding: u32,
}
```

**Step 2.** Implement `run_to_completion(initial_prompt) -> Result<HypothesisBoard, RuntimeError>`:

```
loop:
    summary = auditor.run_turn(prompt, ...)
    for tool_result in summary.tool_results:
        if tool was finding_draft:
            parse FindingDraft from tool result payload
            board.push_drafted(draft) → finding
            session = reviewer_factory.build(&finding)
            verdict = session.vote(&finding.draft)?
            board.apply_verdict(&finding.id, verdict.clone())
            match verdict:
                NeedsMoreEvidence{request} if revision_count < max:
                    feed request as next-turn user prompt
                    revision_count++
                else:
                    move on
    if no more open hypotheses or auditor signaled MessageStop:
        break
return Ok(board)
```

**Step 3.** Tests — use a scripted `ApiClient` stub for the auditor (it returns canned `AssistantEvent::ToolUse { name: "finding_draft", input: <json> }` then `MessageStop`) and a scripted `ReviewerSession` stub that returns the verdict the test wants. Cover:
- One auditor finding → reviewer Confirm → `board.published()` includes it.
- One auditor finding → reviewer Refute → `board.published()` is empty; finding still on board with `Refuted` status.
- One auditor finding → reviewer NeedsMoreEvidence → driver re-invokes auditor with the request prompt, revision_count increments.
- Cap: `max_revisions_per_finding = 1` halts the back-and-forth after one round even if reviewer keeps asking.

**Step 4.** Commit `feat(audit): DualAgentLoop driver with fresh-runtime-per-finding reviewer`.

---

## Task 3.3 — Status-panel renderer

**Files:**
- Modify: `rust/crates/runtime/src/audit.rs` (or new `audit_panel.rs` if size warrants)

**Step 1.** Render a multi-line panel from `(board, RolePresence, Option<CurrentTool>, UsageSnapshot)`:

```
┌─ OpenAudit ─────────────────────────────────────────────────────────┐
│ auditor   moonshot/kimi-2.6              ◉ investigating            │
│ reviewer  anthropic/claude-opus-4-7      ◌ idle                     │
│ planner   moonshot/kimi-2.6              ✓ done (3 hypotheses)      │
├─────────────────────────────────────────────────────────────────────┤
│ tool   grep_search  pattern="\$_(GET|POST|REQUEST)"                  │
│ usage  in 12,847 · out 2,103 · cached 8,200 · $0.0214               │
├─────────────────────────────────────────────────────────────────────┤
│ findings                                                            │
│   F-0001 [HIGH ] Reflected XSS in /search?q=          confirmed     │
│   F-0002 [MED  ] Possible SSRF in fetchProxy(url)     reviewing     │
└─────────────────────────────────────────────────────────────────────┘
```

ANSI-colored when `colorize: bool` is true. Plain Unicode-box-drawing otherwise. Status icons map: `Investigating ◉`, `Drafted …`, `Reviewing ◌`, `Confirmed ✓`, `Refuted ✗`, `NeedsMoreEvidence ?`.

**Step 2.** Tests:
- Snapshot a panel with two findings (one Confirmed, one Reviewing) and assert the rendered string contains the right cells.
- Snapshot the colorize=false plain-text variant.
- Empty board → "no findings yet" placeholder row.

**Step 3.** Commit `feat(audit): status-panel renderer for the dual-agent loop`.

---

## Task 3.4 — `openaudit audit` CLI subcommand

**Files:**
- Modify: `rust/crates/rusty-claude-cli/src/main.rs`
  - Add `CliAction::Audit { target_path, playbook_path, output_format }`.
  - Parse it from `openaudit audit <PATH> --playbook <SLUG-OR-PATH>`.
  - Wire it to construct an auditor `ConversationRuntime` from the resolved auditor role and a reviewer-runtime factory bound to the resolved reviewer role (both from `~/.openaudit/config.toml`).
  - Print the status panel after each turn (poll `loop.board()`); print the final summary on completion.

**Step 1.** Add an `--audit` test-only fixture playbook directory under `fixtures/playbooks/generic/` (just `system.md` + `reviewer.md` + `playbook.toml` stubs) so the integration test in 3.5 has something to load.

**Step 2.** When `--no-tui` is in argv, suppress the status panel and stream JSON events instead (`{"kind": "finding", "id": "F-0001", "status": "confirmed", ...}` per line).

**Step 3.** Wire `--dry-run` to also print the resolved auditor / reviewer / planner roles before launching (no audit work; just verifies wiring).

**Step 4.** Commit `feat(cli): openaudit audit subcommand with live status panel`.

---

## Task 3.5 — Fixture-driven acceptance test

**Files:**
- Create: `fixtures/vuln-sql-injection/app.py` — a small Python file with an obvious string-concat SQLi.
- Create: `fixtures/vuln-sql-injection/README.md` — empty placeholder so the fixture is a valid repo.
- Create: `fixtures/safe-parameterized/app.py` — same shape but using `cursor.execute(sql, params)`.
- Create: `rust/crates/rusty-claude-cli/tests/dual_agent_acceptance.rs`.

**Step 1.** Use `mock-anthropic-service` (the existing fixture mock) to script the auditor's responses for both fixtures and the reviewer's verdicts. Two test cases:
- `audit_finds_planted_sqli_and_reviewer_confirms`: scripted auditor emits a `finding_draft` for SQLi; scripted reviewer returns `confirm`; assert `board.published().count() == 1` and the finding's CWE is `CWE-89`.
- `audit_drafts_safe_parameterized_query_and_reviewer_refutes`: scripted auditor emits a `finding_draft`; scripted reviewer returns `refute { rationale: "uses parameterized binding via cursor.execute(sql, params)" }`; assert `board.published().count() == 0` and one finding with `Refuted` status whose rationale mentions "parameterized".

**Step 2.** Commit `test(audit): dual-agent acceptance against vuln-sqli and safe-parameterized fixtures`.

---

## Out of scope (explicitly)

- Real LLM calls — all Phase 3 testing uses scripted mock providers.
- Full Phase 7 hypothesis-board TUI with keybinds (`p`/`k`/`f`/`?`). Phase 3 ships a status **panel**, not a full TUI.
- Evidence store wiring — Phase 4 will wrap the driver with logging adapters.
- Playbook resolution from a registry — Phase 5. Phase 3 only handles `--playbook ./<path>` (local directory).
- Planner agent — its hypothesis-up-front pass is feature-flagged behind `--plan`. Phase 3 leaves the lane visible in the panel but only implements `auditor` + `reviewer` runtime-side.
- Prompt-injection regression test — Phase 3 wires the `<untrusted>` framing into the reviewer's system prompt (Guardrail G1), but the dedicated regression fixture lands when we author the `prompt-injection` fixture in a follow-up.

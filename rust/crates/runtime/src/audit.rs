//! Audit-mode domain types: findings, draft/verdict/status enums, and the
//! per-run `HypothesisBoard` that the dual-agent loop in Phase 3 mutates.
//!
//! These types are **pure data** — no I/O, no tool execution, no API calls.
//! Higher layers (Phase 3 driver, Phase 4 evidence store, Phase 6 reporters)
//! consume them. They are `serde` round-trippable so the evidence store can
//! persist them to `SQLite` blobs and the report renderers can read them back
//! without re-defining the schema.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::conversation::{ApiClient, ConversationRuntime, RuntimeError, ToolError, ToolExecutor};

/// Severity tier surfaced in reports and the status panel. Lowercase string
/// when serialized; matches SARIF's level scheme loosely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingSeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl FindingSeverity {
    /// Short uppercase tag for the status panel ("LOW ", "MED ", "HIGH",
    /// "CRIT") — padded to 4 chars so columns stay aligned.
    #[must_use]
    pub const fn short_tag(self) -> &'static str {
        match self {
            Self::Low => "LOW ",
            Self::Medium => "MED ",
            Self::High => "HIGH",
            Self::Critical => "CRIT",
        }
    }
}

/// Pointer to a piece of evidence the auditor cited for a finding. Phase 4
/// will lift the `r#ref` field into a content-addressed blob hash; Phase 3
/// keeps it as a free-form string (e.g. `"app.py:12-15"`, `"tool_call_42"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub kind: String,
    /// Free-form locator. `r#ref` because `ref` is a Rust keyword.
    pub r#ref: String,
}

/// Auditor-supplied finding draft. The reviewer sees only this struct plus
/// the resolved evidence payloads — never the auditor's transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindingDraft {
    pub title: String,
    pub severity: FindingSeverity,
    /// Optional CWE identifier (e.g. `"CWE-89"` for `SQLi`).
    pub cwe: Option<String>,
    /// Auditor's claimed confidence in `0.0..=1.0`. The reviewer may
    /// overrule.
    pub confidence: f32,
    pub description: String,
    pub evidence: Vec<EvidenceRef>,
}

/// Reviewer's vote on a single finding draft.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum ReviewVerdict {
    Confirm,
    Refute { rationale: String },
    NeedsMoreEvidence { request: String },
}

/// Lifecycle state for a single finding. Drafted → Reviewing → terminal.
/// Investigating is reserved for Phase 7's hypothesis-board lanes; the
/// dual-agent loop only writes Drafted and the post-review terminals.
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

impl FindingStatus {
    #[must_use]
    pub const fn icon(self) -> char {
        match self {
            Self::Investigating => '◉',
            Self::Drafted => '…',
            Self::Reviewing => '◌',
            Self::Confirmed => '✓',
            Self::Refuted => '✗',
            Self::NeedsMoreEvidence => '?',
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Investigating => "investigating",
            Self::Drafted => "drafted",
            Self::Reviewing => "reviewing",
            Self::Confirmed => "confirmed",
            Self::Refuted => "refuted",
            Self::NeedsMoreEvidence => "needs more evidence",
        }
    }
}

/// A finding in the dual-agent loop. Owns its draft, lifecycle status, the
/// reviewer's verdict (once captured), and a counter for `NeedsMoreEvidence`
/// rounds so the driver can cap the back-and-forth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub draft: FindingDraft,
    pub status: FindingStatus,
    pub review: Option<ReviewVerdict>,
    pub revision_count: u32,
}

impl Finding {
    #[must_use]
    pub fn new_drafted(id: impl Into<String>, draft: FindingDraft) -> Self {
        Self {
            id: id.into(),
            draft,
            status: FindingStatus::Drafted,
            review: None,
            revision_count: 0,
        }
    }

    /// Apply a reviewer verdict, updating status accordingly. Drops any
    /// previous review (the reviewer can be re-invoked after a revision).
    pub fn apply_verdict(&mut self, verdict: ReviewVerdict) {
        self.status = match &verdict {
            ReviewVerdict::Confirm => FindingStatus::Confirmed,
            ReviewVerdict::Refute { .. } => FindingStatus::Refuted,
            ReviewVerdict::NeedsMoreEvidence { .. } => FindingStatus::NeedsMoreEvidence,
        };
        self.review = Some(verdict);
    }

    /// Mark the finding as Reviewing while a fresh reviewer runtime is
    /// in flight. Cleared by `apply_verdict`.
    pub fn mark_reviewing(&mut self) {
        self.status = FindingStatus::Reviewing;
    }
}

/// Per-run accumulator. Phase 4 will wrap mutations with evidence-store
/// writes; Phase 3 keeps the board purely in-memory.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HypothesisBoard {
    findings: Vec<Finding>,
}

impl HypothesisBoard {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a fresh `Finding` in `Drafted` status with auto-assigned id.
    /// Returns the assigned id so the driver can track per-finding state
    /// without holding a mutable borrow on the board.
    pub fn push_drafted(&mut self, draft: FindingDraft) -> String {
        let id = format!("F-{:04}", self.findings.len() + 1);
        self.findings.push(Finding::new_drafted(id.clone(), draft));
        id
    }

    /// Apply a reviewer verdict to a known finding id. Returns `true` if
    /// the id matched and the verdict was applied; `false` otherwise.
    pub fn apply_verdict(&mut self, finding_id: &str, verdict: ReviewVerdict) -> bool {
        if let Some(finding) = self.findings.iter_mut().find(|f| f.id == finding_id) {
            finding.apply_verdict(verdict);
            true
        } else {
            false
        }
    }

    /// Mark a finding as Reviewing (reviewer in flight). Returns false if
    /// the id is unknown.
    pub fn mark_reviewing(&mut self, finding_id: &str) -> bool {
        if let Some(finding) = self.findings.iter_mut().find(|f| f.id == finding_id) {
            finding.mark_reviewing();
            true
        } else {
            false
        }
    }

    /// All findings, regardless of status. Iteration order matches insertion.
    #[must_use]
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// Findings whose lifecycle terminated in `Confirmed`. These are the
    /// only ones that ship to a published report.
    pub fn published(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| f.status == FindingStatus::Confirmed)
    }

    /// Findings whose lifecycle terminated in `Refuted`. Kept in the report
    /// (with a tag) so future runs can see what was actively dismissed.
    pub fn refuted(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| f.status == FindingStatus::Refuted)
    }

    /// Increment the revision counter on a finding (driver bumps this each
    /// time it forwards a `NeedsMoreEvidence` request back to the auditor).
    /// Returns the new count, or `None` if the id is unknown.
    pub fn bump_revision(&mut self, finding_id: &str) -> Option<u32> {
        let finding = self.findings.iter_mut().find(|f| f.id == finding_id)?;
        finding.revision_count = finding.revision_count.saturating_add(1);
        Some(finding.revision_count)
    }
}

// ----------------------------------------------------------------------------
// Dual-agent driver
// ----------------------------------------------------------------------------

/// Name of the structured-output tool the auditor calls to register a finding.
/// The dual-agent driver intercepts this name on the executor side; the tool
/// itself is registered as a no-op return that confirms the draft was queued.
pub const FINDING_DRAFT_TOOL_NAME: &str = "finding_draft";

/// Errors the reviewer side can surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewerError {
    /// Underlying provider failed (network, auth, parse).
    Provider(String),
    /// The reviewer's response could not be parsed into a `ReviewVerdict`.
    UnparseableVerdict(String),
}

impl std::fmt::Display for ReviewerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provider(msg) => write!(f, "reviewer provider error: {msg}"),
            Self::UnparseableVerdict(msg) => write!(f, "reviewer returned unparseable verdict: {msg}"),
        }
    }
}

impl std::error::Error for ReviewerError {}

/// Reviewer-side abstraction. A fresh implementation is constructed by
/// [`ReviewerRuntimeFactory`] for **each** finding so the reviewer cannot see
/// state from a prior finding's review.
pub trait ReviewerSession {
    /// Vote on a single finding draft. The reviewer may inspect cited
    /// evidence (passed to it via the implementor's constructor) but cannot
    /// call tools — its single output is the verdict.
    ///
    /// # Errors
    /// Returns an error if the reviewer's underlying provider call fails or
    /// the response cannot be parsed into a [`ReviewVerdict`].
    fn vote(&mut self, draft: &FindingDraft) -> Result<ReviewVerdict, ReviewerError>;
}

/// Factory that produces a fresh [`ReviewerSession`] per finding. The driver
/// invokes this once per intercepted `finding_draft`; the returned session is
/// dropped after the verdict is captured.
pub trait ReviewerRuntimeFactory {
    /// Build a fresh reviewer session for the given finding. Implementations
    /// typically construct a brand-new `ConversationRuntime` with an
    /// audit-mode system prompt that wraps the finding draft and cited
    /// evidence in `<untrusted>` tags.
    fn build(&mut self, finding: &Finding) -> Box<dyn ReviewerSession>;
}

/// Shared queue between the wrapping tool executor and the dual-agent driver.
/// The executor pushes parsed drafts whenever the auditor calls
/// `finding_draft`; the driver drains the queue between turns.
type DraftQueue = Arc<Mutex<Vec<FindingDraft>>>;

/// `ToolExecutor` wrapper that intercepts `finding_draft` calls, parses the
/// supplied input as a [`FindingDraft`], and stashes it for the driver.
/// All other tool calls forward to the inner executor unchanged.
pub struct DraftCapturingExecutor<T: ToolExecutor> {
    inner: T,
    queue: DraftQueue,
}

impl<T: ToolExecutor> DraftCapturingExecutor<T> {
    fn new(inner: T, queue: DraftQueue) -> Self {
        Self { inner, queue }
    }
}

impl<T: ToolExecutor> ToolExecutor for DraftCapturingExecutor<T> {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        if tool_name == FINDING_DRAFT_TOOL_NAME {
            let draft: FindingDraft = serde_json::from_str(input)
                .map_err(|err| ToolError::new(format!("finding_draft: invalid input: {err}")))?;
            self.queue
                .lock()
                .map_err(|err| ToolError::new(format!("finding_draft: queue poisoned: {err}")))?
                .push(draft);
            return Ok(json!({"queued": true}).to_string());
        }
        self.inner.execute(tool_name, input)
    }
}

/// Cap on how many `NeedsMoreEvidence` round-trips a single finding can
/// trigger. Prevents pathological reviewer/auditor loops.
const DEFAULT_MAX_REVISIONS_PER_FINDING: u32 = 1;

/// Cap on how many auditor turns the driver will run in a single
/// `run_to_completion`. Prevents runaway audits when the auditor keeps
/// emitting drafts indefinitely.
const DEFAULT_MAX_AUDITOR_TURNS: u32 = 32;

/// Outcome of a completed dual-agent run.
#[derive(Debug, Clone, PartialEq)]
pub struct DualAgentRunOutcome {
    pub board: HypothesisBoard,
    pub auditor_turns: u32,
}

/// Dual-agent driver. Owns the auditor's `ConversationRuntime` and a factory
/// that produces a fresh reviewer session per finding. See the Phase-3
/// sub-plan for the state machine.
pub struct DualAgentLoop<C, T, R>
where
    C: ApiClient,
    T: ToolExecutor,
    R: ReviewerRuntimeFactory,
{
    auditor: ConversationRuntime<C, DraftCapturingExecutor<T>>,
    reviewer_factory: R,
    board: HypothesisBoard,
    queue: DraftQueue,
    max_revisions_per_finding: u32,
    max_auditor_turns: u32,
}

impl<C, T, R> DualAgentLoop<C, T, R>
where
    C: ApiClient,
    T: ToolExecutor,
    R: ReviewerRuntimeFactory,
{
    /// Build a driver. The caller hands in an already-configured auditor
    /// runtime (with its own session, system prompt, permission policy)
    /// minus the executor wrapper — this constructor wraps the supplied
    /// executor so the driver can intercept `finding_draft` calls.
    pub fn new(
        session: crate::session::Session,
        api_client: C,
        tool_executor: T,
        permission_policy: crate::permissions::PermissionPolicy,
        system_prompt: Vec<String>,
        reviewer_factory: R,
    ) -> Self {
        let queue: DraftQueue = Arc::new(Mutex::new(Vec::new()));
        let wrapped = DraftCapturingExecutor::new(tool_executor, Arc::clone(&queue));
        let auditor = ConversationRuntime::new(
            session,
            api_client,
            wrapped,
            permission_policy,
            system_prompt,
        );
        Self {
            auditor,
            reviewer_factory,
            board: HypothesisBoard::new(),
            queue,
            max_revisions_per_finding: DEFAULT_MAX_REVISIONS_PER_FINDING,
            max_auditor_turns: DEFAULT_MAX_AUDITOR_TURNS,
        }
    }

    #[must_use]
    pub fn with_max_revisions_per_finding(mut self, cap: u32) -> Self {
        self.max_revisions_per_finding = cap;
        self
    }

    #[must_use]
    pub fn with_max_auditor_turns(mut self, cap: u32) -> Self {
        self.max_auditor_turns = cap;
        self
    }

    /// Snapshot of the current board. Cheap clone; primarily used by the
    /// status-panel renderer between turns.
    #[must_use]
    pub fn board(&self) -> &HypothesisBoard {
        &self.board
    }

    /// Drive the auditor + reviewer loop until either the auditor produces
    /// no new drafts in a turn or `max_auditor_turns` is reached.
    ///
    /// # Errors
    /// Surfaces the first auditor `RuntimeError` or reviewer error encountered.
    /// The board contains all findings drafted up to the point of failure.
    pub fn run_to_completion(
        &mut self,
        initial_prompt: impl Into<String>,
    ) -> Result<DualAgentRunOutcome, DualAgentError> {
        let mut next_prompt = initial_prompt.into();
        let mut turns_run: u32 = 0;

        loop {
            if turns_run >= self.max_auditor_turns {
                break;
            }
            let _summary = self
                .auditor
                .run_turn(next_prompt.clone(), None)
                .map_err(DualAgentError::Auditor)?;
            turns_run = turns_run.saturating_add(1);

            let drained_drafts: Vec<FindingDraft> = {
                let mut queue = self
                    .queue
                    .lock()
                    .map_err(|err| DualAgentError::Internal(format!("queue poisoned: {err}")))?;
                std::mem::take(&mut *queue)
            };

            if drained_drafts.is_empty() {
                break;
            }

            // Two-phase: (1) push all drafts onto the board first so the
            // reviewer factory can borrow the resulting Finding; (2) review
            // each freshly-drafted finding with a fresh reviewer session.
            let new_ids: Vec<String> = drained_drafts
                .into_iter()
                .map(|draft| self.board.push_drafted(draft))
                .collect();

            let mut needs_more_request: Option<String> = None;
            for id in new_ids {
                self.board.mark_reviewing(&id);
                let finding = self
                    .board
                    .findings()
                    .iter()
                    .find(|f| f.id == id)
                    .cloned()
                    .ok_or_else(|| {
                        DualAgentError::Internal(format!(
                            "freshly drafted finding {id} vanished from board"
                        ))
                    })?;
                let mut session = self.reviewer_factory.build(&finding);
                let verdict = session
                    .vote(&finding.draft)
                    .map_err(DualAgentError::Reviewer)?;
                self.board.apply_verdict(&id, verdict.clone());
                if let ReviewVerdict::NeedsMoreEvidence { request } = verdict {
                    let revisions = self.board.bump_revision(&id).unwrap_or(0);
                    if revisions <= self.max_revisions_per_finding && needs_more_request.is_none() {
                        needs_more_request = Some(request);
                    }
                }
            }

            match needs_more_request {
                Some(req) => {
                    next_prompt = req;
                }
                None => {
                    break;
                }
            }
        }

        Ok(DualAgentRunOutcome {
            board: self.board.clone(),
            auditor_turns: turns_run,
        })
    }
}

/// Top-level error from [`DualAgentLoop::run_to_completion`].
#[derive(Debug)]
pub enum DualAgentError {
    Auditor(RuntimeError),
    Reviewer(ReviewerError),
    Internal(String),
}

impl std::fmt::Display for DualAgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auditor(err) => write!(f, "auditor runtime error: {err}"),
            Self::Reviewer(err) => write!(f, "reviewer error: {err}"),
            Self::Internal(msg) => write!(f, "dual-agent driver internal error: {msg}"),
        }
    }
}

impl std::error::Error for DualAgentError {}

#[cfg(test)]
mod tests {
    use super::{
        DualAgentError, DualAgentLoop, EvidenceRef, Finding, FindingDraft, FindingSeverity,
        FindingStatus, HypothesisBoard, ReviewVerdict, ReviewerError, ReviewerRuntimeFactory,
        ReviewerSession,
    };
    use crate::conversation::{ApiClient, ApiRequest, AssistantEvent, RuntimeError, ToolExecutor};
    use crate::permissions::PermissionPolicy;
    use crate::session::Session;
    use crate::usage::TokenUsage;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn sample_draft(title: &str) -> FindingDraft {
        FindingDraft {
            title: title.to_string(),
            severity: FindingSeverity::High,
            cwe: Some("CWE-89".to_string()),
            confidence: 0.82,
            description: "string concat in raw SQL query".to_string(),
            evidence: vec![EvidenceRef {
                kind: "file_slice".to_string(),
                r#ref: "app.py:12-15".to_string(),
            }],
        }
    }

    #[test]
    fn finding_apply_verdict_confirm_flips_status_and_stores_verdict() {
        let mut finding = Finding::new_drafted("F-0001", sample_draft("SQLi"));
        assert_eq!(finding.status, FindingStatus::Drafted);
        assert!(finding.review.is_none());

        finding.apply_verdict(ReviewVerdict::Confirm);

        assert_eq!(finding.status, FindingStatus::Confirmed);
        assert_eq!(finding.review, Some(ReviewVerdict::Confirm));
    }

    #[test]
    fn finding_apply_verdict_refute_preserves_rationale() {
        let mut finding = Finding::new_drafted("F-0001", sample_draft("SQLi"));
        finding.apply_verdict(ReviewVerdict::Refute {
            rationale: "uses parameterized binding".to_string(),
        });
        assert_eq!(finding.status, FindingStatus::Refuted);
        match finding.review.as_ref() {
            Some(ReviewVerdict::Refute { rationale }) => {
                assert_eq!(rationale, "uses parameterized binding");
            }
            other => panic!("expected Refute verdict, got {other:?}"),
        }
    }

    #[test]
    fn finding_apply_verdict_needs_more_evidence_records_request() {
        let mut finding = Finding::new_drafted("F-0001", sample_draft("SQLi"));
        finding.apply_verdict(ReviewVerdict::NeedsMoreEvidence {
            request: "show the call site".to_string(),
        });
        assert_eq!(finding.status, FindingStatus::NeedsMoreEvidence);
    }

    #[test]
    fn hypothesis_board_assigns_sequential_ids() {
        let mut board = HypothesisBoard::new();
        let id1 = board.push_drafted(sample_draft("First"));
        let id2 = board.push_drafted(sample_draft("Second"));
        assert_eq!(id1, "F-0001");
        assert_eq!(id2, "F-0002");
        assert_eq!(board.findings().len(), 2);
    }

    #[test]
    fn hypothesis_board_apply_verdict_returns_false_for_unknown_id() {
        let mut board = HypothesisBoard::new();
        let _id = board.push_drafted(sample_draft("Real"));
        let applied = board.apply_verdict("F-9999", ReviewVerdict::Confirm);
        assert!(!applied);
        // The real finding must remain unmolested.
        assert_eq!(board.findings()[0].status, FindingStatus::Drafted);
    }

    #[test]
    fn hypothesis_board_published_iterator_includes_only_confirmed() {
        let mut board = HypothesisBoard::new();
        let id1 = board.push_drafted(sample_draft("Confirmed one"));
        let id2 = board.push_drafted(sample_draft("Refuted one"));
        let id3 = board.push_drafted(sample_draft("Reviewing one"));
        assert!(board.apply_verdict(&id1, ReviewVerdict::Confirm));
        assert!(board.apply_verdict(
            &id2,
            ReviewVerdict::Refute {
                rationale: "false positive".to_string(),
            }
        ));
        // id3 left in Drafted; reviewer never voted.
        assert!(board.mark_reviewing(&id3));

        let published: Vec<&str> = board.published().map(|f| f.id.as_str()).collect();
        assert_eq!(published, vec![id1.as_str()]);

        let refuted: Vec<&str> = board.refuted().map(|f| f.id.as_str()).collect();
        assert_eq!(refuted, vec![id2.as_str()]);
    }

    #[test]
    fn hypothesis_board_bump_revision_increments_and_caps() {
        let mut board = HypothesisBoard::new();
        let id = board.push_drafted(sample_draft("Investigated again"));
        assert_eq!(board.bump_revision(&id), Some(1));
        assert_eq!(board.bump_revision(&id), Some(2));
        assert_eq!(board.bump_revision("F-9999"), None);
    }

    #[test]
    fn finding_round_trips_through_serde_json() {
        let mut finding = Finding::new_drafted("F-0001", sample_draft("SQLi"));
        finding.apply_verdict(ReviewVerdict::Confirm);
        let json = serde_json::to_string(&finding).expect("serialize");
        let back: Finding = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, finding);
    }

    #[test]
    fn finding_severity_short_tags_are_padded() {
        // The status-panel renderer aligns columns by trusting these tags
        // are exactly four characters wide. Lock that in.
        for sev in [
            FindingSeverity::Low,
            FindingSeverity::Medium,
            FindingSeverity::High,
            FindingSeverity::Critical,
        ] {
            assert_eq!(sev.short_tag().chars().count(), 4, "{sev:?}");
        }
    }

    #[test]
    fn finding_status_icons_are_distinct() {
        // No two statuses should render the same icon — the panel uses the
        // icon as a visual primary cue.
        let icons = [
            FindingStatus::Investigating.icon(),
            FindingStatus::Drafted.icon(),
            FindingStatus::Reviewing.icon(),
            FindingStatus::Confirmed.icon(),
            FindingStatus::Refuted.icon(),
            FindingStatus::NeedsMoreEvidence.icon(),
        ];
        let mut sorted: Vec<char> = icons.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), icons.len(), "icons must be distinct");
    }

    // ------------------------------------------------------------------
    // DualAgentLoop tests — scripted auditor + reviewer stubs
    // ------------------------------------------------------------------

    /// Auditor stub: drives a finite script of `Vec<AssistantEvent>` per turn.
    /// Each call to `stream` returns the next batch in `script`. Used to
    /// simulate the auditor emitting `finding_draft` tool calls.
    struct ScriptedAuditor {
        script: Vec<Vec<AssistantEvent>>,
        cursor: usize,
    }

    impl ApiClient for ScriptedAuditor {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            let idx = self.cursor;
            self.cursor += 1;
            self.script
                .get(idx)
                .cloned()
                .ok_or_else(|| RuntimeError::new("scripted auditor exhausted"))
        }
    }

    /// Trivial executor for non-`finding_draft` tool calls. The driver's
    /// wrapping executor intercepts `finding_draft`; everything else (rare in
    /// these tests) lands here.
    struct NoopExecutor;

    impl ToolExecutor for NoopExecutor {
        fn execute(
            &mut self,
            _tool_name: &str,
            _input: &str,
        ) -> Result<String, super::ToolError> {
            Ok("{}".to_string())
        }
    }

    /// Reviewer stub: returns the next pre-scripted verdict per `vote` call.
    struct ScriptedReviewer {
        verdicts: Rc<RefCell<Vec<ReviewVerdict>>>,
    }

    impl ReviewerSession for ScriptedReviewer {
        fn vote(&mut self, _draft: &FindingDraft) -> Result<ReviewVerdict, ReviewerError> {
            self.verdicts
                .borrow_mut()
                .pop()
                .ok_or_else(|| ReviewerError::Provider("no scripted verdicts left".to_string()))
        }
    }

    struct ScriptedReviewerFactory {
        verdicts: Rc<RefCell<Vec<ReviewVerdict>>>,
        builds: Rc<RefCell<u32>>,
    }

    impl ReviewerRuntimeFactory for ScriptedReviewerFactory {
        fn build(&mut self, _finding: &Finding) -> Box<dyn ReviewerSession> {
            *self.builds.borrow_mut() += 1;
            Box::new(ScriptedReviewer {
                verdicts: Rc::clone(&self.verdicts),
            })
        }
    }

    fn finding_draft_tool_use(id: &str, draft: &FindingDraft) -> AssistantEvent {
        AssistantEvent::ToolUse {
            id: id.to_string(),
            name: super::FINDING_DRAFT_TOOL_NAME.to_string(),
            input: serde_json::to_string(draft).expect("serialize FindingDraft"),
        }
    }

    fn build_driver(
        script: Vec<Vec<AssistantEvent>>,
        scripted_verdicts: Vec<ReviewVerdict>,
    ) -> (
        DualAgentLoop<ScriptedAuditor, NoopExecutor, ScriptedReviewerFactory>,
        Rc<RefCell<u32>>,
    ) {
        // Scripted verdicts pop from the back, so reverse so callers can pass
        // them in the order the driver will consume.
        let mut verdicts = scripted_verdicts;
        verdicts.reverse();
        let verdicts = Rc::new(RefCell::new(verdicts));
        let builds = Rc::new(RefCell::new(0_u32));

        let session = Session::new();
        let driver = DualAgentLoop::new(
            session,
            ScriptedAuditor { script, cursor: 0 },
            NoopExecutor,
            PermissionPolicy::new(crate::permissions::PermissionMode::DangerFullAccess),
            vec!["test system prompt".to_string()],
            ScriptedReviewerFactory {
                verdicts,
                builds: Rc::clone(&builds),
            },
        );
        (driver, builds)
    }

    fn sqli_draft() -> FindingDraft {
        FindingDraft {
            title: "SQL injection in /search".to_string(),
            severity: FindingSeverity::High,
            cwe: Some("CWE-89".to_string()),
            confidence: 0.9,
            description: "Raw string concat in cursor.execute".to_string(),
            evidence: vec![EvidenceRef {
                kind: "file_slice".to_string(),
                r#ref: "app.py:12-15".to_string(),
            }],
        }
    }

    #[test]
    fn dual_agent_confirms_planted_finding_and_publishes_it() {
        let draft = sqli_draft();
        let script = vec![
            vec![
                finding_draft_tool_use("tu-1", &draft),
                AssistantEvent::Usage(TokenUsage {
                    input_tokens: 50,
                    output_tokens: 20,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
                AssistantEvent::MessageStop,
            ],
            // Second turn: auditor produces no more drafts. Driver should
            // detect empty queue and stop without further reviewer calls.
            vec![
                AssistantEvent::TextDelta("done".to_string()),
                AssistantEvent::MessageStop,
            ],
        ];

        let (mut driver, builds) = build_driver(script, vec![ReviewVerdict::Confirm]);
        let outcome = driver
            .run_to_completion("audit this repo")
            .expect("run completes");

        assert_eq!(outcome.board.findings().len(), 1);
        let finding = &outcome.board.findings()[0];
        assert_eq!(finding.status, FindingStatus::Confirmed);
        assert_eq!(finding.draft.cwe.as_deref(), Some("CWE-89"));
        assert_eq!(outcome.board.published().count(), 1);
        assert_eq!(outcome.board.refuted().count(), 0);
        assert_eq!(*builds.borrow(), 1, "exactly one fresh reviewer built");
    }

    #[test]
    fn dual_agent_refutes_false_positive_and_excludes_from_published() {
        let draft = FindingDraft {
            title: "Suspicious query".to_string(),
            severity: FindingSeverity::Medium,
            cwe: Some("CWE-89".to_string()),
            confidence: 0.6,
            description: "Looks like SQL but is parameterized".to_string(),
            evidence: vec![EvidenceRef {
                kind: "file_slice".to_string(),
                r#ref: "app.py:12-15".to_string(),
            }],
        };
        let script = vec![
            vec![
                AssistantEvent::TextDelta("looking".to_string()),
                finding_draft_tool_use("tu-1", &draft),
                AssistantEvent::MessageStop,
            ],
            vec![
                AssistantEvent::TextDelta("done".to_string()),
                AssistantEvent::MessageStop,
            ],
        ];

        let refute = ReviewVerdict::Refute {
            rationale: "uses cursor.execute(sql, params) parameterized binding".to_string(),
        };
        let (mut driver, _builds) = build_driver(script, vec![refute.clone()]);
        let outcome = driver
            .run_to_completion("audit this repo")
            .expect("run completes");

        assert_eq!(outcome.board.findings().len(), 1);
        let finding = &outcome.board.findings()[0];
        assert_eq!(finding.status, FindingStatus::Refuted);
        match finding.review.as_ref() {
            Some(ReviewVerdict::Refute { rationale }) => {
                assert!(rationale.contains("parameterized"));
            }
            other => panic!("expected Refute verdict on the board, got {other:?}"),
        }
        assert_eq!(outcome.board.published().count(), 0);
        assert_eq!(outcome.board.refuted().count(), 1);
    }

    #[test]
    fn dual_agent_revisits_finding_when_reviewer_requests_more_evidence() {
        // Turn 1: auditor drafts. Reviewer returns NeedsMoreEvidence.
        // Turn 2: driver re-prompts auditor with the request; auditor
        //         emits no new drafts (it answered the request inline).
        // Stop.
        //
        // The board's only finding should be tagged NeedsMoreEvidence with
        // revision_count == 1.
        let draft = sqli_draft();
        let script = vec![
            // Turn 1, stream call 1: tool use.
            vec![
                AssistantEvent::TextDelta("looking".to_string()),
                finding_draft_tool_use("tu-1", &draft),
                AssistantEvent::MessageStop,
            ],
            // Turn 1, stream call 2: post-tool follow-up text (no further tools).
            vec![
                AssistantEvent::TextDelta("queued".to_string()),
                AssistantEvent::MessageStop,
            ],
            // Turn 2 (after driver re-prompts with the NeedsMoreEvidence
            // request): plain text, no new drafts.
            vec![
                AssistantEvent::TextDelta("answered".to_string()),
                AssistantEvent::MessageStop,
            ],
        ];

        let nme = ReviewVerdict::NeedsMoreEvidence {
            request: "show the call site that builds the query string".to_string(),
        };
        let (mut driver, builds) = build_driver(script, vec![nme]);
        let outcome = driver
            .run_to_completion("audit this repo")
            .expect("run completes");

        assert_eq!(outcome.board.findings().len(), 1);
        let finding = &outcome.board.findings()[0];
        assert_eq!(finding.status, FindingStatus::NeedsMoreEvidence);
        assert_eq!(
            finding.revision_count, 1,
            "driver must bump revision counter when forwarding the request"
        );
        assert_eq!(*builds.borrow(), 1);
        assert_eq!(outcome.auditor_turns, 2, "two auditor turns expected");
    }

    #[test]
    fn dual_agent_stops_on_empty_first_turn_without_calling_reviewer() {
        // Auditor produces no drafts on the first turn. Driver should exit
        // immediately and never construct a reviewer.
        let script = vec![vec![
            AssistantEvent::TextDelta("nothing to find".to_string()),
            AssistantEvent::MessageStop,
        ]];
        let (mut driver, builds) = build_driver(script, vec![]);
        let outcome = driver
            .run_to_completion("audit this repo")
            .expect("run completes");

        assert_eq!(outcome.board.findings().len(), 0);
        assert_eq!(*builds.borrow(), 0, "reviewer must not be built");
        assert_eq!(outcome.auditor_turns, 1);
    }

    #[test]
    fn dual_agent_surfaces_auditor_error_with_partial_board_intact() {
        // Auditor errors on the first call. Driver should propagate the error
        // and the board should be empty.
        struct FailingAuditor;
        impl ApiClient for FailingAuditor {
            fn stream(&mut self, _r: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Err(RuntimeError::new("api timeout"))
            }
        }

        let session = Session::new();
        let mut driver = DualAgentLoop::new(
            session,
            FailingAuditor,
            NoopExecutor,
            PermissionPolicy::new(crate::permissions::PermissionMode::DangerFullAccess),
            vec!["sys".to_string()],
            ScriptedReviewerFactory {
                verdicts: Rc::new(RefCell::new(vec![])),
                builds: Rc::new(RefCell::new(0)),
            },
        );

        let err = driver
            .run_to_completion("go")
            .expect_err("should surface auditor error");
        match err {
            DualAgentError::Auditor(_) => {}
            other => panic!("expected Auditor error, got {other:?}"),
        }
        assert_eq!(driver.board().findings().len(), 0);
    }
}

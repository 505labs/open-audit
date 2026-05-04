//! Audit-mode domain types: findings, draft/verdict/status enums, and the
//! per-run `HypothesisBoard` that the dual-agent loop in Phase 3 mutates.
//!
//! These types are **pure data** — no I/O, no tool execution, no API calls.
//! Higher layers (Phase 3 driver, Phase 4 evidence store, Phase 6 reporters)
//! consume them. They are `serde` round-trippable so the evidence store can
//! persist them to `SQLite` blobs and the report renderers can read them back
//! without re-defining the schema.

use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::{
        EvidenceRef, Finding, FindingDraft, FindingSeverity, FindingStatus, HypothesisBoard,
        ReviewVerdict,
    };

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
}

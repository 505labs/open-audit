//! Status-panel renderer for the dual-agent loop. Produces a multi-line
//! string suitable for printing between auditor turns, with three role lanes
//! (auditor / reviewer / planner), a current-tool footer, a usage counter,
//! and a list of findings on the [`HypothesisBoard`].
//!
//! ANSI-colored when `colorize` is true. Phase 7 will lift this same renderer
//! into a live full-screen TUI (the hypothesis board).

use std::fmt::Write as _;

use crate::audit::HypothesisBoard;
use crate::usage::TokenUsage;

/// What each role is doing right now. The driver tracks the auditor and
/// reviewer; the planner is shown for visibility but only used when the
/// `--plan` flag is wired in (Phase 3.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneStatus {
    /// Lane is configured but not yet active (planner before --plan, etc).
    Idle,
    /// Lane is doing its current activity. Free-form short tag.
    Busy(String),
    /// Lane finished its run.
    Done,
}

impl LaneStatus {
    fn icon(&self) -> char {
        match self {
            Self::Idle => '◌',
            Self::Busy(_) => '◉',
            Self::Done => '✓',
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Idle => "idle".to_string(),
            Self::Busy(activity) => activity.clone(),
            Self::Done => "done".to_string(),
        }
    }
}

/// Snapshot of one role lane to render in the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneSnapshot {
    /// Role name shown in the leftmost column ("auditor", "reviewer",
    /// "planner").
    pub role: String,
    /// Provider/model identifier ("moonshot/kimi-2.6").
    pub provider_model: String,
    pub status: LaneStatus,
}

/// Currently-running tool, surfaced under the lanes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentTool {
    pub name: String,
    /// Compressed input summary (e.g. `pattern="\\$_(GET|POST)"`). The full
    /// tool input is stored in the evidence layer (Phase 4); this is the
    /// human-readable inline tag.
    pub summary: String,
}

/// Inputs for [`render_status_panel`].
#[derive(Debug, Clone)]
pub struct PanelInputs<'a> {
    pub lanes: &'a [LaneSnapshot],
    pub current_tool: Option<&'a CurrentTool>,
    pub usage: &'a TokenUsage,
    pub usage_cost_usd: f64,
    pub board: &'a HypothesisBoard,
}

/// Render the status panel as a multi-line string. ANSI-colored when
/// `colorize` is true; plain Unicode-box-drawing otherwise.
#[must_use]
pub fn render_status_panel(inputs: &PanelInputs<'_>, colorize: bool) -> String {
    const PANEL_WIDTH: usize = 73;

    let mut buf = String::new();
    let dim_open = if colorize { "\x1b[2m" } else { "" };
    let dim_close = if colorize { "\x1b[0m" } else { "" };
    let bold_open = if colorize { "\x1b[1m" } else { "" };
    let bold_close = if colorize { "\x1b[0m" } else { "" };
    let cyan_open = if colorize { "\x1b[38;5;51m" } else { "" };
    let cyan_close = if colorize { "\x1b[0m" } else { "" };

    // Top banner.
    writeln!(
        buf,
        "┌─ {cyan_open}OpenAudit{cyan_close} {fill}┐",
        fill = "─".repeat(PANEL_WIDTH.saturating_sub(13)),
    )
    .expect("write to String never fails");

    // Lanes.
    for lane in inputs.lanes {
        let icon = lane.status.icon();
        let status_label = lane.status.label();
        writeln!(
            buf,
            "│ {role:<10}{dim_open}{model:<32}{dim_close}  {icon} {status:<22}│",
            role = lane.role,
            model = lane.provider_model,
            status = status_label,
        )
        .expect("write to String never fails");
    }

    // Divider.
    buf.push('├');
    for _ in 0..PANEL_WIDTH.saturating_sub(2) {
        buf.push('─');
    }
    buf.push_str("┤\n");

    // Current tool + usage.
    if let Some(tool) = inputs.current_tool {
        writeln!(
            buf,
            "│ tool   {bold_open}{name}{bold_close}  {dim_open}{summary}{dim_close}",
            name = tool.name,
            summary = tool.summary,
        )
        .expect("write to String never fails");
    } else {
        writeln!(
            buf,
            "│ tool   {dim_open}(no tool currently running){dim_close}",
        )
        .expect("write to String never fails");
    }
    writeln!(
        buf,
        "│ usage  in {input} · out {output} · cached {cache_read} · ${cost:.4}",
        input = format_thousands(u64::from(inputs.usage.input_tokens)),
        output = format_thousands(u64::from(inputs.usage.output_tokens)),
        cache_read = format_thousands(u64::from(inputs.usage.cache_read_input_tokens)),
        cost = inputs.usage_cost_usd,
    )
    .expect("write to String never fails");

    // Divider before findings list.
    buf.push('├');
    for _ in 0..PANEL_WIDTH.saturating_sub(2) {
        buf.push('─');
    }
    buf.push_str("┤\n");

    let findings = inputs.board.findings();
    if findings.is_empty() {
        writeln!(buf, "│ findings  {dim_open}(no findings yet){dim_close}")
            .expect("write to String never fails");
    } else {
        buf.push_str("│ findings\n");
        for finding in findings {
            let icon = finding.status.icon();
            let sev = finding.draft.severity.short_tag();
            let title = truncate_for_panel(&finding.draft.title, 36);
            writeln!(
                buf,
                "│   {id} [{sev}] {title:<36}  {icon} {status}",
                id = finding.id,
                status = finding.status.label(),
            )
            .expect("write to String never fails");
        }
    }

    // Bottom border.
    buf.push('└');
    for _ in 0..PANEL_WIDTH.saturating_sub(2) {
        buf.push('─');
    }
    buf.push_str("┘\n");

    buf
}

fn format_thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (idx, &byte) in bytes.iter().enumerate() {
        let from_end = bytes.len() - idx;
        if idx > 0 && from_end.is_multiple_of(3) {
            out.push(',');
        }
        out.push(byte as char);
    }
    out
}

fn truncate_for_panel(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut truncated: String = s.chars().take(max.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

/// Convenience: build the canonical three-lane snapshot for a typical run.
#[must_use]
pub fn standard_lanes(
    auditor_provider_model: impl Into<String>,
    reviewer_provider_model: impl Into<String>,
    planner_provider_model: impl Into<String>,
    auditor_status: LaneStatus,
    reviewer_status: LaneStatus,
    planner_status: LaneStatus,
) -> Vec<LaneSnapshot> {
    vec![
        LaneSnapshot {
            role: "auditor".to_string(),
            provider_model: auditor_provider_model.into(),
            status: auditor_status,
        },
        LaneSnapshot {
            role: "reviewer".to_string(),
            provider_model: reviewer_provider_model.into(),
            status: reviewer_status,
        },
        LaneSnapshot {
            role: "planner".to_string(),
            provider_model: planner_provider_model.into(),
            status: planner_status,
        },
    ]
}

/// Print the panel + a trailing blank line to a writer. Used by the CLI
/// after each auditor turn to refresh the user-facing status display.
///
/// # Errors
/// Forwards any `io::Error` from `writeln!` calls.
pub fn print_status_panel(
    out: &mut impl std::io::Write,
    inputs: &PanelInputs<'_>,
    colorize: bool,
) -> std::io::Result<()> {
    let rendered = render_status_panel(inputs, colorize);
    out.write_all(rendered.as_bytes())?;
    writeln!(out)
}

#[cfg(test)]
mod tests {
    use super::{
        format_thousands, render_status_panel, standard_lanes, truncate_for_panel, CurrentTool,
        LaneStatus, PanelInputs,
    };
    use crate::audit::{
        EvidenceRef, FindingDraft, FindingSeverity, HypothesisBoard, ReviewVerdict,
    };
    use crate::usage::TokenUsage;

    fn sample_board() -> HypothesisBoard {
        let mut board = HypothesisBoard::new();
        let id1 = board.push_drafted(FindingDraft {
            title: "SQL injection in /search".to_string(),
            severity: FindingSeverity::High,
            cwe: Some("CWE-89".to_string()),
            confidence: 0.9,
            description: String::new(),
            evidence: vec![EvidenceRef {
                kind: "file_slice".to_string(),
                r#ref: "app.py:12-15".to_string(),
            }],
        });
        let id2 = board.push_drafted(FindingDraft {
            title: "Possible SSRF in fetchProxy(url)".to_string(),
            severity: FindingSeverity::Medium,
            cwe: Some("CWE-918".to_string()),
            confidence: 0.55,
            description: String::new(),
            evidence: Vec::new(),
        });
        board.apply_verdict(&id1, ReviewVerdict::Confirm);
        board.mark_reviewing(&id2);
        board
    }

    fn sample_usage() -> TokenUsage {
        TokenUsage {
            input_tokens: 12_847,
            output_tokens: 2_103,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 8_200,
        }
    }

    #[test]
    fn panel_renders_lanes_findings_and_usage_in_plain_text() {
        let lanes = standard_lanes(
            "moonshot/kimi-2.6",
            "anthropic/claude-opus-4-7",
            "moonshot/kimi-2.6",
            LaneStatus::Busy("investigating".to_string()),
            LaneStatus::Idle,
            LaneStatus::Done,
        );
        let usage = sample_usage();
        let board = sample_board();
        let tool = CurrentTool {
            name: "grep_search".to_string(),
            summary: r#"pattern="\$_(GET|POST|REQUEST)""#.to_string(),
        };
        let inputs = PanelInputs {
            lanes: &lanes,
            current_tool: Some(&tool),
            usage: &usage,
            usage_cost_usd: 0.0214,
            board: &board,
        };

        let rendered = render_status_panel(&inputs, false);

        // Lanes
        assert!(rendered.contains("auditor"), "auditor row missing");
        assert!(rendered.contains("moonshot/kimi-2.6"), "moonshot row missing");
        assert!(rendered.contains("investigating"), "auditor activity missing");
        assert!(rendered.contains("anthropic/claude-opus-4-7"), "reviewer model missing");
        assert!(rendered.contains("idle"), "reviewer idle marker missing");
        assert!(rendered.contains("planner"), "planner lane missing");
        assert!(rendered.contains("done"), "planner done marker missing");

        // Tool
        assert!(rendered.contains("grep_search"));
        assert!(rendered.contains(r#"pattern="\$_(GET|POST|REQUEST)""#));

        // Usage
        assert!(rendered.contains("in 12,847"));
        assert!(rendered.contains("out 2,103"));
        assert!(rendered.contains("cached 8,200"));
        assert!(rendered.contains("$0.0214"));

        // Findings
        assert!(rendered.contains("F-0001"));
        assert!(rendered.contains("[HIGH]"));
        assert!(rendered.contains("SQL injection"));
        assert!(rendered.contains("confirmed"));
        assert!(rendered.contains("F-0002"));
        assert!(rendered.contains("[MED ]"));
        assert!(rendered.contains("reviewing"));

        // Box drawing
        assert!(rendered.starts_with("┌"), "panel must start with top-left corner");
        assert!(rendered.trim_end().ends_with("┘"), "panel must end with bottom-right corner");
    }

    #[test]
    fn panel_renders_no_findings_placeholder_for_empty_board() {
        let lanes = standard_lanes(
            "moonshot/kimi-2.6",
            "anthropic/claude-opus-4-7",
            "moonshot/kimi-2.6",
            LaneStatus::Idle,
            LaneStatus::Idle,
            LaneStatus::Idle,
        );
        let usage = TokenUsage::default();
        let board = HypothesisBoard::new();
        let inputs = PanelInputs {
            lanes: &lanes,
            current_tool: None,
            usage: &usage,
            usage_cost_usd: 0.0,
            board: &board,
        };

        let rendered = render_status_panel(&inputs, false);
        assert!(rendered.contains("(no findings yet)"));
        assert!(rendered.contains("(no tool currently running)"));
    }

    #[test]
    fn panel_with_colorize_emits_ansi_escapes() {
        let lanes = standard_lanes(
            "moonshot/kimi-2.6",
            "anthropic/claude-opus-4-7",
            "moonshot/kimi-2.6",
            LaneStatus::Busy("investigating".to_string()),
            LaneStatus::Idle,
            LaneStatus::Idle,
        );
        let usage = TokenUsage::default();
        let board = HypothesisBoard::new();
        let inputs = PanelInputs {
            lanes: &lanes,
            current_tool: None,
            usage: &usage,
            usage_cost_usd: 0.0,
            board: &board,
        };

        let rendered = render_status_panel(&inputs, true);
        // ANSI escape sequence introducer.
        assert!(rendered.contains("\x1b["), "colorized panel must include ANSI escapes");
    }

    #[test]
    fn format_thousands_groups_digits_with_commas() {
        assert_eq!(format_thousands(0), "0");
        assert_eq!(format_thousands(42), "42");
        assert_eq!(format_thousands(1_234), "1,234");
        assert_eq!(format_thousands(1_000_000), "1,000,000");
    }

    #[test]
    fn truncate_for_panel_appends_ellipsis_when_too_long() {
        let truncated = truncate_for_panel("a".repeat(60).as_str(), 10);
        assert_eq!(truncated.chars().count(), 10);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn truncate_for_panel_keeps_short_strings_intact() {
        assert_eq!(truncate_for_panel("short", 36), "short");
    }
}

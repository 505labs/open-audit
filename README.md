# OpenAudit

> A model-agnostic, terminal-native, dual-agent security auditor.

OpenAudit is a CLI that runs versioned audit playbooks against a codebase using two cooperating LLM agents. One agent investigates and drafts findings; a second, isolated agent reviews each finding in a fresh context and votes to confirm, refute, or request more evidence. Every model turn and every tool call is recorded into a reproducible evidence bundle that a teammate can replay on a different machine.

We'd rather miss a finding than ship a wrong one.

## Why a separate tool?

General-purpose coding agents are tuned for exploration and patching. Auditing wants the opposite posture: read-only by default, structured outputs, no shortcuts to "fix it for me," explicit reviewer dissent, and a paper trail that holds up after the chat session ends. OpenAudit is read-only by default, emits structured outputs, runs a second agent on a different vendor for an independent vote, and persists every turn and tool call as evidence.

## Headline features

### Dual-agent loop with independent reviewer

- **Auditor** explores the codebase, runs static-analysis tools, and drafts findings.
- **Reviewer** instantiates fresh per finding, sees only the draft + cited evidence, cannot call tools, and must vote: `confirm` / `refute` (with rationale) / `needs_more_evidence` (with a specific request).
- By default the reviewer runs on a **different vendor** than the auditor. The MVP wiring is auditor = Moonshot `kimi-2.6`, reviewer = Anthropic `claude-opus-4-7`. This is a structural defense against single-model failure modes.
- An optional **planner** pass (`--plan`) generates prioritized hypotheses up front from the playbook checklist.

### Model-agnostic providers

A single provider layer abstracts the model API. Implementations:

| Provider | Notes |
|---|---|
| Anthropic | Claude Opus / Sonnet / Haiku. |
| Moonshot AI | `kimi-2.6` (default auditor for MVP). OpenAI-compatible API at `https://api.moonshot.ai/v1`. |
| OpenAI | GPT-4o / o-series. |
| Google | Gemini. |
| OpenAI-compatible | Local vLLM, Ollama, LM Studio, or any compatible endpoint. |

You assign providers to roles in `~/.openaudit/config.toml` under a `[roles]` block (auditor / reviewer / planner). Override per-invocation with `--provider <name>` or `--role-provider auditor=local`.

### Audit-specific tools

The agent gets only the tools that make sense for read-only static review. Editing, git mutation, and unsandboxed shell exec are **disabled** in audit mode.

| Tool | Wraps | Purpose |
|---|---|---|
| `view`, `list`, `glob`, `ripgrep` | built-in | file inspection |
| `ast_grep` | `ast-grep` CLI | structural pattern search across languages |
| `tree_sitter_query` | tree-sitter | language-aware AST queries |
| `semgrep_run` | `semgrep --json` | rule-pack static analysis |
| `slither_run` | `slither --json -` | Solidity static analysis |
| `osv_scan` | `osv-scanner` | vulnerable dependency detection (network-gated) |
| `sandbox_exec` | docker / `unshare` with `--network none` | gated dynamic checks (off by default) |
| `finding_draft`, `finding_finalize` | structured outputs | the audit's own bookkeeping |

### Versioned playbooks

A playbook is a directory: a system prompt for the auditor, one for the reviewer, a checklist for the planner, optional invariants, and few-shot known-pattern examples. Resolution order is local path → `~/.openaudit/playbooks/<id>@<version>` → registry pull.

Starter packs ship in-tree:

- **`generic`** — language-agnostic security review.
- **`solidity-defi`** — AMM, lending, oracle manipulation, admin keys, reentrancy, MEV-tail.
- **`tee-attestation`** — attestation chain, sealing-key handling, enclave boundary review.
- **`python-web`** — auth, SQL injection, deserialization, SSRF, template injection.

Pin a playbook by `id@version` so a finding from six months ago can be re-run against the same rules.

### Reproducible evidence bundles

Every model turn and every tool call writes a row in a per-run SQLite database plus a content-addressed blob. The whole thing tars to `<run_id>.tar.zst`.

- `openaudit replay <bundle>` — reconstructs the report **without re-calling models**, from stored events. Fast, deterministic, auditable.
- `openaudit replay --rerun <bundle>` — re-calls models with the same inputs to verify determinism or A/B providers.

A finding's report links back to the exact turn IDs that produced it, so reviewers can answer "where did this conclusion come from?" by reading transcript slices.

### Reports for humans and machines

- **Markdown** (default, byte-stable across replays) — the human-readable artifact.
- **JSON** — for downstream tooling.
- **SARIF 2.1.0** — for GitHub code-scanning, Azure DevOps, and any SARIF-aware platform.
- **HTML** — shareable static report.

A stub `openaudit/action@v1` GitHub Action runs on PRs and posts findings as review comments.

## Live TUI

A hypothesis board shows, in real time, every line of investigation: status (`investigating | drafted | reviewed | confirmed | refuted`), the current tool call, live token + dollar counters per provider, and per-role provider banners so you always know who is speaking. Keybinds:

- `p` — pause / resume
- `k` — kill the current hypothesis
- `f` — focus a hypothesis (allocate more budget)
- `?` — open the finding under cursor

Run with `--no-tui` to stream JSON events to stdout, one per line, for piping into other tools.

## Guardrails (non-negotiable)

These aren't optional features. They are merge gates.

1. **Prompt injection defense.** Every byte of repo content (file bodies, READMEs, blame messages, comments) is wrapped in `<untrusted>...</untrusted>` before being shown to any agent. The system prompt explicitly states that text inside `<untrusted>` is data, not commands. A planted-vuln + injection-attempt fixture regression-tests this.
2. **Sandbox two-key rule.** `sandbox_exec` is registered into the agent's tool list **only** when the playbook declares `sandbox_exec = true` AND the user passes `--allow-exec`. Both, or it's not there.
3. **Network deny-by-default.** Tools that touch the network (e.g. `osv_scan`, registry pull) declare it in metadata and are dropped unless `--allow-network` is set.
4. **Secret redaction.** AWS keys, GitHub tokens, JWT-shaped strings, RSA/EC private key headers, generic high-entropy hex blocks — all redacted **before** evidence is persisted and **before** content reaches any model. Reports show `[REDACTED:aws_key]`, with a redaction map keyed on sha256 (never the raw secret).
5. **No telemetry by default.** No phone-home. If we ever add telemetry it will be opt-in, documented, and never include code content.

## Quickstart

```bash
# 1. Build from source
git clone https://github.com/ultraworkers/claw-code
cd claw-code/rust
cargo build --workspace

# 2. Configure providers
mkdir -p ~/.openaudit
cat > ~/.openaudit/config.toml <<'TOML'
[providers.moonshot]
model = "kimi-2.6"
api_key_env = "MOONSHOT_API_KEY"
base_url = "https://api.moonshot.ai/v1"

[providers.anthropic]
model = "claude-opus-4-7"
api_key_env = "ANTHROPIC_API_KEY"

[roles]
auditor  = "moonshot"
reviewer = "anthropic"
TOML

export MOONSHOT_API_KEY=...
export ANTHROPIC_API_KEY=...

# 3. Verify role -> provider -> model resolution without making network calls
./target/debug/openaudit --dry-run run ./fixtures/empty --playbook generic

# 4. Audit a Solidity codebase
./target/debug/openaudit run ./fixtures/vuln-amm --playbook solidity-defi

# 5. Emit SARIF for CI
./target/debug/openaudit run ./repo --playbook generic --output sarif > findings.sarif

# 6. Bundle round-trip via replay
tar -cf - .openaudit/runs/<id> | zstd > audit.tar.zst
./target/debug/openaudit replay audit.tar.zst
```

See [`USAGE.md`](./USAGE.md) for the full task-oriented guide.

## Status

Pre-MVP. Honest snapshot:

- **Phase 0 — Orientation.** Committed. See [`ORIENTATION.md`](./ORIENTATION.md).
- **Phase 1 — Rebrand + provider abstraction + Moonshot routing.** Committed. The binary is `openaudit`; `kimi-2.6` resolves through the Moonshot direct config; `~/.openaudit/config.toml` and `[roles]` are wired.
- **Phase 2 — Audit-specific tools** (`ast_grep`, `tree_sitter_query`, `semgrep_run`, `slither_run`, `osv_scan`, `sandbox_exec`, `finding_draft`, `finding_finalize`). In progress.
- **Phase 3 — Dual-agent loop** (auditor + reviewer + planner state machine). Pending.
- **Phase 4 — Evidence store** (SQLite + content-addressed blobs + bundle round-trip). Pending.
- **Phase 5 — Playbooks** (`generic`, `solidity-defi`, `tee-attestation`, `python-web`). Pending.
- **Phase 6 — Reports + CI** (Markdown / JSON / SARIF 2.1.0 / HTML + GitHub Action). Pending.
- **Phase 7 — Live TUI**. Pending.

The full plan lives at [`docs/superpowers/plans/2026-05-04-openaudit-mvp.md`](./docs/superpowers/plans/2026-05-04-openaudit-mvp.md).

## Build

From the repo root, format check:

```bash
scripts/fmt.sh --check    # use scripts/fmt.sh (no flag) to apply formatting
```

From `rust/`, lint and test:

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The binary is `openaudit`. The legacy `claw` name is retained as a deprecation shim for one release and emits a stderr warning when invoked. For the full task-oriented build / auth / session / harness guide, see [`USAGE.md`](./USAGE.md).

## Threat model — in scope vs out of scope

**In scope:** static and structural code review. Pattern-based detection. AST-aware queries. Dependency vulnerability lookup. Reasoning over evidence the auditor pulls from the codebase. Optional gated dynamic checks inside a network-isolated sandbox.

**Out of scope:** active exploitation, dynamic fuzzing of running services, network-level attack tooling, supply-chain compromise, detection evasion. OpenAudit is a code auditor; exploitation tools are out of scope and won't be wired in — they would change the project's threat model and legal posture. For dynamic testing, run a separate authorized-target tool. OpenAudit reads code; it does not attack systems.

## Heritage

OpenAudit forks the [`claw-code`](https://github.com/ultraworkers/claw-code) Rust harness — a Claude Code reimplementation by ultraworkers/claw-code. The auditor's tool plumbing, agent loop, terminal renderer, session store, and config system are inherited from that base; OpenAudit adds the dual-agent loop, audit-specific tool surface, playbooks, evidence store, and provider abstraction on top. The project's coordination methodology lives on in [`PHILOSOPHY.md`](./PHILOSOPHY.md); binary parity status against the upstream harness lives in [`PARITY.md`](./PARITY.md).

## License

Apache-2.0 (target).

# OpenAudit — Orientation Report

> Snapshot of the inherited Claude Code architecture as of 2026-05-04.
> This document is the authoritative answer to the six Phase-0 questions
> in the OpenAudit MVP plan. Phase 1+ sub-plans MUST be drafted against
> the file paths recorded here, not against assumptions.

## 0. Surface ownership decision

**Verdict: Rust is canonical.** The user-facing binary, the agent loop, the tool runtime, and the model API client all live in `rust/crates/*`. The Python tree at `src/` is a porting/inventory workspace — not a runtime — and stays in tree as documentation tooling.

**Citations:**

- `rust/Cargo.toml:1-2` — workspace at `rust/` enrolls all production crates: `[workspace] members = ["crates/*"]`.
- `rust/crates/rusty-claude-cli/Cargo.toml:8-10` declares the binary: `[[bin]] name = "claw" path = "src/main.rs"`. This is the inherited CLI we are renaming to `openaudit`.
- `rust/crates/rusty-claude-cli/src/main.rs:26-57` imports the agent-loop building blocks from sibling crates: `api::{ProviderClient, ProviderKind, AnthropicClient, StreamEvent, ...}`, `runtime::{ConversationRuntime, ToolExecutor, McpServerManager, ...}`, `tools::{execute_tool, mvp_tool_specs, GlobalToolRegistry}`, `commands::{...}`, `plugins::{...}`. The agent loop is assembled here from these crates.
- `PARITY.md:6-11` — "Canonical document: this top-level `PARITY.md` is the file consumed by `rust/scripts/run_mock_parity_diff.py`. ... Repository stats at this checkpoint: 292 commits on main / 9 crates, 48,599 tracked Rust LOC, 2,568 test LOC."
- `PARITY.md:59` — "Main-branch reality: `rust/crates/runtime/src/bash.rs` is still the active on-`main` implementation at 283 LOC." Rust is described as the canonical, on-main runtime.
- `PHILOSOPHY.md:7` — "The Python rewrite was a byproduct. The Rust rewrite was also a byproduct." Both are framed as products of a coordination loop, but only Rust is the production binary.
- `src/main.py:22` — argparse description: "Python porting workspace for the Claude Code rewrite effort." The Python CLI subcommands are inventory operations (`manifest`, `summary`, `parity-audit`, `command-graph`, `tool-pool`, `bootstrap-graph`) — not chat/agent commands.

**Disposition of the Python tree:**
- Keep in tree, mark its purpose explicitly. `src/` will not become the OpenAudit runtime.
- During the rebrand (Phase 1) the binary `claw` → `openaudit`. Python `src/main.py` argparse description should be updated to reference OpenAudit instead of Claude Code (cosmetic), but no code there blocks the rename.
- If `src/parity_audit.py` and friends remain useful as a sanity check during the rebrand, leave them. After Phase 1 lands and CI is green, evaluate moving the Python tree to `archive/python-port/` in a follow-up. Out of MVP scope.

**Implication for Phase 1:**
- Phase 1's binary rename touches `rust/crates/rusty-claude-cli/Cargo.toml` (the `[[bin]]` block), the build script `rust/crates/rusty-claude-cli/build.rs`, default config dir (`.claw/` → `.openaudit/`), `~/.claw.json` → `~/.openaudit/config.toml`, and user-facing strings.
- Provider abstraction work happens in `rust/crates/api` (see Section 3) — there is a partial provider abstraction in place. The Phase-1 plan will be smaller in scope than the spec assumed.

## 1. CLI entry point and agent loop

**Binary:** `claw` (declared in `rust/crates/rusty-claude-cli/Cargo.toml:8-10`).

**Entry point:** `rust/crates/rusty-claude-cli/src/main.rs:202` — `fn main()` calls `run()`, which parses argv and dispatches to subcommand handlers. The CLI binary is **monolithic — 13,705 LOC** in this single `main.rs` file. Subcommand dispatch goes to handlers like `handle_repl_command` (main.rs:4638), `handle_session_command` (main.rs:5183), and `handle_plugins_command` (main.rs:5314).

**Call chain to the agent loop:**

```
claw <args>
└── fn main                                           rusty-claude-cli/src/main.rs:202
    └── run()                                          dispatches subcommands
        └── handle_repl_command / print-mode handler   main.rs:4638 (and friends)
            └── ConversationRuntime::new(...)          wires:
                  • impl ApiClient for AnthropicRuntimeClient   main.rs:7824
                      adapter: runtime::ApiClient → api::ProviderClient
                  • impl ToolExecutor for CliToolExecutor       main.rs:9048
                      adapter: runtime::ToolExecutor → tools::execute_tool
                  • Session, PermissionPolicy, system_prompt, hooks
            └── ConversationRuntime::run_turn          runtime/src/conversation.rs:314
                └── loop { ... }                       runtime/src/conversation.rs:342
                      iterations++;
                      let request = ApiRequest { system_prompt, messages };
                      let events = api_client.stream(request)?;
                      for event in events {
                          AssistantEvent::ToolUse { id, name, input }
                            → tool_executor.execute(name, input)
                          AssistantEvent::TextDelta(_) → render
                          AssistantEvent::Usage(_)     → UsageTracker
                          AssistantEvent::MessageStop  → exit inner loop
                      }
                      if no tool calls remain → break;
```

**Key contracts (already in place — important for Phase 1 / Phase 3):**

- `runtime::ApiClient` trait (`runtime/src/conversation.rs:53-55`) is **minimal** and synchronous in shape — `fn stream(&mut self, ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError>`. Returns a fully-collected event vector per call (it is not a streaming iterator at this layer; streaming happens *inside* the api crate, then is materialized here).
- `runtime::ToolExecutor` trait (`runtime/src/conversation.rs:58-60`) — `fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError>`. Stringly-typed input/output by design; tools use JSON-in / text-or-JSON-out.
- `runtime::AssistantEvent` enum (`runtime/src/conversation.rs:30-40`) is the canonical event type the agent loop consumes. Variants: `TextDelta(String)`, `ToolUse { id, name, input }`, `Usage(TokenUsage)`, `PromptCache(PromptCacheEvent)`, `MessageStop`. **The Phase-1 Provider abstraction must preserve this event shape** — the api crate's lower-level `StreamEvent` is mapped into `AssistantEvent` by the CLI's `AnthropicRuntimeClient` adapter at main.rs:7824.

**Implication for OpenAudit:** the agent loop already exists, is tested, and has clean trait seams (`ApiClient`, `ToolExecutor`). Phase 3's dual-agent design can be implemented as **two `ConversationRuntime` instances with different system prompts and (typically) different `ApiClient` impls bound to different providers** — no new agent loop needs to be written. Phase 4's evidence store hooks the same trait boundaries: a wrapping `EvidenceLoggingApiClient` and `EvidenceLoggingToolExecutor` are sufficient to capture every turn and tool call.

## 2. Tool registry

**Source of truth:** `rust/crates/tools/src/lib.rs` — 9,708 LOC. Tool specs are returned by the function `mvp_tool_specs()` at `rust/crates/tools/src/lib.rs:392`. Each spec is a `ToolSpec { name, description, input_schema (serde_json::Value), required_permission: PermissionMode }`. The dispatcher is `pub fn execute_tool(name, input)` at `rust/crates/tools/src/lib.rs:1196`. The `GlobalToolRegistry` (line 109) holds runtime tools, plugin tools, and the permission enforcer.

**Permission classification scheme** (already implemented):
- `PermissionMode::ReadOnly` — safe-by-default tool, no workspace mutation, no shell.
- `PermissionMode::WorkspaceWrite` — writes inside the workspace boundary.
- `PermissionMode::DangerFullAccess` — bash, sub-agent spawning, network, anything not bounded.

This is exactly the classification axis OpenAudit needs. **For audit mode, only `ReadOnly` tools should be visible to the auditor by default.** `WorkspaceWrite` tools are repurposed for `note_append` / `finding_draft` / `finding_finalize` (audit-internal writes). `DangerFullAccess` tools are dropped or replaced with `sandbox_exec` behind the two-key rule.

### Built-in tool inventory (from `mvp_tool_specs()` in `rust/crates/tools/src/lib.rs`)

| # | Tool | Line | Permission | Audit disposition |
|---|---|---|---|---|
| 1 | `bash` | 395 | DangerFullAccess | **drop** in audit mode; replace with `sandbox_exec` (Phase 2) |
| 2 | `read_file` | 416 | ReadOnly | keep |
| 3 | `write_file` | 431 | WorkspaceWrite | drop (or restrict to `.openaudit/` evidence dir) |
| 4 | `edit_file` | 445 | WorkspaceWrite | drop |
| 5 | `glob_search` | 461 | ReadOnly | keep |
| 6 | `grep_search` | 475 | ReadOnly | keep |
| 7 | `WebFetch` | 501 | ReadOnly | network-gated under `--allow-network` |
| 8 | `WebSearch` | 516 | ReadOnly | network-gated under `--allow-network` |
| 9 | `TodoWrite` | 537 | WorkspaceWrite | repurpose as audit-internal note tool |
| 10 | `Skill` | 565 | ReadOnly | keep (playbook checklist surfaces) |
| 11 | `Agent` | 579 | DangerFullAccess | drop in audit mode (auditor doesn't spawn sub-agents directly; reviewer is wired by the runtime, not via tool) |
| 12 | `ToolSearch` | 596 | ReadOnly | keep |
| 13 | `NotebookEdit` | 610 | WorkspaceWrite | drop |
| 14 | `Sleep` | 627 | ReadOnly | keep (rate-limit handling) |
| 15 | `SendUserMessage` | 640 | varies | drop (no Discord routing in audit mode) |
| 16 | `Config` | 661 | varies | drop (audit run is config-frozen) |
| 17 | `EnterPlanMode` / `ExitPlanMode` | 677 / 687 | ReadOnly | keep |
| 18 | `StructuredOutput` | 697 | ReadOnly | keep — basis for `finding_draft` |
| 19 | `REPL` | 706 | DangerFullAccess | drop |
| 20 | `PowerShell` | 721 | DangerFullAccess | drop |
| 21 | `AskUserQuestion` | 737 | ReadOnly | keep (interactive review prompts) |
| 22 | `TaskCreate` / `TaskGet` / `TaskList` / `TaskStop` / `TaskUpdate` / `TaskOutput` | 754 / 800 / 813 / 823 / 836 / 850 | varies | repurpose for hypothesis-board state |
| 23 | `RunTaskPacket` | 768 | DangerFullAccess | drop |
| 24 | `WorkerCreate` … `WorkerObserveCompletion` | 863 … 989 | DangerFullAccess | drop in audit MVP; revisit if planner spawns workers |
| 25 | `TeamCreate` / `TeamDelete` | 1004 / 1028 | DangerFullAccess | drop |
| 26 | `CronCreate` / `CronDelete` / `CronList` | 1041 / 1056 / 1069 | DangerFullAccess | drop |
| 27 | `LSP` | 1079 | ReadOnly | keep — cheap structural lookups |
| 28 | `ListMcpResources` / `ReadMcpResource` / `McpAuth` / `MCP` | 1096 / 1108 / 1122 / 1151 | varies | keep `ReadMcpResource` (read-only); MCP shape is how playbook-supplied tools will register |
| 29 | `RemoteTrigger` | 1135 | DangerFullAccess | drop |
| 30 | `TestingPermission` | 1166 | varies | drop — internal test fixture |

**Tools to add in Phase 2** (none of these exist yet — see plan §Phase 2): `ast_grep`, `tree_sitter_query`, `semgrep_run`, `slither_run`, `osv_scan`, `sandbox_exec`, `note_append`, `finding_draft`, `finding_finalize`.

**File operations** (called from tool wrappers): `runtime/src/file_ops.rs:744` already provides `read_file`, `write_file`, `edit_file`, `glob_search`, `grep_search` with `MAX_READ_SIZE`, `MAX_WRITE_SIZE`, NUL-byte binary detection, and workspace-boundary validation. **Reuse these directly** — they meet OpenAudit's safety bar already.

**Bash sandbox** (relevant to `sandbox_exec`): `runtime/src/sandbox.rs` — Linux `unshare`-based isolation, capability-probed at startup. Container detection in `detect_container_environment`. **Use this for `sandbox_exec`'s implementation; do not write a new sandbox path.**

**MCP tool surface** (relevant to playbook-supplied tools): `runtime/src/mcp_tool_bridge.rs`, `runtime/src/mcp_stdio.rs`, `runtime/src/mcp_lifecycle_hardened.rs`. Playbooks can ship their own static-analysis tool wrappers as MCP servers; the existing MCP plumbing handles discovery, lifecycle, and execution.

## 3. Model provider abstraction

## 4. Conversation / turn state

## 5. System prompt assembly

## 6. Test harness

## 7. Risks and unknowns

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

## 3. Model provider abstraction

## 4. Conversation / turn state

## 5. System prompt assembly

## 6. Test harness

## 7. Risks and unknowns

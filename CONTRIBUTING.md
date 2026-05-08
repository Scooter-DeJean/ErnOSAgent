# Contributing to Ern-OS

> **Read this entire document before opening your first PR.**
> Every rule here exists because violating it caused a production incident.
> If your change conflicts with any rule here, your change is wrong — not the rule.

Thank you for your interest in contributing to Ern-OS. This project is held to an unusually high standard of engineering rigour. This is intentional. Ern-OS is a sovereign AI agent engine — bugs here don't just break a feature, they can cause silent data loss, security breaches, or undetectable behavioural corruption.

This guide will walk you through the complete contribution workflow.

---

## Table of Contents

1. [Before You Start](#before-you-start)
2. [Development Environment](#development-environment)
3. [Understanding the Codebase](#understanding-the-codebase)
4. [The Contribution Workflow](#the-contribution-workflow)
5. [Code Standards](#code-standards)
6. [Testing Requirements](#testing-requirements)
7. [PR Submission Checklist](#pr-submission-checklist)
8. [What Will Get Your PR Rejected](#what-will-get-your-pr-rejected)
9. [For AI Agent Contributors](#for-ai-agent-contributors)
10. [Getting Help](#getting-help)

---

## Before You Start

### Required Reading

Before writing any code, read these documents in order:

1. **This file** — contribution workflow and standards
2. **[`agents and controbutions/rust_code_governance.md`](agents%20and%20controbutions/rust_code_governance.md)** — the full governance framework (§1–§15). Every rule is load-bearing. This is not optional reading.
3. **[`README.md`](README.md)** — architecture overview, quick start, and project philosophy

### Scope Your Contribution

Before writing code, open an issue describing what you intend to do. This prevents:

- Duplicated effort (someone else may already be working on it)
- Wasted work (the maintainer may have a different approach in mind)
- Scope creep (defining the scope up front keeps the PR focused)

**Exception**: Typo fixes, documentation improvements, and single-line bug fixes can go straight to a PR.

---

## Development Environment

### Prerequisites

| Tool | Version | Purpose |
|------|---------|---------|
| Rust | stable (latest) | Language toolchain |
| cargo | ships with Rust | Build, test, dependency management |
| git | 2.30+ | Version control |
| A local LLM server | Any OpenAI-compatible | Required for E2E testing |

### Setup

```bash
# Clone the repository
git clone https://github.com/mettamazza/Ern-OS.git
cd Ern-OS

# Build (should complete with zero warnings)
cargo build --release

# Run the full test suite
cargo test --release

# Verify zero warnings
cargo build --release 2>&1 | grep "warning" && echo "FAIL: warnings present" || echo "PASS: zero warnings"
```

### Environment Configuration

```bash
# Copy the example config
cp .env.example .env
# Edit .env with your local provider details (never commit .env)
```

The `data/` directory is created at runtime. Never commit anything from `data/`.

---

## Understanding the Codebase

### Architecture

```
src/
├── config/           # Configuration loading and validation
├── inference/        # Stream consumption, context assembly
├── interpretability/ # SAE training and feature analysis
├── memory/           # 5-tier memory persistence
├── observer/         # Response audit pipeline
├── platform/         # Discord, Telegram adapters (WebUI clients)
├── provider/         # LLM provider trait + implementations
├── tools/            # Tool definitions and execution
├── checkpoint/       # State snapshot and recovery
└── web/              # WebUI server, handlers, WebSocket
```

### Key Design Principles

1. **WebUI is the hub** — all platforms connect through the WebUI API (§6)
2. **Model-neutral** — no model-specific code paths (§7.1)
3. **Provider-neutral** — the `Provider` trait is the universal interface (§7.2)
4. **Everything auto-derived** — context length, capabilities, parameters come from the model/provider, never hardcoded (§2.1)
5. **Fail loud, not silent** — errors are logged and surfaced, never swallowed (§2.4, §2.6)

---

## The Contribution Workflow

### Step 1: Create a Branch

```bash
git checkout -b fix/short-description   # for bug fixes
git checkout -b feat/short-description  # for features
git checkout -b refactor/short-description  # for refactors
```

**One concern per branch.** A fix branch contains only the fix. A feature branch contains only the feature. Never mix concerns (§8.7, §10.2).

### Step 2: Make Your Changes

Follow the [Code Standards](#code-standards) below for every file you touch.

### Step 3: Test Thoroughly

Every change must pass all three testing tiers:

```bash
# 1. Unit tests — must pass before anything else
cargo test --release --lib

# 2. Full test suite including integration tests
cargo test --release

# 3. Build must produce ZERO warnings
cargo build --release 2>&1 | grep "warning" && echo "FAIL" || echo "PASS"
```

See [Testing Requirements](#testing-requirements) for the complete testing mandate.

### Step 4: Manual Verification

Before submitting, **manually verify your change works end-to-end**:

1. Start the engine: `cargo run --release`
2. Open the WebUI in your browser
3. Exercise the feature/fix you changed through the actual UI
4. For platform adapter changes: test through the actual platform (Discord/Telegram)
5. For tool changes: invoke the tool through a conversation and verify the output
6. For API changes: test with `curl` or your preferred HTTP client

> **If you cannot manually test it, say so in the PR description.** Do not claim manual testing you did not perform. The maintainer will verify.

### Step 5: Commit

```bash
git add -A
git commit -m "fix: brief description of what and why

Detailed explanation of the root cause and how this fix addresses it.
Reference the specific governance section if applicable."
```

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):

| Prefix | Usage |
|--------|-------|
| `fix:` | Bug fix |
| `feat:` | New feature |
| `refactor:` | Code restructuring (no behaviour change) |
| `chore:` | Build, CI, dependency, governance changes |
| `test:` | Adding or fixing tests |
| `docs:` | Documentation only |

### Step 6: Submit a Pull Request

Open a PR against `main` using the [PR template](.github/PULL_REQUEST_TEMPLATE.md). Fill in every section — incomplete PRs will be returned without review.

---

## Code Standards

These are enforced on every file touch. They come directly from the [governance framework](agents%20and%20controbutions/rust_code_governance.md).

### File Structure

| Metric | Limit | Reference |
|--------|-------|-----------|
| File length | ≤500 lines (excluding tests) | §1.1 |
| Function length | ≤50 lines | §1.2 |
| Methods per `impl` block | ≤15 | §1.3 |
| Doc comment on every module | Required (`//!` at top) | §1.4 |

### Code Quality

- **No `unwrap()` on `Result` or `Option`** outside of tests — use `anyhow::Result` with `.context()` (R3)
- **No `todo!()`, `unimplemented!()`, or empty function bodies** — if you can't implement it fully, stop and say so (§2.3)
- **No magic numbers** — derive from model/provider/runtime data. Named constants for anything else (§8.3)
- **No silent failures** — every error path logs with `tracing::error!` or `tracing::warn!` and includes context (§2.6)
- **No hardcoded model parameters** — context length, temperature, etc. are auto-derived (§2.1)
- **No `unsafe` blocks** — banned by default. Human-only, with project owner approval (§13.8)

### Error Handling

```rust
// ✅ Correct: context-rich error handling
let config = std::fs::read_to_string(&path)
    .context(format!("Failed to read config from {}", path.display()))?;

// ❌ Wrong: bare unwrap
let config = std::fs::read_to_string(&path).unwrap();

// ❌ Wrong: silent failure
let config = std::fs::read_to_string(&path).unwrap_or_default();
```

### Logging

Use `tracing` with structured fields, not string interpolation:

```rust
// ✅ Correct
tracing::info!(session = %session_id, tool = %name, "Tool executed successfully");

// ❌ Wrong
tracing::info!("Tool {} executed for session {}", name, session_id);
```

---

## Testing Requirements

### The Three Tiers

Every PR must satisfy all three tiers:

#### Tier 1: Automated Tests (Mandatory — CI Enforced)

```bash
cargo test --release        # ALL tests must pass
cargo build --release       # ZERO warnings
```

- Unit tests for every public function you add or modify
- Tests for both **success paths AND error paths**
- Test names describe the scenario: `test_ingest_turn_rejects_empty_input`, not `test_1`
- No test stubs — every test asserts something meaningful (§8.5)

#### Tier 2: End-to-End Tests (Mandatory for Feature/Fix PRs)

- If your change affects inference: test a full conversation through the WebUI
- If your change affects tools: invoke the tool through a real conversation
- If your change affects streaming: verify the SSE/WebSocket stream delivers correctly
- If your change affects a platform adapter: test through the actual platform

#### Tier 3: Manual Verification (Mandatory — Self-Certified in PR)

You must personally verify that your change works as expected in a running instance. In your PR, describe:

1. **What you tested** — the specific scenario
2. **How you tested it** — the steps you followed
3. **What you observed** — the actual result
4. **Expected vs actual** — confirmation they match

> **Honesty is non-negotiable.** If you could not perform manual testing (e.g., you don't have a Discord bot token), say so explicitly. Do not fabricate test results.

### Writing Good Tests

```rust
// ✅ Good: tests a behavioural contract
#[test]
fn test_containment_blocks_path_traversal() {
    assert!(check_path("../../etc/passwd").is_some(),
        "Path traversal must be blocked");
}

// ❌ Bad: test theatre — tests nothing meaningful
#[test]
fn test_provider_exists() {
    let provider = OllamaProvider::new(&config);
    assert_eq!(provider.id(), "ollama"); // This can never fail
}
```

The test for every function should answer: **"If I broke the implementation, would this test catch it?"** If no, the test is theatre (§8.5).

---

## PR Submission Checklist

Copy this into your PR description and check every box:

```markdown
### Pre-Submission Checklist

- [ ] I have read `agents and controbutions/rust_code_governance.md` in full
- [ ] My change addresses ONE concern (bug fix OR feature OR refactor)
- [ ] `cargo test --release` passes with zero failures
- [ ] `cargo build --release` produces zero warnings
- [ ] Every new public function has at least one unit test
- [ ] Tests cover both success and error paths
- [ ] No `unwrap()` on `Result`/`Option` outside of tests
- [ ] No `todo!()`, `unimplemented!()`, or empty function bodies
- [ ] No hardcoded model parameters or magic numbers
- [ ] No silent failures — all error paths log with context
- [ ] All new modules have `//!` doc comments
- [ ] No file exceeds 500 lines (excluding tests)
- [ ] No function exceeds 50 lines
- [ ] Commit messages follow conventional commits format
- [ ] I have manually tested this change in a running instance
- [ ] Manual test results are documented in this PR

### Manual Testing Evidence

**What I tested:**
<!-- Describe the specific scenario -->

**How I tested it:**
<!-- Describe the steps -->

**What I observed:**
<!-- Describe the actual result -->

**Expected vs actual:**
<!-- Confirm they match, or explain discrepancies -->
```

---

## What Will Get Your PR Rejected

These are **immediate rejection triggers**. No discussion, no exceptions. Full list in [governance §11](agents%20and%20controbutions/rust_code_governance.md#11-review-rejection-criteria).

| # | Rejection Trigger |
|---|---|
| R1 | Hardcoded model parameter (context length, temperature, token limit) |
| R2 | `todo!()`, `unimplemented!()`, `// TODO`, or empty function body |
| R3 | `unwrap()` on a `Result` or `Option` outside of tests |
| R4 | Silent fallback that masks a failure |
| R5 | Heuristic where the measurement API exists |
| R6 | Test that doesn't assert a behavioural contract |
| R7 | Multiple unrelated concerns in a single PR |
| R8 | Model-specific code path (`if model.contains("gemma")`) |
| R9 | Provider-specific logic outside `src/provider/<name>.rs` |
| R10 | Missing `//!` doc comment on a new module |
| R11 | Function exceeding 50 lines |
| R12 | Lifecycle invariant exposed as a toggleable config option |
| R13 | Hardcoded value that should be derived from model/provider/runtime |
| R14 | `catch_unwind` without the underlying panic fixed in the same PR |
| R15 | Data file change without commit message explaining the impact |
| R16 | Secret, API key, or credential in source code |
| R17 | Network listener without authentication or access control |
| R18 | Deserialization of untrusted input without size limits |
| R19 | Shell command from user input without sanitisation |
| R20 | Any `unsafe` block without explicit human justification + owner approval |

### Additional Rejection Reasons

- **Fabricated test results** — claiming manual testing that was not performed
- **Missing test evidence** — no manual testing section in the PR
- **Incomplete checklist** — unchecked items without explanation
- **Build warnings** — the build must be clean
- **Test failures** — all tests must pass, no exceptions

---

## For AI Agent Contributors

AI agents (including Ern-OS itself when self-modifying) are subject to all rules above, plus these additional constraints from [governance §15](agents%20and%20controbutions/rust_code_governance.md#15-ai-agent-contributor-safety):

1. **Minimal authority** — request the minimum permissions needed. A bug fix does not restructure the module (§15.1)
2. **No `unsafe`** — absolute prohibition, zero exceptions (§13.8, R20)
3. **No governance weakening** — changes to governance files require human approval. An AI cannot weaken its own constraints (§15.2)
4. **No silent tool execution** — every tool invocation must be logged (§15.3)
5. **No scope creep** — do exactly what was asked, nothing more (§8.7)
6. **No heuristic smuggling** — if the measurement API exists, use it. Comments do not make heuristics compliant (§8.3)
7. **No reward hacking** — every fix addresses the root cause. No wallpapering (§8.1)

### Common AI Pitfalls

These are patterns that AI agents have historically introduced into this codebase:

| Pitfall | What It Looks Like | Why It's Rejected |
|---------|-------------------|-------------------|
| Comment-as-compliance | `// HEURISTIC: chars/3 approximation` | A comment does not make a heuristic acceptable (§8.3) |
| Follow-up PR promise | "Will fix the underlying panic in a follow-up" | The fix ships together or not at all (§8.1) |
| Estimation over measurement | `budget_tokens * 4` for chars | The server has a tokenizer — use `count_tokens()` (§8.3) |
| Conditional governance | "Acceptable if you add a justification comment" | Governance rules are absolute, not conditional (V6) |
| Complexity injection | `RetryPolicy` trait for a 3-line retry loop | Simplest correct implementation wins (§8.2) |

---

## Getting Help

- **Questions about governance**: Open a [Discussion](https://github.com/mettamazza/Ern-OS/discussions) with the `governance` label
- **Questions about architecture**: Read the [architecture docs](docs/) first, then ask in Discussions
- **Bug reports**: Use the [Bug Report issue template](.github/ISSUE_TEMPLATE/bug_report.md)
- **Feature requests**: Use the [Feature Request issue template](.github/ISSUE_TEMPLATE/feature_request.md)

---

## License

Ern-OS is licensed under the [MIT License](LICENSE). By contributing, you agree that your contributions will be licensed under the same terms.

---

*Scientific rigour, not shortcuts.*

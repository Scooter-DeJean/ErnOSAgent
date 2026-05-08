## Description

<!-- What does this PR do? Link the related issue if applicable. -->

Fixes #

## Type of Change

- [ ] 🐛 Bug fix (non-breaking change that fixes an issue)
- [ ] ✨ New feature (non-breaking change that adds functionality)
- [ ] ♻️ Refactor (no behaviour change)
- [ ] 📖 Documentation
- [ ] 🧪 Test improvement
- [ ] 🔧 Chore (build, CI, dependency, governance)

## Root Cause Analysis (Bug Fixes Only)

<!-- 
For bug fixes, you MUST identify the root cause (§10.1).
Do NOT list possible causes — identify THE cause.
-->

**Specific error:** <!-- paste the log line or error message -->
**Traced to:** <!-- file:line where the failure originates -->
**Root cause:** <!-- one sentence explaining WHY it fails -->
**Fix:** <!-- one sentence explaining HOW this PR addresses the root cause -->

---

## Governance Compliance Checklist

### Code Quality
- [ ] I have read [`agents and controbutions/rust_code_governance.md`](agents%20and%20controbutions/rust_code_governance.md) in full
- [ ] My change addresses **ONE** concern (fix OR feature OR refactor — §10.2)
- [ ] No `unwrap()` on `Result`/`Option` outside of tests (R3)
- [ ] No `todo!()`, `unimplemented!()`, or empty function bodies (R2)
- [ ] No hardcoded model parameters or magic numbers (R1, R5, R13)
- [ ] No silent failures — all error paths log with context (§2.6, R4)
- [ ] No `unsafe` blocks (R20, §13.8)

### Structure
- [ ] All new modules have `//!` doc comments (R10)
- [ ] No file exceeds 500 lines excluding tests (§1.1)
- [ ] No function exceeds 50 lines (R11)
- [ ] No `impl` block exceeds 15 methods (§1.3)

### Testing
- [ ] `cargo test --release` — **zero failures**
- [ ] `cargo build --release` — **zero warnings**
- [ ] Every new public function has at least one unit test
- [ ] Tests cover both success AND error paths
- [ ] No test stubs — every test asserts a behavioural contract (R6)
- [ ] End-to-end testing completed (see below)
- [ ] Manual testing completed (see below)

---

## Test Results

### Automated Tests

```
cargo test --release
# Paste output summary here (e.g., "test result: ok. 669 passed; 0 failed")
```

```
cargo build --release
# Confirm: "0 warnings" or paste warning output
```

### End-to-End Testing

<!-- 
Describe the E2E test you performed:
- For inference changes: full conversation through WebUI
- For tool changes: tool invocation through a real conversation  
- For streaming changes: SSE/WebSocket delivery verification
- For platform adapter changes: test through actual platform
-->

**Scenario tested:**

**Steps performed:**

**Result:**

### Manual Verification

<!-- 
This section is MANDATORY. If you cannot perform manual testing,
explain why. Do NOT fabricate results.
-->

**What I tested:**

**How I tested it:**

**What I observed:**

**Expected vs actual:** <!-- Confirm match or explain discrepancy -->

---

## Screenshots / Logs (if applicable)

<!-- Paste relevant log output, screenshots, or recordings -->

---

## Breaking Changes

- [ ] This PR introduces no breaking changes
- [ ] This PR introduces breaking changes (describe below)

<!-- If breaking: what breaks, who is affected, and what they need to do -->

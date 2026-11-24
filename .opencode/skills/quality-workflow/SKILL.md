---
name: quality-workflow
description: Mandatory quality workflow for OpenScrcpy changes and reviews. Use before editing code, before committing, when reviewing a diff or pull request, or when asked to audit quality, security, accessibility, tests, or performance. Routes to the project skills in .agents/skills, enforces the repository gates, and defines the review report format.
---

# OpenScrcpy Quality Workflow

This is the project's mandatory workflow for any code change, review, or commit. Follow the
phases in order. If a phase does not apply, say why in the final report instead of skipping it
silently.

## Phase 0 — Frame the work

1. Read `AGENTS.md` and `CONTRIBUTING.md`. They override generic advice.
2. Establish the scope:
   - Change: `git status --short` and `git diff` (staged + unstaged).
   - Review: use the range the user gave; otherwise `git diff $(git merge-base HEAD main)...HEAD`.
   - If the diff is empty or the ref does not resolve, stop and ask.
3. Classify the change and load only the matching skills from the routing table below.
4. Identify the risk surface before editing:
   - Does it change an IPC command, a Tauri capability, or the CSP?
   - Does it add a value that reaches `adb`/`scrcpy`/a shell/process argument?
   - Does it touch a download, archive extraction, updater, or file path?
   - Does it change user-visible text, layout, keyboard flow, or RTL layout?
   - Does it touch a platform-specific module (`kwin.rs`, `grab.rs`, `shortcuts.rs`)?
   - Does it change `tauri.conf.json` or a `tauri.<platform>.conf.json`?
   - Does it add/update a dependency or a GitHub Action?

## Phase 1 — Plan and tests first

- Write a short plan (3–6 bullets): behavior change, files, tests, docs, platform impact.
- For a behavior change or bug fix, define the failing test **before** the implementation:
  - Frontend: Vitest + Testing Library; use the existing `src/**/__tests__/` pattern.
  - Backend: Rust `#[cfg(test)]` module next to the code; test validation, errors, and edge cases.
  - IPC: register the command in `generate_handler!` and keep the frontend `invoke()` contract in sync.
  - i18n: add the key to **all eight** locale files; the parity test must pass.
- If the change is a pure refactor, state the behavior-preservation evidence (existing tests,
  characterization test, or a before/after check).

## Phase 2 — Skill routing

Load only the skills that fit the change. Do not load the whole catalog.

| Work type | Load these skills |
| --- | --- |
| Rust backend / process / download | `rust-best-practices`, `tauri-v2`, `security-review`; use `rust-review` and `rust-testing` when available |
| Svelte / TypeScript / UI | `svelte-core-bestpractices`, `accessibility`, `performance-optimization`; `ux-heuristics` when evaluating UX |
| Tauri IPC / capabilities / CSP / config | `tauri-v2`, `security-review`; `tauri-ipc` when available |
| New feature or bug fix | `test-driven-development`, plus the stack-specific skill above |
| Refactor / cleanup | `code-simplification`, `clean-code`, `refactoring-patterns`, `clean-architecture` |
| Debugging a failure | `systematic-debugging`; do not propose a fix before the root cause is named |
| Code review / PR review | `code-review-and-quality`, `code-review` (when a fixed point and spec exist), `security-review`, `accessibility` |
| Docs / ADR / release notes | `documentation-and-adrs` |
| Commit / PR / changelog | `git-workflow-and-versioning` |
| Agent instructions | `agents-md` |

Project-installed skills live in `.agents/skills/`. The environment may also expose global
skills such as `clean-code`, `clean-architecture`, `refactoring-patterns`, `rust-review`,
`rust-testing`, `tauri-ipc`, `ux-heuristics`, and `dead-code`; use them when present.

## Phase 3 — Implement with the repository's guardrails

- **Rust:** prefer borrowed parameters (`&str`, `&[T]`), explicit `Result` errors, no panics in
  command paths, and no `unwrap()` on untrusted input. Run `cargo fmt` and `cargo clippy`.
- **Svelte 5:** use runes (`$state`, `$derived`, `$effect`, `$state.raw` for large reassigned
  data). Keep the store in `src/lib/`; do not duplicate state in components.
- **Accessibility:** every interactive control needs an accessible name, role, keyboard path,
  visible focus, and correct disabled/expanded state. Dialogs need focus management and Escape.
- **i18n:** no hardcoded user-facing English in `.svelte` or `.ts`; add all locale keys and check
  RTL (Arabic) layout when the change is directional.
- **IPC:** register new commands in `src-tauri/src/lib.rs`; keep argument names/types aligned with
  the frontend; return structured errors, not raw panics.
- **Validation:** every value that becomes a process argument goes through
  `crates/open-scrcpy-core/src/validate.rs`; add a unit test for the accepted and rejected cases.
- **Downloads/files:** never trust a URL, archive entry, or path; keep SHA-256 verification,
  reject path traversal, and avoid shell interpolation.
- **Tauri config:** capabilities are least-privilege; platform configs are JSON Merge Patch
  (arrays are replaced, not merged). Keep `tauri.conf.json` and `tauri.<platform>.conf.json` in sync.
- **Dependencies:** justify new packages; check license, maintenance, and `npm audit`/`cargo audit`
  impact. Pin GitHub Actions by full SHA, matching the existing workflows.

## Phase 4 — Multi-pass self-review

Review the final diff in this order. Do not mix passes; finish one before starting the next.

### 4.1 Spec and correctness
- Does the change do what the request/spec says, including edge cases and error paths?
- Are null/empty/boundary/offline/permission-denied cases handled?
- Are concurrent starts/stops, retries, and cancellation safe (see the store's guards)?
- Does the change alter behavior outside its scope?

### 4.2 Readability and simplicity
- Are names honest and consistent with the surrounding code?
- Is nesting or branching reducible? Is an abstraction earning its complexity?
- Are there dead artifacts, commented-out code, or "temporary" branches?
- Would a new maintainer understand the change without the author explaining it?

### 4.3 Architecture and dependencies
- Does the change follow existing module boundaries and dependency direction?
- Is logic in the right layer (UI vs store vs Tauri command vs Rust core)?
- Is duplication real, or a premature abstraction? Prefer deleting an abstraction to polishing it.
- Does a new dependency or global state make the system harder to test?

### 4.4 Security and privacy
- Trace every untrusted input: webview, localStorage, IPC payload, device serial, URL, file path,
  archive entry, environment variable.
- Confirm validation in `validate.rs` and least-privilege capabilities/CSP.
- Look for command injection, path traversal, symlink/zip-slip, unsafe deserialization, secret
  leakage, and unsafe logging.
- Check the updater and download paths: signature/SHA-256 verification, HTTPS, redirects, TOCTOU.
- Report only high-confidence, exploitable findings; mark uncertain items as "needs verification".

### 4.5 Performance
- Measure before optimizing. Look for unbounded loops/log growth, repeated work in reactivity,
  large deeply reactive objects, unnecessary clones/allocations, and blocking work on the UI thread.
- For Rust, check allocations, blocking calls inside async, and lock scope.
- For Svelte, check `$effect` dependencies, derived values, and list keying.

### 4.6 Accessibility and i18n
- Run the axe-based component tests; add a test for new interactive UI.
- Verify keyboard-only operation, focus order, labels, roles, contrast, and reduced-motion behavior.
- Verify all locale keys exist and the parity test passes; check text expansion and RTL.

### 4.7 Tests
- Does a regression test fail before the fix and pass after it?
- Are error paths and boundary values covered, not just the happy path?
- Are tests deterministic (no real network, clock, or device dependencies)?
- Is the IPC contract still aligned? Is the i18n parity test green?

### 4.8 Documentation
- Update `README.md`, `GUIDE.md`, `docs/`, or `docs/RELEASE_NOTES.md` when behavior, setup, or
  user-facing workflows change.
- Record a significant architecture decision as an ADR (see `documentation-and-adrs`).
- Keep examples and commands executable; do not document unverified flags.

## Phase 5 — Run the gates

Run these from the repository root. Record the result of each one.

```bash
npm run lint
npm test
(cd src-tauri && cargo fmt --check)
(cd src-tauri && cargo clippy --all-targets -- -D warnings)
(cd src-tauri && cargo test)
npm run tauri build -- --no-bundle
```

CI parity / supply chain, when the tools are available:

```bash
npm audit --omit=dev --audit-level=high
cargo audit --file src-tauri/Cargo.lock
git diff --check
```

If a gate cannot run (missing system library, no device, signing key absent), say exactly which
one and why. Do not mark it passed. For a local unsigned build, the documented alternative is:

```bash
npm run tauri build -- --config src-tauri/tauri.local.conf.json
```

## Phase 6 — Report

Use this format for reviews and for the final summary of a change:

```text
Verdict: APPROVE | REQUEST CHANGES | BLOCKED
Scope: <diff range / files>   Platform: <tested / not tested>
Gates: <command — pass/fail/not run (reason)>

Findings (ordered by severity):
[BLOCKER] path:line — problem. Why it matters. Concrete fix.
[MAJOR]   path:line — problem. Why it matters. Concrete fix.
[MINOR]   path:line — problem. Suggested fix.
[NIT]     path:line — optional preference.

Checked and clean: <axes actually verified>
Open questions: <only real ambiguities>
```

Severity rules:

- **BLOCKER** — correctness, security, data loss, or a red required gate; must fix before merge.
- **MAJOR** — likely bug, regression, missing test for changed behavior, or a design problem that
  will be expensive to undo; fix before merge unless explicitly deferred.
- **MINOR** — worthwhile improvement that does not block merge.
- **NIT** — style/preference; never block on it, and do not report what the formatter/linter enforces.

Every finding must cite `path:line`, explain the impact, and propose a fix. Separate pre-existing
issues from issues introduced by this diff. If there are no findings, list what you checked.

## Phase 7 — Commit and pull request

- One logical change per commit; imperative, scoped subject, e.g.
  `fix(mirror): keep the window on screen after a monitor change`.
- Do not commit generated output, binaries, `dist/`, `node_modules/`, `artifacts/`, or secrets.
- Complete `.github/pull_request_template.md`; list the commands run, platforms/devices tested,
  and attach a screenshot or recording for visible UI changes.
- Update i18n, docs, and tests in the same change. A green CI run is required before merge.

## Stop conditions

Stop and ask, or mark the work blocked, when:

- The request is ambiguous about behavior, platform, or scope.
- A required gate fails for a reason unrelated to the change (say which and provide the output).
- The diff mixes unrelated changes; propose splitting it.
- A security or data-loss risk cannot be validated with the available evidence.
- The fix would require weakening CSP, capabilities, validation, or tests. Never do that to get green.

## Definition of done

- [ ] Behavior is covered by a test that failed before the change (or a documented reason why not).
- [ ] `npm run lint`, `npm test`, Rust fmt/clippy/test pass; build checked or reason recorded.
- [ ] No hardcoded UI strings; all locales updated; parity test green.
- [ ] New IPC commands registered and used; `validate.rs` covers new process arguments.
- [ ] Capabilities/CSP unchanged or justified; no secrets, binaries, or generated files.
- [ ] Accessibility checked (axe + keyboard) for UI changes.
- [ ] Docs/release notes updated when behavior or setup changed.
- [ ] Review report has no unresolved BLOCKER/MAJOR findings.

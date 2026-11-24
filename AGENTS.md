# AGENTS.md — OpenScrcpy

Instructions for coding agents. Humans should read [CONTRIBUTING.md](CONTRIBUTING.md).

## Stack

- Tauri 2 desktop app: shell in `src-tauri/`, UI-agnostic domain in `crates/open-scrcpy-core/`
  (no Tauri/GPUI dependency), native bridges in `crates/open-scrcpy-platform/`.
- Svelte 5 + strict TypeScript frontend in `src/`.
- Tailwind CSS 4, Vite, Vitest + Testing Library, `axe-core` accessibility tests.
- Eight UI locales in `src/i18n/locales/`; the parity test is a merge gate.
- Core process is `scrcpy`/`adb`; the webview is **not** a trust boundary.

## Mandatory quality workflow

Before changing code, reviewing a diff/PR, or committing, load the project skill
`quality-workflow` (`.opencode/skills/quality-workflow/SKILL.md`). It routes work to the
project skills in `.agents/skills/` and defines the required review report.

Non-negotiables:

- User-facing text belongs in `src/i18n/locales/*.ts`; update **every** locale.
- Register every new Tauri command in `generate_handler!` in `src-tauri/src/lib.rs` and call
  it through `invoke()` from the frontend. The IPC contract test must stay green.
- Validate every value that can reach a process argument in `crates/open-scrcpy-core/src/validate.rs`.
  Treat persisted state and IPC payloads as untrusted.
- Tauri merges `tauri.<platform>.conf.json` with JSON Merge Patch (RFC 7386): arrays are
  replaced, not merged. Keep platform window/config objects in sync.
- Use Svelte 5 runes (`$state`, `$derived`, `$effect`); do not introduce legacy stores.
- Keep controls accessible: labels, roles, keyboard operation, focus management.
- Do not weaken the CSP or add Tauri capabilities/permissions without a threat-model reason.
- Never commit downloaded binaries, `dist/`, `node_modules/`, `artifacts/`, signing keys, or secrets.
- Do not silence checks (`@ts-ignore`, `clippy::allow`, skipped/deleted tests) to reach green.

## Gates (same as CI)

| Check | Command |
| --- | --- |
| TypeScript + Svelte | `npm run lint` |
| Frontend tests | `npm test` |
| Rust format | `(cd src-tauri && cargo fmt --check)` · `(cd crates/open-scrcpy-core && cargo fmt --check)` · `(cd crates/open-scrcpy-platform && cargo fmt --check)` |
| Rust lints | `(cd src-tauri && cargo clippy --all-targets -- -D warnings)` · same for both crates |
| Rust tests | `(cd src-tauri && cargo test)` · same for both crates |
| Build | `npm run tauri build -- --no-bundle` |
| npm audit (CI) | `npm audit --omit=dev --audit-level=high` |
| Cargo audit (CI, needs `cargo-audit`) | `cargo audit --file src-tauri/Cargo.lock` |

If a gate cannot run locally, say which one and why. Never claim a check passed without running it.

## Commits and pull requests

- Use imperative, scoped subjects: `fix(mirror): keep the window on screen after a monitor change`.
- Keep commits focused; one logical change per commit.
- Complete `.github/pull_request_template.md`, add tests, and update `README.md`, `GUIDE.md`,
  or `docs/` when behavior or setup changes.
- Attach a screenshot or recording for visible UI changes and list the platforms tested.

## Project skills

`.agents/skills/` and `skills-lock.json` are managed by the `skills` CLI. Update them with
`npx skills update -p` (project scope only); do not hand-edit installed skills. The hand-written
workflow lives in `.opencode/skills/quality-workflow/`.

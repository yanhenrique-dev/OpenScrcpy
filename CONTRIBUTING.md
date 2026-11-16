# Contributing to OpenScrcpy

Thanks for taking the time to contribute. This document covers the local setup,
the checks a pull request must pass, and the conventions used in the codebase.

By participating you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).
Security issues must be reported privately as described in
[SECURITY.md](SECURITY.md), not in a public issue.

## Development setup

### Prerequisites

- Node.js 22.12+ (Node 24 is what CI uses; `.nvmrc` pins it)
- A stable Rust toolchain with `cargo`, `rustfmt` and `clippy`
- The [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS

On Debian/Ubuntu the Linux packages are:

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev \
  libjavascriptcoregtk-4.1-dev libsoup-3.0-dev
```

### Run it

```bash
npm ci
npm run tauri dev
```

The frontend also runs in a plain browser with `npm run dev`, but the Tauri
commands, device list and updater are unavailable there.

## Before opening a pull request

Run the same gates as CI:

```bash
npm run lint
npm test
(cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test)
npm run tauri build -- --no-bundle
```

`npm run tauri build` (without `--no-bundle`) signs updater artifacts and needs
`TAURI_SIGNING_PRIVATE_KEY`; use the `--no-bundle` form for a local check.

### What each check covers

| Check | Command | Notes |
| --- | --- | --- |
| TypeScript + Svelte | `npm run lint` | `tsc --noEmit` and `svelte-check` |
| Frontend tests | `npm test` | Vitest + Testing Library |
| Rust formatting | `cargo fmt --check` | run inside `src-tauri/` |
| Rust lints | `cargo clippy --all-targets -- -D warnings` | warnings are errors |
| Rust tests | `cargo test` | run inside `src-tauri/` |
| IPC contract | `npm test` | every `invoke()` must be registered and every registered command used |

## Code conventions

- The frontend uses Svelte 5 runes (`$state`, `$derived`, `$effect`) and strict
  TypeScript. Keep new components accessible (labels, roles, keyboard support);
  the accessibility tests render the main panels with axe.
- User-facing strings belong in `src/i18n/locales/*.ts`. Add the key to **every**
  locale; `src/i18n/locales.parity.test.ts` fails otherwise. Do not hardcode UI
  text in English outside the locale files.
- Rust commands live in `src-tauri/src/`. Register a new command in the
  `generate_handler!` list in `lib.rs` and call it through `invoke()` from the
  frontend, otherwise the IPC contract test fails.
- Validate values that become process arguments in `src-tauri/src/validate.rs`.
  The webview is not a trust boundary: treat persisted state and IPC payloads as
  untrusted.
- Tauri merges `tauri.<platform>.conf.json` with JSON Merge Patch (RFC 7386),
  so arrays are **replaced**, not merged. `tauri.linux.conf.json` therefore
  repeats the full `app.windows` object when it overrides `decorations`; keep
  the two files in sync when the main window changes.
- Do not commit downloaded binaries, `dist/`, `node_modules/`, `artifacts/` or
  signing keys. `.gitignore` covers the generated paths.
- Keep commits focused and use an imperative, scoped subject, e.g.
  `fix(mirror): keep the window on screen after a monitor change`.

## Pull requests

1. Open an issue first for a non-trivial change so the approach can be agreed on.
2. Branch from `main`, keep the diff scoped, and add or update tests.
3. Include a screenshot or recording for visible UI changes, and say which
   platforms you tested.
4. Update `README.md`, `GUIDE.md` or `docs/` when behaviour or setup changes.
5. Make sure the checklist in the pull request template is complete.

Maintainers may ask for changes; a green CI run is required before merge.

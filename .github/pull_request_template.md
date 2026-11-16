## Summary

<!-- What does this change and why? Link the issue it closes. -->

Closes #

## Type of change

- [ ] Bug fix
- [ ] New feature
- [ ] Refactor / internal cleanup
- [ ] Documentation
- [ ] Build, CI or release

## How was it tested?

<!-- Commands run and platforms/device models used. -->

- [ ] `npm run lint`
- [ ] `npm test`
- [ ] `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
- [ ] `npm run tauri build -- --no-bundle`
- [ ] Tested manually on: <!-- Linux / Windows / macOS + device model -->

## Checklist

- [ ] User-facing strings use the i18n locale files and every locale is updated
- [ ] New Tauri commands are registered in `generate_handler!` and covered by the IPC contract test
- [ ] Inputs that reach a process argument are validated in `src-tauri/src/validate.rs`
- [ ] Tests were added or updated for the changed behaviour
- [ ] Documentation (`README.md`, `GUIDE.md`, `docs/`) reflects the change
- [ ] No secrets, binaries or generated files are included
- [ ] Screenshots or a recording are attached for visible UI changes

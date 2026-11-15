# Releasing OpenScrcpy

The normal release path is tag-driven: pushing a `vX.Y.Z` tag runs
`.github/workflows/release.yml`, which builds every platform, signs the
updater artifacts, uploads them to the GitHub release and publishes
`latest.json`.

## Automated bump (optional)

The `Bump Version` workflow and the `Tag on Version Bump Merge` workflow are
enabled only when the repository has a `RELEASE_TOKEN` secret. Create a
fine-grained personal access token with:

- `contents: write` — push the bump branch, create the tag and edit releases;
- `workflow` — only needed if tags created with the token must trigger other
  workflows;
- `pull requests: write` — open the bump PR.

Save it as `RELEASE_TOKEN` under **Settings > Secrets and variables > Actions**.
Without it, both workflows stop immediately with an explanatory error; use the
manual path below.

## Manual release

1. Bump the same version in all four files (the CI test
   `scripts/__tests__/version-consistency.test.ts` enforces this):

   ```bash
   npm version patch --no-git-tag-version   # or minor/major
   VERSION=$(node -p "require('./package.json').version")
   jq ".version = \"$VERSION\"" src-tauri/tauri.conf.json > /tmp/tauri.conf.json \
     && mv /tmp/tauri.conf.json src-tauri/tauri.conf.json
   sed -i "s/^version = \".*\"/version = \"$VERSION\"/" src-tauri/Cargo.toml
   sed -i "/^name = \"openscrcpy\"$/ { n; s/^version = \".*\"/version = \"$VERSION\"/ }" src-tauri/Cargo.lock
   ```

2. Run the gates locally:

   ```bash
   npm run lint
   npm test
   (cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test)
   npm run tauri build -- --no-bundle
   ```

3. Commit the bump and push it to `main`:

   ```bash
   git add package.json package-lock.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
   git commit -m "chore: bump version to v$VERSION"
   git push origin main
   ```

4. Create and push the tag. The tag must point at the commit that passed the
   gates; do not move or force-push an existing release tag:

   ```bash
   git tag "v$VERSION"
   git push origin "v$VERSION"
   ```

5. Watch the `Release CI` workflow. When it finishes, check that the release
   contains the signed artifacts, their `.sig` files and `latest.json`, and that
   `latest.json` reports `v$VERSION` for every platform.

6. Smoke-test the updater once: install the previous version, let it detect the
   new release, install it and confirm the app relaunches with its data intact.

The `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets
must already be configured; the updater refuses unsigned bundles.

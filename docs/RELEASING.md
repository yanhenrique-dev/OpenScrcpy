# Releasing OpenScrcpy

The normal release path is tag-driven: pushing a `vX.Y.Z` tag runs
`.github/workflows/release.yml`, which builds every platform, signs the
updater artifacts, uploads them to the GitHub release and publishes
`latest.json`.

The commands below use `origin` for the GitHub remote. In a checkout where the
GitHub remote has another name (this repository's local setup calls it
`github`), substitute that name.

## First publication in a new repository

These steps only apply when the repository has no release yet (for example,
after moving the project to a new owner). Do them **before** pushing the first
tag, otherwise the release job fails on the first platform.

1. **Push `main` without tags.** Tags are pushed separately in step 4;
   `git push <remote> main` does not push them by default.

2. **Configure the repository** under **Settings**:

   - **Actions > General**: allow the workflows to run. The release job
     requests `contents: write` itself, so the default read-only token is fine.
   - **Secrets and variables > Actions**: add the secrets listed below.
   - **Security**: enable **Private vulnerability reporting** so
     [SECURITY.md](../SECURITY.md) works as written.
   - **Branches**: protect `main` (require the `Pull Request CI` checks, no
     force pushes). The release workflow trusts the tag, so the tag should
     only be created from a commit that passed CI.
   - Add a short description, the website field and topics
     (`android`, `scrcpy`, `tauri`, `screen-mirroring`).

3. **Configure the updater signing key.** `src-tauri/tauri.conf.json` contains
   the minisign **public** key under `plugins.updater.pubkey`; the matching
   private key is never committed. Store it as:

   | Secret | Value |
   | --- | --- |
   | `TAURI_SIGNING_PRIVATE_KEY` | the full contents of the private key file |
   | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | the password chosen when the key was generated |

   If the old private key is not available, generate a new pair:

   ```bash
   npm run tauri signer generate -- -w ~/.tauri/openscrcpy.key
   ```

   Copy the contents of the generated `.pub` file (the base64 public key)
   into `plugins.updater.pubkey` and put the contents of
   `~/.tauri/openscrcpy.key` in the secret. **Changing the key
   breaks in-app updates for every existing installation**, because the old
   app verifies the new signature with the old public key. Only rotate it when
   the key is lost or intentionally retired.

   During the build, Tauri prints `The updater secret key ... does not match
   the public key ...` when the secret and `plugins.updater.pubkey` are not a
   pair. Do not publish a release with that warning: the updater would reject
   every artifact.

4. **Write the initial release notes.** With no previous tag there is no
   commit range to render, so the release body falls back to
   [`docs/RELEASE_NOTES.md`](./RELEASE_NOTES.md). Update that file before
   tagging; later releases use the commit log between tags instead.

5. **Push the tag** that points at the release commit:

   ```bash
   git tag -a "v2.0.0" -m "OpenScrcpy 2.0.0"
   git push origin main
   git push origin "v2.0.0"
   ```

6. Watch the `Release CI` workflow. It creates the release, uploads every
   signed artifact and finally the `latest.json` manifest.

## Automated bump (optional)

The `Bump Version` workflow and the `Tag on Version Bump Merge` workflow
require a `RELEASE_TOKEN` secret. Create a fine-grained personal access token
with:

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

## Known limitations

- The Windows and macOS executables are not code-signed by a platform
  certificate; users may see SmartScreen or Gatekeeper warnings. The guide
  documents the override.
- The AppImage is built with `NO_STRIP=1` and excludes the bundled Wayland
  client libraries (see the `build:linux` script) so linuxdeploy can package it
  on current runners.
- `updater/latest.json` is generated by the release workflow and uploaded as a
  release asset. It is intentionally not committed; the copy under
  `scripts/__tests__/fixtures/` is only a test fixture.

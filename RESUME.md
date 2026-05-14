# Pick up where you left off

Last touched: 2026-05-14. This doc captures everything in flight so you can
clear the chat and come back without losing context.

## TL;DR — current state

- **MVP shipped**: scanner, three-pane UI, persistence, triage workflow,
  apply patches, multi-fix selector, export (md / PDF / SARIF), settings,
  dark mode, file tree, etc. All 9 numbered build-order steps green.
- **Bundle exists**: `src-tauri/target/release/bundle/dmg/Open Security_0.1.0_aarch64.dmg`
  (13 MB, unsigned, arm64). Right-click → Open works on macOS.
- **Auto-update**: scaffolded but **disabled** (`tauri.conf.json` →
  `plugins.updater.active: false`). Re-enable later by:
  1. Set `active: true` + add `"createUpdaterArtifacts": true` back to the
     `bundle` block in `tauri.conf.json`
  2. Flip `includeUpdaterJson: false` → `true` in `.github/workflows/release.yml`
  3. Make repo (or release downloads) public, or swap the `endpoints` URL
     to a private host
  4. Add `TAURI_SIGNING_PRIVATE_KEY` secret to the GitHub repo
- **Apple signing**: not done. Mid-flight at "create the CSR" step.

## In-flight: Apple Developer ID Application cert

You have a paid Apple Developer account. Need a "Developer ID Application"
cert to sign + notarize the `.dmg` so first-launch is clean.

### Step 1 — generate CSR (Mac GUI, you do it)

1. Open **Keychain Access**
2. Menu: **Keychain Access → Certificate Assistant → Request a Certificate
   From a Certificate Authority…**
3. Fill in:
   - User Email: your Apple Dev account email
   - Common Name: your name
   - CA Email: blank
   - Request is: **Saved to disk** (not "Emailed to the CA")
4. Click Continue → save the `.certSigningRequest` to Desktop
5. Click Done

### Step 2 — upload CSR to Apple

1. Go to https://developer.apple.com/account/resources/certificates/list
2. Click **+**
3. Pick **Developer ID Application** (under "Software"), Continue
4. Upload the `.certSigningRequest` file from step 1
5. Continue → Download the resulting `.cer`

### Step 3 — import the cert

1. Double-click the downloaded `.cer` — it imports into your login keychain
2. In Keychain Access, search for "Developer ID Application" — you should
   see "Developer ID Application: Your Name (TEAMID)"

### Step 4 — export to `.p12` for CI

1. Right-click the cert → **Export "Developer ID Application…"** → choose
   `.p12` format
2. Pick a password — that becomes the `APPLE_CERTIFICATE_PASSWORD` secret
3. Save as `developer-id.p12`
4. Base64 it (for the GitHub secret):
   ```bash
   base64 -i developer-id.p12 | pbcopy
   ```

### Step 5 — find your signing identity string

```bash
security find-identity -v -p codesigning
```

Copy the full "Developer ID Application: Your Name (TEAMID)" line (without
the quotes themselves). That goes in the `APPLE_SIGNING_IDENTITY` secret.

## After Apple: GitHub repo secrets

Set at **Settings → Secrets and variables → Actions → New repository secret**:

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | The base64 string from step 4 |
| `APPLE_CERTIFICATE_PASSWORD` | The password you picked during export |
| `APPLE_SIGNING_IDENTITY` | The full identity line from step 5 |
| `APPLE_ID` | Your Apple Developer email |
| `APPLE_PASSWORD` | App-specific password from https://appleid.apple.com/ → Sign-In and Security → App-Specific Passwords |
| `APPLE_TEAM_ID` | 10-char team ID from https://developer.apple.com/account/ → Membership |

Then `git tag v0.1.0 && git push --tags` triggers `.github/workflows/release.yml`
which builds, signs, notarizes, and uploads the `.dmg`s for arm64 + x86_64.

## Local files to keep

- `~/.tauri/open-sec-updater.key` — updater signing key (no password). Back
  this up to a password manager. Public key is already in `tauri.conf.json`.
- `~/.tauri/open-sec-updater.key.pub` — public key file. Not secret.

## Parked / future work

- **Workspace split for CLIs** — production `.app` carries `triage_cli` (8.4 MB).
  Move calibration CLIs (`scan_cli`, `triage_cli`, `verify_cli`, `patch_cli`,
  `pipeline_cli`) into a sibling crate so they don't end up in `[[bin]]` for
  the main package. See note in `src-tauri/Cargo.toml`.
- **Architecture review pass** — single-Opus pass over a repo summary that
  emits cross-cutting observations (different shape than per-file findings).
  Sketched but not started.
- **Auto-update** — scaffolded, disabled. See top of this doc.
- **Real app icon** — current icon is a 5-minute SVG (white ring + `<>`
  brackets + small magnifier handle on slate). Swap source PNG and run
  `bun tauri icon path/to/source.png` to regenerate.
- **Windows + Linux targets** — release workflow currently builds macOS only.

## Useful commands

```bash
# Dev loop
bun run tauri dev

# Frontend typecheck
bun run check

# Rust tests
cd src-tauri && cargo test --lib   # 78 tests

# Local release build
bun run tauri build

# Calibration CLIs (for prompt tuning, no GUI)
cd src-tauri
cargo run --bin pipeline_cli -- <dir>
cargo run --bin triage_cli   -- <dir>
cargo run --bin scan_cli     -- <file>
cargo run --bin verify_cli   -- <file>
cargo run --bin patch_cli    -- <file>

# Regenerate icon set after editing the source PNG
bun tauri icon /path/to/source.png
```

## Where stuff lives

- `CLAUDE.md` — locked product spec + calibration log (Steps 3 → 9c)
- `README.md` — user-facing install + usage
- `SECRETS.md` — detailed GitHub-secrets walkthrough (this doc is the short version)
- `.github/workflows/release.yml` — CI for tagged releases
- `src-tauri/tauri.conf.json` — product name, identifier (`com.oazab.open-sec`), version 0.1.0, updater config
- `src-tauri/icons/` — generated icon set (regenerate via `bun tauri icon`)
- `fixtures/` — calibration fixtures (textbook vulns + adversarial + multi-lang)

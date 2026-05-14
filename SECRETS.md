# Release secrets

These are the GitHub repo secrets the `release.yml` workflow expects. Set
them at **Settings → Secrets and variables → Actions → New repository
secret**. Set each only once; the workflow reads them via `${{ secrets.X }}`.

## TL;DR — 8 secrets, ~30 minutes of one-time setup

| Secret | Purpose | Source |
|---|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | Signs updater bundles so the in-app updater can verify them | `bun tauri signer generate` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password for the above (leave empty if you didn't set one) | You picked it during `signer generate` |
| `APPLE_CERTIFICATE` | macOS code-signing cert, base64-encoded | Export `.p12` from Keychain, `base64` it |
| `APPLE_CERTIFICATE_PASSWORD` | Password for the `.p12` | You picked it during export |
| `APPLE_SIGNING_IDENTITY` | Exact identity string | Read from Keychain or `security find-identity` |
| `APPLE_ID` | Your Apple Developer email | apple.com login |
| `APPLE_PASSWORD` | App-specific password (NOT your real Apple ID password) | appleid.apple.com → Sign-In and Security → App-Specific Passwords |
| `APPLE_TEAM_ID` | Your 10-character Apple Team ID | developer.apple.com → Membership |

---

## 1. Updater signing key (Tauri)

This key signs the updater `.tar.gz` files so the in-app updater can verify
nothing was tampered with in transit.

```bash
bun tauri signer generate -w ~/.tauri/open-sec-updater.key
```

You'll be prompted for an optional password. If you set one, it becomes
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` below. Hitting Enter (no password) is
fine for a single-maintainer project; the key file itself is the secret.

The command outputs both files and a **public key** to stdout. Copy that
public key into `src-tauri/tauri.conf.json` replacing
`REPLACE_WITH_PUBKEY_FROM_TAURI_SIGNER_GENERATE`. Commit that change.

Then set the secrets:

```bash
# Linux/macOS
cat ~/.tauri/open-sec-updater.key | pbcopy
# → paste as TAURI_SIGNING_PRIVATE_KEY on GitHub

# If you set a password:
# → paste it as TAURI_SIGNING_PRIVATE_KEY_PASSWORD
# Otherwise leave that secret unset.
```

Keep `~/.tauri/open-sec-updater.key` somewhere safe (password manager,
1Password Document, etc.). If you lose it, you can't ship updates that
existing installs will accept — they'll reject anything signed by a new
key.

---

## 2. Apple Developer ID cert (code signing)

You need a paid Apple Developer account ($99/year) to issue this cert.

1. **developer.apple.com → Certificates, Identifiers & Profiles**
2. Click **+** → **Developer ID Application** → Continue
3. Generate a CSR (Certificate Signing Request) on your Mac:
   - Open **Keychain Access** → menu **Certificate Assistant → Request a Certificate from a Certificate Authority**
   - Enter your email, leave "Saved to disk", click Continue
   - Save the `.certSigningRequest` file
4. Upload that CSR back on the Apple site, download the resulting `.cer`
5. Double-click the `.cer` — it imports into your login keychain
6. Now export it as a `.p12`:
   - In Keychain Access, find **"Developer ID Application: Your Name (TEAMID)"**
   - Right-click → **Export** → choose `.p12` format
   - Pick a password — that becomes `APPLE_CERTIFICATE_PASSWORD`
   - Save as `developer-id.p12`
7. Base64-encode the `.p12` for GitHub secrets:

```bash
base64 -i developer-id.p12 | pbcopy
# → paste as APPLE_CERTIFICATE on GitHub
```

8. Find the **signing identity string** to paste as `APPLE_SIGNING_IDENTITY`:

```bash
security find-identity -v -p codesigning
# Look for: "Developer ID Application: Your Name (TEAMID)"
# Paste the FULL quoted string (without the quotes themselves) as APPLE_SIGNING_IDENTITY
```

---

## 3. Notarization (Apple)

Required since Catalina (2019) so first-launch doesn't show the
"developer cannot be verified" dialog.

1. **Apple ID** → paste your Developer-account email as `APPLE_ID`

2. **App-specific password** (NOT your real Apple ID password):
   - appleid.apple.com → Sign In and Security → **App-Specific Passwords** → +
   - Name it "open-sec notarization"
   - Copy the 16-char password (looks like `abcd-efgh-ijkl-mnop`)
   - → paste as `APPLE_PASSWORD`

3. **Team ID**:
   - developer.apple.com → Membership → Team ID (10 alphanumeric chars)
   - → paste as `APPLE_TEAM_ID`

---

## Test the chain locally before tagging a release

Once secrets are set in CI, you can dry-run the same logic locally:

```bash
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/open-sec-updater.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""        # if you set one
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_ID="you@example.com"
export APPLE_PASSWORD="abcd-efgh-ijkl-mnop"
export APPLE_TEAM_ID="ABCDE12345"

bun run tauri build
```

If that succeeds you'll see in `src-tauri/target/release/bundle/`:
- `dmg/Open Security_0.1.0_aarch64.dmg` (signed)
- `macos/Open Security.app.tar.gz` (updater bundle)
- `macos/Open Security.app.tar.gz.sig` (updater signature)

You can then notarize the `.dmg` manually:

```bash
xcrun notarytool submit "src-tauri/target/release/bundle/dmg/Open Security_0.1.0_aarch64.dmg" \
  --apple-id "$APPLE_ID" \
  --team-id "$APPLE_TEAM_ID" \
  --password "$APPLE_PASSWORD" \
  --wait
xcrun stapler staple "src-tauri/target/release/bundle/dmg/Open Security_0.1.0_aarch64.dmg"
```

If that round-trips cleanly, CI will too.

---

## Cutting a release

Once everything's set:

```bash
git tag v0.1.0
git push --tags
```

GitHub Actions picks up the tag, runs the matrix (aarch64 + x86_64),
signs + notarizes both `.dmg`s, signs the updater bundles, and uploads
everything plus a `latest.json` manifest to a new GitHub Release. Users
with the app installed see the **Update available** banner on their next
launch.

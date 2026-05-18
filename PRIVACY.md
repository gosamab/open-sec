# Privacy

_Last updated: 2026-05-18_

Open Security is a local desktop app. This document describes what data it
handles and where that data goes.

## What leaves your device

**Source code you scan is sent to the Anthropic API.** Each scan walks the
folder you pick and sends file contents to Anthropic's Claude models across
four stages (triage, detect, verify, patch). This is the core function of the
app — there is no way to run a scan without sending code to Anthropic.

- **Destination:** `api.anthropic.com` over HTTPS.
- **What is sent:** file paths (relative to the scan root) and file contents
  for files that pass the ingest filter (text, ≤ 500 KB, not in a vendor /
  build directory, not in `.gitignore`).
- **What is _not_ sent:** files outside the scan root, files filtered out at
  ingest, your API key (sent only as an auth header), or anything from other
  apps or directories on your machine.
- **Anthropic's handling of that data** is governed by their terms and
  privacy policy: https://www.anthropic.com/legal/privacy.

If you do not want a particular file or directory scanned, exclude it via
`.gitignore` or move it outside the scan root before running a scan.

## What stays on your device

- **Anthropic API key** — stored in the macOS Keychain under the service name
  `open-sec`, account `anthropic`. Never written to disk in plaintext, never
  sent anywhere except as the `x-api-key` header on requests to
  `api.anthropic.com`.
- **Scan history and findings** — stored in a local SQLite database at
  `<app_data_dir>/open-sec.db` (on macOS: `~/Library/Application Support/com.oazab.open-sec/open-sec.db`).
  This includes finding text, verdicts, patch proposals, and your
  accept/dismiss decisions. Delete the file to wipe all history.
- **Application logs** — written to stderr at runtime; not persisted by the
  app itself.

## What the app does _not_ do

- No telemetry. No analytics. No crash reporting. No "phone home".
- No background activity — the app only does work while you have it open and
  are running a scan.
- No automatic updates that send data — the app does not check for updates.
- No third-party services other than the Anthropic API.

## Source code

Open Security is open source under the MIT License. You can audit exactly
what data is sent and where in the [`providers/`](src-tauri/src/providers/)
and [`scanner/`](src-tauri/src/scanner/) directories.

## Warranty

This software is provided "as is", without warranty of any kind. See
[LICENSE](LICENSE).

## Contact

Questions: gosamab@hotmail.com

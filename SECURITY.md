# Security Policy

## Supported Versions

Ferris is pre-1.0: only the latest minor line receives security fixes.

| Version | Supported          |
| ------- | ------------------ |
| 0.3.x   | :white_check_mark: |
| 0.2.x   | :x: End of life    |
| ≤ 0.1.x | :x: End of life    |

`0.2.x` reached end of life with the `0.3.0` release (2026-11-22). No further
security backports are issued for it; upgrade to `0.3.x`. The exact policy and
the release checklist live in [`VERSIONING.md`](VERSIONING.md).

## Reporting a Vulnerability

If you discover a security vulnerability in Ferris, please report it responsibly.

**Do not** open a public GitHub issue for security vulnerabilities.

Instead, report it privately through
[GitHub Security Advisories](https://github.com/OpenFerris/FerrisFramework/security/advisories/new).
If advisories are unavailable, email the maintainer at
<holasoybarbiequetal1090@gmail.com> with `[SECURITY]` in the subject.

Include the following information:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Any suggested fixes

We will acknowledge receipt of your report within 48 hours and work with you to understand and address the issue.

Thank you for helping keep Ferris secure!

## Secret fields (`Secret` fieldtype)

`Secret` fields (OAuth client secrets, webhook signing keys, SMTP/IMAP
passwords) are encrypted at rest with AES-256-GCM. The key is **not** derived
from any authentication setting: rotating a login credential must not make
every stored integration credential undecryptable. Configure it explicitly, as
32 bytes of hex or base64:

```toml
[secrets]
key = "…64 hex characters or base64…"
# Retired keys still accepted while rows are rewritten after a rotation.
previous_keys = []
```

or `FERRIS_SECRETS__KEY` / `FERRIS_SECRET_KEY` (the CLI accepts both).

Operational notes:

- Losing every installed key makes the stored secrets unrecoverable. Keep the
  key with the rest of the deployment secrets (and back it up); `ferris doctor`
  reports `Secret` rows it cannot read.
- Rotation is `key = <new>`, `previous_keys = [<old>]`, then re-save each
  credential (or leave the old key installed). Removing a key that still
  protects a row surfaces `SecretError::UnknownKeyId`.
- A `Secret` value is masked (`********`) on every read. Sending the mask back
  means "unchanged"; send the plaintext to replace it. An empty value does not
  clear the stored secret — delete the row instead.
- Renaming a DocType or a `Secret` field invalidates the associated-data binding
  of existing values: they must be re-entered after such a rename.
- Rows written before the `Secret` fieldtype existed hold one-way Argon2 hashes
  produced by the old `Password` treatment. They are **not** recoverable:
  re-enter the credential; `ferris doctor` reports the affected rows.

## Inbound mail (IMAP)

- **Credentials**: the poller reads the IMAP password from
  `[email.accounts]` in `ferris.toml` (the same source as the SMTP hub). The
  Desk's `Email Account` row declares `imap_password` as a `Secret`, encrypted
  at rest and masked on read, but it is not the runtime source. `ImapConfig`
  and `EmailAccountConfig` redact the password in `Debug`, `ferris config show`
  masks it, and the poller never logs the command or the password.
- **Transport**: `rustls` with the `ring` provider only (`tokio-rustls`); no
  `native-tls` and no OpenSSL. Implicit TLS (993) and STARTTLS (143) are the
  supported modes; an account with neither is **rejected**, not silently
  downgraded. Certificate verification uses the webpki root store; there is no
  "skip verification" switch. `allow_plaintext` exists in the library for
  loopback tests and is not exposed through `[email.accounts]`.
- **Bounded parsing**: response lines (64 KiB), message size (25 MiB) and the
  literal drain ceiling are enforced before allocation. An oversized message is
  drained and left **unseen** (never deleted server-side); a malformed one is
  logged with its UID and left unseen too. A storage failure never sets `\Seen`.
- **Stored data**: `Inbound Email.raw` archives the original RFC822 message, so
  it can contain credentials, tokens and personal data. The DocType is
  `System Manager`-only; apply your retention policy to the table and to backups.
  Attachments are recorded as name/type/size, not copied into the row.

## Push notifications are in-app only

A `Push Notification` is delivered as a `Notification` row plus a publish on the
process-wide in-app feed (SSE/socket.io), addressed to one user. There is no
APNs/FCM transport and therefore no device token, device certificate or
third-party push credential stored or sent by Ferris. A delivery failure is
recorded on the row (`attempts`, `error`, `next_attempt_at`) and retried; it never
falls back to a different user or to email.

## Plugin blobs are private by default

`blobstore::put` stores plugin blobs with `is_private = 1` and owner
`ferris-plugin`, so knowing the URI no longer grants read access. `get`,
`get-range`, `metadata` and `signed-url` check the calling app's capability
context, and `delete` only removes objects owned by that plugin's namespace. An
idempotent migration marks pre-existing `blobstore/` rows with
`is_private = 0` as private.

## Deployment boundaries

- **CORS is an explicit allow-list.** `CorsPolicy` only answers with CORS
  headers for origins configured in `cors_origins` when `enable_cors` is on. It
  never reflects an arbitrary `Origin` and refuses `*`; a disabled policy or an
  unlisted origin emits no header, which is the browser's same-origin default.
- **`Desired State` reads are open to every authenticated user** by contract
  (control-plane agents depend on it); writes are restricted to `System
  Manager`. The payload of every entity is therefore visible to any session —
  withhold the DocType permission if a deployment treats it as sensitive.
- **Plugin UI assets are trusted same-origin code (H15).** Scripts and
  stylesheets served from `/plugins/{name}/asset/…` run with the Desk's origin
  and can use its session. The WASM guest is sandboxed; the browser-side assets
  are not. Install only plugins whose source you trust; `ferris serve` logs the
  boundary once when `/plugins` is mounted.

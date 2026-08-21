# Claude and Codex providers — tokens, endpoints, the filesystem-view problem
Expands S7, G8, G10, I2, I7, N4, N5, M17, M18. Code: `src/providers/claude.rs`,
`src/providers/codex.rs`, `src/providers/mod.rs`.

Every family is a module exposing `fetch() -> Snapshot { family, plan, limits }`.

## Claude
Covers Claude Desktop *and* Claude Code / CLI — same account, same quota.

Token chain (all local, all read-only):
1. `%APPDATA%\Claude\config.json` → key `oauth:tokenCache`
2. ciphertext is Chromium-style: `v10` prefix + 12-byte nonce + AES-256-GCM payload
3. master key from `%APPDATA%\Claude\Local State` → `os_crypt.encrypted_key`, base64-decode,
   strip the `DPAPI` prefix, `CryptUnprotectData`
4. usage: `https://api.anthropic.com/api/oauth/usage`

Both files are re-read on every poll — Claude Desktop replaces them atomically, so they are
probed by *reading* (`read_with_retry`, 5 × 120 ms), never by `is_file()`. The data
directory is resolved from several bases in order: `APPDATA` → `dirs::config_dir()` →
`USERPROFILE\AppData\Roaming` → home (I7).

### G10 — multiple OAuth entries
`config.json` holds more than one entry and only some are accepted (observed 200, 429, 403).
`load_tokens()` returns the profile-scoped ones with the subscription entry first; `fetch()`
tries each in turn and caches the winner in `LAST_GOOD`, clearing it on failure.

### N4 / N5 — dead ends
PowerShell 5.1 cannot decrypt this (no `AesGcm`). There is no `.claude/.credentials.json`
on this machine and no Claude entry in Credential Manager — the encrypted `config.json` is
the only source.

## Codex
`~/.codex/auth.json`, shared by the Codex app and the Codex CLI.
- usage: `https://chatgpt.com/backend-api/codex/usage`
- headers: `Authorization: Bearer <access>`, `chatgpt-account-id: <id>`
- response: `plan_type` plus
  `rate_limit.{primary,secondary}_window.{used_percent, limit_window_seconds, reset_at}`

**Never write `auth.json` and never refresh the token** (I2): refresh tokens rotate, so a
refresh performed by Quotty would invalidate the one Codex holds and sign the user out.
Access tokens live roughly 10 days and Codex refreshes them itself — which is why the source
goes offline if Codex has not been run for that long (G17), by design.

## G8 — the same path shows different contents to elevated and normal-user processes
Machine-specific, proven, unresolved.

Proof: a marker file `%APPDATA%\Quotty\probe_elevated.txt` written from the elevated session
is invisible to an Explorer-launched process. In the elevated view `%APPDATA%\Claude`
contains the full Claude Desktop data; in the normal-user view it contains only `CLAUDE.md`.

Consequences:
- editing `settings.json` from the agent's shell does **not** reach the app when the app runs
  normally — write it through an Explorer-launched helper;
- a normally-launched Quotty reports "Claude data dir not found" here (S7) while Codex and
  Antigravity work;
- test UI and provider behaviour with an Explorer-launched instance, never one started from
  the agent's shell.

The ACL grants the user full control and the directory is not a reparse point, so plain
permissions do not explain it.

Leading hypothesis (being implemented, not yet proven — P2): the visible `%APPDATA%\Claude`
is a leftover and the live data belongs to a **Store / MSIX** install, whose package
container redirects `%APPDATA%` to
`%LOCALAPPDATA%\Packages\<package>\LocalCache\Roaming\Claude`. A process running inside such
a container and one outside it then see different contents at literally the same path
string, which matches every observation above.

The uncommitted `find_claude_files()` rework in `src/providers/claude.rs`:
- enumerates `%LOCALAPPDATA%\Packages\*claude*|*anthropic*\LocalCache\{Roaming,Local}\Claude`
  alongside the classic roots;
- skips a directory outright when `config.json` has no metadata — a `read_with_retry` on an
  absent file burns ~0.6 s per candidate;
- picks the candidate whose `config.json` was modified **last**, because the leftover install
  still holds a stale token that would authenticate and then fail.

# Antigravity provider — discovery, transport, quota shape
Expands G4, G5, G6, N6, D2, M19, M20. Code: `src/providers/antigravity.rs`, `src/winproc.rs`.

## Transport
- `POST https://127.0.0.1:<port>/exa.language_server_pb.LanguageServerService/GetUserStatus`
- body: `{"metadata":{"ideName":"antigravity"}}`
- headers: `X-Codeium-Csrf-Token: <token>`, `Connect-Protocol-Version: 1`
- This is the same call the IDE's own usage panel makes. Nothing leaves the machine.

## Response shape
```
userStatus.cascadeModelConfigData.clientModelConfigs[]
    .label
    .quotaInfo.remainingFraction
    .quotaInfo.resetTime
userStatus.userTier.name
userStatus.planStatus.planInfo.planName
```
`remainingFraction` is a fraction remaining, not a used percent — invert it for the bar.

## Discovery order (`candidates`)
1. cached last-good endpoint (`LAST_GOOD`, in-process only)
2. running processes (`from_processes`) — the only source that works for the IDE
3. daemon descriptors `~/.gemini/*/daemon/ls_*.json` (`httpsPort`, `csrfToken`), newest first
4. Electron log `%APPDATA%\Antigravity[ IDE]\logs\main.log`, markers `--csrf_token …` and
   `Local: https://127.0.0.1:<port>/`

## G4 — the IDE announces itself nowhere
The 2.0 **app** runs `language_server.exe` and logs both its port and `--csrf_token`.
The **IDE** runs `language_server_windows_x64.exe`, writes no Electron log entry and no
daemon descriptor — the `ls_*.json` present on this machine dates from March and is stale.
So the processes themselves are the only source:
- port: `GetExtendedTcpTable` → `winproc::listening_ports()`
- CSRF token: `NtQueryInformationProcess(ProcessCommandLineInformation)` → `winproc::command_line()`

Both succeed from a normal-user process against an **elevated** IDE, because
`PROCESS_QUERY_LIMITED_INFORMATION` is granted up the integrity ladder.
`PROCESS_VM_READ` is not — see N6, never read the PEB for this.

## G5 — the port pair
The language server listens on adjacent ports; the HTTPS endpoint is the **lower** of the
pair. A third port speaks LSP and will swallow a TLS handshake until the timeout expires,
so only the pair member is ever tried.

## G6 — self-signed certificate
The server presents a self-signed cert for `127.0.0.1`. A dedicated `ureq` agent is built
with a rustls `ServerCertVerifier` (`AcceptAnyCert`) that accepts anything, and that agent
is used only for this host. `rustls` is a direct dependency solely for this.

## D2 — two rows, not one per model
Gemini Pro and Gemini Flash draw from one shared pool (owner-confirmed), so
`group_of` folds every model label into one of `GROUP_TITLES = ["Gemini", "Claude / GPT"]`.

## Window length
No window length is returned at all. `WINDOW_SECS = 5 * 3600` is assumed and stretched when
the reset time is further out, which is what `window_start` is synthesized from (I5).

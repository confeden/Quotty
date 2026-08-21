# Quotty — agent rules

Rust + eframe/egui, Windows 10/11 x64 only. A borderless always-on-top strip showing
the quota + reset timers of the AI tool currently in use (Claude / Codex / Antigravity).
Ships as `dist\Quotty-Setup-<ver>.exe` (Inno Setup, per-user install). GPL-3.0,
`https://github.com/confeden/Quotty`, default branch `main`.

## Commands
| Task | Command | cwd |
|---|---|---|
| build | `cargo build --release` | repo root |
| test | `cargo test` | repo root |
| run (dev) | `target\release\quotty.exe` — launch from Explorer, not this shell (see Never) | repo root |
| installer | `powershell -ExecutionPolicy Bypass -File tools\build-installer.ps1` | repo root |

Installer needs Inno Setup 6 at `D:\Programs\Inno Setup 6`; the version comes from
`Cargo.toml` only.

## Never
- Never write `~/.codex/auth.json` or refresh the Codex token — refresh tokens rotate,
  so writing it signs the user out of Codex. Same for every other vendor store: Quotty
  reads them, never writes them.
- Do not judge UI or provider behaviour from an instance started in this shell. This
  machine shows different filesystem contents to elevated and normal-user processes at
  the same path, so quota sources differ. Launch through Explorer to test.
- Do not commit `ROADMAP.md`, `RELEASE-1.0.0.md`, `.claude/` or a `.gitignore` — working
  notes stay local; ignore rules live in `.git/info/exclude` (owner decision).
- Do not bump the version in one place: `Cargo.toml` and `quotty.rc` must move together.
- Always `git fetch`/rebase before pushing — the owner edits `README.md` directly on GitHub.

## Context
`ROADMAP.md` is the project state — read it once before substantive work.
Deep detail lives in `.claude/kb/`, indexed at the end of ROADMAP.md. Both are
maintained by the `roadmap` skill.

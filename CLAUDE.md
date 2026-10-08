# Repository Guidelines

Communicate with the owner in Korean. Write repository artifacts (docs, comments, commit messages, UI copy, error messages, release notes) in English.

## Build and verification

Windows x64 Rust application; checks are in README.md ("Development checks").

- Rerun `scripts/build-release.ps1` after user-facing changes so `dist/netladder.exe` matches; when the exe is locked it writes `dist/netladder-updated.exe`, say so.

## Engine invariants

- A failed socket-owner table read keeps the previous mapping and must not stop the engine.
- Process IDs leave a row after `IDLE_TIMEOUT`.
- The scheduler syncs `order` and limits with the UI on `CONFIG_SYNC_INTERVAL` or when a new process appears, never per packet.

## UI invariants

- Header and row columns share the width constants at the top of `app.rs`; change them together.
- Unlimited rows leave after `IDLE_TIMEOUT`.
- The computed row order (limited rows first) is reused for `SORT_REFRESH_INTERVAL` (or until the row set, sort or a limit toggle changes) so a usage sort does not reshuffle rows under the pointer.
- Row content is vertically centered with a visible boundary between rows in both themes; colors come from the theme or `RowPalette`.
- Malgun Gothic leads the proportional family and only backs up the monospace family; monospace text stays fixed-width.

## Dependencies and packaged files

- Do not edit generated Cargo artifacts or hand-modify WinDivert binaries in `vendor` or `dist`; `scripts/setup-windivert.ps1` obtains them.
- Preserve unrelated working-tree changes.

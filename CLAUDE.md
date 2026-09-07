# Repository Guidelines

Communicate with the owner in Korean. Write repository artifacts (docs, comments, commit messages, UI copy, error messages, release notes) in English.

## Build and verification

Windows x64 Rust application. Set `WINDIVERT_PATH` before invoking Cargo directly, then run the checks before handing off:

```powershell
$env:WINDIVERT_PATH = (Resolve-Path .\vendor\windivert).Path
cargo fmt --all -- --check
cargo check
cargo test
```

- `scripts/build-release.ps1` makes distributable builds and packages `dist/`. Rerun it after user-facing changes so `dist/netladder.exe` matches; when the exe is locked it writes `dist/netladder-updated.exe`, say so.
- A debug build with `NETLADDER_PREVIEW=1` seeds sample rows for UI work without admin rights or live traffic; the hook is debug-only.

## Engine invariants

- One queue and token bucket per process name. A queue holds about 0.25 s of traffic at its rate (128 KiB..8 MiB); packets beyond that are dropped so TCP backs off, and dropped bytes are reported per process.
- Per-process usage counts reinjected bytes, so a limited process never shows more than its limit; the header peak uses arrival bytes and is informational.
- A failed socket-owner table read keeps the previous mapping and must not stop the engine.
- Process IDs leave a row after `IDLE_TIMEOUT`.
- The scheduler syncs `order` and limits with the UI on `CONFIG_SYNC_INTERVAL` or when a new process appears, never per packet.
- Dropping `EngineHandle` stops the threads and reinjects queued packets before the WinDivert handle closes.

## UI invariants

- The table header stays outside the scrolling region.
- Header and row columns share the width constants at the top of `app.rs`; change them together.
- Limited rows stay visible while idle so the limit can be turned off; unlimited rows leave after `IDLE_TIMEOUT`.
- Limited rows sort before unlimited rows. The computed order is reused for `SORT_REFRESH_INTERVAL` (or until the row set, sort or a limit toggle changes) so a usage sort does not reshuffle rows under the pointer.
- Row content is vertically centered with a visible boundary between rows in both themes; colors come from the theme or `RowPalette`.
- A process's limit value is remembered while disabled and restored when enabled.
- Malgun Gothic leads the proportional family and only backs up the monospace family; monospace text stays fixed-width.

## Dependencies and packaged files

- Do not edit generated Cargo artifacts or hand-modify WinDivert binaries in `vendor` or `dist`; `scripts/setup-windivert.ps1` obtains them.
- Preserve unrelated working-tree changes.

# Repository Guidelines

## Language

- Communicate with the project owner in Korean.
- Write repository artifacts in English, including documentation, comments,
  commit messages, UI copy added by agents, and release notes.

## Build and verification

- This is a Windows x64 Rust application.
- Set `WINDIVERT_PATH` before invoking Cargo directly:

  ```powershell
  $env:WINDIVERT_PATH = (Resolve-Path .\vendor\windivert).Path
  ```

- Before handing off code changes, run:

  ```powershell
  cargo fmt --all -- --check
  cargo check
  cargo test
  ```

- Use `scripts/build-release.ps1` for distributable builds.
- After user-facing changes, update `dist/netladder.exe` and verify that its
  SHA-256 hash matches the release build. If the executable is running and
  locked, write `dist/netladder-updated.exe` instead and explain why.

## UI invariants

- Keep the process table header outside the scrolling region.
- Only pinned rows may be dragged.
- The pin checkbox must remain in the same position when its rank appears.
- Keep row content vertically centered and row geometry synchronized with the
  animated slot height to prevent overlap.
- Reorder pinned rows continuously while dragging. A move is triggered when the
  dragged row's center crosses another row's center.
- Preserve a visible boundary between adjacent rows in the dark theme.

## Dependencies and packaged files

- Do not edit generated Cargo artifacts.
- Do not modify WinDivert binaries in `vendor` or `dist` by hand.
- Use `scripts/setup-windivert.ps1` to obtain WinDivert and
  `scripts/build-release.ps1` to package it.
- Preserve unrelated working-tree changes.

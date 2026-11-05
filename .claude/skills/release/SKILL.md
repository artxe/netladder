---
name: release
description: Rebuild NetLadder and replace the archive and its SHA-256 on the v1.0.0 GitHub release. Use when the owner asks for a release or a re-release, or after the history was folded into the v1.0.0 commit and pushed.
---
# Re-release v1.0.0

The only release is `v1.0.0`: every change folds into the one `v1.0.0` commit, its tag moves with it, and the release's archive is rebuilt from it.

1. The `v1.0.0` commit and tag are on origin: `git log -n 1 --format=%s` is `v1.0.0`, and `git ls-remote origin refs/tags/v1.0.0` names the commit `git rev-parse HEAD` gives.
2. `./scripts/build-release.ps1` writes `dist/netladder-windows-x64.zip`. It packs `target/release/netladder.exe`, so a warning that `dist/netladder.exe` is running does not stop the release.
3. `Get-FileHash dist/netladder-windows-x64.zip` gives the archive's SHA-256.
4. `gh release upload v1.0.0 dist/netladder-windows-x64.zip --clobber` replaces the asset.
5. `gh release view v1.0.0 --json body` gives the notes: write them to `.tmp/release-notes.md` with only the hash on the `SHA-256` line replaced (upper case, as `Get-FileHash` prints it), then `gh release edit v1.0.0 --notes-file .tmp/release-notes.md`.
6. `gh release view v1.0.0` shows the one asset and the new hash.

# NetLadder

NetLadder is a native Windows application for setting an independent download
speed limit for each active process. Processes without an enabled limit pass
through without shaping.

## Download

[Download the latest NetLadder release for Windows x64](https://github.com/artxe/netladder/releases/latest/download/netladder-windows-x64.zip)

Extract the archive, keep all packaged files together, and run
`netladder.exe`. Administrator access is required for WinDivert.

## Highlights

- Discovers processes with inbound TCP or UDP traffic in real time.
- Sets a separate Mbps download limit for every process.
- Uses an independent queue and token bucket per process, so one limited process
  does not consume another process's allowance.
- Displays executable icons, process names, PIDs, cumulative traffic, and the
  current transfer rate.
- Sorts limited and unrestricted processes independently by name or current
  usage.
- Keeps the table header visible while process rows scroll.

## Quick start

NetLadder targets Windows 10/11 x64. Keep these files together:

```text
dist/
|-- netladder.exe
|-- WinDivert.dll
|-- WinDivert64.sys
|-- LICENSE
|-- README.md
`-- WinDivert-LICENSE.txt
```

1. Run `netladder.exe` and accept the UAC prompt.
2. Start an application that uses the network.
3. Enable `Limit` for its process and enter the desired value in Mbps.
4. Disable `Limit` to restore unrestricted throughput for that process.

Limits apply to downloads (inbound traffic). Upload shaping is outside the
current scope.

## How it works

```mermaid
flowchart LR
    A[Inbound packet] --> B[WinDivert capture]
    B --> C[Flow-to-process lookup]
    C --> D[Per-process queue]
    D --> E[Per-process token bucket]
    E --> F[Round-robin scheduler]
    F --> G[Packet reinjection]
```

NetLadder maps each inbound packet to its owning process and places it in that
process's queue. The scheduler visits queues in round-robin order. A configured
token bucket paces its process at the selected bit rate, while an unrestricted
queue remains immediately eligible. The UI's detected throughput is
informational and does not alter configured limits.

## Build from source

### Requirements

- Windows 10/11 x64
- Stable Rust with the MSVC toolchain
- PowerShell
- Visual Studio Build Tools with C++ support

### Release build

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\scripts\build-release.ps1
```

The release script obtains WinDivert when needed, builds the optimized Windows
executable, and packages it with the DLL and signed driver under `dist`.

### Development checks

```powershell
$env:WINDIVERT_PATH = (Resolve-Path .\vendor\windivert).Path
cargo fmt --all -- --check
cargo check
cargo test
```

## Project layout

```text
src/
|-- app.rs       # egui process table and per-process limit controls
|-- engine.rs    # shared state and throughput estimation
|-- main.rs      # native window and application bootstrap
`-- windows.rs   # WinDivert capture, process queues, and rate scheduler
```

## Scope and security

NetLadder requires administrator privileges because packet capture and
reinjection use the WinDivert kernel driver. Traffic shaping occurs only on the
local machine and cannot change upstream limits imposed by a server, ISP, VPN,
or router. Use only trusted WinDivert binaries and keep all packaged files
together.

NetLadder is MIT-licensed; see `LICENSE`. WinDivert is a separate project
distributed under its own dual LGPL-3.0/GPL-2.0 licensing terms; see
`WinDivert-LICENSE.txt` in the release archive.

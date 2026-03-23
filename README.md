<div align="center">

<img alt="Velin logo" src="assets/logo.svg" width="96">

# Velin

LAN audio routing beta for Windows and Linux

[![Rust](https://img.shields.io/badge/Rust-systems-orange?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![Windows](https://img.shields.io/badge/Windows-tested-0078D6?style=flat-square&logo=windows)](.)
[![Linux](https://img.shields.io/badge/Linux-target-FCC624?style=flat-square&logo=linux&logoColor=black)](.)
[![Status](https://img.shields.io/badge/status-0.2.0--beta.1-555?style=flat-square)](.)
[![License](https://img.shields.io/badge/license-MIT-111111?style=flat-square)](LICENSE)

</div>

> Velin `0.2.0-beta.1` is a cross-platform LAN audio routing beta for sending system audio and microphone audio between Windows and Linux machines on the same local network. It includes a desktop GUI, headless/session commands, encrypted pairing, receiver discovery, saved settings, and a working TCP/UDP transport path.

<p align="center">
  <a href="#current-state">Current State</a> |
  <a href="#what-works-today">What Works Today</a> |
  <a href="#what-is-not-built-yet">What Is Not Built Yet</a> |
  <a href="#stack">Stack</a> |
  <a href="#workspace-layout">Workspace Layout</a> |
  <a href="#roadmap">Roadmap</a> |
  <a href="#getting-started">Getting Started</a>
</p>

---

## Current State

Velin is now usable as a beta!

What exists now:

- A Slint desktop GUI with `Session`, `Metrics`, `Settings`, and `Extra Options` tabs
- `Sender` and `Receiver` session modes in the app
- Manual IP connection for sender mode
- Automatic receiver discovery in the GUI
- Persisted local settings for target IP, control port, audio port, and theme
- Persisted local settings for bind IP and output device selection
- Sender capture mode selection for system audio, microphone, or mixed system-plus-microphone
- Advanced sender capture selection for specific Linux sources, microphones, and external virtual devices
- Receiver-side concurrent multi-stream mixing from multiple senders
- Headless sender and receiver commands that run from saved settings without the GUI
- Fingerprint-confirmed encrypted pairing with trusted-peer persistence
- Dark and light mode in the app
- TCP control handshake plus UDP frame streaming
- Stop, disconnect, mute, and graceful window-close shutdown
- CLI and headless commands for transport testing and unattended sessions
- A bind-IP setting for receiver mode
- Output device enumeration and selection for receiver playback
- Receiver playback of streamed audio
- Windows system audio capture for sender mode via WASAPI loopback
- Linux system and microphone capture for sender mode via Pulse/PipeWire-compatible `parec`

What it does not do yet:

- Full per-stream routing, naming, and mixer controls on the receiver
- Production-grade background service integration for auto-start / OS service install
- Broader automated test coverage for transport, audio timing, and pairing flows

```text
+----------------+      local network       +----------------+
| Sender machine | -----------------------> | Receiver       |
| system audio   |   TCP control + UDP      | plays stream   |
| capture        |                          | audio          |
+----------------+                          +----------------+
```

---

## What Works Today

| Area | Current behavior |
| --- | --- |
| GUI | Starts by default with a small desktop window |
| Roles | Sender and receiver actions are available in the session tab |
| Connection | Receiver discovery works, with manual IP connect as fallback |
| Settings | Target IP, bind IP, output device, capture defaults, control port, audio port, and theme are saved locally |
| Transport | TCP control plus UDP audio streaming works, with sender auto-reconnect |
| Sender behavior | Windows sender mode uses WASAPI loopback and input capture. Linux sender mode uses Pulse/PipeWire-compatible `parec`. Sender capture can run as system audio, microphone, or system-plus-microphone |
| Receiver behavior | Receiver advertises itself on LAN, accepts multiple concurrent senders, mixes them into one output, and reports stream activity and latency metrics |
| Security | Sessions use encrypted pairing with trusted fingerprint confirmation and stored trusted peers |
| Session controls | Start, stop, disconnect, and mute are available in the GUI |
| Headless mode | `headless` / `service` sender and receiver commands run without the GUI and reuse saved settings |
| CLI fallback | `listen` and `connect <ip>` still work |

---

## What Is Not Built Yet

These are still planned, not implemented:

- Per-stream routing and mixer controls on the receiver
- Real background service / daemon installation and management
- Wider automated test coverage and release-hardening work

---

## Stack

| Area | Choice |
| --- | --- |
| Language | Rust |
| UI | Slint |
| Async runtime | Tokio |
| Control + security | JSON handshake messages, X25519 session key exchange, HKDF, ChaCha20-Poly1305 |
| Audio payload | UDP PCM frames with per-stream IDs |
| Transport | TCP for control, UDP for frame streaming |
| Config and storage | Serde + local JSON settings |

---

## Workspace Layout

This is the current repo shape, not the final intended crate layout:

```text
velin/
|-- assets/
|   `-- logo.svg
|-- crates/
|   |-- velin-app
|   |   `-- src/
|   |       |-- audio.rs
|   |       |-- app.rs
|   |       |-- capture.rs
|   |       |-- discovery.rs
|   |       |-- main.rs
|   |       |-- settings.rs
|   |       |-- transport.rs
|   |       `-- ui.rs
|   `-- velin-proto
|       `-- src/
|           `-- lib.rs
|-- Cargo.toml
`-- README.md
```

---

## Roadmap

### Phase 1: Prototype Backbone

- [x] Workspace and crate structure
- [x] Sender and receiver role flow
- [x] GUI-first app shell
- [x] Manual IP connection
- [x] Persisted basic settings
- [x] Dark and light mode
- [x] TCP/UDP transport prototype
- [x] Basic TCP handshake (`Hello` / `Accept`)
- [x] Real receiver playback
- [x] Audio device enumeration
- [x] Automatic peer discovery
- [x] Real system audio capture
- [x] Basic connect/disconnect/mute session controls

### Phase 2: Real Audio Routing

- [x] Capture full system audio on Windows
- [x] Capture full system audio on Linux
- [x] Improve stream health and latency reporting
- [x] Remember preferred peers and devices
- [x] Auto-reconnect

### Phase 3: Advanced Routing

- [x] Specific or per-application audio capture
- [x] Microphone forwarding
- [x] Virtual sinks and sources
- [x] Multi-stream support
- [x] Encrypted sessions and trusted pairing
- [x] Headless or service mode

---

## Getting Started

Velin can run with the desktop GUI or in headless/service mode.

```bash
git clone https://github.com/p-stanchev/velin.git
cd velin
cargo run -p velin-app
```

GUI:

- Open the `Session` tab
- Use `Start Receiver` on one machine
- Use `Refresh` in sender mode to look for receivers, or enter the receiver IP manually
- Use `Start Sender` on the other machine once a receiver is selected or entered

CLI and headless commands:

```bash
cargo run -p velin-app -- listen
cargo run -p velin-app -- connect 127.0.0.1
cargo run -p velin-app -- headless receiver
cargo run -p velin-app -- headless sender
cargo run -p velin-app -- service sender 192.168.0.62
```

Headless and service commands:

- `headless receiver` or `service receiver` starts the receiver without opening the GUI
- `headless sender [target-ip]` or `service sender [target-ip]` starts the sender without the GUI
- `headless` prompts in the terminal when an unknown peer fingerprint must be approved
- `service` is non-interactive and only works with already-trusted peers
- if sender mode omits `[target-ip]`, Velin uses the saved target IP from settings

### Requirements

| Platform | Requirements |
| --- | --- |
| Windows | Rust toolchain |
| Linux | Rust toolchain, `parec`, `aplay`, and PulseAudio or PipeWire pulse compatibility |
| Both | `cargo`, `git`, and two machines on the same local network if you want a real network test |

### Packaging

Windows MSI:

```powershell
cargo install cargo-wix
.\scripts\dist-windows.ps1
```

Linux Debian package:

```bash
cargo install cargo-deb
./scripts/dist-linux.sh
```

Both scripts write installer output into `dist/`.
The Debian package also installs a desktop launcher and application icon for menu integration.

### Windows Firewall

When the receiver runs on Windows, inbound firewall rules may be required before another machine can connect.

- TCP `49000` for the control channel
- UDP `49001` for audio frames
- UDP `49002` for discovery

Example PowerShell commands:

```powershell
New-NetFirewallRule -DisplayName "Velin TCP 49000" -Direction Inbound -Action Allow -Protocol TCP -LocalPort 49000
New-NetFirewallRule -DisplayName "Velin UDP 49001" -Direction Inbound -Action Allow -Protocol UDP -LocalPort 49001
New-NetFirewallRule -DisplayName "Velin UDP 49002" -Direction Inbound -Action Allow -Protocol UDP -LocalPort 49002
```

---

## Notes

- Windows sender mode now captures real system audio from the default render endpoint.
- Linux sender mode captures system audio through a monitor source using `parec`. It can use `VELIN_LINUX_MONITOR` to override the detected monitor source when needed.
- Linux microphone capture also uses Pulse/PipeWire-compatible `parec` sources instead of relying on the less stable ALSA input path.
- Sender mode can now run as `System`, `Mic`, or `System + Mic`, and the `Extra Options` tab stores those defaults locally.
- External virtual devices can be used by selecting or entering the source/device name exposed by the host audio stack.
- Receiver mode now supports multiple concurrent senders and mixes them into one playback stream.
- The current receiver can play streamed audio on a selected output device.
- Linux is a supported beta target, but lower-powered Linux machines may still need slightly higher latency for smooth playback.
- Receiver mode can bind to `Automatic` (`0.0.0.0`) or a specific local IPv4 address.
- Receiver discovery uses local-network UDP discovery packets, and sender mode also has a manual refresh action.
- Receiver playback currently uses the selected device's default output config.
- Linux capture currently expects `parec` plus PulseAudio/PipeWire pulse compatibility to be available.

## Troubleshooting

- If Linux cannot connect to a Windows receiver and TCP `49000` times out, check Windows Defender Firewall first.
- If receiver discovery does not find a peer, use the `Refresh` action in sender mode or enter the receiver IP manually.
- If headless mode asks for a fingerprint, compare it on both machines before typing `y`.
- If `service` mode refuses a new peer, trust that peer once in the GUI or in `headless` mode first.
- If Linux sender capture produces no audio, verify `parec` is installed and the selected monitor/source exists in `pactl list short sources`.
- If Linux receiver playback fails, verify `aplay` is installed and the selected output device is healthy.
- If Linux playback is a little choppy on older hardware, increase the system load headroom first: close other apps, reduce stream count, and retest before changing app settings.

---

## License

[MIT](LICENSE)

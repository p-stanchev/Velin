<div align="center">

<img alt="Velin logo" src="assets/logo.svg" width="96">

# Velin

LAN audio routing prototype for Windows and Linux

[![Rust](https://img.shields.io/badge/Rust-systems-orange?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![Windows](https://img.shields.io/badge/Windows-tested-0078D6?style=flat-square&logo=windows)](.)
[![Linux](https://img.shields.io/badge/Linux-target-FCC624?style=flat-square&logo=linux&logoColor=black)](.)
[![Status](https://img.shields.io/badge/status-prototype-555?style=flat-square)](.)
[![License](https://img.shields.io/badge/license-MIT-111111?style=flat-square)](LICENSE)

</div>

> Velin is an early desktop prototype for sending system audio between machines on the same local network. Right now it provides a GUI, sender/receiver session flow, LAN receiver discovery with manual IP fallback, saved settings, output device selection, encrypted pairing, and a working TCP/UDP transport path.

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

Velin is not a complete audio router yet. The current codebase is a transport and app-shell prototype.

What exists now:

- A Slint desktop GUI with `Session`, `Metrics`, `Settings`, and `Extra Options` tabs
- `Sender` and `Receiver` session modes in the app
- Manual IP connection for sender mode
- Automatic receiver discovery in the GUI
- Persisted local settings for target IP, control port, audio port, and theme
- Persisted local settings for bind IP and output device selection
- Sender capture mode selection for system audio, microphone, or mixed system-plus-microphone
- Advanced sender capture overrides for specific Linux sources and external virtual devices
- Receiver-side concurrent multi-stream mixing from multiple senders
- Headless sender and receiver commands that run from saved settings without the GUI
- Dark and light mode in the app
- TCP control handshake plus UDP frame streaming
- Stop, disconnect, mute, and graceful window-close shutdown
- CLI fallback commands for transport testing
- A bind-IP setting for receiver mode
- Output device enumeration and selection for receiver playback
- Receiver playback of streamed audio
- Windows system audio capture for sender mode via WASAPI loopback
- Linux system audio capture for sender mode via monitor-source capture (`parec`)

What it does not do yet:

- Per-app routing
- Sample-rate conversion
- Per-stream routing, naming, and mixer controls

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
| Transport | TCP handshake (`Hello` -> `Accept`) and UDP frame path work |
| Sender behavior | Windows sender mode uses WASAPI loopback. Linux sender mode uses a monitor-source capture path on PipeWire/PulseAudio systems. Sender capture can run as system audio, microphone, or system-plus-microphone |
| Receiver behavior | Receiver advertises itself on LAN, accepts multiple concurrent senders, mixes them into one output, and reports frame activity |
| Security | Sessions use encrypted pairing with trusted fingerprint confirmation |
| Session controls | Start, stop, disconnect, and mute are available in the GUI |
| Headless mode | `headless` / `service` sender and receiver commands run without the GUI and reuse saved settings |
| CLI fallback | `listen` and `connect <ip>` still work |

---

## What Is Not Built Yet

These are still planned, not implemented:

- Per-app routing
- Per-stream routing and mixer controls on the receiver
- Sample-rate conversion beyond the current basic resampling path

---

## Stack

| Area | Choice |
| --- | --- |
| Language | Rust |
| UI | Slint |
| Async runtime | Tokio |
| Wire format | JSON control messages (`Hello` / `Accept`) + raw PCM test frames |
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

The current app is a prototype. You can run the GUI or use the CLI fallback for transport testing.

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

CLI fallback:

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
| Linux | Rust toolchain, `parec`, and PulseAudio or PipeWire pulse compatibility |
| Both | `cargo`, `git`, and two machines on the same local network if you want a real network test |

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
- Linux sender mode now captures system audio through a monitor source using `parec`. It can use `VELIN_LINUX_MONITOR` to override the detected monitor source when needed.
- Sender mode can now run as `System`, `Mic`, or `System + Mic`, and the `Extra Options` tab stores those defaults locally.
- External virtual devices can be used by selecting or entering the source/device name exposed by the host audio stack.
- Receiver mode now supports multiple concurrent senders and mixes them into one playback stream.
- The current receiver can play streamed audio on a selected output device.
- Linux is part of the project target, but the current prototype work has primarily been exercised on Windows.
- Receiver mode can bind to `Automatic` (`0.0.0.0`) or a specific local IPv4 address.
- Receiver discovery uses local-network UDP discovery packets, and sender mode also has a manual refresh action.
- Receiver playback currently uses the selected device's default output config. Sample-rate conversion is not implemented yet.
- Windows capture currently expects the default system mix format to be `48 kHz`.
- Linux capture currently expects `parec` plus PulseAudio/PipeWire pulse compatibility to be available, and it also runs at `48 kHz`.

## Troubleshooting

- If Linux cannot connect to a Windows receiver and TCP `49000` times out, check Windows Defender Firewall first.
- If receiver discovery does not find a peer, use the `Refresh` action in sender mode or enter the receiver IP manually.
- If Linux playback is choppy, the current prototype is still missing a real jitter buffer, resampling, and richer receiver-side timing recovery.

---

## License

[MIT](LICENSE)

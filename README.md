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

> Velin is an early desktop prototype for moving audio-related session traffic between machines on the same local network. Right now it provides a GUI, sender/receiver session flow, manual IP connection, saved basic settings, and a working TCP/UDP transport path using generated audio frames.

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

- A Slint desktop GUI with `Session` and `Settings` tabs
- `Sender` and `Receiver` session modes in the app
- Manual IP connection for sender mode
- Persisted local settings for target IP, control port, audio port, and theme
- Dark and light mode in the app
- TCP control handshake plus UDP frame streaming
- Stop handling and graceful window-close shutdown
- CLI fallback commands for transport testing
- A bind-IP setting for receiver mode

What it does not do yet:

- Auto-discovery
- Real system audio capture
- Real playback on the receiver
- Device enumeration or device selection
- Mute control
- Per-app routing

```text
+----------------+      local network       +----------------+
| Sender machine | -----------------------> | Receiver       |
| generated test |   TCP control + UDP      | receives test  |
| frames         |                          | frames         |
+----------------+                          +----------------+
```

---

## What Works Today

| Area | Current behavior |
| --- | --- |
| GUI | Starts by default with a small desktop window |
| Roles | Sender and receiver actions are available in the session tab |
| Connection | Manual IP connect works |
| Settings | Target IP, control port, audio port, and theme are saved locally |
| Transport | TCP handshake (`Hello` -> `Accept`) and UDP frame path work |
| Test signal | Sender currently streams generated dummy PCM frames |
| Receiver behavior | Receiver accepts a connection and reports frame activity |
| CLI fallback | `listen` and `connect <ip>` still work |

---

## What Is Not Built Yet

These are still planned, not implemented:

- LAN auto-discovery
- System audio capture on Windows
- System audio capture on Linux
- Receiver playback to a selected device
- Audio device enumeration
- Local mute while streaming
- Stream diagnostics beyond simple status text
- Trusted pairing or encryption

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
|   |       |-- app.rs
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
- [ ] Automatic peer discovery
- [ ] Real receiver playback
- [ ] Real system audio capture
- [ ] Audio device enumeration
- [ ] Basic connect/disconnect/mute session controls

### Phase 2: Real Audio Routing

- [ ] Play received audio on a selected output device
- [ ] Capture full system audio on Windows
- [ ] Capture full system audio on Linux
- [ ] Improve stream health and latency reporting
- [ ] Remember preferred peers and devices
- [ ] Auto-reconnect

### Phase 3: Advanced Routing

- [ ] Local mute while streaming
- [ ] Specific or per-application audio capture
- [ ] Microphone forwarding
- [ ] Virtual sinks and sources
- [ ] Multi-stream support
- [ ] Encrypted sessions and trusted pairing
- [ ] Headless or service mode

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
- Use `Start Sender` on another machine, with the receiver IP entered

CLI fallback:

```bash
cargo run -p velin-app -- listen
cargo run -p velin-app -- connect 127.0.0.1
```

### Requirements

| Platform | Requirements |
| --- | --- |
| Windows | Rust toolchain |
| Linux | Rust toolchain |
| Both | `cargo`, `git`, and two machines on the same local network if you want a real network test |

---

## Notes

- The current sender uses generated test frames, not live system audio.
- The current receiver logs frame activity through app status updates; it does not play audio yet.
- Linux is part of the project target, but the current prototype work has primarily been exercised on Windows.
- Receiver mode can bind to `Automatic` (`0.0.0.0`) or a specific local IPv4 address.
- The current TCP handshake is minimal and unencrypted. Encryption and trusted pairing are future work.

---

## License

[MIT](LICENSE)

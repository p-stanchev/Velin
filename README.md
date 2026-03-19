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

> Velin is an early desktop prototype for sending system audio between machines on the same local network. Right now it provides a GUI, sender/receiver session flow, LAN receiver discovery with manual IP fallback, saved basic settings, output device selection, and a working TCP/UDP transport path.

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
- Automatic receiver discovery in the GUI
- Persisted local settings for target IP, control port, audio port, and theme
- Persisted local settings for bind IP and output device selection
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
- Stream diagnostics beyond simple status text
- Trusted pairing or encryption
- Sample-rate conversion

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
| Settings | Target IP, bind IP, output device, control port, audio port, and theme are saved locally |
| Transport | TCP handshake (`Hello` -> `Accept`) and UDP frame path work |
| Sender behavior | Windows sender mode uses WASAPI loopback. Linux sender mode uses a monitor-source capture path on PipeWire/PulseAudio systems |
| Receiver behavior | Receiver advertises itself on LAN, accepts a connection, plays streamed audio, and reports frame activity |
| Session controls | Start, stop, disconnect, and mute are available in the GUI |
| CLI fallback | `listen` and `connect <ip>` still work |

---

## What Is Not Built Yet

These are still planned, not implemented:

- Stream diagnostics beyond simple status text
- Trusted pairing or encryption
- Per-app routing

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
- [ ] Improve stream health and latency reporting
- [ ] Remember preferred peers and devices
- [ ] Auto-reconnect

### Phase 3: Advanced Routing

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
- Use `Refresh` in sender mode to look for receivers, or enter the receiver IP manually
- Use `Start Sender` on the other machine once a receiver is selected or entered

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

- Windows sender mode now captures real system audio from the default render endpoint.
- Linux sender mode now captures system audio through a monitor source using `parec`. It can use `VELIN_LINUX_MONITOR` to override the detected monitor source when needed.
- The current receiver can play streamed audio on a selected output device.
- Linux is part of the project target, but the current prototype work has primarily been exercised on Windows.
- Receiver mode can bind to `Automatic` (`0.0.0.0`) or a specific local IPv4 address.
- Receiver discovery uses local-network UDP discovery packets, and sender mode also has a manual refresh action.
- The current TCP handshake is minimal and unencrypted. Encryption and trusted pairing are future work.
- Receiver playback currently uses the selected device's default output config. Sample-rate conversion is not implemented yet.
- Windows capture currently expects the default system mix format to be `48 kHz`.
- Linux capture currently expects `parec` plus PulseAudio/PipeWire pulse compatibility to be available, and it also runs at `48 kHz`.

---

## License

[MIT](LICENSE)

<div align="center">

# Velin

LAN audio routing for Windows and Linux

[![Rust](https://img.shields.io/badge/Rust-systems-orange?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![Windows](https://img.shields.io/badge/Windows-supported-0078D6?style=flat-square&logo=windows)](.)
[![Linux](https://img.shields.io/badge/Linux-supported-FCC624?style=flat-square&logo=linux&logoColor=black)](.)
[![Status](https://img.shields.io/badge/status-in%20development-555?style=flat-square)](.)
[![License](https://img.shields.io/badge/license-MIT-111111?style=flat-square)](LICENSE)

</div>

> Velin is a desktop app for sending audio between Windows and Linux machines on the same local network. Each device can run as a source that captures audio or a target that receives and plays it.

<p align="center">
  <a href="#overview">Overview</a> |
  <a href="#how-it-works">How It Works</a> |
  <a href="#core-features">Core Features</a> |
  <a href="#stack">Stack</a> |
  <a href="#workspace-layout">Workspace Layout</a> |
  <a href="#roadmap">Roadmap</a> |
  <a href="#getting-started">Getting Started</a>
</p>

---

## Overview

Velin is built for multi-machine setups where the machine producing sound is not the one connected to your speakers, DAC, or audio interface. The first goal is simple: make it practical to move audio across the same local network with low friction and low latency.

The app is intended to work over both Ethernet and Wi-Fi, with the main focus on same-network use rather than internet streaming.

```text
+----------------+      LAN / local network      +----------------+
| Source machine | ----------------------------> | Target machine |
| capture audio  |     stream + session ctrl    | play audio     |
+----------------+                               +----------------+
```

| At a glance | |
| --- | --- |
| Roles | Source and target |
| Platforms | Windows and Linux |
| Network scope | Same local network, Ethernet or Wi-Fi |
| Discovery | Automatic peer discovery with manual IP fallback |
| Audio direction | Source captures, target plays back |
| Current focus | Low-latency desktop audio transfer |

Typical uses:

- Send Linux workstation audio to speakers connected to a Windows machine
- Send Windows desktop audio to a Linux box with a better DAC
- Keep audio hardware attached to one machine while working from another

---

## How It Works

When the app opens, the device is configured as either a source or a target.

- A `source` captures audio and sends it over the local network
- A `target` receives that audio and plays it on a selected output device
- Peers should be discovered automatically when possible
- If discovery fails, the user can connect manually by entering an IP address

The source should also be able to mute local playback while streaming so audio is not heard from both the source machine and the target at the same time.

---

## Core Features

| Feature | Notes |
| --- | --- |
| Source / target roles | Clear role selection when launching or configuring the app |
| Automatic discovery | Find peers on the same network without manual setup |
| Manual IP connect | Fallback path when discovery is unavailable or unreliable |
| System audio capture | Stream full desktop audio from the source machine |
| Specific audio selection | Support for sending selected audio instead of only full-system output |
| Output device selection | Choose where the target plays the received stream |
| Local mute while streaming | Prevent double playback on the source machine |
| Session controls | Connect, disconnect, mute, and stream status controls |
| Low-latency transport | Prioritize local-network responsiveness over long-haul reliability |

> Note: "specific audio" needs a precise implementation boundary. In practice this may mean per-application audio, selected session audio, or another narrower capture mode.

---

## Stack

| Area | Choice |
| --- | --- |
| Language | Rust |
| UI | Slint |
| Async runtime | Tokio |
| Codec | Opus |
| Windows audio | WASAPI |
| Linux audio | PipeWire |
| Discovery | mDNS / Zeroconf |
| Transport | UDP for audio, TCP for control |
| Config and storage | Serde + local storage |

---

## Workspace Layout

```text
velin/
|-- crates/
|   |-- velin-app          # Desktop app entry point
|   |-- velin-ui           # Native UI layer
|   |-- velin-core         # Shared domain logic and types
|   |-- velin-proto        # Wire protocol and message schema
|   |-- velin-net          # Discovery, sessions, transport
|   |-- velin-codec        # Audio encoding and decoding
|   |-- velin-audio        # Shared audio abstractions
|   |-- velin-audio-win    # Windows backend (WASAPI)
|   |-- velin-audio-linux  # Linux backend (PipeWire)
|   |-- velin-store        # Settings and persistent state
|   |-- velin-service      # Orchestration layer
|   `-- velin-testkit      # Test helpers and fake peers
|-- docs/
|-- assets/
|-- scripts/
`-- examples/
```

---

## Roadmap

### Phase 1: MVP

- [x] Workspace and crate structure
- [ ] Source and target role selection
- [ ] Automatic peer discovery
- [ ] Manual IP connection fallback
- [ ] Audio device enumeration
- [ ] Full system audio capture on Windows and Linux
- [ ] Stream audio to a target and play on a selected output
- [ ] Source-side local mute while streaming
- [ ] Basic session controls: connect, disconnect, mute
- [ ] Basic latency and stream health display

### Phase 2: Usability

- [ ] Remembered peers and preferred devices
- [ ] Auto-reconnect
- [ ] Better stream diagnostics such as packet loss and jitter
- [ ] Cleaner session management and error handling
- [ ] System tray or background mode

### Phase 3: Advanced Routing

- [ ] Specific or per-application audio capture
- [ ] Microphone forwarding
- [ ] Virtual sinks and sources
- [ ] Multi-stream support
- [ ] Encrypted sessions and trusted pairing
- [ ] Headless or service mode
- [ ] Saved routing profiles

---

## Getting Started

Velin is still early in development. Setup and runtime details will change as the capture, transport, and playback pieces are built out.

```bash
git clone https://github.com/p-stanchev/velin.git
cd velin
cargo run -p velin-app
```

### Requirements

| Platform | Requirements |
| --- | --- |
| Windows | Rust toolchain and an audio device |
| Linux | Rust toolchain, PipeWire, and an audio device |
| Both | `cargo`, `git`, and a second machine on the same local network for testing |

---

## Contributing

Contributions are welcome, especially in these areas:

- Discovery and connection flow
- Windows and Linux audio backend work
- Stream transport and latency tuning
- UI and session control design
- Test tooling and fake peers
- Packaging and distribution

Keep changes focused and readable.

---

## License

[MIT](LICENSE)

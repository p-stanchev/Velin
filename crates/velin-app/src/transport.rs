use crate::audio::open_output_device;
use crate::capture::{CaptureMode, start_audio_capture};
use crate::security::{SecurityStore, TrustOutcome, pairing_fingerprint};
use anyhow::{Context, Result, bail};
use chacha20poly1305::aead::AeadInPlace;
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use getrandom::fill as random_fill;
use local_ip_address::list_afinet_netifas;
use std::collections::BTreeMap;
use std::collections::{HashMap, VecDeque};
use std::env;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket, tcp::OwnedReadHalf};
use tokio::sync::{mpsc, watch};
use tokio::time;
use velin_proto::{
    Accept, AudioFrame, CHANNELS, DEFAULT_DISCOVERY_PORT, DiscoveryAnnouncement, DiscoveryPacket,
    Hello, PairingDecision, PairingRequired, frame_samples_per_channel,
};

pub type StatusSink = Arc<dyn Fn(String) + Send + Sync>;
pub type MetricsSink = Arc<dyn Fn(String) + Send + Sync>;
pub type PairingPrompt = Arc<dyn Fn(PairingRequest) -> Result<bool> + Send + Sync>;
const JITTER_BUFFER_MIN_TARGET_FRAMES: usize = 2;
const JITTER_BUFFER_MAX_FRAMES: usize = 160;
const CONSECUTIVE_MISSING_REBUFFER_THRESHOLD: u64 = 4;
const AUDIO_PACKET_HEADER_LEN: usize = 16;

#[cfg(target_os = "linux")]
const JITTER_BUFFER_DEFAULT_TARGET_FRAMES: usize = 6;
#[cfg(not(target_os = "linux"))]
const JITTER_BUFFER_DEFAULT_TARGET_FRAMES: usize = 4;

#[cfg(target_os = "linux")]
const JITTER_BUFFER_MAX_TARGET_FRAMES: usize = 16;
#[cfg(not(target_os = "linux"))]
const JITTER_BUFFER_MAX_TARGET_FRAMES: usize = 12;

#[cfg(target_os = "linux")]
const PLAYBACK_QUEUE_TARGET_FRAMES: usize = 6;
#[cfg(not(target_os = "linux"))]
const PLAYBACK_QUEUE_TARGET_FRAMES: usize = 3;

#[cfg(target_os = "linux")]
const PLAYBACK_QUEUE_LOW_WATER_FRAMES: usize = 3;
#[cfg(not(target_os = "linux"))]
const PLAYBACK_QUEUE_LOW_WATER_FRAMES: usize = 1;

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub target_ip: String,
    pub bind_ip: String,
    pub output_device_name: String,
    pub capture_mode: CaptureMode,
    pub capture_source_name: String,
    pub microphone_device_name: String,
    pub control_port: u16,
    pub audio_port: u16,
}

struct SessionCipher {
    cipher: ChaCha20Poly1305,
}

struct ReceiverStreamState {
    sender_name: String,
    sample_rate_hz: u32,
    session_cipher: SessionCipher,
    buffered_frames: BTreeMap<u64, Vec<i16>>,
    next_play_sequence: Option<u64>,
    jitter_primed: bool,
    jitter_target_frames: usize,
    stable_played_frames: u64,
    depth_samples: u64,
    depth_sum: u64,
    last_good_output: Vec<i16>,
    consecutive_missing_frames: u64,
    received_frames: u64,
    played_frames: u64,
    dropped_frames: u64,
    underrun_frames: u64,
    late_packets: u64,
    reordered_packets: u64,
    missing_packets: u64,
    highest_received_sequence: Option<u64>,
    disconnect_requested: bool,
    last_packet_at: Instant,
}

enum ReceiverControlEvent {
    Connected {
        stream_id: u64,
        sender_name: String,
        sample_rate_hz: u32,
        session_cipher: SessionCipher,
    },
    Disconnected {
        stream_id: u64,
    },
    Status(String),
}

#[derive(Debug, Clone)]
pub struct PairingRequest {
    pub peer_name: String,
    pub fingerprint: String,
    pub role: String,
}

pub async fn run_target(
    config: SessionConfig,
    status: StatusSink,
    metrics: Option<MetricsSink>,
    pairing_prompt: Option<PairingPrompt>,
    mut stop_rx: watch::Receiver<bool>,
    mut mute_rx: watch::Receiver<bool>,
) -> Result<()> {
    let local_public_key = SecurityStore::load_or_create()
        .context("failed to load local pairing identity")?
        .local_identity()?
        .public_key_hex();
    let bind_ip = normalized_bind_ip(&config.bind_ip);
    let control_addr = format!("{bind_ip}:{}", config.control_port);
    let audio_addr = format!("{bind_ip}:{}", config.audio_port);

    let listener = TcpListener::bind(&control_addr)
        .await
        .with_context(|| format!("failed to bind control listener on {control_addr}"))?;
    let audio_socket = UdpSocket::bind(&audio_addr)
        .await
        .with_context(|| format!("failed to bind audio socket on {audio_addr}"))?;
    let (mut player, mut device_name) = open_output_device(&config.output_device_name)
        .context("failed to open playback device")?;
    let discovery_status = Arc::clone(&status);
    let discovery_bind_ip = bind_ip.clone();
    let discovery_control_port = config.control_port;
    let mut discovery_stop_rx = stop_rx.clone();
    tokio::spawn(async move {
        if let Err(error) =
            run_discovery_broadcaster(discovery_bind_ip, discovery_control_port, &mut discovery_stop_rx).await
        {
            discovery_status(format!("Discovery broadcast unavailable. {error}"));
        }
    });

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<ReceiverControlEvent>();
    {
        let status = Arc::clone(&status);
        let pairing_prompt = pairing_prompt.clone();
        let local_public_key = local_public_key.clone();
        let mut accept_stop_rx = stop_rx.clone();
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    result = listener.accept() => result,
                    _ = wait_for_stop(&mut accept_stop_rx) => break,
                };

                let (stream, peer_addr) = match accepted {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = event_tx.send(ReceiverControlEvent::Status(format!(
                            "Failed to accept sender connection. {error:#}"
                        )));
                        continue;
                    }
                };

                let event_tx = event_tx.clone();
                let pairing_prompt = pairing_prompt.clone();
                let local_public_key = local_public_key.clone();
                let status = Arc::clone(&status);
                let stop_rx = accept_stop_rx.clone();
                tokio::spawn(async move {
                    let status_tx = event_tx.clone();
                    if let Err(error) = accept_receiver_stream(
                        stream,
                        peer_addr,
                        &local_public_key,
                        pairing_prompt,
                        status,
                        event_tx,
                        config.audio_port,
                        stop_rx,
                    )
                    .await
                    {
                        let _ = status_tx.send(ReceiverControlEvent::Status(format!(
                            "Receiver stream setup failed. {error:#}"
                        )));
                    }
                });
            }
        });
    }

    status(if bind_ip == "0.0.0.0" {
        let local_ips = local_ipv4_addresses();
        match local_ips.as_slice() {
            [] => format!("Receiver listening on port {}.", config.control_port),
            [only] => format!("Receiver listening on {only}:{}.", config.control_port),
            many => format!(
                "Receiver listening on {}.",
                many.iter()
                    .map(|ip| format!("{ip}:{}", config.control_port))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    } else {
        format!("Receiver listening on {bind_ip}:{}.", config.control_port)
    });
    status(format!("Playback device: {device_name}."));
    status(format!("Playback config: {}.", player.config_summary()));
    emit_metrics(
        &metrics,
        format!(
            "Mode: Receiver\nState: Listening\nBind: {bind_ip}:{}\nPlayback device: {device_name}\nPlayback config: {}",
            config.control_port,
            player.config_summary()
        ),
    );

    let mut output_frame_samples = frame_sample_count(player.sample_rate_hz());
    let mut packet = vec![0_u8; 2048];
    let mut playback_tick = time::interval(Duration::from_millis(2));
    playback_tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    let mut streams = HashMap::<u64, ReceiverStreamState>::new();
    let mut last_restart_attempt = Instant::now() - Duration::from_secs(5);
    let mut metrics_counter = 0_u64;

    loop {
        tokio::select! {
            Some(event) = event_rx.recv() => {
                match event {
                    ReceiverControlEvent::Connected { stream_id, sender_name, sample_rate_hz, session_cipher } => {
                        status(format!("Sender {sender_name} connected as stream #{stream_id}."));
                        streams.insert(stream_id, ReceiverStreamState::new(
                            sender_name,
                            sample_rate_hz,
                            session_cipher,
                            output_frame_samples,
                        ));
                    }
                    ReceiverControlEvent::Disconnected { stream_id } => {
                        if let Some(stream) = streams.get_mut(&stream_id) {
                            stream.disconnect_requested = true;
                            status(format!("Sender {} disconnected.", stream.sender_name));
                        }
                    }
                    ReceiverControlEvent::Status(message) => status(message),
                }
            }
            result = audio_socket.recv_from(&mut packet) => {
                let (len, from) = result.context("failed to receive audio frame")?;

                let Some((stream_id, sequence)) = audio_packet_header(&packet[..len]) else {
                    status(format!("Discarded malformed packet from {from}."));
                    continue;
                };
                let Some(stream) = streams.get_mut(&stream_id) else {
                    continue;
                };
                let Some(frame) = decode_audio_packet_for_stream(&packet[..len], sequence, &stream.session_cipher) else {
                    status(format!("Discarded malformed packet from {from} on stream #{stream_id}."));
                    continue;
                };
                stream.receive_frame(frame, player.sample_rate_hz(), output_frame_samples);
            }
            _ = playback_tick.tick() => {
                if player.take_backend_error() && last_restart_attempt.elapsed() >= Duration::from_secs(1) {
                    last_restart_attempt = Instant::now();
                    status("Playback backend fault detected. Reopening output stream...".to_string());
                    match open_output_device(&config.output_device_name) {
                        Ok((next_player, next_device_name)) => {
                            player = next_player;
                            device_name = next_device_name;
                            output_frame_samples = frame_sample_count(player.sample_rate_hz());
                            for stream in streams.values_mut() {
                                stream.reset_for_output(output_frame_samples);
                            }
                            player.clear_buffer();
                            status(format!("Playback stream reopened on {device_name}."));
                            status(format!("Playback config: {}.", player.config_summary()));
                            emit_metrics(
                                &metrics,
                                format!(
                                    "Mode: Receiver\nState: Recovering\nStreams: {}\nPlayback device: {device_name}\nPlayback config: {}\nAction: Output stream restarted after backend fault",
                                    streams.len(),
                                    player.config_summary()
                                ),
                            );
                        }
                        Err(error) => {
                            status(format!("Playback restart failed. {error:#}"));
                        }
                    }
                }

                let low_water_samples = PLAYBACK_QUEUE_LOW_WATER_FRAMES * output_frame_samples;
                let target_samples = PLAYBACK_QUEUE_TARGET_FRAMES * output_frame_samples;

                if player.buffered_sample_count() > low_water_samples {
                    continue;
                }
                let mut queued_any = false;
                while player.buffered_sample_count() < target_samples {
                    let mixed = mix_receiver_streams(&mut streams, output_frame_samples, &status);
                    let Some(samples) = mixed else {
                        break;
                    };
                    player.push_samples(&samples);
                    queued_any = true;
                }

                streams.retain(|_, stream| {
                    !(stream.disconnect_requested
                        && stream.buffered_frames.is_empty()
                        && !stream.jitter_primed)
                        && stream.last_packet_at.elapsed() < Duration::from_secs(10)
                });

                if queued_any {
                    metrics_counter += 1;
                    if metrics_counter == 1 || metrics_counter % 100 == 0 {
                        emit_metrics(&metrics, receiver_metrics_summary(&streams, &player, &device_name));
                    }
                }
            }
            result = mute_rx.changed() => {
                if result.is_ok() {
                    player.set_muted(*mute_rx.borrow());
                }
            }
            _ = wait_for_stop(&mut stop_rx) => return Ok(()),
        }
    }
}

pub async fn run_source(
    config: SessionConfig,
    status: StatusSink,
    metrics: Option<MetricsSink>,
    pairing_prompt: Option<PairingPrompt>,
    mut stop_rx: watch::Receiver<bool>,
    mute_rx: watch::Receiver<bool>,
) -> Result<()> {
    let mut security =
        SecurityStore::load_or_create().context("failed to load local pairing identity")?;
    let local_identity = security.local_identity()?;
    let local_public_key = local_identity.public_key_hex();
    let stream_id = random_stream_id()?;
    let control_addr = format!("{}:{}", config.target_ip, config.control_port);
    emit_metrics(
        &metrics,
        format!(
            "Mode: Sender\nState: Connecting\nTarget: {control_addr}\nReconnect: waiting for receiver"
        ),
    );
    let mut stream = tokio::select! {
        result = time::timeout(Duration::from_secs(3), TcpStream::connect(&control_addr)) => {
            result
                .context("timed out while connecting to receiver control channel")?
                .with_context(|| format!("failed to connect to target control channel at {control_addr}"))?
        },
        _ = wait_for_stop(&mut stop_rx) => return Ok(()),
    };

    let mut capture = start_audio_capture(
        config.capture_mode,
        &config.capture_source_name,
        &config.microphone_device_name,
    )
    .context("failed to start configured audio capture")?;
    status(format!(
        "Capturing {} at {} Hz.",
        describe_capture_mode(config.capture_mode),
        capture.sample_rate_hz()
    ));
    emit_metrics(
        &metrics,
        format!(
            "Mode: Sender\nState: Connected control channel\nTarget: {control_addr}\nCapture mode: {}\nCapture rate: {} Hz",
            describe_capture_mode(config.capture_mode),
            capture.sample_rate_hz()
        ),
    );

    let hello = Hello {
        source_name: host_name(),
        stream_id,
        sample_rate_hz: capture.sample_rate_hz(),
        channels: CHANNELS,
        identity_public_key: local_public_key.clone(),
    };
    write_json_message(&mut stream, &hello).await?;

    let accept = match tokio::select! {
        result = read_source_response(&mut stream) => result,
        _ = wait_for_stop(&mut stop_rx) => return Ok(()),
    }? {
        SourceResponse::Accept(accept) => {
            verify_or_prompt_pairing(
                &mut security,
                pairing_prompt.as_ref(),
                "receiver",
                &local_public_key,
                &accept.target_name,
                &accept.identity_public_key,
            )?;
            accept
        }
        SourceResponse::PairingRequired(required) => {
            let fingerprint = pairing_fingerprint(&local_public_key, &required.identity_public_key)?;
            let Some(prompt) = pairing_prompt.as_ref() else {
                bail!(
                    "receiver {} is not trusted yet; fingerprint confirmation is required",
                    required.target_name
                );
            };
            let approved = prompt(PairingRequest {
                peer_name: required.target_name.clone(),
                fingerprint: fingerprint.clone(),
                role: "receiver".to_string(),
            })?;
            write_json_message(&mut stream, &PairingDecision { approved }).await?;
            if !approved {
                bail!("pairing rejected for receiver {}", required.target_name);
            }
            let accept: Accept = tokio::select! {
                result = read_json_message(&mut stream) => result,
                _ = wait_for_stop(&mut stop_rx) => return Ok(()),
            }?;
            security.trust_peer(&accept.target_name, &accept.identity_public_key)?;
            status(format!(
                "Trusted receiver {} with fingerprint {}.",
                accept.target_name,
                fingerprint
            ));
            accept
        }
    };
    let session_cipher = SessionCipher::new(
        local_identity
            .derive_session_key(&accept.identity_public_key)
            .context("failed to derive sender session key")?,
    );
    let (mut control_read, control_write) = stream.into_split();
    let audio_addr = format!("{}:{}", config.target_ip, accept.audio_port);

    let audio_socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .context("failed to bind local UDP socket")?;
    audio_socket
        .connect(&audio_addr)
        .await
        .with_context(|| format!("failed to connect audio socket to {audio_addr}"))?;

    status(format!("Connected to receiver {}.", accept.target_name));
    let _control_write = control_write;
    let mute_rx = mute_rx;
    let mut pending_samples = VecDeque::<i16>::new();
    let frame_sample_count = frame_sample_count(capture.sample_rate_hz());
    let mut sequence = 0_u64;
    let send_start = time::Instant::now();

    loop {
        let Some(mut samples) = (tokio::select! {
            chunk = capture.recv() => chunk,
            result = poll_control_channel(&mut control_read) => match result? {
                ControlChannelState::Closed => {
                    status("Receiver disconnected.".to_string());
                    return Ok(());
                }
                ControlChannelState::Alive => continue,
            },
            _ = wait_for_stop(&mut stop_rx) => return Ok(()),
        }) else {
            bail!("system audio capture ended unexpectedly");
        };

        if *mute_rx.borrow() {
            samples.fill(0);
        }

        pending_samples.extend(samples);

        while pending_samples.len() >= frame_sample_count {
            let mut frame_samples = Vec::with_capacity(frame_sample_count);
            for _ in 0..frame_sample_count {
                if let Some(sample) = pending_samples.pop_front() {
                    frame_samples.push(sample);
                }
            }

            let frame = AudioFrame {
                sequence,
                samples: frame_samples,
            };
            let encoded = encode_audio_packet(stream_id, &frame, &session_cipher);
            let scheduled_send = send_start + frame_duration_for(sequence, capture.sample_rate_hz());
            if scheduled_send > time::Instant::now() {
                tokio::select! {
                    _ = time::sleep_until(scheduled_send) => {}
                    _ = wait_for_stop(&mut stop_rx) => return Ok(()),
                }
            }
            let sent = tokio::select! {
                result = audio_socket.send(&encoded) => result.context("failed to send audio frame")?,
                _ = wait_for_stop(&mut stop_rx) => return Ok(()),
            };

            if sent != encoded.len() {
                bail!("short UDP send: expected {}, sent {sent}", encoded.len());
            }

            if sequence == 0 || sequence % 100 == 0 {
                status(format!("Sent frame {sequence}."));
                let seconds = send_start.elapsed().as_secs_f32();
                let pending_frame_count = pending_samples.len() / frame_sample_count.max(1);
                let pending_latency_ms =
                    ((pending_samples.len() as f32 / CHANNELS as usize as f32) / capture.sample_rate_hz() as f32 * 1000.0)
                        .round() as u32;
                emit_metrics(
                    &metrics,
                    format!(
                        "Mode: Sender\nState: Streaming\nReceiver: {}\nCapture mode: {}\nCapture rate: {} Hz\nFrames sent: {sequence}\nPending capture queue: {} frames\nPending capture latency: {} ms\nUptime: {:.1}s",
                        accept.target_name,
                        describe_capture_mode(config.capture_mode),
                        capture.sample_rate_hz(),
                        pending_frame_count,
                        pending_latency_ms,
                        seconds
                    ),
                );
            }

            sequence += 1;
        }
    }

    #[allow(unreachable_code)]
    Ok(())
}

fn describe_capture_mode(mode: CaptureMode) -> &'static str {
    match mode {
        CaptureMode::System => "system audio",
        CaptureMode::Microphone => "microphone",
        CaptureMode::SystemPlusMicrophone => "system + microphone",
    }
}

pub async fn run_source_with_reconnect(
    config: SessionConfig,
    status: StatusSink,
    metrics: Option<MetricsSink>,
    pairing_prompt: Option<PairingPrompt>,
    mut stop_rx: watch::Receiver<bool>,
    mute_rx: watch::Receiver<bool>,
) -> Result<()> {
    let mut attempt = 0_u32;
    let mut delay_seconds = 2_u64;

    loop {
        let result = run_source(
            config.clone(),
            Arc::clone(&status),
            metrics.clone(),
            pairing_prompt.clone(),
            stop_rx.clone(),
            mute_rx.clone(),
        )
        .await;

        if *stop_rx.borrow() {
            return Ok(());
        }

        attempt += 1;
        let reconnect_message = format!(
            "Mode: Sender\nState: Reconnecting\nTarget: {}:{}\nNext attempt: {} in {}s",
            config.target_ip,
            config.control_port,
            attempt,
            delay_seconds
        );
        emit_metrics(&metrics, reconnect_message);

        match result {
            Ok(()) => status(format!(
                "Receiver disconnected. Reconnecting in {}s (attempt {}).",
                delay_seconds, attempt
            )),
            Err(error) => status(format!(
                "Sender session interrupted. Reconnecting in {}s (attempt {}). {}",
                delay_seconds,
                attempt,
                describe_short_error(&error)
            )),
        }

        tokio::select! {
            _ = time::sleep(Duration::from_secs(delay_seconds)) => {}
            _ = wait_for_stop(&mut stop_rx) => return Ok(()),
        }

        delay_seconds = (delay_seconds + 1).min(5);
    }
}

async fn run_discovery_broadcaster(
    bind_ip: String,
    control_port: u16,
    stop_rx: &mut watch::Receiver<bool>,
) -> Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .context("failed to bind discovery broadcast socket")?;
    socket
        .set_broadcast(true)
        .context("failed to enable broadcast on discovery socket")?;

    let addresses = advertised_ipv4_addresses_for(&bind_ip);
    if addresses.is_empty() {
        return Ok(());
    }

    let announcement = DiscoveryPacket::Announcement(DiscoveryAnnouncement {
        machine_name: host_name(),
        control_port,
        addresses,
    });
    let payload = serde_json::to_vec(&announcement).context("failed to encode discovery packet")?;
    let destination = format!("255.255.255.255:{DEFAULT_DISCOVERY_PORT}");
    let mut ticker = time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let _ = socket.send_to(&payload, &destination).await;
            }
            _ = wait_for_stop(stop_rx) => return Ok(()),
        }
    }
}

fn frame_duration_for(sequence: u64, sample_rate_hz: u32) -> Duration {
    let samples_per_channel = frame_samples_per_channel(sample_rate_hz) as f64;
    let seconds = (sequence as f64 * samples_per_channel) / sample_rate_hz as f64;
    Duration::from_secs_f64(seconds)
}

fn emit_metrics(metrics: &Option<MetricsSink>, message: String) {
    if let Some(sink) = metrics {
        sink(message);
    }
}

fn describe_short_error(error: &anyhow::Error) -> String {
    let text = format!("{error:#}");
    text.lines().next().unwrap_or("unknown error").to_string()
}

fn frame_sample_count(sample_rate_hz: u32) -> usize {
    frame_samples_per_channel(sample_rate_hz) * CHANNELS as usize
}

impl ReceiverStreamState {
    fn new(
        sender_name: String,
        sample_rate_hz: u32,
        session_cipher: SessionCipher,
        output_frame_samples: usize,
    ) -> Self {
        Self {
            sender_name,
            sample_rate_hz,
            session_cipher,
            buffered_frames: BTreeMap::new(),
            next_play_sequence: None,
            jitter_primed: false,
            jitter_target_frames: JITTER_BUFFER_DEFAULT_TARGET_FRAMES,
            stable_played_frames: 0,
            depth_samples: 0,
            depth_sum: 0,
            last_good_output: vec![0; output_frame_samples],
            consecutive_missing_frames: 0,
            received_frames: 0,
            played_frames: 0,
            dropped_frames: 0,
            underrun_frames: 0,
            late_packets: 0,
            reordered_packets: 0,
            missing_packets: 0,
            highest_received_sequence: None,
            disconnect_requested: false,
            last_packet_at: Instant::now(),
        }
    }

    fn reset_for_output(&mut self, output_frame_samples: usize) {
        self.last_good_output = vec![0; output_frame_samples];
        self.jitter_primed = false;
        self.consecutive_missing_frames = 0;
    }

    fn receive_frame(&mut self, frame: AudioFrame, output_rate_hz: u32, output_frame_samples: usize) {
        self.received_frames += 1;
        self.last_packet_at = Instant::now();
        if self.next_play_sequence.is_none() {
            self.next_play_sequence = Some(frame.sequence);
        }

        if let Some(play_sequence) = self.next_play_sequence {
            if frame.sequence < play_sequence {
                self.late_packets += 1;
                return;
            }
        }

        if let Some(highest) = self.highest_received_sequence {
            if frame.sequence < highest {
                self.reordered_packets += 1;
            }
        }
        self.highest_received_sequence = Some(
            self.highest_received_sequence
                .map_or(frame.sequence, |value| value.max(frame.sequence)),
        );

        if let Some(play_sequence) = self.next_play_sequence {
            if frame.sequence + JITTER_BUFFER_MAX_FRAMES as u64 <= play_sequence {
                self.dropped_frames += 1;
                return;
            }
        }

        let resampled = resample_stereo_i16(
            &frame.samples,
            self.sample_rate_hz,
            output_rate_hz,
            CHANNELS as usize,
        );
        self.buffered_frames.entry(frame.sequence).or_insert(resampled);

        while self.buffered_frames.len() > JITTER_BUFFER_MAX_FRAMES {
            if let Some((&oldest, _)) = self.buffered_frames.first_key_value() {
                self.buffered_frames.remove(&oldest);
                self.dropped_frames += 1;
            } else {
                break;
            }
        }

        if !self.jitter_primed && self.buffered_frames.len() >= self.jitter_target_frames {
            if let Some((&first_sequence, _)) = self.buffered_frames.first_key_value() {
                self.next_play_sequence = Some(first_sequence);
            }
            self.jitter_primed = true;
        }

        if self.last_good_output.len() != output_frame_samples {
            self.last_good_output.resize(output_frame_samples, 0);
        }

        self.depth_sum += self.buffered_frames.len() as u64;
        self.depth_samples += 1;
    }
}

impl SessionCipher {
    fn new(key: [u8; 32]) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new((&key).into()),
        }
    }

    fn nonce_for(sequence: u64) -> Nonce {
        let mut bytes = [0_u8; 12];
        bytes[4..].copy_from_slice(&sequence.to_le_bytes());
        *Nonce::from_slice(&bytes)
    }
}

fn encode_audio_packet(stream_id: u64, frame: &AudioFrame, session_cipher: &SessionCipher) -> Vec<u8> {
    let mut plaintext = Vec::with_capacity(2 + frame.samples.len() * 2);
    let sample_count = frame.samples.len() as u16;
    plaintext.extend_from_slice(&sample_count.to_le_bytes());
    for sample in &frame.samples {
        plaintext.extend_from_slice(&sample.to_le_bytes());
    }

    let nonce = SessionCipher::nonce_for(frame.sequence);
    let mut aad = [0_u8; AUDIO_PACKET_HEADER_LEN];
    aad[..8].copy_from_slice(&stream_id.to_le_bytes());
    aad[8..].copy_from_slice(&frame.sequence.to_le_bytes());
    session_cipher
        .cipher
        .encrypt_in_place(&nonce, &aad, &mut plaintext)
        .expect("audio packet encryption should not fail");

    let mut bytes = Vec::with_capacity(AUDIO_PACKET_HEADER_LEN + plaintext.len());
    bytes.extend_from_slice(&stream_id.to_le_bytes());
    bytes.extend_from_slice(&frame.sequence.to_le_bytes());
    bytes.extend_from_slice(&plaintext);
    bytes
}

fn audio_packet_header(bytes: &[u8]) -> Option<(u64, u64)> {
    if bytes.len() < AUDIO_PACKET_HEADER_LEN {
        return None;
    }

    let stream_id = u64::from_le_bytes(bytes[..8].try_into().ok()?);
    let sequence = u64::from_le_bytes(bytes[8..AUDIO_PACKET_HEADER_LEN].try_into().ok()?);
    Some((stream_id, sequence))
}

fn decode_audio_packet_for_stream(
    bytes: &[u8],
    sequence: u64,
    session_cipher: &SessionCipher,
) -> Option<AudioFrame> {
    if bytes.len() < AUDIO_PACKET_HEADER_LEN {
        return None;
    }

    let nonce = SessionCipher::nonce_for(sequence);
    let mut ciphertext = bytes[AUDIO_PACKET_HEADER_LEN..].to_vec();
    let mut aad = [0_u8; AUDIO_PACKET_HEADER_LEN];
    aad.copy_from_slice(&bytes[..AUDIO_PACKET_HEADER_LEN]);
    session_cipher
        .cipher
        .decrypt_in_place(&nonce, &aad, &mut ciphertext)
        .ok()?;

    if ciphertext.len() < 2 {
        return None;
    }

    let sample_count = u16::from_le_bytes(ciphertext[..2].try_into().ok()?) as usize;
    let payload = &ciphertext[2..];
    if payload.len() != sample_count * 2 {
        return None;
    }

    let mut samples = Vec::with_capacity(sample_count);
    for chunk in payload.chunks_exact(2) {
        samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }

    Some(AudioFrame { sequence, samples })
}

async fn accept_receiver_stream(
    mut stream: TcpStream,
    peer_addr: std::net::SocketAddr,
    local_public_key: &str,
    pairing_prompt: Option<PairingPrompt>,
    status: StatusSink,
    event_tx: mpsc::UnboundedSender<ReceiverControlEvent>,
    audio_port: u16,
    mut stop_rx: watch::Receiver<bool>,
) -> Result<()> {
    let mut security =
        SecurityStore::load_or_create().context("failed to load local pairing identity")?;
    let local_identity = security.local_identity()?;
    let hello: Hello = read_json_message(&mut stream).await?;

    match security.verify_peer(&hello.source_name, &hello.identity_public_key)? {
        TrustOutcome::Trusted => {}
        TrustOutcome::Untrusted { machine_name: peer_name, .. } => {
            let Some(prompt) = pairing_prompt.as_ref() else {
                bail!("sender {peer_name} is not trusted yet; fingerprint confirmation is required");
            };
            let fingerprint = pairing_fingerprint(local_public_key, &hello.identity_public_key)?;
            let pairing_required = PairingRequired {
                target_name: host_name(),
                identity_public_key: local_public_key.to_string(),
            };
            write_json_message(&mut stream, &pairing_required).await?;
            let approved = prompt(PairingRequest {
                peer_name: peer_name.clone(),
                fingerprint: fingerprint.clone(),
                role: "sender".to_string(),
            })?;
            if !approved {
                bail!("pairing rejected for sender {peer_name}");
            }
            let decision: PairingDecision = read_json_message(&mut stream).await?;
            if !decision.approved {
                bail!("sender {peer_name} rejected the fingerprint confirmation");
            }
            security.trust_peer(&peer_name, &hello.identity_public_key)?;
            status(format!("Trusted sender {peer_name} with fingerprint {fingerprint}."));
        }
    }

    let session_cipher = SessionCipher::new(
        local_identity
            .derive_session_key(&hello.identity_public_key)
            .context("failed to derive receiver session key")?,
    );

    let accept = Accept {
        target_name: host_name(),
        audio_port,
        identity_public_key: local_public_key.to_string(),
    };
    write_json_message(&mut stream, &accept).await?;
    let stream_id = hello.stream_id;
    let sender_name = hello.source_name.clone();
    let sample_rate_hz = hello.sample_rate_hz;

    let _ = event_tx.send(ReceiverControlEvent::Connected {
        stream_id,
        sender_name: sender_name.clone(),
        sample_rate_hz,
        session_cipher,
    });
    status(format!("Sender {sender_name} connected from {peer_addr}."));

    let (mut control_read, control_write) = stream.into_split();
    tokio::spawn(async move {
        let _control_write = control_write;
        loop {
            tokio::select! {
                _ = wait_for_stop(&mut stop_rx) => {
                    let _ = event_tx.send(ReceiverControlEvent::Disconnected { stream_id });
                    break;
                }
                result = poll_control_channel(&mut control_read) => match result {
                    Ok(ControlChannelState::Alive) => time::sleep(Duration::from_millis(50)).await,
                    Ok(ControlChannelState::Closed) | Err(_) => {
                        let _ = event_tx.send(ReceiverControlEvent::Disconnected { stream_id });
                        break;
                    }
                }
            }
        }
    });

    Ok(())
}

fn mix_receiver_streams(
    streams: &mut HashMap<u64, ReceiverStreamState>,
    output_frame_samples: usize,
    status: &StatusSink,
) -> Option<Vec<i16>> {
    let mut mix_accum = vec![0_i32; output_frame_samples];
    let mut active_streams = 0_i32;
    let stream_ids = streams.keys().copied().collect::<Vec<_>>();

    for stream_id in stream_ids {
        let Some(stream) = streams.get_mut(&stream_id) else {
            continue;
        };
        let Some(samples) = next_receiver_stream_samples(stream, output_frame_samples, status) else {
            continue;
        };

        for (index, sample) in samples.into_iter().enumerate() {
            mix_accum[index] += sample as i32;
        }
        active_streams += 1;
    }

    if active_streams == 0 {
        return None;
    }

    Some(
        mix_accum
            .into_iter()
            .map(|value| (value / active_streams).clamp(i16::MIN as i32, i16::MAX as i32) as i16)
            .collect(),
    )
}

fn next_receiver_stream_samples(
    stream: &mut ReceiverStreamState,
    output_frame_samples: usize,
    status: &StatusSink,
) -> Option<Vec<i16>> {
    if !stream.jitter_primed {
        if stream.buffered_frames.len() >= stream.jitter_target_frames {
            if let Some((&first_sequence, _)) = stream.buffered_frames.first_key_value() {
                stream.next_play_sequence = Some(first_sequence);
                stream.jitter_primed = true;
                status(format!(
                    "Jitter buffer primed with {} frames for {}.",
                    stream.buffered_frames.len(),
                    stream.sender_name
                ));
            }
        } else {
            return None;
        }
    }

    let play_sequence = stream.next_play_sequence.get_or_insert(0);
    let output = if let Some(samples) = stream.buffered_frames.remove(play_sequence) {
        stream.last_good_output.clone_from(&samples);
        stream.consecutive_missing_frames = 0;
        samples
    } else if let Some((&first_available, _)) = stream.buffered_frames.first_key_value() {
        if first_available > *play_sequence {
            stream.missing_packets += 1;
            stream.underrun_frames += 1;
            stream.consecutive_missing_frames += 1;
            if stream.jitter_target_frames < JITTER_BUFFER_MAX_TARGET_FRAMES {
                stream.jitter_target_frames = (stream.jitter_target_frames + 1).min(JITTER_BUFFER_MAX_TARGET_FRAMES);
            }
            stream.stable_played_frames = 0;

            if stream.consecutive_missing_frames >= CONSECUTIVE_MISSING_REBUFFER_THRESHOLD {
                stream.jitter_primed = false;
                stream.consecutive_missing_frames = 0;
                stream.next_play_sequence = stream.buffered_frames.first_key_value().map(|(&sequence, _)| sequence);
                status(format!(
                    "Receiver rebuffering {} after packet loss. New target {} frames.",
                    stream.sender_name,
                    stream.jitter_target_frames
                ));
                return None;
            }

            if stream.consecutive_missing_frames <= 2 {
                stream.last_good_output.clone()
            } else {
                vec![0_i16; output_frame_samples]
            }
        } else {
            return None;
        }
    } else {
        stream.underrun_frames += 1;
        stream.consecutive_missing_frames += 1;
        if stream.consecutive_missing_frames >= CONSECUTIVE_MISSING_REBUFFER_THRESHOLD {
            stream.jitter_primed = false;
            stream.consecutive_missing_frames = 0;
            return None;
        }
        if stream.consecutive_missing_frames <= 2 && !stream.last_good_output.is_empty() {
            stream.last_good_output.clone()
        } else {
            vec![0_i16; output_frame_samples]
        }
    };

    *play_sequence += 1;
    stream.played_frames += 1;
    stream.stable_played_frames += 1;
    if stream.stable_played_frames >= 250
        && stream.jitter_target_frames > JITTER_BUFFER_MIN_TARGET_FRAMES
        && stream.buffered_frames.len() >= stream.jitter_target_frames
    {
        stream.jitter_target_frames -= 1;
        stream.stable_played_frames = 0;
    }

    Some(output)
}

fn receiver_metrics_summary(
    streams: &HashMap<u64, ReceiverStreamState>,
    player: &crate::audio::OutputPlayer,
    device_name: &str,
) -> String {
    let sender_summary = if streams.is_empty() {
        "none".to_string()
    } else {
        streams
            .values()
            .map(|stream| stream.sender_name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let total_network_frames: usize = streams.values().map(|stream| stream.buffered_frames.len()).sum();
    let total_received: u64 = streams.values().map(|stream| stream.received_frames).sum();
    let total_played: u64 = streams.values().map(|stream| stream.played_frames).sum();
    let total_missing: u64 = streams.values().map(|stream| stream.missing_packets).sum();
    let total_late: u64 = streams.values().map(|stream| stream.late_packets).sum();
    let total_reordered: u64 = streams.values().map(|stream| stream.reordered_packets).sum();
    let total_dropped: u64 = streams.values().map(|stream| stream.dropped_frames).sum();
    let total_underruns: u64 = streams.values().map(|stream| stream.underrun_frames).sum();
    let audio_queue_frames = player.buffered_sample_count() / frame_sample_count(player.sample_rate_hz()).max(1);

    format!(
        "Mode: Receiver\nState: Streaming\nActive streams: {}\nSenders: {}\nPlayback device: {}\nFrames: received {}, played {}\nLatency estimate: {} ms total ({} ms network + {} ms audio)\nNetwork buffer: {} frames\nAudio queue: {} frames\nLoss: missing {}, late {}, reordered {}, dropped {}, underruns {}",
        streams.len(),
        sender_summary,
        device_name,
        total_received,
        total_played,
        total_network_frames * 10 + audio_queue_frames * 10,
        total_network_frames * 10,
        audio_queue_frames * 10,
        total_network_frames,
        audio_queue_frames,
        total_missing,
        total_late,
        total_reordered,
        total_dropped,
        total_underruns,
    )
}

fn random_stream_id() -> Result<u64> {
    let mut bytes = [0_u8; 8];
    random_fill(&mut bytes).map_err(|error| anyhow::anyhow!("failed to generate stream id: {error}"))?;
    Ok(u64::from_le_bytes(bytes))
}

fn resample_stereo_i16(
    input: &[i16],
    input_rate_hz: u32,
    output_rate_hz: u32,
    output_channels: usize,
) -> Vec<i16> {
    if input.is_empty() {
        return Vec::new();
    }

    let input_channels = CHANNELS as usize;
    let input_frames = input.len() / input_channels;
    if input_frames == 0 {
        return Vec::new();
    }

    let output_frames = ((input_frames as u64 * output_rate_hz as u64) / input_rate_hz as u64)
        .max(1) as usize;

    let mut output = Vec::with_capacity(output_frames * output_channels.max(1));
    let ratio = input_rate_hz as f32 / output_rate_hz as f32;

    for out_index in 0..output_frames {
        let position = out_index as f32 * ratio;
        let left_index = position.floor() as usize;
        let right_index = left_index.min(input_frames.saturating_sub(1));
        let next_index = (right_index + 1).min(input_frames.saturating_sub(1));
        let frac = position - left_index as f32;

        let left_a = input[right_index * input_channels] as f32;
        let right_a = input[right_index * input_channels + 1] as f32;
        let left_b = input[next_index * input_channels] as f32;
        let right_b = input[next_index * input_channels + 1] as f32;

        let left = (left_a + (left_b - left_a) * frac).round() as i16;
        let right = (right_a + (right_b - right_a) * frac).round() as i16;

        match output_channels {
            0 => {}
            1 => output.push(((left as i32 + right as i32) / 2) as i16),
            _ => {
                output.push(left);
                output.push(right);
                for extra_channel in 2..output_channels {
                    output.push(if extra_channel % 2 == 0 { left } else { right });
                }
            }
        }
    }

    output
}

async fn wait_for_stop(stop_rx: &mut watch::Receiver<bool>) {
    loop {
        if *stop_rx.borrow() {
            return;
        }
        if stop_rx.changed().await.is_err() {
            return;
        }
    }
}

enum ControlChannelState {
    Alive,
    Closed,
}

enum SourceResponse {
    Accept(Accept),
    PairingRequired(PairingRequired),
}

async fn poll_control_channel(stream: &mut OwnedReadHalf) -> Result<ControlChannelState> {
    match time::timeout(Duration::from_millis(5), stream.read_u8()).await {
        Ok(Ok(_)) => bail!("receiver control channel sent unexpected data"),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(ControlChannelState::Closed),
        Ok(Err(error)) => Err(error).context("receiver control channel failed"),
        Err(_) => Ok(ControlChannelState::Alive),
    }
}

async fn write_json_message<T>(stream: &mut TcpStream, value: &T) -> Result<()>
where
    T: serde::Serialize,
{
    let body = serde_json::to_vec(value).context("failed to serialize control message")?;
    let len = u32::try_from(body.len()).context("control message too large")?;
    stream
        .write_u32_le(len)
        .await
        .context("failed to write control length")?;
    stream
        .write_all(&body)
        .await
        .context("failed to write control payload")?;
    Ok(())
}

async fn read_json_message<T>(stream: &mut TcpStream) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let len = stream
        .read_u32_le()
        .await
        .context("failed to read control length")?;
    let mut body = vec![0_u8; len as usize];
    stream
        .read_exact(&mut body)
        .await
        .context("failed to read control payload")?;
    serde_json::from_slice(&body).context("failed to decode control message")
}

async fn read_source_response(stream: &mut TcpStream) -> Result<SourceResponse> {
    let len = stream
        .read_u32_le()
        .await
        .context("failed to read control length")?;
    let mut body = vec![0_u8; len as usize];
    stream
        .read_exact(&mut body)
        .await
        .context("failed to read control payload")?;

    let value: serde_json::Value =
        serde_json::from_slice(&body).context("failed to decode control message")?;
    if value.get("audio_port").is_some() {
        return serde_json::from_value(value)
            .map(SourceResponse::Accept)
            .context("failed to decode accept message");
    }
    if value.get("identity_public_key").is_some() && value.get("target_name").is_some() {
        return serde_json::from_value(value)
            .map(SourceResponse::PairingRequired)
            .context("failed to decode pairing prompt");
    }

    bail!("unexpected control message during source handshake")
}

fn verify_or_prompt_pairing(
    security: &mut SecurityStore,
    pairing_prompt: Option<&PairingPrompt>,
    role: &str,
    local_public_key: &str,
    peer_name: &str,
    peer_public_key: &str,
) -> Result<()> {
    match security.verify_peer(peer_name, peer_public_key)? {
        TrustOutcome::Trusted => Ok(()),
        TrustOutcome::Untrusted { machine_name, .. } => {
            let Some(prompt) = pairing_prompt else {
                bail!("{role} {machine_name} is not trusted yet; fingerprint confirmation is required");
            };
            let fingerprint = pairing_fingerprint(local_public_key, peer_public_key)?;
            let approved = prompt(PairingRequest {
                peer_name: machine_name.clone(),
                fingerprint: fingerprint.clone(),
                role: role.to_string(),
            })?;
            if !approved {
                bail!("pairing rejected for {role} {machine_name}");
            }
            security.trust_peer(&machine_name, peer_public_key)?;
            Ok(())
        }
    }
}

fn host_name() -> String {
    windows_host_name()
        .or_else(unix_host_name)
        .or_else(env_host_name)
        .unwrap_or_else(|| "unknown".to_string())
}

fn env_host_name() -> Option<String> {
    env::var("COMPUTERNAME")
        .ok()
        .or_else(|| env::var("HOSTNAME").ok())
        .and_then(|value| {
            let trimmed = value.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        })
}

#[cfg(target_os = "windows")]
fn windows_host_name() -> Option<String> {
    env::var("COMPUTERNAME").ok().and_then(|value| {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

#[cfg(not(target_os = "windows"))]
fn windows_host_name() -> Option<String> {
    None
}

#[cfg(unix)]
fn unix_host_name() -> Option<String> {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .and_then(|value| {
            let trimmed = value.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        })
}

#[cfg(not(unix))]
fn unix_host_name() -> Option<String> {
    None
}

pub fn local_ipv4_summary() -> String {
    let local_ips = local_ipv4_addresses();
    if local_ips.is_empty() {
        "Local IP unavailable".to_string()
    } else {
        local_ips.join("  |  ")
    }
}

fn normalized_bind_ip(bind_ip: &str) -> String {
    let trimmed = bind_ip.trim();
    if trimmed.is_empty() {
        "0.0.0.0".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn advertised_ipv4_addresses_for(bind_ip: &str) -> Vec<String> {
    if bind_ip != "0.0.0.0" {
        return vec![bind_ip.to_string()];
    }

    local_ipv4_addresses()
}

pub fn local_primary_ipv4() -> String {
    local_ipv4_addresses()
        .into_iter()
        .next()
        .unwrap_or_else(|| "Unavailable".to_string())
}

pub fn local_machine_name() -> String {
    host_name()
}

pub fn local_ipv4_addresses() -> Vec<String> {
    let mut ips = Vec::new();

    if let Ok(netifs) = list_afinet_netifas() {
        for (_name, ip) in netifs {
            if let IpAddr::V4(ipv4) = ip {
                if !ipv4.is_loopback()
                    && !ipv4.is_link_local()
                    && ipv4.is_private()
                    && !ips.iter().any(|value| value == &ipv4.to_string())
                {
                    ips.push(ipv4.to_string());
                }
            }
        }
    }

    ips.sort();
    ips
}

use crate::audio::open_output_device;
use crate::capture::start_system_audio_capture;
use anyhow::{Context, Result, bail};
use local_ip_address::list_afinet_netifas;
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::env;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket, tcp::OwnedReadHalf};
use tokio::sync::watch;
use tokio::time;
use velin_proto::{
    Accept, AudioFrame, CHANNELS, DEFAULT_DISCOVERY_PORT, DiscoveryAnnouncement, DiscoveryPacket,
    Hello,
};

pub type StatusSink = Arc<dyn Fn(String) + Send + Sync>;
pub type MetricsSink = Arc<dyn Fn(String) + Send + Sync>;
const JITTER_BUFFER_MIN_TARGET_FRAMES: usize = 6;
const JITTER_BUFFER_DEFAULT_TARGET_FRAMES: usize = 12;
const JITTER_BUFFER_MAX_FRAMES: usize = 160;
const JITTER_BUFFER_MAX_TARGET_FRAMES: usize = 40;
const CONSECUTIVE_MISSING_REBUFFER_THRESHOLD: u64 = 4;

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub target_ip: String,
    pub bind_ip: String,
    pub output_device_name: String,
    pub control_port: u16,
    pub audio_port: u16,
}

pub async fn run_target(
    config: SessionConfig,
    status: StatusSink,
    metrics: Option<MetricsSink>,
    mut stop_rx: watch::Receiver<bool>,
    mut mute_rx: watch::Receiver<bool>,
) -> Result<()> {
    let bind_ip = normalized_bind_ip(&config.bind_ip);
    let control_addr = format!("{bind_ip}:{}", config.control_port);
    let audio_addr = format!("{bind_ip}:{}", config.audio_port);

    let listener = TcpListener::bind(&control_addr)
        .await
        .with_context(|| format!("failed to bind control listener on {control_addr}"))?;
    let audio_socket = UdpSocket::bind(&audio_addr)
        .await
        .with_context(|| format!("failed to bind audio socket on {audio_addr}"))?;
    let (player, device_name) = open_output_device(&config.output_device_name)
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

    let (mut stream, peer_addr) = tokio::select! {
        result = listener.accept() => result.context("failed to accept source")?,
        _ = wait_for_stop(&mut stop_rx) => return Ok(()),
    };
    let hello: Hello = read_json_message(&mut stream).await?;

    status(format!("Sender {} connected from {peer_addr}.", hello.source_name));
    emit_metrics(
        &metrics,
        format!(
            "Mode: Receiver\nState: Connected\nSender: {}\nSender sample rate: {} Hz\nPlayback device: {device_name}\nPlayback config: {}",
            hello.source_name,
            hello.sample_rate_hz,
            player.config_summary()
        ),
    );

    let accept = Accept {
        target_name: host_name(),
        audio_port: config.audio_port,
    };
    write_json_message(&mut stream, &accept).await?;

    let output_frame_samples = frame_sample_count(player.sample_rate_hz());
    let mut packet = vec![0_u8; 2048];
    let mut received_frames = 0_u64;
    let started = Instant::now();
    let mut playback_tick = time::interval(Duration::from_millis(2));
    playback_tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    let mut buffered_frames = BTreeMap::<u64, Vec<i16>>::new();
    let mut next_play_sequence = None;
    let mut jitter_primed = false;
    let silence_frame = vec![0_i16; output_frame_samples];
    let mut played_frames = 0_u64;
    let mut dropped_frames = 0_u64;
    let mut underrun_frames = 0_u64;
    let mut late_packets = 0_u64;
    let mut reordered_packets = 0_u64;
    let mut missing_packets = 0_u64;
    let mut highest_received_sequence = None;
    let mut jitter_target_frames = JITTER_BUFFER_DEFAULT_TARGET_FRAMES;
    let mut stable_played_frames = 0_u64;
    let mut depth_samples = 0_u64;
    let mut depth_sum = 0_u64;
    let mut last_good_output = silence_frame.clone();
    let mut consecutive_missing_frames = 0_u64;

    loop {
        tokio::select! {
            result = audio_socket.recv_from(&mut packet) => {
                let (len, from) = result.context("failed to receive audio frame")?;

                let Some(frame) = AudioFrame::decode(&packet[..len]) else {
                    status(format!("Discarded malformed packet from {from}."));
                    continue;
                };

                received_frames += 1;
                if next_play_sequence.is_none() {
                    next_play_sequence = Some(frame.sequence);
                }

                if let Some(play_sequence) = next_play_sequence {
                    if frame.sequence < play_sequence {
                        late_packets += 1;
                        continue;
                    }
                }

                if let Some(highest) = highest_received_sequence {
                    if frame.sequence < highest {
                        reordered_packets += 1;
                    }
                }
                highest_received_sequence = Some(highest_received_sequence.map_or(frame.sequence, |value| value.max(frame.sequence)));

                if let Some(play_sequence) = next_play_sequence {
                    if frame.sequence + JITTER_BUFFER_MAX_FRAMES as u64 <= play_sequence {
                        dropped_frames += 1;
                        continue;
                    }
                }

                let resampled = resample_stereo_i16(
                    &frame.samples,
                    hello.sample_rate_hz,
                    player.sample_rate_hz(),
                    CHANNELS as usize,
                );
                buffered_frames.entry(frame.sequence).or_insert(resampled);

                while buffered_frames.len() > JITTER_BUFFER_MAX_FRAMES {
                    if let Some((&oldest, _)) = buffered_frames.first_key_value() {
                        buffered_frames.remove(&oldest);
                        dropped_frames += 1;
                    } else {
                        break;
                    }
                }

                if !jitter_primed && buffered_frames.len() >= jitter_target_frames {
                    if let Some((&first_sequence, _)) = buffered_frames.first_key_value() {
                        next_play_sequence = Some(first_sequence);
                    }
                    jitter_primed = true;
                    status(format!("Jitter buffer primed with {} frames.", buffered_frames.len()));
                }

                depth_sum += buffered_frames.len() as u64;
                depth_samples += 1;
            }
            _ = playback_tick.tick() => {
                if !jitter_primed {
                    continue;
                }

                let low_water_samples = (jitter_target_frames.max(2) / 2).max(2) * output_frame_samples;
                let target_samples = jitter_target_frames * output_frame_samples;

                if player.buffered_sample_count() > low_water_samples {
                    continue;
                }

                let play_sequence = next_play_sequence.get_or_insert(0);
                let mut queued_any = false;

                while player.buffered_sample_count() < target_samples {
                    if let Some(samples) = buffered_frames.remove(play_sequence) {
                        last_good_output.clone_from(&samples);
                        player.push_samples(&samples);
                        consecutive_missing_frames = 0;
                    } else if let Some((&first_available, _)) = buffered_frames.first_key_value() {
                        if first_available > *play_sequence {
                            missing_packets += 1;
                            underrun_frames += 1;
                            consecutive_missing_frames += 1;
                            if consecutive_missing_frames <= 2 {
                                player.push_samples(&last_good_output);
                            } else {
                                player.push_samples(&silence_frame);
                            }
                            if jitter_target_frames < JITTER_BUFFER_MAX_TARGET_FRAMES {
                                jitter_target_frames = (jitter_target_frames + 2).min(JITTER_BUFFER_MAX_TARGET_FRAMES);
                            }
                            stable_played_frames = 0;

                            if consecutive_missing_frames >= CONSECUTIVE_MISSING_REBUFFER_THRESHOLD {
                                player.clear_buffer();
                                jitter_primed = false;
                                consecutive_missing_frames = 0;
                                next_play_sequence = buffered_frames.first_key_value().map(|(&sequence, _)| sequence);
                                status(format!(
                                    "Receiver rebuffering after packet loss. New target {} frames.",
                                    jitter_target_frames
                                ));
                                break;
                            }
                        } else {
                            break;
                        }
                    } else {
                        jitter_primed = false;
                        consecutive_missing_frames = 0;
                        break;
                    }

                    *play_sequence += 1;
                    played_frames += 1;
                    queued_any = true;
                }

                if !jitter_primed && buffered_frames.len() >= jitter_target_frames {
                    if let Some((&first_sequence, _)) = buffered_frames.first_key_value() {
                        next_play_sequence = Some(first_sequence);
                    }
                    jitter_primed = true;
                    status(format!("Jitter buffer refilled with {} frames.", buffered_frames.len()));
                }

                if queued_any {
                    stable_played_frames += 1;
                    if stable_played_frames >= 400
                        && jitter_target_frames > JITTER_BUFFER_MIN_TARGET_FRAMES
                        && buffered_frames.len() >= jitter_target_frames
                    {
                        jitter_target_frames -= 1;
                        stable_played_frames = 0;
                    }
                }

                if queued_any && (played_frames == 1 || played_frames % 100 == 0) {
                    let seconds = started.elapsed().as_secs_f32();
                    let average_depth = if depth_samples == 0 {
                        0.0
                    } else {
                        depth_sum as f32 / depth_samples as f32
                    };
                    status(format!(
                        "Received {received_frames} frames, played {played_frames}, late {late_packets}, reordered {reordered_packets}, missing {missing_packets}, underruns {underrun_frames}, dropped {dropped_frames}, target {jitter_target_frames}, avg buffer {average_depth:.1}, net buffer {}, audio queue {} in {seconds:.1}s.",
                        buffered_frames.len(),
                        player.buffered_sample_count() / output_frame_samples
                    ));
                    emit_metrics(
                        &metrics,
                        format!(
                            "Mode: Receiver\nState: Streaming\nSender: {}\nSource: {} Hz -> Playback: {} Hz\nFrames: received {received_frames}, played {played_frames}\nLatency estimate: {} ms\nNetwork buffer: {} frames\nAudio queue: {} frames\nJitter target: {} frames\nLoss: missing {missing_packets}, late {late_packets}, reordered {reordered_packets}, dropped {dropped_frames}, underruns {underrun_frames}\nAverage network buffer: {:.1} frames\nUptime: {:.1}s",
                            hello.source_name,
                            hello.sample_rate_hz,
                            player.sample_rate_hz(),
                            ((buffered_frames.len() + (player.buffered_sample_count() / output_frame_samples)) * 10),
                            buffered_frames.len(),
                            player.buffered_sample_count() / output_frame_samples,
                            jitter_target_frames,
                            average_depth,
                            seconds
                        ),
                    );
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
    mut stop_rx: watch::Receiver<bool>,
    mute_rx: watch::Receiver<bool>,
) -> Result<()> {
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

    let mut capture = start_system_audio_capture().context("failed to start system audio capture")?;
    status(format!(
        "Capturing system audio at {} Hz.",
        capture.sample_rate_hz()
    ));
    emit_metrics(
        &metrics,
        format!(
            "Mode: Sender\nState: Connected control channel\nTarget: {control_addr}\nCapture rate: {} Hz",
            capture.sample_rate_hz()
        ),
    );

    let hello = Hello {
        source_name: host_name(),
        sample_rate_hz: capture.sample_rate_hz(),
        channels: CHANNELS,
    };
    write_json_message(&mut stream, &hello).await?;

    let accept: Accept = tokio::select! {
        result = read_json_message(&mut stream) => result,
        _ = wait_for_stop(&mut stop_rx) => return Ok(()),
    }?;
    let (mut control_read, _control_write) = stream.into_split();
    let audio_addr = format!("{}:{}", config.target_ip, accept.audio_port);

    let audio_socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .context("failed to bind local UDP socket")?;
    audio_socket
        .connect(&audio_addr)
        .await
        .with_context(|| format!("failed to connect audio socket to {audio_addr}"))?;

    status(format!("Connected to receiver {}.", accept.target_name));
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
            let encoded = frame.encode();
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
                        "Mode: Sender\nState: Streaming\nReceiver: {}\nCapture rate: {} Hz\nFrames sent: {sequence}\nPending capture queue: {} frames\nPending capture latency: {} ms\nUptime: {:.1}s",
                        accept.target_name,
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

pub async fn run_source_with_reconnect(
    config: SessionConfig,
    status: StatusSink,
    metrics: Option<MetricsSink>,
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
    let samples_per_channel = samples_per_10ms(sample_rate_hz) as f64;
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

fn samples_per_10ms(sample_rate_hz: u32) -> usize {
    ((sample_rate_hz as u64 * 10) / 1000) as usize
}

fn frame_sample_count(sample_rate_hz: u32) -> usize {
    samples_per_10ms(sample_rate_hz) * CHANNELS as usize
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

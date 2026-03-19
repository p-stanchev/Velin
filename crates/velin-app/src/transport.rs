use crate::audio::open_output_device;
use anyhow::{Context, Result, bail};
use local_ip_address::list_afinet_netifas;
use std::env;
use std::f32::consts::TAU;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::watch;
use tokio::time;
use velin_proto::{
    Accept, AudioFrame, CHANNELS, DEFAULT_DISCOVERY_PORT, DiscoveryAnnouncement, FRAME_SAMPLES,
    Hello, SAMPLE_RATE_HZ,
};

pub type StatusSink = Arc<dyn Fn(String) + Send + Sync>;

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

    let (mut stream, peer_addr) = tokio::select! {
        result = listener.accept() => result.context("failed to accept source")?,
        _ = wait_for_stop(&mut stop_rx) => return Ok(()),
    };
    let hello: Hello = read_json_message(&mut stream).await?;

    status(format!("Sender {} connected from {peer_addr}.", hello.source_name));

    let accept = Accept {
        target_name: host_name(),
        audio_port: config.audio_port,
    };
    write_json_message(&mut stream, &accept).await?;

    let mut packet = vec![0_u8; 2048];
    let mut received_frames = 0_u64;
    let mut last_sequence = None;
    let started = Instant::now();

    loop {
        let (len, from) = tokio::select! {
            result = audio_socket.recv_from(&mut packet) => result.context("failed to receive audio frame")?,
            result = mute_rx.changed() => {
                if result.is_ok() {
                    player.set_muted(*mute_rx.borrow());
                }
                continue;
            }
            _ = wait_for_stop(&mut stop_rx) => return Ok(()),
        };

        let Some(frame) = AudioFrame::decode(&packet[..len]) else {
            status(format!("Discarded malformed packet from {from}."));
            continue;
        };

        if let Some(previous) = last_sequence {
            let expected = previous + 1;
            if frame.sequence != expected {
                status(format!("Frame gap: expected {expected}, got {}.", frame.sequence));
            }
        }

        last_sequence = Some(frame.sequence);
        received_frames += 1;
        player.push_samples(&frame.samples);

        if received_frames == 1 || received_frames % 100 == 0 {
            let seconds = started.elapsed().as_secs_f32();
            status(format!("Received {received_frames} frames in {seconds:.1}s."));
        }
    }
}

pub async fn run_source(
    config: SessionConfig,
    status: StatusSink,
    mut stop_rx: watch::Receiver<bool>,
    mute_rx: watch::Receiver<bool>,
) -> Result<()> {
    let control_addr = format!("{}:{}", config.target_ip, config.control_port);
    let mut stream = tokio::select! {
        result = TcpStream::connect(&control_addr) => result.with_context(|| format!("failed to connect to target control channel at {control_addr}"))?,
        _ = wait_for_stop(&mut stop_rx) => return Ok(()),
    };

    let hello = Hello {
        source_name: host_name(),
        sample_rate_hz: SAMPLE_RATE_HZ,
        channels: CHANNELS,
    };
    write_json_message(&mut stream, &hello).await?;

    let accept: Accept = tokio::select! {
        result = read_json_message(&mut stream) => result,
        _ = wait_for_stop(&mut stop_rx) => return Ok(()),
    }?;
    let audio_addr = format!("{}:{}", config.target_ip, accept.audio_port);

    let audio_socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .context("failed to bind local UDP socket")?;
    audio_socket
        .connect(&audio_addr)
        .await
        .with_context(|| format!("failed to connect audio socket to {audio_addr}"))?;

    status(format!("Connected to receiver {}.", accept.target_name));

    let mut ticker = time::interval(frame_duration());
    let mut phase = 0.0_f32;
    let step = 440.0_f32 / SAMPLE_RATE_HZ as f32;
    let mute_rx = mute_rx;

    for sequence in 0_u64.. {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = wait_for_stop(&mut stop_rx) => return Ok(()),
        }

        let mut samples = Vec::with_capacity(FRAME_SAMPLES * CHANNELS as usize);
        let muted = *mute_rx.borrow();
        for _ in 0..FRAME_SAMPLES {
            let pcm = if muted {
                0
            } else {
                let sample = (phase * TAU).sin();
                (sample * i16::MAX as f32 * 0.2) as i16
            };
            for _ in 0..CHANNELS {
                samples.push(pcm);
            }
            phase = (phase + step) % 1.0;
        }

        let frame = AudioFrame { sequence, samples };
        let encoded = frame.encode();
        let sent = tokio::select! {
            result = audio_socket.send(&encoded) => result.context("failed to send audio frame")?,
            _ = wait_for_stop(&mut stop_rx) => return Ok(()),
        };

        if sent != encoded.len() {
            bail!("short UDP send: expected {}, sent {sent}", encoded.len());
        }

        if sequence == 0 || sequence % 100 == 0 {
            status(format!("Sent frame {sequence}."));
        }
    }

    #[allow(unreachable_code)]
    Ok(())
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

    let addresses = advertised_ipv4_addresses(&bind_ip);
    if addresses.is_empty() {
        return Ok(());
    }

    let announcement = DiscoveryAnnouncement {
        machine_name: host_name(),
        control_port,
        addresses,
    };
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

fn frame_duration() -> Duration {
    Duration::from_secs_f64(FRAME_SAMPLES as f64 / SAMPLE_RATE_HZ as f64)
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
    env::var("COMPUTERNAME")
        .or_else(|_| env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
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

fn advertised_ipv4_addresses(bind_ip: &str) -> Vec<String> {
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

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{I24, Sample, SampleFormat, SizedSample, Stream, StreamConfig, SupportedStreamConfig, U24};
#[cfg(target_os = "linux")]
use cpal::{BufferSize, SupportedBufferSize};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use velin_proto::{CHANNELS, SAMPLE_RATE_HZ};

const MAX_BUFFERED_SAMPLES: usize = SAMPLE_RATE_HZ as usize * CHANNELS as usize * 2;

#[derive(Default)]
struct PlaybackBuffer {
    samples: VecDeque<f32>,
}

pub struct OutputPlayer {
    _stream: Stream,
    buffer: Arc<Mutex<PlaybackBuffer>>,
    muted: Arc<AtomicBool>,
}

impl OutputPlayer {
    pub fn push_samples(&self, samples: &[i16]) {
        let mut buffer = self.buffer.lock().expect("playback buffer poisoned");
        for sample in samples {
            buffer.samples.push_back(*sample as f32 / i16::MAX as f32);
        }

        if buffer.samples.len() > MAX_BUFFERED_SAMPLES {
            let overflow = buffer.samples.len() - MAX_BUFFERED_SAMPLES;
            buffer.samples.drain(0..overflow);
        }
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }
}

pub fn output_device_names() -> Result<Vec<String>> {
    let mut names: Vec<String> = enumerate_output_device_labels()?.into_values().collect();
    names.sort();
    Ok(names)
}

pub fn default_output_device_name() -> Result<Option<String>> {
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        return Ok(None);
    };
    Ok(Some(default_output_device_label(&host, &device)?))
}

pub fn open_output_device(selected_name: &str) -> Result<(OutputPlayer, String)> {
    let host = cpal::default_host();
    let device = select_output_device(&host, selected_name)?;
    let device_name = if selected_name.trim().is_empty() {
        default_output_device_label(&host, &device)?
    } else {
        selected_name.to_string()
    };
    let supported_config = select_stream_config(&device)?;
    let stream_config = stream_config_for_playback(&supported_config);
    let sample_format = supported_config.sample_format();
    let output_channels = stream_config.channels as usize;
    let buffer = Arc::new(Mutex::new(PlaybackBuffer::default()));
    let muted = Arc::new(AtomicBool::new(false));
    let stream = build_stream(
        &device,
        &stream_config,
        sample_format,
        output_channels,
        Arc::clone(&buffer),
        Arc::clone(&muted),
    )?;

    stream.play().context("failed to start output stream")?;

    Ok((
        OutputPlayer {
            _stream: stream,
            buffer,
            muted,
        },
        device_name,
    ))
}

fn select_output_device(host: &cpal::Host, selected_name: &str) -> Result<cpal::Device> {
    if !selected_name.trim().is_empty() {
        let labels = enumerate_output_device_labels()?;
        for host_id in cpal::available_hosts() {
            let candidate_host = cpal::host_from_id(host_id)
                .with_context(|| format!("failed to open audio host {host_id:?}"))?;
            let devices = candidate_host
                .output_devices()
                .with_context(|| format!("failed to enumerate output devices for {host_id:?}"))?;

            for (index, device) in devices.enumerate() {
                if labels.get(&(host_id, index)).is_some_and(|label| label == selected_name) {
                    return Ok(device);
                }
            }
        }
    }

    host.default_output_device()
        .ok_or_else(|| anyhow!("no output device available"))
}

fn select_stream_config(device: &cpal::Device) -> Result<SupportedStreamConfig> {
    device
        .default_output_config()
        .context("failed to read default output config")
}

fn stream_config_for_playback(supported_config: &SupportedStreamConfig) -> StreamConfig {
    #[cfg(target_os = "linux")]
    let mut config = supported_config.config();

    #[cfg(not(target_os = "linux"))]
    let config = supported_config.config();

    #[cfg(target_os = "linux")]
    {
        config.buffer_size = choose_linux_buffer_size(supported_config.buffer_size());
    }

    config
}

#[cfg(target_os = "linux")]
fn choose_linux_buffer_size(buffer_size: &SupportedBufferSize) -> BufferSize {
    const TARGET_FRAMES: u32 = 4096;

    match buffer_size {
        SupportedBufferSize::Range { min, max } => BufferSize::Fixed(TARGET_FRAMES.clamp(*min, *max)),
        SupportedBufferSize::Unknown => BufferSize::Default,
    }
}

fn build_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    sample_format: SampleFormat,
    output_channels: usize,
    buffer: Arc<Mutex<PlaybackBuffer>>,
    muted: Arc<AtomicBool>,
) -> Result<Stream> {
    let error_callback = |error| eprintln!("audio output stream error: {error}");

    match sample_format {
        SampleFormat::I8 => build_stream_for_format::<i8>(device, config, output_channels, buffer, muted, error_callback),
        SampleFormat::F32 => build_stream_for_format::<f32>(device, config, output_channels, buffer, muted, error_callback),
        SampleFormat::F64 => build_stream_for_format::<f64>(device, config, output_channels, buffer, muted, error_callback),
        SampleFormat::I16 => build_stream_for_format::<i16>(device, config, output_channels, buffer, muted, error_callback),
        SampleFormat::I24 => build_stream_for_format::<I24>(device, config, output_channels, buffer, muted, error_callback),
        SampleFormat::I32 => build_stream_for_format::<i32>(device, config, output_channels, buffer, muted, error_callback),
        SampleFormat::U8 => build_stream_for_format::<u8>(device, config, output_channels, buffer, muted, error_callback),
        SampleFormat::U16 => build_stream_for_format::<u16>(device, config, output_channels, buffer, muted, error_callback),
        SampleFormat::U24 => build_stream_for_format::<U24>(device, config, output_channels, buffer, muted, error_callback),
        SampleFormat::U32 => build_stream_for_format::<u32>(device, config, output_channels, buffer, muted, error_callback),
        other => Err(anyhow!("unsupported output sample format: {other:?}")),
    }
}

fn build_stream_for_format<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    output_channels: usize,
    buffer: Arc<Mutex<PlaybackBuffer>>,
    muted: Arc<AtomicBool>,
    error_callback: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<Stream>
where
    T: SizedSample + cpal::FromSample<f32>,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| write_output_data::<T>(data, output_channels, &buffer, &muted),
            error_callback,
            None,
        )
        .context("failed to build output stream")
}

fn write_output_data<T>(
    data: &mut [T],
    output_channels: usize,
    buffer: &Arc<Mutex<PlaybackBuffer>>,
    muted: &Arc<AtomicBool>,
)
where
    T: Sample + cpal::FromSample<f32>,
{
    if muted.load(Ordering::Relaxed) {
        for sample in data.iter_mut() {
            *sample = T::from_sample(0.0);
        }
        return;
    }

    let mut playback = buffer.lock().expect("playback buffer poisoned");

    for frame in data.chunks_mut(output_channels) {
        let left = playback.samples.pop_front().unwrap_or(0.0);
        let right = playback.samples.pop_front().unwrap_or(left);

        for (channel_index, sample) in frame.iter_mut().enumerate() {
            let value = match output_channels {
                0 => 0.0,
                1 => (left + right) * 0.5,
                _ => match channel_index % 2 {
                    0 => left,
                    _ => right,
                },
            };

            *sample = T::from_sample(value);
        }
    }
}

fn enumerate_output_device_labels() -> Result<HashMap<(cpal::HostId, usize), String>> {
    let mut entries = Vec::new();

    for host_id in cpal::available_hosts() {
        let host = cpal::host_from_id(host_id)
            .with_context(|| format!("failed to open audio host {host_id:?}"))?;
        let devices = host
            .output_devices()
            .with_context(|| format!("failed to enumerate output devices for {host_id:?}"))?;

        for (index, device) in devices.enumerate() {
            entries.push(((host_id, index), base_device_label(&device), short_device_suffix(&device)));
        }
    }

    let mut counts = HashMap::<String, usize>::new();
    for (_, base_label, _) in &entries {
        *counts.entry(base_label.clone()).or_insert(0) += 1;
    }

    let mut labels = HashMap::new();
    for (key, base_label, suffix) in entries {
        let label = if counts.get(&base_label).copied().unwrap_or(0) > 1 {
            format!("{base_label}{suffix}")
        } else {
            base_label
        };
        labels.insert(key, label);
    }

    Ok(labels)
}

fn default_output_device_label(host: &cpal::Host, device: &cpal::Device) -> Result<String> {
    let labels = enumerate_output_device_labels()?;
    let host_id = host.id();
    let devices = host
        .output_devices()
        .with_context(|| format!("failed to enumerate output devices for {host_id:?}"))?;

    for (index, candidate) in devices.enumerate() {
        if candidate.id().ok() == device.id().ok() {
            if let Some(label) = labels.get(&(host_id, index)) {
                return Ok(label.clone());
            }
        }
    }

    Ok(base_device_label(device))
}

fn base_device_label(device: &cpal::Device) -> String {
    if let Ok(description) = device.description() {
        if let Some(first) = description.extended().first() {
            let trimmed = first.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }

        if let Some(driver) = description.driver() {
            if !driver.is_empty() && driver != description.name() {
                return format!("{} ({driver})", description.name());
            }
        }

        return description.name().to_string();
    }

    #[allow(deprecated)]
    device
        .name()
        .unwrap_or_else(|_| "Unknown output device".to_string())
}

fn short_device_suffix(device: &cpal::Device) -> String {
    device
        .id()
        .ok()
        .map(|id| {
            let raw = id.1;
            let short = raw.chars().rev().take(6).collect::<String>().chars().rev().collect::<String>();
            format!(" [{short}]")
        })
        .unwrap_or_default()
}

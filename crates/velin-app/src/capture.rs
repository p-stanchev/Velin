use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::sync::mpsc;
use velin_proto::CHANNELS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    System,
    Microphone,
    SystemPlusMicrophone,
}

impl Default for CaptureMode {
    fn default() -> Self {
        Self::System
    }
}

pub struct AudioCapture {
    sample_rate_hz: u32,
    receiver: mpsc::UnboundedReceiver<Vec<i16>>,
    _inner: CaptureBackend,
}

impl AudioCapture {
    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    pub async fn recv(&mut self) -> Option<Vec<i16>> {
        self.receiver.recv().await
    }
}

enum CaptureBackend {
    Platform { _capture: PlatformCapture },
    Microphone { _capture: MicrophoneCapture },
    Mixed { _capture: MixedCapture },
}

pub fn start_audio_capture(
    mode: CaptureMode,
    system_source_name: &str,
    microphone_device_name: &str,
) -> Result<AudioCapture> {
    let (sample_rate_hz, receiver, inner) = match mode {
        CaptureMode::System => {
            let (sample_rate_hz, receiver, inner) = platform::start_system_capture(system_source_name)?;
            (sample_rate_hz, receiver, CaptureBackend::Platform { _capture: inner })
        }
        CaptureMode::Microphone => {
            let (sample_rate_hz, receiver, inner) = start_microphone_capture(microphone_device_name)?;
            (sample_rate_hz, receiver, CaptureBackend::Microphone { _capture: inner })
        }
        CaptureMode::SystemPlusMicrophone => {
            let (sample_rate_hz, receiver, inner) =
                start_mixed_capture(system_source_name, microphone_device_name)?;
            (sample_rate_hz, receiver, CaptureBackend::Mixed { _capture: inner })
        }
    };

    Ok(AudioCapture {
        sample_rate_hz,
        receiver,
        _inner: inner,
    })
}

pub fn input_device_names() -> Result<Vec<String>> {
    #[cfg(target_os = "linux")]
    {
        platform::microphone_source_names()
    }

    #[cfg(not(target_os = "linux"))]
    {
        let host = cpal::default_host();
        let mut names = host
            .input_devices()?
            .map(|device| input_device_label(&device))
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        Ok(names)
    }
}

pub fn default_input_device_name() -> Result<Option<String>> {
    #[cfg(target_os = "linux")]
    {
        Ok(platform::default_microphone_source_name())
    }

    #[cfg(not(target_os = "linux"))]
    {
        let host = cpal::default_host();
        let Some(device) = host.default_input_device() else {
            return Ok(None);
        };
        Ok(Some(input_device_label(&device)))
    }
}

#[cfg(target_os = "linux")]
pub fn system_capture_source_names() -> Result<Vec<String>> {
    platform::system_capture_source_names()
}

#[cfg(not(target_os = "linux"))]
pub fn system_capture_source_names() -> Result<Vec<String>> {
    Ok(vec!["System Default".to_string()])
}

#[cfg(target_os = "linux")]
pub fn default_system_capture_source_name() -> Result<Option<String>> {
    Ok(platform::default_system_capture_source_name())
}

#[cfg(not(target_os = "linux"))]
pub fn default_system_capture_source_name() -> Result<Option<String>> {
    Ok(Some("System Default".to_string()))
}

struct MicrophoneCapture {
    _backend: MicrophoneBackend,
}

enum MicrophoneBackend {
    Stream { _stream: Stream },
    #[cfg(target_os = "linux")]
    LinuxPipe { _pipe: LinuxPulseCapture },
}

struct MixedCapture {
    _system: PlatformCapture,
    _microphone: MicrophoneCapture,
    thread: Option<thread::JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
struct LinuxPulseCapture {
    child: std::process::Child,
    thread: Option<thread::JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
impl Drop for LinuxPulseCapture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for MixedCapture {
    fn drop(&mut self) {
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn start_mixed_capture(
    system_source_name: &str,
    microphone_device_name: &str,
) -> Result<(u32, mpsc::UnboundedReceiver<Vec<i16>>, MixedCapture)> {
    let (sample_rate_hz, mut system_rx, system_capture) = platform::start_system_capture(system_source_name)?;
    let (microphone_rate_hz, mut microphone_rx, microphone_capture) = start_microphone_capture(microphone_device_name)?;
    let (sender, receiver) = mpsc::unbounded_channel();

    let thread = thread::Builder::new()
        .name("velin-mixed-capture".to_string())
        .spawn(move || {
            let mut mic_buffer = VecDeque::<i16>::new();
            while let Some(system_chunk) = system_rx.blocking_recv() {
                while let Ok(mic_chunk) = microphone_rx.try_recv() {
                    let resampled =
                        resample_stereo_i16(&mic_chunk, microphone_rate_hz, sample_rate_hz, CHANNELS as usize);
                    mic_buffer.extend(resampled);
                    let max_samples = sample_rate_hz as usize * CHANNELS as usize * 2;
                    if mic_buffer.len() > max_samples {
                        let overflow = mic_buffer.len() - max_samples;
                        mic_buffer.drain(0..overflow);
                    }
                }

                let mut mixed = Vec::with_capacity(system_chunk.len());
                for sample in system_chunk {
                    let mic = mic_buffer.pop_front().unwrap_or(0);
                    mixed.push((sample as i32 + mic as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16);
                }

                if sender.send(mixed).is_err() {
                    break;
                }
            }
        })?;

    Ok((
        sample_rate_hz,
        receiver,
        MixedCapture {
            _system: system_capture,
            _microphone: microphone_capture,
            thread: Some(thread),
        },
    ))
}

fn start_microphone_capture(
    selected_name: &str,
) -> Result<(u32, mpsc::UnboundedReceiver<Vec<i16>>, MicrophoneCapture)> {
    #[cfg(target_os = "linux")]
    if let Ok((sample_rate_hz, receiver, pipe)) = platform::start_microphone_capture(selected_name) {
        return Ok((
            sample_rate_hz,
            receiver,
            MicrophoneCapture {
                _backend: MicrophoneBackend::LinuxPipe { _pipe: pipe },
            },
        ));
    }

    start_microphone_capture_cpal(selected_name)
}

fn start_microphone_capture_cpal(
    selected_name: &str,
) -> Result<(u32, mpsc::UnboundedReceiver<Vec<i16>>, MicrophoneCapture)> {
    let host = cpal::default_host();
    let device = select_input_device(&host, selected_name)?;
    let supported_config = device.default_input_config()?;
    let sample_rate_hz = supported_config.sample_rate();
    let channels = supported_config.channels() as usize;
    let stream_config: StreamConfig = supported_config.clone().into();
    let (sender, receiver) = mpsc::unbounded_channel();
    let state = Arc::new(Mutex::new(MicrophoneBuffer::new(sample_rate_hz, channels, sender)));
    let error_callback = |error| {
        eprintln!("microphone capture stream error: {error}");
    };

    let stream = match supported_config.sample_format() {
        SampleFormat::I8 => build_microphone_stream::<i8>(&device, &stream_config, Arc::clone(&state), error_callback)?,
        SampleFormat::I16 => build_microphone_stream::<i16>(&device, &stream_config, Arc::clone(&state), error_callback)?,
        SampleFormat::I24 => build_microphone_stream::<cpal::I24>(&device, &stream_config, Arc::clone(&state), error_callback)?,
        SampleFormat::I32 => build_microphone_stream::<i32>(&device, &stream_config, Arc::clone(&state), error_callback)?,
        SampleFormat::I64 => build_microphone_stream::<i64>(&device, &stream_config, Arc::clone(&state), error_callback)?,
        SampleFormat::U8 => build_microphone_stream::<u8>(&device, &stream_config, Arc::clone(&state), error_callback)?,
        SampleFormat::U16 => build_microphone_stream::<u16>(&device, &stream_config, Arc::clone(&state), error_callback)?,
        SampleFormat::U32 => build_microphone_stream::<u32>(&device, &stream_config, Arc::clone(&state), error_callback)?,
        SampleFormat::U64 => build_microphone_stream::<u64>(&device, &stream_config, Arc::clone(&state), error_callback)?,
        SampleFormat::F32 => build_microphone_stream::<f32>(&device, &stream_config, Arc::clone(&state), error_callback)?,
        SampleFormat::F64 => build_microphone_stream::<f64>(&device, &stream_config, Arc::clone(&state), error_callback)?,
        _ => anyhow::bail!("unsupported microphone sample format"),
    };

    stream.play()?;

    Ok((
        sample_rate_hz,
        receiver,
        MicrophoneCapture {
            _backend: MicrophoneBackend::Stream { _stream: stream },
        },
    ))
}

struct MicrophoneBuffer {
    sender: mpsc::UnboundedSender<Vec<i16>>,
    mono_samples: VecDeque<f32>,
    chunk_frames: usize,
    channels: usize,
}

impl MicrophoneBuffer {
    fn new(sample_rate_hz: u32, channels: usize, sender: mpsc::UnboundedSender<Vec<i16>>) -> Self {
        Self {
            sender,
            mono_samples: VecDeque::new(),
            chunk_frames: ((sample_rate_hz as u64 * 10) / 1000) as usize,
            channels: channels.max(1),
        }
    }

    fn push<T>(&mut self, input: &[T])
    where
        T: Sample,
        f32: cpal::FromSample<T>,
    {
        for frame in input.chunks(self.channels) {
            let mut sum = 0.0_f32;
            for sample in frame {
                sum += sample.to_sample::<f32>();
            }
            self.mono_samples.push_back(sum / frame.len().max(1) as f32);
        }

        while self.mono_samples.len() >= self.chunk_frames {
            let mut chunk = Vec::with_capacity(self.chunk_frames * CHANNELS as usize);
            for _ in 0..self.chunk_frames {
                let Some(sample) = self.mono_samples.pop_front() else {
                    break;
                };
                let pcm = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                chunk.push(pcm);
                chunk.push(pcm);
            }
            if chunk.len() != self.chunk_frames * CHANNELS as usize {
                break;
            }
            if self.sender.send(chunk).is_err() {
                break;
            }
        }
    }
}

fn build_microphone_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    state: Arc<Mutex<MicrophoneBuffer>>,
    error_callback: impl Fn(cpal::StreamError) + Send + 'static,
) -> Result<Stream>
where
    T: SizedSample + Sample,
    f32: cpal::FromSample<T>,
{
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| {
            if let Ok(mut state) = state.lock() {
                state.push(data);
            }
        },
        error_callback,
        None,
    )?)
}

fn select_input_device(host: &cpal::Host, selected_name: &str) -> Result<cpal::Device> {
    if !selected_name.trim().is_empty() {
        for device in host.input_devices()? {
            if input_device_label(&device) == selected_name.trim() {
                return Ok(device);
            }
        }
    }

    host.default_input_device()
        .ok_or_else(|| anyhow::anyhow!("no input device available"))
}

fn input_device_label(device: &cpal::Device) -> String {
    if let Ok(description) = device.description() {
        let text = description.to_string();
        if !text.trim().is_empty() {
            return text;
        }
    }

    #[allow(deprecated)]
    device.name().unwrap_or_else(|_| "Unknown input device".to_string())
}

fn resample_stereo_i16(input: &[i16], input_rate_hz: u32, output_rate_hz: u32, channels: usize) -> Vec<i16> {
    if input_rate_hz == output_rate_hz || input.is_empty() || channels == 0 {
        return input.to_vec();
    }

    let input_frames = input.len() / channels;
    if input_frames == 0 {
        return Vec::new();
    }

    let output_frames = ((input_frames as u64 * output_rate_hz as u64) / input_rate_hz as u64).max(1) as usize;
    let ratio = input_rate_hz as f32 / output_rate_hz as f32;
    let mut output = Vec::with_capacity(output_frames * channels);

    for output_frame in 0..output_frames {
        let position = output_frame as f32 * ratio;
        let base = position.floor() as usize;
        let next = (base + 1).min(input_frames.saturating_sub(1));
        let frac = position - base as f32;

        for channel in 0..channels {
            let left = input[base * channels + channel] as f32;
            let right = input[next * channels + channel] as f32;
            output.push((left + (right - left) * frac).round() as i16);
        }
    }

    output
}

#[cfg(target_os = "windows")]
type PlatformCapture = platform::WindowsSystemCapture;

#[cfg(target_os = "linux")]
type PlatformCapture = platform::LinuxSystemCapture;

#[cfg(all(not(target_os = "windows"), not(target_os = "linux")))]
type PlatformCapture = platform::UnsupportedCapture;

#[cfg(target_os = "linux")]
mod platform {
    use anyhow::{Context, Result, anyhow};
    use crate::capture::LinuxPulseCapture;
    use std::env;
    use std::io::Read;
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use velin_proto::{CHANNELS, SAMPLE_RATE_HZ};

    pub struct LinuxSystemCapture {
        child: Child,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl Drop for LinuxSystemCapture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            if let Some(handle) = self.thread.take() {
                let _ = handle.join();
            }
        }
    }

    pub fn start_system_capture(
        selected_source: &str,
    ) -> Result<(u32, mpsc::UnboundedReceiver<Vec<i16>>, LinuxSystemCapture)> {
        let monitor_source = detect_monitor_source(selected_source);
        let sample_rate_hz = detect_source_sample_rate(monitor_source.as_deref()).unwrap_or(SAMPLE_RATE_HZ);
        let mut last_error = None;

        for candidate in launch_candidates(&monitor_source, sample_rate_hz) {
            match spawn_capture_process(&candidate) {
                Ok(mut child) => {
                    let stdout = child
                        .stdout
                        .take()
                        .context("capture process did not provide a stdout stream")?;
                    let (sender, receiver) = mpsc::unbounded_channel();
                    let thread = thread::Builder::new()
                        .name("velin-linux-capture".to_string())
                        .spawn(move || {
                            let mut stdout = stdout;
                            let mut bytes =
                                vec![0_u8; samples_per_10ms(sample_rate_hz) * CHANNELS as usize * 2];
                            loop {
                                if stdout.read_exact(&mut bytes).is_err() {
                                    break;
                                }

                                let mut samples = Vec::with_capacity(bytes.len() / 2);
                                for chunk in bytes.chunks_exact(2) {
                                    samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
                                }

                                if sender.send(samples).is_err() {
                                    break;
                                }
                            }
                        })
                        .context("failed to spawn Linux capture thread")?;

                    return Ok((
                        sample_rate_hz,
                        receiver,
                        LinuxSystemCapture {
                            child,
                            thread: Some(thread),
                        },
                    ));
                }
                Err(error) => last_error = Some(error),
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow!("failed to start Linux system audio capture")
        }))
    }

    pub fn system_capture_source_names() -> Result<Vec<String>> {
        let output = Command::new("pactl")
            .args(["list", "short", "sources"])
            .output()
            .context("failed to enumerate PulseAudio sources")?;
        if !output.status.success() {
            return Err(anyhow!("pactl list short sources exited with {}", output.status));
        }

        let mut sources = vec!["System Default".to_string()];
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some(name) = line.split_whitespace().nth(1) {
                if !sources.iter().any(|existing| existing == name) {
                    sources.push(name.to_string());
                }
            }
        }
        Ok(sources)
    }

    pub fn default_system_capture_source_name() -> Option<String> {
        detect_monitor_source("")
    }

    pub fn microphone_source_names() -> Result<Vec<String>> {
        let output = Command::new("pactl")
            .args(["list", "short", "sources"])
            .output()
            .context("failed to enumerate PulseAudio microphone sources")?;
        if !output.status.success() {
            return Err(anyhow!("pactl list short sources exited with {}", output.status));
        }

        let mut sources = vec!["System Default".to_string()];
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some(name) = line.split_whitespace().nth(1) {
                if name.ends_with(".monitor") {
                    continue;
                }
                if !sources.iter().any(|existing| existing == name) {
                    sources.push(name.to_string());
                }
            }
        }
        Ok(sources)
    }

    pub fn default_microphone_source_name() -> Option<String> {
        detect_microphone_source("")
    }

    pub fn start_microphone_capture(
        selected_source: &str,
    ) -> Result<(u32, mpsc::UnboundedReceiver<Vec<i16>>, LinuxPulseCapture)> {
        let microphone_source = detect_microphone_source(selected_source);
        let sample_rate_hz = detect_source_sample_rate(microphone_source.as_deref()).unwrap_or(SAMPLE_RATE_HZ);
        let mut last_error = None;

        for candidate in microphone_launch_candidates(&microphone_source, sample_rate_hz) {
            match spawn_capture_process(&candidate) {
                Ok(mut child) => {
                    let stdout = child
                        .stdout
                        .take()
                        .context("microphone capture process did not provide a stdout stream")?;
                    let (sender, receiver) = mpsc::unbounded_channel();
                    let thread = thread::Builder::new()
                        .name("velin-linux-microphone".to_string())
                        .spawn(move || {
                            let mut stdout = stdout;
                            let mut bytes =
                                vec![0_u8; samples_per_10ms(sample_rate_hz) * CHANNELS as usize * 2];
                            loop {
                                if stdout.read_exact(&mut bytes).is_err() {
                                    break;
                                }

                                let mut samples = Vec::with_capacity(bytes.len() / 2);
                                for chunk in bytes.chunks_exact(2) {
                                    samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
                                }

                                if sender.send(samples).is_err() {
                                    break;
                                }
                            }
                        })
                        .context("failed to spawn Linux microphone capture thread")?;

                    return Ok((
                        sample_rate_hz,
                        receiver,
                        LinuxPulseCapture {
                            child,
                            thread: Some(thread),
                        },
                    ));
                }
                Err(error) => last_error = Some(error),
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("failed to start Linux microphone capture")))
    }

    #[derive(Clone)]
    struct CaptureCommand {
        program: &'static str,
        args: Vec<String>,
        description: String,
    }

    fn launch_candidates(monitor_source: &Option<String>, sample_rate_hz: u32) -> Vec<CaptureCommand> {
        let mut candidates = Vec::new();
        let mut sources = Vec::new();

        if let Some(source) = monitor_source {
            sources.push(source.clone());
        }
        if !sources.iter().any(|value| value == "@DEFAULT_MONITOR@") {
            sources.push("@DEFAULT_MONITOR@".to_string());
        }

        for source in sources {
            candidates.push(CaptureCommand {
                program: "parec",
                args: vec![
                    format!("--device={source}"),
                    "--raw".to_string(),
                    "--format=s16le".to_string(),
                    "--fix-format".to_string(),
                    "--fix-rate".to_string(),
                    "--fix-channels".to_string(),
                    "--latency-msec=20".to_string(),
                    format!("--rate={sample_rate_hz}"),
                    "--channels=2".to_string(),
                ],
                description: format!("parec monitor source {source}"),
            });
        }

        candidates
    }

    fn microphone_launch_candidates(source: &Option<String>, sample_rate_hz: u32) -> Vec<CaptureCommand> {
        let mut candidates = Vec::new();
        let mut sources = Vec::new();

        if let Some(source) = source {
            sources.push(source.clone());
        }
        if !sources.iter().any(|value| value == "@DEFAULT_SOURCE@") {
            sources.push("@DEFAULT_SOURCE@".to_string());
        }

        for source in sources {
            candidates.push(CaptureCommand {
                program: "parec",
                args: vec![
                    format!("--device={source}"),
                    "--raw".to_string(),
                    "--format=s16le".to_string(),
                    "--fix-format".to_string(),
                    "--fix-rate".to_string(),
                    "--fix-channels".to_string(),
                    "--latency-msec=20".to_string(),
                    format!("--rate={sample_rate_hz}"),
                    "--channels=2".to_string(),
                ],
                description: format!("parec microphone source {source}"),
            });
        }

        candidates
    }

    fn spawn_capture_process(candidate: &CaptureCommand) -> Result<Child> {
        let mut child = Command::new(candidate.program)
            .args(&candidate.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start {}", candidate.description))?;

        thread::sleep(Duration::from_millis(150));
        if let Some(status) = child.try_wait().context("failed to poll capture process")? {
            return Err(anyhow!(
                "{} exited immediately with status {status}",
                candidate.description
            ));
        }

        Ok(child)
    }

    fn detect_monitor_source(selected_source: &str) -> Option<String> {
        let trimmed_selected = selected_source.trim();
        if !trimmed_selected.is_empty() && trimmed_selected != "System Default" {
            return Some(trimmed_selected.to_string());
        }

        if let Ok(value) = env::var("VELIN_LINUX_MONITOR") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }

        if let Ok(output) = Command::new("pactl").args(["get-default-sink"]).output() {
            if output.status.success() {
                let sink = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !sink.is_empty() {
                    return Some(format!("{sink}.monitor"));
                }
            }
        }

        if let Ok(output) = Command::new("pactl").args(["info"]).output() {
            if output.status.success() {
                for line in String::from_utf8_lossy(&output.stdout).lines() {
                    if let Some(value) = line.strip_prefix("Default Sink:") {
                        let sink = value.trim();
                        if !sink.is_empty() {
                            return Some(format!("{sink}.monitor"));
                        }
                    }
                }
            }
        }

        None
    }

    fn detect_microphone_source(selected_source: &str) -> Option<String> {
        let trimmed_selected = selected_source.trim();
        if !trimmed_selected.is_empty() && trimmed_selected != "System Default" {
            return Some(trimmed_selected.to_string());
        }

        if let Ok(value) = env::var("VELIN_LINUX_MIC_SOURCE") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }

        if let Ok(output) = Command::new("pactl").args(["get-default-source"]).output() {
            if output.status.success() {
                let source = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !source.is_empty() {
                    return Some(source);
                }
            }
        }

        if let Ok(output) = Command::new("pactl").args(["info"]).output() {
            if output.status.success() {
                for line in String::from_utf8_lossy(&output.stdout).lines() {
                    if let Some(value) = line.strip_prefix("Default Source:") {
                        let source = value.trim();
                        if !source.is_empty() {
                            return Some(source.to_string());
                        }
                    }
                }
            }
        }

        None
    }

    fn detect_source_sample_rate(selected_source: Option<&str>) -> Option<u32> {
        let output = Command::new("pactl").args(["list", "sources"]).output().ok()?;
        if !output.status.success() {
            return None;
        }

        let selected_source = selected_source
            .filter(|source| !source.is_empty() && *source != "@DEFAULT_MONITOR@" && *source != "@DEFAULT_SOURCE@")
            .map(str::trim);
        let listing = String::from_utf8_lossy(&output.stdout);
        let mut current_name: Option<String> = None;

        for line in listing.lines() {
            let trimmed = line.trim();

            if let Some(name) = trimmed.strip_prefix("Name:") {
                current_name = Some(name.trim().to_string());
                continue;
            }

            if let Some(spec) = trimmed.strip_prefix("Sample Specification:") {
                let matches_selected = selected_source
                    .map(|source| current_name.as_deref() == Some(source))
                    .unwrap_or(false);

                if matches_selected {
                    return parse_sample_rate_from_spec(spec);
                }
            }
        }

        None
    }

    fn parse_sample_rate_from_spec(spec: &str) -> Option<u32> {
        spec.split_whitespace().find_map(|token| {
            let hz = token.strip_suffix("Hz")?;
            hz.parse::<u32>().ok()
        })
    }

    fn samples_per_10ms(sample_rate_hz: u32) -> usize {
        ((sample_rate_hz as u64 * 10) / 1000) as usize
    }
}

#[cfg(all(not(target_os = "windows"), not(target_os = "linux")))]
mod platform {
    use anyhow::{Result, bail};
    use tokio::sync::mpsc;

    pub struct UnsupportedCapture;

    pub fn start_system_capture(
        _selected_source: &str,
    ) -> Result<(u32, mpsc::UnboundedReceiver<Vec<i16>>, UnsupportedCapture)> {
        bail!("system audio capture is not implemented on this platform yet")
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_op_in_unsafe_fn)]
mod platform {
    #![allow(unsafe_op_in_unsafe_fn)]
    use anyhow::{Context, Result, anyhow, bail};
    use std::{ptr, slice};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc as std_mpsc;
    use std::thread;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use windows::core::GUID;
    use windows::Win32::Media::Audio::{
        AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
        IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
        WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eConsole, eRender,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
        CoUninitialize,
    };

    const WAVE_FORMAT_PCM: u16 = 0x0001;
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
    const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;
    const SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
    const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

    pub struct WindowsSystemCapture {
        stop: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl Drop for WindowsSystemCapture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = self.thread.take() {
                let _ = handle.join();
            }
        }
    }

    pub fn start_system_capture(
        _selected_source: &str,
    ) -> Result<(u32, mpsc::UnboundedReceiver<Vec<i16>>, WindowsSystemCapture)> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std_mpsc::channel();
        let stop_for_thread = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("velin-system-capture".to_string())
            .spawn(move || {
                let setup = unsafe { prepare_capture() };
                match setup {
                    Ok(setup) => {
                        let _ = ready_tx.send(Ok(setup.sample_rate_hz));
                        if let Err(error) = unsafe { capture_loop(setup, sender, stop_for_thread) } {
                            eprintln!("system audio capture stopped: {error:#}");
                        }
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                    }
                }
            })
            .context("failed to spawn system capture thread")?;
        let sample_rate_hz = match ready_rx.recv().context("failed to receive capture startup state")? {
            Ok(sample_rate_hz) => sample_rate_hz,
            Err(message) => return Err(anyhow!(message)),
        };

        Ok((
            sample_rate_hz,
            receiver,
            WindowsSystemCapture {
                stop,
                thread: Some(thread),
            },
        ))
    }

    struct CaptureSetup {
        audio_client: IAudioClient,
        capture_client: IAudioCaptureClient,
        channel_count: usize,
        sample_rate_hz: u32,
        sample_format: CaptureSampleFormat,
    }

    enum CaptureSampleFormat {
        F32,
        I16,
    }

    unsafe fn prepare_capture() -> Result<CaptureSetup> {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .context("failed to initialize COM for capture")?;

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).context("failed to create device enumerator")?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .context("failed to get default render endpoint")?;
        let audio_client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .context("failed to activate default render endpoint")?;

        let mix_format_ptr = audio_client
            .GetMixFormat()
            .context("failed to get output mix format")?;
        let mix_format = *mix_format_ptr;
        let sample_rate_hz = mix_format.nSamplesPerSec;
        let channel_count = mix_format.nChannels as usize;
        let sample_format = parse_sample_format(mix_format_ptr).context("unsupported output mix format for loopback capture")?;

        audio_client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                0,
                0,
                mix_format_ptr,
                None,
            )
            .context("failed to initialize WASAPI loopback client")?;

        CoTaskMemFree(Some(mix_format_ptr as _));

        let capture_client: IAudioCaptureClient = audio_client
            .GetService()
            .context("failed to get capture client service")?;

        Ok(CaptureSetup {
            audio_client,
            capture_client,
            channel_count,
            sample_rate_hz,
            sample_format,
        })
    }

    unsafe fn capture_loop(
        setup: CaptureSetup,
        sender: mpsc::UnboundedSender<Vec<i16>>,
        stop: Arc<AtomicBool>,
    ) -> Result<()> {
        setup.audio_client.Start().context("failed to start loopback capture")?;

        while !stop.load(Ordering::Relaxed) {
            let packet_frames = setup
                .capture_client
                .GetNextPacketSize()
                .context("failed to query capture packet size")?;

            if packet_frames == 0 {
                thread::sleep(Duration::from_millis(4));
                continue;
            }

            let mut data_ptr = std::ptr::null_mut();
            let mut frame_count = 0u32;
            let mut flags = 0u32;

            setup
                .capture_client
                .GetBuffer(
                    &mut data_ptr,
                    &mut frame_count,
                    &mut flags,
                    None,
                    None,
                )
                .context("failed to read loopback packet")?;

            let sample_count = frame_count as usize * setup.channel_count;
            let samples = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data_ptr.is_null() {
                vec![0_i16; frame_count as usize * 2]
            } else {
                match setup.sample_format {
                    CaptureSampleFormat::F32 => {
                        let input = slice::from_raw_parts(data_ptr.cast::<f32>(), sample_count);
                        convert_to_stereo_i16_from_f32(input, setup.channel_count)
                    }
                    CaptureSampleFormat::I16 => {
                        let input = slice::from_raw_parts(data_ptr.cast::<i16>(), sample_count);
                        convert_to_stereo_i16_from_i16(input, setup.channel_count)
                    }
                }
            };

            setup
                .capture_client
                .ReleaseBuffer(frame_count)
                .context("failed to release loopback packet")?;

            if sender.send(samples).is_err() {
                break;
            }
        }

        let _ = setup.audio_client.Stop();
        CoUninitialize();
        Ok(())
    }

    unsafe fn parse_sample_format(format_ptr: *mut WAVEFORMATEX) -> Result<CaptureSampleFormat> {
        let format = ptr::read_unaligned(format_ptr);
        let format_tag = format.wFormatTag;
        let bits_per_sample = format.wBitsPerSample;
        match format_tag {
            value if value == WAVE_FORMAT_IEEE_FLOAT => Ok(CaptureSampleFormat::F32),
            value if value == WAVE_FORMAT_PCM => {
                if bits_per_sample == 16 {
                    Ok(CaptureSampleFormat::I16)
                } else {
                    bail!("PCM capture only supports 16-bit samples right now")
                }
            }
            value if value == WAVE_FORMAT_EXTENSIBLE => {
                let extensible = ptr::read_unaligned(format_ptr as *const WAVEFORMATEXTENSIBLE);
                let sub_format = extensible.SubFormat;
                if sub_format == SUBTYPE_IEEE_FLOAT {
                    Ok(CaptureSampleFormat::F32)
                } else if sub_format == SUBTYPE_PCM && bits_per_sample == 16 {
                    Ok(CaptureSampleFormat::I16)
                } else {
                    bail!("unsupported extensible capture format")
                }
            }
            other => Err(anyhow!("unsupported loopback format tag {other}")),
        }
    }

    fn convert_to_stereo_i16_from_f32(input: &[f32], channels: usize) -> Vec<i16> {
        let mut output = Vec::with_capacity(input.len().max(2));
        for frame in input.chunks(channels.max(1)) {
            let left = frame.first().copied().unwrap_or(0.0).clamp(-1.0, 1.0);
            let right = frame.get(1).copied().unwrap_or(left).clamp(-1.0, 1.0);
            output.push((left * i16::MAX as f32) as i16);
            output.push((right * i16::MAX as f32) as i16);
        }
        output
    }

    fn convert_to_stereo_i16_from_i16(input: &[i16], channels: usize) -> Vec<i16> {
        let mut output = Vec::with_capacity(input.len().max(2));
        for frame in input.chunks(channels.max(1)) {
            let left = frame.first().copied().unwrap_or(0);
            let right = frame.get(1).copied().unwrap_or(left);
            output.push(left);
            output.push(right);
        }
        output
    }
}

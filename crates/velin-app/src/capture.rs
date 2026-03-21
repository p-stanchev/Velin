use anyhow::Result;
use tokio::sync::mpsc;

pub struct SystemAudioCapture {
    sample_rate_hz: u32,
    receiver: mpsc::UnboundedReceiver<Vec<i16>>,
    _inner: PlatformCapture,
}

impl SystemAudioCapture {
    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    pub async fn recv(&mut self) -> Option<Vec<i16>> {
        self.receiver.recv().await
    }
}

pub fn start_system_audio_capture() -> Result<SystemAudioCapture> {
    let (sample_rate_hz, receiver, inner) = platform::start_capture()?;

    Ok(SystemAudioCapture {
        sample_rate_hz,
        receiver,
        _inner: inner,
    })
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

    pub fn start_capture() -> Result<(u32, mpsc::UnboundedReceiver<Vec<i16>>, LinuxSystemCapture)> {
        let monitor_source = detect_monitor_source();
        let sample_rate_hz = detect_monitor_sample_rate(monitor_source.as_deref()).unwrap_or(SAMPLE_RATE_HZ);
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
                    format!("--rate={sample_rate_hz}"),
                    "--channels=2".to_string(),
                ],
                description: format!("parec monitor source {source}"),
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

    fn detect_monitor_source() -> Option<String> {
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

    fn detect_monitor_sample_rate(selected_source: Option<&str>) -> Option<u32> {
        let output = Command::new("pactl").args(["list", "sources"]).output().ok()?;
        if !output.status.success() {
            return None;
        }

        let selected_source = selected_source
            .filter(|source| *source != "@DEFAULT_MONITOR@")
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
                    .unwrap_or_else(|| {
                        current_name
                            .as_deref()
                            .is_some_and(|name| name.ends_with(".monitor"))
                    });

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

    pub fn start_capture() -> Result<(u32, mpsc::UnboundedReceiver<Vec<i16>>, UnsupportedCapture)> {
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

    pub fn start_capture() -> Result<(u32, mpsc::UnboundedReceiver<Vec<i16>>, WindowsSystemCapture)> {
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

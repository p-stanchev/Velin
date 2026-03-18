use anyhow::{Context, Result, anyhow, bail};
use slint::{CloseRequestResponse, ComponentHandle};
use std::env;
use std::f32::consts::TAU;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::watch;
use tokio::time;
use velin_proto::{
    Accept, AudioFrame, CHANNELS, DEFAULT_AUDIO_PORT, DEFAULT_CONTROL_PORT, FRAME_SAMPLES, Hello,
    SAMPLE_RATE_HZ,
};

type StatusSink = Arc<dyn Fn(String) + Send + Sync>;

slint::slint! {
    import { Button, LineEdit, HorizontalBox, VerticalBox } from "std-widgets.slint";

    export component AppWindow inherits Window {
        in-out property <string> target-ip: "127.0.0.1";
        in-out property <string> status-text: "Idle";
        in-out property <bool> running: false;
        callback start-target();
        callback start-source(string);
        callback stop-session();

        title: "Velin";
        width: 460px;
        height: 320px;

        Rectangle {
            background: #121212;

            VerticalBox {
                padding: 16px;
                spacing: 10px;

                Text {
                    text: "Velin";
                    color: #f2f2f2;
                    font-size: 22px;
                    font-weight: 600;
                }

                Rectangle {
                    height: 1px;
                    background: #2a2a2a;
                }

                Text {
                    text: "Target IP";
                    color: #bdbdbd;
                    font-size: 13px;
                }

                LineEdit {
                    text <=> root.target-ip;
                    enabled: !root.running;
                    placeholder-text: "127.0.0.1";
                }

                HorizontalBox {
                    spacing: 8px;

                    Button {
                        text: "Target";
                        enabled: !root.running;
                        clicked => {
                            root.start-target();
                        }
                    }

                    Button {
                        text: "Source";
                        enabled: !root.running;
                        clicked => {
                            root.start-source(root.target-ip);
                        }
                    }

                    Button {
                        text: "Stop";
                        enabled: root.running;
                        clicked => {
                            root.stop-session();
                        }
                    }
                }

                Rectangle {
                    border-color: #2a2a2a;
                    border-width: 1px;
                    background: #181818;
                    height: 100px;

                    VerticalBox {
                        padding: 12px;
                        spacing: 6px;

                        Text {
                            text: root.running ? "Running" : "Status";
                            color: #f2f2f2;
                            font-size: 14px;
                            font-weight: 600;
                        }

                        Text {
                            text: root.status-text;
                            color: #cfcfcf;
                            wrap: word-wrap;
                        }
                    }
                }
            }
        }
    }
}

#[derive(Default)]
struct SessionState {
    current: Option<watch::Sender<bool>>,
    exit_when_stopped: bool,
}

fn main() -> Result<()> {
    let runtime = Arc::new(
        Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("failed to build tokio runtime")?,
    );

    let args: Vec<String> = env::args().collect();
    if matches!(args.get(1).map(String::as_str), Some("gui")) {
        return run_gui(runtime);
    }
    if args.len() > 1 {
        return runtime.block_on(run_cli(&args));
    }

    run_gui(runtime)
}

async fn run_cli(args: &[String]) -> Result<()> {
    let mode = args.get(1).map(String::as_str).ok_or_else(usage)?;
    let status = cli_status_sink();
    let (_, stop_rx) = watch::channel(false);

    match mode {
        "target" | "listen" => run_target(status, stop_rx).await,
        "source" | "connect" => {
            let address = args.get(2).map(String::as_str).ok_or_else(usage)?;
            run_source(address, status, stop_rx).await
        }
        _ => Err(usage()),
    }
}

fn run_gui(runtime: Arc<Runtime>) -> Result<()> {
    let app = AppWindow::new().context("failed to create Slint app window")?;
    let session_state = Arc::new(Mutex::new(SessionState::default()));

    {
        let runtime = Arc::clone(&runtime);
        let weak = app.as_weak();
        let session_state = Arc::clone(&session_state);
        app.on_start_target(move || {
            if is_running(&session_state) {
                return;
            }

            let weak = weak.clone();
            let status = ui_status_sink(&weak);
            let stop_rx = begin_session(&session_state);
            set_running(&weak, true);
            status("Starting target...".to_string());

            let session_state = Arc::clone(&session_state);
            runtime.spawn(async move {
                let result = run_target(status.clone(), stop_rx).await;
                finish_session(&weak, &session_state, status, result);
            });
        });
    }

    {
        let runtime = Arc::clone(&runtime);
        let weak = app.as_weak();
        let session_state = Arc::clone(&session_state);
        app.on_start_source(move |target_ip| {
            if is_running(&session_state) {
                return;
            }

            let weak = weak.clone();
            let target_ip = target_ip.to_string();
            if target_ip.trim().is_empty() {
                set_status(&weak, "Enter a target IP address.".to_string());
                return;
            }

            let status = ui_status_sink(&weak);
            let stop_rx = begin_session(&session_state);
            set_running(&weak, true);
            status(format!("Starting source for {target_ip}..."));

            let session_state = Arc::clone(&session_state);
            runtime.spawn(async move {
                let result = run_source(&target_ip, status.clone(), stop_rx).await;
                finish_session(&weak, &session_state, status, result);
            });
        });
    }

    {
        let weak = app.as_weak();
        let session_state = Arc::clone(&session_state);
        app.on_stop_session(move || {
            request_stop(&weak, &session_state, false);
        });
    }

    {
        let weak = app.as_weak();
        let session_state = Arc::clone(&session_state);
        app.window().on_close_requested(move || {
            if is_running(&session_state) {
                request_stop(&weak, &session_state, true);
                CloseRequestResponse::KeepWindowShown
            } else {
                CloseRequestResponse::HideWindow
            }
        });
    }

    app.run().context("failed to run Slint event loop")
}

fn usage() -> anyhow::Error {
    anyhow!(
        "usage:\n  cargo run -p velin-app\n  cargo run -p velin-app -- listen\n  cargo run -p velin-app -- connect <target-ip>\n\nlegacy aliases:\n  target -> listen\n  source <ip> -> connect <ip>"
    )
}

async fn run_target(status: StatusSink, mut stop_rx: watch::Receiver<bool>) -> Result<()> {
    let control_addr = format!("0.0.0.0:{DEFAULT_CONTROL_PORT}");
    let audio_addr = format!("0.0.0.0:{DEFAULT_AUDIO_PORT}");

    let listener = TcpListener::bind(&control_addr)
        .await
        .with_context(|| format!("failed to bind control listener on {control_addr}"))?;
    let audio_socket = UdpSocket::bind(&audio_addr)
        .await
        .with_context(|| format!("failed to bind audio socket on {audio_addr}"))?;

    status(format!("Target listening on {control_addr}."));

    let (mut stream, peer_addr) = tokio::select! {
        result = listener.accept() => result.context("failed to accept source")?,
        _ = wait_for_stop(&mut stop_rx) => return Ok(()),
    };
    let hello: Hello = read_json_message(&mut stream).await?;

    status(format!(
        "Source {} connected from {peer_addr}.",
        hello.source_name
    ));

    let accept = Accept {
        target_name: host_name(),
        audio_port: DEFAULT_AUDIO_PORT,
    };
    write_json_message(&mut stream, &accept).await?;

    let mut packet = vec![0_u8; 2048];
    let mut received_frames = 0_u64;
    let mut last_sequence = None;
    let started = Instant::now();

    loop {
        let (len, from) = tokio::select! {
            result = audio_socket.recv_from(&mut packet) => result.context("failed to receive audio frame")?,
            _ = wait_for_stop(&mut stop_rx) => return Ok(()),
        };

        let Some(frame) = AudioFrame::decode(&packet[..len]) else {
            status(format!("Discarded malformed packet from {from}."));
            continue;
        };

        if let Some(previous) = last_sequence {
            let expected = previous + 1;
            if frame.sequence != expected {
                status(format!(
                    "Frame gap: expected {expected}, got {}.",
                    frame.sequence
                ));
            }
        }

        last_sequence = Some(frame.sequence);
        received_frames += 1;

        if received_frames == 1 || received_frames % 100 == 0 {
            let seconds = started.elapsed().as_secs_f32();
            status(format!(
                "Received {received_frames} frames after {seconds:.1}s from {from}."
            ));
        }
    }
}

async fn run_source(target_ip: &str, status: StatusSink, mut stop_rx: watch::Receiver<bool>) -> Result<()> {
    let control_addr = format!("{target_ip}:{DEFAULT_CONTROL_PORT}");
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
    let audio_addr = format!("{target_ip}:{}", accept.audio_port);

    let audio_socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .context("failed to bind local UDP socket")?;
    audio_socket
        .connect(&audio_addr)
        .await
        .with_context(|| format!("failed to connect audio socket to {audio_addr}"))?;

    status(format!("Connected to {}. Streaming to {audio_addr}.", accept.target_name));

    let mut ticker = time::interval(frame_duration());
    let mut phase = 0.0_f32;
    let step = 440.0_f32 / SAMPLE_RATE_HZ as f32;

    for sequence in 0_u64.. {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = wait_for_stop(&mut stop_rx) => return Ok(()),
        }

        let mut samples = Vec::with_capacity(FRAME_SAMPLES * CHANNELS as usize);
        for _ in 0..FRAME_SAMPLES {
            let sample = (phase * TAU).sin();
            let pcm = (sample * i16::MAX as f32 * 0.2) as i16;
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

fn cli_status_sink() -> StatusSink {
    Arc::new(|message| {
        println!("{message}");
    })
}

fn ui_status_sink(weak: &slint::Weak<AppWindow>) -> StatusSink {
    let weak = weak.clone();
    Arc::new(move |message| {
        set_status(&weak, message);
    })
}

fn begin_session(state: &Arc<Mutex<SessionState>>) -> watch::Receiver<bool> {
    let (stop_tx, stop_rx) = watch::channel(false);
    let mut state = state.lock().expect("session state poisoned");
    state.current = Some(stop_tx);
    state.exit_when_stopped = false;
    stop_rx
}

fn finish_session(
    weak: &slint::Weak<AppWindow>,
    state: &Arc<Mutex<SessionState>>,
    status: StatusSink,
    result: Result<()>,
) {
    let exit_when_stopped = {
        let mut state = state.lock().expect("session state poisoned");
        let exit = state.exit_when_stopped;
        state.current = None;
        state.exit_when_stopped = false;
        exit
    };

    match result {
        Ok(()) => status("Stopped.".to_string()),
        Err(error) => status(format!("Session failed: {error:#}")),
    }

    set_running(weak, false);

    if exit_when_stopped {
        let _ = slint::quit_event_loop();
    }
}

fn request_stop(weak: &slint::Weak<AppWindow>, state: &Arc<Mutex<SessionState>>, exit_when_stopped: bool) {
    let sender = {
        let mut state = state.lock().expect("session state poisoned");
        state.exit_when_stopped = exit_when_stopped;
        state.current.clone()
    };

    if let Some(sender) = sender {
        let _ = sender.send(true);
        set_status(
            weak,
            if exit_when_stopped {
                "Stopping session and closing...".to_string()
            } else {
                "Stopping session...".to_string()
            },
        );
    } else if exit_when_stopped {
        let _ = slint::quit_event_loop();
    }
}

fn is_running(state: &Arc<Mutex<SessionState>>) -> bool {
    state.lock().expect("session state poisoned").current.is_some()
}

fn set_status(weak: &slint::Weak<AppWindow>, message: String) {
    let _ = weak.upgrade_in_event_loop(move |app| {
        app.set_status_text(message.into());
    });
}

fn set_running(weak: &slint::Weak<AppWindow>, running: bool) {
    let _ = weak.upgrade_in_event_loop(move |app| {
        app.set_running(running);
    });
}

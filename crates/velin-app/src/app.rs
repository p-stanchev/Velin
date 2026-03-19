use crate::audio::{default_output_device_name, output_device_names};
use crate::settings::{AppSettings, SettingsStore, ThemeMode};
use crate::transport::{
    StatusSink, local_ipv4_addresses, local_ipv4_summary, local_machine_name, local_primary_ipv4,
    run_source, run_target,
};
use crate::ui::AppWindow;
use anyhow::{Context, Result, anyhow};
use slint::{CloseRequestResponse, ComponentHandle, ModelRc, VecModel};
use std::process::Command;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio::sync::watch;

#[derive(Default)]
struct SessionState {
    current: Option<watch::Sender<bool>>,
    exit_when_stopped: bool,
}

pub async fn run_cli(args: &[String]) -> Result<()> {
    let mode = args.get(1).map(String::as_str).ok_or_else(usage)?;
    let settings = SettingsStore::new()?
        .load_or_default()
        .context("failed to load settings")?;
    let config = settings.session_config();
    let status = cli_status_sink();
    let (_stop_tx, stop_rx) = watch::channel(false);

    match mode {
        "target" | "listen" => run_target(config, status, stop_rx).await,
        "source" | "connect" => {
            let address = args.get(2).map(String::as_str).ok_or_else(usage)?;
            let mut config = config;
            config.target_ip = address.to_string();
            run_source(config, status, stop_rx).await
        }
        _ => Err(usage()),
    }
}

pub fn run_gui(runtime: Arc<Runtime>) -> Result<()> {
    let store = Arc::new(SettingsStore::new()?);
    let settings = Arc::new(Mutex::new(store.load_or_default()?));
    let app = AppWindow::new().context("failed to create Slint app window")?;
    let session_state = Arc::new(Mutex::new(SessionState::default()));

    {
        let current = settings.lock().expect("settings poisoned").clone();
        let bind_ip_options = bind_ip_options();
        let output_device_options = output_device_options();
        app.set_target_ip(current.target_ip.clone().into());
        app.set_bind_ip(current.bind_ip.clone().into());
        app.set_bind_ip_options(ModelRc::new(VecModel::from(bind_ip_options.clone())));
        app.set_bind_ip_selection(bind_ip_selection_label(&current.bind_ip, &bind_ip_options).into());
        app.set_output_device_options(ModelRc::new(VecModel::from(output_device_options.clone())));
        app.set_output_device_selection(
            output_device_selection_label(&current.output_device_name, &output_device_options).into(),
        );
        app.set_control_port(current.control_port.to_string().into());
        app.set_audio_port(current.audio_port.to_string().into());
        app.set_dark_mode(matches!(current.theme_mode, ThemeMode::Dark));
        app.set_local_addresses(local_ipv4_summary().into());
        app.set_local_machine_name(local_machine_name().into());
        app.set_local_primary_ip(local_primary_ipv4().into());
        app.set_status_text("Idle".into());
        app.set_log_text("Velin ready.\n".into());
    }

    {
        let app_handle = app.as_weak();
        app.on_select_session_tab(move || {
            if let Some(app) = app_handle.upgrade() {
                app.set_active_tab(0);
            }
        });
    }

    {
        let app_handle = app.as_weak();
        app.on_select_settings_tab(move || {
            if let Some(app) = app_handle.upgrade() {
                app.set_active_tab(1);
            }
        });
    }

    {
        let store = Arc::clone(&store);
        let settings = Arc::clone(&settings);
        let weak = app.as_weak();
        app.on_save_settings(move |target_ip, bind_ip_selection, output_device_selection, control_port, audio_port, dark_mode| {
            let target_ip = target_ip.to_string();
            let bind_ip_selection = bind_ip_selection.to_string();
            let output_device_selection = output_device_selection.to_string();
            let status = ui_status_sink(&weak);
            match parse_settings_input(
                &target_ip,
                &selected_bind_ip(&bind_ip_selection),
                &selected_output_device(&output_device_selection),
                control_port.as_str(),
                audio_port.as_str(),
                dark_mode,
            ) {
                Ok(next) => {
                    if let Err(error) = persist_settings(&store, &settings, &weak, &next) {
                        status(format!("Failed to save settings: {error:#}"));
                    }
                }
                Err(_) => {}
            }
        });
    }

    {
        let weak = app.as_weak();
        app.on_report_bug(move || {
            if let Err(error) = open_bug_report_page() {
                set_status(&weak, format!("Failed to open bug report page. {error}"));
            }
        });
    }

    {
        let runtime = Arc::clone(&runtime);
        let weak = app.as_weak();
        let session_state = Arc::clone(&session_state);
        let settings = Arc::clone(&settings);
        app.on_start_target(move || {
            if is_running(&session_state) {
                return;
            }

            let weak = weak.clone();
            let status = ui_status_sink(&weak);
            let stop_rx = begin_session(&session_state);
            let config = settings.lock().expect("settings poisoned").session_config();
            set_running(&weak, true);
            status("Starting target...".to_string());

            let session_state = Arc::clone(&session_state);
            runtime.spawn(async move {
                let result = run_target(config, status.clone(), stop_rx).await;
                finish_session(&weak, &session_state, status, result);
            });
        });
    }

    {
        let runtime = Arc::clone(&runtime);
        let weak = app.as_weak();
        let session_state = Arc::clone(&session_state);
        let settings = Arc::clone(&settings);
        app.on_start_source(move |target_ip| {
            if is_running(&session_state) {
                return;
            }

            let target_ip = target_ip.to_string();
            if target_ip.trim().is_empty() {
                set_status(&weak, "Enter a target IP address.".to_string());
                return;
            }

            {
                let mut current = settings.lock().expect("settings poisoned");
                current.target_ip = target_ip.clone();
            }

            let weak = weak.clone();
            let status = ui_status_sink(&weak);
            let stop_rx = begin_session(&session_state);
            let mut config = settings.lock().expect("settings poisoned").session_config();
            config.target_ip = target_ip.clone();
            set_running(&weak, true);
            status(format!("Starting source for {target_ip}..."));

            let session_state = Arc::clone(&session_state);
            runtime.spawn(async move {
                let result = run_source(config, status.clone(), stop_rx).await;
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

fn bind_ip_options() -> Vec<slint::SharedString> {
    let mut options = vec![slint::SharedString::from("Automatic")];
    options.extend(
        local_ipv4_addresses()
            .into_iter()
            .map(slint::SharedString::from),
    );
    options
}

fn output_device_options() -> Vec<slint::SharedString> {
    let mut options = vec![slint::SharedString::from("System Default")];
    if let Ok(names) = output_device_names() {
        options.extend(names.into_iter().map(slint::SharedString::from));
    }
    options
}

fn persist_settings(
    store: &Arc<SettingsStore>,
    settings: &Arc<Mutex<AppSettings>>,
    weak: &slint::Weak<AppWindow>,
    next: &AppSettings,
) -> Result<()> {
    {
        let mut settings = settings.lock().expect("settings poisoned");
        *settings = next.clone();
    }

    store.save(next)?;

    if let Some(app) = weak.upgrade() {
        let bind_ip_options = bind_ip_options();
        let output_device_options = output_device_options();
        app.set_target_ip(next.target_ip.clone().into());
        app.set_bind_ip(next.bind_ip.clone().into());
        app.set_bind_ip_options(ModelRc::new(VecModel::from(bind_ip_options.clone())));
        app.set_bind_ip_selection(bind_ip_selection_label(&next.bind_ip, &bind_ip_options).into());
        app.set_output_device_options(ModelRc::new(VecModel::from(output_device_options.clone())));
        app.set_output_device_selection(
            output_device_selection_label(&next.output_device_name, &output_device_options).into(),
        );
        app.set_control_port(next.control_port.to_string().into());
        app.set_audio_port(next.audio_port.to_string().into());
        app.set_dark_mode(matches!(next.theme_mode, ThemeMode::Dark));
    }

    Ok(())
}

fn open_bug_report_page() -> Result<()> {
    let url = "https://github.com/p-stanchev/Velin/issues";

    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .context("could not launch browser")?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(url)
            .spawn()
            .context("could not launch browser")?;
        return Ok(());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(url)
            .spawn()
            .context("could not launch browser")?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    Err(anyhow!("unsupported platform"))
}

fn bind_ip_selection_label(bind_ip: &str, options: &[slint::SharedString]) -> String {
    let normalized = bind_ip.trim();
    if normalized.is_empty() || normalized == "0.0.0.0" {
        return "Automatic".to_string();
    }

    if options.iter().any(|value| value.as_str() == normalized) {
        normalized.to_string()
    } else {
        "Automatic".to_string()
    }
}

fn output_device_selection_label(
    output_device_name: &str,
    options: &[slint::SharedString],
) -> String {
    let normalized = output_device_name.trim();
    if normalized.is_empty() {
        return default_output_device_name()
            .ok()
            .flatten()
            .filter(|name| options.iter().any(|value| value.as_str() == name))
            .unwrap_or_else(|| "System Default".to_string());
    }

    if options.iter().any(|value| value.as_str() == normalized) {
        normalized.to_string()
    } else {
        "System Default".to_string()
    }
}

fn selected_bind_ip(selection: &str) -> String {
    if selection.trim().is_empty() || selection == "Automatic" {
        "0.0.0.0".to_string()
    } else {
        selection.trim().to_string()
    }
}

fn selected_output_device(selection: &str) -> String {
    if selection.trim().is_empty() || selection == "System Default" {
        String::new()
    } else {
        selection.trim().to_string()
    }
}

pub fn usage() -> anyhow::Error {
    anyhow!(
        "usage:\n  cargo run -p velin-app\n  cargo run -p velin-app -- listen\n  cargo run -p velin-app -- connect <target-ip>\n\nlegacy aliases:\n  target -> listen\n  source <ip> -> connect <ip>"
    )
}

fn parse_settings_input(
    target_ip: &str,
    bind_ip: &str,
    output_device_name: &str,
    control_port: &str,
    audio_port: &str,
    dark_mode: bool,
) -> Result<AppSettings> {
    let control_port = control_port
        .trim()
        .parse::<u16>()
        .context("control port must be a valid u16")?;
    let audio_port = audio_port
        .trim()
        .parse::<u16>()
        .context("audio port must be a valid u16")?;

    Ok(AppSettings {
        target_ip: target_ip.trim().to_string(),
        bind_ip: bind_ip.trim().to_string(),
        output_device_name: output_device_name.trim().to_string(),
        control_port,
        audio_port,
        theme_mode: if dark_mode {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
    })
}

fn cli_status_sink() -> StatusSink {
    Arc::new(|message| {
        println!("{message}");
    })
}

fn ui_status_sink(weak: &slint::Weak<AppWindow>) -> StatusSink {
    let weak = weak.clone();
    Arc::new(move |message| {
        append_status(&weak, message);
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
        Err(error) => status(describe_session_error(&error)),
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

fn append_status(weak: &slint::Weak<AppWindow>, message: String) {
    let _ = weak.upgrade_in_event_loop(move |app| {
        let new_log = push_log_line(&app.get_log_text().to_string(), &message);
        app.set_status_text(message.clone().into());
        app.set_log_text(new_log.into());
    });
}

fn push_log_line(existing: &str, line: &str) -> String {
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect();

    lines.push(line.to_string());
    if lines.len() > 14 {
        let drain_count = lines.len() - 14;
        lines.drain(0..drain_count);
    }

    let mut output = lines.join("\n");
    output.push('\n');
    output
}

fn describe_session_error(error: &anyhow::Error) -> String {
    let text = format!("{error:#}");
    let lower = text.to_lowercase();

    if lower.contains("actively refused") || lower.contains("os error 10061") {
        return "Connection failed. No receiver is listening at that IP and port.".to_string();
    }

    if lower.contains("failed to bind control listener")
        || lower.contains("failed to bind audio socket")
        || lower.contains("address already in use")
    {
        return "Start failed. One of the configured ports is already in use.".to_string();
    }

    if lower.contains("failed to bind local udp socket") {
        return "Start failed. Could not create the local audio socket.".to_string();
    }

    format!("Session failed. {text}")
}

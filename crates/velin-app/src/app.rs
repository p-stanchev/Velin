use crate::security::SecurityStore;
use crate::audio::{default_output_device_name, output_device_names};
use crate::discovery::{DiscoveredPeer, DiscoveryAdvertiser, PeerUpdateSink, request_discovery, run_discovery_service};
use crate::settings::{AppSettings, PreferredPeer, SettingsStore, ThemeMode};
use crate::transport::{
    MetricsSink, PairingPrompt, PairingRequest, StatusSink, advertised_ipv4_addresses_for, local_ipv4_addresses,
    local_ipv4_summary, local_machine_name, local_primary_ipv4, run_source_with_reconnect,
    run_target,
};
use crate::ui::AppWindow;
use anyhow::{Context, Result, anyhow};
use slint::{CloseRequestResponse, ComponentHandle, ModelRc, VecModel};
use std::process::Command;
use std::collections::HashSet;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio::sync::watch;

#[derive(Default)]
struct SessionState {
    current: Option<SessionControls>,
    exit_when_stopped: bool,
}

#[derive(Clone)]
struct SessionControls {
    stop_tx: watch::Sender<bool>,
    mute_tx: watch::Sender<bool>,
}

#[derive(Debug, Clone)]
struct PeerChoice {
    label: String,
    ip: String,
}

#[derive(Debug, Clone)]
struct TrustedFingerprintChoice {
    label: String,
    fingerprint: String,
}

#[derive(Default)]
struct PairingPromptState {
    pending: bool,
    peer_name: String,
    fingerprint: String,
    role: String,
    decision_tx: Option<mpsc::Sender<bool>>,
}

pub async fn run_cli(args: &[String]) -> Result<()> {
    let mode = args.get(1).map(String::as_str).ok_or_else(usage)?;
    let settings = SettingsStore::new()?
        .load_or_default()
        .context("failed to load settings")?;
    let config = settings.session_config();
    let status = cli_status_sink();
    let (stop_tx, stop_rx) = watch::channel(false);
    let (_mute_tx, mute_rx) = watch::channel(false);
    let ctrl_c_status = Arc::clone(&status);
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = stop_tx.send(true);
            ctrl_c_status("Ctrl+C received. Stopping session...".to_string());
        }
    });

    match mode {
        "target" | "listen" => run_target(config, status, None, None, stop_rx, mute_rx).await,
        "source" | "connect" => {
            let address = args.get(2).map(String::as_str).ok_or_else(usage)?;
            let mut config = config;
            config.target_ip = address.to_string();
            run_source_with_reconnect(config, status, None, None, stop_rx, mute_rx).await
        }
        _ => Err(usage()),
    }
}

pub fn run_gui(runtime: Arc<Runtime>) -> Result<()> {
    let store = Arc::new(SettingsStore::new()?);
    let settings = Arc::new(Mutex::new(store.load_or_default()?));
    let discovered_peers = Arc::new(Mutex::new(Vec::<DiscoveredPeer>::new()));
    let peer_choices = Arc::new(Mutex::new(Vec::<PeerChoice>::new()));
    let trusted_fingerprints = Arc::new(Mutex::new(Vec::<TrustedFingerprintChoice>::new()));
    let pairing_state = Arc::new(Mutex::new(PairingPromptState::default()));
    let discovery_advertiser = DiscoveryAdvertiser::default();
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
        app.set_metrics_text("No active stream.\n".into());
        app.set_log_text("Velin ready.\n".into());
        app.set_pairing_pending(false);
        app.set_pairing_peer_name("".into());
        app.set_pairing_fingerprint("".into());
        app.set_pairing_role("".into());
        apply_peer_options(&app, &settings.lock().expect("settings poisoned"), &peer_choices, &[]);
        refresh_security_settings(&app, &trusted_fingerprints)?;
    }

    {
        let weak = app.as_weak();
        let discovered_peers = Arc::clone(&discovered_peers);
        let peer_choices = Arc::clone(&peer_choices);
        let settings = Arc::clone(&settings);
        let update: PeerUpdateSink = Arc::new(move |peers| {
            {
                let mut state = discovered_peers.lock().expect("discovered peers poisoned");
                *state = peers.clone();
            }

            let settings = Arc::clone(&settings);
            let peer_choices = Arc::clone(&peer_choices);
            let _ = weak.upgrade_in_event_loop(move |app| {
                apply_peer_options(
                    &app,
                    &settings.lock().expect("settings poisoned"),
                    &peer_choices,
                    &peers,
                );
            });
        });

        let advertiser = discovery_advertiser.clone();
        runtime.spawn(async move {
            let _ = run_discovery_service(update, advertiser).await;
        });
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
                app.set_active_tab(2);
                app.set_discovered_peer_menu_open(false);
            }
        });
    }

    {
        let app_handle = app.as_weak();
        app.on_select_metrics_tab(move || {
            if let Some(app) = app_handle.upgrade() {
                app.set_active_tab(1);
                app.set_discovered_peer_menu_open(false);
            }
        });
    }

    {
        let store = Arc::clone(&store);
        let settings = Arc::clone(&settings);
        let trusted_fingerprints = Arc::clone(&trusted_fingerprints);
        let weak = app.as_weak();
        app.on_save_settings(move |target_ip, bind_ip_selection, output_device_selection, control_port, audio_port, dark_mode| {
            let target_ip = target_ip.to_string();
            let bind_ip_selection = bind_ip_selection.to_string();
            let output_device_selection = output_device_selection.to_string();
            let status = ui_status_sink(&weak);
            let preferred_peers = settings
                .lock()
                .expect("settings poisoned")
                .preferred_peers
                .clone();
            match parse_settings_input(
                &target_ip,
                &selected_bind_ip(&bind_ip_selection),
                &selected_output_device(&output_device_selection),
                control_port.as_str(),
                audio_port.as_str(),
                dark_mode,
                preferred_peers,
            ) {
                Ok(next) => {
                    if let Err(error) = persist_settings(&store, &settings, &weak, &next) {
                        status(format!("Failed to save settings: {error:#}"));
                    } else if let Some(app) = weak.upgrade() {
                        if let Err(error) = refresh_security_settings(&app, &trusted_fingerprints) {
                            status(format!("Failed to refresh security settings: {error:#}"));
                        }
                    }
                }
                Err(_) => {}
            }
        });
    }

    {
        let weak = app.as_weak();
        let peer_choices = Arc::clone(&peer_choices);
        let settings = Arc::clone(&settings);
        let store = Arc::clone(&store);
        app.on_choose_discovered_peer(move |label| {
            let label = label.to_string();
            let peer = peer_choices
                .lock()
                .expect("peer choices poisoned")
                .iter()
                .find(|peer| peer.label == label)
                .cloned();

            if let Some(peer) = peer {
                remember_preferred_peer(&store, &settings, &peer.label, &peer.ip);
                let _ = weak.upgrade_in_event_loop(move |app| {
                    app.set_target_ip(peer.ip.clone().into());
                    app.set_discovered_peer_selection(peer.label.clone().into());
                    app.set_discovered_peer_menu_open(false);
                });
            }
        });
    }

    {
        let runtime = Arc::clone(&runtime);
        let weak = app.as_weak();
        app.on_refresh_discovery(move || {
            let weak = weak.clone();
            runtime.spawn(async move {
                if let Err(error) = request_discovery(local_machine_name()).await {
                    set_status(&weak, format!("Discovery refresh failed. {error}"));
                } else {
                    set_status(&weak, "Refreshing receivers...".to_string());
                }
            });
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
        let weak = app.as_weak();
        let trusted_fingerprints = Arc::clone(&trusted_fingerprints);
        app.on_save_trust_validity(move |days| {
            let status = ui_status_sink(&weak);
            match days.trim().parse::<u32>() {
                Ok(days) if days > 0 => {
                    match SecurityStore::load_or_create().and_then(|mut store| store.set_trust_validity_days(days)) {
                        Ok(()) => {
                            if let Some(app) = weak.upgrade() {
                                if let Err(error) = refresh_security_settings(&app, &trusted_fingerprints) {
                                    status(format!("Failed to refresh fingerprint settings: {error:#}"));
                                } else {
                                    status(format!("Fingerprint validity set to {days} days."));
                                }
                            }
                        }
                        Err(error) => status(format!("Failed to save fingerprint validity: {error:#}")),
                    }
                }
                _ => status("Fingerprint validity must be a positive whole number of days.".to_string()),
            }
        });
    }

    {
        let weak = app.as_weak();
        let trusted_fingerprints = Arc::clone(&trusted_fingerprints);
        app.on_clear_selected_fingerprint(move |label| {
            let status = ui_status_sink(&weak);
            let label = label.to_string();
            let fingerprint = trusted_fingerprints
                .lock()
                .expect("trusted fingerprints poisoned")
                .iter()
                .find(|entry| entry.label == label)
                .map(|entry| entry.fingerprint.clone());

            let Some(fingerprint) = fingerprint else {
                status("Select a fingerprint to clear.".to_string());
                return;
            };

            match SecurityStore::load_or_create().and_then(|mut store| store.remove_trusted_peer_by_fingerprint(&fingerprint)) {
                Ok(true) => {
                    if let Some(app) = weak.upgrade() {
                        if let Err(error) = refresh_security_settings(&app, &trusted_fingerprints) {
                            status(format!("Failed to refresh fingerprint settings: {error:#}"));
                        } else {
                            status(format!("Cleared fingerprint {fingerprint}."));
                        }
                    }
                }
                Ok(false) => status("Selected fingerprint was already missing.".to_string()),
                Err(error) => status(format!("Failed to clear fingerprint: {error:#}")),
            }
        });
    }

    {
        let weak = app.as_weak();
        let trusted_fingerprints = Arc::clone(&trusted_fingerprints);
        app.on_clear_all_fingerprints(move || {
            let status = ui_status_sink(&weak);
            match SecurityStore::load_or_create().and_then(|mut store| store.clear_trusted_peers()) {
                Ok(()) => {
                    if let Some(app) = weak.upgrade() {
                        if let Err(error) = refresh_security_settings(&app, &trusted_fingerprints) {
                            status(format!("Failed to refresh fingerprint settings: {error:#}"));
                        } else {
                            status("Cleared all trusted fingerprints.".to_string());
                        }
                    }
                }
                Err(error) => status(format!("Failed to clear trusted fingerprints: {error:#}")),
            }
        });
    }

    let pairing_prompt = make_pairing_prompt(&app.as_weak(), &pairing_state);

    {
        let weak = app.as_weak();
        let pairing_state = Arc::clone(&pairing_state);
        app.on_pairing_decision(move |approved| {
            let sender = {
                let mut state = pairing_state.lock().expect("pairing state poisoned");
                state.pending = false;
                state.peer_name.clear();
                state.fingerprint.clear();
                state.role.clear();
                state.decision_tx.take()
            };
            if let Some(sender) = sender {
                let _ = sender.send(approved);
            }
            clear_pairing_prompt(&weak, &pairing_state);
        });
    }

    {
        let runtime = Arc::clone(&runtime);
        let weak = app.as_weak();
        let session_state = Arc::clone(&session_state);
        let settings = Arc::clone(&settings);
        let discovery_advertiser = discovery_advertiser.clone();
        let pairing_prompt = Arc::clone(&pairing_prompt);
        app.on_start_target(move || {
            if is_running(&session_state) {
                return;
            }

            let weak = weak.clone();
            let status = ui_status_sink(&weak);
            let metrics = ui_metrics_sink(&weak);
            let (stop_rx, mute_rx) = begin_session(&session_state);
            let config = settings.lock().expect("settings poisoned").session_config();
            discovery_advertiser.set(velin_proto::DiscoveryAnnouncement {
                machine_name: local_machine_name(),
                control_port: config.control_port,
                addresses: advertised_ipv4_addresses_for(&config.bind_ip),
            });
            set_running(&weak, true);
            set_muted(&weak, false);
            status("Starting target...".to_string());

            let session_state = Arc::clone(&session_state);
            let discovery_advertiser = discovery_advertiser.clone();
            let pairing_prompt = Arc::clone(&pairing_prompt);
            runtime.spawn(async move {
                let result = run_target(
                    config,
                    status.clone(),
                    Some(metrics),
                    Some(pairing_prompt),
                    stop_rx,
                    mute_rx,
                )
                .await;
                finish_session(&weak, &session_state, status, result, Some(&discovery_advertiser));
            });
        });
    }

    {
        let runtime = Arc::clone(&runtime);
        let weak = app.as_weak();
        let session_state = Arc::clone(&session_state);
        let settings = Arc::clone(&settings);
        let store = Arc::clone(&store);
        let pairing_prompt = Arc::clone(&pairing_prompt);
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
            remember_preferred_peer(&store, &settings, &target_ip, &target_ip);

            let weak = weak.clone();
            let status = ui_status_sink(&weak);
            let metrics = ui_metrics_sink(&weak);
            let (stop_rx, mute_rx) = begin_session(&session_state);
            let mut config = settings.lock().expect("settings poisoned").session_config();
            config.target_ip = target_ip.clone();
            set_running(&weak, true);
            set_muted(&weak, false);
            status(format!("Starting source for {target_ip}..."));

            let session_state = Arc::clone(&session_state);
            let pairing_prompt = Arc::clone(&pairing_prompt);
            runtime.spawn(async move {
                let result = run_source_with_reconnect(
                    config,
                    status.clone(),
                    Some(metrics),
                    Some(pairing_prompt),
                    stop_rx,
                    mute_rx,
                )
                .await;
                finish_session(&weak, &session_state, status, result, None);
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
        app.on_toggle_mute(move || {
            toggle_mute(&weak, &session_state);
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

    {
        let weak = app.as_weak();
        let session_state = Arc::clone(&session_state);
        runtime.spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                request_stop(&weak, &session_state, true);
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

fn refresh_security_settings(
    app: &AppWindow,
    trusted_fingerprints: &Arc<Mutex<Vec<TrustedFingerprintChoice>>>,
) -> Result<()> {
    let mut security = SecurityStore::load_or_create()?;
    let peers = security.trusted_peer_views()?;
    let validity_days = security.trust_validity_days();

    let choices = peers
        .iter()
        .map(|peer| TrustedFingerprintChoice {
            label: format!(
                "{} | {} | {}",
                peer.machine_name,
                peer.fingerprint,
                peer.expires_in_days
                    .map(|days| format!("{}d left", days))
                    .unwrap_or_else(|| "expired".to_string())
            ),
            fingerprint: peer.fingerprint.clone(),
        })
        .collect::<Vec<_>>();
    {
        let mut state = trusted_fingerprints
            .lock()
            .expect("trusted fingerprints poisoned");
        *state = choices.clone();
    }

    let options = if choices.is_empty() {
        vec![slint::SharedString::from("No trusted fingerprints")]
    } else {
        choices
            .iter()
            .map(|entry| slint::SharedString::from(entry.label.clone()))
            .collect::<Vec<_>>()
    };
    let selection = options
        .first()
        .cloned()
        .unwrap_or_else(|| slint::SharedString::from("No trusted fingerprints"));

    app.set_trust_validity_days(validity_days.to_string().into());
    app.set_trusted_fingerprint_options(ModelRc::new(VecModel::from(options)));
    if app.get_trusted_fingerprint_selection().is_empty()
        || app.get_trusted_fingerprint_selection() == "No trusted fingerprints"
    {
        app.set_trusted_fingerprint_selection(selection);
    } else if !choices
        .iter()
        .any(|entry| app.get_trusted_fingerprint_selection() == entry.label)
    {
        app.set_trusted_fingerprint_selection(selection);
    }

    Ok(())
}

fn apply_peer_options(
    app: &AppWindow,
    settings: &AppSettings,
    peer_choices: &Arc<Mutex<Vec<PeerChoice>>>,
    discovered_peers: &[DiscoveredPeer],
) {
    let choices = merged_peer_choices(&settings.preferred_peers, discovered_peers);
    {
        let mut state = peer_choices.lock().expect("peer choices poisoned");
        *state = choices.clone();
    }

    let labels: Vec<slint::SharedString> = if choices.is_empty() {
        vec![slint::SharedString::from("No receivers found")]
    } else {
        choices
            .iter()
            .map(|peer| slint::SharedString::from(peer.label.clone()))
            .collect()
    };

    let current_target_ip = app.get_target_ip().to_string();
    let selection = choices
        .iter()
        .find(|peer| peer.ip == current_target_ip)
        .or_else(|| choices.first())
        .map(|peer| peer.label.clone())
        .unwrap_or_else(|| "No receivers found".to_string());

    app.set_discovered_peer_options(ModelRc::new(VecModel::from(labels)));
    app.set_discovered_peer_selection(selection.into());
}

fn merged_peer_choices(preferred: &[PreferredPeer], discovered: &[DiscoveredPeer]) -> Vec<PeerChoice> {
    let mut seen = HashSet::<String>::new();
    let mut choices = Vec::new();

    for peer in discovered {
        if seen.insert(peer.ip.clone()) {
            choices.push(PeerChoice {
                label: peer.label.clone(),
                ip: peer.ip.clone(),
            });
        }
    }

    for peer in preferred {
        if seen.insert(peer.ip.clone()) {
            choices.push(PeerChoice {
                label: format!("Saved: {}", peer.label),
                ip: peer.ip.clone(),
            });
        }
    }

    choices
}

fn remember_preferred_peer(
    store: &Arc<SettingsStore>,
    settings: &Arc<Mutex<AppSettings>>,
    label: &str,
    ip: &str,
) {
    let mut current = settings.lock().expect("settings poisoned");
    current.target_ip = ip.to_string();
    current.preferred_peers.retain(|peer| peer.ip != ip);
    current.preferred_peers.insert(
        0,
        PreferredPeer {
            label: label.to_string(),
            ip: ip.to_string(),
        },
    );
    if current.preferred_peers.len() > 8 {
        current.preferred_peers.truncate(8);
    }
    let _ = store.save(&current);
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
    preferred_peers: Vec<PreferredPeer>,
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
        preferred_peers,
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

fn begin_session(state: &Arc<Mutex<SessionState>>) -> (watch::Receiver<bool>, watch::Receiver<bool>) {
    let (stop_tx, stop_rx) = watch::channel(false);
    let (mute_tx, mute_rx) = watch::channel(false);
    let mut state = state.lock().expect("session state poisoned");
    state.current = Some(SessionControls { stop_tx, mute_tx });
    state.exit_when_stopped = false;
    (stop_rx, mute_rx)
}

fn finish_session(
    weak: &slint::Weak<AppWindow>,
    state: &Arc<Mutex<SessionState>>,
    status: StatusSink,
    result: Result<()>,
    discovery_advertiser: Option<&DiscoveryAdvertiser>,
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

    if let Some(advertiser) = discovery_advertiser {
        advertiser.clear();
    }
    set_running(weak, false);
    set_muted(weak, false);
    let _ = weak.upgrade_in_event_loop(move |app| {
        app.set_pairing_pending(false);
    });
    set_metrics(
        weak,
        "No active stream.\nMetrics update during the next sender or receiver session.".to_string(),
    );

    if exit_when_stopped {
        let _ = slint::quit_event_loop();
    }
}

fn request_stop(weak: &slint::Weak<AppWindow>, state: &Arc<Mutex<SessionState>>, exit_when_stopped: bool) {
    let sender = {
        let mut state = state.lock().expect("session state poisoned");
        state.exit_when_stopped = exit_when_stopped;
        state.current.as_ref().map(|controls| controls.stop_tx.clone())
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

fn toggle_mute(weak: &slint::Weak<AppWindow>, state: &Arc<Mutex<SessionState>>) {
    let sender = {
        let state = state.lock().expect("session state poisoned");
        state.current.as_ref().map(|controls| controls.mute_tx.clone())
    };

    if let Some(sender) = sender {
        let next = !*sender.borrow();
        let _ = sender.send(next);
        set_muted(weak, next);
        set_status(
            weak,
            if next {
                "Session muted.".to_string()
            } else {
                "Session unmuted.".to_string()
            },
        );
    }
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

fn set_muted(weak: &slint::Weak<AppWindow>, muted: bool) {
    let _ = weak.upgrade_in_event_loop(move |app| {
        app.set_muted(muted);
    });
}

fn set_metrics(weak: &slint::Weak<AppWindow>, message: String) {
    let _ = weak.upgrade_in_event_loop(move |app| {
        app.set_metrics_text(message.into());
    });
}

fn append_status(weak: &slint::Weak<AppWindow>, message: String) {
    let _ = weak.upgrade_in_event_loop(move |app| {
        let new_log = push_log_line(&app.get_log_text().to_string(), &message);
        app.set_status_text(message.clone().into());
        app.set_log_text(new_log.into());
    });
}

fn ui_metrics_sink(weak: &slint::Weak<AppWindow>) -> MetricsSink {
    let weak = weak.clone();
    Arc::new(move |message| {
        set_metrics(&weak, message);
    })
}

fn make_pairing_prompt(
    weak: &slint::Weak<AppWindow>,
    pairing_state: &Arc<Mutex<PairingPromptState>>,
) -> PairingPrompt {
    let weak = weak.clone();
    let pairing_state = Arc::clone(pairing_state);
    Arc::new(move |request: PairingRequest| {
        let (decision_tx, decision_rx) = mpsc::channel();
        {
            let mut state = pairing_state.lock().expect("pairing state poisoned");
            state.pending = true;
            state.peer_name = request.peer_name.clone();
            state.fingerprint = request.fingerprint.clone();
            state.role = request.role.clone();
            state.decision_tx = Some(decision_tx);
        }

        let peer_name = request.peer_name;
        let fingerprint = request.fingerprint;
        let role = request.role;
        let _ = weak.upgrade_in_event_loop(move |app| {
            app.set_pairing_peer_name(peer_name.into());
            app.set_pairing_fingerprint(fingerprint.into());
            app.set_pairing_role(role.into());
            app.set_pairing_pending(true);
        });

        decision_rx
            .recv()
            .map_err(|_| anyhow!("pairing confirmation was interrupted"))
    })
}

fn clear_pairing_prompt(
    weak: &slint::Weak<AppWindow>,
    pairing_state: &Arc<Mutex<PairingPromptState>>,
) {
    {
        let mut state = pairing_state.lock().expect("pairing state poisoned");
        state.pending = false;
        state.peer_name.clear();
        state.fingerprint.clear();
        state.role.clear();
        state.decision_tx = None;
    }
    let _ = weak.upgrade_in_event_loop(move |app| {
        app.set_pairing_pending(false);
        app.set_pairing_peer_name("".into());
        app.set_pairing_fingerprint("".into());
        app.set_pairing_role("".into());
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

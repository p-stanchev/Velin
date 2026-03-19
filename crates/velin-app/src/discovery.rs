use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::time;
use velin_proto::{DEFAULT_DISCOVERY_PORT, DiscoveryAnnouncement, DiscoveryPacket};
use crate::transport::{local_ipv4_addresses, local_machine_name};

const PEER_TTL: Duration = Duration::from_secs(4);
const DISCOVERY_TICK: Duration = Duration::from_millis(800);

#[derive(Debug, Clone)]
pub struct DiscoveredPeer {
    pub label: String,
    pub ip: String,
}

#[derive(Debug, Clone, Default)]
pub struct DiscoveryAdvertiser {
    state: Arc<Mutex<Option<DiscoveryAnnouncement>>>,
}

impl DiscoveryAdvertiser {
    pub fn set(&self, announcement: DiscoveryAnnouncement) {
        *self.state.lock().expect("discovery advertiser poisoned") = Some(announcement);
    }

    pub fn clear(&self) {
        *self.state.lock().expect("discovery advertiser poisoned") = None;
    }

    fn current(&self) -> Option<DiscoveryAnnouncement> {
        self.state.lock().expect("discovery advertiser poisoned").clone()
    }
}

#[derive(Debug, Clone)]
struct PeerRecord {
    machine_name: String,
    ip: String,
    last_seen: Instant,
}

pub type PeerUpdateSink = Arc<dyn Fn(Vec<DiscoveredPeer>) + Send + Sync>;

pub async fn run_discovery_service(update: PeerUpdateSink, advertiser: DiscoveryAdvertiser) -> Result<()> {
    let bind_addr = format!("0.0.0.0:{DEFAULT_DISCOVERY_PORT}");
    let socket = UdpSocket::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind discovery listener on {bind_addr}"))?;

    let mut peers = HashMap::<String, PeerRecord>::new();
    let local_name = local_machine_name();
    let local_ips = local_ipv4_addresses();
    let mut packet = vec![0_u8; 2048];
    let mut ticker = time::interval(DISCOVERY_TICK);

    loop {
        tokio::select! {
            result = socket.recv_from(&mut packet) => {
                let (len, from) = result.context("failed to receive discovery packet")?;
                if let Ok(message) = serde_json::from_slice::<DiscoveryPacket>(&packet[..len]) {
                    match message {
                        DiscoveryPacket::Announcement(announcement) => {
                            remember_announcement(&mut peers, announcement, &local_name, &local_ips);
                            emit_peer_snapshot(&peers, &update);
                        }
                        DiscoveryPacket::Request { .. } => {
                            if let Some(announcement) = advertiser.current() {
                                let payload = serde_json::to_vec(&DiscoveryPacket::Announcement(announcement))
                                    .context("failed to encode discovery response")?;
                                let _ = socket.send_to(&payload, from).await;
                            }
                        }
                    }
                }
            }
            _ = ticker.tick() => {
                let now = Instant::now();
                peers.retain(|_, peer| now.duration_since(peer.last_seen) <= PEER_TTL);
                emit_peer_snapshot(&peers, &update);
            }
        }
    }
}

pub async fn request_discovery(requester_name: String) -> Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .context("failed to bind discovery request socket")?;
    socket
        .set_broadcast(true)
        .context("failed to enable broadcast on discovery socket")?;
    let payload = serde_json::to_vec(&DiscoveryPacket::Request { requester_name })
        .context("failed to encode discovery request")?;
    let destination = format!("255.255.255.255:{DEFAULT_DISCOVERY_PORT}");
    socket
        .send_to(&payload, &destination)
        .await
        .context("failed to send discovery request")?;
    Ok(())
}

fn remember_announcement(
    peers: &mut HashMap<String, PeerRecord>,
    announcement: DiscoveryAnnouncement,
    local_name: &str,
    local_ips: &[String],
) {
    if announcement.machine_name == local_name {
        return;
    }

    let now = Instant::now();
    for address in announcement.addresses {
        if local_ips.iter().any(|value| value == &address) {
            continue;
        }

        let key = format!("{}|{}", announcement.machine_name, address);
        peers.insert(
            key,
            PeerRecord {
                machine_name: announcement.machine_name.clone(),
                ip: address,
                last_seen: now,
            },
        );
    }
}

fn emit_peer_snapshot(peers: &HashMap<String, PeerRecord>, update: &PeerUpdateSink) {
    let mut entries: Vec<DiscoveredPeer> = peers
        .values()
        .map(|peer| DiscoveredPeer {
            label: format!("{} ({})", peer.machine_name, peer.ip),
            ip: peer.ip.clone(),
        })
        .collect();

    entries.sort_by(|left, right| left.label.cmp(&right.label));
    update(entries);
}

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::time;
use velin_proto::{DEFAULT_DISCOVERY_PORT, DiscoveryAnnouncement};

const PEER_TTL: Duration = Duration::from_secs(4);
const DISCOVERY_TICK: Duration = Duration::from_millis(800);

#[derive(Debug, Clone)]
pub struct DiscoveredPeer {
    pub label: String,
    pub ip: String,
}

#[derive(Debug, Clone)]
struct PeerRecord {
    machine_name: String,
    ip: String,
    last_seen: Instant,
}

pub type PeerUpdateSink = Arc<dyn Fn(Vec<DiscoveredPeer>) + Send + Sync>;

pub async fn run_discovery_listener(update: PeerUpdateSink) -> Result<()> {
    let bind_addr = format!("0.0.0.0:{DEFAULT_DISCOVERY_PORT}");
    let socket = UdpSocket::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind discovery listener on {bind_addr}"))?;

    let mut peers = HashMap::<String, PeerRecord>::new();
    let mut packet = vec![0_u8; 2048];
    let mut ticker = time::interval(DISCOVERY_TICK);

    loop {
        tokio::select! {
            result = socket.recv_from(&mut packet) => {
                let (len, _from) = result.context("failed to receive discovery announcement")?;
                if let Ok(announcement) = serde_json::from_slice::<DiscoveryAnnouncement>(&packet[..len]) {
                    let now = Instant::now();
                    for address in announcement.addresses {
                        let key = format!("{}|{}", announcement.machine_name, address);
                        peers.insert(key, PeerRecord {
                            machine_name: announcement.machine_name.clone(),
                            ip: address,
                            last_seen: now,
                        });
                    }

                    emit_peer_snapshot(&peers, &update);
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

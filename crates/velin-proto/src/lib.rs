use serde::{Deserialize, Serialize};

pub const DEFAULT_CONTROL_PORT: u16 = 49000;
pub const DEFAULT_AUDIO_PORT: u16 = 49001;
pub const DEFAULT_DISCOVERY_PORT: u16 = 49002;
pub const FRAME_SAMPLES: usize = 480;
pub const SAMPLE_RATE_HZ: u32 = 48_000;
pub const CHANNELS: u16 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub source_name: String,
    pub stream_id: u64,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub identity_public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Accept {
    pub target_name: String,
    pub audio_port: u16,
    pub identity_public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingRequired {
    pub target_name: String,
    pub identity_public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingDecision {
    pub approved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryAnnouncement {
    pub machine_name: String,
    pub control_port: u16,
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiscoveryPacket {
    Announcement(DiscoveryAnnouncement),
    Request { requester_name: String },
}

#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub sequence: u64,
    pub samples: Vec<i16>,
}

impl AudioFrame {
    pub fn encode(&self) -> Vec<u8> {
        let sample_count = self.samples.len() as u16;
        let mut bytes = Vec::with_capacity(10 + self.samples.len() * 2);
        bytes.extend_from_slice(&self.sequence.to_le_bytes());
        bytes.extend_from_slice(&sample_count.to_le_bytes());
        for sample in &self.samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 10 {
            return None;
        }

        let sequence = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
        let sample_count = u16::from_le_bytes(bytes[8..10].try_into().ok()?) as usize;
        let payload = &bytes[10..];

        if payload.len() != sample_count * 2 {
            return None;
        }

        let mut samples = Vec::with_capacity(sample_count);
        for chunk in payload.chunks_exact(2) {
            samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
        }

        Some(Self { sequence, samples })
    }
}

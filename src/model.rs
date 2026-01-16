use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const KNOWN_PEERS_FILE: &str = "known_peers.json";
pub const HISTORY_FILE: &str = "history.jsonl";
pub const UDP_DISCOVERY_PORT: u16 = 44517;
pub const UDP_MESSAGE_PORT: u16 = 44518;
pub const TCP_FILE_PORT: u16 = 44517;
pub const TCP_DIR_PORT: u16 = 44518;

#[derive(Clone, Debug)]
pub struct User {
    pub id: String,
    pub name: String,
    pub online: bool,
    pub ip: Option<String>,
    pub port: Option<u16>,
    pub tcp_port: Option<u16>,
    pub bound_interface: Option<String>,
    pub best_interface: Option<String>,
    pub has_unread: bool,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub from_me: bool,
    pub text: String,
    pub send_ts: String,
    pub recv_ts: Option<String>,
    pub file_path: Option<String>,
    pub transfer_status: Option<String>,
    pub msg_id: Option<String>,
    pub is_read: bool,
}

#[derive(Deserialize)]
pub struct HistoryEntry {
    pub peer_id: String,
    pub from_me: bool,
    pub text: String,
    pub send_ts: String,
    pub recv_ts: Option<String>,
    pub ts: Option<String>,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Peer {
    pub id: String,
    pub ip: String,
    pub port: u16,
    pub tcp_port: Option<u16>,
    pub name: Option<String>,
    pub last_seen: DateTime<Local>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownPeer {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing, default)]
    pub port: Option<u16>,
    #[serde(skip_serializing, default)]
    #[allow(dead_code)]
    pub tcp_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound_interface: Option<String>,
}

#[derive(Clone, Debug)]
pub struct QueuedMsg {
    pub text: String,
    pub send_ts: String,
    pub msg_id: Option<String>,
    pub file_path: Option<PathBuf>,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredPeer {
    pub id: String,
    pub ip: String,
    pub port: u16,
    pub tcp_port: Option<u16>,
    pub name: Option<String>,
}

#[derive(Debug)]
pub enum NetCmd {
    ChangeName(String),
    SendChat {
        ip: String,
        text: String,
        ts: String,
        via: Option<String>,
        msg_id: String,
    },
    ProbePeer {
        ip: String,
        via: Option<String>,
    },
    SendFile {
        peer_id: String,
        ip: String,
        tcp_port: u16,
        path: PathBuf,
        is_dir: bool,
        via: Option<String>,
    },
}

#[derive(Debug)]
pub enum PeerEvent {
    Discovered(DiscoveredPeer, String),
    LocalBound { ip: String, port: u16 },
    ChatReceived {
        from_id: String,
        from_ip: String,
        from_port: u16,
        text: String,
        send_ts: String,
        recv_ts: String,
        msg_id: String,
        local_ip: String,
    },
    ChatAck {
        from_id: String,
        msg_id: String,
    },
    FileProgress {
        peer_id: Option<String>,
        file_name: String,
        progress: f32,
        status: String,
        is_incoming: bool,
        is_dir: bool,
        local_path: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
pub struct HelloMsg {
    pub msg_type: String,
    pub id: String,
    pub name: Option<String>,
    pub port: u16,
    pub tcp_port: Option<u16>,
    pub version: String,
    #[serde(default)]
    pub is_reply: bool,
    #[serde(default)]
    pub is_probe: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatPayload {
    pub msg_type: String,
    pub msg_id: String,
    pub from_id: String,
    pub from_name: Option<String>,
    pub text: String,
    pub timestamp: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AckPayload {
    pub msg_type: String,
    pub msg_id: String,
    pub from_id: String,
}

#[derive(Debug)]
pub enum FileCmd {
    SendFile {
        peer_id: String,
        peer_ip: String,
        tcp_port: u16,
        path: PathBuf,
        is_dir: bool,
        via: Option<String>,
    },
}

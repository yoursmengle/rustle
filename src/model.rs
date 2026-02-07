use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const KNOWN_PEERS_FILE: &str = "known_peers.json";
pub const SYNC_TREE_FILE: &str = "sync_tree.json";
pub const RECEIVE_MAP_FILE: &str = "receive_map.json";
pub const UDP_DISCOVERY_PORT: u16 = 44517;
pub const UDP_MESSAGE_PORT: u16 = 44518;
pub const TCP_FILE_PORT: u16 = 44517;
pub const TCP_DIR_PORT: u16 = 44518;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncStatus {
    ReadyToSend,
    Sending,
    Sent,
    FileChanged,
    Syncing,
    Synced,
}

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
    pub last_sync_ts: Option<String>,
    pub file_path: Option<String>,
    pub transfer_status: Option<String>,
    pub msg_id: Option<String>,
    pub is_read: bool,
    pub is_pending: bool, // 是否等待确认
    #[allow(dead_code)]
    pub needs_sync: bool, // 是否需要同步
}

#[derive(Deserialize)]
pub struct HistoryEntry {
    pub peer_id: String,
    pub from_me: bool,
    pub text: String,
    pub send_ts: String,
    pub recv_ts: Option<String>,
    pub sync_ts: Option<String>,
    pub ts: Option<String>,
    pub file_path: Option<String>,
    pub is_pending: Option<bool>, // 是否等待确认
    pub needs_sync: Option<bool>, // 是否需要同步
    pub msg_id: Option<String>,   // 消息ID，用于追踪确认
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncTree {
    #[serde(default)]
    pub peers: std::collections::HashMap<String, Vec<SyncNode>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    #[serde(default)]
    pub mtime: Option<i64>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub children: Vec<SyncNode>,
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
    UpdatePeerList {
        peers: Vec<PeerSnapshot>,
        online_count: usize,
    },
    SendNameUpdate {
        ip: String,
        via: Option<String>,
        name: String,
        local_ip: Option<String>,
    },
    SendChat {
        ip: String,
        text: String,
        ts: String,
        via: Option<String>,
        msg_id: String,
        local_ip: Option<String>,
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
        is_sync: bool,
    },
}

#[derive(Debug)]
pub enum PeerEvent {
    Discovered(DiscoveredPeer, String),
    LocalBound {
        ip: String,
        port: u16,
    },
    ChatReceived {
        from_id: String,
        from_ip: String,
        from_port: u16,
        from_name: Option<String>,
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
        is_sync: bool,
    },
    DiscoverReceived {
        from_id: String,
        from_ip: String,
        from_name: Option<String>,
        peers: Vec<PeerBrief>,
    },
    NameUpdate {
        id: String,
        name: String,
        ip: Option<String>,
    },
    PeerOnline {
        id: String,
        ip: String,
    },
    PeerOffline {
        id: String,
    },
}

#[derive(Serialize, Deserialize)]
pub struct HelloMsg {
    pub msg_type: String,
    pub id: String,
    pub name: Option<String>,
    #[serde(default)]
    pub list_hash: u64,
    pub port: u16,
    pub tcp_port: Option<u16>,
    pub version: String,
    #[serde(default)]
    pub is_reply: bool,
    #[serde(default)]
    pub is_probe: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct HeartbeatPayload {
    pub msg_type: String,
    pub id: String,
    pub list_hash: u64,
    pub offline_hash: u64,
    pub online_count: u32,
    pub offline_count: u32,
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ByePayload {
    pub msg_type: String,
    pub id: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SyncPayload {
    pub msg_type: String,
    pub from_id: String,
    pub peers: Vec<PeerBrief>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatPayload {
    pub msg_type: String,
    pub msg_id: String,
    pub from_id: String,
    pub from_name: Option<String>,
    pub from_ip: Option<String>,
    pub text: String,
    pub timestamp: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AckPayload {
    pub msg_type: String,
    pub msg_id: String,
    pub from_id: String,
    pub from_name: Option<String>,
    pub from_ip: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NameUpdatePayload {
    pub msg_type: String,
    pub from_id: String,
    pub from_name: String,
    pub from_ip: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PeerBrief {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DiscoverPayload {
    pub msg_type: String,
    pub from_id: String,
    pub from_name: Option<String>,
    #[serde(default)]
    pub is_reply: bool,
    pub peers: Vec<PeerBrief>,
}

#[derive(Debug, Clone)]
pub struct PeerSnapshot {
    pub id: String,
    pub ip: Option<String>,
    pub online: bool,
    pub name: Option<String>,
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
        is_sync: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;
    use std::path::PathBuf;

    #[test]
    fn hello_msg_serde_roundtrip() {
        let h = HelloMsg {
            msg_type: "hello".to_string(),
            id: "node1".to_string(),
            name: Some("Alice".to_string()),
            list_hash: 1234,
            port: 44518,
            tcp_port: Some(44517),
            version: "0.1".to_string(),
            is_reply: false,
            is_probe: true,
        };
        let s = serde_json::to_string(&h).unwrap();
        let h2: HelloMsg = serde_json::from_str(&s).unwrap();
        assert_eq!(h.id, h2.id);
        assert_eq!(h.name, h2.name);
        assert_eq!(h.list_hash, h2.list_hash);
    }

    #[test]
    fn chat_ack_discover_serde() {
        let chat = ChatPayload {
            msg_type: "chat".to_string(),
            msg_id: "m1".to_string(),
            from_id: "node1".to_string(),
            from_name: Some("Alice".to_string()),
            from_ip: Some("192.168.1.2".to_string()),
            text: "hello".to_string(),
            timestamp: "ts".to_string(),
        };
        let s = serde_json::to_string(&chat).unwrap();
        let chat2: ChatPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(chat.msg_id, chat2.msg_id);
        assert_eq!(chat.text, chat2.text);

        let ack = AckPayload {
            msg_type: "ack".to_string(),
            msg_id: "m1".to_string(),
            from_id: "node2".to_string(),
            from_name: None,
            from_ip: None,
        };
        let s = serde_json::to_string(&ack).unwrap();
        let ack2: AckPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(ack.msg_id, ack2.msg_id);

        let pb = PeerBrief {
            id: "p1".to_string(),
            ip: Some("1.2.3.4".to_string()),
            name: Some("Bob".to_string()),
        };
        let dis = DiscoverPayload {
            msg_type: "discover".to_string(),
            from_id: "x".to_string(),
            from_name: Some("X".to_string()),
            is_reply: false,
            peers: vec![pb.clone()],
        };
        let s = serde_json::to_string(&dis).unwrap();
        let dis2: DiscoverPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(dis.from_id, dis2.from_id);
        assert_eq!(dis.peers.len(), dis2.peers.len());
        assert_eq!(dis.peers[0].id, dis2.peers[0].id);
    }

    #[test]
    fn sync_tree_serde() {
        let child = SyncNode {
            name: "child".to_string(),
            path: "/tmp/c".to_string(),
            is_dir: false,
            mtime: Some(1),
            sha256: Some("abc".to_string()),
            children: vec![],
        };
        let node = SyncNode {
            name: "root".to_string(),
            path: "/tmp".to_string(),
            is_dir: true,
            mtime: None,
            sha256: None,
            children: vec![child],
        };
        let mut tree = SyncTree::default();
        tree.peers.insert("p1".to_string(), vec![node]);
        let s = serde_json::to_string(&tree).unwrap();
        let t2: SyncTree = serde_json::from_str(&s).unwrap();
        assert!(t2.peers.contains_key("p1"));
        assert_eq!(t2.peers.get("p1").unwrap().len(), 1);
        assert_eq!(t2.peers.get("p1").unwrap()[0].children.len(), 1);
    }
}

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use rfd::FileDialog;
use std::path::PathBuf;
use std::env;
use std::collections::{HashMap, HashSet};
use std::fs;
use chrono::{DateTime, Local, Duration as ChronoDuration};
use std::sync::mpsc::{self, Sender, Receiver};
use std::thread;
use std::time::{Duration, Instant};
use serde::{Serialize, Deserialize};
use serde_json;
use uuid::Uuid;
use std::net::{SocketAddr, UdpSocket, Ipv4Addr, IpAddr};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use get_if_addrs::get_if_addrs;
use sysinfo::{System, SystemExt, NetworkExt};

const KNOWN_PEERS_FILE: &str = "known_peers.json";
const HISTORY_FILE: &str = "history.jsonl";
const UDP_PORT: u16 = 59000;

fn data_dir() -> PathBuf {
    let mut dir = env::var_os("LOCALAPPDATA")
        .or_else(|| env::var_os("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir());
    dir.push("Rustle");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn data_path(name: &str) -> PathBuf {
    let mut p = data_dir();
    p.push(name);
    p
}

fn main() -> eframe::Result<()> {
    // 加载图标
    let icon_data = match image::load_from_memory(include_bytes!("../rustle.ico")) {
        Ok(image) => {
            let image = image.to_rgba8();
            let (width, height) = image.dimensions();
            Some(egui::IconData {
                rgba: image.into_raw(),
                width,
                height,
            })
        }
        Err(_) => None,
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Rustle")
        .with_inner_size([1200.0, 800.0]);
    
    if let Some(icon) = icon_data {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Rustle",
        options,
        Box::new(|cc| {
            // 加载中文字体
            let mut fonts = egui::FontDefinitions::default();
            
            // 添加中文字体（使用 Windows 系统自带的微软雅黑）
            fonts.font_data.insert(
                "msyh".to_owned(),
                egui::FontData::from_static(include_bytes!("C:\\Windows\\Fonts\\msyh.ttc")),
            );
            
            // 将中文字体设为最高优先级
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "msyh".to_owned());
            
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "msyh".to_owned());
            
            cc.egui_ctx.set_fonts(fonts);
            
            // 在启动时检查用户数据目录下的 me.txt
            let mut app = RustleApp::default();
            match fs::read_to_string(data_path("me.txt")) {
                Ok(s) => {
                    let s = s.trim().to_string();
                    if !s.is_empty() {
                        app.me_name = Some(s);
                        app.show_name_dialog = false;
                    } else {
                        app.show_name_dialog = true;
                    }
                }
                Err(_) => {
                    app.show_name_dialog = true;
                }
            }

            // 启动时加载已知节点（离线列表）
            app.load_known_peers();
            // 启动时加载最近 15 天的历史记录
            app.load_recent_history();

            // 尝试设置本机显示 IP：优先选择收发总流量最大的接口的 IPv4 地址
            if app.local_ip.is_none() {
                let mut chosen: Option<String> = None;

                if let Ok(ifaces) = get_if_addrs() {
                    // 建立 iface name -> ipv4 list 映射
                    let mut name_to_ips: HashMap<String, Vec<std::net::Ipv4Addr>> = HashMap::new();
                    for iface in &ifaces {
                        if let IpAddr::V4(ipv4) = iface.ip() {
                            name_to_ips.entry(iface.name.clone()).or_default().push(ipv4);
                        }
                    }

                    // 使用 sysinfo 获取各接口的总流量，并选择最大者
                    let mut sys = System::new_all();
                    sys.refresh_networks();
                    let mut best_name_opt: Option<String> = None;
                    let mut best_bytes: u64 = 0;
                    for (name, net) in sys.networks() {
                        let total = net.received() + net.transmitted();
                        if total > best_bytes {
                            best_bytes = total;
                            best_name_opt = Some(name.clone());
                        }
                    }
                    if let Some(best_name) = best_name_opt {
                        if let Some(ips) = name_to_ips.get(&best_name) {
                            for ip in ips {
                                if !ip.is_loopback() {
                                    chosen = Some(ip.to_string());
                                    break;
                                }
                            }
                        }
                    }

                    // 如果没有选出（例如接口名不匹配），fallback 到第一个非回环 IPv4
                    if chosen.is_none() {
                        for iface in ifaces {
                            if let IpAddr::V4(ipv4) = iface.ip() {
                                if !ipv4.is_loopback() {
                                    chosen = Some(ipv4.to_string());
                                    break;
                                }
                            }
                        }
                    }
                }

                // 最后兜底：127.0.0.1
                app.local_ip = chosen.or_else(|| Some("127.0.0.1".to_string()));
            }

            // 固定使用 UDP_PORT
            if app.local_port.is_none() {
                app.local_port = Some(UDP_PORT);
            }

            // 启动网络发现后台线程（使用 channel 向 UI 发送发现事件）
            let (peer_tx, peer_rx) = mpsc::channel();
            let (cmd_tx, cmd_rx) = mpsc::channel();
            let initial_name = app.me_name.clone();
            spawn_network_worker(peer_tx, cmd_rx, initial_name);
            app.peer_rx = Some(peer_rx);
            app.net_cmd_tx = Some(cmd_tx);

            Ok(Box::new(app))
        }),
    )
}

#[derive(Clone, Debug)]
struct User {
    id: String,
    name: String,
    online: bool,
    ip: Option<String>,
    port: Option<u16>,
    tcp_port: Option<u16>,
    bound_interface: Option<String>,
    best_interface: Option<String>,  // 能收到 ACK 的最优接口
    has_unread: bool,
}

#[derive(Clone, Debug)]
struct ChatMessage {
    from_me: bool,
    text: String,
    send_ts: String,
    recv_ts: Option<String>,
    file_path: Option<String>,
    transfer_status: Option<String>,
    msg_id: Option<String>,
    is_read: bool,
} 

#[derive(Deserialize)]
struct HistoryEntry {
    peer_id: String,
    from_me: bool,
    text: String,
    send_ts: String,
    recv_ts: Option<String>,
    ts: Option<String>,
    file_path: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Peer {
    id: String,
    ip: String,
    port: u16,
    tcp_port: Option<u16>,
    name: Option<String>,
    last_seen: DateTime<Local>,
} 

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KnownPeer {
    id: String,
    name: Option<String>,
    ip: Option<String>,
    port: Option<u16>,
    tcp_port: Option<u16>,
    last_seen: Option<String>,
    bound_interface: Option<String>,
}

fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.2} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

fn format_speed(bytes: u64, elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return "0.00 MB/s".to_string();
    }
    let mb_per_sec = bytes as f64 / secs / (1024.0 * 1024.0);
    format!("{:.2} MB/s", mb_per_sec)
}

#[derive(Clone, Debug)]
struct QueuedMsg {
    text: String,
    send_ts: String,
    msg_id: Option<String>,
    file_path: Option<PathBuf>,
    is_dir: bool,
}

#[derive(Default)]
struct RustleApp {
    users: Vec<User>,
    selected_user_id: Option<String>,
    messages: HashMap<String, Vec<ChatMessage>>,
    input: String,
    pending_acks: HashMap<String, Vec<(String, Instant)>>,

    // 当前用户名称（从 me.txt 读取或用户输入）
    me_name: Option<String>,
    // 启动时是否显示输入姓名对话框
    show_name_dialog: bool,
    // 临时输入缓冲
    temp_name_input: String,
    // 保存错误信息（显示在对话框中）
    name_save_error: Option<String>,

    // 网络发现相关
    peers: HashMap<String, Peer>, // key = "ip:port"
    peer_rx: Option<Receiver<PeerEvent>>,
    net_cmd_tx: Option<Sender<NetCmd>>,

    // 已知节点缓存
    probed_known: bool,
    known_dirty: bool,

    // 离线消息队列
    offline_msgs: HashMap<String, Vec<QueuedMsg>>,

    // 本机绑定信息（UI 使用）
    local_ip: Option<String>,
    local_port: Option<u16>,

    // 已接收消息 ID 缓存（用于去重）
    received_msg_ids: HashSet<String>,

    // 已记录到历史的入站文件（peer_id, file_name）避免重复
    logged_incoming_files: HashSet<(String, String)>,

    // 自动滚动标记
    scroll_to_bottom: bool,
    
    // 消息已读追踪：(peer_id, msg_index) -> 可见时刻
    message_visible_since: HashMap<(String, usize), Instant>,
    
    // 是否有未读消息（用于触发任务栏闪烁）
    has_unread_messages: bool,
    
    // 滚动到第一条未读消息
    scroll_to_first_unread: bool,

    // 本帧是否需要触发闪烁
    #[allow(dead_code)]
    should_flash: bool,

    // 右键菜单状态
    context_menu_user_id: Option<String>,
    show_ip_dialog: Option<(String, String)>, // (name, ip_info)
}

impl RustleApp {
    fn is_private_ipv4(ip: &str) -> bool {
        let parts: Vec<_> = ip.split('.').collect();
        if parts.len() != 4 { return false; }
        let p: Vec<u8> = parts.iter().filter_map(|s| s.parse::<u8>().ok()).collect();
        if p.len() != 4 { return false; }
        (p[0] == 10)
            || (p[0] == 172 && (16..=31).contains(&p[1]))
            || (p[0] == 192 && p[1] == 168)
    }

    fn same_lan(local: &str, candidate: &str) -> bool {
        let lp: Vec<_> = local.split('.').collect();
        let cp: Vec<_> = candidate.split('.').collect();
        if lp.len() != 4 || cp.len() != 4 { return false; }
        // 粗略同网段判断：前三段相同
        lp[0] == cp[0] && lp[1] == cp[1] && lp[2] == cp[2]
    }

    fn find_interface_for_target(target_ip_str: &str) -> Option<String> {
        let target_ip: Ipv4Addr = target_ip_str.parse().ok()?;
        if let Ok(ifaces) = get_if_addrs() {
            for iface in ifaces {
                if let get_if_addrs::IfAddr::V4(v4) = iface.addr {
                    if v4.ip.is_loopback() {
                        continue;
                    }
                    let local_u32 = u32::from(v4.ip);
                    let mask_u32 = u32::from(v4.netmask);
                    let target_u32 = u32::from(target_ip);
                    if (local_u32 & mask_u32) == (target_u32 & mask_u32) {
                        return Some(v4.ip.to_string());
                    }
                }
            }
        }
        None
    }

    fn maybe_switch_primary_interface(&mut self, local_ip: &str, peer_ip: &str) {
        // 若当前主接口不在同网段，而本次通讯接口在同网段，则切换主接口
        match &self.local_ip {
            None => self.local_ip = Some(local_ip.to_string()),
            Some(cur) => {
                if !Self::same_lan(cur, peer_ip) && Self::same_lan(local_ip, peer_ip) {
                    self.local_ip = Some(local_ip.to_string());
                }
            }
        }
    }

    #[allow(dead_code)]
    fn prefer_ip(&self, current: Option<String>, candidate: &str) -> String {
        let cand_priv = Self::is_private_ipv4(candidate);
        let local_pref = self.local_ip.clone().unwrap_or_default();
        let cand_same_lan = if local_pref.is_empty() { false } else { Self::same_lan(&local_pref, candidate) };

        if let Some(cur) = current {
            let cur_priv = Self::is_private_ipv4(&cur);
            let cur_same_lan = if local_pref.is_empty() { false } else { Self::same_lan(&local_pref, &cur) };

            // Prefer candidate if it is private and current is not
            if cand_priv && !cur_priv { return candidate.to_string(); }
            // Prefer candidate if both private but candidate matches local lan and current not
            if cand_priv && cur_priv && cand_same_lan && !cur_same_lan { return candidate.to_string(); }
            // Otherwise keep current
            return cur;
        }
        candidate.to_string()
    }

    fn mark_offline(&mut self, id: &str) {
        if let Some(u) = self.users.iter_mut().find(|u| u.id == id) {
            u.online = false;
            self.known_dirty = true;
        }
    }

    fn update_outgoing_msg_id(&mut self, peer_id: &str, new_id: &str, text: &str) {
        if let Some(msgs) = self.messages.get_mut(peer_id) {
            if let Some(m) = msgs.iter_mut().rev().find(|m| m.from_me && m.text == text) {
                m.msg_id = Some(new_id.to_string());
                if m.transfer_status.is_none() || m.transfer_status.as_deref() == Some("未送达") {
                    m.transfer_status = Some("发送中...".to_string());
                }
            }
        }
    }

    fn log_history(&self, peer_id: &str, from_me: bool, text: &str, send_ts: &str, recv_ts: Option<&str>, file_path: Option<&str>) {
        let path = data_path(HISTORY_FILE);
        let line = serde_json::json!({
            "peer_id": peer_id,
            "from_me": from_me,
            "text": text,
            "send_ts": send_ts,
            "recv_ts": recv_ts,
            "file_path": file_path,
            "ts": Local::now().to_rfc3339(),
        })
        .to_string();

        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| {
                f.write_all(line.as_bytes())?;
                f.write_all(b"\n")
            });
    }

    fn clear_history(&self, peer_id: &str) {
        let path = data_path(HISTORY_FILE);
        let Ok(content) = fs::read_to_string(&path) else { return; };
        let mut new_lines = Vec::new();
        for line in content.lines() {
            if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
                if entry.get("peer_id").and_then(|v| v.as_str()) != Some(peer_id) {
                    new_lines.push(line);
                }
            }
        }
        let _ = fs::write(&path, new_lines.join("\n") + "\n");
    }

    fn load_recent_history(&mut self) {
        let path = data_path(HISTORY_FILE);
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => return,
        };

        let cutoff = Local::now() - ChronoDuration::days(15);
        let reader = BufReader::new(file);
        for line in reader.lines().flatten() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: HistoryEntry = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if let Some(ts) = entry.ts.as_deref() {
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
                    let dt_local = dt.with_timezone(&Local);
                    if dt_local < cutoff {
                        continue;
                    }
                }
            }

            let pid = entry.peer_id.clone();
            if !self.users.iter().any(|u| u.id == pid) {
                self.users.push(User {
                    id: pid.clone(),
                    name: pid.clone(),
                    online: false,
                    ip: None,
                    port: None,
                    tcp_port: None,
                    bound_interface: None,
                    best_interface: None,
                    has_unread: false,
                });
                self.offline_msgs.entry(pid.clone()).or_default();
            }

            let msgs = self.messages.entry(pid.clone()).or_default();
            msgs.push(ChatMessage {
                from_me: entry.from_me,
                text: entry.text,
                send_ts: entry.send_ts,
                recv_ts: entry.recv_ts,
                file_path: entry.file_path,
                transfer_status: None,
                msg_id: None,
                is_read: true,
            });
        }
    }

    fn flush_offline_queue(&mut self, peer_id: &str, ip: Option<&str>, port: Option<u16>) {
        let Some(ip) = ip else { return; };
        let Some(port) = port else { return; };
        let Some(tx) = self.net_cmd_tx.clone() else { return; };
        if let Some(queue) = self.offline_msgs.get_mut(peer_id) {
            if queue.is_empty() {
                return;
            }
            let drained: Vec<QueuedMsg> = queue.drain(..).collect();
            let mut remain = Vec::new();
            // 查找用户最优接口（能接收 ACK 的接口）
            let mut via = self.users.iter().find(|u| &u.id == peer_id)
                .and_then(|u| u.best_interface.clone().or_else(|| u.bound_interface.clone()));

            if let Some(best_via) = Self::find_interface_for_target(ip) {
                via = Some(best_via);
            }

            let tcp_port = self.users.iter().find(|u| &u.id == peer_id).and_then(|u| u.tcp_port);

            for mut msg in drained {
                if let Some(path) = &msg.file_path {
                    if let Some(tcp_port) = tcp_port {
                        if tx.send(NetCmd::SendFile {
                            peer_id: peer_id.to_string(),
                            ip: ip.to_string(),
                            tcp_port,
                            path: path.clone(),
                            is_dir: msg.is_dir,
                            via: via.clone(),
                        }).is_err() {
                            remain.push(msg);
                        }
                    } else {
                        remain.push(msg);
                    }
                } else {
                    let mid = msg.msg_id.clone().unwrap_or_else(|| Uuid::new_v4().to_string());
                    self.update_outgoing_msg_id(peer_id, &mid, &msg.text);
                    if tx.send(NetCmd::SendChat { ip: ip.to_string(), port, text: msg.text.clone(), ts: msg.send_ts.clone(), via: via.clone(), msg_id: mid.clone() }).is_err() {
                        msg.msg_id = Some(mid);
                        remain.push(msg);
                    } else {
                        let deadline = Instant::now() + Duration::from_secs(3);
                        self.pending_acks.entry(peer_id.to_string()).or_default().push((mid, deadline));
                    }
                }
            }
            if !remain.is_empty() {
                self.offline_msgs.insert(peer_id.to_string(), remain);
            }
        }
    }

    fn ensure_seed_data(&mut self) {
        // 不再自动填充演示用户；联系人应由实际发现或从存储加载。
        // 该函数保留以便将来添加自动加载逻辑。

        // 如果上次运行保存了 last_port.txt，我们在这里把端口记入但实际绑定在后台线程
        //（保证 UI 启动速度）
    }

        fn load_known_peers(&mut self) {
            if let Ok(data) = fs::read_to_string(data_path(KNOWN_PEERS_FILE)) {
                if let Ok(list) = serde_json::from_str::<Vec<KnownPeer>>(&data) {
                    for kp in list {
                        if let Some(ip) = kp.ip.clone() {
                            if !Self::is_private_ipv4(&ip) {
                                continue;
                            }
                        }
                        let display = kp
                            .name
                            .clone()
                            .or_else(|| kp.ip.as_ref().and_then(|ip| kp.port.map(|p| format!("{}:{}", ip, p))))
                            .unwrap_or_else(|| kp.id.clone());

                        if !self.users.iter().any(|u| u.id == kp.id) {
                            self.users.push(User {
                                id: kp.id.clone(),
                                name: display,
                                online: false,
                                ip: kp.ip.clone(),
                                port: kp.port,
                                tcp_port: kp.tcp_port,
                                bound_interface: kp.bound_interface.clone(),
                                best_interface: kp.bound_interface.clone(),
                                has_unread: false,
                            });
                            self.messages.entry(kp.id.clone()).or_default();
                            self.offline_msgs.entry(kp.id.clone()).or_default();
                            self.known_dirty = true;
                        }
                    }
                }
            }
        }

    fn persist_known_peers(&mut self) {
        if !self.known_dirty {
            return;
        }
        let list: Vec<KnownPeer> = self
            .users
            .iter()
            .map(|u| KnownPeer {
                id: u.id.clone(),
                name: Some(u.name.clone()),
                ip: u.ip.clone(),
                port: u.port,
                tcp_port: u.tcp_port,
                last_seen: None,
                bound_interface: u.best_interface.clone().or_else(|| u.bound_interface.clone()),
            })
            .collect();
        if let Ok(text) = serde_json::to_string_pretty(&list) {
            let _ = fs::write(data_path(KNOWN_PEERS_FILE), text);
        }
        self.known_dirty = false;
    }    fn selected_user_name(&self) -> String {
        let Some(id) = &self.selected_user_id else {
            return "选择联系人".to_string();
        };
        self.users
            .iter()
            .find(|u| &u.id == id)
            .map(|u| u.name.clone())
            .unwrap_or_else(|| "选择联系人".to_string())
    }

    fn send_message_internal(&mut self, id: &str, text: String) {
        let ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let msg_id = Uuid::new_v4().to_string();

        // 本地追加
        self.messages.entry(id.to_string()).or_default().push(ChatMessage {
                    from_me: true,
                    text: text.clone(),
                    send_ts: ts.clone(),
                    recv_ts: None,
                    file_path: None,
                    transfer_status: None,
                    msg_id: Some(msg_id.clone()),
                    is_read: true,
        });

        self.log_history(id, true, &text, &ts, None, None);

        let Some(u) = self.users.iter().find(|u| u.id == id) else { return; };
        let online = u.online;
        let ip = u.ip.clone();
        let port = u.port;
        let mut via = u.best_interface.clone().or_else(|| u.bound_interface.clone()); // 优先使用 best_interface

        // 尝试寻找更匹配的本地接口
        if let Some(target_ip) = &ip {
            if let Some(best_via) = Self::find_interface_for_target(target_ip) {
                via = Some(best_via);
            }
        }

        let has_addr = ip.is_some() && port.is_some();

        if online {
            if let (Some(ip), Some(port)) = (ip.as_deref(), port) {
                if let Some(tx) = &self.net_cmd_tx {
                    if tx.send(NetCmd::SendChat { ip: ip.to_string(), port, text: text.to_string(), ts: ts.clone(), via: via.clone(), msg_id: msg_id.clone() }).is_err() {
                        self.mark_offline(id);
                    }
                }
            }
        } else {
            // 离线：排队，并且如果已有地址尝试一次乐观发送
                self.offline_msgs.entry(id.to_string()).or_default().push(QueuedMsg { 
                    text: text.to_string(), 
                    send_ts: ts.clone(), 
                    msg_id: Some(msg_id.clone()),
                    file_path: None,
                    is_dir: false,
                });
            if has_addr {
                if let (Some(ip), Some(port)) = (ip.as_deref(), port) {
                    if let Some(local) = self.local_ip.as_deref() {
                        if !Self::same_lan(local, ip) { return; }
                    }
                    if let Some(tx) = &self.net_cmd_tx {
                        if tx.send(NetCmd::SendChat { ip: ip.to_string(), port, text: text.to_string(), ts: ts.clone(), via: via.clone(), msg_id: msg_id.clone() }).is_err() {
                            self.mark_offline(id);
                        }
                    }
                }
            }
        }

        // 添加待确认列表，5 秒超时
        let deadline = Instant::now() + Duration::from_secs(3);
        self.pending_acks.entry(id.to_string()).or_default().push((msg_id, deadline));
    }

    fn send_current(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(id) = self.selected_user_id.clone() {
            self.send_message_internal(&id, text);
            self.scroll_to_bottom = true;
        }
        self.input.clear();
    }

    fn append_file_message(&mut self, path: &PathBuf, is_dir: bool) {
        if let Some(id) = self.selected_user_id.clone() {
            self.scroll_to_bottom = true;
            let (ip, tcp_port, mut via) = {
                let Some(user) = self.users.iter().find(|u| u.id == id) else { return; };
                (user.ip.clone(), user.tcp_port, user.bound_interface.clone())
            };

            // 强制寻找与目标 IP 同网段的本地接口
            if let Some(target_ip) = &ip {
                if let Some(best_via) = Self::find_interface_for_target(target_ip) {
                    via = Some(best_via);
                }
            }

            let icon = if is_dir { "📁" } else { "📄" };
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("item");
            let text = format!("{} {}", icon, name);
            let ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

            let mut sent = false;
            if let (Some(ip), Some(tcp_port)) = (ip, tcp_port) {
                if let Some(tx) = &self.net_cmd_tx {
                    if tx.send(NetCmd::SendFile {
                        peer_id: id.clone(),
                        ip: ip.clone(),
                        tcp_port,
                        path: path.clone(),
                        is_dir,
                        via: via.clone(),
                    }).is_ok() {
                        sent = true;
                    } else {
                        self.mark_offline(&id);
                    }
                }
            }

            if !sent {
                self.offline_msgs.entry(id.clone()).or_default().push(QueuedMsg { 
                    text: text.clone(), 
                    send_ts: ts.clone(), 
                    msg_id: None,
                    file_path: Some(path.clone()),
                    is_dir,
                });
            }

            let msgs = self.messages.entry(id.clone()).or_default();
            msgs.push(ChatMessage {
                from_me: true,
                text: text.clone(),
                send_ts: ts.clone(),
                recv_ts: None,
                file_path: Some(path.to_string_lossy().to_string()),
                transfer_status: Some(if sent { "发送中...".to_string() } else { "等待对方上线...".to_string() }),
                msg_id: None,
                is_read: true,
            });

            self.log_history(&id, true, &text, &ts, None, Some(&path.to_string_lossy()));
        }
    }

    fn pick_and_send(&mut self, pick_folder: bool) {
        let selection = if pick_folder {
            FileDialog::new().pick_folder()
        } else {
            FileDialog::new().pick_file()
        };

        if let Some(path) = selection {
            let is_dir = path.is_dir();
            self.append_file_message(&path, is_dir);
        }

        // 如果我们刚保存了用户名，通知网络线程更新广播（非阻塞）
        if let Some(tx) = &self.net_cmd_tx {
            let _ = tx.send(NetCmd::SendHello);
        }
    }

    fn handle_dropped_files(&mut self, dropped: Vec<egui::DroppedFile>) {
        for file in dropped {
            if let Some(path) = &file.path {
                let is_dir = path.is_dir();
                self.append_file_message(&path, is_dir);
            } else if !file.name.is_empty() {
                let text = format!("📄 {}", file.name);
                if let Some(id) = self.selected_user_id.clone() {
                    self.send_message_internal(&id, text);
                }
            }
        }
    }
}

impl eframe::App for RustleApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 强制每 100ms 刷新一次，确保消息及时显示
        ctx.request_repaint_after(Duration::from_millis(100));

        self.ensure_seed_data();
        let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());

        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("如梭");
                ui.separator();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let addr = match (self.local_ip.as_deref(), self.local_port) {
                        (Some(ip), Some(p)) => format!("{}:{}", ip, p),
                        _ => "".to_string(),
                    };
                    if !addr.is_empty() {
                        ui.label(egui::RichText::new(addr).weak());
                        ui.separator();
                    }

                    let status = self.me_name.as_deref().map(|n| format!("已登录: {}", n)).unwrap_or_else(|| "未登录".to_string());
                    ui.label(egui::RichText::new(status).weak());
                });
            });
        });

        // 处理网络发现事件
        if let Some(rx) = &self.peer_rx {
            let mut pending: Vec<PeerEvent> = Vec::new();
            while let Ok(evt) = rx.try_recv() {
                pending.push(evt);
            }
            for evt in pending {
                match evt {
                    PeerEvent::Discovered(peer, local_ip) => {
                        // 更新 UI 中保存的本地端口（若尚未设置）
                        if self.local_port.is_none() {
                            self.local_port = Some(UDP_PORT);
                        }

                        // 使用 peer.id 作为唯一键，避免创建重复联系人（多网卡场景）
                        let key = peer.id.clone();
                        let name = peer.name.clone().unwrap_or_else(|| format!("{}:{}", peer.ip, peer.port));

                        // 更新 peers map（以 peer.id 为键）
                        self.peers.insert(
                            key.clone(),
                            Peer {
                                id: peer.id.clone(),
                                ip: peer.ip.clone(),
                                port: peer.port,
                                tcp_port: peer.tcp_port,
                                name: peer.name.clone(),
                                last_seen: Local::now(),
                            },
                        );

                        // 选择更合适的 IP（优先私网/同网段）
                        // 既然收到了 Hello，说明该路径是通的，直接更新为最新路径，避免 prefer_ip 导致死守旧 IP
                        let preferred_ip = peer.ip.clone();
                        
                        // 标记为在线并添加/更新联系人（以 peer.id 为 id）
                        let (ip_clone, port_clone) = (preferred_ip.clone(), peer.port);
                        self.maybe_switch_primary_interface(&local_ip, &peer.ip);
                        if let Some(u) = self.users.iter_mut().find(|u| u.id == key) {
                            u.online = true;
                            u.name = name.clone();
                            u.ip = Some(ip_clone.clone());
                            u.port = Some(port_clone);
                            u.tcp_port = peer.tcp_port;
                            u.bound_interface = Some(local_ip.clone());
                            u.best_interface = Some(local_ip.clone());  // 首次以接收 hello 的接口作为最优接口
                            self.known_dirty = true;
                            let peer_id = u.id.clone();
                            let ip_opt = u.ip.clone();
                            let port_opt = u.port;
                            let _ = u; // release mutable borrow before flushing
                            self.flush_offline_queue(&peer_id, ip_opt.as_deref(), port_opt);
                        } else {
                            // 将 peer 添加到联系人列表（如果不存在）
                            self.users.push(User {
                                id: key.clone(),
                                name: name.clone(),
                                online: true,
                                ip: Some(ip_clone.clone()),
                                port: Some(port_clone),
                                tcp_port: peer.tcp_port,
                                bound_interface: Some(local_ip.clone()),
                                best_interface: Some(local_ip.clone()),  // 首次以接收 hello 的接口作为最优接口
                                has_unread: false,
                            });
                            self.messages.entry(key.clone()).or_default();
                            self.offline_msgs.entry(key.clone()).or_default();
                            self.known_dirty = true;
                            if self.selected_user_id.is_none() {
                                self.selected_user_id = Some(key.clone());
                            }
                            self.flush_offline_queue(&key, Some(&ip_clone), Some(port_clone));
                        }
                    }
                    PeerEvent::ChatReceived { from_id, from_ip, from_port, text, send_ts, recv_ts, msg_id, local_ip } => {
                        // 把接收到的消息添加到对应 peer.id 的历史记录
                        let key = from_id.clone();
                        
                        // 使用 msg_id 进行去重
                        if !self.received_msg_ids.contains(&msg_id) {
                            self.received_msg_ids.insert(msg_id.clone());
                            // 限制缓存大小，防止无限增长 (保留最近 1000 条)
                            if self.received_msg_ids.len() > 1000 {
                                self.received_msg_ids.clear();
                                self.received_msg_ids.insert(msg_id.clone());
                            }

                            let msgs = self.messages.entry(key.clone()).or_default();
                            msgs.push(ChatMessage {
                                from_me: false,
                                text: text.clone(),
                                send_ts: send_ts.clone(),
                                recv_ts: Some(recv_ts.clone()),
                                file_path: None,
                                transfer_status: None,
                                msg_id: Some(msg_id.clone()),
                                is_read: false,
                            });
                            self.log_history(&key, false, &text, &send_ts, Some(&recv_ts), None);
                            
                            // 触发任务栏闪烁
                            if !self.has_unread_messages {
                                self.has_unread_messages = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
                            }
                            
                            if self.selected_user_id.as_deref() == Some(&key) {
                                self.scroll_to_bottom = true;
                            }
                        }

                        // 确保 contact 存在，并强制更新为当前收信路径
                        // 收到消息意味着该路径当前是通畅的，应立即更新
                        let (ip_clone, port_clone) = (from_ip.clone(), from_port);
                        self.maybe_switch_primary_interface(&local_ip, &from_ip);
                        
                        if let Some(u) = self.users.iter_mut().find(|u| u.id == key) {
                            if self.selected_user_id.as_deref() != Some(&key) {
                                u.has_unread = true;
                            }
                            u.online = true;
                            u.ip = Some(ip_clone.clone());
                            u.port = Some(port_clone);
                            u.bound_interface = Some(local_ip.clone());
                            u.best_interface = Some(local_ip.clone());  // 更新最优接口为收到聊天的接口
                            self.known_dirty = true;
                            let peer_id = u.id.clone();
                            let ip_opt = u.ip.clone();
                            let port_opt = u.port;
                            let _ = u; // release mutable borrow before flushing
                            self.flush_offline_queue(&peer_id, ip_opt.as_deref(), port_opt);
                        } else {
                            let display = if from_id.is_empty() { format!("{}:{}", from_ip, from_port) } else { from_id.clone() };
                            self.users.push(User {
                                id: key.clone(),
                                name: display,
                                online: true,
                                ip: Some(ip_clone.clone()),
                                port: Some(port_clone),
                                tcp_port: None,
                                bound_interface: Some(local_ip.clone()),
                                best_interface: Some(local_ip.clone()),  // 初始最优接口为收到聊天的接口
                                has_unread: self.selected_user_id.as_deref() != Some(&key),
                            });
                            self.offline_msgs.entry(key.clone()).or_default();
                            self.known_dirty = true;
                            self.flush_offline_queue(&key, Some(&ip_clone), Some(port_clone));
                        }
                    }
                    PeerEvent::ChatAck { from_id, msg_id } => {
                        if let Some(list) = self.pending_acks.get_mut(&from_id) {
                            list.retain(|(mid, _)| mid != &msg_id);
                        }
                        // 更新用户最优接口（能收到 ACK 的接口）
                        if let Some(u) = self.users.iter_mut().find(|u| u.id == from_id) {
                            eprintln!("ACK received from {} (msg_id={}), updating best_interface to {}", from_id, msg_id, u.bound_interface.as_deref().unwrap_or("unknown"));
                            u.best_interface = u.bound_interface.clone();
                        }
                        if let Some(msgs) = self.messages.get_mut(&from_id) {
                            if let Some(m) = msgs.iter_mut().rev().find(|m| m.msg_id.as_deref() == Some(&msg_id)) {
                                m.transfer_status = Some("已送达".to_string());
                                m.recv_ts = Some(Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
                            }
                        }
                        if let Some(queue) = self.offline_msgs.get_mut(&from_id) {
                            queue.retain(|q| q.msg_id.as_deref() != Some(&msg_id));
                        }
                    }
                    PeerEvent::LocalBound { ip, port } => {
                        // 首次设置本机显示端口与 ip（优先选用已选择的本地 ip）
                        if self.local_port.is_none() {
                            self.local_port = Some(port);
                        }
                        if self.local_ip.is_none() {
                            self.local_ip = Some(ip);
                        }
                    }
                    PeerEvent::FileProgress { peer_id, file_name, progress, status, is_incoming, is_dir, local_path } => {
                        if let Some(pid) = peer_id {
                            if let Some(msgs) = self.messages.get_mut(&pid) {
                                if is_incoming {
                                    let log_key = (pid.clone(), file_name.clone());
                                    if progress == 0.0 {
                                        let ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                                        let text = if is_dir {
                                            format!("📁 {}", file_name)
                                        } else {
                                            format!("📄 {}", file_name)
                                        };
                                        msgs.push(ChatMessage {
                                            from_me: false,
                                            text: text.clone(),
                                            send_ts: ts.clone(),
                                            recv_ts: Some(ts.clone()),
                                            // 先用文件名占位，后续用本地路径覆盖，便于匹配更新
                                            file_path: Some(file_name.clone()),
                                            transfer_status: Some(status.clone()),
                                            msg_id: None,
                                            is_read: false,
                                        });

                                        if self.logged_incoming_files.insert(log_key.clone()) {
                                            let path_for_history = local_path
                                                .as_deref()
                                                .unwrap_or(file_name.as_str());
                                            self.log_history(
                                                &pid,
                                                false,
                                                &text,
                                                &ts,
                                                Some(&ts),
                                                Some(path_for_history),
                                            );
                                        }
                                    } else {
                                        if let Some(msg) = msgs.iter_mut().rev().find(|m| {
                                            !m.from_me && (
                                                m.file_path.as_ref().map(|p| p.ends_with(&file_name)).unwrap_or(false)
                                                || m.text.contains(&file_name)
                                            )
                                        }) {
                                            msg.transfer_status = Some(status.clone());
                                            if let Some(path) = local_path.as_deref() {
                                                msg.file_path = Some(path.to_string());
                                            } else if msg.file_path.is_none() {
                                                msg.file_path = Some(file_name.clone());
                                            }

                                            let mut to_log: Option<(String, String, String)> = None;
                                            if progress >= 1.0 && self.logged_incoming_files.insert(log_key.clone()) {
                                                let ts = msg
                                                    .recv_ts
                                                    .clone()
                                                    .unwrap_or_else(|| Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
                                                let path_for_history = msg
                                                    .file_path
                                                    .clone()
                                                    .unwrap_or_else(|| file_name.clone());
                                                to_log = Some((msg.text.clone(), ts, path_for_history));
                                            }

                                            if let Some((text, ts, path)) = to_log {
                                                self.log_history(
                                                    &pid,
                                                    false,
                                                    &text,
                                                    &ts,
                                                    Some(&ts),
                                                    Some(&path),
                                                );
                                            }
                                        }
                                    }
                                } else {
                                    if let Some(msg) = msgs.iter_mut().rev().find(|m| m.from_me && m.file_path.as_ref().map(|p| p.ends_with(&file_name)).unwrap_or(false)) {
                                        msg.transfer_status = Some(status.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        egui::SidePanel::left("contacts")
            .resizable(false)
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.heading("联系人");
                ui.add_space(8.0);

                        if self.users.is_empty() {
                    ui.label(egui::RichText::new("暂无联系人").weak());
                } else {
                    // 排序：在线优先，然后按名字排序
                    let mut sorted_users = self.users.clone();
                    sorted_users.sort_by(|a, b| {
                        if a.online != b.online {
                            b.online.cmp(&a.online) // 在线在前
                        } else {
                            a.name.cmp(&b.name)
                        }
                    });

                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for user in sorted_users {
                            let selected = self.selected_user_id.as_deref() == Some(&user.id);
                            
                            let mut label_text = user.name.clone();
                            if !user.online {
                                label_text = format!("{} (离线)", label_text);
                            }
                            if user.has_unread {
                                label_text.push_str(" *");
                            }

                            let text_color = if selected {
                                egui::Color32::WHITE
                            } else if user.has_unread {
                                egui::Color32::BLACK
                            } else if user.online {
                                egui::Color32::from_rgb(34, 197, 94) // 绿色
                            } else {
                                egui::Color32::from_gray(140) // 浅灰色
                            };

                            let bg_color = if user.has_unread {
                                egui::Color32::from_rgb(255, 255, 0) // 黄色背景
                            } else {
                                egui::Color32::TRANSPARENT
                            };

                            let text = if user.online {
                                egui::RichText::new(label_text).strong().color(text_color)
                            } else {
                                egui::RichText::new(label_text).color(text_color)
                            };

                            let resp = egui::Frame::none()
                                .fill(bg_color)
                                .inner_margin(2.0)
                                .rounding(2.0)
                                .show(ui, |ui| {
                                    ui.selectable_label(selected, text)
                                }).inner;

                            if resp.clicked() {
                                self.selected_user_id = Some(user.id.clone());
                                // 清除未读标记
                                if let Some(u) = self.users.iter_mut().find(|u| u.id == user.id) {
                                    u.has_unread = false;
                                }
                                // 滚动到第一条未读消息或最后一条
                                self.scroll_to_first_unread = true;
                            }

                            // 右键菜单
                            resp.context_menu(|ui| {
                                if ui.button("用户信息").clicked() {
                                    let ip_info = format!("IP: {}\nPort: {}\nTCP Port: {}\nID: {}", 
                                        user.ip.as_deref().unwrap_or("未知"),
                                        user.port.map(|p| p.to_string()).unwrap_or_else(|| "未知".to_string()),
                                        user.tcp_port.map(|p| p.to_string()).unwrap_or_else(|| "未知".to_string()),
                                        user.id
                                    );
                                    self.show_ip_dialog = Some((user.name.clone(), ip_info));
                                    ui.close_menu();
                                }
                                if ui.button("删除联系人").clicked() {
                                    // 标记删除，稍后在 update 中处理
                                    self.context_menu_user_id = Some(user.id.clone());
                                    ui.close_menu();
                                }
                            });
                        }
                    });
                }
            });

        // 处理删除联系人
        if let Some(id_to_delete) = self.context_menu_user_id.take() {
            if let Some(index) = self.users.iter().position(|u| u.id == id_to_delete) {
                self.users.remove(index);
                self.messages.remove(&id_to_delete);
                self.offline_msgs.remove(&id_to_delete);
                self.pending_acks.remove(&id_to_delete);
                self.peers.remove(&id_to_delete);
                
                // 清除相关的辅助状态
                self.logged_incoming_files.retain(|(pid, _)| pid != &id_to_delete);
                self.message_visible_since.retain(|(pid, _), _| pid != &id_to_delete);

                // 清除磁盘上的历史记录文件
                self.clear_history(&id_to_delete);

                self.known_dirty = true;
                if self.selected_user_id.as_deref() == Some(&id_to_delete) {
                    self.selected_user_id = None;
                }
            }
        }

        // 显示 IP 对话框
        let mut close_dialog = false;
        if let Some((name, info)) = &self.show_ip_dialog {
            let mut open = true;
            egui::Window::new(format!("{} 的 IP 信息", name))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(info);
                    if ui.button("关闭").clicked() {
                        close_dialog = true;
                    }
                });
            if !open {
                close_dialog = true;
            }
        }
        if close_dialog {
            self.show_ip_dialog = None;
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(self.selected_user_name());
            });
            ui.separator();
            
            let total_height = ui.available_height();
            let top_height = total_height * 0.75;

            // 上部区域：消息显示（3/4）
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), top_height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.set_height(top_height);
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let msgs: &[ChatMessage] = self
                                .selected_user_id
                                .as_ref()
                                .and_then(|id| self.messages.get(id))
                                .map(|v| v.as_slice())
                                .unwrap_or(&[]);

                            let mut last_msg_resp = None;
                            let mut first_unread_resp = None;
                            let now = Instant::now();
                            
                            for (i, msg) in msgs.iter().enumerate() {
                                let align = if msg.from_me {
                                    egui::Align::RIGHT
                                } else {
                                    egui::Align::LEFT
                                };
                                let resp = ui.with_layout(egui::Layout::top_down(align), |ui| {
                                    // 时间信息：发送时间 / 接收时间（小字）
                                    let mut meta = format!("发送: {}", msg.send_ts);
                                    if let Some(r) = &msg.recv_ts {
                                        meta.push_str(&format!("  |  接收: {}", r));
                                    }
                                    ui.label(egui::RichText::new(meta).small().weak());

                                    let bg = if msg.from_me {
                                        if msg.transfer_status.as_deref() == Some("未送达") {
                                            egui::Color32::from_gray(200)
                                        } else {
                                            egui::Color32::from_rgb(149, 236, 105)
                                        }
                                    } else {
                                        egui::Color32::WHITE
                                    };
                                    let fg = egui::Color32::BLACK;

                                    egui::Frame::none()
                                        .fill(bg)
                                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(220)))
                                        .rounding(egui::Rounding::same(6.0))
                                        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                                        .show(ui, |ui| {
                                            ui.label(egui::RichText::new(&msg.text).color(fg));
                                            
                                            if let Some(status) = &msg.transfer_status {
                                                ui.separator();
                                                ui.label(egui::RichText::new(format!("状态: {}", status)).small().color(fg));
                                            }
                                            
                                            if let Some(path) = &msg.file_path {
                                                ui.horizontal(|ui| {
                                                    if ui.link("📂 打开所在目录").clicked() {
                                                        if let Some(parent) = std::path::Path::new(path).parent() {
                                                            let _ = open::that(parent);
                                                        }
                                                    }
                                                });
                                            }
                                        });
                                }).response;
                                
                                // 追踪未读消息的可见性
                                if !msg.from_me && !msg.is_read {
                                    if first_unread_resp.is_none() {
                                        first_unread_resp = Some(resp.clone());
                                    }
                                    
                                    if ui.clip_rect().intersects(resp.rect) {
                                        if let Some(peer_id) = &self.selected_user_id {
                                            let key = (peer_id.clone(), i);
                                            self.message_visible_since.entry(key).or_insert(now);
                                        }
                                    }
                                }
                                
                                if i == msgs.len() - 1 {
                                    last_msg_resp = Some(resp);
                                }
                                ui.add_space(6.0);
                            }

                            if self.scroll_to_bottom {
                                if let Some(resp) = last_msg_resp {
                                    resp.scroll_to_me(Some(egui::Align::Center));
                                    self.scroll_to_bottom = false;
                                }
                            } else if self.scroll_to_first_unread {
                                if let Some(resp) = first_unread_resp {
                                    resp.scroll_to_me(Some(egui::Align::TOP));
                                } else if let Some(resp) = last_msg_resp {
                                    resp.scroll_to_me(Some(egui::Align::Center));
                                }
                                self.scroll_to_first_unread = false;
                            }
                        });
                },
            );

            ui.separator();

            // 下部区域：输入和按钮（剩余空间，约 1/4）
            let bottom_height = ui.available_height();
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), bottom_height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.set_height(bottom_height);
                    ui.spacing_mut().item_spacing.y = 6.0;

                    // 顶部行：文件/文件夹按钮（左对齐） + 发送按钮（右对齐）
                    ui.horizontal(|ui| {
                        ui.horizontal(|ui| {
                            if ui.add_sized([100.0, 36.0], egui::Button::new(egui::RichText::new("📁 文件").size(16.0))).clicked() {
                                self.pick_and_send(false);
                            }
                            ui.add_space(4.0);
                            if ui.add_sized([120.0, 36.0], egui::Button::new(egui::RichText::new("📂 文件夹").size(16.0))).clicked() {
                                self.pick_and_send(true);
                            }
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new("支持拖放文件/文件夹到窗口").weak().small());
                        });

                        // 右侧发送按钮，使用 right_to_left 布局保证对齐且不超出范围
                        // 添加右侧间距避免被滚动条遮挡
                        ui.add_space(8.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(8.0); // 距离右边缘留出间距
                            if ui.add_sized([100.0, 36.0], egui::Button::new(egui::RichText::new("🚀 发送").size(16.0))).clicked()
                                || ctx.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                self.send_current();
                            }
                        });
                    });

                    ui.add_space(6.0);

                    // 底部行：多行输入框，填满剩余空间
                    let input_height = ui.available_height();
                    ui.add(
                        egui::TextEdit::multiline(&mut self.input)
                            .hint_text("输入消息...")
                            .desired_width(f32::INFINITY)
                            .min_size(egui::vec2(0.0, input_height)),
                    );
                },
            );
        });

        // 启动或未设置用户名时显示输入对话框
        if self.show_name_dialog {
            egui::Window::new("欢迎使用 Rustle (如梭)")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("请输入你的姓名（建议使用真实姓名）：");
                    ui.add_space(6.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.temp_name_input)
                            .hint_text("例如：张三"),
                    );
                    ui.add_space(8.0);
                    if let Some(err) = &self.name_save_error {
                        ui.label(egui::RichText::new(err).color(egui::Color32::RED));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("保存并登录").clicked() {
                            let name = self.temp_name_input.trim();
                            if name.is_empty() {
                                self.name_save_error = Some("请输入名字后再保存".to_string());
                            } else {
                                match fs::write(data_path("me.txt"), name) {
                                    Ok(_) => {
                                        self.me_name = Some(name.to_string());
                                        self.show_name_dialog = false;
                                        self.name_save_error = None;
                                        self.temp_name_input.clear();

                                        // 通知网络线程更新用户名并广播 hello
                                        if let Some(tx) = &self.net_cmd_tx {
                                            let _ = tx.send(NetCmd::ChangeName(self.me_name.clone().unwrap_or_default()));
                                        }
                                    }
                                    Err(e) => {
                                        self.name_save_error = Some(format!("保存失败: {}", e));
                                    }
                                }
                            }
                        }
                        if ui.button("稍后再说").clicked() {
                            self.show_name_dialog = false;
                        }
                    });
                });
        }

        if !dropped_files.is_empty() {
            self.handle_dropped_files(dropped_files);
        }

        // 启动后仅触发一次：主动向已知节点定向 hello，提高上线成功率
        if !self.probed_known {
            if let Some(tx) = &self.net_cmd_tx {
                for u in &self.users {
                    if let (Some(ip), Some(port)) = (u.ip.as_ref(), u.port) {
                        let _ = tx.send(NetCmd::ProbePeer { ip: ip.clone(), port, via: u.bound_interface.clone() });
                    }
                }
                self.probed_known = true;
            }
        }

        // 检查待确认消息超时
        let now_instant = Instant::now();
        let mut timeouts: Vec<(String, String)> = Vec::new(); // peer_id, msg_id
        for (peer, list) in self.pending_acks.iter() {
            for (msg_id, deadline) in list.iter() {
                if now_instant > *deadline {
                    timeouts.push((peer.clone(), msg_id.clone()));
                }
            }
        }
        if !timeouts.is_empty() {
            for (peer, msg_id) in &timeouts {
                if let Some(msgs) = self.messages.get_mut(peer) {
                    if let Some(m) = msgs.iter_mut().rev().find(|m| m.msg_id.as_deref() == Some(msg_id)) {
                        m.transfer_status = Some("未送达".to_string());
                        // 重新加入离线队列时，保留原始 msg_id 以便接收方去重
                        self.offline_msgs.entry(peer.clone()).or_default().push(QueuedMsg { 
                            text: m.text.clone(), 
                            send_ts: m.send_ts.clone(), 
                            msg_id: Some(msg_id.clone()),
                            file_path: None,
                            is_dir: false,
                        });
                    }
                }
                self.mark_offline(peer);
            }
            for (peer, msg_id) in timeouts {
                if let Some(list) = self.pending_acks.get_mut(&peer) {
                    list.retain(|(mid, _)| mid != &msg_id);
                }
            }
        }

        // 清理长时间离线的 peers（例如 120 秒未见）
        let timeout = Duration::from_secs(120);
        let now = Local::now();
        let mut to_offline = Vec::new();
        for (k, p) in &self.peers {
            if now.signed_duration_since(p.last_seen).num_seconds() > timeout.as_secs() as i64 {
                to_offline.push(k.clone());
            }
        }
        for k in to_offline {
            if let Some(_p) = self.peers.remove(&k) {
                if let Some(u) = self.users.iter_mut().find(|u| u.id == k) {
                    u.online = false;
                    self.known_dirty = true;
                }
            }
        }
        
        // 标记已读：检查可见超过 3 秒的消息
        let read_threshold = Duration::from_secs(3);
        let mut to_mark_read = Vec::new();
        for ((peer_id, msg_idx), &visible_since) in &self.message_visible_since {
            if now_instant.duration_since(visible_since) >= read_threshold {
                to_mark_read.push((peer_id.clone(), *msg_idx));
            }
        }
        
        for (peer_id, msg_idx) in &to_mark_read {
            if let Some(msgs) = self.messages.get_mut(peer_id) {
                if let Some(msg) = msgs.get_mut(*msg_idx) {
                    if !msg.is_read {
                        msg.is_read = true;
                    }
                }
            }
            self.message_visible_since.remove(&(peer_id.clone(), *msg_idx));
        }
        
        // 检查是否还有未读消息
        let has_any_unread = self.messages.values().any(|msgs| {
            msgs.iter().any(|m| !m.from_me && !m.is_read)
        });
        
        if !has_any_unread && self.has_unread_messages {
            // 所有消息已读，停止闪烁
            self.has_unread_messages = false;
            // egui 0.29 ViewportCommand::RequestUserAttention takes UserAttentionType, which doesn't have None.
            // However, usually flashing stops when window is focused.
            // If we want to clear it programmatically, we might need a different approach or it might not be supported.
            // Let's try passing None if it was Option, but it seems it is not.
            // If we can't clear it, we just don't send anything.
            // But wait, if the API expects UserAttentionType, we can't pass None.
            // Let's comment it out for now if we can't verify the API.
            // Actually, let's try to see if we can use winit directly? No, too complex.
            // Let's assume we can't clear it programmatically via egui for now if None is not supported.
            // But let's try to see if there is a Reset.
            // If I just remove the line, the state `self.has_unread_messages` is updated, so we won't request it again.
        }

        // 将已知联系人持久化（仅当有更新时才写文件）
        self.persist_known_peers();

        // 如果我们刚刚保存了用户名，通知网络线程发送一次 hello
        if let Some(tx) = &self.net_cmd_tx {
            let _ = tx.send(NetCmd::SendHello);
        }
    }
}

// ----- 网络发现实现 -----

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiscoveredPeer {
    id: String,
    ip: String,
    port: u16,
    tcp_port: Option<u16>,
    name: Option<String>,
}

#[derive(Debug)]
enum NetCmd {
    ChangeName(String),
    SendHello,
    SendChat { ip: String, port: u16, text: String, ts: String, via: Option<String>, msg_id: String },
    ProbePeer { ip: String, port: u16, via: Option<String> },
    SendFile { peer_id: String, ip: String, tcp_port: u16, path: PathBuf, is_dir: bool, via: Option<String> },
}

#[derive(Debug)]
enum PeerEvent {
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
struct HelloMsg {
    msg_type: String,
    id: String,
    name: Option<String>,
    port: u16,
    tcp_port: Option<u16>,
    version: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ChatPayload {
    msg_type: String,
    msg_id: String,
    from_id: String,
    from_name: Option<String>,
    text: String,
    timestamp: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct AckPayload {
    msg_type: String,
    msg_id: String,
    from_id: String,
}

#[derive(Debug)]
enum FileCmd {
    SendFile {
        peer_id: String,
        peer_ip: String,
        tcp_port: u16,
        path: PathBuf,
        is_dir: bool,
        via: Option<String>,
    },
}

async fn handle_incoming_file(mut socket: TcpStream, addr: SocketAddr, peer_tx: Sender<PeerEvent>) {
    let _ = socket.set_nodelay(true);
    let mut type_buf = [0u8; 1];
    if socket.read_exact(&mut type_buf).await.is_err() {
        eprintln!("[rx] failed to read type from {addr}");
        return;
    }
    let is_dir = type_buf[0] == 1;

    let mut id_len_buf = [0u8; 1];
    if socket.read_exact(&mut id_len_buf).await.is_err() {
        eprintln!("[rx] failed to read id_len from {addr}");
        return;
    }
    let id_len = id_len_buf[0] as usize;

    let mut id_buf = vec![0u8; id_len];
    if socket.read_exact(&mut id_buf).await.is_err() {
        eprintln!("[rx] failed to read id bytes from {addr}");
        return;
    }
    let sender_id = String::from_utf8_lossy(&id_buf).to_string();

    let mut len_buf = [0u8; 2];
    if socket.read_exact(&mut len_buf).await.is_err() {
        eprintln!("[rx] failed to read name_len from {addr}");
        return;
    }
    let name_len = u16::from_be_bytes(len_buf) as usize;

    let mut name_buf = vec![0u8; name_len];
    if socket.read_exact(&mut name_buf).await.is_err() {
        eprintln!("[rx] failed to read name bytes from {addr}");
        return;
    }
    let filename = String::from_utf8_lossy(&name_buf).to_string();

    let mut size_buf = [0u8; 8];
    if socket.read_exact(&mut size_buf).await.is_err() {
        eprintln!("[rx] failed to read size bytes from {addr}");
        return;
    }
    let total_size = u64::from_be_bytes(size_buf);

    eprintln!("[rx] header from {addr} id={sender_id} name={filename} is_dir={is_dir} size={total_size}");

    let initial_status = if total_size > 0 {
        format!("正在接收 0% / {}", human_size(total_size))
    } else {
        "正在接收...".to_string()
    };

    let _ = peer_tx.send(PeerEvent::FileProgress {
        peer_id: Some(sender_id.clone()),
        file_name: filename.clone(),
        progress: 0.0,
        status: initial_status,
        is_incoming: true,
        is_dir,
        local_path: None,
    });

    let temp_dir = std::env::temp_dir();
    // 无论文件还是文件夹，都创建一个独立的子目录
    let sub_dir_name = format!("rustle_{}_{}", filename, Local::now().format("%H%M%S"));
    let sub_dir = temp_dir.join(sub_dir_name);
    let _ = fs::create_dir_all(&sub_dir);
    
    let save_path = sub_dir.join(&filename);
    
    let mut success = false;
    let mut last_progress = 0.0f32;
    let mut final_received: u64 = 0;
    let mut last_report_instant = Instant::now();
    let mut last_report_bytes: u64 = 0;

    if is_dir {
        let tar_path = save_path.with_extension("tar");
        if let Ok(mut file) = tokio::fs::File::create(&tar_path).await {
            let mut buf = [0u8; 256 * 1024];
            let mut received: u64 = 0;
            loop {
                match socket.read(&mut buf).await {
                    Ok(0) => { eprintln!("[rx] dir EOF after {received} bytes (target {total_size})"); break; },
                    Ok(n) => {
                        if file.write_all(&buf[..n]).await.is_err() { break; }
                        received += n as u64;
                        if total_size > 0 {
                            let progress = (received as f32 / total_size as f32).min(1.0);
                            if progress - last_progress >= 0.05 || last_report_bytes == 0 {
                                let now = Instant::now();
                                let status = format!(
                                    "正在接收 {:.0}% / {} ({})",
                                    progress * 100.0,
                                    human_size(total_size),
                                    format_speed(received - last_report_bytes, now.duration_since(last_report_instant)),
                                );
                                let _ = peer_tx.send(PeerEvent::FileProgress {
                                    peer_id: Some(sender_id.clone()),
                                    file_name: filename.clone(),
                                    progress,
                                    status,
                                    is_incoming: true,
                                    is_dir,
                                    local_path: None,
                                });
                                last_progress = progress;
                                last_report_instant = now;
                                last_report_bytes = received;
                            }
                            if received >= total_size {
                                eprintln!("[rx] dir reached declared size {received}/{total_size}");
                                success = true; break;
                            }
                        } else {
                            if received % (8 * 1024 * 1024) == 0 { // log every 8MB for unknown size
                                eprintln!("[rx] dir received {received} bytes (size unknown)");
                            }
                        }
                    }
                    Err(e) => { eprintln!("[rx] dir read error after {received}: {e}"); break; },
                }
            }
            
            if success || (total_size == 0 && received > 0) {
                let tar_path_clone = tar_path.clone();
                let unpack_dst = sub_dir.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(f) = std::fs::File::open(&tar_path_clone) {
                        let mut ar = tar::Archive::new(f);
                        let _ = ar.unpack(&unpack_dst);
                    }
                }).await;
                success = true;
                eprintln!("[rx] dir unpacked into {:?}", save_path);
            }
            final_received = received;
            let _ = tokio::fs::remove_file(&tar_path).await;
        } else {
            eprintln!("[rx] failed to create tar temp file at {:?}", tar_path);
        }
    } else {
        if let Ok(mut file) = tokio::fs::File::create(&save_path).await {
            let mut buf = [0u8; 512 * 1024];
            let mut received: u64 = 0;
            loop {
                match socket.read(&mut buf).await {
                    Ok(0) => { eprintln!("[rx] file EOF after {received} bytes (target {total_size})"); break; },
                    Ok(n) => {
                        if file.write_all(&buf[..n]).await.is_err() { break; }
                        received += n as u64;
                        if total_size > 0 {
                            let progress = (received as f32 / total_size as f32).min(1.0);
                            if progress - last_progress >= 0.05 || last_report_bytes == 0 {
                                let now = Instant::now();
                                let status = format!(
                                    "正在接收 {:.0}% / {} ({})",
                                    progress * 100.0,
                                    human_size(total_size),
                                    format_speed(received - last_report_bytes, now.duration_since(last_report_instant)),
                                );
                                let _ = peer_tx.send(PeerEvent::FileProgress {
                                    peer_id: Some(sender_id.clone()),
                                    file_name: filename.clone(),
                                    progress,
                                    status,
                                    is_incoming: true,
                                    is_dir,
                                    local_path: None,
                                });
                                last_progress = progress;
                                last_report_instant = now;
                                last_report_bytes = received;
                            }
                            if received >= total_size {
                                eprintln!("[rx] file reached declared size {received}/{total_size}");
                                success = true; break;
                            }
                        } else {
                            if received % (8 * 1024 * 1024) == 0 {
                                eprintln!("[rx] file received {received} bytes (size unknown)");
                            }
                        }
                    }
                    Err(e) => { eprintln!("[rx] file read error after {received}: {e}"); break; },
                }
            }
            if total_size == 0 && received > 0 {
                success = true;
            }
            final_received = received;
        } else {
            eprintln!("[rx] failed to create file at {:?}", save_path);
        }
    }

    let completed_size = if total_size > 0 { total_size } else { final_received };
    let status = if success {
        if completed_size > 0 {
            format!("接收完成 ({})", human_size(completed_size))
        } else {
            "接收完成".to_string()
        }
    } else {
        "接收失败".to_string()
    };
    let _ = peer_tx.send(PeerEvent::FileProgress {
        peer_id: Some(sender_id),
        file_name: filename,
        progress: 1.0,
        status,
        is_incoming: true,
        is_dir,
        local_path: Some(save_path.to_string_lossy().to_string()),
    });
}

async fn handle_outgoing_file(my_id: String, peer_id: String, peer_ip: String, tcp_port: u16, path: PathBuf, is_dir: bool, via: Option<String>, peer_tx: Sender<PeerEvent>) {
    let addr_str = format!("{}:{}", peer_ip, tcp_port);
    let socket = if let Some(via_ip) = via {
        match tokio::net::TcpSocket::new_v4() {
            Ok(s) => {
                if let Ok(bind_addr) = format!("{}:0", via_ip).parse::<SocketAddr>() {
                    let _ = s.bind(bind_addr);
                }
                s.connect(addr_str.parse().unwrap()).await
            }
            Err(_) => TcpStream::connect(&addr_str).await
        }
    } else {
        TcpStream::connect(&addr_str).await
    };

    let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let declared_size: u64;
    let mut send_path = path.clone();
    let mut cleanup_path: Option<PathBuf> = None;

    if is_dir {
        let temp_tar = std::env::temp_dir().join(format!("{}.tar", Uuid::new_v4()));
        let path_clone = path.clone();
        let tar_path = temp_tar.clone();
        let filename_clone = filename.clone();

        let res = tokio::task::spawn_blocking(move || {
            let file = std::fs::File::create(&tar_path)?;
            let mut builder = tar::Builder::new(file);
            builder.append_dir_all(&filename_clone, &path_clone)?;
            builder.finish()?;
            Ok::<(), std::io::Error>(())
        }).await;

        match res {
            Ok(Ok(())) => {
                if let Ok(meta) = tokio::fs::metadata(&temp_tar).await {
                    declared_size = meta.len();
                    send_path = temp_tar.clone();
                    cleanup_path = Some(temp_tar);
                } else {
                    let _ = peer_tx.send(PeerEvent::FileProgress {
                        peer_id: Some(peer_id),
                        file_name: filename,
                        progress: 0.0,
                        status: "打包目录失败".to_string(),
                        is_incoming: false,
                        is_dir,
                        local_path: Some(path.to_string_lossy().to_string()),
                    });
                    return;
                }
            }
            _ => {
                let _ = tokio::fs::remove_file(&temp_tar).await;
                let _ = peer_tx.send(PeerEvent::FileProgress {
                    peer_id: Some(peer_id),
                    file_name: filename,
                    progress: 0.0,
                    status: "打包目录失败".to_string(),
                    is_incoming: false,
                    is_dir,
                    local_path: Some(path.to_string_lossy().to_string()),
                });
                return;
            }
        }
    } else {
        declared_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    }

    if let Ok(mut socket) = socket {
        let _ = socket.set_nodelay(true);
        let _ = peer_tx.send(PeerEvent::FileProgress {
            peer_id: Some(peer_id.clone()),
            file_name: filename.clone(),
            progress: 0.0,
            status: "发送中...".to_string(),
            is_incoming: false,
            is_dir,
            local_path: Some(path.to_string_lossy().to_string()),
        });

        let name_bytes = filename.as_bytes();
        let name_len = name_bytes.len() as u16;
        let id_bytes = my_id.as_bytes();
        let id_len = id_bytes.len() as u8;

        let mut header = Vec::new();
        header.push(if is_dir { 1 } else { 0 });
        header.push(id_len);
        header.extend_from_slice(id_bytes);
        header.extend_from_slice(&name_len.to_be_bytes());
        header.extend_from_slice(name_bytes);
        header.extend_from_slice(&declared_size.to_be_bytes());

        eprintln!("[tx] sending header to {addr_str} id={my_id} peer={peer_id} name={filename} is_dir={is_dir} size={declared_size}");

        if socket.write_all(&header).await.is_ok() {
            match tokio::fs::File::open(&send_path).await {
                Ok(mut file) => {
                    let mut buf = [0u8; 512 * 1024];
                    let mut sent_total: u64 = 0;
                    let mut last_progress = 0.0f32;
                    let mut last_report_instant = Instant::now();
                    let mut last_report_bytes: u64 = 0;
                    let mut send_ok = true;

                    loop {
                        match file.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                if socket.write_all(&buf[..n]).await.is_err() {
                                    eprintln!("[tx] write error to {addr_str}");
                                    send_ok = false;
                                    break;
                                }
                                sent_total += n as u64;

                                if declared_size > 0 {
                                    let progress = (sent_total as f32 / declared_size as f32).min(1.0);
                                    let now = Instant::now();
                                    let elapsed = now.duration_since(last_report_instant);
                                    if progress - last_progress >= 0.05 || elapsed.as_secs_f64() >= 1.0 || last_report_bytes == 0 {
                                        let status = format!(
                                            "发送中 {:.0}% / {} ({})",
                                            progress * 100.0,
                                            human_size(declared_size),
                                            format_speed(sent_total - last_report_bytes, elapsed),
                                        );
                                        let _ = peer_tx.send(PeerEvent::FileProgress {
                                            peer_id: Some(peer_id.clone()),
                                            file_name: filename.clone(),
                                            progress,
                                            status,
                                            is_incoming: false,
                                            is_dir,
                                            local_path: Some(path.to_string_lossy().to_string()),
                                        });
                                        last_progress = progress;
                                        last_report_instant = now;
                                        last_report_bytes = sent_total;
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("[tx] read error from {:?}: {e}", send_path);
                                send_ok = false;
                                break;
                            }
                        }
                    }

                    let _ = socket.shutdown().await;
                    eprintln!("[tx] shutdown write half to {addr_str}");

                    if let Some(p) = cleanup_path.as_ref() {
                        let _ = tokio::fs::remove_file(p).await;
                    }

                    let final_progress = if declared_size > 0 {
                        (sent_total as f32 / declared_size as f32).min(1.0)
                    } else if send_ok {
                        1.0
                    } else {
                        0.0
                    };

                    let status_text = if send_ok && (declared_size == 0 || sent_total >= declared_size) {
                        "发送完成".to_string()
                    } else {
                        "发送失败".to_string()
                    };

                    let _ = peer_tx.send(PeerEvent::FileProgress {
                        peer_id: Some(peer_id),
                        file_name: filename,
                        progress: final_progress,
                        status: status_text,
                        is_incoming: false,
                        is_dir,
                        local_path: Some(path.to_string_lossy().to_string()),
                    });
                }
                Err(e) => {
                    eprintln!("[tx] open send_path {:?} failed: {e}", send_path);
                    if let Some(p) = cleanup_path.as_ref() {
                        let _ = tokio::fs::remove_file(p).await;
                    }
                    let _ = peer_tx.send(PeerEvent::FileProgress {
                        peer_id: Some(peer_id),
                        file_name: filename,
                        progress: 0.0,
                        status: "发送失败".to_string(),
                        is_incoming: false,
                        is_dir,
                        local_path: Some(path.to_string_lossy().to_string()),
                    });
                }
            }
        } else {
             if let Some(p) = cleanup_path.as_ref() {
                 let _ = tokio::fs::remove_file(p).await;
             }
             let _ = peer_tx.send(PeerEvent::FileProgress {
                peer_id: Some(peer_id),
                file_name: filename,
                progress: 0.0,
                status: "连接失败".to_string(),
                is_incoming: false,
                is_dir,
                local_path: Some(path.to_string_lossy().to_string()),
            });
        }
    } else {
        if let Some(p) = cleanup_path {
            let _ = tokio::fs::remove_file(p).await;
        }
        let _ = peer_tx.send(PeerEvent::FileProgress {
            peer_id: Some(peer_id),
            file_name: filename,
            progress: 0.0,
            status: "连接失败".to_string(),
            is_incoming: false,
            is_dir,
            local_path: Some(path.to_string_lossy().to_string()),
        });
    }
}

// spawn worker implementation will persist/read a node id in node_id.txt
fn spawn_network_worker(peer_tx: Sender<PeerEvent>, cmd_rx: Receiver<NetCmd>, initial_name: Option<String>) {
    thread::spawn(move || {
        // ensure we have a stable node id persisted across runs
        let id_path = "node_id.txt";
        let my_id = match fs::read_to_string(id_path) {
            Ok(s) => s.trim().to_string(),
            Err(_) => {
                let new_id = Uuid::new_v4().to_string();
                let _ = fs::write(id_path, &new_id);
                new_id
            }
        };
        let my_id_clone = my_id.clone();

        // Initialize Tokio runtime
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (file_cmd_tx, mut file_cmd_rx) = tokio::sync::mpsc::channel::<FileCmd>(32);

        // Bind TCP listener
        let (tcp_listener, tcp_port) = rt.block_on(async {
            let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            (listener, port)
        });

        let peer_tx_clone = peer_tx.clone();
        rt.spawn(async move {
            loop {
                if let Ok((socket, addr)) = tcp_listener.accept().await {
                    let tx = peer_tx_clone.clone();
                    tokio::spawn(async move {
                        handle_incoming_file(socket, addr, tx).await;
                    });
                }
            }
        });

        let peer_tx_clone2 = peer_tx.clone();
        rt.spawn(async move {
            while let Some(cmd) = file_cmd_rx.recv().await {
                match cmd {
                    FileCmd::SendFile { peer_id, peer_ip, tcp_port, path, is_dir, via } => {
                        handle_outgoing_file(my_id_clone.clone(), peer_id, peer_ip, tcp_port, path, is_dir, via, peer_tx_clone2.clone()).await;
                    }
                }
            }
        });

        // 端口选择并在每个 IPv4 接口上绑定一个 socket（固定 UDP_PORT）
        let last_port_path = data_path("last_port.txt");
        let mut bound_sockets: Vec<(UdpSocket, Ipv4Addr, u16)> = Vec::new();

        let is_private = |ip: &Ipv4Addr| {
            let o = ip.octets();
            (o[0] == 10) || (o[0] == 172 && (16..=31).contains(&o[1])) || (o[0] == 192 && o[1] == 168)
        };

        if let Ok(ifaces) = get_if_addrs() {
            for iface in ifaces {
                if let IpAddr::V4(ipv4) = iface.ip() {
                    if ipv4.is_loopback() {
                        continue; // 跳过 loopback 接口
                    }
                    if !is_private(&ipv4) {
                        continue; // 只绑定私网接口
                    }
                    match UdpSocket::bind((ipv4, UDP_PORT)) {
                        Ok(sock) => {
                            let _ = sock.set_broadcast(true);
                            let _ = sock.set_nonblocking(true);
                            bound_sockets.push((sock, ipv4, UDP_PORT));
                            // 报告回 UI 我们在该接口上绑定了端口
                            let _ = peer_tx.send(PeerEvent::LocalBound { ip: ipv4.to_string(), port: UDP_PORT });
                            // 保存首选端口（覆盖 last_port.txt）
                            let _ = fs::write(&last_port_path, UDP_PORT.to_string());
                        }
                        Err(e) => {
                            eprintln!("Net discovery: failed to bind {ipv4}:{UDP_PORT} - {e}");
                        }
                    }
                }
            }
        }

        // 如果没有找到任何私网接口绑定，无法继续（不使用 loopback）
        if bound_sockets.is_empty() {
            eprintln!("Net discovery: 未找到任何私网接口或端口 {UDP_PORT} 被占用，无法启动（不支持 loopback 模式）");
            return;
        }

        let mut our_name = initial_name.unwrap_or_default();

        // 发送 hello 的函数（包含 our id）
        let send_hello = |sock: &UdpSocket, target: SocketAddr, name: &str, port: u16, my_id: &str, tcp_port: u16| {
            let msg = HelloMsg {
                msg_type: "hello".to_string(),
                id: my_id.to_string(),
                name: if name.is_empty() { None } else { Some(name.to_string()) },
                port,
                tcp_port: Some(tcp_port),
                version: "0.1".to_string(),
            };
            if let Ok(payload) = serde_json::to_vec(&msg) {
                let _ = sock.send_to(&payload, target);
            }
        };

        // 构建目标地址列表：对每个有效接口计算定向广播地址并发送到该地址
        // 广播目标列表：仅全局广播（不使用 loopback），固定 UDP_PORT
        let mut base_targets: Vec<SocketAddr> = vec![
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)), UDP_PORT),
        ];

        // 针对每个接口计算定向广播地址（根据 netmask），过滤 loopback/link-local
        if let Ok(ifaces) = get_if_addrs() {
            for iface in ifaces {
                if let IpAddr::V4(ipv4) = iface.ip() {
                    if ipv4.is_loopback() || ipv4.octets()[0] == 169 { // skip link-local 169.254.x.x
                        continue;
                    }

                    // 计算接口的广播地址：优先使用常见私网掩码规则，如果有特殊子网需改进可再接入系统 netmask
                    fn heuristic_broadcast(ipv4: Ipv4Addr) -> Ipv4Addr {
                        let oct = ipv4.octets();
                        // 10.0.0.0/8 -> broadcast 10.255.255.255
                        if oct[0] == 10 {
                            return Ipv4Addr::new(10, 255, 255, 255);
                        }
                        // 172.16.0.0 - 172.31.255.255 -> /12 mask 255.240.0.0
                        if oct[0] == 172 && (16..=31).contains(&oct[1]) {
                            let first = oct[0];
                            let second = oct[1] | (!0xF0u8);
                            return Ipv4Addr::new(first, second, 255, 255);
                        }
                        // 192.168.x.x -> /24
                        if oct[0] == 192 && oct[1] == 168 {
                            return Ipv4Addr::new(oct[0], oct[1], oct[2], 255);
                        }
                        // otherwise fallback to /24
                        Ipv4Addr::new(oct[0], oct[1], oct[2], 255)
                    }

                    let bcast_ip = heuristic_broadcast(ipv4);

                    base_targets.push(SocketAddr::new(IpAddr::V4(bcast_ip), UDP_PORT));
                }
            }
        }

        // 去重
        base_targets.sort_by_key(|a| (a.ip().to_string(), a.port()));
        base_targets.dedup();

        // 频繁度控制：3 秒广播一次
        let mut last_broadcast = Instant::now() - Duration::from_secs(3);

        let mut buf = [0u8; 2048];

        loop {
            // 处理命令（非阻塞）
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    NetCmd::ChangeName(new_name) => {
                        our_name = new_name;
                        // 立刻广播（从每个 socket 发出）
                        for (sock, _ip, port) in &bound_sockets {
                            for addr in &base_targets {
                                send_hello(sock, *addr, &our_name, *port, &my_id, tcp_port);
                            }
                        }
                    }
                    NetCmd::SendHello => {
                        for (sock, _ip, port) in &bound_sockets {
                            for addr in &base_targets {
                                send_hello(sock, *addr, &our_name, *port, &my_id, tcp_port);
                            }
                        }
                    }
                    NetCmd::SendFile { peer_id, ip, tcp_port, path, is_dir, via } => {
                        let _ = rt.block_on(file_cmd_tx.send(FileCmd::SendFile { peer_id, peer_ip: ip, tcp_port, path, is_dir, via }));
                    }
                    NetCmd::ProbePeer { ip, port, via } => {
                        if let Ok(ipaddr) = ip.parse::<IpAddr>() {
                            if let IpAddr::V4(v4) = ipaddr {
                                // 仅同网段探测，避免跨网噪声
                                let send_ok = bound_sockets.iter().any(|(_, lip, _)| {
                                    let lp = lip.octets();
                                    let vp = v4.octets();
                                    lp[0] == vp[0] && lp[1] == vp[1] && lp[2] == vp[2]
                                });
                                if !send_ok { continue; }
                            }
                            let target = SocketAddr::new(ipaddr, port);
                            eprintln!("Probing known peer {}", target);
                            for (sock, _ip, p) in &bound_sockets {
                                if let Some(v) = &via {
                                    if _ip.to_string() != *v { continue; }
                                }
                                send_hello(sock, target, &our_name, *p, &my_id, tcp_port);
                            }
                        }
                    }
                    NetCmd::SendChat { ip, port, text, ts, via, msg_id } => {
                        // 同时从所有绑定的 sockets 发出，增加送达概率
                        let payload = ChatPayload {
                            msg_type: "chat".to_string(),
                            msg_id: msg_id.clone(),
                            from_id: my_id.clone(),
                            from_name: if our_name.is_empty() { None } else { Some(our_name.clone()) },
                            text: text.clone(),
                            timestamp: ts.clone(),
                        };
                        if let Ok(data) = serde_json::to_vec(&payload) {
                            let target = match ip.parse::<IpAddr>() {
                                Ok(ipaddr) => SocketAddr::new(ipaddr, port),
                                Err(_) => continue,
                            };
                            eprintln!("Sending chat to {}: {:?}", target, payload);
                            // 发送给所有绑定接口（本地网卡可靠性高，不需额外重试）
                            for (sock, _ip, _port) in &bound_sockets {
                                if let Some(v) = &via {
                                    if _ip.to_string() != *v { continue; }
                                }
                                let _ = sock.send_to(&data, target);
                                eprintln!("Sent chat from {}:{}", _ip, _port);
                            }
                        }
                    }
                }
            }

            // 周期性广播（3s）
            if last_broadcast.elapsed() > Duration::from_secs(3) {
                for (sock, _ip, port) in &bound_sockets {
                    for addr in &base_targets {
                        send_hello(sock, *addr, &our_name, *port, &my_id, tcp_port);
                    }
                }
                last_broadcast = Instant::now();
            }

            // 接收来自所有 sockets（非阻塞）
            for (sock, _ip, _port) in &bound_sockets {
                loop {
                    match sock.recv_from(&mut buf) {
                        Ok((n, src)) => {
                            if let Ok(text) = std::str::from_utf8(&buf[..n]) {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                                    if let Some(mt) = v.get("msg_type").and_then(|m| m.as_str()) {
                                        match mt {
                                            "hello" => {
                                                if let Ok(h) = serde_json::from_value::<HelloMsg>(v) {
                                                    if h.id != my_id {
                                                        let peer = DiscoveredPeer {
                                                            id: h.id.clone(),
                                                            ip: src.ip().to_string(),
                                                            port: h.port,
                                                            tcp_port: h.tcp_port,
                                                            name: h.name.clone(),
                                                        };
                                                        let _ = peer_tx.send(PeerEvent::Discovered(peer, _ip.to_string()));

                                                        // 回复到对方报告的端口（包含我们的 id）
                                                        let target = SocketAddr::new(src.ip(), h.port);
                                                        let reply = HelloMsg {
                                                            msg_type: "hello".to_string(),
                                                            id: my_id.clone(),
                                                            name: if our_name.is_empty() { None } else { Some(our_name.clone()) },
                                                            port: *_port,
                                                            tcp_port: Some(tcp_port),
                                                            version: "0.1".to_string(),
                                                        };
                                                        let _ = sock.send_to(&serde_json::to_vec(&reply).unwrap(), target);
                                                    }
                                                }
                                            }
                                            "chat" => {
                                                if let Ok(c) = serde_json::from_value::<ChatPayload>(v) {
                                                    eprintln!("Received chat from {}: {:?}", src, c);
                                                    // 回复 ACK（仅 1 次，超时重传由发送端处理）
                                                    let ack = AckPayload { msg_type: "ack".to_string(), msg_id: c.msg_id.clone(), from_id: my_id.clone() };
                                                    if let Ok(ack_data) = serde_json::to_vec(&ack) {
                                                        eprintln!("Sending ACK for msg_id={} to {}", c.msg_id, src);
                                                        let _ = sock.send_to(&ack_data, src);
                                                    }

                                                    let _ = peer_tx.send(PeerEvent::ChatReceived {
                                                        from_id: c.from_id.clone(),
                                                        from_ip: src.ip().to_string(),
                                                        from_port: src.port(),
                                                        text: c.text.clone(),
                                                        send_ts: c.timestamp.clone(),
                                                        recv_ts: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                                                        msg_id: c.msg_id.clone(),
                                                        local_ip: _ip.to_string(),
                                                    });
                                                }
                                            }
                                            "ack" => {
                                                if let Ok(a) = serde_json::from_value::<AckPayload>(v) {
                                                    eprintln!("Received ACK for msg_id={} from {}", a.msg_id, src);
                                                    let _ = peer_tx.send(PeerEvent::ChatAck { from_id: a.from_id, msg_id: a.msg_id });
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            // Windows UDP can surface ConnectionReset (os error 10054) when a peer replies ICMP port unreachable.
                            match e.kind() {
                                ErrorKind::WouldBlock => break,
                                ErrorKind::ConnectionReset => continue,
                                _ => {
                                    eprintln!("Net discovery recv error: {}", e);
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            // 避免忙循环
            thread::sleep(Duration::from_millis(10));
        }
    });
}

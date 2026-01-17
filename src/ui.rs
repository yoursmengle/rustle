use crate::model::{
    ChatMessage, HistoryEntry, KnownPeer, NetCmd, Peer, PeerEvent, QueuedMsg, User, HISTORY_FILE,
    KNOWN_PEERS_FILE, TCP_DIR_PORT, TCP_FILE_PORT, UDP_DISCOVERY_PORT, UDP_MESSAGE_PORT,
};
use crate::net::spawn_network_worker;
use crate::storage::data_path;
use chrono::{Duration as ChronoDuration, Local};
use eframe::egui;
use rfd::FileDialog;
use serde_json;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;
use sysinfo::{NetworkExt, SystemExt};

pub fn run() -> eframe::Result<()> {
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
        .with_title(format!("Rustle {}", crate::APP_VERSION))
        .with_inner_size([1200.0, 800.0]);

    if let Some(icon) = icon_data {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions { viewport, ..Default::default() };

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

                if let Ok(ifaces) = get_if_addrs::get_if_addrs() {
                    // 建立 iface name -> ipv4 list 映射
                    let mut name_to_ips: HashMap<String, Vec<std::net::Ipv4Addr>> = HashMap::new();
                    for iface in &ifaces {
                        if let IpAddr::V4(ipv4) = iface.ip() {
                            name_to_ips.entry(iface.name.clone()).or_default().push(ipv4);
                        }
                    }

                    // 使用 sysinfo 获取各接口的总流量，并选择最大者
                    let mut sys = sysinfo::System::new_all();
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

            // 固定使用 UDP_DISCOVERY_PORT 展示本机监听信息
            if app.local_port.is_none() {
                app.local_port = Some(UDP_DISCOVERY_PORT);
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

#[derive(Default)]
pub struct RustleApp {
    pub users: Vec<User>,
    pub selected_user_id: Option<String>,
    pub messages: HashMap<String, Vec<ChatMessage>>,
    pub input: String,
    pub pending_acks: HashMap<String, Vec<(String, Instant)>>,

    // 当前用户名称（从 me.txt 读取或用户输入）
    pub me_name: Option<String>,
    // 启动时是否显示输入姓名对话框
    pub show_name_dialog: bool,
    // 临时输入缓冲
    pub temp_name_input: String,
    // 保存错误信息（显示在对话框中）
    pub name_save_error: Option<String>,

    // 网络发现相关
    pub peers: HashMap<String, Peer>,
    pub peer_rx: Option<Receiver<PeerEvent>>,
    pub net_cmd_tx: Option<Sender<NetCmd>>,

    // 已知节点缓存
    pub probed_known: bool,
    pub known_dirty: bool,

    // 离线消息队列
    pub offline_msgs: HashMap<String, Vec<QueuedMsg>>,

    // 本机绑定信息（UI 使用）
    pub local_ip: Option<String>,
    pub local_port: Option<u16>,
    // 已成功绑定的接口列表（用于验证显示的本地地址是否可用）
    pub bound_interfaces: HashSet<String>,

    // 已接收消息 ID 缓存（用于去重）
    pub received_msg_ids: HashSet<String>,

    // 已记录到历史的入站文件（peer_id, file_name）避免重复
    pub logged_incoming_files: HashSet<(String, String)>,

    // 自动滚动标记
    pub scroll_to_bottom: bool,

    // 消息已读追踪：(peer_id, msg_index) -> 可见时刻
    pub message_visible_since: HashMap<(String, usize), Instant>,

    // 是否有未读消息（用于触发任务栏闪烁）
    pub has_unread_messages: bool,

    // 滚动到第一条未读消息
    pub scroll_to_first_unread: bool,

    // 本帧是否需要触发闪烁
    #[allow(dead_code)]
    pub should_flash: bool,

    // 右键菜单状态
    pub context_menu_user_id: Option<String>,
    pub show_ip_dialog: Option<(String, String)>,

    // 菜单状态
    pub show_usage_window: bool,
    pub show_update_dialog: bool,
    pub show_about_window: bool,

    // 修改用户名窗口
    pub show_edit_name_dialog: bool,
    pub edit_name_input: String,
    pub edit_name_error: Option<String>,

    // 升级检查
    pub update_check_rx: Option<Receiver<Result<(String, String), String>>>,
    pub is_checking_update: bool,
    pub new_version_info: Option<(String, String)>,
}

impl RustleApp {
    fn is_private_ipv4(ip: &str) -> bool {
        let parts: Vec<_> = ip.split('.').collect();
        if parts.len() != 4 {
            return false;
        }
        let p: Vec<u8> = parts.iter().filter_map(|s| s.parse::<u8>().ok()).collect();
        if p.len() != 4 {
            return false;
        }
        (p[0] == 10) || (p[0] == 172 && (16..=31).contains(&p[1])) || (p[0] == 192 && p[1] == 168)
    }

    fn same_lan(local: &str, candidate: &str) -> bool {
        let lp: Vec<_> = local.split('.').collect();
        let cp: Vec<_> = candidate.split('.').collect();
        if lp.len() != 4 || cp.len() != 4 {
            return false;
        }
        // 粗略同网段判断：前三段相同
        lp[0] == cp[0] && lp[1] == cp[1] && lp[2] == cp[2]
    }

    fn find_interface_for_target(target_ip_str: &str) -> Option<String> {
        let target_ip: std::net::Ipv4Addr = target_ip_str.parse().ok()?;
        if let Ok(ifaces) = get_if_addrs::get_if_addrs() {
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

    // 统一接口选择逻辑 - 修复消息发送问题
    fn get_best_interface_for_peer(&self, peer_ip: &str) -> Option<String> {
        // 优先选择能直接路由到目标的接口
        if let Some(direct_interface) = Self::find_interface_for_target(peer_ip) {
            // 验证该接口是否在我们的绑定列表中
            if self.bound_interfaces.contains(&direct_interface) {
                return Some(direct_interface);
            }
        }

        // 其次选择用户记录的最优接口
        if let Some(user) = self.users.iter().find(|u| u.ip.as_deref() == Some(peer_ip)) {
            if let Some(best) = &user.best_interface {
                if self.bound_interfaces.contains(best) {
                    return Some(best.clone());
                }
            }
            if let Some(bound) = &user.bound_interface {
                if self.bound_interfaces.contains(bound) {
                    return Some(bound.clone());
                }
            }
        }

        // 最后选择任意同网段的绑定接口
        for bound_ip in &self.bound_interfaces {
            if Self::same_lan(bound_ip, peer_ip) {
                return Some(bound_ip.clone());
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
        let cand_same_lan = if local_pref.is_empty() {
            false
        } else {
            Self::same_lan(&local_pref, candidate)
        };

        if let Some(cur) = current {
            let cur_priv = Self::is_private_ipv4(&cur);
            let cur_same_lan = if local_pref.is_empty() {
                false
            } else {
                Self::same_lan(&local_pref, &cur)
            };

            // Prefer candidate if it is private and current is not
            if cand_priv && !cur_priv {
                return candidate.to_string();
            }
            // Prefer candidate if both private but candidate matches local lan and current not
            if cand_priv && cur_priv && cand_same_lan && !cur_same_lan {
                return candidate.to_string();
            }
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

    fn log_history(
        &self,
        peer_id: &str,
        from_me: bool,
        text: &str,
        send_ts: &str,
        recv_ts: Option<&str>,
        file_path: Option<&str>,
    ) {
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
        let Ok(content) = fs::read_to_string(&path) else { return };
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
        let Some(ip) = ip else { return };
        let Some(port) = port else { return };
        let Some(tx) = self.net_cmd_tx.clone() else { return };
        if let Some(queue) = self.offline_msgs.get_mut(peer_id) {
            if queue.is_empty() {
                return;
            }
            let drained: Vec<QueuedMsg> = queue.drain(..).collect();
            let mut remain = Vec::new();

            // 使用统一的接口选择逻辑
            let via = self.get_best_interface_for_peer(ip);

            for msg in drained {
                if let Some(path) = &msg.file_path {
                    let target_tcp_port = if msg.is_dir { TCP_DIR_PORT } else { TCP_FILE_PORT };
                    if tx
                        .send(NetCmd::SendFile {
                            peer_id: peer_id.to_string(),
                            ip: ip.to_string(),
                            tcp_port: target_tcp_port,
                            path: path.clone(),
                            is_dir: msg.is_dir,
                            via: via.clone(),
                        })
                        .is_err()
                    {
                        remain.push(msg);
                    }
                } else {
                    let mid = msg.msg_id.clone().unwrap_or_else(|| Uuid::new_v4().to_string());
                    self.update_outgoing_msg_id(peer_id, &mid, &msg.text);

                    // 使用重试机制发送消息
                    self.try_send_message_with_retry(
                        peer_id,
                        ip,
                        port,
                        &msg.text,
                        &msg.send_ts,
                        &mid,
                        via.clone(),
                    );
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
                            port: None,
                            tcp_port: None,
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
                port: None,
                tcp_port: None,
                last_seen: None,
                bound_interface: u.best_interface.clone().or_else(|| u.bound_interface.clone()),
            })
            .collect();
        if let Ok(text) = serde_json::to_string_pretty(&list) {
            let _ = fs::write(data_path(KNOWN_PEERS_FILE), text);
        }
        self.known_dirty = false;
    }

    fn merge_users_by_id(&mut self) {
        let before = self.users.len();
        let mut first: HashMap<String, usize> = HashMap::new();
        let mut i = 0;
        while i < self.users.len() {
            let id = self.users[i].id.clone();
            if let Some(&keep_idx) = first.get(&id) {
                let dup = self.users.remove(i);
                let primary = &mut self.users[keep_idx];
                primary.online |= dup.online;
                if primary.ip.is_none() {
                    primary.ip = dup.ip.clone();
                }
                if primary.port.is_none() {
                    primary.port = dup.port;
                }
                if primary.tcp_port.is_none() {
                    primary.tcp_port = dup.tcp_port;
                }
                if primary.bound_interface.is_none() {
                    primary.bound_interface = dup.bound_interface.clone();
                }
                if primary.best_interface.is_none() {
                    primary.best_interface = dup.best_interface.clone();
                }
                primary.has_unread |= dup.has_unread;
                if primary.name.trim().is_empty() || primary.name == primary.id {
                    if !dup.name.trim().is_empty() {
                        primary.name = dup.name;
                    }
                }
                continue;
            } else {
                first.insert(id, i);
                i += 1;
            }
        }
        if before != self.users.len() {
            self.known_dirty = true;
        }
    }

    fn selected_user_name(&self) -> String {
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
        self.messages
            .entry(id.to_string())
            .or_default()
            .push(ChatMessage {
                from_me: true,
                text: text.clone(),
                send_ts: ts.clone(),
                recv_ts: None,
                file_path: None,
                transfer_status: Some("发送中...".to_string()),
                msg_id: Some(msg_id.clone()),
                is_read: true,
            });

        self.log_history(id, true, &text, &ts, None, None);

        let Some(u) = self.users.iter().find(|u| u.id == id) else { return };
        let online = u.online;
        let ip = u.ip.clone();
        let port = u.port;

        // 使用统一的接口选择逻辑
        let via = if let Some(target_ip) = &ip {
            self.get_best_interface_for_peer(target_ip)
        } else {
            None
        };

        let has_addr = ip.is_some() && port.is_some();

        if online && has_addr {
            if let (Some(ip), Some(port)) = (ip.as_deref(), port) {
                self.try_send_message_with_retry(id, &ip, port, &text, &ts, &msg_id, via);
            }
        } else {
            // 离线：排队，并且如果已有地址尝试一次乐观发送
            self.offline_msgs
                .entry(id.to_string())
                .or_default()
                .push(QueuedMsg {
                    text: text.to_string(),
                    send_ts: ts.clone(),
                    msg_id: Some(msg_id.clone()),
                    file_path: None,
                    is_dir: false,
                });

            if has_addr {
                if let (Some(ip), Some(port)) = (ip.as_deref(), port) {
                    if let Some(local) = self.local_ip.as_deref() {
                        if Self::same_lan(local, ip) {
                            self.try_send_message_with_retry(id, &ip, port, &text, &ts, &msg_id, via);
                        }
                    }
                }
            } else {
                // 没有地址信息，直接标记为等待上线
                if let Some(msgs) = self.messages.get_mut(id) {
                    if let Some(m) = msgs.iter_mut().rev().find(|m| m.msg_id.as_deref() == Some(&msg_id)) {
                        m.transfer_status = Some("等待对方上线...".to_string());
                    }
                }
            }
        }
    }

    fn try_send_message_with_retry(
        &mut self,
        peer_id: &str,
        ip: &str,
        _port: u16,
        text: &str,
        ts: &str,
        msg_id: &str,
        preferred_via: Option<String>,
    ) {
        let Some(tx) = &self.net_cmd_tx else { return };

        // 首先尝试使用首选接口发送
        if let Some(via) = preferred_via {
            if tx
                .send(NetCmd::SendChat {
                    ip: ip.to_string(),
                    text: text.to_string(),
                    ts: ts.to_string(),
                    via: Some(via),
                    msg_id: msg_id.to_string(),
                })
                .is_ok()
            {
                // 添加到待确认列表，延长超时时间到5秒
                let deadline = Instant::now() + Duration::from_secs(5);
                self.pending_acks
                    .entry(peer_id.to_string())
                    .or_default()
                    .push((msg_id.to_string(), deadline));
                return;
            }
        }

        // 如果首选接口失败，尝试所有可用接口
        let mut sent = false;
        for bound_ip in &self.bound_interfaces.clone() {
            if Self::same_lan(bound_ip, ip) {
                if tx
                    .send(NetCmd::SendChat {
                        ip: ip.to_string(),
                        text: text.to_string(),
                        ts: ts.to_string(),
                        via: Some(bound_ip.clone()),
                        msg_id: msg_id.to_string(),
                    })
                    .is_ok()
                {
                    sent = true;
                    break;
                }
            }
        }

        if sent {
            // 添加到待确认列表
            let deadline = Instant::now() + Duration::from_secs(5);
            self.pending_acks
                .entry(peer_id.to_string())
                .or_default()
                .push((msg_id.to_string(), deadline));
        } else {
            // 所有接口都失败，标记离线
            self.mark_offline(peer_id);
            if let Some(msgs) = self.messages.get_mut(peer_id) {
                if let Some(m) = msgs.iter_mut().rev().find(|m| m.msg_id.as_deref() == Some(msg_id)) {
                    m.transfer_status = Some("发送失败".to_string());
                }
            }
        }
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
            let (ip, via, online) = {
                let Some(user) = self.users.iter().find(|u| u.id == id) else { return };
                let via = if let Some(target_ip) = &user.ip {
                    self.get_best_interface_for_peer(target_ip)
                } else {
                    None
                };
                (user.ip.clone(), via, user.online)
            };

            let target_tcp_port = if is_dir { TCP_DIR_PORT } else { TCP_FILE_PORT };

            let icon = if is_dir { "📁" } else { "📄" };
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("item");

            if is_dir {
                let prep_text = format!("对方有一个文件夹（{}）正在准备发送", name);
                self.send_message_internal(&id, prep_text);
            }

            let text = format!("{} {}", icon, name);
            let ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

            let mut sent = false;
            if online {
                if let Some(ip) = ip {
                    if let Some(tx) = &self.net_cmd_tx {
                        if tx
                            .send(NetCmd::SendFile {
                                peer_id: id.clone(),
                                ip: ip.clone(),
                                tcp_port: target_tcp_port,
                                path: path.clone(),
                                is_dir,
                                via: via.clone(),
                            })
                            .is_ok()
                        {
                            sent = true;
                        } else {
                            self.mark_offline(&id);
                        }
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

fn spawn_check_update(tx: Sender<Result<(String, String), String>>) {
    thread::spawn(move || {
        let cmd = r#"
$ErrorActionPreference = 'Stop'
try {
    $r = Invoke-RestMethod -Uri "https://api.github.com/repos/yoursmengle/rustle/releases/latest" -TimeoutSec 10
    if ($r) {
        $r | Select-Object tag_name, html_url | ConvertTo-Json -Compress
    }
} catch {
    if ($_.Exception.Response.StatusCode -eq [System.Net.HttpStatusCode]::NotFound) {
        Write-Output '{"tag_name": "none", "html_url": ""}'
        exit 0
    }
    Write-Error $_
}
"#;
        #[cfg(target_os = "windows")]
        use std::os::windows::process::CommandExt;

        #[cfg(target_os = "windows")]
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", cmd])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .output();

        #[cfg(not(target_os = "windows"))]
        let output: std::io::Result<std::process::Output> =
            Err(std::io::Error::new(std::io::ErrorKind::Other, "Not supported"));

        match output {
            Ok(out) => {
                if out.status.success() {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&stdout) {
                        let tag = val["tag_name"].as_str().unwrap_or("").to_string();
                        let url = val["html_url"].as_str().unwrap_or("").to_string();

                        if tag == "none" {
                            let _ = tx.send(Err("暂无发布版本".to_string()));
                            return;
                        }

                        if !tag.is_empty() && !url.is_empty() {
                            let _ = tx.send(Ok((tag, url)));
                            return;
                        }
                    }
                    let _ = tx.send(Err("Failed to parse update info".to_string()));
                } else {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let _ = tx.send(Err(stderr.to_string()));
                }
            }
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
            }
        }
    });
}

impl eframe::App for RustleApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 强制每 100ms 刷新一次，确保消息及时显示
        ctx.request_repaint_after(Duration::from_millis(100));

        self.ensure_seed_data();

        // 检查升级结果
        if let Some(rx) = &self.update_check_rx {
            if let Ok(res) = rx.try_recv() {
                self.is_checking_update = false;
                match res {
                    Ok((ver, url)) => {
                        self.new_version_info = Some((ver, url));
                    }
                    Err(e) => {
                        eprintln!("Update check failed: {}", e);
                    }
                }
                self.update_check_rx = None;
            }
        }

        // 使用说明窗口
        let mut show_usage = self.show_usage_window;
        if show_usage {
            egui::Window::new("使用说明").open(&mut show_usage).show(ctx, |ui| {
                ui.label(egui::RichText::new("Rustle (如梭) 使用说明").heading());
                ui.add_space(10.0);
                ui.label("1. 节点发现：\n   软件启动会自动发现局域网内的其他 Rustle 节点。无需配置。");
                ui.add_space(5.0);
                ui.label("2. 发送消息：\n   点击左侧列表中的用户，在右侧输入框输入文字并回车即可发送。");
                ui.add_space(5.0);
                ui.label("3. 文件传输：\n   直接将文件或文件夹拖入聊天窗口即可发送给当前选中的用户。");
                ui.add_space(5.0);
            });
            self.show_usage_window = show_usage;
        }

        // 关于窗口
        let mut show_about = self.show_about_window;
        if show_about {
            egui::Window::new("关于")
                .open(&mut show_about)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(egui::RichText::new("Rustle (如梭)").heading());
                    ui.add_space(6.0);
                    ui.label(format!("版本号: {}", crate::APP_VERSION));
                });
            self.show_about_window = show_about;
        }

        // 升级窗口
        let mut show_update = self.show_update_dialog;
        if show_update {
            egui::Window::new("软件升级")
                .open(&mut show_update)
                .collapsible(false)
                .show(ctx, |ui| {
                    if self.is_checking_update {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("正在检查新版本...");
                        });
                    } else if let Some((ver, url)) = &self.new_version_info {
                        let current = env!("CARGO_PKG_VERSION");
                        let ver_clean = ver.trim_start_matches('v');

                        if ver_clean != current {
                            ui.label(format!("发现新版本: {}", ver));
                            ui.label(format!("当前版本: {}", current));
                            ui.add_space(10.0);
                            if ui.button("前往下载更新").clicked() {
                                let _ = open::that(url);
                            }
                        } else {
                            ui.label("当前已是最新版本。");
                            ui.label(format!("版本: {}", current));
                        }
                    } else if ui.button("检查更新").clicked() {
                        let (tx, rx) = mpsc::channel();
                        self.update_check_rx = Some(rx);
                        self.is_checking_update = true;
                        self.new_version_info = None;
                        spawn_check_update(tx);
                    }
                });
            self.show_update_dialog = show_update;
        }

        let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());

        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.heading("如梭");
                ui.separator();

                ui.menu_button("帮助", |ui| {
                    if ui.button("使用说明").clicked() {
                        self.show_usage_window = true;
                        ui.close_menu();
                    }
                    if ui.button("关于").clicked() {
                        self.show_about_window = true;
                        ui.close_menu();
                    }
                    if ui.button("软件升级").clicked() {
                        self.show_update_dialog = true;
                        let (tx, rx) = mpsc::channel();
                        self.update_check_rx = Some(rx);
                        self.is_checking_update = true;
                        self.new_version_info = None;
                        spawn_check_update(tx);
                        ui.close_menu();
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let addr = match (self.local_ip.as_deref(), self.local_port) {
                        (Some(ip), Some(p)) => format!("{}:{}", ip, p),
                        _ => "".to_string(),
                    };
                    if !addr.is_empty() {
                        ui.label(egui::RichText::new(addr).weak());
                        ui.separator();
                    }

                    let status = self
                        .me_name
                        .as_deref()
                        .map(|n| format!("用户名[双击修改]: {}", n))
                        .unwrap_or_else(|| "未登录".to_string());
                    let resp = ui.add(egui::Label::new(egui::RichText::new(status).weak()));
                    if resp.double_clicked() {
                        self.edit_name_input = self.me_name.clone().unwrap_or_default();
                        self.edit_name_error = None;
                        self.show_edit_name_dialog = true;
                    }
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
                            self.local_port = Some(UDP_DISCOVERY_PORT);
                        }

                        // 使用 peer.id 作为唯一键，避免创建重复联系人（多网卡场景）
                        let key = peer.id.clone();
                        let name = peer
                            .name
                            .clone()
                            .unwrap_or_else(|| format!("{}:{}", peer.ip, peer.port));

                        // 更新 peers map（以 peer.id 为键）
                        self.peers.insert(
                            key.clone(),
                            Peer {
                                id: peer.id.clone(),
                                ip: peer.ip.clone(),
                                port: UDP_MESSAGE_PORT,
                                tcp_port: None,
                                name: peer.name.clone(),
                                last_seen: Local::now(),
                            },
                        );

                        // 选择更合适的 IP（优先私网/同网段）
                        let preferred_ip = peer.ip.clone();

                        // 标记为在线并添加/更新联系人（以 peer.id 为 id）
                        let (ip_clone, port_clone) = (preferred_ip.clone(), UDP_MESSAGE_PORT);
                        self.maybe_switch_primary_interface(&local_ip, &peer.ip);
                        if let Some(u) = self.users.iter_mut().find(|u| u.id == key) {
                            u.online = true;
                            u.name = name.clone();
                            u.ip = Some(ip_clone.clone());
                            u.port = Some(port_clone);
                            u.tcp_port = None;
                            u.bound_interface = Some(local_ip.clone());
                            u.best_interface = Some(local_ip.clone());
                            self.known_dirty = true;
                            let peer_id = u.id.clone();
                            let ip_opt = u.ip.clone();
                            let port_opt = u.port;
                            let _ = u;
                            self.flush_offline_queue(&peer_id, ip_opt.as_deref(), port_opt);
                        } else {
                            self.users.push(User {
                                id: key.clone(),
                                name: name.clone(),
                                online: true,
                                ip: Some(ip_clone.clone()),
                                port: Some(port_clone),
                                tcp_port: None,
                                bound_interface: Some(local_ip.clone()),
                                best_interface: Some(local_ip.clone()),
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
                    PeerEvent::ChatReceived {
                        from_id,
                        from_ip,
                        from_port,
                        text,
                        send_ts,
                        recv_ts,
                        msg_id,
                        local_ip,
                    } => {
                        let key = from_id.clone();

                        if !self.received_msg_ids.contains(&msg_id) {
                            self.received_msg_ids.insert(msg_id.clone());
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
                                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                                    egui::UserAttentionType::Informational,
                                ));
                            }

                            if self.selected_user_id.as_deref() == Some(&key) {
                                self.scroll_to_bottom = true;
                            }
                        }

                        // 确保 contact 存在，并强制更新为当前收信路径
                        let (ip_clone, port_clone) = (from_ip.clone(), UDP_MESSAGE_PORT);
                        self.maybe_switch_primary_interface(&local_ip, &from_ip);

                        if let Some(u) = self.users.iter_mut().find(|u| u.id == key) {
                            if self.selected_user_id.as_deref() != Some(&key) {
                                u.has_unread = true;
                            }
                            u.online = true;
                            u.ip = Some(ip_clone.clone());
                            u.port = Some(port_clone);
                            u.bound_interface = Some(local_ip.clone());
                            u.best_interface = Some(local_ip.clone());
                            self.known_dirty = true;
                            let peer_id = u.id.clone();
                            let ip_opt = u.ip.clone();
                            let port_opt = u.port;
                            let _ = u;
                            self.flush_offline_queue(&peer_id, ip_opt.as_deref(), port_opt);
                        } else {
                            let display = if from_id.is_empty() {
                                format!("{}:{}", from_ip, from_port)
                            } else {
                                from_id.clone()
                            };
                            self.users.push(User {
                                id: key.clone(),
                                name: display,
                                online: true,
                                ip: Some(ip_clone.clone()),
                                port: Some(port_clone),
                                tcp_port: None,
                                bound_interface: Some(local_ip.clone()),
                                best_interface: Some(local_ip.clone()),
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
                            if let Some(bound) = &u.bound_interface {
                                u.best_interface = Some(bound.clone());
                                debug_println!(
                                    "ACK received from {} (msg_id={}), confirmed best_interface: {}",
                                    from_id,
                                    msg_id,
                                    bound
                                );
                            }
                        }
                        if let Some(msgs) = self.messages.get_mut(&from_id) {
                            if let Some(m) = msgs.iter_mut().rev().find(|m| m.msg_id.as_deref() == Some(&msg_id)) {
                                m.transfer_status = Some("已送达".to_string());
                                m.recv_ts =
                                    Some(Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
                            }
                        }
                        if let Some(queue) = self.offline_msgs.get_mut(&from_id) {
                            queue.retain(|q| q.msg_id.as_deref() != Some(&msg_id));
                        }
                    }
                    PeerEvent::LocalBound { ip, port } => {
                        // 记录可用的绑定接口
                        self.bound_interfaces.insert(ip.clone());
                        // 首次设置本机显示端口与 ip（优先选用已选择的本地 ip）
                        if self.local_port.is_none() {
                            self.local_port = Some(port);
                        }
                        // 如果当前显示的 ip 不在可用绑定列表，使用本次可用 ip 纠正
                        if self.local_ip.is_none()
                            || !self
                                .local_ip
                                .as_ref()
                                .map(|cur| self.bound_interfaces.contains(cur))
                                .unwrap_or(false)
                        {
                            self.local_ip = Some(ip);
                        }
                    }
                    PeerEvent::FileProgress {
                        peer_id,
                        file_name,
                        progress,
                        status,
                        is_incoming,
                        is_dir,
                        local_path,
                    } => {
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
                                            file_path: Some(file_name.clone()),
                                            transfer_status: Some(status.clone()),
                                            msg_id: None,
                                            is_read: false,
                                        });

                                        if self.logged_incoming_files.insert(log_key.clone()) {
                                            let path_for_history =
                                                local_path.as_deref().unwrap_or(file_name.as_str());
                                            self.log_history(&pid, false, &text, &ts, Some(&ts), Some(path_for_history));
                                        }
                                    } else if let Some(msg) = msgs.iter_mut().rev().find(|m| {
                                        !m.from_me
                                            && (m.file_path.as_ref().map(|p| p.ends_with(&file_name)).unwrap_or(false)
                                                || m.text.contains(&file_name))
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
                                            self.log_history(&pid, false, &text, &ts, Some(&ts), Some(&path));
                                        }
                                    }
                                } else if let Some(msg) = msgs.iter_mut().rev().find(|m| {
                                    m.from_me && m.file_path.as_ref().map(|p| p.ends_with(&file_name)).unwrap_or(false)
                                }) {
                                    msg.transfer_status = Some(status.clone());
                                }
                            }
                        }
                    }
                }
            }
            self.merge_users_by_id();
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
                    let mut online_users: Vec<User> = self.users.iter().filter(|u| u.online).cloned().collect();
                    let mut offline_users: Vec<User> = self.users.iter().filter(|u| !u.online).cloned().collect();
                    online_users.sort_by(|a, b| a.name.cmp(&b.name));
                    offline_users.sort_by(|a, b| a.name.cmp(&b.name));

                    let render_user = |ui: &mut egui::Ui, user: &User, this: &mut RustleApp| {
                        let selected = this.selected_user_id.as_deref() == Some(&user.id);

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
                            egui::Color32::from_rgb(34, 197, 94)
                        } else {
                            egui::Color32::from_gray(140)
                        };

                        let bg_color = if user.has_unread {
                            egui::Color32::from_rgb(255, 255, 0)
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
                                ui.push_id(&user.id, |ui| ui.add(egui::SelectableLabel::new(selected, text))).inner
                            })
                            .inner;

                        if resp.clicked() {
                            this.selected_user_id = Some(user.id.clone());
                            if let Some(u) = this.users.iter_mut().find(|u| u.id == user.id) {
                                u.has_unread = false;
                            }
                            this.scroll_to_first_unread = true;
                        }

                        resp.context_menu(|ui| {
                            if ui.button("用户信息").clicked() {
                                let ip_info: String = format!(
                                    "IP: {}\nID: {}",
                                    user.ip.as_deref().unwrap_or("未知"),
                                    user.id
                                );
                                this.show_ip_dialog = Some((user.name.clone(), ip_info));
                                ui.close_menu();
                            }
                            if ui.button("删除联系人").clicked() {
                                this.context_menu_user_id = Some(user.id.clone());
                                ui.close_menu();
                            }
                        });
                    };

                    let avail = ui.available_height();
                    let online_height = (avail * 0.5).max(120.0).min(avail);
                    let offline_height = (avail - online_height - 12.0).max(80.0);

                    ui.label(egui::RichText::new("在线").strong());
                    egui::ScrollArea::vertical()
                        .id_salt("online_users")
                        .max_height(online_height)
                        .show(ui, |ui| {
                            for user in &online_users {
                                render_user(ui, user, self);
                            }
                        });

                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(6.0);

                    ui.label(egui::RichText::new("离线").strong());
                    egui::ScrollArea::vertical()
                        .id_salt("offline_users")
                        .max_height(offline_height)
                        .show(ui, |ui| {
                            for user in &offline_users {
                                render_user(ui, user, self);
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

                self.logged_incoming_files.retain(|(pid, _)| pid != &id_to_delete);
                self.message_visible_since.retain(|(pid, _), _| pid != &id_to_delete);

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
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
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
                            let align = if msg.from_me { egui::Align::RIGHT } else { egui::Align::LEFT };
                            let resp = ui
                                .with_layout(egui::Layout::top_down(align), |ui| {
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
                                })
                                .response;

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

                    ui.horizontal(|ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .add_sized([100.0, 36.0], egui::Button::new(egui::RichText::new("📁 文件").size(16.0)))
                                .clicked()
                            {
                                self.pick_and_send(false);
                            }
                            ui.add_space(4.0);
                            if ui
                                .add_sized([120.0, 36.0], egui::Button::new(egui::RichText::new("📂 文件夹").size(16.0)))
                                .clicked()
                            {
                                self.pick_and_send(true);
                            }
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new("支持拖放文件/文件夹到窗口").weak().small());
                        });

                        ui.add_space(8.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(8.0);
                            if ui
                                .add_sized([100.0, 36.0], egui::Button::new(egui::RichText::new("🚀 发送").size(16.0)))
                                .clicked()
                                || ctx.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                self.send_current();
                            }
                        });
                    });

                    ui.add_space(6.0);

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

        if self.show_name_dialog {
            egui::Window::new("欢迎使用 Rustle (如梭)")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("请输入你的姓名（建议使用真实姓名）：");
                    ui.add_space(6.0);
                    ui.add(egui::TextEdit::singleline(&mut self.temp_name_input).hint_text("例如：张三"));
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

        if self.show_edit_name_dialog {
            egui::Window::new("修改用户名")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("请输入新的用户名：");
                    ui.add_space(6.0);
                    ui.add(egui::TextEdit::singleline(&mut self.edit_name_input).hint_text("例如：张三"));
                    ui.add_space(8.0);
                    if let Some(err) = &self.edit_name_error {
                        ui.label(egui::RichText::new(err).color(egui::Color32::RED));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("保存").clicked() {
                            let name = self.edit_name_input.trim();
                            if name.is_empty() {
                                self.edit_name_error = Some("请输入名字后再保存".to_string());
                            } else {
                                match fs::write(data_path("me.txt"), name) {
                                    Ok(_) => {
                                        self.me_name = Some(name.to_string());
                                        self.show_edit_name_dialog = false;
                                        self.edit_name_error = None;

                                        if let Some(tx) = &self.net_cmd_tx {
                                            let _ = tx.send(NetCmd::ChangeName(self.me_name.clone().unwrap_or_default()));
                                        }
                                    }
                                    Err(e) => {
                                        self.edit_name_error = Some(format!("保存失败: {}", e));
                                    }
                                }
                            }
                        }
                        if ui.button("取消").clicked() {
                            self.show_edit_name_dialog = false;
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
                    if let (Some(ip), Some(_port)) = (u.ip.as_ref(), u.port) {
                        let _ = tx.send(NetCmd::ProbePeer { ip: ip.clone(), via: u.bound_interface.clone() });
                    }
                }
                self.probed_known = true;
            }
        }

        // 检查待确认消息超时
        let now_instant = Instant::now();
        let mut timeouts: Vec<(String, String)> = Vec::new();
        for (peer, list) in self.pending_acks.iter() {
            for (msg_id, deadline) in list.iter() {
                if now_instant > *deadline {
                    timeouts.push((peer.clone(), msg_id.clone()));
                }
            }
        }
        if !timeouts.is_empty() {
            for (peer, msg_id) in &timeouts {
                let (retry_data, offline_data) = {
                    let user_addr = self
                        .users
                        .iter()
                        .find(|u| &u.id == peer)
                        .and_then(|u| u.ip.clone().and_then(|ip| u.port.map(|port| (ip, port))));

                    let mut retry_data = None;
                    let mut offline_data = None;

                    if let Some(msgs) = self.messages.get_mut(peer) {
                        if let Some(m) = msgs.iter_mut().rev().find(|m| m.msg_id.as_deref() == Some(msg_id)) {
                            if let Some((ip, port)) = user_addr {
                                retry_data = Some((ip, port, m.text.clone(), m.send_ts.clone()));
                            } else {
                                m.transfer_status = Some("未送达".to_string());
                                offline_data = Some((m.text.clone(), m.send_ts.clone()));
                            }
                        }
                    }

                    (retry_data, offline_data)
                };

                if let Some((text, send_ts)) = offline_data {
                    self.offline_msgs.entry(peer.clone()).or_default().push(QueuedMsg {
                        text,
                        send_ts,
                        msg_id: Some(msg_id.clone()),
                        file_path: None,
                        is_dir: false,
                    });
                }

                if let Some((ip, port, text, send_ts)) = retry_data {
                    debug_println!("Message timeout for {}, attempting retry via different interfaces", peer);
                    self.try_send_message_with_retry(peer, &ip, port, &text, &send_ts, msg_id, None);
                    continue;
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
        let has_any_unread = self
            .messages
            .values()
            .any(|msgs| msgs.iter().any(|m| !m.from_me && !m.is_read));

        if !has_any_unread && self.has_unread_messages {
            self.has_unread_messages = false;
        }

        self.persist_known_peers();
    }
}

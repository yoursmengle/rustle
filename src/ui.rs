use crate::debug_println;
use crate::delivery::DeliveryState;
use crate::history;
use crate::model::{
    ChatMessage, KnownPeer, NetCmd, Peer, PeerEvent, SyncNode, SyncTree, User, KNOWN_PEERS_FILE,
    TCP_DIR_PORT, TCP_FILE_PORT, UDP_DISCOVERY_PORT,
};
use crate::net::spawn_network_worker;
use crate::storage::{
    data_path, file_mtime_seconds, load_or_init_node_id, load_settings, load_sync_tree,
    save_sync_tree, sha256_file, AppSettings,
};
use chrono::Local;
use eframe::egui;
use rfd::FileDialog;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use sysinfo::{NetworkExt, SystemExt};
mod delivery_flow;
mod dialog_flow;
mod message_flow;
mod metadata_flow;
mod peer_flow;
mod render_flow;
mod runtime_flow;
mod transfer_flow;

// Theme colors for professional UI - Modern elegant design
#[allow(dead_code)]
mod theme {
    use eframe::egui;

    // Primary colors - Deep blue gradient palette
    pub const PRIMARY: egui::Color32 = egui::Color32::from_rgb(30, 64, 175); // #1e40af (deep blue)
    pub const PRIMARY_LIGHT: egui::Color32 = egui::Color32::from_rgb(59, 130, 246); // #3b82f6 (bright blue)
    pub const SECONDARY: egui::Color32 = egui::Color32::from_rgb(20, 184, 166); // #14b8a6 (teal)
    pub const SECONDARY_LIGHT: egui::Color32 = egui::Color32::from_rgb(45, 212, 191); // #2dd4bf

    // Background colors - Clean modern palette
    pub const BG_PRIMARY: egui::Color32 = egui::Color32::from_rgb(249, 250, 251); // #fafafa
    pub const BG_SECONDARY: egui::Color32 = egui::Color32::from_rgb(243, 244, 246); // #f3f4f6
    pub const BG_HOVER: egui::Color32 = egui::Color32::from_rgb(229, 231, 235); // #e5e7eb
    pub const BG_SELECTED: egui::Color32 = egui::Color32::from_rgb(224, 231, 255); // #e0e7ff

    // Text colors - Professional hierarchy
    pub const TEXT_PRIMARY: egui::Color32 = egui::Color32::from_rgb(17, 24, 39); // #111827
    pub const TEXT_SECONDARY: egui::Color32 = egui::Color32::from_rgb(75, 85, 99); // #4b5563
    pub const TEXT_TERTIARY: egui::Color32 = egui::Color32::from_rgb(156, 163, 175); // #9ca3af
    pub const TEXT_LIGHT: egui::Color32 = egui::Color32::from_rgb(209, 213, 219); // #d1d5db

    // Border colors
    pub const BORDER: egui::Color32 = egui::Color32::from_rgb(209, 213, 219); // #d1d5db
    pub const BORDER_LIGHT: egui::Color32 = egui::Color32::from_rgb(229, 231, 235); // #e5e7eb

    // Message bubble colors
    pub const MSG_SENT_BG: egui::Color32 = egui::Color32::from_rgb(59, 130, 246); // #3b82f6 (blue)
    pub const MSG_SENT_TEXT: egui::Color32 = egui::Color32::WHITE;
    pub const MSG_RECV_BG: egui::Color32 = egui::Color32::WHITE;
    pub const MSG_RECV_TEXT: egui::Color32 = egui::Color32::from_rgb(17, 24, 39); // #111827
    pub const MSG_RECV_BORDER: egui::Color32 = egui::Color32::from_rgb(229, 231, 235);

    // Status colors
    pub const STATUS_ONLINE: egui::Color32 = egui::Color32::from_rgb(16, 185, 129); // #10b981 (green)
    pub const STATUS_OFFLINE: egui::Color32 = egui::Color32::from_rgb(156, 163, 175); // #9ca3af
    pub const STATUS_UNREAD: egui::Color32 = egui::Color32::from_rgb(239, 68, 68);
    // #ef4444 (red)
}

#[cfg(target_os = "windows")]
fn platform_cjk_font_candidates() -> &'static [(&'static str, &'static str, u32)] {
    &[
        ("msyh", r"C:\Windows\Fonts\msyh.ttc", 0),
        ("msyh", r"C:\Windows\Fonts\msyh.ttf", 0),
        ("simhei", r"C:\Windows\Fonts\simhei.ttf", 0),
        ("simsun", r"C:\Windows\Fonts\simsun.ttc", 0),
    ]
}

#[cfg(target_os = "macos")]
fn platform_cjk_font_candidates() -> &'static [(&'static str, &'static str, u32)] {
    &[
        ("pingfang", "/System/Library/Fonts/PingFang.ttc", 0),
        (
            "hiragino_sans_gb",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            0,
        ),
        ("songti", "/System/Library/Fonts/Supplemental/Songti.ttc", 0),
        (
            "stheiti_light",
            "/System/Library/Fonts/STHeiti Light.ttc",
            0,
        ),
    ]
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_cjk_font_candidates() -> &'static [(&'static str, &'static str, u32)] {
    &[
        (
            "noto_sans_cjk_sc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            0,
        ),
        (
            "noto_sans_cjk_sc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            2,
        ),
        (
            "noto_sans_sc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            0,
        ),
        (
            "wqy_zenhei",
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
            0,
        ),
    ]
}

#[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
fn platform_cjk_font_candidates() -> &'static [(&'static str, &'static str, u32)] {
    &[]
}

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    for (font_name, font_path, font_index) in platform_cjk_font_candidates() {
        if let Ok(font_bytes) = fs::read(font_path) {
            let mut font_data = egui::FontData::from_owned(font_bytes);
            font_data.index = *font_index;

            fonts.font_data.insert((*font_name).to_owned(), font_data);
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, (*font_name).to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, (*font_name).to_owned());

            debug_println!(
                "Loaded CJK font '{}' from {} (index {})",
                font_name,
                font_path,
                font_index
            );
            ctx.set_fonts(fonts);
            return;
        }
    }

    debug_println!(
        "No platform CJK font found; falling back to egui default fonts, CJK text may not render correctly"
    );
    ctx.set_fonts(fonts);
}

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

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        "Rustle",
        options,
        Box::new(|cc| {
            // 加载中文字体
            configure_fonts(&cc.egui_ctx);

            // 在启动时检查用户数据目录下的 me.txt
            let mut app = RustleApp::default();
            app.self_id = load_or_init_node_id();
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

            // 加载设置
            app.settings = load_settings();

            // 初始化设置输入框
            let default_recv = crate::storage::default_download_dir();
            app.settings_recv_dir_input = app
                .settings
                .recv_dir
                .clone()
                .unwrap_or_else(|| default_recv.to_string_lossy().to_string());
            app.settings_name_input = app.me_name.clone().unwrap_or_default();

            // 启动时加载已知节点（离线列表）
            app.load_known_peers();
            // 启动时加载最近配置天数的历史记录
            app.load_recent_history();
            app.restore_runtime_state();
            // 启动时加载自动同步树
            app.load_sync_tree();
            // 加载 metadata cache
            app.refresh_metadata_cache();
            // 默认不显示同步管理窗口
            app.show_sync_window = false;

            // 启动时自动检查更新（可配置）
            if app.settings.auto_check_update {
                let (tx, rx) = mpsc::channel();
                app.update_check_rx = Some(rx);
                app.is_checking_update = true;
                app.new_version_info = None;
                spawn_check_update(tx);
            }

            // 尝试设置本机显示 IP：优先选择收发总流量最大的接口的 IPv4 地址
            if app.local_ip.is_none() {
                let mut chosen: Option<String> = None;

                if let Ok(ifaces) = get_if_addrs::get_if_addrs() {
                    // 建立 iface name -> ipv4 list 映射
                    let mut name_to_ips: HashMap<String, Vec<std::net::Ipv4Addr>> = HashMap::new();
                    for iface in &ifaces {
                        if let IpAddr::V4(ipv4) = iface.ip() {
                            name_to_ips
                                .entry(iface.name.clone())
                                .or_default()
                                .push(ipv4);
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
            let known_peers: Vec<crate::model::PeerBrief> = app
                .users
                .iter()
                .map(|u| crate::model::PeerBrief {
                    id: u.id.clone(),
                    ip: u.ip.clone(),
                    name: Some(u.name.clone()),
                })
                .collect();
            spawn_network_worker(peer_tx, cmd_rx, initial_name, known_peers);
            app.peer_rx = Some(peer_rx);
            app.net_cmd_tx = Some(cmd_tx.clone());
            // start metadata watcher with ability to send NetCmd for background sync
            crate::metadata::MetadataStore::start_watcher_with_sender(Some(cmd_tx));

            Ok(Box::new(app))
        }),
    )
}

#[derive(Clone, Debug)]
struct SyncTransfer {
    peer_id: String,
    path: PathBuf,
    is_dir: bool,
}

// Simple helper to represent metadata in UI list
#[derive(Clone, Debug)]
struct MetadataView {
    id: String,
    peer_id: Option<String>,
    peer_ip: Option<String>,
    filename: String,
    abs_path: String,
    size: u64,
    is_dir: bool,
    status: String,
    auto_sync_enabled: bool,
}

impl From<&crate::metadata::Metadata> for MetadataView {
    fn from(m: &crate::metadata::Metadata) -> Self {
        MetadataView {
            id: m.id.clone(),
            peer_id: m.peer_id.clone(),
            peer_ip: m.peer_ip.clone(),
            filename: m.filename.clone(),
            abs_path: m.abs_path.clone(),
            size: m.size,
            is_dir: m.is_dir,
            status: match m.sync_status {
                crate::model::SyncStatus::ReadyToSend => "准备发送".to_string(),
                crate::model::SyncStatus::Sending => "正在发送".to_string(),
                crate::model::SyncStatus::Sent => "发送完成".to_string(),
                crate::model::SyncStatus::FileChanged => "文件变化".to_string(),
                crate::model::SyncStatus::Syncing => "正在同步".to_string(),
                crate::model::SyncStatus::Synced => "同步完成".to_string(),
            },
            auto_sync_enabled: m.auto_sync_enabled,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NameSource {
    Direct,
    Indirect,
}

#[derive(Clone, Debug)]
struct SyncScanResult {
    tree: SyncTree,
    changes: Vec<SyncTransfer>,
}

type PendingHistoryLog = (String, String, String, Option<String>);
type PendingHistorySync = (String, Option<String>, String, bool);
type PendingHistoryPathUpdate = (String, Option<String>, String);
type PendingHistoryNeedsSyncUpdate = (String, Option<String>, bool);

#[derive(Default)]
struct FileProgressHistoryEffects {
    pending_log: Option<PendingHistoryLog>,
    pending_sync: Option<PendingHistorySync>,
    pending_file_done: Option<PendingHistorySync>,
    pending_path_update: Option<PendingHistoryPathUpdate>,
    pending_needs_sync_update: Option<PendingHistoryNeedsSyncUpdate>,
}

#[derive(Default)]
pub struct RustleApp {
    // metadata ui cache
    meta_list: Vec<MetadataView>,
    pub self_id: String,
    pub users: Vec<User>,
    pub selected_user_id: Option<String>,
    pub messages: HashMap<String, Vec<ChatMessage>>,
    pub input: String,
    pub delivery: DeliveryState,

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
    // 重试发送的待处理列表：peer_id -> due instant
    pub pending_resend: std::collections::HashMap<String, Instant>,

    // 自动同步相关
    pub sync_tree: SyncTree,
    pub sync_dirty: bool,
    pub last_sync_scan: Option<Instant>,
    sync_scan_rx: Option<Receiver<SyncScanResult>>,
    pub sync_scan_in_progress: bool,
    offline_sync: HashMap<String, Vec<SyncTransfer>>,

    // 发现帧 peers 列表推送节流
    pub last_peerlist_push: Option<Instant>,

    // 离线用户名更新队列
    pub offline_name_updates: HashMap<String, String>,

    // 用户名来源跟踪（Direct 优先）
    name_source: HashMap<String, NameSource>,

    // 本机绑定信息（UI 使用）
    pub local_ip: Option<String>,
    pub local_port: Option<u16>,
    // 已成功绑定的接口列表（用于验证显示的本地地址是否可用）
    pub bound_interfaces: HashSet<String>,

    // 已接收消息 ID 缓存（用于去重）
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
    pub show_settings_window: bool,
    // 同步管理窗口
    pub show_sync_window: bool,

    // 设置
    pub settings: AppSettings,
    pub settings_recv_dir_input: String,
    pub settings_name_input: String,

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
    fn persist_runtime_state(&self) {
        self.delivery.persist_runtime_state();
    }

    fn restore_runtime_state(&mut self) {
        self.delivery.restore_runtime_state(&mut self.messages);
        self.persist_runtime_state();
    }

    fn peer_supports_reliable_folders(&self, peer_id: &str) -> bool {
        self.users
            .iter()
            .find(|u| u.id == peer_id)
            .map(|u| u.supports_reliable_folders)
            .unwrap_or(false)
    }

    fn peer_supports_transfer_id(&self, peer_id: &str) -> bool {
        self.users
            .iter()
            .find(|u| u.id == peer_id)
            .and_then(|u| u.protocol_version.as_deref())
            .map(|version| version >= crate::model::APP_PROTOCOL_VERSION)
            .unwrap_or(false)
    }

    fn message_matches_transfer(
        message: &ChatMessage,
        from_me: bool,
        transfer_id: Option<&str>,
        file_name: &str,
    ) -> bool {
        if message.from_me != from_me {
            return false;
        }
        if let Some(transfer_id) = transfer_id {
            return message.transfer_id.as_deref() == Some(transfer_id);
        }
        message
            .file_path
            .as_ref()
            .map(|p| p.ends_with(file_name))
            .unwrap_or(false)
            || (!from_me && message.text.contains(file_name))
    }

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
        if let Some(pref) = self.settings.preferred_interface.as_ref() {
            if self.bound_interfaces.contains(pref) {
                return Some(pref.clone());
            }
        }
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

    fn update_outgoing_msg_id(&mut self, peer_id: &str, new_id: &str, text: &str) {
        if let Some(msgs) = self.messages.get_mut(peer_id) {
            let mut target = msgs
                .iter_mut()
                .rev()
                .find(|m| m.from_me && m.msg_id.as_deref() == Some(new_id));
            if target.is_none() {
                target = msgs.iter_mut().rev().find(|m| m.from_me && m.text == text);
            }
            if let Some(m) = target {
                m.msg_id = Some(new_id.to_string());
                if m.transfer_status.is_none()
                    || m.transfer_status.as_deref() == Some("未送达")
                    || m.transfer_status.as_deref() == Some("等待对方上线...")
                {
                    m.transfer_status = Some("发送中...".to_string());
                    m.is_pending = false;
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
        sync_ts: Option<&str>,
        transfer_id: Option<&str>,
        msg_id: Option<&str>,
        is_pending: bool,
        needs_sync: bool,
    ) {
        history::log_history(
            peer_id,
            from_me,
            text,
            send_ts,
            recv_ts,
            file_path,
            sync_ts,
            transfer_id,
            msg_id,
            is_pending,
            needs_sync,
        );
    }

    fn clear_history(&self, peer_id: &str) {
        history::clear_history(peer_id);
    }

    fn load_recent_history(&mut self) {
        for loaded in history::load_recent_history(self.settings.history_days) {
            let pid = loaded.peer_id;
            if !self.users.iter().any(|u| u.id == pid) {
                self.users.push(User {
                    id: pid.clone(),
                    name: pid.clone(),
                    online: false,
                    ip: None,
                    port: None,
                    tcp_port: None,
                    protocol_version: None,
                    supports_reliable_folders: false,
                    bound_interface: None,
                    best_interface: None,
                    has_unread: false,
                });
                self.delivery.offline_msgs.entry(pid.clone()).or_default();
            }

            self.messages.entry(pid).or_default().push(loaded.message);
        }
    }

    fn load_sync_tree(&mut self) {
        self.sync_tree = load_sync_tree();
        self.sync_dirty = false;
        if self.last_sync_scan.is_none() {
            self.last_sync_scan = Some(Instant::now());
        }
    }

    fn persist_sync_tree(&mut self) {
        if self.sync_dirty {
            save_sync_tree(&self.sync_tree);
            self.sync_dirty = false;
        }
    }

    fn push_peer_list_to_net(&mut self) {
        let Some(tx) = self.net_cmd_tx.clone() else {
            return;
        };
        let peers: Vec<crate::model::PeerSnapshot> = self
            .users
            .iter()
            .map(|u| crate::model::PeerSnapshot {
                id: u.id.clone(),
                ip: u.ip.clone(),
                online: u.online,
                name: Some(u.name.clone()),
            })
            .collect();
        let online_count = self.users.iter().filter(|u| u.online).count();
        let _ = tx.send(NetCmd::UpdatePeerList {
            peers,
            online_count,
        });
    }

    fn send_name_update_to_all(&mut self, name: &str) {
        let Some(tx) = self.net_cmd_tx.clone() else {
            return;
        };
        let local_ip = self.local_ip.clone();
        for u in &self.users {
            if u.id == self.self_id {
                continue;
            }
            if u.online {
                if let Some(ip) = &u.ip {
                    let via = self.get_best_interface_for_peer(ip);
                    let _ = tx.send(NetCmd::SendNameUpdate {
                        ip: ip.clone(),
                        via,
                        name: name.to_string(),
                        local_ip: local_ip.clone(),
                    });
                }
            } else {
                self.offline_name_updates
                    .insert(u.id.clone(), name.to_string());
            }
        }
    }

    fn build_sync_node(path: &PathBuf) -> Option<SyncNode> {
        #[derive(Clone)]
        struct NodeEntry {
            name: String,
            path: String,
            is_dir: bool,
            mtime: Option<i64>,
            sha256: Option<String>,
            children: Vec<usize>,
        }

        let meta = std::fs::metadata(path).ok()?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("item")
            .to_string();
        if meta.is_file() {
            let mtime = file_mtime_seconds(path)?;
            let sha = sha256_file(path)?;
            return Some(SyncNode {
                name,
                path: path.to_string_lossy().to_string(),
                is_dir: false,
                mtime: Some(mtime),
                sha256: Some(sha),
                children: Vec::new(),
            });
        }
        if !meta.is_dir() {
            return None;
        }

        let root_path = path.to_string_lossy().to_string();
        let mut entries: Vec<NodeEntry> = Vec::new();
        entries.push(NodeEntry {
            name,
            path: root_path.clone(),
            is_dir: true,
            mtime: None,
            sha256: None,
            children: Vec::new(),
        });

        let mut stack: Vec<(PathBuf, usize)> = vec![(path.clone(), 0)];
        while let Some((dir_path, parent_idx)) = stack.pop() {
            let Ok(read_dir) = std::fs::read_dir(&dir_path) else {
                continue;
            };
            for entry in read_dir.flatten() {
                let child_path = entry.path();
                let child_meta = match std::fs::metadata(&child_path) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let child_name = child_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("item")
                    .to_string();
                let child_idx = entries.len();
                if child_meta.is_file() {
                    let mtime = file_mtime_seconds(&child_path);
                    let sha = sha256_file(&child_path);
                    entries.push(NodeEntry {
                        name: child_name,
                        path: child_path.to_string_lossy().to_string(),
                        is_dir: false,
                        mtime,
                        sha256: sha,
                        children: Vec::new(),
                    });
                } else if child_meta.is_dir() {
                    entries.push(NodeEntry {
                        name: child_name,
                        path: child_path.to_string_lossy().to_string(),
                        is_dir: true,
                        mtime: None,
                        sha256: None,
                        children: Vec::new(),
                    });
                    stack.push((child_path, child_idx));
                } else {
                    continue;
                }
                if let Some(parent) = entries.get_mut(parent_idx) {
                    parent.children.push(child_idx);
                }
            }
        }

        let mut built: Vec<Option<SyncNode>> = vec![None; entries.len()];
        let mut stack: Vec<(usize, bool)> = vec![(0, false)];
        while let Some((idx, visited)) = stack.pop() {
            if !visited {
                stack.push((idx, true));
                let children = entries[idx].children.clone();
                for c in children {
                    stack.push((c, false));
                }
            } else {
                let entry = entries[idx].clone();
                let mut children_nodes = Vec::new();
                for c in entry.children {
                    if let Some(child_node) = built[c].take() {
                        children_nodes.push(child_node);
                    }
                }
                built[idx] = Some(SyncNode {
                    name: entry.name,
                    path: entry.path,
                    is_dir: entry.is_dir,
                    mtime: entry.mtime,
                    sha256: entry.sha256,
                    children: children_nodes,
                });
            }
        }

        built[0].take()
    }

    fn track_sync_source(&mut self, peer_id: &str, path: &PathBuf) {
        let Some(node) = Self::build_sync_node(path) else {
            return;
        };
        let list = self.sync_tree.peers.entry(peer_id.to_string()).or_default();
        if let Some(existing) = list.iter_mut().find(|n| n.path == node.path) {
            *existing = node;
        } else {
            list.push(node);
        }
        self.sync_dirty = true;
    }

    fn update_history_sync(
        &self,
        peer_id: &str,
        file_path: &str,
        transfer_id: Option<&str>,
        sync_ts: &str,
        from_me: bool,
    ) {
        history::update_history_sync(peer_id, file_path, transfer_id, sync_ts, from_me);
    }

    fn update_history_ack(&self, peer_id: &str, msg_id: &str, recv_ts: &str) {
        history::update_history_ack(peer_id, msg_id, recv_ts);
    }

    fn update_history_file_done(
        &self,
        peer_id: &str,
        file_path: &str,
        transfer_id: Option<&str>,
        recv_ts: &str,
        from_me: bool,
    ) {
        history::update_history_file_done(peer_id, file_path, transfer_id, recv_ts, from_me);
    }

    fn update_history_needs_sync(
        &self,
        peer_id: &str,
        file_path: &str,
        transfer_id: Option<&str>,
        needs_sync: bool,
        from_me: bool,
    ) {
        history::update_history_needs_sync(peer_id, file_path, transfer_id, needs_sync, from_me);
    }

    fn update_history_file_path(
        &self,
        peer_id: &str,
        file_name: &str,
        transfer_id: Option<&str>,
        new_path: &str,
        from_me: bool,
    ) {
        history::update_history_file_path(peer_id, file_name, transfer_id, new_path, from_me);
    }

    fn update_history_pending(&self, peer_id: &str, msg_id: &str, is_pending: bool) {
        history::update_history_pending(peer_id, msg_id, is_pending);
    }

    fn scan_node_for_changes(node: &mut SyncNode) -> bool {
        if node.is_dir {
            let mut changed = false;
            for child in &mut node.children {
                if Self::scan_node_for_changes(child) {
                    changed = true;
                }
            }
            return changed;
        }

        let path = PathBuf::from(&node.path);
        let Some(mtime) = file_mtime_seconds(&path) else {
            return false;
        };
        if node.mtime != Some(mtime) {
            let new_sha = sha256_file(&path);
            node.mtime = Some(mtime);
            if let Some(sha) = new_sha {
                let changed = node.sha256.as_deref() != Some(&sha);
                node.sha256 = Some(sha);
                return changed;
            }
        }
        false
    }

    fn scan_sync_tree(mut tree: SyncTree) -> SyncScanResult {
        let mut changes = Vec::new();
        for (peer_id, nodes) in tree.peers.iter_mut() {
            for node in nodes.iter_mut() {
                let changed = Self::scan_node_for_changes(node);
                if changed {
                    changes.push(SyncTransfer {
                        peer_id: peer_id.clone(),
                        path: PathBuf::from(&node.path),
                        is_dir: node.is_dir,
                    });
                }
            }
        }
        SyncScanResult { tree, changes }
    }

    fn maybe_start_sync_scan(&mut self) {
        if self.sync_scan_in_progress {
            return;
        }
        let elapsed_ok = self
            .last_sync_scan
            .map(|t| t.elapsed() >= Duration::from_secs(5 * 60))
            .unwrap_or(true);
        if !elapsed_ok {
            return;
        }

        let tree = self.sync_tree.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = RustleApp::scan_sync_tree(tree);
            let _ = tx.send(result);
        });
        self.sync_scan_rx = Some(rx);
        self.sync_scan_in_progress = true;
        self.last_sync_scan = Some(Instant::now());
    }

    fn handle_sync_scan_result(&mut self) {
        if let Some(rx) = &self.sync_scan_rx {
            if let Ok(result) = rx.try_recv() {
                self.sync_tree = result.tree;
                self.sync_dirty = true;
                self.sync_scan_in_progress = false;
                self.sync_scan_rx = None;

                for change in result.changes {
                    let peer_id = change.peer_id.clone();
                    let (ip, online) = self
                        .users
                        .iter()
                        .find(|u| u.id == peer_id)
                        .map(|u| (u.ip.clone(), u.online))
                        .unwrap_or((None, false));
                    if online {
                        if let Some(ip) = ip {
                            if change.is_dir && !self.peer_supports_reliable_folders(&peer_id) {
                                self.offline_sync
                                    .entry(peer_id.clone())
                                    .or_default()
                                    .push(change);
                                continue;
                            }
                            if let Some(tx) = &self.net_cmd_tx {
                                let target_tcp_port = if change.is_dir {
                                    TCP_DIR_PORT
                                } else {
                                    TCP_FILE_PORT
                                };
                                let via = self.get_best_interface_for_peer(&ip);
                                if tx
                                    .send(NetCmd::SendFile {
                                        peer_id: peer_id.clone(),
                                        ip: ip.clone(),
                                        tcp_port: target_tcp_port,
                                        path: change.path.clone(),
                                        transfer_id: None,
                                        supports_transfer_id: true,
                                        is_dir: change.is_dir,
                                        via,
                                        is_sync: true,
                                    })
                                    .is_err()
                                {
                                    self.offline_sync
                                        .entry(peer_id.clone())
                                        .or_default()
                                        .push(change);
                                }
                            }
                        } else {
                            self.offline_sync
                                .entry(peer_id.clone())
                                .or_default()
                                .push(change);
                        }
                    } else {
                        self.offline_sync
                            .entry(peer_id.clone())
                            .or_default()
                            .push(change);
                    }
                }
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
                        .or_else(|| {
                            kp.ip
                                .as_ref()
                                .and_then(|ip| kp.port.map(|p| format!("{}:{}", ip, p)))
                        })
                        .unwrap_or_else(|| kp.id.clone());

                    if !self.users.iter().any(|u| u.id == kp.id) {
                        self.users.push(User {
                            id: kp.id.clone(),
                            name: display,
                            online: false,
                            ip: kp.ip.clone(),
                            port: None,
                            tcp_port: None,
                            protocol_version: None,
                            supports_reliable_folders: false,
                            bound_interface: kp.bound_interface.clone(),
                            best_interface: kp.bound_interface.clone(),
                            has_unread: false,
                        });
                        self.messages.entry(kp.id.clone()).or_default();
                        self.delivery.offline_msgs.entry(kp.id.clone()).or_default();
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
                bound_interface: u
                    .best_interface
                    .clone()
                    .or_else(|| u.bound_interface.clone()),
            })
            .collect();
        if let Ok(text) = serde_json::to_string_pretty(&list) {
            let _ = fs::write(data_path(KNOWN_PEERS_FILE), text);
        }
        self.known_dirty = false;
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

    fn pick_and_send(&mut self, pick_folder: bool) {
        use std::sync::mpsc;

        let (tx, rx) = mpsc::channel();

        std::thread::spawn(move || {
            let selection = if pick_folder {
                FileDialog::new().pick_folder()
            } else {
                FileDialog::new().pick_file()
            };
            let _ = tx.send(selection);
        });

        if let Ok(Some(path)) = rx.recv() {
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
        #[cfg(target_os = "windows")]
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
        let output: std::io::Result<std::process::Output> = Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Not supported",
        ));

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

        self.render_settings_dialog(ctx);
        self.render_usage_dialog(ctx);
        self.render_about_dialog(ctx);
        self.render_update_dialog(ctx);

        self.render_sync_manager_dialog(ctx);

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

                if ui.button("设置").clicked() {
                    self.settings_recv_dir_input =
                        self.settings.recv_dir.clone().unwrap_or_else(|| {
                            crate::storage::default_download_dir()
                                .to_string_lossy()
                                .to_string()
                        });
                    self.settings_name_input = self.me_name.clone().unwrap_or_default();
                    self.show_settings_window = true;
                }

                if ui.button("同步管理").clicked() {
                    self.show_sync_window = true;
                }

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
                        self.handle_discovered(peer, local_ip);
                    }
                    PeerEvent::ChatReceived {
                        from_id,
                        from_ip,
                        from_port,
                        from_name,
                        text,
                        send_ts,
                        recv_ts,
                        msg_id,
                        local_ip,
                    } => {
                        self.handle_chat_received(
                            ctx, from_id, from_ip, from_port, from_name, text, send_ts, recv_ts,
                            msg_id, local_ip,
                        );
                    }
                    PeerEvent::ChatAck { from_id, msg_id } => {
                        self.handle_chat_ack(from_id, msg_id);
                    }
                    PeerEvent::FileCompletionAck {
                        from_id,
                        transfer_id,
                        file_name,
                        is_dir: _,
                        is_sync,
                        succeeded,
                        status,
                    } => {
                        self.handle_file_completion_ack_event(
                            &from_id,
                            transfer_id.as_deref(),
                            &file_name,
                            is_sync,
                            succeeded,
                            &status,
                        );
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
                        transfer_id,
                        file_name,
                        progress,
                        status,
                        is_incoming,
                        is_dir,
                        local_path,
                        is_sync,
                        is_final,
                        succeeded,
                    } => {
                        if let Some(pid) = peer_id {
                            self.handle_file_progress_event(
                                &pid,
                                transfer_id,
                                file_name,
                                progress,
                                status,
                                is_incoming,
                                is_dir,
                                local_path,
                                is_sync,
                                is_final,
                                succeeded,
                            );
                        }
                    }
                    PeerEvent::DiscoverReceived {
                        from_id,
                        from_ip,
                        from_name,
                        peers,
                    } => {
                        self.handle_discover_received(from_id, from_ip, from_name, peers);
                    }
                    PeerEvent::PeerOnline { id, ip } => {
                        if id == self.self_id {
                            continue;
                        }
                        self.handle_peer_online(id, ip);
                    }
                    PeerEvent::NameUpdate { id, name, ip } => {
                        if id == self.self_id {
                            continue;
                        }
                        self.handle_name_update(id, name, ip);
                    }
                    PeerEvent::PeerOffline { id } => {
                        self.mark_offline(&id);
                    }
                }
            }
            self.merge_users_by_id();
        }

        // 处理自动同步扫描与持久化
        if self
            .last_peerlist_push
            .map(|t| t.elapsed() >= Duration::from_secs(1))
            .unwrap_or(true)
        {
            self.push_peer_list_to_net();
            self.last_peerlist_push = Some(Instant::now());
        }
        self.handle_sync_scan_result();
        self.maybe_start_sync_scan();
        self.persist_sync_tree();

        self.render_contacts_panel(ctx);

        // 处理删除联系人
        if let Some(id_to_delete) = self.context_menu_user_id.take() {
            if let Some(index) = self.users.iter().position(|u| u.id == id_to_delete) {
                self.users.remove(index);
                self.messages.remove(&id_to_delete);
                self.delivery.offline_msgs.remove(&id_to_delete);
                self.delivery.pending_acks.remove(&id_to_delete);
                self.peers.remove(&id_to_delete);

                self.logged_incoming_files
                    .retain(|(pid, _)| pid != &id_to_delete);
                self.message_visible_since
                    .retain(|(pid, _), _| pid != &id_to_delete);

                self.clear_history(&id_to_delete);

                self.known_dirty = true;
                if self.selected_user_id.as_deref() == Some(&id_to_delete) {
                    self.selected_user_id = None;
                }
                self.persist_runtime_state();
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

        self.render_messages_panel(ctx);

        self.render_name_dialogs(ctx);

        if !dropped_files.is_empty() {
            self.handle_dropped_files(dropped_files);
        }

        self.process_startup_probe();
        self.process_pending_ack_timeouts();
        self.process_scheduled_resends();

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
        let now_instant = Instant::now();
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
            self.message_visible_since
                .remove(&(peer_id.clone(), *msg_idx));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_private_ipv4_cases() {
        assert!(RustleApp::is_private_ipv4("10.0.0.1"));
        assert!(RustleApp::is_private_ipv4("172.16.0.1"));
        assert!(RustleApp::is_private_ipv4("192.168.1.1"));
        assert!(!RustleApp::is_private_ipv4("8.8.8.8"));
        assert!(!RustleApp::is_private_ipv4("invalid"));
    }

    #[test]
    fn same_lan_cases() {
        assert!(RustleApp::same_lan("192.168.1.100", "192.168.1.5"));
        assert!(!RustleApp::same_lan("192.168.1.100", "192.168.2.5"));
        assert!(RustleApp::same_lan("10.0.0.1", "10.0.0.2"));
        assert!(!RustleApp::same_lan("10.0.0.1", "11.0.0.1"));
    }

    #[test]
    fn get_best_interface_prefers_setting() {
        let mut app = RustleApp::default();
        app.bound_interfaces.insert("1.2.3.4".to_string());
        app.settings.preferred_interface = Some("1.2.3.4".to_string());
        assert_eq!(
            app.get_best_interface_for_peer("9.8.7.6"),
            Some("1.2.3.4".to_string())
        );
    }

    #[test]
    fn get_best_interface_same_lan_fallback() {
        let mut app = RustleApp::default();
        app.bound_interfaces.insert("192.168.1.10".to_string());
        assert_eq!(
            app.get_best_interface_for_peer("192.168.1.20"),
            Some("192.168.1.10".to_string())
        );
    }

    #[test]
    fn message_matches_transfer_prefers_transfer_id_over_filename() {
        let exact = ChatMessage {
            from_me: true,
            text: "📄 file.txt".to_string(),
            send_ts: "ts".to_string(),
            recv_ts: None,
            last_sync_ts: None,
            file_path: Some("C:/a/file.txt".to_string()),
            transfer_id: Some("transfer-a".to_string()),
            transfer_status: None,
            msg_id: None,
            is_read: true,
            is_pending: false,
            needs_sync: false,
        };
        let same_name_other_transfer = ChatMessage {
            from_me: true,
            text: "📄 file.txt".to_string(),
            send_ts: "ts".to_string(),
            recv_ts: None,
            last_sync_ts: None,
            file_path: Some("C:/b/file.txt".to_string()),
            transfer_id: Some("transfer-b".to_string()),
            transfer_status: None,
            msg_id: None,
            is_read: true,
            is_pending: false,
            needs_sync: false,
        };

        assert!(RustleApp::message_matches_transfer(
            &exact,
            true,
            Some("transfer-a"),
            "file.txt",
        ));
        assert!(!RustleApp::message_matches_transfer(
            &same_name_other_transfer,
            true,
            Some("transfer-a"),
            "file.txt",
        ));
        assert!(RustleApp::message_matches_transfer(
            &same_name_other_transfer,
            true,
            None,
            "file.txt",
        ));
    }
}

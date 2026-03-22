use super::RustleApp;
use crate::debug_println;
use crate::model::{ChatMessage, NetCmd, QueuedMsg, TCP_DIR_PORT, TCP_FILE_PORT};
use chrono::Local;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use uuid::Uuid;

impl RustleApp {
    pub(super) fn flush_offline_queue(
        &mut self,
        peer_id: &str,
        ip: Option<&str>,
        port: Option<u16>,
    ) {
        let Some(ip) = ip else { return };
        let Some(port) = port else { return };
        let Some(tx) = self.net_cmd_tx.clone() else {
            return;
        };
        if let Some(queue) = self.delivery.offline_msgs.get_mut(peer_id) {
            if queue.is_empty() {
                return;
            }
            let drained: Vec<QueuedMsg> = queue.drain(..).collect();
            let mut remain = Vec::new();
            let via = self.get_best_interface_for_peer(ip);

            for msg in drained {
                if let Some(path) = &msg.file_path {
                    if msg.is_dir && !self.peer_supports_reliable_folders(peer_id) {
                        remain.push(msg);
                        continue;
                    }
                    let target_tcp_port = if msg.is_dir {
                        TCP_DIR_PORT
                    } else {
                        TCP_FILE_PORT
                    };
                    if tx
                        .send(NetCmd::SendFile {
                            peer_id: peer_id.to_string(),
                            ip: ip.to_string(),
                            tcp_port: target_tcp_port,
                            path: path.clone(),
                            transfer_id: msg.transfer_id.clone(),
                            supports_transfer_id: self.peer_supports_transfer_id(peer_id),
                            is_dir: msg.is_dir,
                            via: via.clone(),
                            is_sync: false,
                        })
                        .is_err()
                    {
                        remain.push(msg);
                    }
                } else {
                    let mid = msg
                        .msg_id
                        .clone()
                        .unwrap_or_else(|| Uuid::new_v4().to_string());
                    debug_println!(
                        "Flushing offline queue for {} mid={} text={}",
                        peer_id,
                        mid,
                        msg.text
                    );
                    self.update_outgoing_msg_id(peer_id, &mid, &msg.text);
                    self.try_send_message_with_retry(
                        peer_id,
                        ip,
                        port,
                        &msg.text,
                        &msg.send_ts,
                        &mid,
                        via.clone(),
                    );

                    let sent = self
                        .delivery
                        .pending_acks
                        .get(peer_id)
                        .map(|list| list.iter().any(|(mid_item, _)| mid_item == &mid))
                        .unwrap_or(false);
                    debug_println!(
                        "Flush result for {} mid={} sent={} pending_acks_count={}",
                        peer_id,
                        mid,
                        sent,
                        self.delivery
                            .pending_acks
                            .get(peer_id)
                            .map(|list| list.len())
                            .unwrap_or(0)
                    );
                    self.update_history_pending(peer_id, &mid, !sent);
                }
            }
            if !remain.is_empty() {
                self.delivery
                    .offline_msgs
                    .insert(peer_id.to_string(), remain);
            }
        }
        self.persist_runtime_state();
    }

    pub(super) fn send_message_internal(&mut self, id: &str, text: String) {
        let ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let msg_id = Uuid::new_v4().to_string();

        self.messages
            .entry(id.to_string())
            .or_default()
            .push(ChatMessage {
                from_me: true,
                text: text.clone(),
                send_ts: ts.clone(),
                recv_ts: None,
                last_sync_ts: None,
                file_path: None,
                transfer_id: None,
                transfer_status: Some("发送中...".to_string()),
                msg_id: Some(msg_id.clone()),
                is_read: true,
                is_pending: false,
                needs_sync: false,
            });

        let Some(user) = self.users.iter().find(|user| user.id == id) else {
            return;
        };
        let online = user.online;
        let ip = user.ip.clone();
        let port = user.port;
        let via = ip
            .as_ref()
            .map(|target_ip| self.get_best_interface_for_peer(target_ip))
            .unwrap_or(None);
        let has_addr = ip.is_some() && port.is_some();

        if online && has_addr {
            if let (Some(ip), Some(port)) = (ip.as_deref(), port) {
                self.try_send_message_with_retry(id, ip, port, &text, &ts, &msg_id, via);
            }
            self.log_history(
                id,
                true,
                &text,
                &ts,
                None,
                None,
                None,
                None,
                Some(&msg_id),
                false,
                false,
            );
        } else {
            self.delivery
                .offline_msgs
                .entry(id.to_string())
                .or_default()
                .push(QueuedMsg {
                    text: text.to_string(),
                    send_ts: ts.clone(),
                    msg_id: Some(msg_id.clone()),
                    file_path: None,
                    transfer_id: None,
                    is_dir: false,
                });

            if has_addr {
                if let (Some(ip), Some(port)) = (ip.as_deref(), port) {
                    if let Some(local) = self.local_ip.as_deref() {
                        if Self::same_lan(local, ip) {
                            self.try_send_message_with_retry(
                                id, ip, port, &text, &ts, &msg_id, via,
                            );
                        }
                    }
                }
            } else if let Some(msgs) = self.messages.get_mut(id) {
                if let Some(message) = msgs
                    .iter_mut()
                    .rev()
                    .find(|message| message.msg_id.as_deref() == Some(&msg_id))
                {
                    message.transfer_status = Some("等待对方上线...".to_string());
                    message.is_pending = true;
                }
            }

            self.log_history(
                id,
                true,
                &text,
                &ts,
                None,
                None,
                None,
                None,
                Some(&msg_id),
                true,
                false,
            );
        }

        self.persist_runtime_state();
    }

    pub(super) fn try_send_message_with_retry(
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

        let mut mark_pending_ack = |peer_id: &str, msg_id: &str| {
            let deadline = Instant::now() + Duration::from_secs(5);
            let list = self
                .delivery
                .pending_acks
                .entry(peer_id.to_string())
                .or_default();
            if let Some((_, existing_deadline)) = list.iter_mut().find(|(mid, _)| mid == msg_id) {
                *existing_deadline = deadline;
            } else {
                list.push((msg_id.to_string(), deadline));
            }
        };

        if let Some(via) = preferred_via {
            if tx
                .send(NetCmd::SendChat {
                    ip: ip.to_string(),
                    text: text.to_string(),
                    ts: ts.to_string(),
                    via: Some(via),
                    msg_id: msg_id.to_string(),
                    local_ip: self.local_ip.clone(),
                })
                .is_ok()
            {
                mark_pending_ack(peer_id, msg_id);
                return;
            }
        }

        let mut sent = false;
        let bound_list: Vec<String> = self.bound_interfaces.iter().cloned().collect();

        for bound_ip in &bound_list {
            if Self::same_lan(bound_ip, ip) {
                if tx
                    .send(NetCmd::SendChat {
                        ip: ip.to_string(),
                        text: text.to_string(),
                        ts: ts.to_string(),
                        via: Some(bound_ip.clone()),
                        msg_id: msg_id.to_string(),
                        local_ip: self.local_ip.clone(),
                    })
                    .is_ok()
                {
                    sent = true;
                    break;
                }
            }
        }

        if !sent {
            eprintln!("[发送] 未找到同网段接口 -> {}, 尝试所有可用接口", ip);
            for bound_ip in &bound_list {
                if !Self::same_lan(bound_ip, ip) {
                    if tx
                        .send(NetCmd::SendChat {
                            ip: ip.to_string(),
                            text: text.to_string(),
                            ts: ts.to_string(),
                            via: Some(bound_ip.clone()),
                            msg_id: msg_id.to_string(),
                            local_ip: self.local_ip.clone(),
                        })
                        .is_ok()
                    {
                        eprintln!("[发送] 使用跨网段接口 {} -> {} 发送成功", bound_ip, ip);
                        sent = true;
                        break;
                    }
                }
            }
        }

        if sent {
            mark_pending_ack(peer_id, msg_id);
        } else {
            eprintln!(
                "[发送失败] 无可用接口发送到 {} (绑定接口数: {})",
                ip,
                self.bound_interfaces.len()
            );
            if let Some(msgs) = self.messages.get_mut(peer_id) {
                if let Some(message) = msgs
                    .iter_mut()
                    .rev()
                    .find(|message| message.msg_id.as_deref() == Some(msg_id))
                {
                    message.transfer_status = Some("未送达".to_string());
                    message.is_pending = true;
                }
            }
        }

        self.persist_runtime_state();
    }

    pub(super) fn append_file_message(&mut self, path: &PathBuf, is_dir: bool) {
        if let Some(id) = self.selected_user_id.clone() {
            self.scroll_to_bottom = true;
            let (ip, via, online) = {
                let Some(user) = self.users.iter().find(|user| user.id == id) else {
                    return;
                };
                let via = if let Some(target_ip) = &user.ip {
                    self.get_best_interface_for_peer(target_ip)
                } else {
                    None
                };
                (user.ip.clone(), via, user.online)
            };

            let target_tcp_port = if is_dir { TCP_DIR_PORT } else { TCP_FILE_PORT };

            if is_dir {
                let supports_reliable = self
                    .users
                    .iter()
                    .find(|user| user.id == id)
                    .map(|user| user.supports_reliable_folders)
                    .unwrap_or(false);
                if !supports_reliable {
                    let text = format!(
                        "📁 {}",
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("item")
                    );
                    let ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                    let msgs = self.messages.entry(id.clone()).or_default();
                    msgs.push(ChatMessage {
                        from_me: true,
                        text: text.clone(),
                        send_ts: ts.clone(),
                        recv_ts: None,
                        last_sync_ts: None,
                        file_path: Some(path.to_string_lossy().to_string()),
                        transfer_id: None,
                        transfer_status: Some("发送失败：对方版本不支持可靠目录传输".to_string()),
                        msg_id: None,
                        is_read: true,
                        is_pending: false,
                        needs_sync: false,
                    });
                    self.log_history(
                        &id,
                        true,
                        &text,
                        &ts,
                        None,
                        Some(&path.to_string_lossy()),
                        None,
                        None,
                        None,
                        false,
                        false,
                    );
                    return;
                }
            }

            let icon = if is_dir { "📁" } else { "📄" };
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("item");

            self.track_sync_source(&id, path);

            let text = format!("{} {}", icon, name);
            let ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let transfer_id = Some(Uuid::new_v4().to_string());

            let mut sent = false;
            if online {
                if let Some(ip) = ip {
                    if let Some(tx) = &self.net_cmd_tx {
                        let supports_transfer_id = self.peer_supports_transfer_id(&id);
                        if tx
                            .send(NetCmd::SendFile {
                                peer_id: id.clone(),
                                ip: ip.clone(),
                                tcp_port: target_tcp_port,
                                path: path.clone(),
                                transfer_id: if supports_transfer_id {
                                    transfer_id.clone()
                                } else {
                                    None
                                },
                                supports_transfer_id,
                                is_dir,
                                via: via.clone(),
                                is_sync: false,
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
                self.delivery
                    .offline_msgs
                    .entry(id.clone())
                    .or_default()
                    .push(QueuedMsg {
                        text: text.clone(),
                        send_ts: ts.clone(),
                        msg_id: None,
                        file_path: Some(path.clone()),
                        transfer_id: transfer_id.clone(),
                        is_dir,
                    });
            }

            let msgs = self.messages.entry(id.clone()).or_default();
            let needs_sync = self.settings.auto_sync_on_send;
            msgs.push(ChatMessage {
                from_me: true,
                text: text.clone(),
                send_ts: ts.clone(),
                recv_ts: None,
                last_sync_ts: None,
                file_path: Some(path.to_string_lossy().to_string()),
                transfer_id: transfer_id.clone(),
                transfer_status: Some(if sent {
                    "发送中...".to_string()
                } else {
                    "等待对方上线...".to_string()
                }),
                msg_id: None,
                is_read: true,
                is_pending: !sent,
                needs_sync,
            });

            self.log_history(
                &id,
                true,
                &text,
                &ts,
                None,
                Some(&path.to_string_lossy()),
                None,
                transfer_id.as_deref(),
                None,
                !sent,
                needs_sync,
            );
        }
    }
}

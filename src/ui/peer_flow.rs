use super::{NameSource, RustleApp};
use crate::debug_println;
use crate::model::{DiscoveredPeer, NetCmd, Peer, PeerBrief, User, UDP_MESSAGE_PORT};
use chrono::Local;
use eframe::egui;
use std::collections::HashMap;
use std::time::{Duration, Instant};

impl RustleApp {
    fn ensure_user_stub(&mut self, id: &str, name: String, online: bool, ip: Option<String>) {
        if self.users.iter().any(|user| user.id == id) {
            return;
        }

        self.users.push(User {
            id: id.to_string(),
            name,
            online,
            ip,
            port: Some(UDP_MESSAGE_PORT),
            tcp_port: None,
            protocol_version: None,
            supports_reliable_folders: false,
            bound_interface: None,
            best_interface: None,
            has_unread: false,
        });
        self.messages.entry(id.to_string()).or_default();
        self.delivery
            .offline_msgs
            .entry(id.to_string())
            .or_default();
        self.known_dirty = true;
    }

    fn flush_peer_queues(&mut self, id: &str, ip: Option<String>, port: Option<u16>) {
        let ip_opt = ip.as_deref();
        self.flush_offline_queue(id, ip_opt, port);
        self.flush_offline_sync(id, ip_opt);
        self.flush_offline_name_updates(id, ip_opt);
        if let (Some(ip_str), Some(port_value)) = (ip_opt, port) {
            self.resend_pending_for_peer(id, ip_str, port_value);
        }
    }

    pub(super) fn mark_offline(&mut self, id: &str) {
        if let Some(user) = self.users.iter_mut().find(|user| user.id == id) {
            user.online = false;
            self.known_dirty = true;
        }
    }

    pub(super) fn handle_discover_received(
        &mut self,
        from_id: String,
        from_ip: String,
        from_name: Option<String>,
        peers: Vec<PeerBrief>,
    ) {
        let mut flush_targets: Vec<(String, Option<String>, Option<u16>)> = Vec::new();
        let mut pending_name_updates: Vec<(String, String, NameSource)> = Vec::new();

        if let Some(user) = self.users.iter_mut().find(|user| user.id == from_id) {
            user.online = true;
            if user.ip.as_deref() != Some(&from_ip) {
                user.ip = Some(from_ip.clone());
                self.known_dirty = true;
            }
            if user.port.is_none() {
                user.port = Some(UDP_MESSAGE_PORT);
            }
            if let Some(name) = from_name.clone() {
                pending_name_updates.push((from_id.clone(), name, NameSource::Direct));
            }
        } else {
            self.ensure_user_stub(
                &from_id,
                from_name.clone().unwrap_or_else(|| from_id.clone()),
                true,
                Some(from_ip.clone()),
            );
            if let Some(name) = from_name.clone() {
                pending_name_updates.push((from_id.clone(), name, NameSource::Direct));
            }
        }

        if let Some(user) = self.users.iter().find(|user| user.id == from_id) {
            flush_targets.push((from_id.clone(), user.ip.clone(), user.port));
        }

        for peer in peers {
            if peer.id.is_empty() || peer.id == self.self_id {
                continue;
            }

            if let Some(user) = self.users.iter_mut().find(|user| user.id == peer.id) {
                user.online = true;
                if let Some(ip) = peer.ip.clone() {
                    if user.ip.as_deref() != Some(&ip) {
                        user.ip = Some(ip);
                        self.known_dirty = true;
                    }
                }
                if user.port.is_none() {
                    user.port = Some(UDP_MESSAGE_PORT);
                }
                if let Some(name) = peer.name.clone() {
                    pending_name_updates.push((peer.id.clone(), name, NameSource::Indirect));
                }
                flush_targets.push((peer.id.clone(), user.ip.clone(), user.port));
            } else {
                self.ensure_user_stub(
                    &peer.id,
                    peer.name.clone().unwrap_or_else(|| peer.id.clone()),
                    true,
                    peer.ip.clone(),
                );
                if let Some(name) = peer.name.clone() {
                    pending_name_updates.push((peer.id.clone(), name, NameSource::Indirect));
                }
                flush_targets.push((peer.id.clone(), peer.ip.clone(), Some(UDP_MESSAGE_PORT)));
            }
        }

        for (id, ip, port) in flush_targets {
            self.flush_peer_queues(&id, ip, port);
        }

        for (id, name, source) in pending_name_updates {
            self.apply_name_update(&id, &name, source);
        }
    }

    pub(super) fn handle_peer_online(&mut self, id: String, ip: String) {
        if let Some(user) = self.users.iter_mut().find(|user| user.id == id) {
            user.online = true;
            user.ip = Some(ip.clone());
            if user.port.is_none() {
                user.port = Some(UDP_MESSAGE_PORT);
            }
            self.known_dirty = true;
            let ip_opt = user.ip.clone();
            let port_opt = user.port;
            self.flush_offline_queue(&id, ip_opt.as_deref(), port_opt);
            self.flush_offline_sync(&id, ip_opt.as_deref());
            self.flush_offline_name_updates(&id, ip_opt.as_deref());
            self.pending_resend
                .insert(id.clone(), Instant::now() + Duration::from_secs(1));
        } else {
            self.ensure_user_stub(&id, id.clone(), true, Some(ip));
        }
    }

    pub(super) fn handle_name_update(&mut self, id: String, name: String, ip: Option<String>) {
        if let Some(user) = self.users.iter_mut().find(|user| user.id == id) {
            let pending_name = name.clone();
            if let Some(ip_value) = ip.clone() {
                if user.ip.as_deref() != Some(&ip_value) {
                    user.ip = Some(ip_value);
                    self.known_dirty = true;
                }
            }
            let _ = user;
            self.apply_name_update(&id, &pending_name, NameSource::Direct);
        } else {
            self.ensure_user_stub(&id, name, false, ip);
            self.name_source.insert(id.clone(), NameSource::Direct);
        }
    }

    pub(super) fn handle_discovered(&mut self, peer: DiscoveredPeer, local_ip: String) {
        if self.local_port.is_none() {
            self.local_port = Some(UDP_MESSAGE_PORT);
        }

        let key = peer.id.clone();
        let name = peer
            .name
            .clone()
            .unwrap_or_else(|| format!("{}:{}", peer.ip, peer.port));

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

        let preferred_ip = peer.ip.clone();
        let (ip_clone, port_clone) = (preferred_ip.clone(), UDP_MESSAGE_PORT);
        self.maybe_switch_primary_interface(&local_ip, &peer.ip);
        if let Some(user) = self.users.iter_mut().find(|user| user.id == key) {
            user.online = true;
            user.name = name.clone();
            user.ip = Some(ip_clone.clone());
            user.port = Some(port_clone);
            user.tcp_port = None;
            user.protocol_version = Some(peer.version.clone());
            user.supports_reliable_folders = peer.supports_reliable_folders;
            user.bound_interface = Some(local_ip.clone());
            user.best_interface = Some(local_ip.clone());
            self.known_dirty = true;
            let peer_id = user.id.clone();
            let ip_opt = user.ip.clone();
            let port_opt = user.port;
            let _ = user;
            self.flush_offline_queue(&peer_id, ip_opt.as_deref(), port_opt);
            self.flush_offline_sync(&peer_id, ip_opt.as_deref());
            self.flush_offline_name_updates(&peer_id, ip_opt.as_deref());
        } else {
            self.users.push(User {
                id: key.clone(),
                name: name.clone(),
                online: true,
                ip: Some(ip_clone.clone()),
                port: Some(port_clone),
                tcp_port: None,
                protocol_version: Some(peer.version.clone()),
                supports_reliable_folders: peer.supports_reliable_folders,
                bound_interface: Some(local_ip.clone()),
                best_interface: Some(local_ip.clone()),
                has_unread: false,
            });
            self.messages.entry(key.clone()).or_default();
            self.delivery.offline_msgs.entry(key.clone()).or_default();
            self.known_dirty = true;
            if self.selected_user_id.is_none() {
                self.selected_user_id = Some(key.clone());
            }
            self.flush_offline_queue(&key, Some(&ip_clone), Some(port_clone));
            self.flush_offline_sync(&key, Some(&ip_clone));
            self.flush_offline_name_updates(&key, Some(&ip_clone));
        }
    }

    pub(super) fn handle_chat_received(
        &mut self,
        ctx: &egui::Context,
        from_id: String,
        from_ip: String,
        from_port: u16,
        from_name: Option<String>,
        text: String,
        send_ts: String,
        recv_ts: String,
        msg_id: String,
        local_ip: String,
    ) {
        let key = from_id.clone();

        if self.delivery.mark_received_message(&msg_id) {
            self.persist_runtime_state();
            let msgs = self.messages.entry(key.clone()).or_default();
            msgs.push(crate::model::ChatMessage {
                from_me: false,
                text: text.clone(),
                send_ts: send_ts.clone(),
                recv_ts: Some(recv_ts.clone()),
                last_sync_ts: None,
                file_path: None,
                transfer_id: None,
                transfer_status: None,
                msg_id: Some(msg_id.clone()),
                is_read: false,
                is_pending: false,
                needs_sync: false,
            });
            self.log_history(
                &key,
                false,
                &text,
                &send_ts,
                Some(&recv_ts),
                None,
                None,
                None,
                Some(&msg_id),
                false,
                false,
            );

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

        let (ip_clone, port_clone) = (from_ip.clone(), UDP_MESSAGE_PORT);
        self.maybe_switch_primary_interface(&local_ip, &from_ip);

        let mut pending_name_update: Option<String> = None;
        if let Some(user) = self.users.iter_mut().find(|user| user.id == key) {
            if self.selected_user_id.as_deref() != Some(&key) {
                user.has_unread = true;
            }
            user.online = true;
            if user.ip.as_deref() != Some(&ip_clone) {
                user.ip = Some(ip_clone.clone());
                self.known_dirty = true;
            }
            pending_name_update = from_name.clone();
            user.port = Some(port_clone);
            user.bound_interface = Some(local_ip.clone());
            user.best_interface = Some(local_ip.clone());
            let peer_id = user.id.clone();
            let ip_opt = user.ip.clone();
            let port_opt = user.port;
            let _ = user;
            self.flush_offline_queue(&peer_id, ip_opt.as_deref(), port_opt);
            self.flush_offline_sync(&peer_id, ip_opt.as_deref());
            self.flush_offline_name_updates(&peer_id, ip_opt.as_deref());
        } else {
            let display = if from_id.is_empty() {
                format!("{}:{}", from_ip, from_port)
            } else {
                from_name.clone().unwrap_or_else(|| from_id.clone())
            };
            self.users.push(User {
                id: key.clone(),
                name: display,
                online: true,
                ip: Some(ip_clone.clone()),
                port: Some(port_clone),
                tcp_port: None,
                protocol_version: None,
                supports_reliable_folders: false,
                bound_interface: Some(local_ip.clone()),
                best_interface: Some(local_ip.clone()),
                has_unread: self.selected_user_id.as_deref() != Some(&key),
            });
            self.delivery.offline_msgs.entry(key.clone()).or_default();
            self.known_dirty = true;
            self.flush_offline_queue(&key, Some(&ip_clone), Some(port_clone));
            self.flush_offline_sync(&key, Some(&ip_clone));
            self.flush_offline_name_updates(&key, Some(&ip_clone));
        }
        if let Some(name) = pending_name_update {
            self.apply_name_update(&key, &name, NameSource::Direct);
        }
    }

    pub(super) fn handle_chat_ack(&mut self, from_id: String, msg_id: String) {
        if let Some(list) = self.delivery.pending_acks.get_mut(&from_id) {
            list.retain(|(mid, _)| mid != &msg_id);
        }
        if let Some(user) = self.users.iter_mut().find(|user| user.id == from_id) {
            if let Some(bound) = &user.bound_interface {
                user.best_interface = Some(bound.clone());
                debug_println!(
                    "ACK received from {} (msg_id={}), confirmed best_interface: {}",
                    from_id,
                    msg_id,
                    bound
                );
            }
        }
        let ack_ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        if let Some(msgs) = self.messages.get_mut(&from_id) {
            if let Some(message) = msgs
                .iter_mut()
                .rev()
                .find(|message| message.msg_id.as_deref() == Some(&msg_id))
            {
                message.transfer_status = Some("已送达".to_string());
                message.recv_ts = Some(ack_ts.clone());
                message.is_pending = false;
            }
        }
        self.update_history_ack(&from_id, &msg_id, &ack_ts);
        if let Some(queue) = self.delivery.offline_msgs.get_mut(&from_id) {
            queue.retain(|queued| queued.msg_id.as_deref() != Some(&msg_id));
        }
        self.persist_runtime_state();
    }

    pub(super) fn flush_offline_name_updates(&mut self, peer_id: &str, ip: Option<&str>) {
        let Some(ip) = ip else { return };
        let Some(name) = self.offline_name_updates.remove(peer_id) else {
            return;
        };
        let Some(tx) = self.net_cmd_tx.clone() else {
            return;
        };
        let via = self.get_best_interface_for_peer(ip);
        let _ = tx.send(NetCmd::SendNameUpdate {
            ip: ip.to_string(),
            via,
            name,
            local_ip: self.local_ip.clone(),
        });
    }

    pub(super) fn apply_name_update(&mut self, id: &str, new_name: &str, source: NameSource) {
        if new_name.trim().is_empty() {
            return;
        }
        let existing_source = self.name_source.get(id).copied();
        if matches!(existing_source, Some(NameSource::Direct)) && source == NameSource::Indirect {
            return;
        }
        if let Some(user) = self.users.iter_mut().find(|user| user.id == id) {
            if user.name != new_name {
                user.name = new_name.to_string();
                self.known_dirty = true;
            }
        }
        self.name_source.insert(id.to_string(), source);
    }

    pub(super) fn merge_users_by_id(&mut self) {
        let before = self.users.len();
        let mut first: HashMap<String, usize> = HashMap::new();
        let mut index = 0;
        while index < self.users.len() {
            let id = self.users[index].id.clone();
            if let Some(&keep_idx) = first.get(&id) {
                let duplicate = self.users.remove(index);
                let primary = &mut self.users[keep_idx];
                primary.online |= duplicate.online;
                if primary.ip.is_none() {
                    primary.ip = duplicate.ip.clone();
                }
                if primary.port.is_none() {
                    primary.port = duplicate.port;
                }
                if primary.tcp_port.is_none() {
                    primary.tcp_port = duplicate.tcp_port;
                }
                if primary.bound_interface.is_none() {
                    primary.bound_interface = duplicate.bound_interface.clone();
                }
                if primary.best_interface.is_none() {
                    primary.best_interface = duplicate.best_interface.clone();
                }
                primary.has_unread |= duplicate.has_unread;
                if (primary.name.trim().is_empty() || primary.name == primary.id)
                    && !duplicate.name.trim().is_empty()
                {
                    primary.name = duplicate.name;
                }
                continue;
            }

            first.insert(id, index);
            index += 1;
        }
        if before != self.users.len() {
            self.known_dirty = true;
        }
    }
}

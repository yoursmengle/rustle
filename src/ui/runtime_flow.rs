use super::RustleApp;
use crate::debug_println;
use crate::model::{NetCmd, QueuedMsg};
use std::time::Instant;

impl RustleApp {
    pub(super) fn process_startup_probe(&mut self) {
        if self.probed_known {
            return;
        }
        if let Some(tx) = &self.net_cmd_tx {
            for user in &self.users {
                if let (Some(ip), Some(_port)) = (user.ip.as_ref(), user.port) {
                    let _ = tx.send(NetCmd::ProbePeer {
                        ip: ip.clone(),
                        via: user.bound_interface.clone(),
                    });
                }
            }
            self.probed_known = true;
        }
    }

    pub(super) fn process_pending_ack_timeouts(&mut self) {
        let now_instant = Instant::now();
        let mut timeouts: Vec<(String, String)> = Vec::new();
        for (peer, list) in self.delivery.pending_acks.iter() {
            for (msg_id, deadline) in list.iter() {
                if now_instant > *deadline {
                    timeouts.push((peer.clone(), msg_id.clone()));
                }
            }
        }

        if timeouts.is_empty() {
            return;
        }

        for (peer, msg_id) in &timeouts {
            let mut offline_data = None;

            if let Some(msgs) = self.messages.get_mut(peer) {
                if let Some(message) = msgs
                    .iter_mut()
                    .rev()
                    .find(|message| message.msg_id.as_deref() == Some(msg_id))
                {
                    message.transfer_status = Some("等待对方上线...".to_string());
                    message.is_pending = true;
                    offline_data = Some((message.text.clone(), message.send_ts.clone()));
                }
            }

            if let Some((text, send_ts)) = offline_data {
                let queue = self.delivery.offline_msgs.entry(peer.clone()).or_default();
                let exists = queue
                    .iter()
                    .any(|queued| queued.msg_id.as_deref() == Some(msg_id.as_str()));
                if !exists {
                    queue.push(QueuedMsg {
                        text,
                        send_ts,
                        msg_id: Some(msg_id.clone()),
                        file_path: None,
                        transfer_id: None,
                        is_dir: false,
                    });
                }
                self.update_history_pending(peer, msg_id, true);
            }

            self.mark_offline(peer);
        }

        for (peer, msg_id) in timeouts {
            if let Some(list) = self.delivery.pending_acks.get_mut(&peer) {
                list.retain(|(mid, _)| mid != &msg_id);
            }
        }
        self.persist_runtime_state();
    }

    pub(super) fn process_scheduled_resends(&mut self) {
        let now_instant = Instant::now();
        let mut due_resend: Vec<String> = Vec::new();
        for (peer, due) in self.pending_resend.iter() {
            if *due <= now_instant {
                due_resend.push(peer.clone());
            }
        }

        for peer in due_resend.iter() {
            if let Some((ip_clone, port_clone)) = self
                .users
                .iter()
                .find(|user| user.id == *peer)
                .map(|user| (user.ip.clone(), user.port))
            {
                if let (Some(ip), Some(port)) = (ip_clone.as_deref(), port_clone) {
                    debug_println!("Scheduled resend for {}", peer);
                    self.resend_pending_for_peer(peer, ip, port);
                }
            }
            self.pending_resend.remove(peer);
        }
    }
}

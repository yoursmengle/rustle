use super::{RustleApp, SyncTransfer};
use crate::debug_println;
use crate::model::{NetCmd, QueuedMsg, TCP_DIR_PORT, TCP_FILE_PORT};

impl RustleApp {
    pub(super) fn flush_offline_sync(&mut self, peer_id: &str, ip: Option<&str>) {
        let Some(ip) = ip else { return };
        let Some(tx) = self.net_cmd_tx.clone() else {
            return;
        };
        if let Some(queue) = self.offline_sync.get_mut(peer_id) {
            if queue.is_empty() {
                return;
            }
            let drained: Vec<SyncTransfer> = queue.drain(..).collect();
            let mut remain = Vec::new();
            let via = self.get_best_interface_for_peer(ip);
            for item in drained {
                if item.is_dir && !self.peer_supports_reliable_folders(peer_id) {
                    remain.push(item);
                    continue;
                }
                let target_tcp_port = if item.is_dir {
                    TCP_DIR_PORT
                } else {
                    TCP_FILE_PORT
                };
                if tx
                    .send(NetCmd::SendFile {
                        peer_id: peer_id.to_string(),
                        ip: ip.to_string(),
                        tcp_port: target_tcp_port,
                        path: item.path.clone(),
                        transfer_id: None,
                        supports_transfer_id: true,
                        is_dir: item.is_dir,
                        via: via.clone(),
                        is_sync: true,
                    })
                    .is_err()
                {
                    remain.push(item);
                }
            }
            if !remain.is_empty() {
                self.offline_sync.insert(peer_id.to_string(), remain);
            }
        }
    }

    pub(super) fn resend_pending_for_peer(&mut self, peer_id: &str, ip: &str, port: u16) {
        let via = self.get_best_interface_for_peer(ip);
        let mut to_send: Vec<(String, String, String)> = Vec::new();

        if let Some(msgs) = self.messages.get_mut(peer_id) {
            for message in msgs.iter_mut() {
                if message.from_me && message.is_pending {
                    let message_id = message
                        .msg_id
                        .clone()
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                    message.msg_id = Some(message_id.clone());
                    message.transfer_status = Some("发送中...".to_string());
                    to_send.push((message_id, message.text.clone(), message.send_ts.clone()));
                }
            }
        }

        for (message_id, text, send_ts) in to_send {
            debug_println!(
                "Resend pending message to {}: mid={} text={}",
                peer_id,
                message_id,
                text
            );
            self.try_send_message_with_retry(
                peer_id,
                ip,
                port,
                &text,
                &send_ts,
                &message_id,
                via.clone(),
            );

            let was_sent = self
                .delivery
                .pending_acks
                .get(peer_id)
                .map(|list| list.iter().any(|(mid_item, _)| mid_item == &message_id))
                .unwrap_or(false);
            debug_println!(
                "Resend result for {} mid={} sent={} pending_acks_count={}",
                peer_id,
                message_id,
                was_sent,
                self.delivery
                    .pending_acks
                    .get(peer_id)
                    .map(|list| list.len())
                    .unwrap_or(0)
            );

            if was_sent {
                if let Some(msgs) = self.messages.get_mut(peer_id) {
                    if let Some(message) = msgs
                        .iter_mut()
                        .rev()
                        .find(|message| message.msg_id.as_deref() == Some(&message_id))
                    {
                        message.is_pending = false;
                        message.transfer_status = Some("发送中...".to_string());
                    }
                }
                self.update_history_pending(peer_id, &message_id, false);
                if let Some(queue) = self.delivery.offline_msgs.get_mut(peer_id) {
                    queue.retain(|queued| queued.msg_id.as_deref() != Some(&message_id));
                }
            } else {
                if let Some(msgs) = self.messages.get_mut(peer_id) {
                    if let Some(message) = msgs
                        .iter_mut()
                        .rev()
                        .find(|message| message.msg_id.as_deref() == Some(&message_id))
                    {
                        message.transfer_status = Some("等待对方上线...".to_string());
                        message.is_pending = true;
                    }
                }

                if let Some(queue) = self.delivery.offline_msgs.get_mut(peer_id) {
                    if !queue.iter().any(|queued| {
                        queued.msg_id.as_deref() == Some(&message_id) && queued.text == text
                    }) {
                        queue.push(QueuedMsg {
                            text: text.clone(),
                            send_ts: send_ts.clone(),
                            msg_id: Some(message_id.clone()),
                            file_path: None,
                            transfer_id: None,
                            is_dir: false,
                        });
                    }
                } else {
                    self.delivery.offline_msgs.insert(
                        peer_id.to_string(),
                        vec![QueuedMsg {
                            text: text.clone(),
                            send_ts: send_ts.clone(),
                            msg_id: Some(message_id.clone()),
                            file_path: None,
                            transfer_id: None,
                            is_dir: false,
                        }],
                    );
                }
                self.update_history_pending(peer_id, &message_id, true);
            }
        }
        self.persist_runtime_state();
    }
}

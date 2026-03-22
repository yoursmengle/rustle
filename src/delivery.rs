use crate::model::{ChatMessage, QueuedMsg};
use crate::storage::{
    load_runtime_state, save_runtime_state, PersistedPeerRuntimeState, PersistedPendingAck,
    RuntimeState,
};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const MAX_RECENT_RECEIVED_MSG_IDS: usize = 1000;

#[derive(Default, Debug)]
pub struct DeliveryState {
    pub offline_msgs: HashMap<String, Vec<QueuedMsg>>,
    pub pending_acks: HashMap<String, Vec<(String, Instant)>>,
    pub received_msg_ids: HashSet<String>,
    pub recent_received_msg_ids: Vec<String>,
}

impl DeliveryState {
    pub fn persist_runtime_state(&self) {
        let mut peers = HashMap::new();

        for (peer_id, queue) in &self.offline_msgs {
            if !queue.is_empty() {
                peers
                    .entry(peer_id.clone())
                    .or_insert_with(PersistedPeerRuntimeState::default)
                    .offline_msgs = queue.clone();
            }
        }

        for (peer_id, pending) in &self.pending_acks {
            if !pending.is_empty() {
                peers
                    .entry(peer_id.clone())
                    .or_insert_with(PersistedPeerRuntimeState::default)
                    .pending_acks = pending
                    .iter()
                    .map(|(msg_id, _)| PersistedPendingAck {
                        msg_id: msg_id.clone(),
                    })
                    .collect();
            }
        }

        save_runtime_state(&RuntimeState {
            peers,
            recent_received_msg_ids: self.recent_received_msg_ids.clone(),
        });
    }

    pub fn restore_runtime_state(&mut self, messages: &mut HashMap<String, Vec<ChatMessage>>) {
        let state = load_runtime_state();

        self.recent_received_msg_ids = state.recent_received_msg_ids;
        self.received_msg_ids = self.recent_received_msg_ids.iter().cloned().collect();

        for (peer_id, peer_state) in state.peers {
            let queue = self.offline_msgs.entry(peer_id.clone()).or_default();
            for queued in peer_state.offline_msgs {
                let exists = queue.iter().any(|existing| {
                    existing.msg_id == queued.msg_id
                        || (existing.text == queued.text
                            && existing.send_ts == queued.send_ts
                            && existing.file_path == queued.file_path
                            && existing.is_dir == queued.is_dir)
                });
                if !exists {
                    queue.push(queued);
                }
            }

            let pending_list = self.pending_acks.entry(peer_id.clone()).or_default();
            for pending in peer_state.pending_acks {
                if !pending_list
                    .iter()
                    .any(|(msg_id, _)| msg_id == &pending.msg_id)
                {
                    pending_list.push((
                        pending.msg_id.clone(),
                        Instant::now() + Duration::from_secs(5),
                    ));
                }

                if let Some(peer_messages) = messages.get_mut(&peer_id) {
                    if let Some(message) = peer_messages.iter_mut().rev().find(|msg| {
                        msg.from_me && msg.msg_id.as_deref() == Some(pending.msg_id.as_str())
                    }) {
                        message.is_pending = true;
                        if message.file_path.is_none() {
                            message.transfer_status = Some("等待对方确认...".to_string());
                        }
                    }
                }
            }
        }
    }

    pub fn mark_received_message(&mut self, msg_id: &str) -> bool {
        if self.received_msg_ids.contains(msg_id) {
            return false;
        }
        let msg_id = msg_id.to_string();
        self.received_msg_ids.insert(msg_id.clone());
        self.recent_received_msg_ids.push(msg_id);
        while self.recent_received_msg_ids.len() > MAX_RECENT_RECEIVED_MSG_IDS {
            if let Some(evicted) = self.recent_received_msg_ids.first().cloned() {
                self.recent_received_msg_ids.remove(0);
                self.received_msg_ids.remove(&evicted);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_received_message_deduplicates_ids() {
        let mut state = DeliveryState::default();

        assert!(state.mark_received_message("m1"));
        assert!(!state.mark_received_message("m1"));
        assert_eq!(state.recent_received_msg_ids, vec!["m1".to_string()]);
    }

    #[test]
    fn restore_runtime_state_recovers_recent_received_ids() {
        let state = RuntimeState {
            peers: HashMap::new(),
            recent_received_msg_ids: vec!["m1".to_string(), "m2".to_string()],
        };

        save_runtime_state(&state);

        let mut delivery = DeliveryState::default();
        let mut messages = HashMap::new();
        delivery.restore_runtime_state(&mut messages);

        assert!(delivery.received_msg_ids.contains("m1"));
        assert!(delivery.received_msg_ids.contains("m2"));
        assert_eq!(
            delivery.recent_received_msg_ids,
            vec!["m1".to_string(), "m2".to_string()]
        );
    }
}

use super::{FileProgressHistoryEffects, RustleApp};
use crate::model::{ChatMessage, SyncStatus};
use chrono::Local;
use std::collections::HashSet;
use std::path::Path;
use std::thread;

impl RustleApp {
    fn find_transfer_message<'a>(
        messages: &'a [ChatMessage],
        from_me: bool,
        transfer_id: Option<&str>,
        file_name: &str,
    ) -> Option<&'a ChatMessage> {
        messages.iter().rev().find(|message| {
            Self::message_matches_transfer(message, from_me, transfer_id, file_name)
        })
    }

    fn find_transfer_message_mut<'a>(
        messages: &'a mut [ChatMessage],
        from_me: bool,
        transfer_id: Option<&str>,
        file_name: &str,
    ) -> Option<&'a mut ChatMessage> {
        messages.iter_mut().rev().find(|message| {
            Self::message_matches_transfer(message, from_me, transfer_id, file_name)
        })
    }

    fn ensure_message_transfer_id(message: &mut ChatMessage, transfer_id: &Option<String>) {
        if message.transfer_id.is_none() {
            message.transfer_id = transfer_id.clone();
        }
    }

    fn append_incoming_file_message(
        messages: &mut Vec<ChatMessage>,
        file_name: &str,
        status: &str,
        is_dir: bool,
        is_sync: bool,
    ) -> (String, String) {
        let ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let text = if is_dir {
            format!("📁 {}", file_name)
        } else {
            format!("📄 {}", file_name)
        };
        messages.push(ChatMessage {
            from_me: false,
            text: text.clone(),
            send_ts: ts.clone(),
            recv_ts: Some(ts.clone()),
            last_sync_ts: None,
            file_path: Some(file_name.to_string()),
            transfer_id: None,
            transfer_status: Some(status.to_string()),
            msg_id: None,
            is_read: false,
            is_pending: false,
            needs_sync: is_sync,
        });
        (text, ts)
    }

    fn handle_incoming_sync_file_progress(
        messages: &mut [ChatMessage],
        transfer_id: &Option<String>,
        file_name: &str,
        is_final: bool,
        succeeded: bool,
        effects: &mut FileProgressHistoryEffects,
    ) {
        if !(is_final && succeeded) {
            return;
        }

        if let Some(message) =
            Self::find_transfer_message_mut(messages, false, transfer_id.as_deref(), file_name)
        {
            let ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            Self::ensure_message_transfer_id(message, transfer_id);
            message.last_sync_ts = Some(ts.clone());
            if let Some(path) = message.file_path.clone() {
                effects.pending_sync = Some((path, message.transfer_id.clone(), ts, false));
            }
        }
    }

    fn handle_incoming_regular_file_progress(
        peer_id: &str,
        messages: &mut Vec<ChatMessage>,
        transfer_id: &Option<String>,
        file_name: &str,
        progress: f32,
        status: &str,
        is_dir: bool,
        local_path: Option<&str>,
        is_sync: bool,
        is_final: bool,
        succeeded: bool,
        selected_peer_matches: bool,
        logged_incoming_files: &mut HashSet<(String, String)>,
        effects: &mut FileProgressHistoryEffects,
    ) -> bool {
        if progress == 0.0 {
            let (text, ts) =
                Self::append_incoming_file_message(messages, file_name, status, is_dir, is_sync);
            if logged_incoming_files.insert((peer_id.to_string(), file_name.to_string())) {
                let path_for_history = local_path.unwrap_or(file_name).to_string();
                effects.pending_log = Some((text, ts, path_for_history, transfer_id.clone()));
            }
            return selected_peer_matches;
        }

        let Some(message) =
            Self::find_transfer_message_mut(messages, false, transfer_id.as_deref(), file_name)
        else {
            return false;
        };

        Self::ensure_message_transfer_id(message, transfer_id);
        message.transfer_status = Some(status.to_string());
        if let Some(path) = local_path {
            message.file_path = Some(path.to_string());
        } else if message.file_path.is_none() {
            message.file_path = Some(file_name.to_string());
        }

        if !(is_final && succeeded) {
            return false;
        }

        if let Some(path) = local_path {
            effects.pending_path_update = Some((
                file_name.to_string(),
                message.transfer_id.clone(),
                path.to_string(),
            ));
        }
        let ts = message
            .recv_ts
            .clone()
            .unwrap_or_else(|| Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
        if let Some(path) = message.file_path.clone() {
            message.last_sync_ts = Some(ts.clone());
            effects.pending_sync =
                Some((path.clone(), message.transfer_id.clone(), ts.clone(), false));
            if logged_incoming_files.insert((peer_id.to_string(), file_name.to_string())) {
                effects.pending_log =
                    Some((message.text.clone(), ts, path, message.transfer_id.clone()));
            }
        }

        false
    }

    fn handle_outgoing_file_progress(
        messages: &mut [ChatMessage],
        transfer_id: &Option<String>,
        file_name: &str,
        status: &str,
        is_sync: bool,
        is_final: bool,
        succeeded: bool,
        effects: &mut FileProgressHistoryEffects,
    ) {
        let Some(message) =
            Self::find_transfer_message_mut(messages, true, transfer_id.as_deref(), file_name)
        else {
            return;
        };

        Self::ensure_message_transfer_id(message, transfer_id);
        let new_needs_sync = !(is_final && succeeded);
        if message.needs_sync != new_needs_sync {
            message.needs_sync = new_needs_sync;
            if let Some(path) = message.file_path.clone() {
                effects.pending_needs_sync_update =
                    Some((path, message.transfer_id.clone(), new_needs_sync));
            }
        }

        if !is_sync {
            message.transfer_status = Some(status.to_string());
        }
        if is_final && succeeded {
            let ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            message.last_sync_ts = Some(ts.clone());
            message.is_pending = false;
            if is_sync {
                message.transfer_status = Some(format!("已同步，最后同步时间： {}", ts));
            }
            if let Some(path) = message.file_path.clone() {
                effects.pending_sync =
                    Some((path.clone(), message.transfer_id.clone(), ts.clone(), true));
                effects.pending_file_done = Some((path, message.transfer_id.clone(), ts, true));
            }
        }
    }

    fn matched_transfer_history_target(
        &self,
        peer_id: &str,
        from_me: bool,
        transfer_id: Option<&str>,
        file_name: &str,
    ) -> Option<(String, Option<String>)> {
        self.messages
            .get(peer_id)
            .and_then(|messages| {
                Self::find_transfer_message(messages, from_me, transfer_id, file_name)
            })
            .and_then(|message| {
                message
                    .file_path
                    .clone()
                    .map(|path| (path, message.transfer_id.clone()))
            })
    }

    fn update_history_transfer_state(
        &self,
        peer_id: &str,
        path: &str,
        transfer_id: Option<&str>,
        needs_sync: bool,
        completion: Option<(&str, bool)>,
    ) {
        self.update_history_needs_sync(peer_id, path, transfer_id, needs_sync, true);
        if let Some((ts, from_me)) = completion {
            self.update_history_sync(peer_id, path, transfer_id, ts, from_me);
            self.update_history_file_done(peer_id, path, transfer_id, ts, from_me);
        }
    }

    fn sync_status_for_progress(is_sync: bool, is_final: bool, succeeded: bool) -> SyncStatus {
        if is_final && succeeded {
            if is_sync {
                SyncStatus::Synced
            } else {
                SyncStatus::Sent
            }
        } else if is_sync {
            SyncStatus::Syncing
        } else {
            SyncStatus::Sending
        }
    }

    pub(super) fn handle_file_completion_ack_event(
        &mut self,
        from_id: &str,
        transfer_id: Option<&str>,
        file_name: &str,
        is_sync: bool,
        succeeded: bool,
        status: &str,
    ) {
        if let Some(messages) = self.messages.get_mut(from_id) {
            if let Some(message) =
                Self::find_transfer_message_mut(messages, true, transfer_id, file_name)
            {
                message.transfer_status = Some(status.to_string());
                message.is_pending = !succeeded;
                if succeeded {
                    let ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                    message.recv_ts = Some(ts.clone());
                    message.last_sync_ts = Some(ts);
                    message.needs_sync = false;
                }
            }
        }

        if let Some((path, matched_transfer_id)) =
            self.matched_transfer_history_target(from_id, true, transfer_id, file_name)
        {
            let completion_ts =
                succeeded.then(|| Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
            self.update_history_transfer_state(
                from_id,
                &path,
                matched_transfer_id.as_deref(),
                !succeeded,
                completion_ts.as_deref().map(|ts| (ts, true)),
            );
        }

        if !is_sync {
            self.persist_runtime_state();
        }
    }

    fn apply_file_progress_history_effects(
        &mut self,
        peer_id: &str,
        is_sync: bool,
        effects: FileProgressHistoryEffects,
    ) {
        if let Some((path, transfer_id, needs_sync)) = effects.pending_needs_sync_update {
            self.update_history_transfer_state(
                peer_id,
                &path,
                transfer_id.as_deref(),
                needs_sync,
                None,
            );
        }
        if let Some((text, ts, path, transfer_id)) = effects.pending_log {
            self.log_history(
                peer_id,
                false,
                &text,
                &ts,
                Some(&ts),
                Some(&path),
                None,
                transfer_id.as_deref(),
                None,
                false,
                is_sync,
            );
        }
        if let Some((file_name, transfer_id, path)) = effects.pending_path_update {
            self.update_history_file_path(
                peer_id,
                &file_name,
                transfer_id.as_deref(),
                &path,
                false,
            );
        }
        if let Some((path, transfer_id, ts, from_me)) = effects.pending_sync {
            let needs_sync = effects
                .pending_file_done
                .as_ref()
                .map(|_| false)
                .unwrap_or(true);
            let completion = effects.pending_file_done.as_ref().map(
                |(_, completion_transfer_id, completion_ts, completion_from_me)| {
                    let effective_transfer_id =
                        completion_transfer_id.as_deref().or(transfer_id.as_deref());
                    (
                        effective_transfer_id,
                        completion_ts.as_str(),
                        *completion_from_me,
                    )
                },
            );
            if let Some((completion_transfer_id, completion_ts, completion_from_me)) = completion {
                self.update_history_transfer_state(
                    peer_id,
                    &path,
                    completion_transfer_id,
                    needs_sync,
                    Some((completion_ts, completion_from_me)),
                );
            } else {
                self.update_history_sync(peer_id, &path, transfer_id.as_deref(), &ts, from_me);
            }
        }
    }

    fn update_metadata_for_file_progress(
        &mut self,
        peer_id: &str,
        file_name: &str,
        local_path: Option<&String>,
        is_sync: bool,
        is_final: bool,
        succeeded: bool,
        is_dir: bool,
    ) {
        if let Some(store) = self.metadata_store() {
            let path_hint = local_path.cloned();
            let file_hint = file_name.to_string();
            let peer_hint = peer_id.to_string();
            let now_ts = Local::now().timestamp();
            let next_status = Self::sync_status_for_progress(is_sync, is_final, succeeded);
            let _ = store.update_first_matching(|meta| {
                let matched = if let Some(lp) = path_hint.as_ref() {
                    meta.abs_path == *lp
                } else {
                    meta.filename == file_hint
                };
                if !matched || meta.peer_id.as_deref() != Some(&peer_hint) {
                    return None;
                }
                let mut updated = meta.clone();
                if matches!(next_status, SyncStatus::Synced | SyncStatus::Sent) {
                    updated.last_synced_time = Some(now_ts);
                }
                updated.sync_status = next_status.clone();
                Some(updated)
            });
        }

        if is_final && succeeded && !is_dir {
            if let Some(path_for_hash) = local_path.cloned() {
                let peer_clone = peer_id.to_string();
                let file_clone = file_name.to_string();
                let is_sync_flag = is_sync;
                let db_path = crate::metadata::MetadataStore::default_db_path();
                thread::spawn(move || {
                    if let Some(hash) = crate::storage::sha256_file(Path::new(&path_for_hash)) {
                        if let Ok(store) = crate::metadata::MetadataStore::open(&db_path) {
                            let _ = store.update_first_matching(|meta| {
                                if meta.peer_id.as_deref() != Some(&peer_clone) {
                                    return None;
                                }
                                let matches_path =
                                    meta.abs_path == path_for_hash || meta.filename == file_clone;
                                if !matches_path {
                                    return None;
                                }
                                let mut updated = meta.clone();
                                updated.sha256 = Some(hash.clone());
                                updated.last_synced_sha256 = Some(hash.clone());
                                updated.last_synced_time = Some(Local::now().timestamp());
                                updated.sync_status = if is_sync_flag {
                                    SyncStatus::Synced
                                } else {
                                    SyncStatus::Sent
                                };
                                Some(updated)
                            });
                        }
                    }
                });
            }
        }

        self.refresh_metadata_cache();
    }

    pub(super) fn handle_file_progress_event(
        &mut self,
        peer_id: &str,
        transfer_id: Option<String>,
        file_name: String,
        progress: f32,
        status: String,
        is_incoming: bool,
        is_dir: bool,
        local_path: Option<String>,
        is_sync: bool,
        is_final: bool,
        succeeded: bool,
    ) {
        let mut effects = FileProgressHistoryEffects::default();

        if let Some(messages) = self.messages.get_mut(peer_id) {
            if is_incoming {
                if is_sync {
                    Self::handle_incoming_sync_file_progress(
                        messages,
                        &transfer_id,
                        &file_name,
                        is_final,
                        succeeded,
                        &mut effects,
                    );
                } else {
                    let should_scroll = Self::handle_incoming_regular_file_progress(
                        peer_id,
                        messages,
                        &transfer_id,
                        &file_name,
                        progress,
                        &status,
                        is_dir,
                        local_path.as_deref(),
                        is_sync,
                        is_final,
                        succeeded,
                        self.selected_user_id.as_deref() == Some(peer_id),
                        &mut self.logged_incoming_files,
                        &mut effects,
                    );
                    if should_scroll {
                        self.scroll_to_bottom = true;
                    }
                }
            } else {
                Self::handle_outgoing_file_progress(
                    messages,
                    &transfer_id,
                    &file_name,
                    &status,
                    is_sync,
                    is_final,
                    succeeded,
                    &mut effects,
                );
            }
        }

        self.apply_file_progress_history_effects(peer_id, is_sync, effects);
        self.update_metadata_for_file_progress(
            peer_id,
            &file_name,
            local_path.as_ref(),
            is_sync,
            is_final,
            succeeded,
            is_dir,
        );
    }
}

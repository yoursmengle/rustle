use super::RustleApp;
use crate::metadata::MetadataStore;
use crate::model::{NetCmd, SyncStatus, TCP_DIR_PORT, TCP_FILE_PORT};
use std::path::PathBuf;

impl RustleApp {
    pub(super) fn metadata_store(&self) -> Option<MetadataStore> {
        MetadataStore::open_default().ok()
    }

    pub(super) fn refresh_metadata_cache_from_store(&mut self, store: &MetadataStore) {
        if let Ok(items) = store.get_all() {
            self.meta_list = items.iter().map(|m| super::MetadataView::from(m)).collect();
        }
    }

    pub(super) fn refresh_metadata_cache(&mut self) {
        if let Some(store) = self.metadata_store() {
            self.refresh_metadata_cache_from_store(&store);
        }
    }

    pub(super) fn with_metadata_store(
        &mut self,
        refresh_after: bool,
        mut action: impl FnMut(&MetadataStore),
    ) {
        if let Some(store) = self.metadata_store() {
            action(&store);
            if refresh_after {
                self.refresh_metadata_cache_from_store(&store);
            }
        }
    }

    pub(super) fn reload_meta_list(&mut self) {
        self.refresh_metadata_cache();
    }

    pub(super) fn set_meta_auto(&mut self, id: &str, enabled: bool) {
        self.with_metadata_store(true, |store| {
            let _ = store.update_first_matching(|meta| {
                if meta.id == id {
                    let mut updated = meta.clone();
                    updated.auto_sync_enabled = enabled;
                    Some(updated)
                } else {
                    None
                }
            });
        });
    }

    pub(super) fn trigger_sync_for_meta(&mut self, id: &str) {
        let Some(store) = self.metadata_store() else {
            return;
        };
        let Ok(items) = store.get_all() else {
            self.refresh_metadata_cache_from_store(&store);
            return;
        };
        let Some(meta) = items.into_iter().find(|m| m.id == id) else {
            self.refresh_metadata_cache_from_store(&store);
            return;
        };
        if meta.is_dir
            && !self.peer_supports_reliable_folders(meta.peer_id.as_deref().unwrap_or_default())
        {
            self.refresh_metadata_cache_from_store(&store);
            return;
        }
        if let Some(tx) = &self.net_cmd_tx {
            if let Some(peer_ip) = meta.peer_ip.clone() {
                let tcp_port = if meta.is_dir {
                    TCP_DIR_PORT
                } else {
                    TCP_FILE_PORT
                };
                let path = PathBuf::from(meta.abs_path.clone());
                let _ = tx.send(NetCmd::SendFile {
                    peer_id: meta.peer_id.clone().unwrap_or_default(),
                    ip: peer_ip.clone(),
                    tcp_port,
                    path,
                    transfer_id: Some(meta.id.clone()),
                    supports_transfer_id: true,
                    is_dir: meta.is_dir,
                    via: None,
                    is_sync: true,
                });
                let _ = store.update_first_matching(|record| {
                    if record.id == id {
                        let mut updated = record.clone();
                        updated.sync_status = SyncStatus::Syncing;
                        Some(updated)
                    } else {
                        None
                    }
                });
            }
        }
        self.refresh_metadata_cache_from_store(&store);
    }
}

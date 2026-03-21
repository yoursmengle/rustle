use crate::model::SyncStatus;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Metadata {
    pub id: String,
    pub peer_id: Option<String>,
    pub peer_ip: Option<String>,
    pub abs_path: String,
    pub rel_path: Option<String>,
    pub filename: String,
    pub size: u64,
    pub is_dir: bool,
    pub modified_time: i64,
    pub sha256: Option<String>,
    pub last_synced_sha256: Option<String>,
    pub last_synced_time: Option<i64>,
    pub sync_status: SyncStatus,
    pub auto_sync_enabled: bool,
}

pub struct MetadataStore {
    db: sled::Db,
}

impl MetadataStore {
    pub fn open(path: &str) -> Result<Self> {
        let db = sled::open(path)?;
        Ok(MetadataStore { db })
    }

    pub fn default_db_path() -> String {
        crate::storage::data_dir()
            .join("metadata_db")
            .to_string_lossy()
            .to_string()
    }

    pub fn open_default() -> Result<Self> {
        let path = Self::default_db_path();
        Self::open(&path)
    }

    pub fn update_first_matching<F>(&self, mut updater: F) -> Result<bool>
    where
        F: FnMut(&Metadata) -> Option<Metadata>,
    {
        for entry in self.db.iter() {
            let (key, val) = entry?;
            let meta: Metadata = serde_json::from_slice(&val)?;
            if let Some(updated) = updater(&meta) {
                let data = serde_json::to_vec(&updated)?;
                self.db.insert(key, data)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn insert(&self, meta: &Metadata) -> Result<()> {
        let key = meta.id.as_bytes();
        let val = serde_json::to_vec(meta)?;
        self.db.insert(key, val)?;
        Ok(())
    }

    pub fn get_all(&self) -> Result<Vec<Metadata>> {
        let mut v = Vec::new();
        for item in self.db.iter() {
            let (_k, val) = item?;
            let m: Metadata = serde_json::from_slice(&val)?;
            v.push(m);
        }
        Ok(v)
    }

    pub fn start_watcher_with_sender(
        cmd_tx: Option<std::sync::mpsc::Sender<crate::model::NetCmd>>,
    ) {
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("create rt");
            rt.block_on(async move {
                use tokio::time::{interval, Duration};
                let store_path = MetadataStore::default_db_path();
                let store = match MetadataStore::open(&store_path) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("failed to open metadata db: {:?}", e);
                        return;
                    }
                };
                let mut int = interval(Duration::from_secs(60 * 30));
                loop {
                    int.tick().await;
                    match store.get_all() {
                        Ok(items) => {
                            for m in items {
                                // 检查文件修改时间
                                let p = std::path::Path::new(&m.abs_path);
                                if let Some(new_mtime) = crate::storage::file_mtime_seconds(p) {
                                    if new_mtime != m.modified_time {
                                        // 计算 sha256
                                        let new_sha = match tokio::task::spawn_blocking({
                                            let p2 = p.to_path_buf();
                                            move || crate::storage::sha256_file(&p2)
                                        })
                                        .await
                                        {
                                            Ok(s) => s,
                                            Err(_) => None,
                                        };

                                        if new_sha != m.sha256 {
                                            let mut updated = m.clone();
                                            updated.sha256 = new_sha.clone();
                                            updated.modified_time = new_mtime;
                                            updated.sync_status = SyncStatus::FileChanged;
                                            let _ = store.insert(&updated);
                                            if updated.auto_sync_enabled {
                                                if let Some(peer_ip) = updated.peer_ip.clone() {
                                                    if let Some(tx) = &cmd_tx {
                                                        let mut syncing_meta = updated.clone();
                                                        syncing_meta.sync_status =
                                                            SyncStatus::Syncing;
                                                        let _ = store.insert(&syncing_meta);
                                                        let tcp_port = if syncing_meta.is_dir {
                                                            crate::model::TCP_DIR_PORT
                                                        } else {
                                                            crate::model::TCP_FILE_PORT
                                                        };
                                                        let _ = tx.send(
                                                            crate::model::NetCmd::SendFile {
                                                                peer_id: syncing_meta
                                                                    .peer_id
                                                                    .clone()
                                                                    .unwrap_or_default(),
                                                                ip: peer_ip,
                                                                tcp_port,
                                                                path: std::path::PathBuf::from(
                                                                    syncing_meta.abs_path.clone(),
                                                                ),
                                                                transfer_id: Some(syncing_meta.id.clone()),
                                                                supports_transfer_id: true,
                                                                is_dir: syncing_meta.is_dir,
                                                                via: None,
                                                                is_sync: true,
                                                            },
                                                        );
                                                    }
                                                }
                                            }
                                        } else {
                                            // sha 相同，仅更新 mtime
                                            let mut updated = m.clone();
                                            updated.modified_time = new_mtime;
                                            let _ = store.insert(&updated);
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("watcher error: {:?}", e);
                        }
                    }
                }
            });
        });
    }
}

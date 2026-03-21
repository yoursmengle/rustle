use crate::model::{ChatMessage, HistoryEntry};
use crate::storage::{history_dir, peer_history_path};
use chrono::{Duration as ChronoDuration, Local};
use serde_json::{self, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

pub struct LoadedHistoryMessage {
    pub peer_id: String,
    pub message: ChatMessage,
}

pub fn log_history(
    peer_id: &str,
    from_me: bool,
    text: &str,
    send_ts: &str,
    recv_ts: Option<&str>,
    file_path: Option<&str>,
    sync_ts: Option<&str>,
    msg_id: Option<&str>,
    is_pending: bool,
    needs_sync: bool,
) {
    let path = peer_history_path(peer_id);
    log_history_at(
        &path, peer_id, from_me, text, send_ts, recv_ts, file_path, sync_ts, msg_id, is_pending,
        needs_sync,
    );
}

pub fn clear_history(peer_id: &str) {
    let path = peer_history_path(peer_id);
    let _ = fs::remove_file(&path);
}

pub fn load_recent_history(days: i64) -> Vec<LoadedHistoryMessage> {
    load_recent_history_from_dir(&history_dir(), days)
}

pub fn update_history_sync(peer_id: &str, file_path: &str, sync_ts: &str, from_me: bool) {
    let path = peer_history_path(peer_id);
    update_history_file_field(&path, file_path, from_me, |val| {
        val["sync_ts"] = Value::String(sync_ts.to_string());
    });
}

pub fn update_history_ack(peer_id: &str, msg_id: &str, recv_ts: &str) {
    let path = peer_history_path(peer_id);
    update_history_by_msg_id(&path, msg_id, |val| {
        val["recv_ts"] = Value::String(recv_ts.to_string());
        val["is_pending"] = Value::Bool(false);
    });
}

pub fn update_history_file_done(peer_id: &str, file_path: &str, recv_ts: &str, from_me: bool) {
    let path = peer_history_path(peer_id);
    update_history_file_field(&path, file_path, from_me, |val| {
        val["recv_ts"] = Value::String(recv_ts.to_string());
        val["is_pending"] = Value::Bool(false);
    });
}

pub fn update_history_needs_sync(peer_id: &str, file_path: &str, needs_sync: bool, from_me: bool) {
    let path = peer_history_path(peer_id);
    update_history_file_field(&path, file_path, from_me, |val| {
        val["needs_sync"] = Value::Bool(needs_sync);
    });
}

pub fn update_history_file_path(peer_id: &str, file_name: &str, new_path: &str, from_me: bool) {
    let path = peer_history_path(peer_id);
    update_history_file_field_exact_or_suffix(&path, file_name, from_me, |val| {
        val["file_path"] = Value::String(new_path.to_string());
    });
}

pub fn update_history_pending(peer_id: &str, msg_id: &str, is_pending: bool) {
    let path = peer_history_path(peer_id);
    update_history_by_msg_id(&path, msg_id, |val| {
        val["is_pending"] = Value::Bool(is_pending);
    });
}

fn log_history_at(
    path: &Path,
    peer_id: &str,
    from_me: bool,
    text: &str,
    send_ts: &str,
    recv_ts: Option<&str>,
    file_path: Option<&str>,
    sync_ts: Option<&str>,
    msg_id: Option<&str>,
    is_pending: bool,
    needs_sync: bool,
) {
    let line = serde_json::json!({
        "peer_id": peer_id,
        "from_me": from_me,
        "text": text,
        "send_ts": send_ts,
        "recv_ts": recv_ts,
        "file_path": file_path,
        "sync_ts": sync_ts,
        "msg_id": msg_id,
        "is_pending": is_pending,
        "needs_sync": needs_sync,
        "ts": Local::now().to_rfc3339(),
    })
    .to_string();

    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| {
            f.write_all(line.as_bytes())?;
            f.write_all(b"\n")
        });
}

fn load_recent_history_from_dir(dir: &Path, days: i64) -> Vec<LoadedHistoryMessage> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let cutoff = Local::now() - ChronoDuration::days(days.max(1));
    let mut loaded = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }

        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => continue,
        };

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
                    if dt.with_timezone(&Local) < cutoff {
                        continue;
                    }
                }
            }

            let pending = entry.is_pending.unwrap_or(false);
            let transfer_status = if pending && entry.from_me {
                Some("等待对方上线...".to_string())
            } else {
                None
            };

            loaded.push(LoadedHistoryMessage {
                peer_id: entry.peer_id,
                message: ChatMessage {
                    from_me: entry.from_me,
                    text: entry.text,
                    send_ts: entry.send_ts,
                    recv_ts: entry.recv_ts,
                    last_sync_ts: entry.sync_ts,
                    file_path: entry.file_path,
                    transfer_status,
                    msg_id: entry.msg_id,
                    is_read: true,
                    is_pending: pending,
                    needs_sync: entry.needs_sync.unwrap_or(false),
                },
            });
        }
    }

    loaded
}

fn update_history_file_field<F>(path: &Path, file_path: &str, from_me: bool, mut update: F)
where
    F: FnMut(&mut Value),
{
    rewrite_history(path, |val| {
        let matches_from = val.get("from_me").and_then(|v| v.as_bool()) == Some(from_me);
        let matches_file = val
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| p == file_path || p.ends_with(file_path) || file_path.ends_with(p))
            .unwrap_or(false);
        if matches_from && matches_file {
            update(val);
            true
        } else {
            false
        }
    });
}

fn update_history_file_field_exact_or_suffix<F>(
    path: &Path,
    file_name: &str,
    from_me: bool,
    mut update: F,
) where
    F: FnMut(&mut Value),
{
    rewrite_history(path, |val| {
        let matches_from = val.get("from_me").and_then(|v| v.as_bool()) == Some(from_me);
        let matches_file = val
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| p == file_name || p.ends_with(file_name))
            .unwrap_or(false);
        if matches_from && matches_file {
            update(val);
            true
        } else {
            false
        }
    });
}

fn update_history_by_msg_id<F>(path: &Path, msg_id: &str, mut update: F)
where
    F: FnMut(&mut Value),
{
    rewrite_history(path, |val| {
        if val.get("msg_id").and_then(|v| v.as_str()) == Some(msg_id) {
            update(val);
            true
        } else {
            false
        }
    });
}

fn rewrite_history<F>(path: &Path, mut matcher: F)
where
    F: FnMut(&mut Value) -> bool,
{
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };

    let mut lines: Vec<String> = Vec::new();
    let mut updated = false;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(mut val) = serde_json::from_str::<Value>(line) {
            if matcher(&mut val) {
                lines.push(val.to_string());
                updated = true;
                continue;
            }
        }
        lines.push(line.to_string());
    }
    if updated {
        let _ = fs::write(path, lines.join("\n") + "\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn unique_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rustle_history_{}_{}", name, Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_recent_history_roundtrip_preserves_pending_message_state() {
        let dir = unique_temp_dir("roundtrip");
        let path = dir.join("peer-1.jsonl");

        log_history_at(
            &path,
            "peer-1",
            true,
            "hello",
            "2026-03-20 12:00:00",
            None,
            None,
            None,
            Some("m1"),
            true,
            false,
        );

        let loaded = load_recent_history_from_dir(&dir, 30);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].peer_id, "peer-1");
        assert_eq!(loaded[0].message.msg_id.as_deref(), Some("m1"));
        assert!(loaded[0].message.is_pending);
        assert_eq!(
            loaded[0].message.transfer_status.as_deref(),
            Some("等待对方上线...")
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn update_history_ack_marks_message_delivered() {
        let dir = unique_temp_dir("ack");
        let path = dir.join("peer-1.jsonl");

        log_history_at(
            &path,
            "peer-1",
            true,
            "hello",
            "2026-03-20 12:00:00",
            None,
            None,
            None,
            Some("m1"),
            true,
            false,
        );

        update_history_by_msg_id(&path, "m1", |val| {
            val["recv_ts"] = Value::String("2026-03-20 12:00:01".to_string());
            val["is_pending"] = Value::Bool(false);
        });

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("2026-03-20 12:00:01"));
        assert!(text.contains("\"is_pending\":false"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clear_history_removes_file() {
        let dir = unique_temp_dir("clear");
        let path = dir.join("peer-1.jsonl");
        std::fs::write(&path, b"{}\n").unwrap();

        let _ = std::fs::remove_file(&path);

        assert!(!path.exists());

        let _ = std::fs::remove_dir_all(dir);
    }
}

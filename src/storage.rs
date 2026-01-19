use crate::model::{SyncTree, RECEIVE_MAP_FILE, SYNC_TREE_FILE};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;
use uuid::Uuid;

const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "default_history_days")]
    pub history_days: i64,
    #[serde(default)]
    pub recv_dir: Option<String>,
    #[serde(default = "default_auto_sync_on_send")]
    pub auto_sync_on_send: bool,
    #[serde(default = "default_auto_check_update")]
    pub auto_check_update: bool,
    #[serde(default)]
    pub preferred_interface: Option<String>,
}

fn default_history_days() -> i64 {
    180
}

fn default_auto_sync_on_send() -> bool {
    true
}

fn default_auto_check_update() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            history_days: default_history_days(),
            recv_dir: None,
            auto_sync_on_send: default_auto_sync_on_send(),
            auto_check_update: default_auto_check_update(),
            preferred_interface: None,
        }
    }
}

pub fn load_settings() -> AppSettings {
    let path = data_path(SETTINGS_FILE);
    if let Ok(text) = fs::read_to_string(&path) {
        if let Ok(mut settings) = serde_json::from_str::<AppSettings>(&text) {
            if settings.history_days <= 0 {
                settings.history_days = default_history_days();
            }
            if let Some(dir) = settings.recv_dir.as_ref() {
                if dir.trim().is_empty() {
                    settings.recv_dir = None;
                }
            }
            return settings;
        }
    }
    AppSettings::default()
}

pub fn save_settings(settings: &AppSettings) {
    let path = data_path(SETTINGS_FILE);
    if let Ok(text) = serde_json::to_string_pretty(settings) {
        let _ = fs::write(path, text);
    }
}

pub fn data_dir() -> PathBuf {
    let mut dir = env::var_os("LOCALAPPDATA")
        .or_else(|| env::var_os("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir());
    dir.push("Rustle");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn data_path(name: &str) -> PathBuf {
    let mut p = data_dir();
    p.push(name);
    p
}

pub fn history_dir() -> PathBuf {
    let mut dir = data_dir();
    dir.push("history");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn peer_history_path(peer_id: &str) -> PathBuf {
    let mut p = history_dir();
    p.push(format!("{}.jsonl", peer_id));
    p
}

fn sync_tree_path() -> PathBuf {
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join(SYNC_TREE_FILE);
        }
    }
    data_path(SYNC_TREE_FILE)
}

pub fn load_sync_tree() -> SyncTree {
    let path = sync_tree_path();
    if let Ok(text) = fs::read_to_string(path) {
        let mut de = serde_json::Deserializer::from_str(&text);
        let de = serde_stacker::Deserializer::new(&mut de);
        if let Ok(tree) = SyncTree::deserialize(de) {
            return tree;
        }
    }
    SyncTree::default()
}

pub fn save_sync_tree(tree: &SyncTree) {
    let path = sync_tree_path();
    if let Ok(mut file) = fs::File::create(path) {
        let mut ser = serde_json::Serializer::pretty(&mut file);
        let ser = serde_stacker::Serializer::new(&mut ser);
        let _ = tree.serialize(ser);
    }
}

pub fn load_receive_map() -> HashMap<String, String> {
    let path = data_path(RECEIVE_MAP_FILE);
    if let Ok(text) = fs::read_to_string(path) {
        if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&text) {
            return map;
        }
    }
    HashMap::new()
}

pub fn save_receive_map(map: &HashMap<String, String>) {
    let path = data_path(RECEIVE_MAP_FILE);
    if let Ok(text) = serde_json::to_string_pretty(map) {
        let _ = fs::write(path, text);
    }
}

pub fn file_mtime_seconds(path: &Path) -> Option<i64> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let dur = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(dur.as_secs() as i64)
}

pub fn sha256_file(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 1024 * 1024];
    loop {
        let n = std::io::Read::read(&mut file, &mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Some(hex::encode(digest))
}

pub fn default_download_dir() -> PathBuf {
    let settings = load_settings();
    if let Some(dir) = settings.recv_dir.as_ref() {
        if !dir.trim().is_empty() {
            let custom = PathBuf::from(dir.trim());
            let _ = fs::create_dir_all(&custom);
            return custom;
        }
    }
    #[cfg(target_os = "windows")]
    {
        // Prefer D: if present; otherwise fallback to C:
        let has_d = Path::new("D:\\").exists();
        let base = if has_d {
            PathBuf::from("D:/rustle_downloads")
        } else {
            PathBuf::from("C:/rustle_downloads")
        };
        let _ = fs::create_dir_all(&base);
        base
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut p = data_dir();
        p.push("downloads");
        let _ = fs::create_dir_all(&p);
        p
    }
}

#[cfg(target_os = "windows")]
pub fn windows_long_path(p: &Path) -> PathBuf {
    // Enable long-path (MAX_PATH) support by adding a verbatim prefix.
    let s = p.as_os_str().to_string_lossy();
    if s.starts_with(r"\\?\") {
        return p.to_path_buf();
    }
    // Normalize forward slashes to backslashes to appease Windows verbatim rules.
    let normalized = s.replace('/', r"\");
    // Avoid duplicating prefix when path already includes drive/backslash.
    if normalized.starts_with(r"\\") {
        // UNC path
        format!(r"\\?\UNC\{}", normalized.trim_start_matches(r"\\")).into()
    } else {
        format!(r"\\?\{}", normalized).into()
    }
}

#[cfg(not(target_os = "windows"))]
pub fn windows_long_path(p: &Path) -> PathBuf {
    p.to_path_buf()
}

pub fn read_machine_uuid() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        fn extract_uuid(bytes: &[u8]) -> Option<String> {
            let text = String::from_utf8_lossy(bytes);
            text.lines()
                .map(|l| l.trim())
                .find(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_graphic()))
                .map(|s| s.to_string())
        }

        if let Ok(output) = Command::new("powershell")
            .args(["-NoProfile", "-Command", "(Get-CimInstance Win32_ComputerSystemProduct).UUID"])
            .output()
        {
            if output.status.success() {
                if let Some(uuid) = extract_uuid(&output.stdout) {
                    return Some(uuid);
                }
            }
        }

        if let Ok(output) = Command::new("wmic")
            .args(["csproduct", "get", "UUID"])
            .output()
        {
            if output.status.success() {
                if let Some(uuid) = extract_uuid(&output.stdout) {
                    return Some(uuid);
                }
            }
        }

        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

pub fn load_or_init_node_id() -> String {
    // 仅在内存中生成 ID：优先硬件 UUID，失败则随机 UUID；不再读写 node_id.txt
    if let Some(hw_uuid) = read_machine_uuid() {
        hw_uuid
    } else {
        Uuid::new_v4().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn data_dir_exists_and_named() {
        let dir = data_dir();
        assert!(dir.exists());
        assert!(dir.ends_with(OsStr::new("Rustle")));
    }

    #[test]
    fn data_path_appends_name() {
        let p = data_path("foo.txt");
        let tail = PathBuf::from("Rustle").join("foo.txt");
        assert!(p.ends_with(tail) || p.ends_with(OsStr::new("foo.txt")));
    }

    #[test]
    fn default_download_dir_created_and_named() {
        let p = default_download_dir();
        assert!(p.exists());
        #[cfg(target_os = "windows")]
        assert!(p.to_string_lossy().to_lowercase().ends_with("/rustle_downloads")
            || p.to_string_lossy().to_lowercase().ends_with("\\rustle_downloads"));
        #[cfg(not(target_os = "windows"))]
        assert!(p.ends_with(OsStr::new("downloads")));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_long_path_adds_prefix() {
        let p = Path::new("C:\\temp\\rustle_test");
        let out = windows_long_path(p);
        let s = out.to_string_lossy();
        assert!(s.starts_with(r"\\?\"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn windows_long_path_passthrough() {
        let p = Path::new("/tmp/rustle_test");
        let out = windows_long_path(p);
        assert_eq!(out, p);
    }

    #[test]
    fn read_machine_uuid_non_empty_if_present() {
        if let Some(uuid) = read_machine_uuid() {
            assert!(!uuid.trim().is_empty());
        }
    }

    #[test]
    fn load_or_init_node_id_non_empty() {
        let id = load_or_init_node_id();
        assert!(!id.trim().is_empty());
    }
}

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

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

pub fn default_download_dir() -> PathBuf {
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

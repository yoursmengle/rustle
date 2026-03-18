use crate::debug_println;
#[allow(unused_imports)]
use crate::metadata::{Metadata, MetadataStore};
use crate::model::{FileCompletionPayload, PeerEvent, TCP_DIR_PORT, TCP_FILE_PORT, UDP_MESSAGE_PORT};
use crate::storage::{
    default_download_dir, load_or_init_node_id, load_receive_map, save_receive_map,
    windows_long_path,
};
use chrono::Local;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, ErrorKind, Read};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

const MAX_ID_LEN: usize = 64;
const MAX_NAME_LEN: usize = 255;
const FLAG_SYNC: u8 = 0x01;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FolderManifestEntryType {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FolderManifestEntry {
    path: String,
    entry_type: FolderManifestEntryType,
    size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FolderManifest {
    root_name: String,
    entries: Vec<FolderManifestEntry>,
    file_count: usize,
    dir_count: usize,
    total_bytes: u64,
}

#[derive(Debug, Clone)]
struct FolderPackage {
    manifest: FolderManifest,
    tar_sha256: String,
}

fn emit_progress(
    peer_tx: &Sender<PeerEvent>,
    peer_id: Option<String>,
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
    let _ = peer_tx.send(PeerEvent::FileProgress {
        peer_id,
        file_name,
        progress,
        status,
        is_incoming,
        is_dir,
        local_path,
        is_sync,
        is_final,
        succeeded,
    });
}

fn normalize_relative_path(path: &Path) -> io::Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => {
                let text = part.to_string_lossy();
                if text.is_empty() {
                    return Err(io::Error::new(ErrorKind::InvalidData, "empty path segment"));
                }
                parts.push(text.replace('\\', "/"));
            }
            std::path::Component::Prefix(_)
            | std::path::Component::RootDir
            | std::path::Component::ParentDir => {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    format!("unsafe relative path: {:?}", path),
                ));
            }
        }
    }
    Ok(parts.join("/"))
}

fn normalized_path_to_buf(path: &str) -> PathBuf {
    let mut buf = PathBuf::new();
    for part in path.split('/').filter(|part| !part.is_empty()) {
        buf.push(part);
    }
    buf
}

fn path_collision_key(path: &str) -> String {
    if cfg!(windows) {
        path.to_ascii_lowercase()
    } else {
        path.to_string()
    }
}

fn sha256_reader<R: Read>(mut reader: R) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 1024 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn tar_archive_path(root_name: &str, rel: &str) -> PathBuf {
    let mut path = PathBuf::from(root_name);
    if !rel.is_empty() {
        path.push(normalized_path_to_buf(rel));
    }
    path
}

fn collect_folder_manifest(root: &Path, root_name: &str) -> io::Result<Vec<(PathBuf, FolderManifestEntry)>> {
    let mut stack = vec![root.to_path_buf()];
    let mut collected = Vec::new();

    while let Some(current_dir) = stack.pop() {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(current_dir.as_path())? {
            entries.push(entry?);
        }
        entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

        for entry in entries {
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .map_err(|_| io::Error::new(ErrorKind::InvalidData, "failed to strip folder root"))?;
            let rel_norm = normalize_relative_path(rel)?;
            let meta = std::fs::metadata(&path)?;

            if meta.is_dir() {
                collected.push((
                    path.clone(),
                    FolderManifestEntry {
                        path: rel_norm,
                        entry_type: FolderManifestEntryType::Directory,
                        size: 0,
                        sha256: None,
                    },
                ));
                stack.push(path);
            } else if meta.is_file() {
                let sha256 = crate::storage::sha256_file(&path).ok_or_else(|| {
                    io::Error::new(
                        ErrorKind::InvalidData,
                        format!("failed to hash folder file {:?}", path),
                    )
                })?;
                collected.push((
                    path,
                    FolderManifestEntry {
                        path: rel_norm,
                        entry_type: FolderManifestEntryType::File,
                        size: meta.len(),
                        sha256: Some(sha256),
                    },
                ));
            } else {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    format!("unsupported folder entry under {}", root_name),
                ));
            }
        }
    }

    collected.sort_by(|a, b| a.1.path.cmp(&b.1.path));
    Ok(collected)
}

fn build_folder_package(source_dir: &Path, tar_path: &Path, root_name: &str) -> io::Result<FolderPackage> {
    let entries = collect_folder_manifest(source_dir, root_name)?;
    let tar_fs_path = windows_long_path(tar_path);
    let src_fs_path = windows_long_path(source_dir);

    let file = std::fs::File::create(&tar_fs_path)?;
    let mut builder = tar::Builder::new(file);
    builder.append_dir(root_name, &src_fs_path)?;

    let mut manifest_entries = Vec::new();
    let mut file_count = 0usize;
    let mut dir_count = 0usize;
    let mut total_bytes = 0u64;

    for (path, manifest_entry) in entries {
        let archive_path = tar_archive_path(root_name, &manifest_entry.path);
        match manifest_entry.entry_type {
            FolderManifestEntryType::Directory => {
                builder.append_dir(&archive_path, windows_long_path(&path))?;
                dir_count += 1;
            }
            FolderManifestEntryType::File => {
                builder.append_path_with_name(windows_long_path(&path), &archive_path)?;
                file_count += 1;
                total_bytes += manifest_entry.size;
            }
        }
        manifest_entries.push(manifest_entry);
    }

    builder.finish()?;

    let tar_sha256 = sha256_reader(std::fs::File::open(&tar_fs_path)?)?;
    Ok(FolderPackage {
        manifest: FolderManifest {
            root_name: root_name.to_string(),
            entries: manifest_entries,
            file_count,
            dir_count,
            total_bytes,
        },
        tar_sha256,
    })
}

fn decode_folder_manifest(bytes: &[u8]) -> io::Result<FolderManifest> {
    serde_json::from_slice(bytes).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))
}

fn normalize_archive_member_path(path: &Path, expected_root: &str) -> io::Result<Option<String>> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            std::path::Component::Prefix(_)
            | std::path::Component::RootDir
            | std::path::Component::ParentDir => {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    format!("unsafe archive path {:?}", path),
                ));
            }
        }
    }

    if parts.is_empty() {
        return Err(io::Error::new(ErrorKind::InvalidData, "archive entry has empty path"));
    }
    if parts[0] != expected_root {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("archive root mismatch: expected {}, got {}", expected_root, parts[0]),
        ));
    }
    if parts.len() == 1 {
        return Ok(None);
    }
    let mut rel_buf = PathBuf::new();
    for part in &parts[1..] {
        rel_buf.push(part);
    }
    normalize_relative_path(&rel_buf).map(Some)
}

fn unpack_folder_archive(
    tar_path: &Path,
    extraction_root: &Path,
    manifest: &FolderManifest,
) -> io::Result<PathBuf> {
    let final_root = extraction_root.join(&manifest.root_name);
    std::fs::create_dir_all(&final_root)?;
    let mut archive = tar::Archive::new(std::fs::File::open(tar_path)?);
    let mut seen = HashSet::new();

    for entry_result in archive.entries()? {
        let mut entry = entry_result?;
        let entry_path = entry.path()?.to_path_buf();
        let rel = normalize_archive_member_path(&entry_path, &manifest.root_name)?;
        let header_type = entry.header().entry_type();
        if header_type.is_symlink() || header_type.is_hard_link() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("unsupported archive link entry {:?}", entry_path),
            ));
        }

        let destination = match rel {
            Some(ref rel_path) => {
                let collision = path_collision_key(rel_path);
                if !seen.insert(collision) {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        format!("archive path collision for {}", rel_path),
                    ));
                }
                final_root.join(normalized_path_to_buf(rel_path))
            }
            None => final_root.clone(),
        };

        if header_type.is_dir() {
            std::fs::create_dir_all(&destination)?;
            continue;
        }
        if !header_type.is_file() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("unsupported archive entry type for {:?}", entry_path),
            ));
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        entry.unpack(&destination)?;
    }

    Ok(final_root)
}

fn enumerate_extracted_entries(root: &Path) -> io::Result<HashMap<String, FolderManifestEntry>> {
    let mut result = HashMap::new();
    for (path, entry) in collect_folder_manifest(root, root.file_name().and_then(|n| n.to_str()).unwrap_or("folder"))? {
        let _ = path;
        result.insert(entry.path.clone(), entry);
    }
    Ok(result)
}

fn validate_extracted_folder(root: &Path, manifest: &FolderManifest) -> io::Result<()> {
    if !root.exists() || !root.is_dir() {
        return Err(io::Error::new(ErrorKind::NotFound, "validated folder root is missing"));
    }

    let actual = enumerate_extracted_entries(root)?;
    let mut expected = HashMap::new();
    for entry in &manifest.entries {
        expected.insert(entry.path.clone(), entry.clone());
    }

    for (path, entry) in &expected {
        let Some(actual_entry) = actual.get(path) else {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("validated folder is missing {}", path),
            ));
        };
        if actual_entry.entry_type != entry.entry_type {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("validated folder type mismatch for {}", path),
            ));
        }
        if actual_entry.size != entry.size {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("validated folder size mismatch for {}", path),
            ));
        }
        if actual_entry.sha256 != entry.sha256 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("validated folder digest mismatch for {}", path),
            ));
        }
    }

    for extra in actual.keys() {
        if !expected.contains_key(extra) {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("validated folder has unexpected entry {}", extra),
            ));
        }
    }

    Ok(())
}

fn promote_staged_folder(staged_root: &Path, final_root: &Path) -> io::Result<()> {
    if let Some(parent) = final_root.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if final_root.exists() {
        std::fs::remove_dir_all(final_root)?;
    }
    std::fs::rename(staged_root, final_root)
}

fn staged_file_path(transfer_root: &Path, safe_filename: &str) -> PathBuf {
    transfer_root.join(format!(".{}.part-{}", safe_filename, Uuid::new_v4()))
}

fn promote_staged_file(staged_path: &Path, final_path: &Path) -> io::Result<()> {
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if final_path.exists() {
        std::fs::remove_file(final_path)?;
    }
    std::fs::rename(staged_path, final_path)
}

fn cleanup_failed_regular_file(staged_path: &Path, final_path: &Path) {
    let _ = std::fs::remove_file(staged_path);
    let _ = std::fs::remove_file(final_path);
}

async fn send_file_completion_ack(
    sender_ip: std::net::IpAddr,
    file_name: &str,
    is_dir: bool,
    is_sync: bool,
    success: bool,
    status: &str,
) {
    let payload = FileCompletionPayload {
        msg_type: "file_completion_ack".to_string(),
        from_id: load_or_init_node_id(),
        file_name: file_name.to_string(),
        is_dir,
        is_sync,
        success,
        status: status.to_string(),
    };

    if let Ok(data) = serde_json::to_vec(&payload) {
        if let Ok(socket) = tokio::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).await {
            let target = SocketAddr::new(sender_ip, UDP_MESSAGE_PORT);
            let _ = socket.send_to(&data, target).await;
        }
    }
}

fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.2} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

fn format_speed(bytes: u64, elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return "0.00 MB/s".to_string();
    }
    let mb_per_sec = bytes as f64 / secs / (1024.0 * 1024.0);
    format!("{:.2} MB/s", mb_per_sec)
}

fn receive_map_key(sender_id: &str, is_dir: bool, filename: &str) -> String {
    format!(
        "{}|{}|{}",
        sender_id,
        if is_dir { "dir" } else { "file" },
        filename
    )
}

fn sanitize_filename(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "unnamed".to_string();
    }
    let candidate = Path::new(trimmed)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut cleaned = String::with_capacity(candidate.len());
    for c in candidate.chars() {
        if c.is_control()
            || c == '/'
            || c == '\\'
            || c == ':'
            || c == '*'
            || c == '?'
            || c == '"'
            || c == '<'
            || c == '>'
            || c == '|'
        {
            cleaned.push('_');
        } else {
            cleaned.push(c);
        }
    }
    let cleaned = cleaned.trim().trim_end_matches(&[' ', '.'][..]).to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "unnamed".to_string()
    } else {
        cleaned
    }
}

pub async fn handle_incoming_file(
    mut socket: TcpStream,
    addr: SocketAddr,
    peer_tx: Sender<PeerEvent>,
) {
    let _ = socket.set_nodelay(true);

    let fail_and_emit = |peer_tx: &Sender<PeerEvent>,
                         sender_id: &str,
                         filename: &str,
                         is_dir: bool,
                         is_sync: bool,
                         local_path: Option<String>,
                         status: String| {
        emit_progress(
            peer_tx,
            Some(sender_id.to_string()),
            filename.to_string(),
            1.0,
            status,
            true,
            is_dir,
            local_path,
            is_sync,
            true,
            false,
        );
    };

    let mut type_buf = [0u8; 1];
    if socket.read_exact(&mut type_buf).await.is_err() {
        debug_println!("[rx] failed to read type from {addr}");
        return;
    }
    let mut flag_buf = [0u8; 1];
    if socket.read_exact(&mut flag_buf).await.is_err() {
        debug_println!("[rx] failed to read flags from {addr}");
        return;
    }
    let is_dir = type_buf[0] == 1;
    let is_sync = (flag_buf[0] & FLAG_SYNC) == FLAG_SYNC;

    let mut id_len_buf = [0u8; 1];
    if socket.read_exact(&mut id_len_buf).await.is_err() {
        debug_println!("[rx] failed to read id_len from {addr}");
        return;
    }
    let id_len = id_len_buf[0] as usize;
    if id_len == 0 || id_len > MAX_ID_LEN {
        eprintln!("[rx] invalid id_len {id_len} from {addr}");
        return;
    }

    let mut id_buf = vec![0u8; id_len];
    if socket.read_exact(&mut id_buf).await.is_err() {
        eprintln!("[rx] failed to read id bytes from {addr}");
        return;
    }
    let sender_id = String::from_utf8_lossy(&id_buf).to_string();

    let mut len_buf = [0u8; 2];
    if socket.read_exact(&mut len_buf).await.is_err() {
        eprintln!("[rx] failed to read name_len from {addr}");
        return;
    }
    let name_len = u16::from_be_bytes(len_buf) as usize;
    if name_len == 0 || name_len > MAX_NAME_LEN {
        eprintln!("[rx] invalid name_len {name_len} from {addr}");
        return;
    }

    let mut name_buf = vec![0u8; name_len];
    if socket.read_exact(&mut name_buf).await.is_err() {
        eprintln!("[rx] failed to read name bytes from {addr}");
        return;
    }
    let filename = String::from_utf8_lossy(&name_buf).to_string();
    let safe_filename = sanitize_filename(&filename);

    let mut size_buf = [0u8; 8];
    if socket.read_exact(&mut size_buf).await.is_err() {
        eprintln!("[rx] failed to read size bytes from {addr}");
        return;
    }
    let total_size = u64::from_be_bytes(size_buf);

    let mut leading_payload: Vec<u8> = Vec::new();
    let mut sha256_present_buf = [0u8; 1];
    let expected_sha256: Option<String> = if socket.read_exact(&mut sha256_present_buf).await.is_ok() {
        match sha256_present_buf[0] {
            1 => {
                let mut sha256_buf = [0u8; 32];
                if socket.read_exact(&mut sha256_buf).await.is_ok() {
                    Some(hex::encode(sha256_buf))
                } else {
                    None
                }
            }
            0 => None,
            legacy_first_byte => {
                leading_payload.push(legacy_first_byte);
                None
            }
        }
    } else {
        None
    };

    let mut expected_manifest: Option<FolderManifest> = None;
    let mut expected_tar_sha256: Option<String> = None;
    if is_dir {
        if !leading_payload.is_empty() {
            fail_and_emit(
                &peer_tx,
                &sender_id,
                &filename,
                true,
                is_sync,
                None,
                "接收失败：发送方不支持可靠目录校验".to_string(),
            );
            return;
        }

        let mut manifest_present_buf = [0u8; 1];
        if socket.read_exact(&mut manifest_present_buf).await.is_err() {
            fail_and_emit(
                &peer_tx,
                &sender_id,
                &filename,
                true,
                is_sync,
                None,
                "接收失败：目录元数据缺失".to_string(),
            );
            return;
        }
        if manifest_present_buf[0] != 1 {
            fail_and_emit(
                &peer_tx,
                &sender_id,
                &filename,
                true,
                is_sync,
                None,
                "接收失败：发送方未提供目录校验清单".to_string(),
            );
            return;
        }
        let mut manifest_len_buf = [0u8; 4];
        if socket.read_exact(&mut manifest_len_buf).await.is_err() {
            fail_and_emit(
                &peer_tx,
                &sender_id,
                &filename,
                true,
                is_sync,
                None,
                "接收失败：目录清单读取失败".to_string(),
            );
            return;
        }
        let manifest_len = u32::from_be_bytes(manifest_len_buf) as usize;
        let mut manifest_buf = vec![0u8; manifest_len];
        if socket.read_exact(&mut manifest_buf).await.is_err() {
            fail_and_emit(
                &peer_tx,
                &sender_id,
                &filename,
                true,
                is_sync,
                None,
                "接收失败：目录清单不完整".to_string(),
            );
            return;
        }
        match decode_folder_manifest(&manifest_buf) {
            Ok(manifest) => expected_manifest = Some(manifest),
            Err(err) => {
                fail_and_emit(
                    &peer_tx,
                    &sender_id,
                    &filename,
                    true,
                    is_sync,
                    None,
                    format!("接收失败：目录清单无效 ({err})"),
                );
                return;
            }
        }

        let mut tar_sha_present = [0u8; 1];
        if socket.read_exact(&mut tar_sha_present).await.is_err() {
            fail_and_emit(
                &peer_tx,
                &sender_id,
                &filename,
                true,
                is_sync,
                None,
                "接收失败：目录归档校验缺失".to_string(),
            );
            return;
        }
        if tar_sha_present[0] == 1 {
            let mut tar_sha_buf = [0u8; 32];
            if socket.read_exact(&mut tar_sha_buf).await.is_err() {
                fail_and_emit(
                    &peer_tx,
                    &sender_id,
                    &filename,
                    true,
                    is_sync,
                    None,
                    "接收失败：目录归档校验读取失败".to_string(),
                );
                return;
            }
            expected_tar_sha256 = Some(hex::encode(tar_sha_buf));
        }
    }

    eprintln!(
        "[rx] header from {addr} id={sender_id} name={filename} is_dir={is_dir} size={total_size} sha256={:?} manifest={}",
        expected_sha256.as_ref().map(|s| &s[..8]),
        expected_manifest.is_some()
    );

    let initial_status = if is_dir {
        "正在接收目录...".to_string()
    } else if total_size > 0 {
        format!("正在接收 0% / {}", human_size(total_size))
    } else {
        "正在接收...".to_string()
    };
    emit_progress(
        &peer_tx,
        Some(sender_id.clone()),
        filename.clone(),
        0.0,
        initial_status,
        true,
        is_dir,
        None,
        is_sync,
        false,
        false,
    );

    let base_dir = default_download_dir();
    let mapped_path = if is_sync {
        let map = load_receive_map();
        let key = receive_map_key(&sender_id, is_dir, &filename);
        map.get(&key).map(PathBuf::from)
    } else {
        None
    };

    let transfer_root = if is_dir {
        mapped_path
            .as_ref()
            .and_then(|path| path.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| {
                let sub_dir_name = format!(
                    "rustle_{}_{}",
                    safe_filename,
                    Local::now().format("%H%M%S")
                );
                base_dir.join(sub_dir_name)
            })
    } else {
        mapped_path
            .as_ref()
            .and_then(|path| path.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| {
                let sub_dir_name = format!(
                    "rustle_{}_{}",
                    safe_filename,
                    Local::now().format("%H%M%S")
                );
                base_dir.join(sub_dir_name)
            })
    };
    let _ = fs::create_dir_all(&transfer_root);

    let final_save_path = if let Some(mapped) = mapped_path.clone() {
        mapped
    } else {
        transfer_root.join(&safe_filename)
    };

    let mut success = false;
    let mut final_local_path = final_save_path.clone();
    let mut final_received: u64 = 0;
    let mut last_progress = 0.0f32;
    let mut last_report_instant = Instant::now();
    let mut last_report_bytes: u64 = 0;
    let mut failure_reason: Option<String> = None;

    if is_dir {
        let staging_root = transfer_root.join(format!(".staging-{}", Uuid::new_v4()));
        let archive_path = staging_root.join("incoming.tar");
        let extract_root = staging_root.join("extract");
        let _ = fs::create_dir_all(&extract_root);

        if let Ok(mut file) = tokio::fs::File::create(&archive_path).await {
            let mut buf = vec![0u8; 64 * 1024];
            let mut received = 0u64;
            loop {
                match socket.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if file.write_all(&buf[..n]).await.is_err() {
                            failure_reason = Some("接收失败：写入目录归档失败".to_string());
                            break;
                        }
                        received += n as u64;
                        if total_size > 0 {
                            let progress = (received as f32 / total_size as f32).min(1.0);
                            let now = Instant::now();
                            if (progress - last_progress >= 0.05
                                && now.duration_since(last_report_instant).as_millis() >= 200)
                                || last_report_bytes == 0
                            {
                                emit_progress(
                                    &peer_tx,
                                    Some(sender_id.clone()),
                                    filename.clone(),
                                    progress,
                                    format!(
                                        "正在接收目录 {:.0}% / {} ({})",
                                        progress * 100.0,
                                        human_size(total_size),
                                        format_speed(
                                            received - last_report_bytes,
                                            now.duration_since(last_report_instant)
                                        )
                                    ),
                                    true,
                                    true,
                                    None,
                                    is_sync,
                                    false,
                                    false,
                                );
                                last_progress = progress;
                                last_report_instant = now;
                                last_report_bytes = received;
                            }
                        }
                    }
                    Err(err) => {
                        failure_reason = Some(format!("接收失败：目录数据读取失败 ({err})"));
                        break;
                    }
                }
            }
            final_received = received;

            if failure_reason.is_none() && total_size > 0 && received != total_size {
                failure_reason = Some(format!(
                    "接收失败：目录数据不完整（{received}/{total_size}）"
                ));
            }

            if failure_reason.is_none() {
                emit_progress(
                    &peer_tx,
                    Some(sender_id.clone()),
                    filename.clone(),
                    1.0,
                    "正在解压目录...".to_string(),
                    true,
                    true,
                    None,
                    is_sync,
                    false,
                    false,
                );

                let archive_path_clone = archive_path.clone();
                let extract_root_clone = extract_root.clone();
                let manifest = expected_manifest.clone().unwrap();
                let expected_tar_sha = expected_tar_sha256.clone();
                let unpack_result = tokio::task::spawn_blocking(move || -> io::Result<PathBuf> {
                    if let Some(expected) = expected_tar_sha {
                        let actual = sha256_reader(std::fs::File::open(&archive_path_clone)?)?;
                        if actual != expected {
                            return Err(io::Error::new(
                                ErrorKind::InvalidData,
                                "directory archive digest mismatch",
                            ));
                        }
                    }
                    unpack_folder_archive(&archive_path_clone, &extract_root_clone, &manifest)
                })
                .await;

                match unpack_result {
                    Ok(Ok(staged_root)) => {
                        emit_progress(
                            &peer_tx,
                            Some(sender_id.clone()),
                            filename.clone(),
                            1.0,
                            "正在校验目录...".to_string(),
                            true,
                            true,
                            None,
                            is_sync,
                            false,
                            false,
                        );
                        let manifest = expected_manifest.clone().unwrap();
                        let final_path = final_save_path.clone();
                        let validate_result = tokio::task::spawn_blocking(move || -> io::Result<()> {
                            validate_extracted_folder(&staged_root, &manifest)?;
                            promote_staged_folder(&staged_root, &final_path)
                        })
                        .await;

                        match validate_result {
                            Ok(Ok(())) => {
                                success = true;
                                final_local_path = final_save_path.clone();
                            }
                            Ok(Err(err)) => {
                                failure_reason = Some(format!("接收失败：目录校验失败 ({err})"));
                            }
                            Err(err) => {
                                failure_reason = Some(format!("接收失败：目录校验任务失败 ({err})"));
                            }
                        }
                    }
                    Ok(Err(err)) => {
                        failure_reason = Some(format!("接收失败：目录解压失败 ({err})"));
                    }
                    Err(err) => {
                        failure_reason = Some(format!("接收失败：目录解压任务失败 ({err})"));
                    }
                }
            }
        } else {
            failure_reason = Some("接收失败：无法创建目录暂存文件".to_string());
        }

        let _ = tokio::fs::remove_dir_all(&staging_root).await;
        if !success {
            let _ = tokio::fs::remove_dir_all(&final_save_path).await;
        }
    } else {
        let staged_path = staged_file_path(&transfer_root, &safe_filename);
        if let Some(parent) = staged_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(mut file) = tokio::fs::File::create(&staged_path).await {
        let mut buf = vec![0u8; 64 * 1024];
        let mut received: u64 = 0;
        if !leading_payload.is_empty() {
            if file.write_all(&leading_payload).await.is_err() {
                failure_reason = Some("接收失败：文件预写入失败".to_string());
            } else {
                received = leading_payload.len() as u64;
            }
        }
        while failure_reason.is_none() {
            match socket.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if file.write_all(&buf[..n]).await.is_err() {
                        failure_reason = Some("接收失败：写入文件失败".to_string());
                        break;
                    }
                    received += n as u64;
                    if total_size > 0 {
                        let progress = (received as f32 / total_size as f32).min(1.0);
                        let now = Instant::now();
                        if (progress - last_progress >= 0.05
                            && now.duration_since(last_report_instant).as_millis() >= 200)
                            || last_report_bytes == 0
                        {
                            emit_progress(
                                &peer_tx,
                                Some(sender_id.clone()),
                                filename.clone(),
                                progress,
                                format!(
                                    "正在接收 {:.0}% / {} ({})",
                                    progress * 100.0,
                                    human_size(total_size),
                                    format_speed(
                                        received - last_report_bytes,
                                        now.duration_since(last_report_instant)
                                    )
                                ),
                                true,
                                false,
                                None,
                                is_sync,
                                false,
                                false,
                            );
                            last_progress = progress;
                            last_report_instant = now;
                            last_report_bytes = received;
                        }
                    }
                }
                Err(err) => {
                    failure_reason = Some(format!("接收失败：文件读取失败 ({err})"));
                }
            }
        }
        final_received = received;
        if failure_reason.is_none() && total_size > 0 && received != total_size {
            failure_reason = Some(format!("接收失败：文件大小不匹配（{received}/{total_size}）"));
        }
        if failure_reason.is_none() && expected_sha256.is_some() {
            let received_sha256 = crate::storage::sha256_file(&staged_path);
            if received_sha256 != expected_sha256 {
                failure_reason = Some("接收失败：文件校验失败".to_string());
            }
        }
        if failure_reason.is_none() {
            let staged_clone = staged_path.clone();
            let final_clone = final_save_path.clone();
            match tokio::task::spawn_blocking(move || promote_staged_file(&staged_clone, &final_clone)).await {
                Ok(Ok(())) => {
                    success = true;
                    final_local_path = final_save_path.clone();
                }
                Ok(Err(err)) => {
                    failure_reason = Some(format!("接收失败：文件提交失败 ({err})"));
                }
                Err(err) => {
                    failure_reason = Some(format!("接收失败：文件提交任务失败 ({err})"));
                }
            }
        }
        if !success {
            let staged_cleanup = staged_path.clone();
            let final_cleanup = final_save_path.clone();
            let _ = tokio::task::spawn_blocking(move || {
                cleanup_failed_regular_file(&staged_cleanup, &final_cleanup)
            })
            .await;
        }
        } else {
            failure_reason = Some("接收失败：无法创建暂存文件".to_string());
        }
    }

    if success {
        let mut map = load_receive_map();
        let key = receive_map_key(&sender_id, is_dir, &filename);
        map.insert(
            key,
            final_local_path.to_string_lossy().to_string(),
        );
        save_receive_map(&map);
    }

    let completed_size = if total_size > 0 { total_size } else { final_received };
    let final_status = if success {
        if is_dir {
            format!("目录接收完成并已校验 ({})", human_size(completed_size))
        } else if completed_size > 0 {
            format!("接收完成 ({})", human_size(completed_size))
        } else {
            "接收完成".to_string()
        }
    } else {
        failure_reason.unwrap_or_else(|| "接收失败".to_string())
    };

    emit_progress(
        &peer_tx,
        Some(sender_id),
        filename.clone(),
        1.0,
        final_status.clone(),
        true,
        is_dir,
        if success {
            Some(final_local_path.to_string_lossy().to_string())
        } else {
            None
        },
        is_sync,
        true,
        success,
    );

    send_file_completion_ack(addr.ip(), &filename, is_dir, is_sync, success, &final_status).await;
}

pub async fn handle_outgoing_file(
    my_id: String,
    peer_id: String,
    peer_ip: String,
    tcp_port: u16,
    path: PathBuf,
    is_dir: bool,
    via: Option<String>,
    is_sync: bool,
    peer_tx: Sender<PeerEvent>,
) {
    async fn connect_with_via(
        peer_ip: &str,
        port: u16,
        via: &Option<String>,
    ) -> std::io::Result<TcpStream> {
        let addr_str = format!("{}:{}", peer_ip, port);
        if let Some(via_ip) = via {
            eprintln!("[tx] attempting to connect to {} via interface {}", addr_str, via_ip);
            match tokio::net::TcpSocket::new_v4() {
                Ok(s) => {
                    if let Ok(bind_addr) = format!("{}:0", via_ip).parse::<SocketAddr>() {
                        match s.bind(bind_addr) {
                            Ok(_) => {
                                eprintln!("[tx] successfully bound to {}", bind_addr);
                                match s.connect(addr_str.parse().unwrap()).await {
                                    Ok(stream) => {
                                        eprintln!("[tx] successfully connected to {} via {}", addr_str, via_ip);
                                        Ok(stream)
                                    }
                                    Err(e) => {
                                        eprintln!("[tx] connect failed via {}: {}", via_ip, e);
                                        Err(e)
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("[tx] bind to {} failed: {}, trying direct connect", bind_addr, e);
                                TcpStream::connect(&addr_str).await
                            }
                        }
                    } else {
                        eprintln!("[tx] invalid bind address for via {}, trying direct connect", via_ip);
                        TcpStream::connect(&addr_str).await
                    }
                }
                Err(e) => {
                    eprintln!("[tx] failed to create socket: {}, trying direct connect", e);
                    TcpStream::connect(&addr_str).await
                }
            }
        } else {
            eprintln!("[tx] connecting directly to {}", addr_str);
            TcpStream::connect(&addr_str).await
        }
    }

    let mut used_port = tcp_port;
    let settings = crate::storage::load_settings();
    let max_retries = settings.transfer_retry_count;
    
    let mut socket;
    let mut retry_count = 0;
    
    loop {
        socket = connect_with_via(&peer_ip, tcp_port, &via).await;
        
        if socket.is_ok() {
            break;
        }
        
        retry_count += 1;
        
        if retry_count >= max_retries || max_retries == 0 {
            break;
        }
        
        eprintln!("[tx] connection failed (attempt {}/{}), retrying...", retry_count, max_retries);
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
    
    if socket.is_err() && is_dir && tcp_port == TCP_DIR_PORT {
        let alt_port = TCP_FILE_PORT;
        if alt_port != tcp_port {
            retry_count = 0;
            loop {
                socket = connect_with_via(&peer_ip, alt_port, &via).await;
                
                if socket.is_ok() {
                    used_port = alt_port;
                    eprintln!("[tx] dir connect fallback to file port {}", used_port);
                    break;
                }
                
                retry_count += 1;
                
                if retry_count >= max_retries || max_retries == 0 {
                    break;
                }
                
                eprintln!("[tx] dir connection failed (attempt {}/{}), retrying...", retry_count, max_retries);
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        }
    }

    let addr_str = format!("{}:{}", peer_ip, used_port);

    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let declared_size: u64;
    let mut send_path = path.clone();
    let mut cleanup_path: Option<PathBuf> = None;
    let mut folder_manifest: Option<FolderManifest> = None;
    let mut folder_archive_sha256: Option<String> = None;

    if is_dir {
        let temp_tar = std::env::temp_dir().join(format!("{}.tar", Uuid::new_v4()));
        let path_clone = path.clone();
        let tar_path = temp_tar.clone();
        let filename_clone = filename.clone();

        let res = tokio::task::spawn_blocking(move || {
            build_folder_package(&path_clone, &tar_path, &filename_clone)
        })
        .await;

        match res {
            Ok(Ok(package)) => {
                if let Ok(meta) = tokio::fs::metadata(&temp_tar).await {
                    declared_size = meta.len();
                    send_path = temp_tar.clone();
                    cleanup_path = Some(temp_tar);
                    folder_manifest = Some(package.manifest);
                    folder_archive_sha256 = Some(package.tar_sha256);
                } else {
                    eprintln!("[tx] tar metadata failed for {:?}", temp_tar);
                    emit_progress(
                        &peer_tx,
                        Some(peer_id),
                        filename,
                        1.0,
                        "目录打包失败：无法读取临时归档信息".to_string(),
                        false,
                        is_dir,
                        Some(path.to_string_lossy().to_string()),
                        is_sync,
                        true,
                        false,
                    );
                    return;
                }
            }
            Ok(Err(e)) => {
                eprintln!("[tx] tar build failed for {:?}: {}", path, e);
                let _ = tokio::fs::remove_file(&temp_tar).await;
                emit_progress(
                    &peer_tx,
                    Some(peer_id),
                    filename,
                    1.0,
                    format!("目录打包失败：{e}"),
                    false,
                    is_dir,
                    Some(path.to_string_lossy().to_string()),
                    is_sync,
                    true,
                    false,
                );
                return;
            }
            Err(join_err) => {
                eprintln!("[tx] tar build join error for {:?}: {}", path, join_err);
                let _ = tokio::fs::remove_file(&temp_tar).await;
                emit_progress(
                    &peer_tx,
                    Some(peer_id),
                    filename,
                    1.0,
                    format!("目录打包失败：后台任务异常 ({join_err})"),
                    false,
                    is_dir,
                    Some(path.to_string_lossy().to_string()),
                    is_sync,
                    true,
                    false,
                );
                return;
            }
        }
    } else {
        declared_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    }

    if let Ok(mut socket) = socket {
        let _ = socket.set_nodelay(true);
        emit_progress(
            &peer_tx,
            Some(peer_id.clone()),
            filename.clone(),
            0.0,
            if is_dir {
                "正在打包目录...".to_string()
            } else {
                "发送中...".to_string()
            },
            false,
            is_dir,
            Some(path.to_string_lossy().to_string()),
            is_sync,
            false,
            false,
        );

        let name_bytes = filename.as_bytes();
        let name_len = name_bytes.len() as u16;
        let id_bytes = my_id.as_bytes();
        let id_len = id_bytes.len() as u8;

        let mut header = Vec::new();
        // create metadata record for this outgoing transfer
        let meta_id = Uuid::new_v4().to_string();
        let abs_path = path.to_string_lossy().to_string();
        let modified_time = crate::storage::file_mtime_seconds(&path).unwrap_or(0);
        let sha = match tokio::task::spawn_blocking({
            let send_path = send_path.clone();
            move || crate::storage::sha256_file(&send_path)
        })
        .await
        {
            Ok(s) => s,
            Err(_) => None,
        };
        let meta = Metadata {
            id: meta_id.clone(),
            peer_id: Some(peer_id.clone()),
            peer_ip: Some(peer_ip.clone()),
            abs_path: abs_path.clone(),
            rel_path: None,
            filename: filename.clone(),
            size: declared_size,
            is_dir,
            modified_time,
            sha256: sha.clone(),
            last_synced_sha256: None,
            last_synced_time: None,
            sync_status: crate::model::SyncStatus::ReadyToSend,
            auto_sync_enabled: crate::storage::load_settings().auto_sync_on_send,
        };
        if let Ok(store) = MetadataStore::open_default() {
            let _ = store.insert(&meta);
        }

        header.push(if is_dir { 1 } else { 0 });
        header.push(if is_sync { FLAG_SYNC } else { 0 });
        header.push(id_len);
        header.extend_from_slice(id_bytes);
        header.extend_from_slice(&name_len.to_be_bytes());
        header.extend_from_slice(name_bytes);
        header.extend_from_slice(&declared_size.to_be_bytes());
        
        // 添加 SHA256 校验码（如果有）
        if let Some(ref sha_str) = sha {
            header.push(1); // SHA256 present
            let sha_bytes = hex::decode(sha_str).unwrap_or_default();
            header.extend_from_slice(&sha_bytes); // 32 bytes, padded with zeros if needed
        } else {
            header.push(0); // No SHA256
        }

        if is_dir {
            match folder_manifest.as_ref() {
                Some(manifest) => {
                    let manifest_json = match serde_json::to_vec(manifest) {
                        Ok(bytes) => bytes,
                        Err(err) => {
                            if let Some(p) = cleanup_path.as_ref() {
                                let _ = tokio::fs::remove_file(p).await;
                            }
                            emit_progress(
                                &peer_tx,
                                Some(peer_id),
                                filename,
                                1.0,
                                format!("目录打包失败：无法序列化清单 ({err})"),
                                false,
                                true,
                                Some(path.to_string_lossy().to_string()),
                                is_sync,
                                true,
                                false,
                            );
                            return;
                        }
                    };
                    header.push(1);
                    header.extend_from_slice(&(manifest_json.len() as u32).to_be_bytes());
                    header.extend_from_slice(&manifest_json);
                }
                None => {
                    if let Some(p) = cleanup_path.as_ref() {
                        let _ = tokio::fs::remove_file(p).await;
                    }
                    emit_progress(
                        &peer_tx,
                        Some(peer_id),
                        filename,
                        1.0,
                        "目录打包失败：目录清单缺失".to_string(),
                        false,
                        true,
                        Some(path.to_string_lossy().to_string()),
                        is_sync,
                        true,
                        false,
                    );
                    return;
                }
            }

            if let Some(ref sha_str) = folder_archive_sha256 {
                header.push(1);
                let sha_bytes = hex::decode(sha_str).unwrap_or_default();
                header.extend_from_slice(&sha_bytes);
            } else {
                header.push(0);
            }
        }

        eprintln!("[tx] sending header to {addr_str} id={my_id} peer={peer_id} name={filename} is_dir={is_dir} size={declared_size}");

        if socket.write_all(&header).await.is_ok() {
            match tokio::fs::File::open(&send_path).await {
                Ok(mut file) => {
                    let mut buf = vec![0u8; 64 * 1024];
                    let mut sent_total: u64 = 0;
                    let mut last_progress = 0.0f32;
                    let mut last_report_instant = Instant::now();
                    let mut last_report_bytes: u64 = 0;
                    let mut send_ok = true;

                    loop {
                        match file.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                if socket.write_all(&buf[..n]).await.is_err() {
                                    eprintln!("[tx] write error to {addr_str}");
                                    send_ok = false;
                                    break;
                                }
                                sent_total += n as u64;

                                if declared_size > 0 {
                                    let progress =
                                        (sent_total as f32 / declared_size as f32).min(1.0);
                                    let now = Instant::now();
                                    let elapsed = now.duration_since(last_report_instant);
                                    let elapsed_ms = elapsed.as_millis();
                                    if (progress - last_progress >= 0.05 && elapsed_ms >= 200)
                                        || last_report_bytes == 0
                                    {
                                        let status = format!(
                                            "{} {:.0}% / {} ({})",
                                            if is_dir { "正在发送目录" } else { "发送中" },
                                            progress * 100.0,
                                            human_size(declared_size),
                                            format_speed(sent_total - last_report_bytes, elapsed),
                                        );
                                        emit_progress(
                                            &peer_tx,
                                            Some(peer_id.clone()),
                                            filename.clone(),
                                            progress,
                                            status,
                                            false,
                                            is_dir,
                                            Some(path.to_string_lossy().to_string()),
                                            is_sync,
                                            false,
                                            false,
                                        );
                                        last_progress = progress;
                                        last_report_instant = now;
                                        last_report_bytes = sent_total;
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("[tx] read error from {:?}: {e}", send_path);
                                send_ok = false;
                                break;
                            }
                        }
                    }

                    let _ = socket.shutdown().await;
                    eprintln!("[tx] shutdown write half to {addr_str}");

                    if let Some(p) = cleanup_path.as_ref() {
                        let _ = tokio::fs::remove_file(p).await;
                    }

                    let final_progress = if declared_size > 0 {
                        (sent_total as f32 / declared_size as f32).min(1.0)
                    } else if send_ok {
                        1.0
                    } else {
                        0.0
                    };

                    let send_completed = send_ok && (declared_size == 0 || sent_total >= declared_size);
                    if send_completed {
                        emit_progress(
                            &peer_tx,
                            Some(peer_id),
                            filename,
                            final_progress,
                            if is_dir {
                                "目录发送完成，等待对端确认".to_string()
                            } else {
                                "发送完成，等待对端确认".to_string()
                            },
                            false,
                            is_dir,
                            Some(path.to_string_lossy().to_string()),
                            is_sync,
                            false,
                            false,
                        );
                    } else {
                        emit_progress(
                            &peer_tx,
                            Some(peer_id),
                            filename,
                            final_progress,
                            if is_dir {
                                "目录发送失败".to_string()
                            } else {
                                "发送失败".to_string()
                            },
                            false,
                            is_dir,
                            Some(path.to_string_lossy().to_string()),
                            is_sync,
                            true,
                            false,
                        );
                    }
                }
                Err(e) => {
                    eprintln!("[tx] open send_path {:?} failed: {e}", send_path);
                    if let Some(p) = cleanup_path.as_ref() {
                        let _ = tokio::fs::remove_file(p).await;
                    }
                    emit_progress(
                        &peer_tx,
                        Some(peer_id),
                        filename,
                        1.0,
                        format!("发送失败：无法读取待发送内容 ({e})"),
                        false,
                        is_dir,
                        Some(path.to_string_lossy().to_string()),
                        is_sync,
                        true,
                        false,
                    );
                }
            }
        } else {
            if let Some(p) = cleanup_path.as_ref() {
                let _ = tokio::fs::remove_file(p).await;
            }
            emit_progress(
                &peer_tx,
                Some(peer_id),
                filename,
                1.0,
                "连接失败".to_string(),
                false,
                is_dir,
                Some(path.to_string_lossy().to_string()),
                is_sync,
                true,
                false,
            );
        }
    } else {
        if let Some(p) = cleanup_path {
            let _ = tokio::fs::remove_file(p).await;
        }
        emit_progress(
            &peer_tx,
            Some(peer_id),
            filename,
            1.0,
            "连接失败".to_string(),
            false,
            is_dir,
            Some(path.to_string_lossy().to_string()),
            is_sync,
            true,
            false,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_formats() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1024), "1.00 KB");
        assert_eq!(human_size(1024 * 1024), "1.00 MB");
    }

    #[test]
    fn format_speed_formats() {
        let s = format_speed(1024 * 1024, Duration::from_secs(1));
        assert!(s.contains("MB/s"));
    }

    #[test]
    fn sanitize_filename_strips_paths() {
        assert_eq!(sanitize_filename("dir/evil.txt"), "evil.txt");
        assert_eq!(sanitize_filename(r"..\evil.txt"), "evil.txt");
    }

    #[test]
    fn sanitize_filename_rejects_empty() {
        assert_eq!(sanitize_filename("  "), "unnamed");
        assert_eq!(sanitize_filename(".."), "unnamed");
    }

    #[test]
    fn folder_manifest_includes_empty_dirs_and_files() {
        let base = std::env::temp_dir().join(format!("rustle_manifest_{}", Uuid::new_v4()));
        let root = base.join("folder");
        std::fs::create_dir_all(root.join("nested/empty")).unwrap();
        std::fs::write(root.join("nested/file.txt"), b"hello").unwrap();

        let entries = collect_folder_manifest(&root, "folder").unwrap();
        let names: Vec<String> = entries.into_iter().map(|(_, e)| e.path).collect();

        assert!(names.contains(&"nested".to_string()));
        assert!(names.contains(&"nested/empty".to_string()));
        assert!(names.contains(&"nested/file.txt".to_string()));

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn unpack_folder_archive_rejects_normalized_path_collision() {
        let base = std::env::temp_dir().join(format!("rustle_unpack_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let tar_path = base.join("bad.tar");
        let tar_file = std::fs::File::create(&tar_path).unwrap();
        let mut builder = tar::Builder::new(tar_file);

        for entry_name in ["folder/dup.txt", "folder/DUP.txt"] {
            let mut header = tar::Header::new_gnu();
            let data = b"oops";
            header.set_path(entry_name).unwrap();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, &data[..]).unwrap();
        }
        builder.finish().unwrap();

        let manifest = FolderManifest {
            root_name: "folder".to_string(),
            entries: vec![],
            file_count: 0,
            dir_count: 0,
            total_bytes: 0,
        };

        let err = unpack_folder_archive(&tar_path, &base.join("out"), &manifest).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn validate_extracted_folder_rejects_digest_mismatch() {
        let base = std::env::temp_dir().join(format!("rustle_validate_{}", Uuid::new_v4()));
        let root = base.join("folder");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("file.txt"), b"actual").unwrap();

        let manifest = FolderManifest {
            root_name: "folder".to_string(),
            entries: vec![FolderManifestEntry {
                path: "file.txt".to_string(),
                entry_type: FolderManifestEntryType::File,
                size: 6,
                sha256: Some("deadbeef".to_string()),
            }],
            file_count: 1,
            dir_count: 0,
            total_bytes: 6,
        };

        let err = validate_extracted_folder(&root, &manifest).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn promote_staged_file_moves_file_into_place() {
        let base = std::env::temp_dir().join(format!("rustle_stage_file_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let staged = base.join("file.part");
        let final_path = base.join("file.txt");
        std::fs::write(&staged, b"hello").unwrap();

        promote_staged_file(&staged, &final_path).unwrap();

        assert!(!staged.exists());
        assert_eq!(std::fs::read(&final_path).unwrap(), b"hello");

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn cleanup_failed_regular_file_removes_temp_and_partial_target() {
        let base = std::env::temp_dir().join(format!("rustle_cleanup_file_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let staged = base.join("file.part");
        let final_path = base.join("file.txt");
        std::fs::write(&staged, b"temp").unwrap();
        std::fs::write(&final_path, b"partial").unwrap();

        cleanup_failed_regular_file(&staged, &final_path);

        assert!(!staged.exists());
        assert!(!final_path.exists());

        let _ = std::fs::remove_dir_all(base);
    }
}

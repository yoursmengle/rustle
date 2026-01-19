use crate::model::PeerEvent;
use crate::storage::{default_download_dir, load_receive_map, save_receive_map, windows_long_path};
use chrono::Local;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

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
    format!("{}|{}|{}", sender_id, if is_dir { "dir" } else { "file" }, filename)
}

pub async fn handle_incoming_file(mut socket: TcpStream, addr: SocketAddr, peer_tx: Sender<PeerEvent>) {
    let _ = socket.set_nodelay(true);
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
    let is_sync = (flag_buf[0] & 0x01) == 0x01;

    let mut id_len_buf = [0u8; 1];
    if socket.read_exact(&mut id_len_buf).await.is_err() {
        debug_println!("[rx] failed to read id_len from {addr}");
        return;
    }
    let id_len = id_len_buf[0] as usize;

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

    let mut name_buf = vec![0u8; name_len];
    if socket.read_exact(&mut name_buf).await.is_err() {
        eprintln!("[rx] failed to read name bytes from {addr}");
        return;
    }
    let filename = String::from_utf8_lossy(&name_buf).to_string();

    let mut size_buf = [0u8; 8];
    if socket.read_exact(&mut size_buf).await.is_err() {
        eprintln!("[rx] failed to read size bytes from {addr}");
        return;
    }
    let total_size = u64::from_be_bytes(size_buf);

    eprintln!("[rx] header from {addr} id={sender_id} name={filename} is_dir={is_dir} size={total_size}");

    let initial_status = if total_size > 0 {
        format!("正在接收 0% / {}", human_size(total_size))
    } else {
        "正在接收...".to_string()
    };

    let _ = peer_tx.send(PeerEvent::FileProgress {
        peer_id: Some(sender_id.clone()),
        file_name: filename.clone(),
        progress: 0.0,
        status: initial_status,
        is_incoming: true,
        is_dir,
        local_path: None,
        is_sync,
    });

    let base_dir = default_download_dir();
    let mut mapped_path: Option<PathBuf> = None;
    if is_sync {
        let map = load_receive_map();
        let key = receive_map_key(&sender_id, is_dir, &filename);
        if let Some(p) = map.get(&key) {
            mapped_path = Some(PathBuf::from(p));
        }
    }

    let (sub_dir, save_path) = if let Some(mapped) = mapped_path {
        if is_dir {
            let parent = mapped.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| base_dir.clone());
            let _ = fs::create_dir_all(&parent);
            let _ = fs::create_dir_all(&mapped);
            (parent, mapped)
        } else {
            if let Some(parent) = mapped.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let parent = mapped.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| base_dir.clone());
            (parent, mapped)
        }
    } else {
        // 无论文件还是文件夹，都创建一个独立的子目录
        let sub_dir_name = format!("rustle_{}_{}", filename, Local::now().format("%H%M%S"));
        let sub_dir = base_dir.join(sub_dir_name);
        let _ = fs::create_dir_all(&sub_dir);
        let save_path = sub_dir.join(&filename);
        (sub_dir, save_path)
    };

    let mut success = false;
    let mut last_progress = 0.0f32;
    let mut final_received: u64 = 0;
    let mut last_report_instant = Instant::now();
    let mut last_report_bytes: u64 = 0;

    if is_dir {
        let tar_path = save_path.with_extension("tar");
        if let Ok(mut file) = tokio::fs::File::create(&tar_path).await {
            let mut buf = vec![0u8; 64 * 1024];
            let mut received: u64 = 0;
            loop {
                match socket.read(&mut buf).await {
                    Ok(0) => {
                        eprintln!("[rx] dir EOF after {received} bytes (target {total_size})");
                        break;
                    }
                    Ok(n) => {
                        if file.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                        received += n as u64;
                        if total_size > 0 {
                            let progress = (received as f32 / total_size as f32).min(1.0);
                            let now = Instant::now();
                            let elapsed_ms = now.duration_since(last_report_instant).as_millis();
                            if (progress - last_progress >= 0.05 && elapsed_ms >= 200) || last_report_bytes == 0 {
                                let status = format!(
                                    "正在接收 {:.0}% / {} ({})",
                                    progress * 100.0,
                                    human_size(total_size),
                                    format_speed(received - last_report_bytes, now.duration_since(last_report_instant)),
                                );
                                let _ = peer_tx.send(PeerEvent::FileProgress {
                                    peer_id: Some(sender_id.clone()),
                                    file_name: filename.clone(),
                                    progress,
                                    status,
                                    is_incoming: true,
                                    is_dir,
                                    local_path: None,
                                    is_sync,
                                });
                                last_progress = progress;
                                last_report_instant = now;
                                last_report_bytes = received;
                            }
                            if received >= total_size {
                                eprintln!("[rx] dir reached declared size {received}/{total_size}");
                                success = true;
                                break;
                            }
                        } else if received % (8 * 1024 * 1024) == 0 {
                            eprintln!("[rx] dir received {received} bytes (size unknown)");
                        }
                    }
                    Err(e) => {
                        eprintln!("[rx] dir read error after {received}: {e}");
                        break;
                    }
                }
            }

            if success || (total_size == 0 && received > 0) {
                let tar_path_clone = tar_path.clone();
                let unpack_dst = sub_dir.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(f) = std::fs::File::open(&tar_path_clone) {
                        let mut ar = tar::Archive::new(f);
                        let _ = ar.unpack(&unpack_dst);
                    }
                })
                .await;
                success = true;
                eprintln!("[rx] dir unpacked into {:?}", save_path);
            }
            final_received = received;
            let _ = tokio::fs::remove_file(&tar_path).await;
        } else {
            eprintln!("[rx] failed to create tar temp file at {:?}", tar_path);
        }
    } else if let Ok(mut file) = tokio::fs::File::create(&save_path).await {
        let mut buf = vec![0u8; 64 * 1024];
        let mut received: u64 = 0;
        loop {
            match socket.read(&mut buf).await {
                Ok(0) => {
                    eprintln!("[rx] file EOF after {received} bytes (target {total_size})");
                    break;
                }
                Ok(n) => {
                    if file.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                    received += n as u64;
                    if total_size > 0 {
                        let progress = (received as f32 / total_size as f32).min(1.0);
                        let now = Instant::now();
                        let elapsed_ms = now.duration_since(last_report_instant).as_millis();
                        if (progress - last_progress >= 0.05 && elapsed_ms >= 200) || last_report_bytes == 0 {
                            let status = format!(
                                "正在接收 {:.0}% / {} ({})",
                                progress * 100.0,
                                human_size(total_size),
                                format_speed(received - last_report_bytes, now.duration_since(last_report_instant)),
                            );
                            let _ = peer_tx.send(PeerEvent::FileProgress {
                                peer_id: Some(sender_id.clone()),
                                file_name: filename.clone(),
                                progress,
                                status,
                                is_incoming: true,
                                is_dir,
                                local_path: None,
                                is_sync,
                            });
                            last_progress = progress;
                            last_report_instant = now;
                            last_report_bytes = received;
                        }
                        if received >= total_size {
                            eprintln!("[rx] file reached declared size {received}/{total_size}");
                            success = true;
                            break;
                        }
                    } else if received % (8 * 1024 * 1024) == 0 {
                        eprintln!("[rx] file received {received} bytes (size unknown)");
                    }
                }
                Err(e) => {
                    eprintln!("[rx] file read error after {received}: {e}");
                    break;
                }
            }
        }
        if total_size == 0 && received > 0 {
            success = true;
        }
        final_received = received;
    } else {
        eprintln!("[rx] failed to create file at {:?}", save_path);
    }

    let completed_size = if total_size > 0 { total_size } else { final_received };
    let status = if success {
        if completed_size > 0 {
            format!("接收完成 ({})", human_size(completed_size))
        } else {
            "接收完成".to_string()
        }
    } else {
        "接收失败".to_string()
    };
    if success {
        let mut map = load_receive_map();
        let key = receive_map_key(&sender_id, is_dir, &filename);
        map.insert(key, save_path.to_string_lossy().to_string());
        save_receive_map(&map);
    }

    let _ = peer_tx.send(PeerEvent::FileProgress {
        peer_id: Some(sender_id),
        file_name: filename,
        progress: 1.0,
        status,
        is_incoming: true,
        is_dir,
        local_path: Some(save_path.to_string_lossy().to_string()),
        is_sync,
    });
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
    let addr_str = format!("{}:{}", peer_ip, tcp_port);
    let socket = if let Some(via_ip) = via {
        match tokio::net::TcpSocket::new_v4() {
            Ok(s) => {
                if let Ok(bind_addr) = format!("{}:0", via_ip).parse::<SocketAddr>() {
                    let _ = s.bind(bind_addr);
                }
                s.connect(addr_str.parse().unwrap()).await
            }
            Err(_) => TcpStream::connect(&addr_str).await,
        }
    } else {
        TcpStream::connect(&addr_str).await
    };

    let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let declared_size: u64;
    let mut send_path = path.clone();
    let mut cleanup_path: Option<PathBuf> = None;

    if is_dir {
        let temp_tar = std::env::temp_dir().join(format!("{}.tar", Uuid::new_v4()));
        let path_clone = path.clone();
        let tar_path = temp_tar.clone();
        let filename_clone = filename.clone();

        let res = tokio::task::spawn_blocking(move || {
            fn add_dir_to_tar(
                builder: &mut tar::Builder<std::fs::File>,
                src: &Path,
                base_name: &str,
                skipped: &mut usize,
                processed: &mut usize,
                last_pause: &mut Instant,
            ) -> std::io::Result<()> {
                let read_dir = std::fs::read_dir(src)?;
                for entry in read_dir {
                    let entry = match entry {
                        Ok(e) => e,
                        Err(_) => {
                            *skipped += 1;
                            continue;
                        }
                    };
                    let path = entry.path();
                    let rel = match path.strip_prefix(src) {
                        Ok(r) => r,
                        Err(_) => {
                            *skipped += 1;
                            continue;
                        }
                    };
                    let tar_path = Path::new(base_name).join(rel);
                    if path.is_dir() {
                        if builder.append_dir(&tar_path, &path).is_err() {
                            *skipped += 1;
                        }
                        if add_dir_to_tar(builder, &path, base_name, skipped, processed, last_pause).is_err() {
                            *skipped += 1;
                        }
                    } else if path.is_file() {
                        if builder.append_path_with_name(&path, &tar_path).is_err() {
                            *skipped += 1;
                        }
                    }

                    *processed += 1;
                    if *processed % 200 == 0 && last_pause.elapsed() >= Duration::from_millis(25) {
                        std::thread::sleep(Duration::from_millis(2));
                        *last_pause = Instant::now();
                    }
                }
                Ok(())
            }

            let tar_fs_path = windows_long_path(&tar_path);
            let src_fs_path = windows_long_path(&path_clone);

            let file = std::fs::File::create(&tar_fs_path)?;
            let mut builder = tar::Builder::new(file);
            let mut skipped = 0usize;
            let mut processed = 0usize;
            let mut last_pause = Instant::now();
            add_dir_to_tar(
                &mut builder,
                &src_fs_path,
                &filename_clone,
                &mut skipped,
                &mut processed,
                &mut last_pause,
            )?;
            if skipped > 0 {
                eprintln!("[tx] tar build skipped {} entries due to read/permission errors", skipped);
            }
            builder.finish()?;
            Ok::<(), std::io::Error>(())
        })
        .await;

        match res {
            Ok(Ok(())) => {
                if let Ok(meta) = tokio::fs::metadata(&temp_tar).await {
                    declared_size = meta.len();
                    send_path = temp_tar.clone();
                    cleanup_path = Some(temp_tar);
                } else {
                    eprintln!("[tx] tar metadata failed for {:?}", temp_tar);
                    let _ = peer_tx.send(PeerEvent::FileProgress {
                        peer_id: Some(peer_id),
                        file_name: filename,
                        progress: 0.0,
                        status: "打包目录失败".to_string(),
                        is_incoming: false,
                        is_dir,
                        local_path: Some(path.to_string_lossy().to_string()),
                        is_sync,
                    });
                    return;
                }
            }
            Ok(Err(e)) => {
                eprintln!("[tx] tar build failed for {:?}: {}", path, e);
                let _ = tokio::fs::remove_file(&temp_tar).await;
                let _ = peer_tx.send(PeerEvent::FileProgress {
                    peer_id: Some(peer_id),
                    file_name: filename,
                    progress: 0.0,
                    status: "打包目录失败".to_string(),
                    is_incoming: false,
                    is_dir,
                    local_path: Some(path.to_string_lossy().to_string()),
                    is_sync,
                });
                return;
            }
            Err(join_err) => {
                eprintln!("[tx] tar build join error for {:?}: {}", path, join_err);
                let _ = tokio::fs::remove_file(&temp_tar).await;
                let _ = peer_tx.send(PeerEvent::FileProgress {
                    peer_id: Some(peer_id),
                    file_name: filename,
                    progress: 0.0,
                    status: "打包目录失败".to_string(),
                    is_incoming: false,
                    is_dir,
                    local_path: Some(path.to_string_lossy().to_string()),
                    is_sync,
                });
                return;
            }
        }
    } else {
        declared_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    }

    if let Ok(mut socket) = socket {
        let _ = socket.set_nodelay(true);
        let _ = peer_tx.send(PeerEvent::FileProgress {
            peer_id: Some(peer_id.clone()),
            file_name: filename.clone(),
            progress: 0.0,
            status: "发送中...".to_string(),
            is_incoming: false,
            is_dir,
            local_path: Some(path.to_string_lossy().to_string()),
            is_sync,
        });

        let name_bytes = filename.as_bytes();
        let name_len = name_bytes.len() as u16;
        let id_bytes = my_id.as_bytes();
        let id_len = id_bytes.len() as u8;

        let mut header = Vec::new();
        header.push(if is_dir { 1 } else { 0 });
        header.push(if is_sync { 1 } else { 0 });
        header.push(id_len);
        header.extend_from_slice(id_bytes);
        header.extend_from_slice(&name_len.to_be_bytes());
        header.extend_from_slice(name_bytes);
        header.extend_from_slice(&declared_size.to_be_bytes());

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
                                    let progress = (sent_total as f32 / declared_size as f32).min(1.0);
                                    let now = Instant::now();
                                    let elapsed = now.duration_since(last_report_instant);
                                    let elapsed_ms = elapsed.as_millis();
                                    if (progress - last_progress >= 0.05 && elapsed_ms >= 200) || last_report_bytes == 0 {
                                        let status = format!(
                                            "发送中 {:.0}% / {} ({})",
                                            progress * 100.0,
                                            human_size(declared_size),
                                            format_speed(sent_total - last_report_bytes, elapsed),
                                        );
                                        let _ = peer_tx.send(PeerEvent::FileProgress {
                                            peer_id: Some(peer_id.clone()),
                                            file_name: filename.clone(),
                                            progress,
                                            status,
                                            is_incoming: false,
                                            is_dir,
                                            local_path: Some(path.to_string_lossy().to_string()),
                                            is_sync,
                                        });
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

                    let status_text = if send_ok && (declared_size == 0 || sent_total >= declared_size) {
                        "发送完成".to_string()
                    } else {
                        "发送失败".to_string()
                    };

                    let _ = peer_tx.send(PeerEvent::FileProgress {
                        peer_id: Some(peer_id),
                        file_name: filename,
                        progress: final_progress,
                        status: status_text,
                        is_incoming: false,
                        is_dir,
                        local_path: Some(path.to_string_lossy().to_string()),
                        is_sync,
                    });
                }
                Err(e) => {
                    eprintln!("[tx] open send_path {:?} failed: {e}", send_path);
                    if let Some(p) = cleanup_path.as_ref() {
                        let _ = tokio::fs::remove_file(p).await;
                    }
                    let _ = peer_tx.send(PeerEvent::FileProgress {
                        peer_id: Some(peer_id),
                        file_name: filename,
                        progress: 0.0,
                        status: "发送失败".to_string(),
                        is_incoming: false,
                        is_dir,
                        local_path: Some(path.to_string_lossy().to_string()),
                        is_sync,
                    });
                }
            }
        } else {
            if let Some(p) = cleanup_path.as_ref() {
                let _ = tokio::fs::remove_file(p).await;
            }
            let _ = peer_tx.send(PeerEvent::FileProgress {
                peer_id: Some(peer_id),
                file_name: filename,
                progress: 0.0,
                status: "连接失败".to_string(),
                is_incoming: false,
                is_dir,
                local_path: Some(path.to_string_lossy().to_string()),
                is_sync,
            });
        }
    } else {
        if let Some(p) = cleanup_path {
            let _ = tokio::fs::remove_file(p).await;
        }
        let _ = peer_tx.send(PeerEvent::FileProgress {
            peer_id: Some(peer_id),
            file_name: filename,
            progress: 0.0,
            status: "连接失败".to_string(),
            is_incoming: false,
            is_dir,
            local_path: Some(path.to_string_lossy().to_string()),
            is_sync,
        });
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
}
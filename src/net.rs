use crate::model::{
    AckPayload, ChatPayload, DiscoveredPeer, FileCmd, HelloMsg, NetCmd, PeerEvent,
    TCP_DIR_PORT, TCP_FILE_PORT, UDP_DISCOVERY_PORT, UDP_MESSAGE_PORT,
};
use crate::storage::load_or_init_node_id;
use crate::transfer::{handle_incoming_file, handle_outgoing_file};
use chrono::Local;
use get_if_addrs::get_if_addrs;
use serde_json;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

pub fn spawn_network_worker(peer_tx: Sender<PeerEvent>, cmd_rx: Receiver<NetCmd>, initial_name: Option<String>) {
    thread::spawn(move || {
        // Stable node id: prefer motherboard UUID; fallback to random UUID (no file persistence)
        let my_id = load_or_init_node_id();
        let my_id_clone = my_id.clone();

        // Initialize Tokio runtime
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (file_cmd_tx, mut file_cmd_rx) = tokio::sync::mpsc::channel::<FileCmd>(32);

        // Bind fixed TCP listeners: 44517 for files, 44518 for directories
        let tcp_file_listener = rt
            .block_on(async { TcpListener::bind((Ipv4Addr::UNSPECIFIED, TCP_FILE_PORT)).await })
            .expect("Failed to bind TCP file listener");

        let tcp_dir_listener = rt
            .block_on(async { TcpListener::bind((Ipv4Addr::UNSPECIFIED, TCP_DIR_PORT)).await })
            .expect("Failed to bind TCP directory listener");

        let peer_tx_clone = peer_tx.clone();
        rt.spawn(async move {
            let listener = tcp_file_listener;
            loop {
                if let Ok((socket, addr)) = listener.accept().await {
                    let tx = peer_tx_clone.clone();
                    tokio::spawn(async move {
                        handle_incoming_file(socket, addr, tx).await;
                    });
                }
            }
        });

        let peer_tx_clone = peer_tx.clone();
        rt.spawn(async move {
            let listener = tcp_dir_listener;
            loop {
                if let Ok((socket, addr)) = listener.accept().await {
                    let tx = peer_tx_clone.clone();
                    tokio::spawn(async move {
                        handle_incoming_file(socket, addr, tx).await;
                    });
                }
            }
        });

        let peer_tx_clone2 = peer_tx.clone();
        rt.spawn(async move {
            while let Some(cmd) = file_cmd_rx.recv().await {
                match cmd {
                    FileCmd::SendFile {
                        peer_id,
                        peer_ip,
                        tcp_port,
                        path,
                        is_dir,
                        via,
                        is_sync,
                    } => {
                        handle_outgoing_file(
                            my_id_clone.clone(),
                            peer_id,
                            peer_ip,
                            tcp_port,
                            path,
                            is_dir,
                            via,
                            is_sync,
                            peer_tx_clone2.clone(),
                        )
                        .await;
                    }
                }
            }
        });

        // 在每个非 loopback IPv4 接口上绑定 discovery 与 chat sockets（固定端口）
        let mut discovery_sockets: Vec<(UdpSocket, Ipv4Addr)> = Vec::new();
        let mut chat_sockets: Vec<(UdpSocket, Ipv4Addr)> = Vec::new();

        if let Ok(ifaces) = get_if_addrs() {
            for iface in ifaces {
                if let get_if_addrs::IfAddr::V4(v4) = iface.addr {
                    let ipv4 = v4.ip;
                    if ipv4.is_loopback() || ipv4.is_link_local() {
                        continue; // 跳过 loopback 与 link-local 接口
                    }

                    // 跳过 /32 或无广播的点对点接口，避免 Windows 返回 10049
                    let nm = v4.netmask.octets();
                    let is_host_mask = nm == [255, 255, 255, 255];
                    if is_host_mask || v4.broadcast.is_none() {
                        continue;
                    }

                    // 发现端口（广播用）
                    match UdpSocket::bind((ipv4, UDP_DISCOVERY_PORT)) {
                        Ok(sock) => {
                            let _ = sock.set_broadcast(true);
                            let _ = sock.set_nonblocking(true);
                            discovery_sockets.push((sock, ipv4));
                            eprintln!("Net discovery: bound discovery interface {ipv4}:{UDP_DISCOVERY_PORT}");
                            let _ = peer_tx.send(PeerEvent::LocalBound {
                                ip: ipv4.to_string(),
                                port: UDP_DISCOVERY_PORT,
                            });
                        }
                        Err(e) => {
                            if e.raw_os_error() == Some(10049) {
                                eprintln!("Net discovery: skip unusable iface {ipv4} (os error 10049)");
                            } else {
                                eprintln!(
                                    "Net discovery: failed to bind discovery {ipv4}:{UDP_DISCOVERY_PORT} - {e}"
                                );
                            }
                        }
                    }

                    // 消息端口（收发聊天与 ACK）
                    match UdpSocket::bind((ipv4, UDP_MESSAGE_PORT)) {
                        Ok(sock) => {
                            let _ = sock.set_nonblocking(true);
                            chat_sockets.push((sock, ipv4));
                            eprintln!("Net discovery: bound chat interface {ipv4}:{UDP_MESSAGE_PORT}");
                            let _ = peer_tx.send(PeerEvent::LocalBound {
                                ip: ipv4.to_string(),
                                port: UDP_MESSAGE_PORT,
                            });
                        }
                        Err(e) => {
                            if e.raw_os_error() == Some(10049) {
                                eprintln!("Net discovery: skip unusable iface {ipv4} (os error 10049)");
                            } else {
                                eprintln!(
                                    "Net discovery: failed to bind chat {ipv4}:{UDP_MESSAGE_PORT} - {e}"
                                );
                            }
                        }
                    }
                }
            }
        }

        // 如果没有找到任何接口绑定，尝试兜底绑定 0.0.0.0，避免因单个异常地址导致完全不可用
        if discovery_sockets.is_empty() {
            match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, UDP_DISCOVERY_PORT)) {
                Ok(sock) => {
                    let _ = sock.set_broadcast(true);
                    let _ = sock.set_nonblocking(true);
                    discovery_sockets.push((sock, Ipv4Addr::UNSPECIFIED));
                    eprintln!("Net discovery: bound fallback discovery 0.0.0.0:{UDP_DISCOVERY_PORT}");
                    let _ = peer_tx.send(PeerEvent::LocalBound {
                        ip: Ipv4Addr::UNSPECIFIED.to_string(),
                        port: UDP_DISCOVERY_PORT,
                    });
                }
                Err(e) => {
                    eprintln!(
                        "Net discovery: discovery sockets unavailable and fallback 0.0.0.0:{UDP_DISCOVERY_PORT} failed - {e}"
                    );
                    return;
                }
            }
        }

        if chat_sockets.is_empty() {
            match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, UDP_MESSAGE_PORT)) {
                Ok(sock) => {
                    let _ = sock.set_nonblocking(true);
                    chat_sockets.push((sock, Ipv4Addr::UNSPECIFIED));
                    eprintln!("Net discovery: bound fallback chat 0.0.0.0:{UDP_MESSAGE_PORT}");
                    let _ = peer_tx.send(PeerEvent::LocalBound {
                        ip: Ipv4Addr::UNSPECIFIED.to_string(),
                        port: UDP_MESSAGE_PORT,
                    });
                }
                Err(e) => {
                    eprintln!(
                        "Net discovery: chat sockets unavailable and fallback 0.0.0.0:{UDP_MESSAGE_PORT} failed - {e}"
                    );
                    return;
                }
            }
        }

        // 汇总日志，便于排查为何看不到节点
        if discovery_sockets.is_empty() {
            eprintln!("Net discovery: no discovery UDP sockets bound; discovery will not work");
        } else {
            let summary: Vec<String> = discovery_sockets
                .iter()
                .map(|(_, ip)| format!("{}:{}", ip, UDP_DISCOVERY_PORT))
                .collect();
            eprintln!("Net discovery: discovery UDP sockets -> {}", summary.join(", "));
        }
        if chat_sockets.is_empty() {
            eprintln!("Net discovery: no chat UDP sockets bound; messaging will not work");
        } else {
            let summary: Vec<String> = chat_sockets
                .iter()
                .map(|(_, ip)| format!("{}:{}", ip, UDP_MESSAGE_PORT))
                .collect();
            eprintln!("Net discovery: chat UDP sockets -> {}", summary.join(", "));
        }

        let mut our_name = initial_name.unwrap_or_default();
        // 记录最近收到消息/hello 的时间，用于判定在线与节流查询
        let mut last_from_peer: HashMap<String, Instant> = HashMap::new();
        // 记录最近见过的 peers 的地址（用于定向探测，端口固定为 UDP_DISCOVERY_PORT）
        let mut peers_seen: HashMap<String, (IpAddr, u16)> = HashMap::new();

        // 发送 hello 的函数（包含 our id）
        let send_hello =
            |sock: &UdpSocket, target: SocketAddr, name: &str, my_id: &str, is_probe: bool| {
                let msg = HelloMsg {
                    msg_type: "hello".to_string(),
                    id: my_id.to_string(),
                    name: if name.is_empty() {
                        None
                    } else {
                        Some(name.to_string())
                    },
                    port: UDP_MESSAGE_PORT,
                    tcp_port: Some(TCP_FILE_PORT),
                    version: "0.1".to_string(),
                    is_reply: false,
                    is_probe,
                };
                if let Ok(payload) = serde_json::to_vec(&msg) {
                    if let Err(e) = sock.send_to(&payload, target) {
                        eprintln!(
                            "Net discovery: failed send hello from {}:{} to {} ({})",
                            sock.local_addr()
                                .map(|a| a.ip())
                                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
                            UDP_DISCOVERY_PORT,
                            target,
                            e
                        );
                    } else {
                        eprintln!(
                            "Net discovery: sent hello from {}:{} to {}",
                            sock.local_addr()
                                .map(|a| a.ip())
                                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
                            UDP_DISCOVERY_PORT,
                            target
                        );
                    }
                }
            };

        // 构建目标地址列表：对每个有效接口计算定向广播地址并发送到该地址
        // 广播目标列表：仅全局广播（不使用 loopback），固定 UDP_DISCOVERY_PORT
        let mut base_targets: Vec<SocketAddr> = vec![SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)),
            UDP_DISCOVERY_PORT,
        )];

        // 针对每个接口计算定向广播地址（根据 netmask），过滤 loopback/link-local/无广播接口
        if let Ok(ifaces) = get_if_addrs() {
            for iface in ifaces {
                if let get_if_addrs::IfAddr::V4(v4) = iface.addr {
                    let ipv4 = v4.ip;
                    if ipv4.is_loopback() || ipv4.octets()[0] == 169 {
                        continue;
                    }
                    let nm = v4.netmask.octets();
                    let is_host_mask = nm == [255, 255, 255, 255];
                    if is_host_mask || v4.broadcast.is_none() {
                        continue; // 点对点或无广播接口
                    }

                    // 计算接口的广播地址：优先使用常见私网掩码规则，如果有特殊子网需改进可再接入系统 netmask
                    fn heuristic_broadcast(ipv4: Ipv4Addr) -> Ipv4Addr {
                        let oct = ipv4.octets();
                        // 10.0.0.0/8 -> broadcast 10.255.255.255
                        if oct[0] == 10 {
                            return Ipv4Addr::new(10, 255, 255, 255);
                        }
                        // 172.16.0.0 - 172.31.255.255 -> /12 mask 255.240.0.0
                        if oct[0] == 172 && (16..=31).contains(&oct[1]) {
                            let first = oct[0];
                            let second = oct[1] | (!0xF0u8);
                            return Ipv4Addr::new(first, second, 255, 255);
                        }
                        // 192.168.x.x -> /24
                        if oct[0] == 192 && oct[1] == 168 {
                            return Ipv4Addr::new(oct[0], oct[1], oct[2], 255);
                        }
                        // otherwise fallback to /24
                        Ipv4Addr::new(oct[0], oct[1], oct[2], 255)
                    }

                    let bcast_ip = heuristic_broadcast(ipv4);

                    base_targets.push(SocketAddr::new(IpAddr::V4(bcast_ip), UDP_DISCOVERY_PORT));
                }
            }
        }

        // 去重
        base_targets.sort_by_key(|a| (a.ip().to_string(), a.port()));
        base_targets.dedup();

        if base_targets.is_empty() {
            eprintln!("Net discovery: no broadcast targets computed");
        } else {
            let targets: Vec<String> = base_targets.iter().map(|t| t.to_string()).collect();
            eprintln!("Net discovery: broadcast targets -> {}", targets.join(", "));
        }

        // 广播间隔：60 秒；定向查询间隔：5 秒
        let mut last_broadcast = Instant::now();
        let mut last_direct_probe = Instant::now();

        // 启动时立即发送一轮广播 hello，提高首次发现速度
        for (sock, _ip) in &discovery_sockets {
            for addr in &base_targets {
                send_hello(sock, *addr, &our_name, &my_id, true);
            }
        }

        let mut buf = [0u8; 2048];

        loop {
            // 处理命令（非阻塞）
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    NetCmd::ChangeName(new_name) => {
                        our_name = new_name;
                        // 立刻广播（从每个 socket 发出）
                        for (sock, _ip) in &discovery_sockets {
                            for addr in &base_targets {
                                send_hello(sock, *addr, &our_name, &my_id, true);
                            }
                        }
                    }
                    NetCmd::SendFile {
                        peer_id,
                        ip,
                        tcp_port,
                        path,
                        is_dir,
                        via,
                        is_sync,
                    } => {
                        let _ = rt.block_on(file_cmd_tx.send(FileCmd::SendFile {
                            peer_id,
                            peer_ip: ip,
                            tcp_port,
                            path,
                            is_dir,
                            via,
                            is_sync,
                        }));
                    }
                    NetCmd::ProbePeer { ip, via } => {
                        if let Ok(ipaddr) = ip.parse::<IpAddr>() {
                            if let IpAddr::V4(v4) = ipaddr {
                                // 仅同网段探测，避免跨网噪声
                                let send_ok = discovery_sockets.iter().any(|(_, lip)| {
                                    let lp = lip.octets();
                                    let vp = v4.octets();
                                    lp[0] == vp[0] && lp[1] == vp[1] && lp[2] == vp[2]
                                });
                                if !send_ok {
                                    continue;
                                }
                            }
                            let target = SocketAddr::new(ipaddr, UDP_DISCOVERY_PORT);
                            eprintln!("Probing known peer {}", target);
                            for (sock, _ip) in &discovery_sockets {
                                if let Some(v) = &via {
                                    if _ip.to_string() != *v {
                                        continue;
                                    }
                                }
                                send_hello(sock, target, &our_name, &my_id, false);
                            }
                        }
                    }
                    NetCmd::SendChat { ip, text, ts, via, msg_id } => {
                        // 发送消息时使用指定的接口，如果没有指定则使用所有绑定的 sockets
                        let payload = ChatPayload {
                            msg_type: "chat".to_string(),
                            msg_id: msg_id.clone(),
                            from_id: my_id.clone(),
                            from_name: if our_name.is_empty() {
                                None
                            } else {
                                Some(our_name.clone())
                            },
                            text: text.clone(),
                            timestamp: ts.clone(),
                        };
                        if let Ok(data) = serde_json::to_vec(&payload) {
                            let target = match ip.parse::<IpAddr>() {
                                Ok(ipaddr) => SocketAddr::new(ipaddr, UDP_MESSAGE_PORT),
                                Err(_) => continue,
                            };
                            eprintln!("Sending chat to {}: {:?}", target, payload);

                            let mut sent = false;
                            // 如果指定了接口，只使用该接口发送
                            if let Some(v) = &via {
                                for (sock, _ip) in &chat_sockets {
                                    if _ip.to_string() == *v {
                                        if let Err(e) = sock.send_to(&data, target) {
                                            eprintln!(
                                                "Net discovery: failed to send chat from {}:{} to {} - {}",
                                                _ip, UDP_MESSAGE_PORT, target, e
                                            );
                                        } else {
                                            eprintln!(
                                                "Net discovery: sent chat from {}:{} to {} ({} bytes)",
                                                _ip,
                                                UDP_MESSAGE_PORT,
                                                target,
                                                data.len()
                                            );
                                            sent = true;
                                        }
                                        break;
                                    }
                                }
                            } else {
                                // 没有指定接口，使用所有同网段的绑定接口发送
                                for (sock, _ip) in &chat_sockets {
                                    // 只从同网段的接口发送
                                    let local_octets = _ip.octets();
                                    if let IpAddr::V4(target_v4) = target.ip() {
                                        let target_octets = target_v4.octets();
                                        if local_octets[0] == target_octets[0]
                                            && local_octets[1] == target_octets[1]
                                            && local_octets[2] == target_octets[2]
                                        {
                                            if let Err(e) = sock.send_to(&data, target) {
                                                eprintln!(
                                                    "Net discovery: failed to send chat from {}:{} to {} - {}",
                                                    _ip, UDP_MESSAGE_PORT, target, e
                                                );
                                            } else {
                                                eprintln!(
                                                    "Net discovery: sent chat from {}:{} to {} ({} bytes)",
                                                    _ip,
                                                    UDP_MESSAGE_PORT,
                                                    target,
                                                    data.len()
                                                );
                                                sent = true;
                                            }
                                        }
                                    }
                                }
                            }

                            if !sent {
                                eprintln!(
                                    "Net discovery: failed to send chat to {} - no suitable interface found",
                                    target
                                );
                            }
                        }
                    }
                }
            }

            // 周期性广播（60s）
            if last_broadcast.elapsed() > Duration::from_secs(60) {
                for (sock, _ip) in &discovery_sockets {
                    for addr in &base_targets {
                        send_hello(sock, *addr, &our_name, &my_id, true);
                    }
                }
                last_broadcast = Instant::now();
            }

            // 周期性定向查询（5s）：对已知节点单播 hello；若 5 秒内收过对方消息则跳过
            if last_direct_probe.elapsed() > Duration::from_secs(5) {
                let now = Instant::now();
                for (pid, (ip, port)) in peers_seen.iter() {
                    if let Some(last_seen) = last_from_peer.get(pid) {
                        if now.duration_since(*last_seen) <= Duration::from_secs(5) {
                            continue; // 5 秒内收过消息，不查询
                        }
                        if now.duration_since(*last_seen) > Duration::from_secs(120) {
                            continue; // 太久未见，暂不骚扰
                        }
                    }
                    let target = SocketAddr::new(*ip, *port);
                    for (sock, _ip) in &discovery_sockets {
                        let msg = HelloMsg {
                            msg_type: "hello".to_string(),
                            id: my_id.clone(),
                            name: if our_name.is_empty() {
                                None
                            } else {
                                Some(our_name.clone())
                            },
                            port: UDP_MESSAGE_PORT,
                            tcp_port: Some(TCP_FILE_PORT),
                            version: "0.1".to_string(),
                            is_reply: false,
                            is_probe: false,
                        };
                        if let Ok(payload) = serde_json::to_vec(&msg) {
                            let _ = sock.send_to(&payload, target);
                        }
                    }
                }
                last_direct_probe = Instant::now();
            }

            // 接收来自所有 sockets（非阻塞）
            for (sock, _ip) in discovery_sockets.iter().chain(chat_sockets.iter()) {
                loop {
                    match sock.recv_from(&mut buf) {
                        Ok((n, src)) => {
                            if src.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST)
                                || src.ip().to_string() == _ip.to_string()
                            {
                                continue;
                            }
                            let local_port = sock.local_addr().map(|a| a.port()).unwrap_or(0);
                            eprintln!(
                                "Net discovery: recv {} bytes on {}:{} from {}",
                                n, _ip, local_port, src
                            );
                            if let Ok(text) = std::str::from_utf8(&buf[..n]) {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                                    if let Some(mt) = v.get("msg_type").and_then(|m| m.as_str()) {
                                        match mt {
                                            "hello" => {
                                                if let Ok(h) = serde_json::from_value::<HelloMsg>(v) {
                                                    if h.id != my_id {
                                                        let pid = if h.id.is_empty() {
                                                            src.ip().to_string()
                                                        } else {
                                                            h.id.clone()
                                                        };
                                                        let now = Instant::now();
                                                        last_from_peer.insert(pid.clone(), now);
                                                        peers_seen.insert(
                                                            pid.clone(),
                                                            (src.ip(), UDP_DISCOVERY_PORT),
                                                        );

                                                        let peer = DiscoveredPeer {
                                                            id: pid.clone(),
                                                            ip: src.ip().to_string(),
                                                            port: UDP_MESSAGE_PORT,
                                                            tcp_port: Some(TCP_FILE_PORT),
                                                            name: h.name.clone(),
                                                        };
                                                        let _ = peer_tx
                                                            .send(PeerEvent::Discovered(peer, _ip.to_string()));

                                                        // 回复策略：
                                                        // - 来自在线用户的广播查询（is_probe=true 且最近 15s 内有活动）不回复
                                                        // - 其他情况回复（包含定向查询）
                                                        let recently_active = last_from_peer
                                                            .get(&pid)
                                                            .map(|ts| now.duration_since(*ts) < Duration::from_secs(15))
                                                            .unwrap_or(false);

                                                        let should_reply =
                                                            !h.is_reply && (!h.is_probe || !recently_active);
                                                        if should_reply {
                                                            let target =
                                                                SocketAddr::new(src.ip(), UDP_DISCOVERY_PORT);
                                                            let reply = HelloMsg {
                                                                msg_type: "hello".to_string(),
                                                                id: my_id.clone(),
                                                                name: if our_name.is_empty() {
                                                                    None
                                                                } else {
                                                                    Some(our_name.clone())
                                                                },
                                                                port: UDP_MESSAGE_PORT,
                                                                tcp_port: Some(TCP_FILE_PORT),
                                                                version: "0.1".to_string(),
                                                                is_reply: true,
                                                                is_probe: false,
                                                            };
                                                            let _ = sock.send_to(
                                                                &serde_json::to_vec(&reply).unwrap(),
                                                                target,
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                            "chat" => {
                                                if let Ok(c) =
                                                    serde_json::from_value::<ChatPayload>(v)
                                                {
                                                    // 屏蔽自身广播回环
                                                    if c.from_id == my_id {
                                                        continue;
                                                    }
                                                    last_from_peer
                                                        .insert(c.from_id.clone(), Instant::now());
                                                    eprintln!("Received chat from {}: {:?}", src, c);
                                                    // 回复 ACK 到发送端的监听端口（固定 UDP_MESSAGE_PORT）
                                                    let ack = AckPayload {
                                                        msg_type: "ack".to_string(),
                                                        msg_id: c.msg_id.clone(),
                                                        from_id: my_id.clone(),
                                                    };
                                                    if let Ok(ack_data) = serde_json::to_vec(&ack) {
                                                        // 同时发送到源端口和标准端口，确保 ACK 能被接收
                                                        let ack_target_std =
                                                            SocketAddr::new(src.ip(), UDP_MESSAGE_PORT);
                                                        eprintln!(
                                                            "Sending ACK for msg_id={} to {} and {}",
                                                            c.msg_id, src, ack_target_std
                                                        );
                                                        let _ = sock.send_to(&ack_data, src);
                                                        if src.port() != UDP_MESSAGE_PORT {
                                                            let _ = sock.send_to(&ack_data, ack_target_std);
                                                        }
                                                    }

                                                    let _ = peer_tx.send(PeerEvent::ChatReceived {
                                                        from_id: c.from_id.clone(),
                                                        from_ip: src.ip().to_string(),
                                                        from_port: src.port(),
                                                        text: c.text.clone(),
                                                        send_ts: c.timestamp.clone(),
                                                        recv_ts: Local::now()
                                                            .format("%Y-%m-%d %H:%M:%S")
                                                            .to_string(),
                                                        msg_id: c.msg_id.clone(),
                                                        local_ip: _ip.to_string(),
                                                    });
                                                }
                                            }
                                            "ack" => {
                                                if let Ok(a) = serde_json::from_value::<AckPayload>(v) {
                                                    if a.from_id == my_id {
                                                        continue;
                                                    }
                                                    last_from_peer
                                                        .insert(a.from_id.clone(), Instant::now());
                                                    eprintln!("Received ACK for msg_id={} from {}", a.msg_id, src);
                                                    let _ = peer_tx.send(PeerEvent::ChatAck {
                                                        from_id: a.from_id,
                                                        msg_id: a.msg_id,
                                                    });
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            // Windows UDP can surface ConnectionReset (os error 10054) when a peer replies ICMP port unreachable.
                            match e.kind() {
                                ErrorKind::WouldBlock => break,
                                ErrorKind::ConnectionReset => continue,
                                _ => {
                                    eprintln!("Net discovery recv error: {}", e);
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            // 避免忙循环
            thread::sleep(Duration::from_millis(10));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration as StdDuration;

    #[test]
    #[ignore = "binds fixed ports and runs an infinite loop; run manually in isolation"]
    fn spawn_network_worker_smoke() {
        let (peer_tx, _peer_rx) = mpsc::channel();
        let (_cmd_tx, cmd_rx) = mpsc::channel();
        spawn_network_worker(peer_tx, cmd_rx, None);
        std::thread::sleep(StdDuration::from_millis(50));
    }
}

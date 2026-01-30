use std::io;
use std::net::UdpSocket;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const NTP_UNIX_EPOCH_DIFF_SECS: i64 = 2_208_988_800;

pub fn sync_system_time_at_startup() {
    let servers = [
        "time.windows.com:123",
        "pool.ntp.org:123",
        "ntp.aliyun.com:123",
    ];

    for server in servers {
        match query_ntp_time(server) {
            Ok(ntp_time) => {
                let local_time = SystemTime::now();
                
                // 计算时间差异
                let time_diff = match ntp_time.duration_since(local_time) {
                    Ok(duration) => duration.as_secs_f64(),
                    Err(err) => -err.duration().as_secs_f64(),
                };
                
                // 只提示时间差异，不进行同步（避免需要管理员权限）
                if time_diff.abs() < 5.0 {
                    crate::debug_println!("Time sync: time difference is {:.2}s, no sync needed", time_diff);
                } else {
                    crate::debug_println!("Time sync: time difference is {:.2}s (consider syncing manually if needed)", time_diff);
                }
                return;
            }
            Err(err) => {
                crate::debug_println!("Time sync: failed to query {server}: {err}");
            }
        }
    }
}

fn query_ntp_time(server: &str) -> io::Result<SystemTime> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;

    let mut request = [0u8; 48];
    request[0] = 0x1B; // LI=0, VN=3, Mode=3 (client)

    socket.send_to(&request, server)?;

    let mut response = [0u8; 48];
    socket.recv_from(&mut response)?;

    parse_ntp_response(&response)
}

fn parse_ntp_response(response: &[u8; 48]) -> io::Result<SystemTime> {
    let secs = u32::from_be_bytes([response[40], response[41], response[42], response[43]]);
    let frac = u32::from_be_bytes([response[44], response[45], response[46], response[47]]);

    let unix_secs = secs as i64 - NTP_UNIX_EPOCH_DIFF_SECS;
    if unix_secs < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "NTP time is before Unix epoch",
        ));
    }

    let nanos = (frac as u128 * 1_000_000_000u128) / 4_294_967_296u128;
    Ok(UNIX_EPOCH + Duration::new(unix_secs as u64, nanos as u32))
}

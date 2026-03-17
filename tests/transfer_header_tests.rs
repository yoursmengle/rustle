use std::fs;
use std::io::Write;
use uuid::Uuid;

#[test]
fn header_roundtrip() {
    fn build_header(is_dir: bool, is_sync: bool, id: &str, name: &str, size: u64) -> Vec<u8> {
        let mut header = Vec::new();
        header.push(if is_dir { 1 } else { 0 });
        header.push(if is_sync { 1 } else { 0 });
        let id_bytes = id.as_bytes();
        header.push(id_bytes.len() as u8);
        header.extend_from_slice(id_bytes);
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len() as u16;
        header.extend_from_slice(&name_len.to_be_bytes());
        header.extend_from_slice(name_bytes);
        header.extend_from_slice(&size.to_be_bytes());
        header
    }

    fn parse_header(buf: &[u8]) -> Option<(bool, bool, String, String, u64)> {
        let mut i = 0usize;
        if buf.len() < 1 + 1 + 1 + 2 + 8 {
            return None;
        }
        let t = buf[i];
        i += 1;
        let flags = buf[i];
        i += 1;
        let is_dir = t == 1;
        let is_sync = (flags & 0x01) == 0x01;
        let id_len = buf[i] as usize;
        i += 1;
        if buf.len() < i + id_len + 2 {
            return None;
        }
        let id = String::from_utf8_lossy(&buf[i..i + id_len]).to_string();
        i += id_len;
        let name_len = u16::from_be_bytes([buf[i], buf[i + 1]]) as usize;
        i += 2;
        if buf.len() < i + name_len + 8 {
            return None;
        }
        let name = String::from_utf8_lossy(&buf[i..i + name_len]).to_string();
        i += name_len;
        let size = u64::from_be_bytes([
            buf[i],
            buf[i + 1],
            buf[i + 2],
            buf[i + 3],
            buf[i + 4],
            buf[i + 5],
            buf[i + 6],
            buf[i + 7],
        ]);
        Some((is_dir, is_sync, id, name, size))
    }

    let hdr = build_header(true, true, "sender", "file.txt", 12345);
    let parsed = parse_header(&hdr).expect("parse");
    assert_eq!(parsed.0, true);
    assert_eq!(parsed.1, true);
    assert_eq!(parsed.2, "sender");
    assert_eq!(parsed.3, "file.txt");
    assert_eq!(parsed.4, 12345);

    let hdr2 = build_header(false, false, "", "", 0);
    let parsed2 = parse_header(&hdr2).expect("parse2");
    assert_eq!(parsed2.0, false);
    assert_eq!(parsed2.1, false);
    assert_eq!(parsed2.2, "");
    assert_eq!(parsed2.3, "");
    assert_eq!(parsed2.4, 0);
}

#[test]
fn header_roundtrip_with_folder_manifest_extension() {
    let manifest = serde_json::json!({
        "root_name": "folder",
        "entries": [
            {"path": "nested", "entry_type": "directory", "size": 0},
            {"path": "nested/file.txt", "entry_type": "file", "size": 5, "sha256": "abc"}
        ],
        "file_count": 1,
        "dir_count": 1,
        "total_bytes": 5
    });
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();

    let mut header = Vec::new();
    header.push(1);
    header.push(1);
    header.push(6);
    header.extend_from_slice(b"sender");
    header.extend_from_slice(&(6u16).to_be_bytes());
    header.extend_from_slice(b"folder");
    header.extend_from_slice(&(77u64).to_be_bytes());
    header.push(0);
    header.push(1);
    header.extend_from_slice(&(manifest_bytes.len() as u32).to_be_bytes());
    header.extend_from_slice(&manifest_bytes);
    header.push(1);
    header.extend_from_slice(&[7u8; 32]);

    let mut i = 0usize;
    assert_eq!(header[i], 1);
    i += 1;
    assert_eq!(header[i], 1);
    i += 1;
    let id_len = header[i] as usize;
    i += 1;
    assert_eq!(&header[i..i + id_len], b"sender");
    i += id_len;
    let name_len = u16::from_be_bytes([header[i], header[i + 1]]) as usize;
    i += 2;
    assert_eq!(&header[i..i + name_len], b"folder");
    i += name_len;
    let size = u64::from_be_bytes(header[i..i + 8].try_into().unwrap());
    i += 8;
    assert_eq!(size, 77);
    assert_eq!(header[i], 0);
    i += 1;
    assert_eq!(header[i], 1);
    i += 1;
    let manifest_len = u32::from_be_bytes(header[i..i + 4].try_into().unwrap()) as usize;
    i += 4;
    let decoded: serde_json::Value = serde_json::from_slice(&header[i..i + manifest_len]).unwrap();
    i += manifest_len;
    assert_eq!(decoded["root_name"], "folder");
    assert_eq!(header[i], 1);
    i += 1;
    assert_eq!(&header[i..i + 32], &[7u8; 32]);
}

#[test]
fn tar_pack_unpack_tempdir() {
    let base = std::env::temp_dir().join(format!("rustle_test_{}", Uuid::new_v4()));
    let src = base.join("srcdir");
    fs::create_dir_all(&src).unwrap();
    let file_path = src.join("hello.txt");
    {
        let mut f = fs::File::create(&file_path).unwrap();
        writeln!(f, "hello world").unwrap();
    }

    let tar_path = base.join("out.tar");
    {
        let file = fs::File::create(&tar_path).unwrap();
        let mut builder = tar::Builder::new(file);
        builder
            .append_path_with_name(&file_path, "hello.txt")
            .unwrap();
        builder.finish().unwrap();
    }

    let out_dir = base.join("outdir");
    fs::create_dir_all(&out_dir).unwrap();
    {
        let file = fs::File::open(&tar_path).unwrap();
        let mut ar = tar::Archive::new(file);
        ar.unpack(&out_dir).unwrap();
    }

    let unpacked = fs::read_to_string(out_dir.join("hello.txt")).unwrap();
    assert!(unpacked.contains("hello world"));
}

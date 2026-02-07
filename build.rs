fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("rustle.ico");

        // 设置应用程序为 Windows GUI 应用程序，避免控制台窗口
        res.set_manifest_file("installer/rustle.exe.manifest");

        // Set version info from Cargo package version (e.g. 1.2.0 => 1.2.0.0).
        let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "1.0.0".to_string());
        let mut parts = pkg_version.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
        let major = parts.next().unwrap_or(0) as u64;
        let minor = parts.next().unwrap_or(0) as u64;
        let patch = parts.next().unwrap_or(0) as u64;
        let build = 0u64;
        let ver = (major << 48) | (minor << 32) | (patch << 16) | build;
        res.set_version_info(winres::VersionInfo::PRODUCTVERSION, ver);
        res.set_version_info(winres::VersionInfo::FILEVERSION, ver);

        res.compile().unwrap();
    }
}

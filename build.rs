fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("rustle.ico");
        
        // 设置应用程序为 Windows GUI 应用程序，避免控制台窗口
        res.set_manifest_file("installer/rustle.exe.manifest");
        
        // 设置版本信息
        res.set_version_info(winres::VersionInfo::PRODUCTVERSION, 0x0001000000000000);
        res.set_version_info(winres::VersionInfo::FILEVERSION, 0x0001000000000000);
        
        res.compile().unwrap();
    }
}

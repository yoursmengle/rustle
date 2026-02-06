- [x] Verify that the copilot-instructions.md file in the .github directory is created.
- [x] Clarify Project Requirements
- [x] Scaffold the Project
- [x] Customize the Project
- [x] Install Required Extensions
- [x] Compile the Project
- [x] Create and Run Task
- [x] Launch the Project
- [x] Ensure Documentation is Complete

## 项目概况
Rustle (如梭) 是一个使用 Rust 编写的局域网通讯工具，具备节点发现、可靠文本聊天、文件/文件夹传输等功能。

## 核心特性
- UDP 广播发现节点
- 带有 ACK 确认和重发机制的可靠聊天
- 基于 TCP 的高效文件/文件夹传输（支持进度条）
- 支持发送离线消息，对方上线后自动接收
- 自动保存聊天历史
- 自动同步文件夹
- 支持历史文件和文件夹删除和撤回，撤回后，对方也会删除对应文件和文件夹
- 适配中文显示

## 运行说明
使用 `cargo run` 启动程序。首次运行需输入用户名。

## 构建与测试
- 开发运行：`cargo run`
- Release 构建：`cargo build --release`
- Windows 一键发布/安装包（含 MSVC 环境准备）：`.\build_release.ps1`（可选 `-Clean`）
- 全量测试：`cargo test`
- 单个测试：`cargo test <test_name>`（例如 `cargo test human_size_formats`）

## 高层架构
- `main.rs` 启动入口：启动时做 NTP 时间检查（仅提示差异，不修改系统时间），随后进入 `ui::run()`。
- `ui.rs` 负责 egui 桌面界面与状态管理，启动时加载 `me.txt`、`settings.json`、历史记录与同步树，并通过 channel 与网络线程通信。
- `net.rs` 负责网络发现与消息收发：UDP 广播发现/聊天，TCP 监听文件与目录传输；通过 `NetCmd`/`PeerEvent` 与 UI 交互。
- `transfer.rs` 负责文件/目录传输：目录先 tar 打包再发送，接收端解包；传输进度通过 `PeerEvent::FileProgress` 上报。
- `storage.rs` 负责本地持久化（数据目录、历史记录、同步树、设置）及 Windows 长路径处理。
- `model.rs` 定义协议数据结构与核心类型（端口常量、NetCmd/PeerEvent、payload）。

## 关键约定
- 本地数据目录：`LOCALAPPDATA\Rustle`（或 `APPDATA`/临时目录），用户名文件为 `me.txt`。
- 接收目录默认优先 `D:\rustle_downloads`，否则 `C:\rustle_downloads`；可在 `settings.json` 覆盖。
- 历史记录为每联系人一份 JSONL：`history\<peer_id>.jsonl`；同步树保存为 `sync_tree.json`。
- 端口约定：UDP discovery `44517`、UDP chat `44518`；TCP file `44517`、TCP dir `44518`。
- 目录传输必须走 tar 打包；Windows 上使用 `windows_long_path()` 处理长路径。


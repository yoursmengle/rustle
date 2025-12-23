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
- 自动保存聊天历史
- 适配中文显示

## 运行说明
使用 `cargo run` 启动程序。首次运行需输入用户名。


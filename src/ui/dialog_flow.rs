use super::RustleApp;
use crate::model::NetCmd;
use crate::storage::{data_path, save_settings};
use eframe::egui;
use rfd::FileDialog;
use std::fs;
use std::sync::mpsc;

impl RustleApp {
    pub(super) fn render_settings_dialog(&mut self, ctx: &egui::Context) {
        let mut show_settings = self.show_settings_window;
        if show_settings {
            egui::Window::new("设置")
                .open(&mut show_settings)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    let mut changed = false;

                    ui.label("历史记录保存天数");
                    let mut days = self.settings.history_days.max(1);
                    if ui
                        .add(egui::DragValue::new(&mut days).range(1..=3650))
                        .changed()
                    {
                        self.settings.history_days = days;
                        changed = true;
                    }

                    ui.add_space(6.0);
                    ui.label("接收文件夹");
                    ui.horizontal(|ui| {
                        let resp = ui.text_edit_singleline(&mut self.settings_recv_dir_input);
                        if resp.changed() {
                            changed = true;
                        }
                        let mut pending_folder_select = false;
                        if ui.button("选择...").clicked() {
                            pending_folder_select = true;
                        }
                        if pending_folder_select {
                            let (tx, rx) = mpsc::channel();
                            std::thread::spawn(move || {
                                let path = FileDialog::new().pick_folder();
                                let _ = tx.send(path);
                            });
                            if let Ok(Some(path)) = rx.recv() {
                                self.settings_recv_dir_input = path.to_string_lossy().to_string();
                                changed = true;
                            }
                        }
                        if ui.button("恢复默认").clicked() {
                            let def = crate::storage::default_download_dir();
                            self.settings_recv_dir_input = def.to_string_lossy().to_string();
                            changed = true;
                        }
                    });

                    ui.add_space(6.0);
                    if ui
                        .checkbox(
                            &mut self.settings.auto_sync_on_send,
                            "发送文件/文件夹自动同步",
                        )
                        .changed()
                    {
                        changed = true;
                    }

                    if ui
                        .checkbox(&mut self.settings.auto_check_update, "启动时检查更新")
                        .changed()
                    {
                        changed = true;
                    }

                    ui.add_space(6.0);
                    ui.label("首选发送网卡");
                    let mut interfaces: Vec<String> =
                        self.bound_interfaces.iter().cloned().collect();
                    interfaces.sort();
                    let current_label = self
                        .settings
                        .preferred_interface
                        .as_deref()
                        .unwrap_or("自动");
                    egui::ComboBox::from_id_salt("preferred_iface")
                        .selected_text(current_label)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_value(
                                    &mut self.settings.preferred_interface,
                                    None,
                                    "自动",
                                )
                                .clicked()
                            {
                                changed = true;
                            }
                            for iface in interfaces {
                                if ui
                                    .selectable_value(
                                        &mut self.settings.preferred_interface,
                                        Some(iface.clone()),
                                        iface.clone(),
                                    )
                                    .clicked()
                                {
                                    changed = true;
                                }
                            }
                        });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.label("本机显示名称");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.settings_name_input));
                        if ui.button("保存名称").clicked() {
                            let name = self.settings_name_input.trim();
                            if !name.is_empty() {
                                if fs::write(data_path("me.txt"), name).is_ok() {
                                    self.me_name = Some(name.to_string());
                                    if let Some(tx) = &self.net_cmd_tx {
                                        let _ = tx.send(NetCmd::ChangeName(
                                            self.me_name.clone().unwrap_or_default(),
                                        ));
                                    }
                                    if let Some(name) = self.me_name.clone() {
                                        self.send_name_update_to_all(&name);
                                    }
                                }
                            }
                        }
                    });

                    if changed {
                        let recv = self.settings_recv_dir_input.trim().to_string();
                        self.settings.recv_dir = if recv.is_empty() { None } else { Some(recv) };
                        if let Some(dir) = self.settings.recv_dir.as_ref() {
                            let _ = fs::create_dir_all(dir);
                        }
                        save_settings(&self.settings);
                    }
                });
            self.show_settings_window = show_settings;
        }
    }

    pub(super) fn render_usage_dialog(&mut self, ctx: &egui::Context) {
        let mut show_usage = self.show_usage_window;
        if show_usage {
            egui::Window::new("使用说明").open(&mut show_usage).show(ctx, |ui| {
                ui.label(egui::RichText::new("Rustle (如梭) 使用说明").heading());
                ui.add_space(10.0);
                ui.label("1. 节点发现：\n   软件启动会自动发现局域网内的其他 Rustle 节点。无需配置。");
                ui.add_space(5.0);
                ui.label("2. 发送消息：\n   点击左侧列表中的用户，在右侧输入框输入文字并回车即可发送。");
                ui.add_space(5.0);
                ui.label("3. 文件传输：\n   直接将文件或文件夹拖入聊天窗口即可发送给当前选中的用户。");
                ui.add_space(5.0);
            });
            self.show_usage_window = show_usage;
        }
    }

    pub(super) fn render_about_dialog(&mut self, ctx: &egui::Context) {
        let mut show_about = self.show_about_window;
        if show_about {
            egui::Window::new("关于")
                .open(&mut show_about)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(egui::RichText::new("Rustle (如梭)").heading());
                    ui.add_space(6.0);
                    ui.label(format!("版本号: {}", crate::APP_VERSION));
                });
            self.show_about_window = show_about;
        }
    }

    pub(super) fn render_update_dialog(&mut self, ctx: &egui::Context) {
        let mut show_update = self.show_update_dialog;
        if show_update {
            egui::Window::new("软件升级")
                .open(&mut show_update)
                .collapsible(false)
                .show(ctx, |ui| {
                    if self.is_checking_update {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("正在检查新版本...");
                        });
                    } else if let Some((ver, url)) = &self.new_version_info {
                        let current = env!("CARGO_PKG_VERSION");
                        let ver_clean = ver.trim_start_matches('v');

                        if ver_clean != current {
                            ui.label(format!("发现新版本: {}", ver));
                            ui.label(format!("当前版本: {}", current));
                            ui.add_space(10.0);
                            if ui.button("前往下载更新").clicked() {
                                let _ = open::that(url);
                            }
                        } else {
                            ui.label("当前已是最新版本。");
                            ui.label(format!("版本: {}", current));
                        }
                    } else if ui.button("检查更新").clicked() {
                        let (tx, rx) = mpsc::channel();
                        self.update_check_rx = Some(rx);
                        self.is_checking_update = true;
                        self.new_version_info = None;
                        super::spawn_check_update(tx);
                    }
                });
            self.show_update_dialog = show_update;
        }
    }

    pub(super) fn render_name_dialogs(&mut self, ctx: &egui::Context) {
        if self.show_name_dialog {
            egui::Window::new("欢迎使用 Rustle (如梭)")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("请输入你的姓名（建议使用真实姓名）：");
                    ui.add_space(6.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.temp_name_input)
                            .hint_text("例如：张三"),
                    );
                    ui.add_space(8.0);
                    if let Some(err) = &self.name_save_error {
                        ui.label(egui::RichText::new(err).color(egui::Color32::RED));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("保存并登录").clicked() {
                            let name = self.temp_name_input.trim();
                            if name.is_empty() {
                                self.name_save_error = Some("请输入名字后再保存".to_string());
                            } else {
                                match fs::write(data_path("me.txt"), name) {
                                    Ok(_) => {
                                        self.me_name = Some(name.to_string());
                                        self.show_name_dialog = false;
                                        self.name_save_error = None;
                                        self.temp_name_input.clear();

                                        if let Some(tx) = &self.net_cmd_tx {
                                            let _ = tx.send(NetCmd::ChangeName(
                                                self.me_name.clone().unwrap_or_default(),
                                            ));
                                        }
                                        if let Some(name) = self.me_name.clone() {
                                            self.send_name_update_to_all(&name);
                                        }
                                    }
                                    Err(e) => {
                                        self.name_save_error = Some(format!("保存失败: {}", e));
                                    }
                                }
                            }
                        }
                        if ui.button("稍后再说").clicked() {
                            self.show_name_dialog = false;
                        }
                    });
                });
        }

        if self.show_edit_name_dialog {
            egui::Window::new("修改用户名")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("请输入新的用户名：");
                    ui.add_space(6.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.edit_name_input)
                            .hint_text("例如：张三"),
                    );
                    ui.add_space(8.0);
                    if let Some(err) = &self.edit_name_error {
                        ui.label(egui::RichText::new(err).color(egui::Color32::RED));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("保存").clicked() {
                            let name = self.edit_name_input.trim();
                            if name.is_empty() {
                                self.edit_name_error = Some("请输入名字后再保存".to_string());
                            } else {
                                match fs::write(data_path("me.txt"), name) {
                                    Ok(_) => {
                                        self.me_name = Some(name.to_string());
                                        self.show_edit_name_dialog = false;
                                        self.edit_name_error = None;

                                        if let Some(tx) = &self.net_cmd_tx {
                                            let _ = tx.send(NetCmd::ChangeName(
                                                self.me_name.clone().unwrap_or_default(),
                                            ));
                                        }
                                        if let Some(name) = self.me_name.clone() {
                                            self.send_name_update_to_all(&name);
                                        }
                                    }
                                    Err(e) => {
                                        self.edit_name_error = Some(format!("保存失败: {}", e));
                                    }
                                }
                            }
                        }
                        if ui.button("取消").clicked() {
                            self.show_edit_name_dialog = false;
                        }
                    });
                });
        }
    }

    pub(super) fn render_sync_manager_dialog(&mut self, ctx: &egui::Context) {
        let mut show_sync = self.show_sync_window;
        let mut open = show_sync;
        egui::Window::new("同步管理")
            .open(&mut open)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("刷新").clicked() {
                        self.reload_meta_list();
                    }
                    if ui.button("全部同步").clicked() {
                        let snapshot = self.meta_list.clone();
                        for meta in snapshot.iter() {
                            if meta.peer_ip.is_some() {
                                self.trigger_sync_for_meta(&meta.id);
                            }
                        }
                    }
                });
                ui.separator();

                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("meta_grid").striped(true).show(ui, |ui| {
                        ui.label("文件名");
                        ui.label("类型");
                        ui.label("路径");
                        ui.label("Peer");
                        ui.label("大小");
                        ui.label("状态");
                        ui.label("自动");
                        ui.label("");
                        ui.end_row();

                        let snapshot = self.meta_list.clone();
                        for meta in snapshot.iter() {
                            ui.label(&meta.filename);
                            ui.label(if meta.is_dir { "目录" } else { "文件" });
                            ui.label(&meta.abs_path);
                            let peer_label =
                                meta.peer_id.clone().unwrap_or_else(|| "-".to_string());
                            ui.label(peer_label);
                            ui.label(format!("{}", meta.size));
                            ui.label(&meta.status);
                            let mut auto = meta.auto_sync_enabled;
                            if ui.add(egui::Checkbox::new(&mut auto, "")).changed() {
                                self.set_meta_auto(&meta.id, auto);
                            }
                            if ui.button("同步").clicked() {
                                self.trigger_sync_for_meta(&meta.id);
                            }
                            ui.end_row();
                        }
                    });
                });
            });
        show_sync = open;
        self.show_sync_window = show_sync;
    }
}

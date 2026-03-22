use super::{theme, RustleApp};
use crate::model::ChatMessage;
use crate::storage::default_download_dir;
use eframe::egui;
use std::path::Path;
use std::time::Instant;

impl RustleApp {
    fn render_contact_user_row(&mut self, ui: &mut egui::Ui, user: &crate::model::User) {
        let selected = self.selected_user_id.as_deref() == Some(&user.id);

        let mut label_text = user.name.clone();
        if user.has_unread {
            label_text.push(' ');
            label_text.push_str("●");
        }

        let text_color = if selected {
            theme::MSG_SENT_TEXT
        } else if user.has_unread {
            theme::STATUS_UNREAD
        } else if user.online {
            theme::SECONDARY_LIGHT
        } else {
            theme::TEXT_LIGHT
        };

        let bg_color = if selected {
            theme::PRIMARY_LIGHT
        } else if user.has_unread {
            theme::BG_SELECTED
        } else {
            egui::Color32::TRANSPARENT
        };

        let text = if user.online {
            egui::RichText::new(label_text).strong().color(text_color)
        } else {
            egui::RichText::new(label_text).color(text_color)
        };

        let mut frame = egui::Frame::none()
            .fill(bg_color)
            .inner_margin(egui::Margin::symmetric(8.0, 6.0))
            .rounding(6.0);

        if selected {
            frame = frame.stroke(egui::Stroke::new(2.0, theme::PRIMARY));
        }

        let resp = frame
            .show(ui, |ui| {
                ui.push_id(&user.id, |ui| {
                    ui.add(egui::SelectableLabel::new(selected, text))
                })
                .inner
            })
            .inner;
        if resp.clicked() {
            self.selected_user_id = Some(user.id.clone());
            if let Some(existing_user) = self
                .users
                .iter_mut()
                .find(|existing_user| existing_user.id == user.id)
            {
                existing_user.has_unread = false;
            }
            self.scroll_to_first_unread = true;
        }

        resp.context_menu(|ui| {
            if ui.button("用户信息").clicked() {
                let ip_info = format!(
                    "IP: {}\nID: {}",
                    user.ip.as_deref().unwrap_or("未知"),
                    user.id
                );
                self.show_ip_dialog = Some((user.name.clone(), ip_info));
                ui.close_menu();
            }
            if ui.button("删除联系人").clicked() {
                self.context_menu_user_id = Some(user.id.clone());
                ui.close_menu();
            }
        });
    }

    pub(super) fn render_contacts_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("contacts")
            .resizable(false)
            .default_width(280.0)
            .frame(egui::Frame::default().fill(theme::BG_PRIMARY))
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("📞 联系人")
                        .heading()
                        .color(theme::TEXT_PRIMARY),
                );
                ui.add_space(10.0);

                if self.users.is_empty() {
                    ui.label(
                        egui::RichText::new("暂无联系人")
                            .weak()
                            .color(theme::TEXT_LIGHT),
                    );
                } else {
                    let mut online_users: Vec<crate::model::User> = self
                        .users
                        .iter()
                        .filter(|user| user.online && user.id != self.self_id)
                        .cloned()
                        .collect();
                    let mut offline_users: Vec<crate::model::User> = self
                        .users
                        .iter()
                        .filter(|user| !user.online && user.id != self.self_id)
                        .cloned()
                        .collect();
                    online_users.sort_by(|a, b| a.name.cmp(&b.name));
                    offline_users.sort_by(|a, b| a.name.cmp(&b.name));

                    let avail = ui.available_height();
                    let online_height = (avail * 0.5).max(120.0).min(avail);
                    let offline_height = (avail - online_height - 12.0).max(80.0);

                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("🟢 在线")
                                .strong()
                                .color(theme::SECONDARY_LIGHT),
                        );
                    });
                    egui::ScrollArea::vertical()
                        .id_salt("online_users")
                        .max_height(online_height)
                        .show(ui, |ui| {
                            for user in &online_users {
                                self.render_contact_user_row(ui, user);
                            }
                        });

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("⚪ 离线")
                                .strong()
                                .color(theme::TEXT_LIGHT),
                        );
                    });
                    egui::ScrollArea::vertical()
                        .id_salt("offline_users")
                        .max_height(offline_height)
                        .show(ui, |ui| {
                            for user in &offline_users {
                                self.render_contact_user_row(ui, user);
                            }
                        });
                }
            });
    }

    fn render_message_list(&mut self, ui: &mut egui::Ui) {
        let msgs: &[ChatMessage] = self
            .selected_user_id
            .as_ref()
            .and_then(|id| self.messages.get(id))
            .map(|messages| messages.as_slice())
            .unwrap_or(&[]);

        let mut last_msg_resp = None;
        let mut first_unread_resp = None;
        let now = Instant::now();

        for (index, msg) in msgs.iter().enumerate() {
            let align = if msg.from_me {
                egui::Align::RIGHT
            } else {
                egui::Align::LEFT
            };
            let max_bubble_width = ui.available_width() * 0.7;
            let resp = ui
                .allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 0.0),
                    egui::Layout::top_down(align),
                    |ui| {
                        ui.set_max_width(max_bubble_width);
                        let mut meta = format!("发送: {}", msg.send_ts);
                        if let Some(recv_ts) = &msg.recv_ts {
                            meta.push_str(&format!("  |  接收: {}", recv_ts));
                        }
                        if let Some(sync_ts) = &msg.last_sync_ts {
                            meta.push_str(&format!("  |  同步: {}", sync_ts));
                        }
                        ui.label(
                            egui::RichText::new(meta)
                                .small()
                                .color(theme::TEXT_LIGHT)
                                .weak(),
                        );

                        let (bg, border_color, fg) = if msg.from_me {
                            if msg.transfer_status.as_deref() == Some("未送达") {
                                (theme::BG_HOVER, theme::BORDER, theme::TEXT_PRIMARY)
                            } else {
                                (theme::MSG_SENT_BG, theme::PRIMARY, theme::MSG_SENT_TEXT)
                            }
                        } else {
                            (
                                theme::MSG_RECV_BG,
                                theme::MSG_RECV_BORDER,
                                theme::MSG_RECV_TEXT,
                            )
                        };

                        egui::Frame::none()
                            .fill(bg)
                            .stroke(egui::Stroke::new(1.5, border_color))
                            .rounding(egui::Rounding::same(10.0))
                            .inner_margin(egui::Margin::symmetric(12.0, 10.0))
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new(&msg.text).color(fg));

                                if let Some(status) = &msg.transfer_status {
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(format!("⚡ {}", status))
                                            .small()
                                            .italics()
                                            .color(if msg.from_me {
                                                theme::TEXT_LIGHT
                                            } else {
                                                theme::STATUS_UNREAD
                                            }),
                                    );
                                }

                                if msg.file_path.is_some() {
                                    let status_label = if msg.needs_sync {
                                        Some("🔄 需同步")
                                    } else if msg.last_sync_ts.is_some() {
                                        Some("✓ 已同步")
                                    } else {
                                        None
                                    };
                                    if let Some(label) = status_label {
                                        ui.add_space(4.0);
                                        ui.label(egui::RichText::new(label).small().color(
                                            if msg.from_me {
                                                theme::TEXT_LIGHT
                                            } else {
                                                theme::SECONDARY_LIGHT
                                            },
                                        ));
                                    }
                                }

                                if let Some(path) = &msg.file_path {
                                    ui.horizontal(|ui| {
                                        if ui.link("📂 打开所在目录").clicked() {
                                            let candidate = Path::new(path);
                                            let target = if candidate.is_absolute() {
                                                candidate
                                                    .parent()
                                                    .map(|parent| parent.to_path_buf())
                                                    .unwrap_or_else(default_download_dir)
                                            } else {
                                                default_download_dir()
                                            };
                                            let _ = open::that(target);
                                        }
                                    });
                                }
                            });
                    },
                )
                .response;

            if !msg.from_me && !msg.is_read {
                if first_unread_resp.is_none() {
                    first_unread_resp = Some(resp.clone());
                }

                if ui.clip_rect().intersects(resp.rect) {
                    if let Some(peer_id) = &self.selected_user_id {
                        let key = (peer_id.clone(), index);
                        self.message_visible_since.entry(key).or_insert(now);
                    }
                }
            }

            if index == msgs.len() - 1 {
                last_msg_resp = Some(resp);
            }
            ui.add_space(10.0);
        }

        if self.scroll_to_bottom {
            if let Some(resp) = last_msg_resp {
                resp.scroll_to_me(Some(egui::Align::Center));
                self.scroll_to_bottom = false;
            }
        } else if self.scroll_to_first_unread {
            if let Some(resp) = first_unread_resp {
                resp.scroll_to_me(Some(egui::Align::TOP));
            } else if let Some(resp) = last_msg_resp {
                resp.scroll_to_me(Some(egui::Align::Center));
            }
            self.scroll_to_first_unread = false;
        }
    }

    pub(super) fn render_messages_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::BG_PRIMARY))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(self.selected_user_name())
                            .heading()
                            .color(theme::TEXT_PRIMARY),
                    );
                });
                ui.separator();

                let total_height = ui.available_height();
                let top_height = total_height * 0.75;

                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), top_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.set_height(top_height);
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.render_message_list(ui));
                    },
                );

                ui.separator();

                let bottom_height = ui.available_height();
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), bottom_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.set_height(bottom_height);
                        ui.spacing_mut().item_spacing.y = 8.0;

                        ui.horizontal(|ui| {
                            ui.horizontal(|ui| {
                                let btn_file = egui::Button::new(
                                    egui::RichText::new("📁 文件")
                                        .size(14.0)
                                        .color(theme::TEXT_PRIMARY),
                                )
                                .fill(theme::BG_HOVER)
                                .stroke(egui::Stroke::new(1.5, theme::BORDER))
                                .min_size(egui::vec2(100.0, 36.0));
                                if ui.add(btn_file).clicked() {
                                    self.pick_and_send(false);
                                }
                                ui.add_space(6.0);

                                let btn_folder = egui::Button::new(
                                    egui::RichText::new("📂 文件夹")
                                        .size(14.0)
                                        .color(theme::TEXT_PRIMARY),
                                )
                                .fill(theme::BG_HOVER)
                                .stroke(egui::Stroke::new(1.5, theme::BORDER))
                                .min_size(egui::vec2(120.0, 36.0));
                                if ui.add(btn_folder).clicked() {
                                    self.pick_and_send(true);
                                }
                                ui.add_space(12.0);
                                ui.label(
                                    egui::RichText::new("支持拖放文件/文件夹到窗口")
                                        .weak()
                                        .small()
                                        .color(theme::TEXT_LIGHT),
                                );
                            });

                            ui.add_space(8.0);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add_space(8.0);
                                    let btn_send = egui::Button::new(
                                        egui::RichText::new("🚀 发送")
                                            .size(14.0)
                                            .color(theme::MSG_SENT_TEXT),
                                    )
                                    .fill(theme::PRIMARY)
                                    .stroke(egui::Stroke::new(1.5, theme::PRIMARY))
                                    .min_size(egui::vec2(100.0, 36.0));
                                    if ui.add(btn_send).clicked()
                                        || ctx.input(|input| input.key_pressed(egui::Key::Enter))
                                    {
                                        self.send_current();
                                    }
                                },
                            );
                        });

                        ui.add_space(8.0);

                        let input_height = ui.available_height();
                        ui.add(
                            egui::TextEdit::multiline(&mut self.input)
                                .hint_text("输入消息...")
                                .desired_width(f32::INFINITY)
                                .min_size(egui::vec2(0.0, input_height)),
                        );
                    },
                );
            });
    }
}

use eframe::egui;

use crate::gui_config::{AppConfig, Subscription};

pub struct AgentLoopApp {
    config: AppConfig,
    new_subscription: Subscription,
    is_running: bool,
    status_message: String,
    available_reporters: Vec<String>,
    available_filters: Vec<String>,
}

impl Default for AgentLoopApp {
    fn default() -> Self {
        Self {
            config: AppConfig::load(),
            new_subscription: Subscription::default(),
            is_running: false,
            status_message: "就绪".to_string(),
            available_reporters: Self::scan_reporters(),
            available_filters: Self::scan_filters(),
        }
    }
}

impl AgentLoopApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::setup_fonts(&cc.egui_ctx);
        Self::default()
    }

    fn setup_fonts(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();

        // 尝试加载系统中文字体
        if let Some(font_data) = Self::load_system_font() {
            fonts.font_data.insert("system_chinese".to_owned(), font_data);

            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "system_chinese".to_owned());

            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("system_chinese".to_owned());

            ctx.set_fonts(fonts);
        }
    }

    fn load_system_font() -> Option<egui::FontData> {
        // macOS 系统字体路径
        let font_paths = [
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/STHeiti Light.ttc",
            "/System/Library/Fonts/STHeiti Medium.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            // Windows 路径
            "C:\\Windows\\Fonts\\msyh.ttc",
            "C:\\Windows\\Fonts\\simhei.ttf",
            // Linux 路径
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
        ];

        for path in &font_paths {
            if let Ok(data) = std::fs::read(path) {
                return Some(egui::FontData::from_owned(data));
            }
        }

        None
    }

    fn save_config(&mut self) {
        if let Err(e) = self.config.save() {
            self.status_message = format!("保存失败: {}", e);
        } else {
            self.status_message = "配置已保存".to_string();
        }
    }

    fn scan_reporters() -> Vec<String> {
        let mut reporters = vec!["console".to_string()];

        if let Some(home) = dirs::home_dir() {
            let reporters_dir = home.join(".agent-loop").join("reporters");
            if let Ok(entries) = std::fs::read_dir(reporters_dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.path().file_stem() {
                        if entry.path().extension().and_then(|s| s.to_str()) == Some("wasm") {
                            reporters.push(name.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }

        reporters.sort();
        reporters.dedup();
        reporters
    }

    fn scan_filters() -> Vec<String> {
        let mut filters = vec!["none".to_string()];

        if let Some(home) = dirs::home_dir() {
            let filters_dir = home.join(".agent-loop").join("filters");
            if let Ok(entries) = std::fs::read_dir(filters_dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.path().file_stem() {
                        if entry.path().extension().and_then(|s| s.to_str()) == Some("wasm") {
                            filters.push(name.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }

        filters.push("regex".to_string());
        filters.sort();
        filters.dedup();
        filters
    }
}

impl eframe::App for AgentLoopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("🔁 Agent Loop - 配置管理");
            ui.separator();

            ui.horizontal(|ui| {
                ui.label("状态:");
                if self.is_running {
                    ui.colored_label(egui::Color32::GREEN, "● 运行中");
                } else {
                    ui.colored_label(egui::Color32::GRAY, "○ 已停止");
                }
                ui.separator();
                ui.label(&self.status_message);
            });
            ui.separator();

            egui::CollapsingHeader::new("🔌 WebSocket 配置")
                .default_open(true)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("cc-connect WS URL:");
                        ui.text_edit_singleline(&mut self.config.ws_url);
                    });
                    ui.horizontal(|ui| {
                        ui.label("默认工作区:");
                        ui.text_edit_singleline(&mut self.config.default_workspace);
                    });
                });

            ui.separator();

            egui::CollapsingHeader::new("📋 订阅列表")
                .default_open(true)
                .show(ui, |ui| {
                    let mut to_remove = None;
                    let mut to_toggle = None;

                    for (idx, sub) in self.config.subscriptions.iter_mut().enumerate() {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                let checkbox_response = ui.checkbox(&mut sub.enabled, "");
                                if checkbox_response.changed() {
                                    to_toggle = Some(idx);
                                }

                                ui.label(format!("📌 {}", sub.name));
                                ui.label(format!("| 工作区: {}", sub.workspace.as_ref()
                                    .unwrap_or(&self.config.default_workspace)));

                                if ui.button("🗑 删除").clicked() {
                                    to_remove = Some(idx);
                                }
                            });

                            ui.label(format!("Smee URL: {}", sub.smee_url));
                            ui.label(format!("提示词: {}", sub.base_prompt));
                            ui.horizontal(|ui| {
                                ui.label(format!("Reporter: {}", sub.reporter.as_deref().unwrap_or("console")));
                                if let Some(filter) = &sub.filter_regex {
                                    ui.label(format!("| Filter: {}", filter));
                                }
                            });
                        });
                    }

                    if let Some(idx) = to_remove {
                        self.config.subscriptions.remove(idx);
                        self.save_config();
                    }

                    if let Some(_idx) = to_toggle {
                        self.save_config();
                    }
                });

            ui.separator();

            egui::CollapsingHeader::new("➕ 添加新订阅")
                .default_open(false)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("名称:");
                        ui.text_edit_singleline(&mut self.new_subscription.name);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Smee URL:");
                        ui.text_edit_singleline(&mut self.new_subscription.smee_url);
                    });
                    ui.horizontal(|ui| {
                        ui.label("基础提示词:");
                        ui.text_edit_singleline(&mut self.new_subscription.base_prompt);
                    });
                    ui.horizontal(|ui| {
                        ui.label("工作区 (可选):");
                        let mut workspace = self.new_subscription.workspace.clone()
                            .unwrap_or_default();
                        ui.text_edit_singleline(&mut workspace);
                        self.new_subscription.workspace = if workspace.is_empty() {
                            None
                        } else {
                            Some(workspace)
                        };
                    });

                    ui.horizontal(|ui| {
                        ui.label("Reporter:");
                        let mut selected_reporter = self.new_subscription.reporter.clone()
                            .unwrap_or_else(|| "console".to_string());

                        egui::ComboBox::from_id_salt("reporter_select")
                            .selected_text(&selected_reporter)
                            .show_ui(ui, |ui| {
                                for reporter in &self.available_reporters {
                                    if ui.selectable_label(
                                        selected_reporter == *reporter,
                                        reporter
                                    ).clicked() {
                                        selected_reporter = reporter.clone();
                                    }
                                }
                            });

                        self.new_subscription.reporter = Some(selected_reporter);
                    });

                    ui.horizontal(|ui| {
                        ui.label("Filter:");
                        let mut selected_filter = self.new_subscription.filter_regex.clone()
                            .unwrap_or_else(|| "none".to_string());
                        

                        egui::ComboBox::from_id_salt("filter_select")
                            .selected_text(if selected_filter == "none" {
                                "无过滤"
                            } else if selected_filter.starts_with("regex:") {
                                "自定义正则"
                            } else {
                                &selected_filter
                            })
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(selected_filter == "none", "无过滤").clicked() {
                                    selected_filter = "none".to_string();
                                }
                                if ui.selectable_label(
                                    selected_filter.starts_with("regex:"),
                                    "自定义正则"
                                ).clicked() {
                                    selected_filter = "regex:".to_string();
                                }
                                for filter in &self.available_filters {
                                    if filter != "none" && filter != "regex" {
                                        if ui.selectable_label(
                                            selected_filter == *filter,
                                            filter
                                        ).clicked() {
                                            selected_filter = filter.clone();
                                        }
                                    }
                                }
                            });

                        if selected_filter.starts_with("regex:") {
                            ui.label("正则:");
                            let mut regex_input = selected_filter.strip_prefix("regex:").unwrap_or("").to_string();
                            if ui.text_edit_singleline(&mut regex_input).changed() {
                                selected_filter = format!("regex:{}", regex_input);
                            }
                        }

                        self.new_subscription.filter_regex = if selected_filter == "none" {
                            None
                        } else {
                            Some(selected_filter)
                        };
                    });

                    if ui.button("✅ 添加订阅").clicked() {
                        if !self.new_subscription.name.is_empty()
                            && !self.new_subscription.smee_url.is_empty()
                            && !self.new_subscription.base_prompt.is_empty()
                        {
                            self.config.subscriptions.push(self.new_subscription.clone());
                            self.new_subscription = Subscription::default();
                            self.save_config();
                            self.status_message = "订阅已添加".to_string();
                        } else {
                            self.status_message = "请填写完整信息".to_string();
                        }
                    }
                });

            ui.separator();

            ui.horizontal(|ui| {
                if self.is_running {
                    if ui.button("🛑 停止").clicked() {
                        self.is_running = false;
                        self.status_message = "服务已停止".to_string();
                    }
                } else {
                    if ui.button("▶ 启动").clicked() {
                        self.is_running = true;
                        self.status_message = "服务启动中...".to_string();
                    }
                }

                if ui.button("💾 保存配置").clicked() {
                    self.save_config();
                }
            });
        });
    }
}

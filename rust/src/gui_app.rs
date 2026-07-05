use eframe::egui;

use crate::gui_config::{AppConfig, Subscription};

pub struct AgentLoopApp {
    config: AppConfig,
    new_subscription: Subscription,
    is_running: bool,
    status_message: String,
    available_reporters: Vec<String>,
    available_filters: Vec<String>,
    selected_subscription: Option<usize>,
    show_new_subscription_dialog: bool,
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
            selected_subscription: None,
            show_new_subscription_dialog: false,
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
        // 顶部状态栏
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading("Agent Loop");
                ui.add_space(ui.available_width() - 120.0);

                if self.is_running {
                    ui.colored_label(egui::Color32::from_rgb(46, 204, 113), "🟢 运行中");
                } else {
                    ui.colored_label(egui::Color32::GRAY, "○ 已停止");
                }
            });
            ui.add_space(8.0);
        });

        // 底部操作栏
        egui::TopBottomPanel::bottom("bottom_bar").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);

                if ui.add_sized([90.0, 28.0], egui::Button::new("💾 保存配置")).clicked() {
                    self.save_config();
                }

                ui.add_space(8.0);

                if self.is_running {
                    if ui.add_sized([100.0, 28.0], egui::Button::new("🛑 停止服务")).clicked() {
                        self.is_running = false;
                        self.status_message = "服务已停止".to_string();
                    }
                } else {
                    if ui.add_sized([100.0, 28.0], egui::Button::new("▶ 启动服务")).clicked() {
                        self.is_running = true;
                        self.status_message = "服务启动中...".to_string();
                    }
                }

                ui.add_space(16.0);
                ui.label(&self.status_message);
            });
            ui.add_space(8.0);
        });

        // 左侧订阅列表
        egui::SidePanel::left("subscription_list")
            .resizable(true)
            .default_width(280.0)
            .width_range(200.0..=400.0)
            .show(ctx, |ui| {
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.heading("订阅列表");
                    ui.add_space(ui.available_width() - 65.0);
                    if ui.add_sized([60.0, 28.0], egui::Button::new("+ 新建")).clicked() {
                        self.show_new_subscription_dialog = true;
                        self.new_subscription = Subscription::default();
                    }
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                if self.config.subscriptions.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(40.0);
                        ui.label("暂无订阅");
                        ui.add_space(8.0);
                        ui.label("点击右上角 + 新建 按钮添加");
                    });
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (idx, sub) in self.config.subscriptions.iter_mut().enumerate() {
                            let is_selected = self.selected_subscription == Some(idx);

                            let response = ui.selectable_label(is_selected, &sub.name);
                            if response.clicked() {
                                self.selected_subscription = Some(idx);
                            }

                            if is_selected {
                                ui.horizontal(|ui| {
                                    ui.add_space(8.0);
                                    if sub.enabled {
                                        ui.colored_label(egui::Color32::from_rgb(46, 204, 113), "● 启用");
                                    } else {
                                        ui.colored_label(egui::Color32::GRAY, "○ 禁用");
                                    }
                                });
                            }

                            ui.add_space(4.0);
                        }
                    });
                }
            });

        // 右侧详情面板
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(16.0);

            // 连接配置区域
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::same(16.0))
                .show(ui, |ui| {
                    ui.heading("连接配置");
                    ui.add_space(12.0);

                    ui.label("WS 地址");
                    ui.text_edit_singleline(&mut self.config.ws_url);

                    ui.add_space(8.0);

                    ui.label("默认工作区");
                    ui.text_edit_singleline(&mut self.config.default_workspace);
                });

            ui.add_space(16.0);

            // 订阅详情区域
            let mut delete_selected = false;
            if let Some(idx) = self.selected_subscription {
                if let Some(sub) = self.config.subscriptions.get_mut(idx) {
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(16.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.heading("订阅详情");
                                ui.add_space(ui.available_width() - 65.0);
                                if ui.add_sized([60.0, 28.0], egui::Button::new("🗑 删除")).clicked() {
                                    delete_selected = true;
                                }
                            });

                            ui.add_space(12.0);

                            ui.label("名称");
                            ui.text_edit_singleline(&mut sub.name);

                            ui.add_space(8.0);

                            ui.label("Smee URL");
                            ui.text_edit_singleline(&mut sub.smee_url);

                            ui.add_space(8.0);

                            ui.label("基础提示词");
                            ui.add_sized(
                                [ui.available_width(), 100.0],
                                egui::TextEdit::multiline(&mut sub.base_prompt)
                            );

                            ui.add_space(8.0);

                            ui.label("工作区（可选）");
                            let mut workspace = sub.workspace.clone().unwrap_or_default();
                            ui.text_edit_singleline(&mut workspace);
                            sub.workspace = if workspace.is_empty() { None } else { Some(workspace) };

                            ui.add_space(8.0);

                            ui.label("Reporter");
                            let mut selected_reporter = sub.reporter.clone().unwrap_or_else(|| "console".to_string());
                            egui::ComboBox::from_id_salt("reporter_detail")
                                .selected_text(&selected_reporter)
                                .width(ui.available_width())
                                .show_ui(ui, |ui| {
                                    for reporter in &self.available_reporters {
                                        ui.selectable_value(&mut selected_reporter, reporter.clone(), reporter);
                                    }
                                });
                            sub.reporter = Some(selected_reporter);

                            ui.add_space(8.0);

                            ui.label("Filter");
                            let mut selected_filter = sub.filter_regex.clone().unwrap_or_else(|| "none".to_string());
                            let display_text = if selected_filter == "none" {
                                "无过滤".to_string()
                            } else if selected_filter.starts_with("regex:") {
                                "自定义正则".to_string()
                            } else {
                                selected_filter.clone()
                            };

                            egui::ComboBox::from_id_salt("filter_detail")
                                .selected_text(display_text)
                                .width(ui.available_width())
                                .show_ui(ui, |ui| {
                                    if ui.selectable_label(selected_filter == "none", "无过滤").clicked() {
                                        selected_filter = "none".to_string();
                                    }
                                    if ui.selectable_label(selected_filter.starts_with("regex:"), "自定义正则").clicked() {
                                        selected_filter = "regex:".to_string();
                                    }
                                    for filter in &self.available_filters {
                                        if filter != "none" && filter != "regex" {
                                            if ui.selectable_label(selected_filter == *filter, filter).clicked() {
                                                selected_filter = filter.clone();
                                            }
                                        }
                                    }
                                });

                            if selected_filter.starts_with("regex:") {
                                ui.add_space(4.0);
                                ui.label("正则表达式");
                                let mut regex_input = selected_filter.strip_prefix("regex:").unwrap_or("").to_string();
                                if ui.text_edit_singleline(&mut regex_input).changed() {
                                    selected_filter = format!("regex:{}", regex_input);
                                }
                            }

                            sub.filter_regex = if selected_filter == "none" { None } else { Some(selected_filter) };

                            ui.add_space(12.0);

                            ui.checkbox(&mut sub.enabled, "启用此订阅");
                        });
                }
            } else {
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.label("← 请从左侧选择一个订阅");
                });
            }

            if delete_selected {
                if let Some(idx) = self.selected_subscription {
                    self.config.subscriptions.remove(idx);
                    self.selected_subscription = None;
                    self.save_config();
                }
            }
        });

        // 新建订阅弹窗
        if self.show_new_subscription_dialog {
            egui::Window::new("新建订阅")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_width(480.0);

                    ui.label("名称");
                    ui.text_edit_singleline(&mut self.new_subscription.name);

                    ui.add_space(8.0);

                    ui.label("Smee URL");
                    ui.text_edit_singleline(&mut self.new_subscription.smee_url);

                    ui.add_space(8.0);

                    ui.label("基础提示词");
                    ui.add_sized(
                        [ui.available_width(), 100.0],
                        egui::TextEdit::multiline(&mut self.new_subscription.base_prompt)
                    );

                    ui.add_space(8.0);

                    ui.label("工作区（可选）");
                    let mut workspace = self.new_subscription.workspace.clone().unwrap_or_default();
                    ui.text_edit_singleline(&mut workspace);
                    self.new_subscription.workspace = if workspace.is_empty() { None } else { Some(workspace) };

                    ui.add_space(8.0);

                    ui.label("Reporter");
                    let mut selected_reporter = self.new_subscription.reporter.clone()
                        .unwrap_or_else(|| "console".to_string());
                    egui::ComboBox::from_id_salt("reporter_new")
                        .selected_text(&selected_reporter)
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for reporter in &self.available_reporters {
                                ui.selectable_value(&mut selected_reporter, reporter.clone(), reporter);
                            }
                        });
                    self.new_subscription.reporter = Some(selected_reporter);

                    ui.add_space(8.0);

                    ui.label("Filter");
                    let mut selected_filter = self.new_subscription.filter_regex.clone()
                        .unwrap_or_else(|| "none".to_string());
                    let display_text = if selected_filter == "none" {
                        "无过滤".to_string()
                    } else if selected_filter.starts_with("regex:") {
                        "自定义正则".to_string()
                    } else {
                        selected_filter.clone()
                    };

                    egui::ComboBox::from_id_salt("filter_new")
                        .selected_text(display_text)
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(selected_filter == "none", "无过滤").clicked() {
                                selected_filter = "none".to_string();
                            }
                            if ui.selectable_label(selected_filter.starts_with("regex:"), "自定义正则").clicked() {
                                selected_filter = "regex:".to_string();
                            }
                            for filter in &self.available_filters {
                                if filter != "none" && filter != "regex" {
                                    if ui.selectable_label(selected_filter == *filter, filter).clicked() {
                                        selected_filter = filter.clone();
                                    }
                                }
                            }
                        });

                    if selected_filter.starts_with("regex:") {
                        ui.add_space(4.0);
                        ui.label("正则表达式");
                        let mut regex_input = selected_filter.strip_prefix("regex:").unwrap_or("").to_string();
                        if ui.text_edit_singleline(&mut regex_input).changed() {
                            selected_filter = format!("regex:{}", regex_input);
                        }
                    }

                    self.new_subscription.filter_regex = if selected_filter == "none" { None } else { Some(selected_filter) };

                    ui.add_space(16.0);
                    ui.separator();
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        if ui.add_sized([60.0, 28.0], egui::Button::new("取消")).clicked() {
                            self.show_new_subscription_dialog = false;
                            self.new_subscription = Subscription::default();
                        }

                        ui.add_space(8.0);

                        if ui.add_sized([80.0, 28.0], egui::Button::new("✅ 创建")).clicked() {
                            if !self.new_subscription.name.is_empty()
                                && !self.new_subscription.smee_url.is_empty()
                                && !self.new_subscription.base_prompt.is_empty()
                            {
                                self.config.subscriptions.push(self.new_subscription.clone());
                                self.selected_subscription = Some(self.config.subscriptions.len() - 1);
                                self.new_subscription = Subscription::default();
                                self.show_new_subscription_dialog = false;
                                self.save_config();
                                self.status_message = "订阅已添加".to_string();
                            } else {
                                self.status_message = "请填写完整信息".to_string();
                            }
                        }
                    });
                });
        }
    }
}

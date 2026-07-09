use eframe::egui;

use crate::bridge_client::{ConnState, ConnStatus};
use crate::bridge_service::BridgeService;
use crate::event_log::{LogEntry, LogKind};
use crate::gui_config::{scan_cc_connect_projects, AppConfig, Subscription};

pub struct AgentLoopApp {
    config: AppConfig,
    new_subscription: Subscription,
    is_running: bool,
    status_message: String,
    available_reporters: Vec<String>,
    available_filters: Vec<String>,
    /// cc-connect 配置中的项目名列表（订阅表单下拉框选项）
    available_projects: Vec<String>,
    selected_subscription: Option<usize>,
    show_new_subscription_dialog: bool,
    show_settings_window: bool,
    show_command_dialog: bool,
    command_target: usize,
    command_input: String,
    /// 推送类型：false = 指令（card_action），true = 消息（message）
    command_send_message: bool,
    show_token: bool,
    show_webhook_logs: bool,
    show_result_logs: bool,
    show_system_logs: bool,
    service: Option<BridgeService>,
    conn_status: Option<ConnStatus>,
    log_rx: Option<std::sync::mpsc::Receiver<LogEntry>>,
    logs: Vec<LogEntry>,
    cc_connect_child: Option<std::process::Child>,
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
            available_projects: scan_cc_connect_projects(),
            selected_subscription: None,
            show_new_subscription_dialog: false,
            show_settings_window: false,
            show_command_dialog: false,
            command_target: 0,
            command_input: String::new(),
            command_send_message: false,
            show_token: false,
            show_webhook_logs: true,
            show_result_logs: true,
            show_system_logs: true,
            service: None,
            conn_status: None,
            log_rx: None,
            logs: Vec::new(),
            cc_connect_child: None,
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

    /// 把用户输入的指令规范化为 card_action 值：`/model` → `cmd:/model`
    fn command_action(input: &str) -> String {
        let t = input.trim();
        if t.starts_with("cmd:") {
            t.to_string()
        } else if t.starts_with('/') {
            format!("cmd:{t}")
        } else {
            format!("cmd:/{t}")
        }
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
            // 与 filter::apply_filters 的加载目录保持一致（policy，而非 filters）
            let filters_dir = home.join(".agent-loop").join("policy");
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

    /// 从 WS 地址解析出主机和端口（ws 默认 80 / wss 默认 443）
    fn ws_endpoint(ws_url: &str) -> Option<(String, u16)> {
        let u = url::Url::parse(ws_url).ok()?;
        let host = u.host_str()?.to_string();
        let port = u.port_or_known_default()?;
        Some((host, port))
    }

    /// 探测目标端口是否已有服务在监听（短超时 TCP 连接）
    fn port_listening(host: &str, port: u16) -> bool {
        use std::net::{TcpStream, ToSocketAddrs};
        let Ok(addrs) = (host, port).to_socket_addrs() else {
            return false;
        };
        for addr in addrs {
            if TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(400)).is_ok() {
                return true;
            }
        }
        false
    }

    /// 直接向 GUI 执行记录追加一条本地条目（服务未启动时也可用）
    fn push_local_log(&mut self, kind: LogKind, summary: impl Into<String>) {
        self.logs.push(LogEntry {
            time: chrono::Local::now(),
            kind,
            source: "系统".to_string(),
            session_key: String::new(),
            summary: summary.into(),
            detail: String::new(),
        });
    }

    /// 启动服务前确保 cc-connect 可用：WS 端口已被监听则复用，否则自动拉起
    fn ensure_cc_connect(&mut self) {
        let Some((host, port)) = Self::ws_endpoint(&self.config.ws_url) else {
            self.push_local_log(LogKind::Error, format!("WS 地址无法解析: {}", self.config.ws_url));
            return;
        };
        if Self::port_listening(&host, port) {
            self.push_local_log(
                LogKind::System,
                format!("🔍 检测到 {host}:{port} 已有服务监听，跳过启动 cc-connect"),
            );
            return;
        }
        self.start_cc_connect();
        let note = match self.cc_connect_child.as_ref() {
            Some(child) => format!("🚀 {host}:{port} 未被监听，已自动启动 cc-connect (PID {})", child.id()),
            None => format!("❌ {host}:{port} 未被监听，且 cc-connect 启动失败: {}", self.status_message),
        };
        let kind = if self.cc_connect_child.is_some() { LogKind::System } else { LogKind::Error };
        self.push_local_log(kind, note);
    }

    fn start_service(&mut self) {
        if self.config.ws_url.trim().is_empty() {
            self.status_message = "请先在设置中填写 WS 地址".to_string();
            self.show_settings_window = true;
            return;
        }
        // 启动服务包含启动 cc-connect：端口已监听则跳过
        self.ensure_cc_connect();

        let (log_tx, log_rx) = std::sync::mpsc::channel();
        let conn_status = ConnStatus::new();
        self.service = Some(BridgeService::start(
            self.config.clone(),
            log_tx,
            conn_status.clone(),
        ));
        self.conn_status = Some(conn_status);
        self.log_rx = Some(log_rx);
        self.is_running = true;
        self.status_message = "服务运行中".to_string();
    }

    fn stop_service(&mut self) {
        if let Some(mut svc) = self.service.take() {
            svc.stop();
        }
        // 服务停止时一并回收由本应用拉起的 cc-connect（外部进程不受影响）
        self.stop_cc_connect();
        self.conn_status = None;
        self.log_rx = None;
        self.is_running = false;
        self.status_message = "服务已停止".to_string();
    }

    fn drain_logs(&mut self) {
        if let Some(rx) = &self.log_rx {
            while let Ok(entry) = rx.try_recv() {
                self.logs.push(entry);
            }
            const MAX_LOG_LINES: usize = 1000;
            if self.logs.len() > MAX_LOG_LINES {
                let overflow = self.logs.len() - MAX_LOG_LINES;
                self.logs.drain(..overflow);
            }
        }
    }

    fn cc_connect_running(&mut self) -> bool {
        match self.cc_connect_child.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(None) => true,
                _ => {
                    self.cc_connect_child = None;
                    false
                }
            },
            None => false,
        }
    }

    fn start_cc_connect(&mut self) {
        let cmd_line = self.config.cc_connect_command.trim().to_string();
        if cmd_line.is_empty() {
            self.status_message = "请先填写 cc-connect 启动命令".to_string();
            return;
        }
        let mut parts = cmd_line.split_whitespace();
        let program = parts.next().unwrap_or_default().to_string();
        match std::process::Command::new(&program)
            .args(parts)
            .spawn()
        {
            Ok(child) => {
                self.status_message = format!("cc-connect 已启动 (PID {})", child.id());
                self.cc_connect_child = Some(child);
            }
            Err(e) => {
                self.status_message = format!("启动 cc-connect 失败: {e}");
            }
        }
    }

    fn stop_cc_connect(&mut self) {
        if let Some(mut child) = self.cc_connect_child.take() {
            let _ = child.kill();
            let _ = child.wait();
            self.status_message = "cc-connect 已停止".to_string();
        }
    }

    /// 渲染订阅编辑表单（新建弹窗与详情弹窗共用）。
    /// 短字段两列一排，压缩弹窗高度。
    fn subscription_form(
        ui: &mut egui::Ui,
        sub: &mut Subscription,
        available_reporters: &[String],
        available_filters: &[String],
        available_projects: &[String],
        id_suffix: &str,
    ) {
        let full_line = |ui: &mut egui::Ui, text: &mut String| {
            ui.add(egui::TextEdit::singleline(text).desired_width(ui.available_width()));
        };

        // 名称 | 项目
        ui.columns(2, |cols| {
            cols[0].label("名称");
            full_line(&mut cols[0], &mut sub.name);

            cols[1]
                .label("项目")
                .on_hover_text("绑定到 cc-connect 配置中的项目（register 时指定，会话按项目隔离）");
            let display = if sub.project.is_empty() {
                "(默认项目)".to_string()
            } else {
                sub.project.clone()
            };
            egui::ComboBox::from_id_salt(format!("project_{id_suffix}"))
                .selected_text(display)
                .width(cols[1].available_width())
                .show_ui(&mut cols[1], |ui| {
                    if ui.selectable_label(sub.project.is_empty(), "(默认项目)").clicked() {
                        sub.project.clear();
                    }
                    for p in available_projects {
                        if ui.selectable_label(sub.project == *p, p).clicked() {
                            sub.project = p.clone();
                        }
                    }
                    // 配置文件里已删除的项目仍保留为可见选项，避免误清空
                    if !sub.project.is_empty() && !available_projects.contains(&sub.project) {
                        let _ = ui.selectable_label(true, format!("{}（未在配置中）", sub.project));
                    }
                });
        });

        ui.add_space(6.0);

        ui.label("Smee URL");
        full_line(ui, &mut sub.smee_url);

        ui.add_space(6.0);

        ui.label("基础提示词");
        ui.add_sized(
            [ui.available_width(), 80.0],
            egui::TextEdit::multiline(&mut sub.base_prompt),
        );

        ui.add_space(6.0);

        // Reporter | Filter
        let mut selected_filter = sub.filter_regex.clone().unwrap_or_else(|| "none".to_string());
        ui.columns(2, |cols| {
            cols[0].label("Reporter");
            let mut selected_reporter =
                sub.reporter.clone().unwrap_or_else(|| "console".to_string());
            egui::ComboBox::from_id_salt(format!("reporter_{id_suffix}"))
                .selected_text(&selected_reporter)
                .width(cols[0].available_width())
                .show_ui(&mut cols[0], |ui| {
                    for reporter in available_reporters {
                        ui.selectable_value(&mut selected_reporter, reporter.clone(), reporter);
                    }
                });
            sub.reporter = Some(selected_reporter);

            cols[1].label("Filter");
            let display_text = if selected_filter == "none" {
                "无过滤".to_string()
            } else if selected_filter.starts_with("regex:") {
                "自定义正则".to_string()
            } else {
                selected_filter.clone()
            };
            egui::ComboBox::from_id_salt(format!("filter_{id_suffix}"))
                .selected_text(display_text)
                .width(cols[1].available_width())
                .show_ui(&mut cols[1], |ui| {
                    if ui.selectable_label(selected_filter == "none", "无过滤").clicked() {
                        selected_filter = "none".to_string();
                    }
                    if ui.selectable_label(selected_filter.starts_with("regex:"), "自定义正则").clicked() {
                        selected_filter = "regex:".to_string();
                    }
                    for filter in available_filters {
                        if filter != "none"
                            && filter != "regex"
                            && ui.selectable_label(selected_filter == *filter, filter).clicked()
                        {
                            selected_filter = filter.clone();
                        }
                    }
                });
        });

        if selected_filter.starts_with("regex:") {
            ui.add_space(4.0);
            ui.label("正则条件（AND 关系，最多 3 个，全部命中才放行；留空的忽略）");
            let body = selected_filter.strip_prefix("regex:").unwrap_or("");
            let mut parts: Vec<String> =
                body.split("&&").map(|s| s.trim().to_string()).collect();
            parts.resize(3, String::new());
            let mut changed = false;
            for (i, part) in parts.iter_mut().enumerate() {
                if ui
                    .add(
                        egui::TextEdit::singleline(part)
                            .desired_width(ui.available_width())
                            .hint_text(format!("条件 {}（如 @claudecode01）", i + 1)),
                    )
                    .changed()
                {
                    changed = true;
                }
            }
            if changed {
                // 保留中间空条件的位置（避免输入框内容跳位），只裁掉尾部空条件；
                // 空段在 apply_filters 中会被忽略
                let mut segs: Vec<&str> = parts.iter().map(|s| s.trim()).collect();
                while segs.len() > 1 && segs.last() == Some(&"") {
                    segs.pop();
                }
                selected_filter = format!("regex:{}", segs.join(" && "));
            }
        }

        sub.filter_regex = if selected_filter == "none" { None } else { Some(selected_filter) };
    }
}

impl Drop for AgentLoopApp {
    fn drop(&mut self) {
        // 窗口关闭时回收后台服务与本应用拉起的 cc-connect 子进程；
        // std::process::Child 被 drop 不会终止进程，必须显式 kill
        self.stop_service();
        self.stop_cc_connect();
    }
}

impl eframe::App for AgentLoopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_logs();
        if self.is_running || self.cc_connect_child.is_some() {
            // 服务在后台线程产生日志 / cc-connect 进程状态变化，定期重绘
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }

        // 顶部状态栏：服务状态 + cc-connect 状态
        let cc_proc_running = self.cc_connect_running();
        let cc_pid = self.cc_connect_child.as_ref().map(|c| c.id());

        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading("Agent Loop");

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // 最右：设置按钮
                    if ui.add_sized([70.0, 26.0], egui::Button::new("⚙ 设置")).clicked() {
                        self.show_settings_window = true;
                    }

                    ui.add_space(12.0);

                    // cc-connect 状态（Bridge 连接 + 本地进程）
                    let green = egui::Color32::from_rgb(46, 204, 113);
                    let orange = egui::Color32::from_rgb(243, 156, 18);
                    let red = egui::Color32::from_rgb(231, 76, 60);

                    let conn_state = self
                        .conn_status
                        .as_ref()
                        .map(|s| s.get())
                        .unwrap_or(ConnState::Disconnected);
                    let (color, text) = if self.is_running {
                        match conn_state {
                            ConnState::Connected => (green, "cc-connect 已连接"),
                            ConnState::Connecting => (orange, "cc-connect 连接中…"),
                            ConnState::Disconnected => (red, "cc-connect 已断开"),
                        }
                    } else {
                        (egui::Color32::GRAY, "cc-connect 未连接")
                    };
                    let label = match cc_pid {
                        Some(pid) if cc_proc_running => format!("● {text} · PID {pid}"),
                        _ => format!("● {text}"),
                    };
                    let hover = if cc_proc_running {
                        "圆点表示 Bridge WebSocket 连接状态\n本地 cc-connect 进程由本应用启动，运行中"
                    } else {
                        "圆点表示 Bridge WebSocket 连接状态\n本地进程未由本应用启动（可在 ⚙ 设置 中启动 cc-connect）"
                    };
                    ui.colored_label(color, label).on_hover_text(hover);

                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // 服务状态
                    if self.is_running {
                        ui.colored_label(green, "● 服务运行中");
                    } else {
                        ui.colored_label(egui::Color32::GRAY, "○ 服务已停止");
                    }
                });
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
                        self.stop_service();
                    }
                } else {
                    if ui.add_sized([100.0, 28.0], egui::Button::new("▶ 启动服务")).clicked() {
                        self.start_service();
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
            .default_width(240.0)
            .width_range(180.0..=400.0)
            .show(ctx, |ui| {
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.heading("订阅列表");
                    ui.add_space((ui.available_width() - 65.0).max(0.0));
                    if ui.add_sized([60.0, 28.0], egui::Button::new("+ 新建")).clicked() {
                        self.show_new_subscription_dialog = true;
                        self.new_subscription = Subscription::default();
                        // 打开表单时重新读取 cc-connect 配置，项目列表保持最新
                        self.available_projects = scan_cc_connect_projects();
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
                        for (idx, sub) in self.config.subscriptions.iter().enumerate() {
                            let is_selected = self.selected_subscription == Some(idx);

                            ui.horizontal(|ui| {
                                if sub.enabled {
                                    ui.colored_label(egui::Color32::from_rgb(46, 204, 113), "●");
                                } else {
                                    ui.colored_label(egui::Color32::GRAY, "○");
                                }
                                let response = ui.selectable_label(is_selected, &sub.name);
                                if response.clicked() {
                                    self.selected_subscription = Some(idx);
                                    // 打开详情时重新读取 cc-connect 配置，项目列表保持最新
                                    self.available_projects = scan_cc_connect_projects();
                                }
                            });

                            ui.add_space(4.0);
                        }
                    });
                    ui.add_space(8.0);
                    ui.weak("点击订阅可编辑详情");
                }
            });

        // 主区域：执行记录（Webhook 数据 + 执行结果 + 系统日志）
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading("执行记录");
                ui.add_space(16.0);
                ui.checkbox(&mut self.show_webhook_logs, "📨 Webhook");
                ui.checkbox(&mut self.show_result_logs, "✅ 执行结果");
                ui.checkbox(&mut self.show_system_logs, "⚙ 系统");
                ui.add_space((ui.available_width() - 135.0).max(0.0));
                if ui
                    .add_sized([60.0, 24.0], egui::Button::new("📤 指令"))
                    .on_hover_text("向某个订阅的会话发送指令（card_action）")
                    .clicked()
                {
                    self.show_command_dialog = true;
                }
                ui.add_space(4.0);
                if ui.add_sized([60.0, 24.0], egui::Button::new("清空")).clicked() {
                    self.logs.clear();
                }
            });
            ui.add_space(4.0);
            ui.separator();

            // 只渲染最近 20 条可见记录，避免长时间运行后日志过多造成渲染压力
            // （内存中仍保留最近 1000 条，切换筛选类型时各自取最近 20 条）
            const MAX_VISIBLE_LOGS: usize = 20;

            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(4.0);
                    let visible_entries: Vec<(usize, &LogEntry)> = self
                        .logs
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| match entry.kind {
                            LogKind::Webhook => self.show_webhook_logs,
                            LogKind::Result => self.show_result_logs,
                            LogKind::System => self.show_system_logs,
                            LogKind::Error => true, // 错误始终展示
                        })
                        .collect();
                    let shown = visible_entries.len().min(MAX_VISIBLE_LOGS);
                    let skipped = visible_entries.len() - shown;
                    if skipped > 0 {
                        ui.weak(format!("… 已省略较早的 {skipped} 条，仅展示最近 {MAX_VISIBLE_LOGS} 条"));
                        ui.add_space(2.0);
                    }
                    for &(idx, entry) in visible_entries.iter().skip(skipped) {
                        let (icon, color) = match entry.kind {
                            LogKind::Webhook => ("📨", egui::Color32::from_rgb(100, 181, 246)),
                            LogKind::Result => ("✅", egui::Color32::from_rgb(46, 204, 113)),
                            LogKind::Error => ("❌", egui::Color32::from_rgb(231, 76, 60)),
                            LogKind::System => ("•", egui::Color32::GRAY),
                        };

                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(entry.time.format("%H:%M:%S").to_string())
                                    .monospace()
                                    .weak(),
                            );
                            ui.colored_label(color, format!("{icon} [{}]", entry.source));
                            if !entry.session_key.is_empty() {
                                ui.label(
                                    egui::RichText::new(format!("🔑 {}", entry.session_key))
                                        .monospace()
                                        .weak()
                                        .size(11.0),
                                )
                                .on_hover_text("Bridge 会话 Session Key（同一订阅共用，保留上下文）");
                            }
                            ui.label(&entry.summary);
                        });

                        if !entry.detail.is_empty() {
                            ui.indent(("log_indent", idx), |ui| {
                                egui::CollapsingHeader::new("详情")
                                    .id_salt((idx, entry.time.timestamp_millis()))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(&entry.detail)
                                                .monospace()
                                                .size(12.0),
                                        );
                                    });
                            });
                        }
                        ui.add_space(2.0);
                    }

                    if shown == 0 {
                        ui.add_space(40.0);
                        ui.vertical_centered(|ui| {
                            if self.logs.is_empty() {
                                ui.weak("暂无记录。点击「▶ 启动服务」开始监听 Webhook 事件。");
                            } else {
                                ui.weak("当前筛选条件下没有记录");
                            }
                        });
                    }
                });
        });

        // 指令弹窗：选择订阅会话，发送任意 card_action 指令
        if self.show_command_dialog {
            let mut open = true;
            let mut send_clicked = false;
            egui::Window::new("📤 发送指令")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.set_width(420.0);

                    ui.label("目标订阅");
                    if self.config.subscriptions.is_empty() {
                        ui.weak("暂无订阅，请先创建");
                    } else {
                        if self.command_target >= self.config.subscriptions.len() {
                            self.command_target = 0;
                        }
                        let current_name =
                            self.config.subscriptions[self.command_target].name.clone();
                        egui::ComboBox::from_id_salt("command_target")
                            .selected_text(&current_name)
                            .width(ui.available_width())
                            .show_ui(ui, |ui| {
                                for (i, s) in self.config.subscriptions.iter().enumerate() {
                                    ui.selectable_value(&mut self.command_target, i, &s.name);
                                }
                            });
                        ui.label(
                            egui::RichText::new(BridgeService::session_key_for(
                                &self.config.subscriptions[self.command_target].name,
                            ))
                            .monospace()
                            .weak()
                            .size(11.0),
                        );
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label("类型");
                        ui.selectable_value(&mut self.command_send_message, false, "指令")
                            .on_hover_text("发送 card_action 引擎命令，回执以 📩 记入执行记录");
                        ui.selectable_value(&mut self.command_send_message, true, "消息")
                            .on_hover_text("以用户身份推送一段话，走正常 message 执行流程");
                    });

                    ui.add_space(8.0);
                    let enter_pressed = if self.command_send_message {
                        ui.label("消息内容");
                        ui.add(
                            egui::TextEdit::multiline(&mut self.command_input)
                                .desired_width(ui.available_width())
                                .desired_rows(4)
                                .hint_text("要推送给 agent 的一段话"),
                        );
                        false // 多行输入回车用于换行，不触发发送
                    } else {
                        ui.label("指令");
                        let input = ui.add(
                            egui::TextEdit::singleline(&mut self.command_input)
                                .desired_width(ui.available_width())
                                .hint_text("如 /model switch k2、/dir /tmp、/current"),
                        );
                        input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    };

                    ui.add_space(12.0);
                    let can_send = self.is_running
                        && !self.config.subscriptions.is_empty()
                        && !self.command_input.trim().is_empty();
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(can_send, egui::Button::new("📤 发送"))
                            .on_disabled_hover_text(if self.is_running {
                                "请选择订阅并输入指令"
                            } else {
                                "启动服务后才能发送指令"
                            })
                            .clicked()
                            || (can_send && enter_pressed)
                        {
                            send_clicked = true;
                        }
                    });
                });

            if send_clicked {
                if let Some(svc) = &self.service {
                    let name = self.config.subscriptions[self.command_target].name.clone();
                    if self.command_send_message {
                        svc.send_message(&name, self.command_input.trim());
                        self.status_message = format!("消息已推送 → {name}");
                    } else {
                        let key = BridgeService::session_key_for(&name);
                        let action = Self::command_action(&self.command_input);
                        svc.send_command(&key, &action);
                        self.status_message = format!("指令已发送: {action} → {name}");
                    }
                    self.command_input.clear();
                    self.show_command_dialog = false;
                }
            }
            if !open {
                self.show_command_dialog = false;
            }
        }

        // 设置弹窗（连接配置不常变更，收进这里）
        if self.show_settings_window {
            let mut open = true;
            egui::Window::new("⚙ 设置")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.set_width(480.0);

                    ui.label("WS 地址（cc-connect Bridge 端点）");
                    ui.text_edit_singleline(&mut self.config.ws_url);

                    ui.add_space(8.0);

                    ui.label("Bridge Token");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.config.bridge_token)
                                .password(!self.show_token)
                                .desired_width(ui.available_width() - 40.0),
                        );
                        if ui.add_sized([32.0, 20.0], egui::Button::new("👁")).clicked() {
                            self.show_token = !self.show_token;
                        }
                    });

                    ui.add_space(8.0);

                    ui.label("cc-connect 启动命令");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.config.cc_connect_command)
                            .desired_width(ui.available_width()),
                    );
                    ui.weak("启动服务时自动检测：WS 端口未被监听才会用此命令拉起 cc-connect");

                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(8.0);

                    if ui.add_sized([90.0, 28.0], egui::Button::new("💾 保存配置")).clicked() {
                        self.save_config();
                    }
                });
            if !open {
                self.show_settings_window = false;
            }
        }

        // 订阅详情弹窗
        let mut delete_selected = false;
        let mut close_detail = false;
        let mut save_detail = false;
        let mut clear_session_clicked = false;
        if let Some(idx) = self.selected_subscription {
            if idx >= self.config.subscriptions.len() {
                self.selected_subscription = None;
            } else {
                let available_reporters = &self.available_reporters;
                let available_filters = &self.available_filters;
                let available_projects = &self.available_projects;
                let service_running = self.is_running;
                let sub = &mut self.config.subscriptions[idx];
                egui::Window::new("订阅详情")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    // 内容超过窗口高度时出现滚动条，弹窗不超出屏幕
                    .max_height(ctx.screen_rect().height() - 120.0)
                    .vscroll(true)
                    .show(ctx, |ui| {
                        ui.set_width(480.0);

                        Self::subscription_form(ui, sub, available_reporters, available_filters, available_projects, "detail");

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(6.0);

                        // 启用开关 + 会话上下文操作放同一排
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut sub.enabled, "启用此订阅");
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let btn = egui::Button::new("🧹 清除上下文");
                                if ui
                                    .add_enabled(service_running, btn)
                                    .on_hover_text("向 cc-connect 发送 card_action `cmd:/new` 开启新会话\n（权限模式等由项目配置决定，自动继承）")
                                    .on_disabled_hover_text("启动服务后可清除会话上下文")
                                    .clicked()
                                {
                                    clear_session_clicked = true;
                                }
                            });
                        });
                        // 同一订阅固定 session key，cc-connect 侧保留上下文
                        ui.horizontal(|ui| {
                            ui.weak("会话:");
                            ui.label(
                                egui::RichText::new(BridgeService::session_key_for(&sub.name))
                                    .monospace()
                                    .weak()
                                    .size(11.0),
                            );
                        });

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(6.0);

                        ui.horizontal(|ui| {
                            if ui.add_sized([60.0, 28.0], egui::Button::new("🗑 删除")).clicked() {
                                delete_selected = true;
                            }
                            ui.add_space(8.0);
                            if ui.add_sized([60.0, 28.0], egui::Button::new("关闭")).clicked() {
                                close_detail = true;
                            }
                            ui.add_space(8.0);
                            if ui.add_sized([100.0, 28.0], egui::Button::new("💾 保存并关闭")).clicked() {
                                save_detail = true;
                                close_detail = true;
                            }
                        });
                    });
            }
        }

        if clear_session_clicked {
            if let (Some(svc), Some(idx)) = (&self.service, self.selected_subscription) {
                let key = BridgeService::session_key_for(&self.config.subscriptions[idx].name);
                svc.clear_session(&key);
                self.status_message = format!("已发送清除上下文指令: {key}");
            }
        }
        if delete_selected {
            if let Some(idx) = self.selected_subscription {
                self.config.subscriptions.remove(idx);
                self.selected_subscription = None;
                self.save_config();
            }
        }
        if save_detail {
            self.save_config();
        }
        if close_detail {
            self.selected_subscription = None;
        }

        // 新建订阅弹窗
        if self.show_new_subscription_dialog {
            egui::Window::new("新建订阅")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .max_height(ctx.screen_rect().height() - 120.0)
                .vscroll(true)
                .show(ctx, |ui| {
                    ui.set_width(480.0);

                    let available_reporters = self.available_reporters.clone();
                    let available_filters = self.available_filters.clone();
                    let available_projects = self.available_projects.clone();
                    Self::subscription_form(
                        ui,
                        &mut self.new_subscription,
                        &available_reporters,
                        &available_filters,
                        &available_projects,
                        "new",
                    );

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

use eframe::egui;

use crate::bridge_client::{ConnState, ConnStatus};
use crate::bridge_service::BridgeService;
use crate::event_log::{LogEntry, LogKind};
use crate::gui_config::{scan_cc_connect_projects, Agent, AppConfig};

pub struct AgentLoopApp {
    config: AppConfig,
    new_agent: Agent,
    is_running: bool,
    status_message: String,
    available_reporters: Vec<String>,
    available_filters: Vec<String>,
    /// cc-connect 配置中的项目名列表（Agent 表单下拉框选项）
    available_projects: Vec<String>,
    /// Some = 正在查看该 Agent 的详情页
    selected_agent: Option<usize>,
    show_new_agent_dialog: bool,
    show_settings_window: bool,
    /// 运行日志窗口（系统/服务级日志，默认隐藏）
    show_system_log_window: bool,
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
            new_agent: Agent::default(),
            is_running: false,
            status_message: "就绪".to_string(),
            available_reporters: Self::scan_reporters(),
            available_filters: Self::scan_filters(),
            available_projects: scan_cc_connect_projects(),
            selected_agent: None,
            show_new_agent_dialog: false,
            show_settings_window: false,
            show_system_log_window: false,
            command_input: String::new(),
            // chat 输入栏默认发「消息」，需要引擎指令时再切换
            command_send_message: true,
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

    /// 渲染 Agent 编辑表单（新建弹窗与详情页共用）。
    /// 短字段两列一排，压缩高度。
    fn agent_form(
        ui: &mut egui::Ui,
        sub: &mut Agent,
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

        ui.label("触发方式");
        ui.horizontal(|ui| {
            // 目前仅支持 Webhook 一种触发方式，下拉框占位便于后续扩展
            egui::ComboBox::from_id_salt(format!("trigger_{id_suffix}"))
                .selected_text("Webhook")
                .width(110.0)
                .show_ui(ui, |ui| {
                    let _ = ui.selectable_label(true, "Webhook");
                });
            ui.add(
                egui::TextEdit::singleline(&mut sub.webhook_url)
                    .desired_width(ui.available_width())
                    .hint_text("Webhook URL（如 https://smee.io/xxxx）"),
            );
        });

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

    /// 日志条目是否属于某个 Agent（按来源名或 Bridge 会话键匹配）
    fn belongs_to_agent(entry: &LogEntry, name: &str, session_key: &str) -> bool {
        entry.source == name
            || (!entry.session_key.is_empty() && entry.session_key == session_key)
    }

    /// 是否为会话分界点（清除上下文后开启新会话）
    fn is_session_boundary(entry: &LogEntry) -> bool {
        entry.summary.starts_with("🧹")
    }

    /// 渲染单条日志（详情页会话分组与主页系统日志共用）
    fn render_log_entry(ui: &mut egui::Ui, salt: (usize, usize), entry: &LogEntry) {
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
            ui.label(&entry.summary);
        });

        if !entry.detail.is_empty() {
            ui.indent(("log_indent", salt), |ui| {
                egui::CollapsingHeader::new("详情")
                    .id_salt((salt, entry.time.timestamp_millis()))
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

    /// 主页：Agent 卡片网格（每行按窗口宽度自适应列数，超出纵向滚动）
    fn ui_cards_page(&mut self, ui: &mut egui::Ui) {
        let mut open_new = false;
        let mut clicked: Option<usize> = None;

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.heading("Agents");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_sized([110.0, 28.0], egui::Button::new("＋ 新建 Agent")).clicked() {
                    open_new = true;
                }
            });
        });
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(8.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.config.agents.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(60.0);
                        ui.label("暂无 Agent");
                        ui.add_space(8.0);
                        ui.weak("点击右上角「＋ 新建 Agent」创建第一个 Agent");
                    });
                    return;
                }

                const SPACING: f32 = 12.0;
                ui.spacing_mut().item_spacing = egui::vec2(SPACING, SPACING);
                // 列数按最小卡宽计算，再把剩余宽度均分给各卡片，行尾不留空隙
                let avail = ui.available_width();
                let cols = (((avail + SPACING) / (Self::CARD_W + SPACING)).floor() as usize).max(1);
                let card_w = (avail - SPACING * (cols as f32 - 1.0)) / cols as f32;

                let total = self.config.agents.len();
                for row_start in (0..total).step_by(cols) {
                    ui.horizontal(|ui| {
                        for idx in row_start..(row_start + cols).min(total) {
                            let agent = &self.config.agents[idx];
                            if Self::agent_card(ui, agent, &self.logs, card_w).clicked() {
                                clicked = Some(idx);
                            }
                        }
                    });
                }
            });

        if open_new {
            self.show_new_agent_dialog = true;
            self.new_agent = Agent::default();
            // 打开表单时重新读取 cc-connect 配置，项目列表保持最新
            self.available_projects = scan_cc_connect_projects();
        }
        if let Some(idx) = clicked {
            self.selected_agent = Some(idx);
            self.command_input.clear();
            self.available_projects = scan_cc_connect_projects();
        }
    }

    /// 卡片最小宽度（网格列数按窗口宽度自适应，实际宽度均分填满整行）
    const CARD_W: f32 = 260.0;

    /// 渲染单张 Agent 卡片，返回整卡的点击响应
    fn agent_card(
        ui: &mut egui::Ui,
        agent: &Agent,
        logs: &[LogEntry],
        card_w: f32,
    ) -> egui::Response {
        const CARD_H: f32 = 110.0;
        // 内容区宽度 = 卡片宽度扣除左右内边距
        let inner_w: f32 = card_w - 2.0 * 14.0;

        let session_key = BridgeService::session_key_for(&agent.name);
        let last_entry = logs
            .iter()
            .rev()
            .find(|e| Self::belongs_to_agent(e, &agent.name, &session_key));

        let frame = egui::Frame::none()
            .fill(ui.visuals().faint_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(egui::Margin::same(14.0))
            .show(ui, |ui| {
                // 卡片在网格的横向布局中渲染，内部强制竖排并锁定宽度
                ui.vertical(|ui| {
                    ui.set_width(inner_w);
                    ui.set_max_width(inner_w);
                    ui.set_min_height(CARD_H);
                    ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);

                    ui.horizontal(|ui| {
                        if agent.enabled {
                            ui.colored_label(egui::Color32::from_rgb(46, 204, 113), "●");
                        } else {
                            ui.colored_label(egui::Color32::GRAY, "○");
                        }
                        ui.add(
                            egui::Label::new(egui::RichText::new(&agent.name).strong().size(15.0))
                                .truncate(),
                        );
                    });
                    ui.add_space(4.0);

                    let project = if agent.project.is_empty() {
                        "(默认项目)".to_string()
                    } else {
                        agent.project.clone()
                    };
                    ui.weak(format!("项目: {project}"));
                    ui.weak("触发: Webhook");
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&agent.webhook_url)
                                .weak()
                                .monospace()
                                .size(11.0),
                        )
                        .truncate(),
                    );

                    ui.add_space(4.0);
                    match last_entry {
                        Some(e) => {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "最近 {} · {}",
                                        e.time.format("%H:%M:%S"),
                                        e.summary
                                    ))
                                    .weak()
                                    .size(11.0),
                                )
                                .truncate(),
                            );
                        }
                        None => {
                            ui.weak(egui::RichText::new("暂无活动").size(11.0));
                        }
                    }
                });
            });

        let resp = frame
            .response
            .interact(egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.hovered() {
            ui.painter().rect_stroke(
                resp.rect,
                10.0,
                egui::Stroke::new(1.5, ui.visuals().selection.stroke.color),
            );
        }
        resp
    }

    /// Agent 详情页：配置编辑 + 会话操作 + 发送指令 + 按会话分组的运行日志
    fn ui_detail_page(&mut self, ui: &mut egui::Ui, idx: usize) {
        let is_running = self.is_running;
        let session_key = BridgeService::session_key_for(&self.config.agents[idx].name);

        let mut back = false;
        let mut delete = false;
        let mut save = false;
        let mut clear_session = false;
        let mut send = false;
        let mut clear_logs = false;

        {
            // 拆分借用：表单要改 agent，日志区只读 self.logs
            let Self {
                config,
                logs,
                available_reporters,
                available_filters,
                available_projects,
                command_input,
                command_send_message,
                show_webhook_logs,
                show_result_logs,
                show_system_logs,
                ..
            } = self;
            let agent = &mut config.agents[idx];

            // 底部固定的聊天输入栏（chat 风格：日志在上滚动，输入在下）
            // 左右留 2px，避免圆角边框恰好压在裁剪边界上被切掉
            egui::TopBottomPanel::bottom("agent_chat_panel")
                .frame(egui::Frame::none().inner_margin(egui::Margin {
                    left: 2.0,
                    right: 2.0,
                    top: 8.0,
                    bottom: 10.0,
                }))
                .show_separator_line(false)
                .show_inside(ui, |ui| {
                    let edit_id = egui::Id::new("agent_chat_edit");
                    // Enter 发送、Shift+Enter 换行；仅在输入框聚焦时消费按键
                    let enter_send = ui.ctx().memory(|m| m.has_focus(edit_id))
                        && ui.input_mut(|i| {
                            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                        });
                    let can_send = is_running && !command_input.trim().is_empty();

                    egui::Frame::none()
                        .fill(ui.visuals().extreme_bg_color)
                        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                        .rounding(egui::Rounding::same(10.0))
                        .inner_margin(egui::Margin::same(8.0))
                        .show(ui, |ui| {
                            let hint = if *command_send_message {
                                "给该 Agent 发一段话…（Enter 发送，Shift+Enter 换行）"
                            } else {
                                "输入指令，如 /model switch k2、/dir /tmp、/current（Enter 发送）"
                            };
                            ui.add(
                                egui::TextEdit::multiline(command_input)
                                    .id(edit_id)
                                    .frame(false)
                                    .desired_rows(2)
                                    .desired_width(ui.available_width())
                                    .hint_text(hint),
                            );
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.selectable_value(command_send_message, true, "💬 消息")
                                    .on_hover_text("以用户身份推送一段话，走正常 message 执行流程");
                                ui.selectable_value(command_send_message, false, "⌨ 指令")
                                    .on_hover_text("发送 card_action 引擎命令，回执以 📩 记入运行日志");
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .add_enabled(can_send, egui::Button::new("⬆ 发送"))
                                            .on_disabled_hover_text(if is_running {
                                                "请输入内容"
                                            } else {
                                                "启动服务后才能发送"
                                            })
                                            .clicked()
                                        {
                                            send = true;
                                        }
                                        if !is_running {
                                            ui.weak("启动服务后可发送");
                                        }
                                    },
                                );
                            });
                        });
                    if enter_send && can_send {
                        send = true;
                    }
                });

            // 中央区域：页头 + 配置 + 会话 + 运行日志（占底部输入栏之外的剩余空间）
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show_inside(ui, |ui| {
                // 页头：返回 + 状态点 + 名称 + 删除
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add_sized([70.0, 28.0], egui::Button::new("← 返回")).clicked() {
                        back = true;
                    }
                    ui.add_space(8.0);
                    if agent.enabled {
                        ui.colored_label(egui::Color32::from_rgb(46, 204, 113), "●");
                    } else {
                        ui.colored_label(egui::Color32::GRAY, "○");
                    }
                    ui.heading(&agent.name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_sized([70.0, 28.0], egui::Button::new("🗑 删除")).clicked() {
                            delete = true;
                        }
                    });
                });
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // 配置（默认收起，日志才是详情页主角）
                egui::CollapsingHeader::new("⚙ 配置")
                    .id_salt(("agent_config", idx))
                    .default_open(false)
                    .show(ui, |ui| {
                        Self::agent_form(
                            ui,
                            agent,
                            available_reporters,
                            available_filters,
                            available_projects,
                            "detail",
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut agent.enabled, "启用此 Agent")
                                .on_hover_text("修改启用状态与配置后需保存，重启服务生效");
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.add_sized([90.0, 26.0], egui::Button::new("💾 保存")).clicked() {
                                    save = true;
                                }
                            });
                        });
                    });

                ui.add_space(4.0);

                // 会话信息与操作（同一 Agent 固定 session key，cc-connect 侧保留上下文）
                ui.horizontal(|ui| {
                    ui.weak("会话:");
                    ui.label(
                        egui::RichText::new(&session_key)
                            .monospace()
                            .weak()
                            .size(11.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(is_running, egui::Button::new("🧹 清除上下文"))
                            .on_hover_text("向 cc-connect 发送 card_action `cmd:/new` 开启新会话\n（权限模式等由项目配置决定，自动继承）")
                            .on_disabled_hover_text("启动服务后可清除会话上下文")
                            .clicked()
                        {
                            clear_session = true;
                        }
                    });
                });

                ui.add_space(4.0);

                ui.add_space(8.0);
                ui.separator();

                // 运行日志（按会话分组）
                ui.horizontal(|ui| {
                    ui.strong("运行日志");
                    ui.add_space(12.0);
                    ui.checkbox(show_webhook_logs, "📨 Webhook");
                    ui.checkbox(show_result_logs, "✅ 执行结果");
                    ui.checkbox(show_system_logs, "⚙ 系统");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("清空日志").clicked() {
                            clear_logs = true;
                        }
                    });
                });
                ui.add_space(4.0);

                let visible: Vec<(usize, &LogEntry)> = logs
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| Self::belongs_to_agent(e, &agent.name, &session_key))
                    .filter(|(_, e)| match e.kind {
                        LogKind::Webhook => *show_webhook_logs,
                        LogKind::Result => *show_result_logs,
                        LogKind::System => *show_system_logs,
                        LogKind::Error => true, // 错误始终展示
                    })
                    .collect();

                // 以「🧹 清除上下文」为分界拆分会话，历史会话折叠、当前会话展开
                let mut groups: Vec<Vec<(usize, &LogEntry)>> = Vec::new();
                for (i, e) in visible {
                    let boundary = Self::is_session_boundary(e);
                    if groups.is_empty() || (boundary && !groups.last().unwrap().is_empty()) {
                        groups.push(Vec::new());
                    }
                    groups.last_mut().unwrap().push((i, e));
                }

                // 每组最多渲染最近 50 条，避免长时间运行后渲染压力过大
                const MAX_GROUP_LOGS: usize = 50;
                let render_group =
                    |ui: &mut egui::Ui, gi: usize, group: &[(usize, &LogEntry)]| {
                        let skipped = group.len().saturating_sub(MAX_GROUP_LOGS);
                        if skipped > 0 {
                            ui.weak(format!("… 已省略较早的 {skipped} 条"));
                            ui.add_space(2.0);
                        }
                        for &(i, entry) in group.iter().skip(skipped) {
                            Self::render_log_entry(ui, (gi, i), entry);
                        }
                    };

                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        if groups.is_empty() {
                            ui.add_space(40.0);
                            ui.vertical_centered(|ui| {
                                if logs.is_empty() {
                                    ui.weak("暂无记录。点击「▶ 启动服务」开始监听 Webhook 事件。");
                                } else {
                                    ui.weak("该 Agent 在当前筛选条件下没有记录");
                                }
                            });
                            return;
                        }

                        let last_gi = groups.len() - 1;
                        for (gi, group) in groups.iter().enumerate() {
                            let start = group
                                .first()
                                .map(|(_, e)| e.time.format("%H:%M:%S").to_string())
                                .unwrap_or_default();
                            if gi == last_gi {
                                ui.weak(format!(
                                    "── 当前会话 · 自 {start} · {} 条 ──",
                                    group.len()
                                ));
                                ui.add_space(4.0);
                                render_group(ui, gi, group);
                            } else {
                                let end = group
                                    .last()
                                    .map(|(_, e)| e.time.format("%H:%M:%S").to_string())
                                    .unwrap_or_default();
                                egui::CollapsingHeader::new(format!(
                                    "会话 {} · {start} – {end} · {} 条",
                                    gi + 1,
                                    group.len()
                                ))
                                .id_salt(("session_group", gi))
                                .default_open(false)
                                .show(ui, |ui| render_group(ui, gi, group));
                                ui.add_space(4.0);
                            }
                        }
                    });
                });
        }

        // UI 闭包结束后统一处理动作，避免借用冲突
        if back {
            self.selected_agent = None;
        }
        if save {
            self.save_config();
        }
        if clear_session {
            if let Some(svc) = &self.service {
                svc.clear_session(&session_key);
                self.status_message = format!("已发送清除上下文指令: {session_key}");
            }
        }
        if send {
            if let Some(svc) = &self.service {
                let name = self.config.agents[idx].name.clone();
                if self.command_send_message {
                    svc.send_message(&name, self.command_input.trim());
                    self.status_message = format!("消息已推送 → {name}");
                } else {
                    let action = Self::command_action(&self.command_input);
                    svc.send_command(&session_key, &action);
                    self.status_message = format!("指令已发送: {action} → {name}");
                }
                self.command_input.clear();
            }
        }
        if clear_logs {
            let name = self.config.agents[idx].name.clone();
            self.logs
                .retain(|e| !Self::belongs_to_agent(e, &name, &session_key));
        }
        if delete {
            self.config.agents.remove(idx);
            self.selected_agent = None;
            self.save_config();
        }
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
                    // 最右：设置 + 运行日志按钮
                    if ui.add_sized([70.0, 26.0], egui::Button::new("⚙ 设置")).clicked() {
                        self.show_settings_window = true;
                    }
                    ui.add_space(4.0);
                    if ui
                        .add_sized([90.0, 26.0], egui::Button::new("📋 运行日志"))
                        .on_hover_text("查看服务/连接等系统级日志（各 Agent 的对话日志在其卡片详情页中）")
                        .clicked()
                    {
                        self.show_system_log_window = !self.show_system_log_window;
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

        // 主区域：Agent 卡片主页 / 点击卡片进入的详情页
        if let Some(idx) = self.selected_agent {
            if idx >= self.config.agents.len() {
                self.selected_agent = None;
            }
        }
        // 主区域左右多留一点边距，避免右对齐按钮与内容边框贴死窗口边缘
        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(14.0, 8.0)),
            )
            .show(ctx, |ui| match self.selected_agent {
                Some(idx) => self.ui_detail_page(ui, idx),
                None => self.ui_cards_page(ui),
            });

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

        // 运行日志窗口：不属于任何 Agent 的系统级日志（服务启停、cc-connect 连接等）。
        // 默认隐藏，顶部栏「📋 运行日志」开关。
        if self.show_system_log_window {
            let mut open = true;
            let mut clear_clicked = false;
            let names_keys: Vec<(String, String)> = self
                .config
                .agents
                .iter()
                .map(|a| (a.name.clone(), BridgeService::session_key_for(&a.name)))
                .collect();
            egui::Window::new("📋 运行日志")
                .open(&mut open)
                .default_size([560.0, 360.0])
                .resizable(true)
                .show(ctx, |ui| {
                    let sys_entries: Vec<(usize, &LogEntry)> = self
                        .logs
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| {
                            !names_keys
                                .iter()
                                .any(|(n, k)| Self::belongs_to_agent(e, n, k))
                        })
                        .collect();

                    ui.horizontal(|ui| {
                        ui.weak(format!(
                            "服务与连接状态日志，共 {} 条。各 Agent 的对话日志在其详情页查看。",
                            sys_entries.len()
                        ));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("清空").clicked() {
                                clear_clicked = true;
                            }
                        });
                    });
                    ui.separator();

                    const MAX_SYS_LOGS: usize = 50;
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if sys_entries.is_empty() {
                                ui.add_space(20.0);
                                ui.vertical_centered(|ui| {
                                    ui.weak("暂无系统日志。点击「▶ 启动服务」开始监听。");
                                });
                                return;
                            }
                            let skipped = sys_entries.len().saturating_sub(MAX_SYS_LOGS);
                            if skipped > 0 {
                                ui.weak(format!("… 已省略较早的 {skipped} 条"));
                            }
                            for &(i, entry) in sys_entries.iter().skip(skipped) {
                                Self::render_log_entry(ui, (usize::MAX, i), entry);
                            }
                        });
                });
            if clear_clicked {
                self.logs
                    .retain(|e| names_keys.iter().any(|(n, k)| Self::belongs_to_agent(e, n, k)));
            }
            if !open {
                self.show_system_log_window = false;
            }
        }

        // 新建 Agent 弹窗
        if self.show_new_agent_dialog {
            egui::Window::new("新建 Agent")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_width(480.0);

                    let available_reporters = self.available_reporters.clone();
                    let available_filters = self.available_filters.clone();
                    let available_projects = self.available_projects.clone();

                    // 表单区单独滚动，按钮固定在弹窗底部，窗口再矮也不会被挤出可视区
                    let form_max_h = (ctx.screen_rect().height() - 220.0).max(160.0);
                    egui::ScrollArea::vertical()
                        .max_height(form_max_h)
                        .show(ui, |ui| {
                            Self::agent_form(
                                ui,
                                &mut self.new_agent,
                                &available_reporters,
                                &available_filters,
                                &available_projects,
                                "new",
                            );
                        });

                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(8.0);

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_sized([80.0, 28.0], egui::Button::new("✅ 创建")).clicked() {
                            if !self.new_agent.name.is_empty()
                                && !self.new_agent.webhook_url.is_empty()
                                && !self.new_agent.base_prompt.is_empty()
                            {
                                self.config.agents.push(self.new_agent.clone());
                                self.new_agent = Agent::default();
                                self.show_new_agent_dialog = false;
                                self.save_config();
                                self.status_message = "Agent 已创建".to_string();
                            } else {
                                self.status_message = "请填写完整信息".to_string();
                            }
                        }

                        ui.add_space(8.0);

                        if ui.add_sized([60.0, 28.0], egui::Button::new("取消")).clicked() {
                            self.show_new_agent_dialog = false;
                            self.new_agent = Agent::default();
                        }
                    });
                });
        }
    }
}

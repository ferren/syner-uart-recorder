//! Syner UART Recorder — captures the TL7218 `WAV_RECORD_EN` PCM trace stream from a serial
//! port into WAV files, and plays them back.
//!
//! See `docs/` in the repository root and `bluetooth/utilities/wav/wav.c` in the
//! firmware for the on-wire packet format.

mod library;
mod player;
mod proto;
mod recorder;
mod settings;

use gpui::{
    App, AppContext as _, Application, Bounds, Context, ElementId, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, WindowBounds, WindowOptions,
    div, px, rgb, size,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{InteractiveElement as _, StatefulInteractiveElement as _};

use library::Recording;
use player::Player;
use recorder::Session;
use settings::Settings;

const BG: u32 = 0x1e1f22;
const PANEL: u32 = 0x2b2d31;
const PANEL_HI: u32 = 0x35373c;
const TEXT: u32 = 0xdcdde1;
const TEXT_DIM: u32 = 0x9a9ba1;
const ACCENT: u32 = 0x4f8cff;
const DANGER: u32 = 0xe05561;
const OK: u32 = 0x46b880;

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Settings,
    Record,
    Library,
}

struct Recorder {
    tab: Tab,
    settings: Settings,
    ports: Vec<String>,
    port_menu_open: bool,
    session: Option<Session>,
    /// Status of the most recent session, kept after it stops so the summary stays visible.
    last_status: Option<recorder::Status>,
    player: Player,
    recordings: Vec<Recording>,
    message: Option<(String, bool)>,
}

impl Recorder {
    fn new(cx: &mut Context<Self>) -> Self {
        // Poll the capture thread and the sink so the UI reflects them without
        // needing either to know about gpui.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.player.poll();
                        this.reap_finished_session();
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            tab: Tab::Record,
            settings: Settings::load(),
            ports: available_ports(),
            port_menu_open: false,
            session: None,
            last_status: None,
            player: Player::default(),
            recordings: library::scan(),
            message: None,
        }
    }

    /// A session can die on its own (cable unplugged, no data). Fold it back into
    /// the idle state so the UI doesn't keep showing a live recording.
    fn reap_finished_session(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let status = session.status();
        if !status.stopped {
            self.last_status = Some(status);
            return;
        }

        let error = status.error.clone();
        self.last_status = Some(status);
        self.session = None;
        self.recordings = library::scan();
        self.message = match error {
            Some(err) => Some((format!("录制中断: {err}"), false)),
            None => Some(("录制结束".into(), true)),
        };
    }

    fn toggle_record(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.stop();
            let status = session.status();
            let path = session.path().to_path_buf();
            self.message = match &status.error {
                Some(err) => Some((format!("录制失败: {err}"), false)),
                None => Some((
                    format!(
                        "已保存 {} ({:.1} s)",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        status.seconds()
                    ),
                    true,
                )),
            };
            self.last_status = Some(status);
            self.recordings = library::scan();
            return;
        }

        match Session::start(&self.settings) {
            Ok(session) => {
                self.last_status = None;
                self.session = Some(session);
                self.message = None;
            }
            Err(err) => self.message = Some((format!("{err:#}"), false)),
        }
    }

    fn is_recording(&self) -> bool {
        self.session.is_some()
    }

    fn save_settings(&mut self) {
        self.settings.save();
    }

    fn toggle_play(&mut self, path: std::path::PathBuf) {
        if self.player.is_playing(&path) {
            self.player.stop();
            return;
        }
        if let Err(err) = self.player.play(&path) {
            self.message = Some((format!("{err:#}"), false));
        }
    }

    fn delete(&mut self, path: std::path::PathBuf) {
        if self.player.is_playing(&path) {
            self.player.stop();
        }
        match std::fs::remove_file(&path) {
            Ok(()) => self.message = Some(("已删除".into(), true)),
            Err(err) => self.message = Some((format!("删除失败: {err}"), false)),
        }
        self.recordings = library::scan();
    }
}

fn available_ports() -> Vec<String> {
    let mut ports: Vec<String> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.port_name)
        .collect();
    ports.sort();
    ports
}

// ---------------------------------------------------------------------------
// Reusable bits of UI
// ---------------------------------------------------------------------------

fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    color: u32,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .px_3()
        .py_1p5()
        .rounded_md()
        .bg(rgb(color))
        .text_color(rgb(TEXT))
        .cursor_pointer()
        .hover(|s| s.opacity(0.85))
        .child(label.into())
}

fn label(text: impl Into<SharedString>) -> gpui::Div {
    div().w(px(150.)).text_color(rgb(TEXT_DIM)).child(text.into())
}

fn row(children: Vec<gpui::AnyElement>) -> gpui::Div {
    let mut row = div().flex().items_center().gap_3().py_1();
    for child in children {
        row = row.child(child);
    }
    row
}

fn stat(name: &str, value: String, color: u32) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w(px(110.))
        .child(div().text_xs().text_color(rgb(TEXT_DIM)).child(name.to_string()))
        .child(div().text_lg().text_color(rgb(color)).child(value))
}

/// Horizontal peak meter, 0.0..=1.0.
fn meter(name: &str, level: f32) -> gpui::Div {
    let pct = (level.clamp(0.0, 1.0) * 100.0) as u32;
    let color = if level > 0.95 { DANGER } else { OK };
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(div().w(px(60.)).text_xs().text_color(rgb(TEXT_DIM)).child(name.to_string()))
        .child(
            div()
                .w(px(220.))
                .h(px(10.))
                .rounded_sm()
                .bg(rgb(PANEL_HI))
                .child(
                    div()
                        .w(gpui::relative(pct as f32 / 100.0))
                        .h_full()
                        .rounded_sm()
                        .bg(rgb(color)),
                ),
        )
        .child(div().text_xs().text_color(rgb(TEXT_DIM)).child(format!("{pct}%")))
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

impl Render for Recorder {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.tab {
            Tab::Settings => self.render_settings(cx),
            Tab::Record => self.render_record(cx),
            Tab::Library => self.render_library(cx),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .text_sm()
            // The default Windows UI font has no CJK coverage.
            .font_family("Microsoft YaHei")
            .child(self.render_tabs(cx))
            .child(div().id("body").flex_1().p_4().overflow_y_scroll().child(body))
            .child(self.render_status_bar())
    }
}

impl Recorder {
    fn render_tabs(&self, cx: &mut Context<Self>) -> gpui::Div {
        let tab = |id: &'static str, name: &'static str, which: Tab, active: bool| {
            div()
                .id(id)
                .px_4()
                .py_2()
                .cursor_pointer()
                .text_color(rgb(if active { TEXT } else { TEXT_DIM }))
                .when(active, |s| s.bg(rgb(PANEL_HI)))
                .hover(|s| s.bg(rgb(PANEL_HI)))
                .child(name)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if which == Tab::Library {
                        this.recordings = library::scan();
                    }
                    this.tab = which;
                    cx.notify();
                }))
        };

        div()
            .flex()
            .items_center()
            .bg(rgb(PANEL))
            .child(tab("t-set", "设置", Tab::Settings, self.tab == Tab::Settings))
            .child(tab("t-rec", "录音", Tab::Record, self.tab == Tab::Record))
            .child(tab("t-lib", "记录", Tab::Library, self.tab == Tab::Library))
            .child(div().flex_1())
            .when(self.is_recording(), |s| {
                s.child(
                    div()
                        .px_4()
                        .text_color(rgb(DANGER))
                        .child("● 正在录制"),
                )
            })
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut ports = div().flex().flex_col().gap_2();
        let selected = self
            .settings
            .port
            .as_deref()
            .filter(|p| self.ports.iter().any(|x| x.as_str() == *p));
        let display = match selected {
            Some(p) => format!("{} ▼", p),
            None => "请选择串口 ▼".to_string(),
        };
        ports = ports.child(
            row(vec![
                label("串口").into_any_element(),
                div()
                    .id("port-select")
                    .w(px(240.))
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(PANEL))
                    .text_color(rgb(if selected.is_some() { TEXT } else { TEXT_DIM }))
                    .child(display)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.port_menu_open = !this.port_menu_open;
                        cx.notify();
                    }))
                    .into_any_element(),
                button("refresh", "刷新列表", PANEL)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ports = available_ports();
                        cx.notify();
                    }))
                    .into_any_element(),
            ])
            .into_any_element(),
        );
        if self.port_menu_open {
            if self.ports.is_empty() {
                ports = ports.child(
                    div()
                        .text_color(rgb(TEXT_DIM))
                        .py_1p5()
                        .child("未发现串口。请确认设备已连接且固件 USB_DEVICE_CLASS 为 CLASS_CDC。"),
                );
            } else {
                let mut list = div()
                    .id("port-list")
                    .h(px(180.))
                    .overflow_y_scroll()
                    .p_2()
                    .rounded_md()
                    .bg(rgb(PANEL))
                    .flex()
                    .flex_col()
                    .gap_1();
                for (i, name) in self.ports.iter().enumerate() {
                    let is_selected = self.settings.port.as_deref() == Some(name.as_str());
                    let pick = name.clone();
                    list = list.child(
                        div()
                            .id(("port", i))
                            .px_3()
                            .py_1p5()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(if is_selected { ACCENT } else { PANEL_HI }))
                            .hover(|s| s.opacity(0.85))
                            .child(name.clone())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.settings.port = Some(pick.clone());
                                this.port_menu_open = false;
                                this.save_settings();
                                cx.notify();
                            })),
                    );
                }
                ports = ports.child(list);
            }
        }

        let disabled = self.is_recording();
        // Changing the frame layout mid-capture would corrupt the WAV, so the
        // parameter buttons are inert while recording.
        let cycle = |id: &'static str,
                     value: String,
                     f: fn(&mut Settings)|
         -> gpui::Stateful<gpui::Div> {
            button(id, value, if disabled { PANEL_HI } else { PANEL })
                .when(!disabled, |s| {
                    s.on_click(cx.listener(move |this, _, _, cx| {
                        f(&mut this.settings);
                        this.save_settings();
                        cx.notify();
                    }))
                })
        };

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        row(vec![
                            label("串口").into_any_element(),
                            button("refresh", "刷新列表", PANEL)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.ports = available_ports();
                                    cx.notify();
                                }))
                                .into_any_element(),
                        ])
                        .into_any_element(),
                    )
                    .child(ports),
            )
            .child(row(vec![
                label("波特率").into_any_element(),
                cycle("baud", format!("{}", self.settings.baud), Settings::cycle_baud)
                    .into_any_element(),
                div()
                    .text_xs()
                    .text_color(rgb(TEXT_DIM))
                    .child("CDC 虚拟串口通常忽略此值")
                    .into_any_element(),
            ]))
            .child(row(vec![
                label("每包采样数").into_any_element(),
                cycle(
                    "spp",
                    format!("{}", self.settings.samples_per_packet),
                    Settings::cycle_samples,
                )
                .into_any_element(),
                div()
                    .text_xs()
                    .text_color(rgb(TEXT_DIM))
                    .child("对应固件 WAV_SAMPLES_PER_PACKET (默认 160)")
                    .into_any_element(),
            ]))
            .child(row(vec![
                label("通道模式").into_any_element(),
                cycle(
                    "chmode",
                    self.settings.channel_mode.label().to_string(),
                    |s| s.channel_mode = s.channel_mode.next(),
                )
                .into_any_element(),
            ]))
            .child(row(vec![
                label("采样率").into_any_element(),
                cycle(
                    "rate",
                    self.settings.rate_override_label(),
                    Settings::cycle_rate_override,
                )
                .into_any_element(),
            ]))
            .child(
                div()
                    .mt_2()
                    .text_xs()
                    .text_color(rgb(TEXT_DIM))
                    .child(format!("数据目录: {}", settings::data_dir().display())),
            )
            .when(disabled, |s| {
                s.child(
                    div()
                        .text_xs()
                        .text_color(rgb(DANGER))
                        .child("录制中无法修改采集参数"),
                )
            })
            .into_any_element()
    }

    fn render_record(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let recording = self.is_recording();
        let status = self
            .session
            .as_ref()
            .map(|s| s.status())
            .or_else(|| self.last_status.clone())
            .unwrap_or_default();

        let target = self
            .session
            .as_ref()
            .map(|s| s.path().display().to_string())
            .unwrap_or_else(|| "—".into());

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        button(
                            "rec",
                            if recording { "停止录制" } else { "开始录制" },
                            if recording { DANGER } else { ACCENT },
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_record();
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .text_color(rgb(TEXT_DIM))
                            .child(match &self.settings.port {
                                Some(p) => format!(
                                    "{p} · {} · {} 采样/包",
                                    self.settings.channel_mode.label(),
                                    self.settings.samples_per_packet
                                ),
                                None => "请先在「设置」中选择串口".to_string(),
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_5()
                    .p_3()
                    .rounded_md()
                    .bg(rgb(PANEL))
                    .child(stat("时长", format!("{:.1} s", status.seconds()), TEXT))
                    .child(stat("数据包", format!("{}", status.packets), TEXT))
                    .child(stat(
                        "丢包",
                        format!("{}", status.lost),
                        if status.lost > 0 { DANGER } else { OK },
                    ))
                    .child(stat(
                        "重同步字节",
                        format!("{}", status.resync_bytes),
                        if status.resync_bytes > 0 { DANGER } else { OK },
                    ))
                    .child(stat(
                        "采样率",
                        if status.sample_rate == 0 {
                            "—".into()
                        } else {
                            format!("{} Hz", status.sample_rate)
                        },
                        TEXT,
                    ))
                    .child(stat(
                        "已接收",
                        format!("{:.2} MB", status.bytes as f64 / (1024.0 * 1024.0)),
                        TEXT,
                    ))
                    .child(stat(
                        "包头位宽",
                        if status.bits == 0 {
                            "—".into()
                        } else {
                            format!("{} bit", status.bits)
                        },
                        TEXT,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .bg(rgb(PANEL))
                    .child(meter(
                        if self.settings.channel_mode == proto::ChannelMode::Both {
                            "高16位"
                        } else {
                            "电平"
                        },
                        status.peak[0],
                    ))
                    .when(
                        self.settings.channel_mode == proto::ChannelMode::Both,
                        |s| s.child(meter("低16位", status.peak[1])),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(TEXT_DIM))
                    .child(format!("输出文件: {target}")),
            )
            .into_any_element()
    }

    fn render_library(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.recordings.is_empty() {
            return div()
                .text_color(rgb(TEXT_DIM))
                .child("还没有录音。切到「录音」页开始第一次采集。")
                .into_any_element();
        }

        let mut list = div().id("lib").flex().flex_col().gap_2().overflow_y_scroll();
        for (i, rec) in self.recordings.iter().enumerate() {
            let playing = self.player.is_playing(&rec.path);
            let play_path = rec.path.clone();
            let del_path = rec.path.clone();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .p_3()
                    .rounded_md()
                    .bg(rgb(PANEL))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().child(rec.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_DIM))
                                    .child(rec.summary()),
                            ),
                    )
                    .child(
                        button(
                            ("play", i),
                            if playing { "停止" } else { "播放" },
                            if playing { DANGER } else { ACCENT },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_play(play_path.clone());
                            cx.notify();
                        })),
                    )
                    .child(
                        button(("del", i), "删除", PANEL_HI).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.delete(del_path.clone());
                                cx.notify();
                            },
                        )),
                    ),
            );
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                row(vec![
                    button("rescan", "刷新", PANEL)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.recordings = library::scan();
                            cx.notify();
                        }))
                        .into_any_element(),
                    div()
                        .text_xs()
                        .text_color(rgb(TEXT_DIM))
                        .child(format!("{} 条 · {}", self.recordings.len(), settings::recordings_dir().display()))
                        .into_any_element(),
                ])
                .into_any_element(),
            )
            .child(div().flex_1().child(list))
            .into_any_element()
    }

    fn render_status_bar(&self) -> gpui::Div {
        let (text, ok) = match &self.message {
            Some((msg, ok)) => (msg.clone(), *ok),
            None => (String::new(), true),
        };
        div()
            .h(px(28.))
            .px_4()
            .flex()
            .items_center()
            .bg(rgb(PANEL))
            .text_xs()
            .text_color(rgb(if ok { TEXT_DIM } else { DANGER }))
            .child(text)
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(820.), px(600.)), cx);
        cx.open_window(
            WindowOptions {
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("协极串口录音分析".into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(Recorder::new),
        )
        .expect("open window");
        cx.activate(true);
    });
}

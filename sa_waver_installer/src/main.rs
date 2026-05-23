#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{
    pos2, vec2, Align, Button, CentralPanel, Color32, ColorImage, Context, FontData,
    FontDefinitions, FontFamily, Frame, Layout, Margin, OpenUrl, ProgressBar, Rect, RichText,
    Sense, Shadow, Stroke, TextureHandle, TextureOptions, Ui, Vec2, ViewportBuilder,
};
use egui_extras::install_image_loaders;
use image::ImageFormat;
use rfd::FileDialog;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE, HWND, WAIT_FAILED, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
use windows::Win32::UI::Shell::{ShellExecuteExW, SHELLEXECUTEINFOW, SEE_MASK_NOCLOSEPROCESS};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const WINDOW_WIDTH: f32 = 900.0;
const WINDOW_HEIGHT: f32 = 600.0;
const PROGRESS_MIN_DURATION: Duration = Duration::from_secs(2);

const BG_PNG: &[u8] = include_bytes!("../../assets/bg.png");
const FONT_REGULAR: &[u8] = include_bytes!("../../assets/MapleMono-NF-CN-Regular.ttf");
const VST3_BINARY: &[u8] =
    include_bytes!("../../target/bundled/sa_waver.vst3/Contents/x86_64-win/sa_waver.vst3");
const CLAP_BINARY: &[u8] = include_bytes!("../../target/bundled/sa_waver.clap");

#[derive(Clone, Copy, PartialEq, Eq)]
enum InstallerStep {
    Welcome,
    PluginSelection,
    DirectorySelection,
    Installing,
    Complete,
}

#[derive(Clone)]
struct InstallSettings {
    install_vst3: bool,
    install_clap: bool,
    vst3_dir: PathBuf,
    clap_dir: PathBuf,
}

struct InstallResult {
    installed_items: Vec<String>,
    final_message: String,
}

struct InstallRuntime {
    started_at: Instant,
    progress: f32,
    status: String,
    result: Option<Result<InstallResult, String>>,
    receiver: Receiver<Result<InstallResult, String>>,
}

struct InstallerApp {
    step: InstallerStep,
    install_vst3: bool,
    install_clap: bool,
    vst3_dir_text: String,
    clap_dir_text: String,
    bg_texture: Option<TextureHandle>,
    install_runtime: Option<InstallRuntime>,
    theme_ready: bool,
}

fn main() -> eframe::Result {
    if let Some(code) = maybe_run_elevated_install()? {
        std::process::exit(code);
    }

    env_logger::init();

    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("SA Waver Installer")
            .with_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT])
            .with_min_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT]),
        ..Default::default()
    };

    eframe::run_native(
        "SA Waver Installer",
        options,
        Box::new(|cc| {
            install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(InstallerApp::new(cc.egui_ctx.clone())))
        }),
    )
}

impl InstallerApp {
    fn new(ctx: Context) -> Self {
        set_theme(&ctx);

        let bg_texture = load_texture(&ctx, "installer_bg", BG_PNG).ok();
        let vst3_dir = default_vst3_dir();
        let clap_dir = default_clap_dir();

        Self {
            step: InstallerStep::Welcome,
            install_vst3: true,
            install_clap: true,
            vst3_dir_text: vst3_dir.display().to_string(),
            clap_dir_text: clap_dir.display().to_string(),
            bg_texture,
            install_runtime: None,
            theme_ready: true,
        }
    }

    fn begin_install(&mut self) {
        let settings = InstallSettings {
            install_vst3: self.install_vst3,
            install_clap: self.install_clap,
            vst3_dir: PathBuf::from(self.vst3_dir_text.trim()),
            clap_dir: PathBuf::from(self.clap_dir_text.trim()),
        };
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(perform_install(settings));
        });

        self.install_runtime = Some(InstallRuntime {
            started_at: Instant::now(),
            progress: 0.0,
            status: String::from("如果看到请求管理员权限的话记得点一下谢谢泥"),
            result: None,
            receiver: rx,
        });
        self.step = InstallerStep::Installing;
    }
}

impl eframe::App for InstallerApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        if !self.theme_ready {
            set_theme(ctx);
            self.theme_ready = true;
        }

        ctx.request_repaint_after(Duration::from_millis(16));

        CentralPanel::default()
            .frame(Frame::NONE.fill(Color32::BLACK).inner_margin(0.0))
            .show(ctx, |ui| {
                draw_background(ui, self.bg_texture.as_ref());

                let outer_rect = ui.max_rect().shrink2(vec2(18.0, 18.0));
                ui.allocate_ui_at_rect(outer_rect, |ui| {
                    Frame::new()
                        .fill(Color32::from_hex("#423b36").unwrap().gamma_multiply(0.94))
                        .shadow(Shadow {
                            offset: [0, 4],
                            blur: 64,
                            spread: 0,
                            color: Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.18),
                        })
                        .inner_margin(Margin::same(18))
                        .show(ui, |ui| {
                            ui.set_min_size(ui.available_size());
                            draw_header(ui);
                            ui.add_space(10.0);
                            draw_step_indicator(ui, self.step);
                            ui.add_space(18.0);

                            let card_rect = ui.available_rect_before_wrap();
                            ui.allocate_ui_at_rect(card_rect, |ui| match self.step {
                                InstallerStep::Welcome => draw_welcome(ui, self),
                                InstallerStep::PluginSelection => draw_plugin_selection(ui, self),
                                InstallerStep::DirectorySelection => draw_directory_selection(ui, self),
                                InstallerStep::Installing => draw_installing(ui, self),
                                InstallerStep::Complete => draw_complete(ui, self, ctx),
                            });
                        });
                });
            });
    }
}

fn draw_background(ui: &mut Ui, bg_texture: Option<&TextureHandle>) {
    if let Some(bg_texture) = bg_texture {
        let bg = egui::Shape::image(
            bg_texture.id(),
            ui.max_rect(),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        ui.painter().add(bg);
    }
}

fn draw_header(ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.heading("SA Waver Installer");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                    .size(12.0)
                    .color(Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.8)),
            );
        });
    });

    ui.label(
        RichText::new("BY SOUT AUDIO.")
            .size(13.0)
            .color(Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.88)),
    );
}

fn draw_step_indicator(ui: &mut Ui, step: InstallerStep) {
    let steps = [
        (InstallerStep::Welcome, "1 Welcome"),
        (InstallerStep::PluginSelection, "2 Plugins"),
        (InstallerStep::DirectorySelection, "3 Directory"),
        (InstallerStep::Installing, "4 Install"),
        (InstallerStep::Complete, "5 Done"),
    ];

    ui.horizontal_wrapped(|ui| {
        for (index, (current, label)) in steps.iter().enumerate() {
            let active = *current == step;
            let reached = step_order(step) >= step_order(*current);

            let fill = if active {
                Color32::from_hex("#DB9160").unwrap()
            } else if reached {
                Color32::from_hex("#6b5d54").unwrap()
            } else {
                Color32::from_hex("#34302e").unwrap()
            };

            let text = if active {
                Color32::from_hex("#2B1100").unwrap()
            } else {
                Color32::from_hex("#FFEAD0").unwrap()
            };

            Frame::new()
                .fill(fill)
                .stroke(Stroke::new(1.0_f32, Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.5)))
                .corner_radius(6.0)
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.label(RichText::new(*label).size(12.0).color(text));
                });

            if index + 1 < steps.len() {
                ui.add_space(6.0);
            }
        }
    });
}

fn draw_welcome(ui: &mut Ui, app: &mut InstallerApp) {
    card_fill(ui, |ui, rect| {
        ui.allocate_ui_at_rect(rect.shrink2(vec2(18.0, 18.0)), |ui| {
            ui.set_min_size(ui.available_size());

            ui.heading("这是一个 SA Waver 安装器");
            ui.add_space(10.0);
            ui.label("格莱丑母带工程师精选的Waveshaper效果器哦哦哦");
            ui.label("拉两坨大的到你硬盘里。");

            let footer_rect = bottom_bar_rect(ui, 56.0);
            ui.allocate_ui_at_rect(footer_rect, |ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if primary_button(ui, "下一步", vec2(120.0, 32.0)).clicked() {
                        app.step = InstallerStep::PluginSelection;
                    }

                    ui.add_space(8.0);

                    if ghost_link_button(ui, " GitHub").clicked() {
                        ui.ctx()
                            .open_url(OpenUrl::new_tab("https://github.com/sout233/sa_waver"));
                    }

                    if ghost_link_button(ui, "󰖟 Homepage").clicked() {
                        ui.ctx()
                            .open_url(OpenUrl::new_tab("https://audio.soout.top/sa_waver"));
                    }
                });
            });
        });
    });
}

fn draw_plugin_selection(ui: &mut Ui, app: &mut InstallerApp) {
    card_fill(ui, |ui, rect| {
        ui.allocate_ui_at_rect(rect.shrink2(vec2(18.0, 18.0)), |ui| {
            ui.set_min_size(ui.available_size());

            ui.heading("选择插件格式");
            ui.add_space(10.0);
            ui.label("FL Studio 用户建议使用 CLAP 格式的插件");
            ui.add_space(12.0);

            checkbox_tile(ui, &mut app.install_vst3, "VST3", "安装到标准 VST3 插件目录");
            ui.add_space(8.0);
            checkbox_tile(ui, &mut app.install_clap, "CLAP", "如果DAW支持的话尽量选这个");

            let footer_rect = bottom_bar_rect(ui, 56.0);
            ui.allocate_ui_at_rect(footer_rect, |ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let response = ui.add_enabled(
                        app.install_vst3 || app.install_clap,
                        primary_button_widget("下一步", vec2(110.0, 32.0)),
                    );
                    if response.clicked() {
                        app.step = InstallerStep::DirectorySelection;
                    }

                    ui.add_space(8.0);

                    if secondary_button(ui, "上一步", vec2(110.0, 32.0)).clicked() {
                        app.step = InstallerStep::Welcome;
                    }
                });
            });
        });
    });
}

fn draw_directory_selection(ui: &mut Ui, app: &mut InstallerApp) {
    card_fill(ui, |ui, rect| {
        ui.allocate_ui_at_rect(rect.shrink2(vec2(18.0, 18.0)), |ui| {
            ui.set_min_size(ui.available_size());

            ui.heading("选择安装目录");
            ui.add_space(10.0);
            ui.label("建议保持默认目录。非必要不要修改，求求了……");
            ui.add_space(12.0);

            let row_width = ui.available_width();
            plugin_dir_row(
                ui,
                "VST3",
                &mut app.vst3_dir_text,
                app.install_vst3,
                Some(default_vst3_dir),
                row_width,
            );
            ui.add_space(10.0);
            plugin_dir_row(
                ui,
                "CLAP",
                &mut app.clap_dir_text,
                app.install_clap,
                Some(default_clap_dir),
                row_width,
            );

            let footer_height = 32.0;
            let remaining = (ui.available_height() - footer_height).max(0.0);
            ui.add_space(remaining);

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let has_selected_target = app.install_vst3 || app.install_clap;
                let vst3_ready = !app.install_vst3 || !app.vst3_dir_text.trim().is_empty();
                let clap_ready = !app.install_clap || !app.clap_dir_text.trim().is_empty();
                let response = ui.add_enabled(
                    has_selected_target && vst3_ready && clap_ready,
                    primary_button_widget("开始安装", vec2(110.0, 32.0)),
                );
                if response.clicked() {
                    app.begin_install();
                }

                ui.add_space(8.0);

                if secondary_button(ui, "上一步", vec2(110.0, 32.0)).clicked() {
                    app.step = InstallerStep::PluginSelection;
                }
            });
        });
    });
}

fn draw_installing(ui: &mut Ui, app: &mut InstallerApp) {
    card_fill(ui, |ui, rect| {
        ui.allocate_ui_at_rect(rect.shrink2(vec2(18.0, 18.0)), |ui| {
            ui.set_min_size(ui.available_size());

            ui.heading("正在安装");
            ui.add_space(10.0);

            if let Some(runtime) = &mut app.install_runtime {
                if runtime.result.is_none() {
                    if let Ok(result) = runtime.receiver.try_recv() {
                        runtime.status = match &result {
                            Ok(_) => String::from("安装任务已完成，正在收尾..."),
                            Err(_) => String::from("安装失败，正在整理结果..."),
                        };
                        runtime.result = Some(result);
                    }
                }

                let elapsed = runtime.started_at.elapsed();
                let timed_progress =
                    (elapsed.as_secs_f32() / PROGRESS_MIN_DURATION.as_secs_f32()).clamp(0.0, 1.0);
                runtime.progress = if runtime.result.is_some() {
                    timed_progress
                } else {
                    timed_progress.min(0.9)
                };

                if runtime.result.is_some() && timed_progress >= 1.0 {
                    app.step = InstallerStep::Complete;
                }

                ui.label(runtime.status.as_str());
                ui.add_space(12.0);
                ui.add(
                    ProgressBar::new(runtime.progress)
                        .desired_width(ui.available_width())
                        .text(format!("{:.0}%", runtime.progress * 100.0)),
                );
                ui.add_space(10.0);
                ui.label("不要急……");
            }
        });
    });
}

fn draw_complete(ui: &mut Ui, app: &mut InstallerApp, ctx: &Context) {
    card_fill(ui, |ui, rect| {
        ui.allocate_ui_at_rect(rect.shrink2(vec2(18.0, 18.0)), |ui| {
            ui.set_min_size(ui.available_size());

            ui.heading("安装完成");
            ui.add_space(10.0);

            if let Some(runtime) = &app.install_runtime {
                match runtime.result.as_ref().expect("install result should exist") {
                    Ok(result) => {
                        ui.label(result.final_message.as_str());
                        ui.add_space(10.0);
                        for item in &result.installed_items {
                            ui.label(format!("• {}", item));
                        }
                    }
                    Err(err) => {
                        ui.colored_label(Color32::from_rgb(255, 120, 120), "安装失败");
                        ui.add_space(6.0);
                        ui.label(err.as_str());
                    }
                }
            }

            let footer_rect = bottom_bar_rect(ui, 56.0);
            ui.allocate_ui_at_rect(footer_rect, |ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if primary_button(ui, "完成", vec2(110.0, 32.0)).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }

                    ui.add_space(8.0);

                    if secondary_button(ui, "重新安装", vec2(110.0, 32.0)).clicked() {
                        app.install_runtime = None;
                        app.step = InstallerStep::Welcome;
                    }
                });
            });
        });
    });
}

fn plugin_dir_row(
    ui: &mut Ui,
    label: &str,
    text: &mut String,
    enabled: bool,
    default_dir: Option<fn() -> PathBuf>,
    row_width: f32,
) {
    ui.add_enabled_ui(enabled, |ui| {
        ui.label(RichText::new(label).size(12.0));
        ui.add_space(6.0);

        let text_width = (row_width - 88.0 - 72.0 - 8.0 - 8.0).max(180.0);
        ui.horizontal(|ui| {
            styled_text_edit(ui, text, text_width);
            ui.add_space(8.0);

            if secondary_button(ui, "浏览...", vec2(88.0, 28.0)).clicked() {
                if let Some(path) = FileDialog::new()
                    .set_directory(PathBuf::from(text.trim()))
                    .pick_folder()
                {
                    *text = path.display().to_string();
                }
            }

            if secondary_button(ui, "默认", vec2(72.0, 28.0)).clicked() {
                if let Some(default_dir_fn) = default_dir {
                    *text = default_dir_fn().display().to_string();
                }
            }
        });
    });
}

fn perform_install(settings: InstallSettings) -> Result<InstallResult, String> {
    elevate_and_install(&settings)?;

    let mut installed_items = Vec::new();
    if settings.install_vst3 {
        installed_items.push(settings.vst3_dir.join("sa_waver.vst3").display().to_string());
    }
    if settings.install_clap {
        installed_items.push(settings.clap_dir.join("sa_waver.clap").display().to_string());
    }

    Ok(InstallResult {
        installed_items,
        final_message: String::from("SA Waver 已成功安装，请重新扫描插件。"),
    })
}

fn run_install_payload(settings: &InstallSettings) -> Result<(), String> {
    if settings.install_vst3 {
        let vst3_target = settings.vst3_dir.join("sa_waver.vst3");
        if vst3_target.exists() {
            remove_existing(&vst3_target)?;
        }

        fs::create_dir_all(vst3_target.join("Contents").join("x86_64-win"))
            .map_err(|err| format!("创建 VST3 目录失败: {err}"))?;
        fs::write(
            vst3_target.join("Contents").join("x86_64-win").join("sa_waver.vst3"),
            VST3_BINARY,
        )
        .map_err(|err| format!("写入 VST3 插件失败: {err}"))?;
    }

    if settings.install_clap {
        let clap_target = settings.clap_dir.join("sa_waver.clap");
        if let Some(parent) = clap_target.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("创建 CLAP 目录失败: {err}"))?;
        }
        fs::write(&clap_target, CLAP_BINARY).map_err(|err| format!("写入 CLAP 插件失败: {err}"))?;
    }

    Ok(())
}

fn remove_existing(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|err| format!("删除旧目录失败: {err}"))?;
    } else if path.is_file() {
        fs::remove_file(path).map_err(|err| format!("删除旧文件失败: {err}"))?;
    }
    Ok(())
}

fn maybe_run_elevated_install() -> Result<Option<i32>, eframe::Error> {
    let args: Vec<String> = env::args().collect();
    if args.len() >= 2 && args[1] == "--run-install" {
        let settings = parse_install_args(&args)
            .map_err(|err| eframe::Error::AppCreation(Box::new(std::io::Error::other(err))))?;

        match run_install_payload(&settings) {
            Ok(()) => return Ok(Some(0)),
            Err(err) => {
                eprintln!("{err}");
                return Ok(Some(1));
            }
        }
    }

    Ok(None)
}

fn elevate_and_install(settings: &InstallSettings) -> Result<(), String> {
    let exe = env::current_exe().map_err(|err| format!("获取安装器路径失败: {err}"))?;
    let args = build_install_args(settings);

    let operation = to_wide("runas");
    let file = to_wide(exe.as_os_str().to_string_lossy().as_ref());
    let parameters = to_wide(&args);
    let mut exec_info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        hwnd: HWND(std::ptr::null_mut()),
        lpVerb: PCWSTR(operation.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        lpDirectory: PCWSTR::null(),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };

    unsafe {
        ShellExecuteExW(&mut exec_info).map_err(|err| {
            if err.code().0 == windows::core::HRESULT::from_win32(ERROR_CANCELLED.0).0 {
                String::from("安装已取消：接受下管理员权限请求谢谢喵")
            } else {
                format!("无法请求管理员权限: {err}")
            }
        })?;
    }

    wait_for_install_process(exec_info.hProcess)
}

fn wait_for_install_process(process: HANDLE) -> Result<(), String> {
    unsafe {
        let wait_result = WaitForSingleObject(process, INFINITE);
        if wait_result != WAIT_OBJECT_0 {
            let _ = CloseHandle(process);
            if wait_result == WAIT_FAILED {
                return Err(String::from("failed to wait for admin process"));
            }
            return Err(format!("err while waiting admin prm: {}", wait_result.0));
        }

        let mut exit_code = 0;
        let exit_result = GetExitCodeProcess(process, &mut exit_code);
        let _ = CloseHandle(process);
        exit_result.map_err(|err| format!("get exit code failed: {err}"))?;

        if exit_code == 0 {
            Ok(())
        } else {
            Err(format!("管理员安装进程执行失败，code: {exit_code}"))
        }
    }
}

fn build_install_args(settings: &InstallSettings) -> String {
    format!(
        "--run-install --install-vst3={} --install-clap={} --vst3-dir=\"{}\" --clap-dir=\"{}\"",
        settings.install_vst3,
        settings.install_clap,
        settings.vst3_dir.display(),
        settings.clap_dir.display()
    )
}

fn parse_install_args(args: &[String]) -> Result<InstallSettings, String> {
    let mut install_vst3 = false;
    let mut install_clap = false;
    let mut vst3_dir = None;
    let mut clap_dir = None;

    for arg in args.iter().skip(2) {
        if let Some(value) = arg.strip_prefix("--install-vst3=") {
            install_vst3 = value.eq_ignore_ascii_case("true");
        } else if let Some(value) = arg.strip_prefix("--install-clap=") {
            install_clap = value.eq_ignore_ascii_case("true");
        } else if let Some(value) = arg.strip_prefix("--vst3-dir=") {
            vst3_dir = Some(PathBuf::from(trim_wrapped_quotes(value)));
        } else if let Some(value) = arg.strip_prefix("--clap-dir=") {
            clap_dir = Some(PathBuf::from(trim_wrapped_quotes(value)));
        }
    }

    Ok(InstallSettings {
        install_vst3,
        install_clap,
        vst3_dir: vst3_dir.ok_or_else(|| String::from("缺少 --vst3-dir 参数"))?,
        clap_dir: clap_dir.ok_or_else(|| String::from("缺少 --clap-dir 参数"))?,
    })
}

fn trim_wrapped_quotes(input: &str) -> String {
    input.trim_matches('"').to_string()
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn default_vst3_dir() -> PathBuf {
    PathBuf::from(r"C:\Program Files\Common Files\VST3")
}

fn default_clap_dir() -> PathBuf {
    PathBuf::from(r"C:\Program Files\Common Files\CLAP")
}

fn set_theme(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("maple-mono".to_string(), std::sync::Arc::new(FontData::from_static(FONT_REGULAR)));
    fonts
        .families
        .get_mut(&FontFamily::Proportional)
        .unwrap()
        .insert(0, "maple-mono".to_string());
    ctx.set_fonts(fonts);

    let old = egui::Visuals::dark();
    let stroke = Stroke::new(1.0_f32, Color32::from_hex("#FFEAD0").unwrap());

    let visuals = egui::Visuals {
        dark_mode: true,
        override_text_color: Some(Color32::from_hex("#FFEAD0").unwrap()),
        widgets: egui::style::Widgets {
            noninteractive: widget_visual(old.widgets.noninteractive, Color32::from_hex("#34302E").unwrap()),
            inactive: widget_visual(old.widgets.inactive, Color32::from_hex("#34302E").unwrap()),
            hovered: widget_visual(old.widgets.hovered, Color32::from_hex("#3d3733").unwrap()),
            active: widget_visual(old.widgets.active, Color32::from_hex("#554e4a").unwrap()),
            open: widget_visual(old.widgets.open, Color32::from_hex("#554e4a").unwrap()),
        },
        selection: egui::style::Selection {
            bg_fill: Color32::from_hex("#FFCBA8").unwrap().gamma_multiply(0.2),
            stroke,
        },
        window_fill: Color32::from_hex("#1C1917").unwrap(),
        panel_fill: Color32::TRANSPARENT,
        popup_shadow: Shadow {
            offset: [4, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(80),
        },
        ..old
    };

    ctx.set_visuals(visuals);
}

fn widget_visual(old: egui::style::WidgetVisuals, bg_fill: Color32) -> egui::style::WidgetVisuals {
    egui::style::WidgetVisuals {
        bg_fill,
        weak_bg_fill: bg_fill,
        bg_stroke: Stroke::new(1.0_f32, Color32::from_hex("#FFEAD0").unwrap()),
        fg_stroke: Stroke::new(1.0_f32, Color32::from_hex("#FFEAD0").unwrap()),
        corner_radius: egui::CornerRadius::same(6),
        ..old
    }
}

fn load_texture(ctx: &Context, name: &str, bytes: &[u8]) -> Result<TextureHandle, String> {
    let image = load_image_from_memory(bytes)?;
    Ok(ctx.load_texture(name, image, TextureOptions::LINEAR))
}

fn load_image_from_memory(image_data: &[u8]) -> Result<ColorImage, String> {
    let image = image::load_from_memory_with_format(image_data, ImageFormat::Png)
        .map_err(|err| format!("image decode failed: {err}"))?;
    let size = [image.width() as _, image.height() as _];
    let image_buffer = image.to_rgba8();
    let pixels = image_buffer.as_flat_samples();
    Ok(ColorImage::from_rgba_unmultiplied(size, pixels.as_slice()))
}

fn card_fill(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui, Rect)) {
    let rect = ui.max_rect();
    let response = ui.allocate_rect(rect, Sense::hover());

    ui.painter().rect_filled(
        response.rect,
        10.0,
        Color32::from_hex("#1c1917").unwrap().gamma_multiply(0.94),
    );
    ui.painter().rect_stroke(
        response.rect,
        10.0,
        Stroke::new(1.0_f32, Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.45)),
        egui::StrokeKind::Outside,
    );

    add_contents(ui, response.rect);
}

fn styled_text_edit(ui: &mut Ui, text: &mut String, width: f32) {
    ui.add_sized(
        [width, 30.0],
        egui::TextEdit::singleline(text)
            .desired_width(width)
            .clip_text(false)
            .margin(Margin::symmetric(8, 6)),
    );
}

fn checkbox_tile(ui: &mut Ui, checked: &mut bool, title: &str, desc: &str) {
    Frame::new()
        .fill(Color32::from_hex("#34302e").unwrap())
        .stroke(Stroke::new(
            1.0_f32,
            Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.4),
        ))
        .corner_radius(8.0)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(checked, "");
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).size(14.0));
                    ui.label(
                        RichText::new(desc)
                            .size(12.0)
                            .color(Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.78)),
                    );
                });
            });
        });
}

fn primary_button(ui: &mut Ui, label: &str, size: Vec2) -> egui::Response {
    ui.add(primary_button_widget(label, size))
}

fn primary_button_widget(label: &str, size: Vec2) -> Button<'_> {
    Button::new(RichText::new(label).color(Color32::from_hex("#2B1100").unwrap()))
        .fill(Color32::from_hex("#DB9160").unwrap())
        .stroke(Stroke::new(
            1.0_f32,
            Color32::from_hex("#FFCBA8").unwrap().gamma_multiply(0.8),
        ))
        .min_size(size)
}

fn secondary_button(ui: &mut Ui, label: &str, size: Vec2) -> egui::Response {
    ui.add(
        Button::new(RichText::new(label).color(Color32::from_hex("#FFEAD0").unwrap()))
            .fill(Color32::from_hex("#34302e").unwrap())
            .stroke(Stroke::new(
                1.0_f32,
                Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.6),
            ))
            .min_size(size),
    )
}

fn ghost_link_button(ui: &mut Ui, label: &str) -> egui::Response {
    ui.add(
        Button::new(RichText::new(label).color(Color32::from_hex("#FFEAD0").unwrap()))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(
                1.0_f32,
                Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.4),
            ))
            .min_size(vec2(110.0, 32.0)),
    )
}

fn bottom_bar_rect(ui: &Ui, height: f32) -> Rect {
    Rect::from_min_max(
        pos2(ui.max_rect().left(), ui.max_rect().bottom() - height),
        ui.max_rect().right_bottom(),
    )
}

fn step_order(step: InstallerStep) -> usize {
    match step {
        InstallerStep::Welcome => 0,
        InstallerStep::PluginSelection => 1,
        InstallerStep::DirectorySelection => 2,
        InstallerStep::Installing => 3,
        InstallerStep::Complete => 4,
    }
}

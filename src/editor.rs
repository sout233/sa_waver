use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::collections::VecDeque;

use bevy_lookup_curve::{editor::LookupCurveEguiEditor, LookupCurve};
use bevy_math::Vec2 as BevyVec2;
use nih_plug::{
    editor::Editor,
    nih_error,
    prelude::AtomicF32,
};
use nih_plug_egui::{
    create_egui_editor,
    egui::{self, epaint::CornerRadiusF32, pos2, Color32, ColorImage, Id, Modal, Pos2, Rect, RichText, TextureHandle, Vec2},
};
use parking_lot::Mutex;

use crate::{
    audio_input_to_chart_input, build_default_dbfs_curve, capture_plot_state, curve_lookup,
    curve_lookup_chart, fs, is_default_linear_curve, load_image_from_memory, load_preset_file,
    save_preset_file,
    sync_lut_cache_from_state, transform_curve_for_symmetry_mode,
    oversampling::{OVERSAMPLING_ALGORITHM_FLAT_FIR, OVERSAMPLING_ALGORITHM_LANCZOS3},
    param_knob::ParamKnob,
    sout_ui::{self, SoutTheme},
    DEFAULT_SYMMETRY_MODE, WaverPluginParams, INTERPOLATION_MODE_COSINE, INTERPOLATION_MODE_HERMITE,
    INTERPOLATION_MODE_LINEAR, SYMMETRY_MODE_ASYMMETRIC, SYMMETRY_MODE_SYMMETRIC,
    DISPLAY_MODE_DBFS, DISPLAY_MODE_LINEAR, DISPLAY_SCOPE_XY, DISPLAY_SCOPE_Y_ONLY,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum SegmentGeneratorKind {
    Sine,
    Triangle,
    Square,
    Stairs,
}

impl SegmentGeneratorKind {
    fn label(self) -> &'static str {
        match self {
            Self::Sine => "Sine Wave",
            Self::Triangle => "Triangle",
            Self::Square => "Square",
            Self::Stairs => "Stairs",
        }
    }
}

pub struct EditorData {
    pub lookup_curve: Arc<Mutex<LookupCurve>>,
    pub curve_dirty: Arc<AtomicBool>,
    pub editor: Arc<Mutex<LookupCurveEguiEditor>>,
    pub lut_cache: Arc<Mutex<Vec<f32>>>,
    // lut_size: usize,
    pub waveform_buffer: Arc<Mutex<VecDeque<f32>>>,

    pub colored_waveform: Arc<AtomicBool>,
    pub presets: Arc<Mutex<Vec<String>>>,
    pub current_preset: Arc<Mutex<String>>,
    pub plot_dirty: Arc<AtomicBool>,
    pub saving_preset_name: Arc<Mutex<String>>,
    pub open_save_modal: Arc<AtomicBool>,
    pub open_msg_modal: Arc<AtomicBool>,
    pub open_about_modal: Arc<AtomicBool>,
    pub open_settings_modal: Arc<AtomicBool>,
    pub settings_tab: Arc<AtomicUsize>,
    pub segment_generator_kind: Arc<AtomicUsize>,
    pub segment_generator_cycles: Arc<AtomicUsize>,
    pub segment_generator_steps: Arc<AtomicUsize>,
    pub segment_generator_active: Arc<AtomicBool>,
    pub help_panel_title: Arc<Mutex<String>>,
    pub help_panel_text: Arc<Mutex<String>>,
    pub msg_modal_title: Arc<Mutex<String>>,
    pub msg_modal_content: Arc<Mutex<String>>,
}

#[derive(Default)]
struct EditorVisualCache {
    background: Option<TextureHandle>,
    save_icon: Option<TextureHandle>,
    und3ath_logo: Option<TextureHandle>,
}

impl EditorData {
    pub fn editor(
        &mut self,
        params: Arc<WaverPluginParams>,
        latest_input: Arc<AtomicF32>,
        current_resolution: Arc<AtomicUsize>,
        current_timebase: Arc<AtomicUsize>,
        linear_ext: Arc<AtomicBool>,
        symmetry_mode: Arc<AtomicUsize>,
        current_oversampling_factor: Arc<AtomicUsize>,
        current_interpolation_mode: Arc<AtomicUsize>,
        current_oversampling_algorithm: Arc<AtomicUsize>,
        current_display_mode: Arc<AtomicUsize>,
        current_display_scope: Arc<AtomicUsize>,
        current_strict_dbfs_ticks: Arc<AtomicBool>,
        current_grid_step_x: Arc<AtomicUsize>,
        current_grid_step_y: Arc<AtomicUsize>,
    ) -> Option<Box<dyn Editor>> {
        let lookup_curve = self.lookup_curve.clone();
        let curve_dirty = self.curve_dirty.clone();
        let editor = self.editor.clone();
        let lut_cache = self.lut_cache.clone();
        let waveform_buffer = self.waveform_buffer.clone();
        let latest_input_ptr = latest_input.clone();
        let colored_waveform_ptr = self.colored_waveform.clone();
        let presets_ptr = self.presets.clone();
        let current_preset_ptr = self.current_preset.clone();
        let plot_dirty_ptr = self.plot_dirty.clone();
        let current_resolution_ptr = current_resolution.clone();
        let current_timebase_ptr = current_timebase.clone();
        let linear_ext_enabled_ptr = linear_ext.clone();
        let symmetry_mode_ptr = symmetry_mode.clone();
        let current_oversampling_factor_ptr = current_oversampling_factor.clone();
        let current_interpolation_mode_ptr = current_interpolation_mode.clone();
        let current_oversampling_algorithm_ptr = current_oversampling_algorithm.clone();
        let current_display_mode_ptr = current_display_mode.clone();
        let current_display_scope_ptr = current_display_scope.clone();
        let current_strict_dbfs_ticks_ptr = current_strict_dbfs_ticks.clone();
        let current_grid_step_x_ptr = current_grid_step_x.clone();
        let current_grid_step_y_ptr = current_grid_step_y.clone();
        let open_save_modal_ptr = self.open_save_modal.clone();
        let open_msg_modal_ptr = self.open_msg_modal.clone();
        let open_about_modal_ptr = self.open_about_modal.clone();
        let open_settings_modal_ptr = self.open_settings_modal.clone();
        let settings_tab_ptr = self.settings_tab.clone();
        let segment_generator_kind_ptr = self.segment_generator_kind.clone();
        let segment_generator_cycles_ptr = self.segment_generator_cycles.clone();
        let segment_generator_steps_ptr = self.segment_generator_steps.clone();
        let segment_generator_active_ptr = self.segment_generator_active.clone();
        let help_panel_title_ptr = self.help_panel_title.clone();
        let help_panel_text_ptr = self.help_panel_text.clone();
        let saving_preset_name_ptr = self.saving_preset_name.clone();
        let msg_modal_title_ptr = self.msg_modal_title.clone();
        let msg_modal_content_ptr = self.msg_modal_content.clone();
        create_egui_editor(
            params.editor_state.clone(),
            EditorVisualCache::default(),
            |ctx, state| {
                let mut fonts = egui::FontDefinitions::default();
                fonts.font_data.insert(
                    "maple-mono".to_string(),
                    std::sync::Arc::new(egui::FontData::from_static(
                        include_bytes!("../assets/MapleMono-NF-CN-Regular.ttf"),
                    )),
                );
                fonts
                    .families
                    .get_mut(&egui::FontFamily::Proportional)
                    .unwrap()
                    .insert(0, "maple-mono".to_string());
                ctx.set_fonts(fonts);

                let load_image = |name: &str, image: &[u8]| -> TextureHandle {
                    let image = load_image_from_memory(image);
                    let image = match image {
                        Ok(image) => image,
                        Err(err) => {
                            nih_error!("Couldn't load image {}. Reason: {:?}. Falling back to example image", name, err);
                            ColorImage::example()
                        }
                    };
                    ctx.load_texture(name, image, Default::default())
                };

                state.background = Some(load_image("background", include_bytes!("../assets/bg.png")));
                state.save_icon = Some(load_image("save", include_bytes!("../assets/save.png")));
                state.und3ath_logo = Some(load_image(
                    "und3ath_logo",
                    include_bytes!("../presets/Producers/UnD3ath/producer_logo.png"),
                ));
            },
            move |ctx, setter, state| {
                let theme = SoutTheme::new();
                sout_ui::set_theme(ctx, theme);
                let bg_texture = state
                    .background
                    .clone()
                    .unwrap_or_else(|| ctx.load_texture("background_fallback", ColorImage::example(), Default::default()));
                let save_texture = state
                    .save_icon
                    .clone()
                    .unwrap_or_else(|| ctx.load_texture("save_fallback", ColorImage::example(), Default::default()));
                let und3ath_logo = state
                    .und3ath_logo
                    .clone()
                    .unwrap_or_else(|| ctx.load_texture("und3ath_logo_fallback", ColorImage::example(), Default::default()));

                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(egui::Color32::BLACK).inner_margin(0.0))
                    .show(ctx, |ui| {
                        let default_help_title = "SA Waver";
                        let default_help_text = "by sout audio";
                        let mut hovered_help_title: Option<&'static str> = None;
                        let mut hovered_help_text: Option<&'static str> = None;
                        let current_help_title = help_panel_title_ptr
                            .try_lock()
                            .map(|text| text.clone())
                            .unwrap_or_else(|| default_help_title.to_string());
                        let current_help_text = help_panel_text_ptr
                            .try_lock()
                            .map(|text| text.clone())
                            .unwrap_or_else(|| default_help_text.to_string());

                        sync_lut_cache_from_state(
                            &lookup_curve,
                            &curve_dirty,
                            &lut_cache,
                            current_resolution_ptr.load(Ordering::Relaxed),
                            symmetry_mode_ptr.load(Ordering::Relaxed),
                        );

                        let bg_img = egui::Shape::image(
                            bg_texture.id(),
                            ui.max_rect(),
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );

                        ui.painter().add(bg_img);

                        ui.vertical(|ui| {
                            // top bar
                            let response = egui::Frame::new()
                                .inner_margin(Vec2::new(12.0, 10.0))
                                .fill(Color32::from_hex("#423b36").unwrap())
                                .shadow(egui::Shadow {
                                    offset: [0, 4],
                                    blur: 64,
                                    spread: 0,
                                    color: Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.2),
                                })
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());

                                    ui.horizontal(|ui| {
                                        ui.heading("SA Waver");

                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.add_space(4.0);

                                            let img_size = egui::vec2(12.0, 12.0);
                                            let img_src = egui::load::SizedTexture::new(save_texture.id(), img_size);

                                            ui.scope(|ui| {
                                                let visuals = ui.visuals_mut();
                                                sout_ui::make_ghost_button_visuals(visuals);

                                                ui.allocate_ui(egui::vec2(24.0, ui.available_height()), |ui| {
                                                    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                                                        ui.add_space(4.0);

                                                        let response = ui.add_sized(
                                                            egui::vec2(24.0, 24.0),
                                                            egui::Button::new(
                                                                RichText::new(" ")
                                                                    .size(14.0)
                                                                    .color(Color32::TRANSPARENT),
                                                            ),
                                                        );

                                                        let icon_pos = response.rect.center() + egui::vec2(-2.5, -0.8);
                                                        ui.painter().text(
                                                            icon_pos,
                                                            egui::Align2::CENTER_CENTER,
                                                            "",
                                                            egui::FontId::proportional(14.0),
                                                            Color32::from_hex("#FFEAD0").unwrap(),
                                                        );

                                                        if response.clicked() {
                                                            open_about_modal_ptr.store(true, Ordering::Relaxed);
                                                        }
                                                    });
                                                });
                                            });

                                            ui.scope(|ui| {
                                                let visuals = ui.visuals_mut();
                                                sout_ui::make_ghost_button_visuals(visuals);

                                                ui.allocate_ui(egui::vec2(24.0, ui.available_height()), |ui| {
                                                    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                                                        ui.add_space(4.0);

                                                        if ui
                                                            .add(egui::Button::image(img_src).min_size(egui::vec2(24.0, 24.0)))
                                                            .clicked()
                                                        {
                                                            println!("Save clicked");
                                                            if let Some(current_preset_guard) = current_preset_ptr.try_lock() {
                                                                let suggestion = next_preset_version_name(&current_preset_guard);
                                                                if let Some(mut name_guard) = saving_preset_name_ptr.try_lock() {
                                                                    *name_guard = suggestion;
                                                                }
                                                            }
                                                            open_save_modal_ptr.store(true, Ordering::Relaxed);
                                                        }
                                                    });
                                                });
                                            });

                                            if open_save_modal_ptr.load(Ordering::Relaxed) {
                                                if let Some(mut name_guard) = saving_preset_name_ptr.try_lock() {
                                                    let modal = Modal::new(Id::new("Save Modal")).show(ui.ctx(), |ui| {
                                                        ui.set_width(250.0);

                                                        ui.heading("Save Preset");

                                                        ui.label("Name:");

                                                        ui.text_edit_singleline(&mut *name_guard);

                                                        egui::Sides::new().show(
                                                            ui,
                                                            |_ui| {},
                                                            |ui| {
                                                                if ui.button("Save").clicked() {
                                                                    let show_msg_modal = |title: &str, message: String| {
                                                                        open_msg_modal_ptr.store(true, Ordering::Relaxed);
                                                                        if let (Some(mut title_lock), Some(mut content_lock)) = (
                                                                            msg_modal_title_ptr.try_lock(),
                                                                            msg_modal_content_ptr.try_lock(),
                                                                        ) {
                                                                            *title_lock = title.to_string();
                                                                            *content_lock = message;
                                                                        }
                                                                    };

                                                                    match fs::build_preset_path(&name_guard) {
                                                                        Ok(path) => {
                                                                            if let Some(curve) = lookup_curve.try_lock() {
                                                                                let snapshot = capture_plot_state(
                                                                                    &curve,
                                                                                    symmetry_mode_ptr.load(Ordering::Relaxed),
                                                                                );

                                                                                if let Err(err) = save_preset_file(&path, &snapshot) {
                                                                                    show_msg_modal(
                                                                                        "Error",
                                                                                        format!(
                                                                                            "Error saving curve: {} \n\tError: {:?}",
                                                                                            path, err
                                                                                        ),
                                                                                    );
                                                                                } else {
                                                                                    show_msg_modal(
                                                                                        "Success",
                                                                                        format!("Saved preset to: {}", path),
                                                                                    );
                                                                                    *current_preset_ptr.lock() = path.clone();
                                                                                    *params.saved_plot_state.lock() = snapshot;
                                                                                    plot_dirty_ptr.store(false, Ordering::Relaxed);
                                                                                    let presets = fs::get_presets().unwrap_or_default();
                                                                                    *presets_ptr.lock() = presets;
                                                                                }
                                                                            }
                                                                        }
                                                                        Err(err) => {
                                                                            show_msg_modal("Error", format!("Error build path: {}", err));
                                                                        }
                                                                    }

                                                                    open_save_modal_ptr.store(false, Ordering::Relaxed);
                                                                }
                                                                if ui.button("Cancel").clicked() {
                                                                    open_save_modal_ptr.store(false, Ordering::Relaxed);
                                                                }
                                                            },
                                                        );
                                                    });

                                                    if modal.should_close() {
                                                        open_save_modal_ptr.store(false, Ordering::Relaxed);
                                                    }
                                                }
                                            }

                                            if open_msg_modal_ptr.load(Ordering::Relaxed) {
                                                if let (Some(title_guard), Some(content_guard)) =
                                                    (msg_modal_title_ptr.try_lock(), msg_modal_content_ptr.try_lock())
                                                {
                                                    let modal = Modal::new(Id::new("Msg Modal")).show(ui.ctx(), |ui| {
                                                        ui.set_width(250.0);

                                                        ui.heading(title_guard.as_str());

                                                        ui.label(content_guard.as_str());

                                                        egui::Sides::new().show(
                                                            ui,
                                                            |_ui| {},
                                                            |ui| {
                                                                if ui.button("Done").clicked() {
                                                                    open_msg_modal_ptr.store(false, Ordering::Relaxed);
                                                                }
                                                            },
                                                        );
                                                    });

                                                    if modal.should_close() {
                                                        open_msg_modal_ptr.store(false, Ordering::Relaxed);
                                                    }
                                                }
                                            }

                                            if open_about_modal_ptr.load(Ordering::Relaxed) {
                                                let modal = Modal::new(Id::new("About Modal")).show(ui.ctx(), |ui| {
                                                    ui.set_width(420.0);
                                                    ui.set_min_width(420.0);

                                                    ui.vertical(|ui| {
                                                        ui.heading("SA Waver");
                                                        ui.label(
                                                            RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                                                                .weak(),
                                                        );
                                                        ui.add_space(12.0);

                                                        ui.label(RichText::new("Links").strong());
                                                        ui.add_space(4.0);
                                                        ui.hyperlink_to("󰖟 Homepage", "https://audio.soout.top/sa_waver");
                                                        ui.hyperlink_to(" GitHub", "https://github.com/sout233/sa_waver");

                                                        ui.add_space(12.0);
                                                        ui.separator();
                                                        ui.add_space(8.0);

                                                        ui.label(RichText::new("Author").strong());
                                                        ui.add_space(4.0);
                                                        ui.horizontal_wrapped(|ui| {
                                                            ui.label("sout");
                                                            ui.label("-");
                                                            ui.hyperlink_to("github.com/sout233", "https://github.com/sout233");
                                                        });

                                                        ui.add_space(10.0);
                                                        ui.label(RichText::new("Special Thanks").strong());
                                                        ui.add_space(4.0);
                                                        ui.horizontal_wrapped(|ui| {
                                                            ui.label("NullTech_EndBlue");
                                                            ui.label("-");
                                                            ui.hyperlink_to(
                                                                "space.bilibili.com/487390529",
                                                                "https://space.bilibili.com/487390529",
                                                            );
                                                        });
                                                        ui.horizontal_wrapped(|ui| {
                                                            ui.label("UnD3ath");
                                                            ui.label("-");
                                                            ui.hyperlink_to(
                                                                "space.bilibili.com/224632474",
                                                                "https://space.bilibili.com/224632474",
                                                            );
                                                        });
                                                        ui.horizontal_wrapped(|ui| {
                                                            ui.label("RHYX");
                                                            ui.label("-");
                                                            ui.hyperlink_to(
                                                                "space.bilibili.com/256700038",
                                                                "https://space.bilibili.com/256700038",
                                                            );
                                                        });

                                                        ui.add_space(10.0);
                                                        ui.label(RichText::new("Thanks").strong());
                                                        ui.add_space(4.0);
                                                        ui.horizontal_wrapped(|ui| {
                                                            ui.label("Aqua Sounds");
                                                            ui.label("-");
                                                            ui.hyperlink_to(
                                                                "www.aqua-sounds.top",
                                                                "https://www.aqua-sounds.top/",
                                                            );
                                                        });
                                                    });

                                                    ui.add_space(10.0);

                                                    egui::Sides::new().show(
                                                        ui,
                                                        |_ui| {},
                                                        |ui| {
                                                            if ui.button("Close").clicked() {
                                                                open_about_modal_ptr.store(false, Ordering::Relaxed);
                                                            }
                                                        },
                                                    );
                                                });

                                                if modal.should_close() {
                                                    open_about_modal_ptr.store(false, Ordering::Relaxed);
                                                }
                                            }

                                            if open_settings_modal_ptr.load(Ordering::Relaxed) {
                                                let modal = Modal::new(Id::new("Settings Modal")).show(ui.ctx(), |ui| {
                                                    ui.set_width(620.0);
                                                    ui.set_min_width(620.0);

                                                    let frame_fill = Color32::from_hex("#2f2a27").unwrap();
                                                    let panel_fill = Color32::from_hex("#3a3430").unwrap();
                                                    let tab_idle = Color32::from_hex("#4a433e").unwrap();
                                                    let tab_active = Color32::from_hex("#DB9160").unwrap();
                                                    let border_color =
                                                        Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.35);
                                                    let mut current_settings_tab =
                                                        settings_tab_ptr.load(Ordering::Relaxed);

                                                    let section_label = |ui: &mut egui::Ui, title: &str| {
                                                        ui.label(
                                                            RichText::new(title)
                                                                .size(12.0)
                                                                .color(Color32::from_hex("#FFEAD0").unwrap()),
                                                        );
                                                        ui.add_space(6.0);
                                                    };

                                                    let tab_button = |ui: &mut egui::Ui, label: &str, selected: bool| {
                                                        let fill = if selected { tab_active } else { tab_idle };
                                                        let text = if selected {
                                                            Color32::from_hex("#1c1917").unwrap()
                                                        } else {
                                                            Color32::from_hex("#FFEAD0").unwrap()
                                                        };
                                                        ui.add_sized(
                                                            [132.0, 30.0],
                                                            egui::Button::new(RichText::new(label).color(text))
                                                                .fill(fill)
                                                                .stroke(egui::Stroke::new(1.0, Color32::TRANSPARENT)),
                                                        )
                                                    };

                                                    egui::Frame::new()
                                                        .fill(frame_fill)
                                                        .corner_radius(CornerRadiusF32::same(10.0))
                                                        .stroke(egui::Stroke::new(1.0, border_color))
                                                        .inner_margin(14.0)
                                                        .show(ui, |ui| {
                                                            ui.horizontal(|ui| {
                                                                ui.heading("Settings");
                                                                ui.with_layout(
                                                                    egui::Layout::right_to_left(egui::Align::Center),
                                                                    |ui| {
                                                                        ui.scope(|ui| {
                                                                            let visuals = ui.visuals_mut();
                                                                            sout_ui::make_ghost_button_visuals(visuals);

                                                                            let response = ui.add_sized(
                                                                                [24.0, 24.0],
                                                                                egui::Button::new(
                                                                                    RichText::new(" ")
                                                                                        .size(14.0)
                                                                                        .color(Color32::TRANSPARENT),
                                                                                ),
                                                                            );

                                                                            ui.painter().text(
                                                                                response.rect.center(),
                                                                                egui::Align2::CENTER_CENTER,
                                                                                "󰅖",
                                                                                egui::FontId::proportional(14.0),
                                                                                Color32::from_hex("#FFEAD0").unwrap(),
                                                                            );

                                                                            if response.clicked() {
                                                                                open_settings_modal_ptr
                                                                                    .store(false, Ordering::Relaxed);
                                                                            }
                                                                        });
                                                                    },
                                                                );
                                                            });

                                                            ui.add_space(10.0);
                                                            ui.painter().line_segment(
                                                                [ui.min_rect().left_bottom(), ui.min_rect().right_bottom()],
                                                                egui::Stroke::new(
                                                                    1.0,
                                                                    Color32::from_hex("#FFEAD0")
                                                                        .unwrap()
                                                                        .gamma_multiply(0.15),
                                                                ),
                                                            );
                                                            ui.add_space(10.0);

                                                            ui.horizontal_top(|ui| {
                                                                egui::Frame::new()
                                                                    .fill(panel_fill)
                                                                    .corner_radius(CornerRadiusF32::same(8.0))
                                                                    .stroke(egui::Stroke::new(1.0, border_color))
                                                                    .inner_margin(10.0)
                                                                    .show(ui, |ui| {
                                                                        ui.set_width(135.0);
                                                                        ui.set_max_width(152.0);
                                                                        ui.vertical(|ui| {
                                                                            if tab_button(
                                                                                ui,
                                                                                "Chart View",
                                                                                current_settings_tab == 0,
                                                                            )
                                                                            .clicked()
                                                                            {
                                                                                current_settings_tab = 0;
                                                                            }
                                                                            ui.add_space(6.0);
                                                                            if tab_button(
                                                                                ui,
                                                                                "Waveform View",
                                                                                current_settings_tab == 1,
                                                                            )
                                                                            .clicked()
                                                                            {
                                                                                current_settings_tab = 1;
                                                                            }
                                                                            ui.add_space(6.0);
                                                                            if tab_button(
                                                                                ui,
                                                                                "Audio",
                                                                                current_settings_tab == 2,
                                                                            )
                                                                            .clicked()
                                                                            {
                                                                                current_settings_tab = 2;
                                                                            }
                                                                        });
                                                                    });

                                                                ui.add_space(12.0);

                                                                egui::Frame::new()
                                                                    .fill(panel_fill)
                                                                    .corner_radius(CornerRadiusF32::same(8.0))
                                                                    .stroke(egui::Stroke::new(1.0, border_color))
                                                                    .inner_margin(12.0)
                                                                    .show(ui, |ui| {
                                                                        ui.set_width(380.0);
                                                                        ui.set_max_width(410.0);
                                                                        ui.set_min_height(320.0);
                                                                        ui.vertical(|ui| {
                                                                            match current_settings_tab {
                                                                                0 => {
                                                                                    section_label(ui, "Scale");

                                                                                    ui.scope(|ui| {
                                                                                        let visuals = ui.visuals_mut();
                                                                                        sout_ui::make_combobox_visuals(
                                                                                            visuals,
                                                                                        Color32::from_hex("#554e4a")
                                                                                            .unwrap(),
                                                                                    );
                                                                                    ui.style_mut().spacing.interact_size.y = 24.0;

                                                                                    let current_mode = current_display_mode_ptr
                                                                                        .load(Ordering::Relaxed);
                                                                                    let mut selected_mode = current_mode;

                                                                                    egui::ComboBox::from_id_salt(
                                                                                        "settings_display_mode_selector",
                                                                                    )
                                                                                    .width(220.0)
                                                                                    .selected_text(display_mode_label(
                                                                                        current_mode,
                                                                                    ))
                                                                                    .show_ui(ui, |ui| {
                                                                                        ui.selectable_value(
                                                                                            &mut selected_mode,
                                                                                            DISPLAY_MODE_LINEAR,
                                                                                            "Linear",
                                                                                        );
                                                                                        ui.selectable_value(
                                                                                            &mut selected_mode,
                                                                                            DISPLAY_MODE_DBFS,
                                                                                            "dBFS",
                                                                                        );
                                                                                    });

                                                                                    if selected_mode != current_mode {
                                                                                        current_display_mode_ptr.store(
                                                                                            selected_mode,
                                                                                            Ordering::Relaxed,
                                                                                        );
                                                                                    }
                                                                                    });

                                                                                    ui.add_space(12.0);
                                                                                    section_label(ui, "dBFS Scope");

                                                                                    ui.scope(|ui| {
                                                                                        let visuals = ui.visuals_mut();
                                                                                        sout_ui::make_combobox_visuals(
                                                                                            visuals,
                                                                                        Color32::from_hex("#554e4a")
                                                                                            .unwrap(),
                                                                                    );
                                                                                    ui.style_mut().spacing.interact_size.y = 24.0;

                                                                                    let current_scope = current_display_scope_ptr
                                                                                        .load(Ordering::Relaxed);
                                                                                    let mut selected_scope = current_scope;
                                                                                    let current_display_mode =
                                                                                        current_display_mode_ptr
                                                                                            .load(Ordering::Relaxed);

                                                                                    egui::ComboBox::from_id_salt(
                                                                                        "settings_display_scope_selector",
                                                                                    )
                                                                                    .width(220.0)
                                                                                    .selected_text(display_scope_label(
                                                                                        current_scope,
                                                                                    ))
                                                                                    .show_ui(ui, |ui| {
                                                                                        ui.selectable_value(
                                                                                            &mut selected_scope,
                                                                                            DISPLAY_SCOPE_Y_ONLY,
                                                                                            "Y only",
                                                                                        );
                                                                                        ui.selectable_value(
                                                                                            &mut selected_scope,
                                                                                            DISPLAY_SCOPE_XY,
                                                                                            "X+Y",
                                                                                        );
                                                                                    });

                                                                                    if selected_scope != current_scope {
                                                                                        if current_display_mode
                                                                                            == DISPLAY_MODE_DBFS
                                                                                            && selected_scope
                                                                                                == DISPLAY_SCOPE_Y_ONLY
                                                                                        {
                                                                                            let current_symmetry_mode =
                                                                                                symmetry_mode_ptr
                                                                                                    .load(Ordering::Relaxed);
                                                                                            if let Some(mut curve) =
                                                                                                lookup_curve.try_lock()
                                                                                            {
                                                                                                if is_default_linear_curve(
                                                                                                    &curve,
                                                                                                    current_symmetry_mode,
                                                                                                ) {
                                                                                                    *curve =
                                                                                                        build_default_dbfs_curve(
                                                                                                            current_symmetry_mode,
                                                                                                        );
                                                                                                    curve_dirty.store(
                                                                                                        true,
                                                                                                        Ordering::Relaxed,
                                                                                                    );
                                                                                                    plot_dirty_ptr.store(
                                                                                                        true,
                                                                                                        Ordering::Relaxed,
                                                                                                    );
                                                                                                    if let Some(
                                                                                                        mut editor_ui,
                                                                                                    ) = editor.try_lock()
                                                                                                    {
                                                                                                        editor_ui.fit_to_curve(
                                                                                                            &curve,
                                                                                                        );
                                                                                                    }
                                                                                                }
                                                                                            }
                                                                                        }
                                                                                        current_display_scope_ptr.store(
                                                                                            selected_scope,
                                                                                            Ordering::Relaxed,
                                                                                        );
                                                                                    }
                                                                                    });

                                                                                    ui.add_space(12.0);
                                                                                    section_label(ui, "Grid");

                                                                                let mut strict_dbfs_ticks =
                                                                                    current_strict_dbfs_ticks_ptr
                                                                                        .load(Ordering::Relaxed);
                                                                                if ui
                                                                                    .checkbox(
                                                                                        &mut strict_dbfs_ticks,
                                                                                        "Use experimental dB spacing",
                                                                                    )
                                                                                    .changed()
                                                                                {
                                                                                    current_strict_dbfs_ticks_ptr.store(
                                                                                        strict_dbfs_ticks,
                                                                                        Ordering::Relaxed,
                                                                                    );
                                                                                }

                                                                                ui.add_space(8.0);

                                                                                let grid_step_enabled =
                                                                                    current_display_mode_ptr
                                                                                        .load(Ordering::Relaxed)
                                                                                        == DISPLAY_MODE_LINEAR;
                                                                                let mut grid_step_x =
                                                                                    current_grid_step_x_ptr
                                                                                        .load(Ordering::Relaxed)
                                                                                        as f32
                                                                                        / 1000.0;
                                                                                let mut grid_step_y =
                                                                                    current_grid_step_y_ptr
                                                                                        .load(Ordering::Relaxed)
                                                                                        as f32
                                                                                        / 1000.0;

                                                                                    ui.add_enabled_ui(grid_step_enabled, |ui| {
                                                                                        ui.horizontal(|ui| {
                                                                                            ui.label("Horizontal");
                                                                                            if ui
                                                                                            .add(
                                                                                                egui::DragValue::new(
                                                                                                    &mut grid_step_x,
                                                                                                )
                                                                                                .speed(0.01)
                                                                                                .range(0.01..=1.0)
                                                                                                .fixed_decimals(3),
                                                                                            )
                                                                                            .changed()
                                                                                        {
                                                                                            let stored = (grid_step_x
                                                                                                .clamp(0.01, 1.0)
                                                                                                * 1000.0)
                                                                                                .round()
                                                                                                as usize;
                                                                                            current_grid_step_x_ptr.store(
                                                                                                stored.max(1),
                                                                                                Ordering::Relaxed,
                                                                                            );
                                                                                        }
                                                                                    });

                                                                                    ui.horizontal(|ui| {
                                                                                        ui.label("Vertical");
                                                                                        if ui
                                                                                            .add(
                                                                                                egui::DragValue::new(
                                                                                                    &mut grid_step_y,
                                                                                                )
                                                                                                .speed(0.01)
                                                                                                .range(0.01..=1.0)
                                                                                                .fixed_decimals(3),
                                                                                            )
                                                                                            .changed()
                                                                                        {
                                                                                            let stored = (grid_step_y
                                                                                                .clamp(0.01, 1.0)
                                                                                                * 1000.0)
                                                                                                .round()
                                                                                                as usize;
                                                                                            current_grid_step_y_ptr.store(
                                                                                                stored.max(1),
                                                                                                Ordering::Relaxed,
                                                                                            );
                                                                                        }
                                                                                    });
                                                                                    });

                                                                                    if !grid_step_enabled {
                                                                                        ui.label(
                                                                                            "Only active in Linear display mode.",
                                                                                        );
                                                                                    }
                                                                                }
                                                                                1 => {
                                                                                    section_label(ui, "Timebase");

                                                                                    ui.scope(|ui| {
                                                                                        let visuals = ui.visuals_mut();
                                                                                        sout_ui::make_combobox_visuals(
                                                                                            visuals,
                                                                                        Color32::from_hex("#554e4a")
                                                                                            .unwrap(),
                                                                                    );
                                                                                    ui.style_mut().spacing.interact_size.y = 24.0;

                                                                                    let current_val = current_timebase_ptr
                                                                                        .load(Ordering::Relaxed);
                                                                                    let mut selected_val = current_val;

                                                                                    egui::ComboBox::from_id_salt(
                                                                                        "settings_timebase_selector",
                                                                                    )
                                                                                    .width(220.0)
                                                                                    .selected_text(format!(
                                                                                        "{}",
                                                                                        current_val
                                                                                    ))
                                                                                    .show_ui(ui, |ui| {
                                                                                        ui.selectable_value(
                                                                                            &mut selected_val,
                                                                                            128,
                                                                                            "128",
                                                                                        );
                                                                                        ui.selectable_value(
                                                                                            &mut selected_val,
                                                                                            256,
                                                                                            "256",
                                                                                        );
                                                                                        ui.selectable_value(
                                                                                            &mut selected_val,
                                                                                            512,
                                                                                            "512",
                                                                                        );
                                                                                        ui.selectable_value(
                                                                                            &mut selected_val,
                                                                                            1024,
                                                                                            "1024",
                                                                                        );

                                                                                        ui.separator();
                                                                                        ui.horizontal(|ui| {
                                                                                            ui.label("Custom:");
                                                                                            ui.add(
                                                                                                egui::DragValue::new(
                                                                                                    &mut selected_val,
                                                                                                )
                                                                                                .clamp_existing_to_range(
                                                                                                    true,
                                                                                                )
                                                                                                .range(64..=4096),
                                                                                            );
                                                                                        });
                                                                                    });

                                                                                    if selected_val != current_val {
                                                                                        current_timebase_ptr.store(
                                                                                            selected_val,
                                                                                            Ordering::Relaxed,
                                                                                        );
                                                                                    }
                                                                                    });

                                                                                    // ui.add_space(12.0);
                                                                                    // section_label(ui, "Preview");
                                                                                    // ui.label(
                                                                                    //     "Waveform preview behavior and density live here.",
                                                                                    // );
                                                                                }
                                                                                _ => {
                                                                                    section_label(ui, "Interpolation");

                                                                                    ui.scope(|ui| {
                                                                                        let visuals = ui.visuals_mut();
                                                                                        sout_ui::make_combobox_visuals(
                                                                                            visuals,
                                                                                        Color32::from_hex("#554e4a")
                                                                                            .unwrap(),
                                                                                    );
                                                                                    ui.style_mut().spacing.interact_size.y = 24.0;

                                                                                    let current_mode =
                                                                                        current_interpolation_mode_ptr
                                                                                            .load(Ordering::Relaxed);
                                                                                    let mut selected_mode = current_mode;

                                                                                    egui::ComboBox::from_id_salt(
                                                                                        "settings_interpolation_selector",
                                                                                    )
                                                                                    .width(220.0)
                                                                                    .selected_text(
                                                                                        interpolation_mode_label(
                                                                                            current_mode,
                                                                                        ),
                                                                                    )
                                                                                    .show_ui(ui, |ui| {
                                                                                        ui.selectable_value(
                                                                                            &mut selected_mode,
                                                                                            INTERPOLATION_MODE_LINEAR,
                                                                                            "Linear",
                                                                                        );
                                                                                        ui.selectable_value(
                                                                                            &mut selected_mode,
                                                                                            INTERPOLATION_MODE_COSINE,
                                                                                            "Cosine",
                                                                                        );
                                                                                        ui.selectable_value(
                                                                                            &mut selected_mode,
                                                                                            INTERPOLATION_MODE_HERMITE,
                                                                                            "Hermite",
                                                                                        );
                                                                                    });

                                                                                    if selected_mode != current_mode {
                                                                                        current_interpolation_mode_ptr.store(
                                                                                            selected_mode,
                                                                                            Ordering::Relaxed,
                                                                                        );
                                                                                    }
                                                                                    });

                                                                                    ui.add_space(12.0);
                                                                                    section_label(ui, "Oversampling");

                                                                                    ui.scope(|ui| {
                                                                                        let visuals = ui.visuals_mut();
                                                                                        sout_ui::make_combobox_visuals(
                                                                                            visuals,
                                                                                        Color32::from_hex("#554e4a")
                                                                                            .unwrap(),
                                                                                    );
                                                                                    ui.style_mut().spacing.interact_size.y = 24.0;

                                                                                    let current_algo =
                                                                                        current_oversampling_algorithm_ptr
                                                                                            .load(Ordering::Relaxed);
                                                                                    let mut selected_algo = current_algo;

                                                                                    egui::ComboBox::from_id_salt(
                                                                                        "settings_oversampling_algorithm_selector",
                                                                                    )
                                                                                    .width(220.0)
                                                                                    .selected_text(
                                                                                        oversampling_algorithm_label(
                                                                                            current_algo,
                                                                                        ),
                                                                                    )
                                                                                    .show_ui(ui, |ui| {
                                                                                        ui.selectable_value(
                                                                                            &mut selected_algo,
                                                                                            OVERSAMPLING_ALGORITHM_LANCZOS3,
                                                                                            "Lanczos3",
                                                                                        );
                                                                                        ui.selectable_value(
                                                                                            &mut selected_algo,
                                                                                            OVERSAMPLING_ALGORITHM_FLAT_FIR,
                                                                                            "Flat FIR",
                                                                                        );
                                                                                    });

                                                                                    if selected_algo != current_algo {
                                                                                        current_oversampling_algorithm_ptr.store(
                                                                                            selected_algo,
                                                                                            Ordering::Relaxed,
                                                                                        );
                                                                                    }
                                                                                    });
                                                                                }
                                                                            }
                                                                        });
                                                                    });
                                                            });
                                                        });

                                                    settings_tab_ptr
                                                        .store(current_settings_tab, Ordering::Relaxed);
                                                });

                                                if modal.should_close() {
                                                    open_settings_modal_ptr.store(false, Ordering::Relaxed);
                                                }
                                            }

                                            ui.add_space(2.0);

                                            ui.scope(|ui| {
                                                let visuals = ui.visuals_mut();
                                                sout_ui::make_combobox_visuals(visuals, Color32::from_hex("#554e4a").unwrap());

                                                ui.style_mut().spacing.interact_size.y = 20.0;
                                                ui.style_mut().spacing.button_padding = egui::vec2(10.0, 4.0);

                                                if let Some(mut current_preset_guard) = current_preset_ptr.try_lock() {
                                                    let previous_preset = current_preset_guard.clone();
                                                    let plot_dirty = plot_dirty_ptr.load(Ordering::Relaxed);
                                                    let preset_label = if plot_dirty {
                                                        format!("{} *", preset_display_name(&current_preset_guard))
                                                    } else {
                                                        preset_display_name(&current_preset_guard)
                                                    };

                                                    let preset_menu_height = 10_000.0_f32;
                                                    let button_min_width = ui.spacing().combo_width;
                                                    let preset_button = egui::Button::new(
                                                        RichText::new(format!("{preset_label}  ▾")).color(Color32::from_hex("#FFEAD0").unwrap()),
                                                    )
                                                    .min_size(egui::vec2(button_min_width, ui.spacing().interact_size.y));
                                                    let preset_popup_id = ui.make_persistent_id("preset_selector");
                                                    let is_preset_menu_open = ui.memory(|mem| mem.is_popup_open(preset_popup_id));
                                                    if !is_preset_menu_open {
                                                        let presets = fs::get_presets().unwrap_or_default();
                                                        *presets_ptr.lock() = presets;
                                                    }
                                                    let response = egui::menu::menu_custom_button(ui, preset_button, |ui| {
                                                        ui.spacing_mut().interact_size.y = 18.0;
                                                        ui.spacing_mut().button_padding = egui::vec2(8.0, 2.0);
                                                        ui.spacing_mut().item_spacing.y = 1.0;
                                                        ui.set_min_width(button_min_width);
                                                        egui::ScrollArea::vertical()
                                                            .max_height(preset_menu_height)
                                                            .show(ui, |ui| {
                                                                ui.spacing_mut().interact_size.y = 18.0;
                                                                ui.spacing_mut().button_padding = egui::vec2(8.0, 2.0);
                                                                ui.spacing_mut().item_spacing.y = 1.0;
                                                                ui.set_min_width(button_min_width);

                                                                if ui
                                                                    .selectable_value(
                                                                        &mut *current_preset_guard,
                                                                        "./Default.ron".to_string(),
                                                                        "Default",
                                                                    )
                                                                    .clicked()
                                                                {
                                                                    ui.close_menu();
                                                                }

                                                                let builtin_presets = fs::get_builtin_presets();
                                                                let producers_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                                                                    .join("presets")
                                                                    .join("Producers");
                                                                let producer_presets: Vec<String> = builtin_presets
                                                                    .into_iter()
                                                                    .filter(|preset| {
                                                                        std::path::Path::new(preset).starts_with(&producers_root)
                                                                    })
                                                                    .collect();
                                                                if !producer_presets.is_empty() {
                                                                    ui.separator();
                                                                    ui.menu_button("Producers", |ui| {
                                                                        ui.set_max_height(preset_menu_height);
                                                                        show_preset_tree(
                                                                            ui,
                                                                            &mut *current_preset_guard,
                                                                            &producer_presets,
                                                                            &producers_root,
                                                                            preset_menu_height,
                                                                            Some(&und3ath_logo),
                                                                        );
                                                                    });
                                                                }

                                                                if let Some(presets) = presets_ptr.try_lock() {
                                                                    if !presets.is_empty() {
                                                                        ui.separator();
                                                                        for preset in presets.iter() {
                                                                            if ui
                                                                                .selectable_value(
                                                                                    &mut *current_preset_guard,
                                                                                    preset.to_string(),
                                                                                    preset_display_name(preset),
                                                                                )
                                                                                .clicked()
                                                                            {
                                                                                ui.close_menu();
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            });
                                                    })
                                                    .response;

                                                    if response.hovered() {
                                                        hovered_help_title = Some("Preset");
                                                        hovered_help_text = Some("Load a saved preset. A star means the current state is unsaved.");
                                                    }

                                                    if *current_preset_guard != previous_preset {
                                                        println!("Preset changed to: {}", *current_preset_guard);

                                                        let preset_data =
                                                            std::fs::read(format!("{}", *current_preset_guard)).unwrap_or_default();
                                                        if let Some(mut curve) = lookup_curve.try_lock() {
                                                            match load_preset_file(&preset_data) {
                                                                Ok(snapshot) => {
                                                                    *curve = snapshot.curve.clone();
                                                                    symmetry_mode_ptr.store(
                                                                        snapshot.symmetry_mode,
                                                                        Ordering::Relaxed,
                                                                    );
                                                                    curve_dirty.store(true, Ordering::Relaxed);
                                                                    *params.saved_plot_state.lock() = snapshot;
                                                                    if let Some(mut editor_ui) = editor.try_lock() {
                                                                        editor_ui.fit_to_curve(&curve);
                                                                    }
                                                                    plot_dirty_ptr.store(false, Ordering::Relaxed);
                                                                }
                                                                Err(e) => {
                                                                    println!("Failed to load preset: {}", e);
                                                                    *curve = LookupCurve::load_from_bytes(include_bytes!("default.ron"))
                                                                        .unwrap();
                                                                    symmetry_mode_ptr.store(
                                                                        DEFAULT_SYMMETRY_MODE,
                                                                        Ordering::Relaxed,
                                                                    );
                                                                    curve_dirty.store(true, Ordering::Relaxed);
                                                                    *params.saved_plot_state.lock() = capture_plot_state(
                                                                        &curve,
                                                                        DEFAULT_SYMMETRY_MODE,
                                                                    );
                                                                    if let Some(mut editor_ui) = editor.try_lock() {
                                                                        editor_ui.fit_to_curve(&curve);
                                                                    }
                                                                    plot_dirty_ptr.store(false, Ordering::Relaxed);
                                                                }
                                                            }
                                                        }
                                                    }

                                                }
                                            })
                                        });
                                    });
                                })
                                .response;

                            ui.painter().line_segment(
                                [response.rect.left_bottom(), response.rect.right_bottom()],
                                egui::Stroke::new(1.0, egui::Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.5)),
                            );

                            // ui.label("by Sout Audio");
                            // ui.add(widgets::ParamSlider::for_param(&params.pre_gain, setter));

                            let current_t = latest_input_ptr.load(Ordering::Relaxed);
                            let mut preview_output_value = None;
                            let mut preview_output_screen_y = None;
                            let mut top_y = 0.0;
                            let mut bottom_y = 0.0;
                            let mut plot_has_focus = false;
                            let mut selected_knot_count = 0usize;
                            let mut selected_knot_ids_for_generator: Vec<usize> = Vec::new();
                            let mut selected_generator = match segment_generator_kind_ptr.load(Ordering::Relaxed) {
                                1 => SegmentGeneratorKind::Triangle,
                                2 => SegmentGeneratorKind::Square,
                                3 => SegmentGeneratorKind::Stairs,
                                _ => SegmentGeneratorKind::Sine,
                            };
                            let mut generator_cycles =
                                segment_generator_cycles_ptr.load(Ordering::Relaxed).clamp(1, 32);
                            let mut generator_steps =
                                segment_generator_steps_ptr.load(Ordering::Relaxed).clamp(2, 64);
                            let previous_generator_active =
                                segment_generator_active_ptr.load(Ordering::Relaxed);

                            let colored_waveform = colored_waveform_ptr.load(Ordering::Relaxed);
                            let current_timebase = current_timebase_ptr.load(Ordering::Relaxed);
                            let linear_ext_enabled = linear_ext_enabled_ptr.load(Ordering::Relaxed);
                            let current_symmetry_mode = symmetry_mode_ptr.load(Ordering::Relaxed);

                            egui::Frame::new().inner_margin(12.0).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    if let (Some(mut curve), Some(mut editor_ui)) = (lookup_curve.try_lock(), editor.try_lock()) {
                                        let side_length = ui.available_height().max(350.0);
                                        let square_size = egui::Vec2::splat(side_length);
                                        let editor_domain_min = if current_symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
                                            -1.0
                                        } else {
                                            0.0
                                        };
                                        let editor_domain_max = 1.0;
                                        let current_display_mode =
                                            current_display_mode_ptr.load(Ordering::Relaxed);
                                        let current_display_scope =
                                            current_display_scope_ptr.load(Ordering::Relaxed);
                                        let display_input = audio_input_to_chart_input(
                                            current_t,
                                            current_symmetry_mode,
                                            current_display_mode,
                                            current_display_scope,
                                        );
                                        let should_draw_manual_indicator =
                                            linear_ext_enabled && display_input.abs() > editor_domain_max;
                                        let sample_for_editor = if should_draw_manual_indicator {
                                            None
                                        } else {
                                            Some(display_input.clamp(editor_domain_min, editor_domain_max))
                                        };
                                        editor_ui.bipolar = current_symmetry_mode == SYMMETRY_MODE_ASYMMETRIC;
                                        let strict_dbfs_view =
                                            current_display_mode == DISPLAY_MODE_DBFS
                                                && current_strict_dbfs_ticks_ptr.load(Ordering::Relaxed);
                                        editor_ui.configure_dbfs_mode(
                                            strict_dbfs_view,
                                            current_display_mode == DISPLAY_MODE_DBFS && !strict_dbfs_view,
                                            current_display_mode == DISPLAY_MODE_DBFS
                                                && current_display_scope == DISPLAY_SCOPE_XY,
                                            strict_dbfs_view,
                                        );
                                        if current_display_mode == DISPLAY_MODE_LINEAR {
                                            editor_ui.grid_step_x =
                                                current_grid_step_x_ptr.load(Ordering::Relaxed) as f32 / 1000.0;
                                            editor_ui.grid_step_y =
                                                current_grid_step_y_ptr.load(Ordering::Relaxed) as f32 / 1000.0;
                                        } else {
                                            editor_ui.grid_step_x = 0.1;
                                            editor_ui.grid_step_y = 0.1;
                                        }

                                        // curve editor
                                        egui::Frame::new()
                                            .corner_radius(CornerRadiusF32::same(8.0))
                                            .shadow(egui::Shadow {
                                                offset: [0, 4],
                                                blur: 24,
                                                spread: 0,
                                                color: egui::Color32::from_rgba_unmultiplied(0, 0, 0, 128),
                                            })
                                            .stroke(egui::Stroke::new(
                                                1.0,
                                                egui::Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.5),
                                            ))
                                            .fill(Color32::from_hex("#1c1917").unwrap())
                                            .show(ui, |ui| {
                                                ui.vertical(|ui| {
                                                    ui.allocate_ui(square_size, |ui| {
                                                        let curve_rect = ui.max_rect();

                                                        if editor_ui.ui(ui, &mut curve, sample_for_editor) {
                                                            curve_dirty.store(true, Ordering::Relaxed);
                                                            plot_dirty_ptr.store(true, Ordering::Relaxed);
                                                        }
                                                        plot_has_focus = editor_ui.has_focus;
                                                        selected_knot_count = editor_ui.selected_knot_ids.len();
                                                        selected_knot_ids_for_generator =
                                                            editor_ui.selected_knot_ids.iter().copied().collect();

                                                        let curve_to_screen =
                                                            |curve_x: f32, curve_y: f32| -> Pos2 {
                                                                editor_ui.curve_to_screen(
                                                                    curve_rect,
                                                                    BevyVec2::new(curve_x, curve_y),
                                                                )
                                                            };

                                                        top_y = curve_to_screen(1.0, 1.0).y;
                                                        bottom_y = if current_symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
                                                            curve_to_screen(-1.0, -1.0).y
                                                        } else {
                                                            curve_to_screen(0.0, 0.0).y
                                                        };

                                                        if should_draw_manual_indicator {
                                                            if let Some(lut) = lut_cache.try_lock() {
                                                                let actual_curve_y = curve_lookup(
                                                                    &lut,
                                                                    current_t,
                                                                    linear_ext_enabled,
                                                                    current_interpolation_mode_ptr
                                                                        .load(Ordering::Relaxed),
                                                                    current_symmetry_mode,
                                                                    current_display_mode,
                                                                    current_display_scope,
                                                                );
                                                                let display_curve_y = curve_lookup_chart(
                                                                    &lut,
                                                                    display_input,
                                                                    linear_ext_enabled,
                                                                    current_interpolation_mode_ptr
                                                                        .load(Ordering::Relaxed),
                                                                    current_symmetry_mode,
                                                                );
                                                                if !display_curve_y.is_finite() {
                                                                    preview_output_screen_y = None;
                                                                    preview_output_value = None;
                                                                    return;
                                                                }
                                                                let visible_max_x = editor_ui.offset.x + editor_ui.scale.x;
                                                                let visible_min_x = editor_ui.offset.x;
                                                                let indicator_x =
                                                                    display_input.clamp(visible_min_x, visible_max_x);
                                                                let indicator_pos =
                                                                    curve_to_screen(indicator_x, display_curve_y);
                                                                if !indicator_pos.is_finite() {
                                                                    preview_output_screen_y = None;
                                                                    preview_output_value = None;
                                                                    return;
                                                                }

                                                                preview_output_screen_y = Some(indicator_pos.y);
                                                                preview_output_value = Some(actual_curve_y);

                                                                ui.painter().line_segment(
                                                                    [
                                                                        indicator_pos,
                                                                        egui::pos2(curve_rect.right(), indicator_pos.y),
                                                                    ],
                                                                    egui::Stroke::new(1.0, Color32::LIGHT_GREEN),
                                                                );

                                                                ui.painter()
                                                                    .circle_filled(indicator_pos, 3.0, Color32::LIGHT_GREEN);

                                                                if display_input > visible_max_x {
                                                                    let arrow_tip =
                                                                        egui::pos2(curve_rect.right() - 2.0, indicator_pos.y);
                                                                    let arrow_back_top =
                                                                        egui::pos2(curve_rect.right() - 10.0, indicator_pos.y - 5.0);
                                                                    let arrow_back_bottom =
                                                                        egui::pos2(curve_rect.right() - 10.0, indicator_pos.y + 5.0);

                                                                    ui.painter().add(egui::Shape::convex_polygon(
                                                                        vec![arrow_tip, arrow_back_top, arrow_back_bottom],
                                                                        Color32::LIGHT_GREEN,
                                                                        egui::Stroke::NONE,
                                                                    ));
                                                                } else if display_input < visible_min_x {
                                                                    let arrow_tip =
                                                                        egui::pos2(curve_rect.left() + 2.0, indicator_pos.y);
                                                                    let arrow_back_top =
                                                                        egui::pos2(curve_rect.left() + 10.0, indicator_pos.y - 5.0);
                                                                    let arrow_back_bottom =
                                                                        egui::pos2(curve_rect.left() + 10.0, indicator_pos.y + 5.0);

                                                                    ui.painter().add(egui::Shape::convex_polygon(
                                                                        vec![arrow_tip, arrow_back_top, arrow_back_bottom],
                                                                        Color32::LIGHT_GREEN,
                                                                        egui::Stroke::NONE,
                                                                    ));
                                                                }
                                                            }
                                                        } else {
                                                            preview_output_screen_y = editor_ui.sample_point.map(|point| point.y);
                                                            if let Some(lut) = lut_cache.try_lock() {
                                                                let actual_curve_y = curve_lookup(
                                                                    &lut,
                                                                    current_t,
                                                                    linear_ext_enabled,
                                                                    current_interpolation_mode_ptr
                                                                        .load(Ordering::Relaxed),
                                                                    current_symmetry_mode,
                                                                    current_display_mode,
                                                                    current_display_scope,
                                                                );
                                                                preview_output_value = Some(actual_curve_y);
                                                            }
                                                        }
                                                    });
                                                });
                                            });
                                    }

                                    // oscilloscope
                                    egui::Frame::new()
                                        .corner_radius(CornerRadiusF32::same(8.0))
                                        .shadow(egui::Shadow {
                                            offset: [0, 4],
                                            blur: 24,
                                            spread: 0,
                                            color: egui::Color32::from_rgba_unmultiplied(0, 0, 0, 128),
                                        })
                                        .stroke(egui::Stroke::new(
                                            1.0,
                                            egui::Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.5),
                                        ))
                                        .fill(Color32::from_hex("#1c1917").unwrap())
                                        .show(ui, |ui| {
                                            rolling_oscilloscope(
                                                ui,
                                                waveform_buffer.clone(),
                                                preview_output_value.unwrap_or(0.0),
                                                top_y,
                                                bottom_y,
                                                colored_waveform,
                                                current_timebase,
                                                current_symmetry_mode,
                                            );
                                        });
                                });

                                ui.add_space(8.0);

                                ui.horizontal(|ui| {
                                    let generator_should_show = if plot_has_focus {
                                        true
                                    } else {
                                        previous_generator_active && selected_knot_count > 0
                                    };
                                    segment_generator_active_ptr
                                        .store(generator_should_show, Ordering::Relaxed);

                                    egui::Frame::new()
                                        .fill(Color32::from_hex("#373230").unwrap())
                                        .corner_radius(8.0)
                                        .stroke(egui::Stroke::new(1.0, Color32::from_hex("#FFEAD0").unwrap()))
                                        .show(ui, |ui| {
                                            ui.set_min_width(350.0);
                                            ui.set_min_height(80.0);

                                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                                ui.add_space(16.0);

                                                ui.vertical(|ui| {
                                                    ui.add_space(12.0);

                                                    ui.allocate_ui_with_layout(
                                                        egui::vec2(308.0, 50.0),
                                                        egui::Layout::top_down(egui::Align::Min),
                                                        |ui| {
                                                            ui.set_width(ui.available_width());
                                                            ui.style_mut().spacing.item_spacing.y = 2.0;
                                                            if generator_should_show {
                                                                // ui.label(
                                                                //     RichText::new("Segment Generator")
                                                                //         .size(15.0)
                                                                //         .color(Color32::from_hex("#FFEAD0").unwrap()),
                                                                // );
                                                                // ui.add_space(4.0);

                                                                if selected_knot_count == 2 {
                                                                    ui.horizontal(|ui| {
                                                                        ui.scope(|ui| {
                                                                            let visuals = ui.visuals_mut();
                                                                            sout_ui::make_combobox_visuals(
                                                                                visuals,
                                                                                Color32::from_hex("#34302e").unwrap(),
                                                                            );
                                                                            ui.style_mut().spacing.interact_size.y = 24.0;

                                                                            egui::ComboBox::from_id_salt(
                                                                                "segment_generator_selector",
                                                                            )
                                                                            .width(180.0)
                                                                            .selected_text(selected_generator.label())
                                                                            .show_ui(ui, |ui| {
                                                                                ui.selectable_value(
                                                                                    &mut selected_generator,
                                                                                    SegmentGeneratorKind::Sine,
                                                                                    SegmentGeneratorKind::Sine.label(),
                                                                                );
                                                                                ui.selectable_value(
                                                                                    &mut selected_generator,
                                                                                    SegmentGeneratorKind::Triangle,
                                                                                    SegmentGeneratorKind::Triangle.label(),
                                                                                );
                                                                                ui.selectable_value(
                                                                                    &mut selected_generator,
                                                                                    SegmentGeneratorKind::Square,
                                                                                    SegmentGeneratorKind::Square.label(),
                                                                                );
                                                                                ui.selectable_value(
                                                                                    &mut selected_generator,
                                                                                    SegmentGeneratorKind::Stairs,
                                                                                    SegmentGeneratorKind::Stairs.label(),
                                                                                );
                                                                            });
                                                                        });

                                                                        segment_generator_kind_ptr.store(
                                                                            match selected_generator {
                                                                                SegmentGeneratorKind::Sine => 0,
                                                                                SegmentGeneratorKind::Triangle => 1,
                                                                                SegmentGeneratorKind::Square => 2,
                                                                                SegmentGeneratorKind::Stairs => 3,
                                                                            },
                                                                            Ordering::Relaxed,
                                                                        );

                                                                        if ui
                                                                            .add_sized(
                                                                                egui::vec2(52.0, 24.0),
                                                                                egui::Button::new(
                                                                                    RichText::new("Gen")
                                                                                        .color(Color32::from_hex("#FFEAD0").unwrap()),
                                                                                ),
                                                                            )
                                                                            .clicked()
                                                                        {
                                                                            if let Some(mut curve) = lookup_curve.try_lock() {
                                                                                if generate_segment_shape(
                                                                                    &mut curve,
                                                                                    &selected_knot_ids_for_generator,
                                                                                    selected_generator,
                                                                                    generator_cycles,
                                                                                    generator_steps,
                                                                                    current_symmetry_mode,
                                                                                ) {
                                                                                    curve_dirty.store(true, Ordering::Relaxed);
                                                                                    plot_dirty_ptr.store(true, Ordering::Relaxed);
                                                                                    if let Some(mut editor_ui) = editor.try_lock() {
                                                                                        editor_ui.fit_to_curve(&curve);
                                                                                    }
                                                                                }
                                                                            }
                                                                        }
                                                                    });

                                                                    ui.add_space(6.0);
                                                                    ui.horizontal(|ui| {
                                                                        if selected_generator
                                                                            == SegmentGeneratorKind::Stairs
                                                                        {
                                                                            ui.label(
                                                                                RichText::new("Steps")
                                                                                    .size(12.5)
                                                                                    .color(
                                                                                        Color32::from_hex("#FFEAD0")
                                                                                            .unwrap()
                                                                                            .gamma_multiply(0.9),
                                                                                    ),
                                                                            );
                                                                            if ui
                                                                                .add(
                                                                                    egui::Slider::new(
                                                                                        &mut generator_steps,
                                                                                        2..=64,
                                                                                    )
                                                                                    .show_value(true),
                                                                                )
                                                                                .changed()
                                                                            {
                                                                                segment_generator_steps_ptr.store(
                                                                                    generator_steps,
                                                                                    Ordering::Relaxed,
                                                                                );
                                                                            }
                                                                        } else {
                                                                            ui.label(
                                                                                RichText::new("Cycles")
                                                                                    .size(12.5)
                                                                                    .color(
                                                                                        Color32::from_hex("#FFEAD0")
                                                                                            .unwrap()
                                                                                            .gamma_multiply(0.9),
                                                                                    ),
                                                                            );
                                                                            if ui
                                                                                .add(
                                                                                    egui::Slider::new(
                                                                                        &mut generator_cycles,
                                                                                        1..=32,
                                                                                    )
                                                                                    .show_value(true),
                                                                                )
                                                                                .changed()
                                                                            {
                                                                                segment_generator_cycles_ptr.store(
                                                                                    generator_cycles,
                                                                                    Ordering::Relaxed,
                                                                                );
                                                                            }
                                                                        }
                                                                    });
                                                                } else {
                                                                    // ui.label(
                                                                    //     RichText::new("Select exactly two knots in the plot.")
                                                                    //         .size(13.0)
                                                                    //         .color(
                                                                    //             Color32::from_hex("#FFEAD0")
                                                                    //                 .unwrap()
                                                                    //                 .gamma_multiply(0.85),
                                                                    //         ),
                                                                    // );
                                                                    // ui.add_space(2.0);
                                                                    ui.label(
                                                                        RichText::new(
                                                                            "Click the plot, then select two knots to generate sine and more...",
                                                                        )
                                                                        .size(12.5)
                                                                        .color(
                                                                            Color32::from_hex("#FFEAD0")
                                                                                .unwrap()
                                                                                .gamma_multiply(0.75),
                                                                        ),
                                                                    );
                                                                }
                                                            } else {
                                                                let help_title_size = if current_help_title == default_help_title {
                                                                    20.0
                                                                } else {
                                                                    14.0
                                                                };

                                                                ui.label(
                                                                    RichText::new(current_help_title.as_str())
                                                                        .size(help_title_size)
                                                                        .color(Color32::from_hex("#FFEAD0").unwrap()),
                                                                );
                                                                ui.add_space(1.0);
                                                                ui.add(
                                                                    egui::Label::new(
                                                                        RichText::new(current_help_text.as_str())
                                                                            .size(13.0)
                                                                            .color(
                                                                                Color32::from_hex("#FFEAD0")
                                                                                    .unwrap()
                                                                                    .gamma_multiply(0.85),
                                                                            ),
                                                                    )
                                                                    .wrap(),
                                                                );
                                                            }
                                                        },
                                                    );
                                                });
                                            });
                                        });

                                    // 它曾经是一个红色的边框……可惜边框已不再是少年，长得越发透明了
                                    egui::Frame::new().stroke(egui::Stroke::new(1.0, Color32::TRANSPARENT)).show(ui, |ui| {
                                        ui.set_min_width(ui.available_width());
                                        ui.set_min_height(80.0);

                                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                            ui.add_space(12.0);

                                            ui.vertical(|ui| {
                                                ui.label(RichText::new("󰭄 Resolution").size(12.0));

                                                ui.scope(|ui| {
                                                    let visuals = ui.visuals_mut();
                                                    sout_ui::make_combobox_visuals(visuals, Color32::from_hex("#34302e").unwrap());
                                                    ui.style_mut().spacing.interact_size.y = 24.0;

                                                    ui.allocate_ui_with_layout(
                                                        egui::vec2(100.0, 24.0),
                                                        egui::Layout::top_down_justified(egui::Align::Min),
                                                        |ui| {
                                                            ui.set_width(ui.available_width());
                                                            ui.set_height(ui.available_height());

                                                            let current_val = current_resolution_ptr.load(Ordering::Relaxed);
                                                            let mut selected_val = current_val;

                                                            let response = egui::ComboBox::from_id_salt("resolution_selector")
                                                                .width(100.0)
                                                                .selected_text(format!("{}", selected_val))
                                                                .show_ui(ui, |ui| {
                                                                    ui.selectable_value(&mut selected_val, 1024, "1024");

                                                                    ui.selectable_value(&mut selected_val, 2048, "2048");

                                                                    ui.selectable_value(&mut selected_val, 4096, "4096");

                                                                    ui.separator();
                                                                    ui.horizontal(|ui| {
                                                                        ui.label("Custom:");
                                                                        ui.add(
                                                                            egui::DragValue::new(&mut selected_val)
                                                                                .clamp_existing_to_range(true)
                                                                                .range(256..=4096 * 2),
                                                                        );
                                                                    });
                                                                })
                                                                .response;

                                                            if response.hovered() {
                                                                hovered_help_title = Some("Resolution");
                                                                hovered_help_text = Some("Set LUT size for the curve. Higher values reduce lookup noises.");
                                                            }

                                                            if selected_val != current_val {
                                                                current_resolution_ptr.store(selected_val, Ordering::Relaxed);

                                                                println!(
                                                                    "Resolution changed to: {}",
                                                                    current_resolution_ptr.load(Ordering::Relaxed)
                                                                );

                                                                curve_dirty.store(true, Ordering::Relaxed);
                                                            }
                                                        },
                                                    );
                                                });

                                                ui.add_space(4.0);

                                                ui.scope(|ui| {
                                                    let visuals = ui.visuals_mut();
                                                    sout_ui::make_combobox_visuals(visuals, Color32::from_hex("#34302e").unwrap());

                                                    ui.style_mut().spacing.button_padding = egui::vec2(8.0, 2.0);
                                                    ui.style_mut().spacing.interact_size.y = 24.0;

                                                    ui.allocate_ui_with_layout(
                                                        egui::vec2(100.0, 24.0),
                                                        egui::Layout::top_down_justified(egui::Align::Min),
                                                        |ui| {
                                                            ui.set_width(ui.available_width());
                                                            ui.set_height(ui.available_height());

                                                            let visuals = ui.visuals_mut();
                                                            if colored_waveform {
                                                                sout_ui::make_btn_visuals(
                                                                    visuals,
                                                                    Color32::from_hex("#DB9160").unwrap(),
                                                                    Color32::from_hex("#FFCBA8").unwrap().gamma_multiply(0.8),
                                                                    Color32::from_hex("#2B1100").unwrap(),
                                                                );
                                                            }

                                                            let text_color = if colored_waveform {
                                                                Color32::from_hex("#2B1100").unwrap()
                                                            } else {
                                                                Color32::from_hex("#FFEAD0").unwrap()
                                                            };

                                                            let button = egui::Button::new(
                                                                RichText::new("󰉦 Colorful").color(text_color),
                                                            )
                                                            .min_size(ui.available_size());

                                                            let response = ui
                                                                .with_layout(
                                                                    egui::Layout::centered_and_justified(
                                                                        egui::Direction::LeftToRight,
                                                                    ),
                                                                    |ui| ui.add_sized(ui.available_size(), button),
                                                                )
                                                                .inner;

                                                            if response.hovered() {
                                                                hovered_help_title = Some("Colorful");
                                                                hovered_help_text = Some("Color the output history by level to reveal clipping and dynamics.");
                                                            }

                                                            if response.clicked() {
                                                                println!("󰉦 Colorful clicked");
                                                                colored_waveform_ptr.store(
                                                                    !colored_waveform_ptr.load(Ordering::Relaxed),
                                                                    Ordering::Relaxed,
                                                                );
                                                            }
                                                        },
                                                    );
                                                });
                                            });

                                            ui.add_space(4.0);

                                            ui.vertical(|ui| {
                                                ui.horizontal(|ui| {
                                                    ui.vertical(|ui| {
                                                        ui.label(RichText::new(" Symmetry").size(12.0));

                                                        ui.scope(|ui| {
                                                            let visuals = ui.visuals_mut();
                                                            sout_ui::make_combobox_visuals(visuals, Color32::from_hex("#34302e").unwrap());
                                                            ui.style_mut().spacing.interact_size.y = 24.0;

                                                            ui.allocate_ui_with_layout(
                                                                egui::vec2(100.0, 24.0),
                                                                egui::Layout::top_down_justified(egui::Align::Min),
                                                                |ui| {
                                                                    ui.set_width(ui.available_width());
                                                                    ui.set_height(ui.available_height());

                                                                    let current_val = symmetry_mode_ptr.load(Ordering::Relaxed);
                                                                    let mut selected_val = current_val;

                                                                    let response = egui::ComboBox::from_id_salt("symmetry_selector")
                                                                        .width(100.0)
                                                                        .selected_text(symmetry_mode_label(selected_val))
                                                                        .show_ui(ui, |ui| {
                                                                            ui.selectable_value(
                                                                                &mut selected_val,
                                                                                SYMMETRY_MODE_SYMMETRIC,
                                                                                "Symmetric",
                                                                            );
                                                                            ui.selectable_value(
                                                                                &mut selected_val,
                                                                                SYMMETRY_MODE_ASYMMETRIC,
                                                                                "Asymmetric",
                                                                            );
                                                                        })
                                                                        .response;

                                                                    if response.hovered() {
                                                                        hovered_help_title = Some("Symmetry");
                                                                        hovered_help_text = Some("Switch between mirrored shaping and independent negative shaping.");
                                                                    }

                                                                    if selected_val != current_val {
                                                                        if let Some(mut curve) = lookup_curve.try_lock() {
                                                                            *curve =
                                                                                transform_curve_for_symmetry_mode(&curve, selected_val);
                                                                            if let Some(mut editor_ui) = editor.try_lock() {
                                                                                editor_ui.fit_to_curve(&curve);
                                                                            }
                                                                            curve_dirty.store(true, Ordering::Relaxed);
                                                                        }
                                                                        symmetry_mode_ptr.store(selected_val, Ordering::Relaxed);
                                                                        plot_dirty_ptr.store(true, Ordering::Relaxed);
                                                                    }
                                                                },
                                                            );
                                                        });

                                                        ui.add_space(4.0);

                                                        ui.scope(|ui| {
                                                            let visuals = ui.visuals_mut();
                                                            sout_ui::make_combobox_visuals(visuals, Color32::from_hex("#34302e").unwrap());

                                                            ui.style_mut().spacing.button_padding = egui::vec2(8.0, 2.0);
                                                            ui.style_mut().spacing.interact_size.y = 24.0;

                                                            ui.allocate_ui_with_layout(
                                                                egui::vec2(100.0, 24.0),
                                                                egui::Layout::top_down_justified(egui::Align::Min),
                                                                |ui| {
                                                                    ui.set_width(ui.available_width());
                                                                    ui.set_height(ui.available_height());

                                                                    let visuals = ui.visuals_mut();
                                                                    if linear_ext_enabled {
                                                                        sout_ui::make_btn_visuals(
                                                                            visuals,
                                                                            Color32::from_hex("#DB9160").unwrap(),
                                                                            Color32::from_hex("#FFCBA8")
                                                                                .unwrap()
                                                                                .gamma_multiply(0.8),
                                                                            Color32::from_hex("#2B1100").unwrap(),
                                                                        );
                                                                    }

                                                                    let text_color = if linear_ext_enabled {
                                                                        Color32::from_hex("#2B1100").unwrap()
                                                                    } else {
                                                                        Color32::from_hex("#FFEAD0").unwrap()
                                                                    };

                                                                    let button = egui::Button::new(
                                                                        RichText::new("Linear Ext.").color(text_color),
                                                                    )
                                                                    .wrap_mode(egui::TextWrapMode::Extend)
                                                                    .min_size(ui.available_size());

                                                                    let response = ui
                                                                        .with_layout(
                                                                            egui::Layout::centered_and_justified(
                                                                                egui::Direction::LeftToRight,
                                                                            ),
                                                                            |ui| ui.add_sized(ui.available_size(), button),
                                                                        )
                                                                        .inner;

                                                                    if response.hovered() {
                                                                        hovered_help_title = Some("Linear Ext.");
                                                                        hovered_help_text = Some("Extend the curve linearly beyond the last point instead of clamping.");
                                                                    }

                                                                    if response.clicked() {
                                                                        println!("Linear Extension clicked");
                                                                        linear_ext_enabled_ptr.store(
                                                                            !linear_ext_enabled_ptr.load(Ordering::Relaxed),
                                                                            Ordering::Relaxed,
                                                                        );
                                                                    }
                                                                },
                                                            );
                                                        });
                                                    });

                                                    ui.add_space(6.0);

                                                    ui.vertical(|ui| {
                                                        ui.label(RichText::new(" Oversample").size(12.0));

                                                        ui.scope(|ui| {
                                                            let visuals = ui.visuals_mut();
                                                            sout_ui::make_combobox_visuals(visuals, Color32::from_hex("#34302e").unwrap());
                                                            ui.style_mut().spacing.interact_size.y = 24.0;

                                                            ui.allocate_ui_with_layout(
                                                                egui::vec2(100.0, 24.0),
                                                                egui::Layout::top_down_justified(egui::Align::Min),
                                                                |ui| {
                                                                    ui.set_width(ui.available_width());
                                                                    ui.set_height(ui.available_height());

                                                                    let current_factor =
                                                                        current_oversampling_factor_ptr.load(Ordering::Relaxed);
                                                                    let mut selected_factor = current_factor;

                                                                    let response = egui::ComboBox::from_id_salt("oversampling_selector")
                                                                        .width(100.0)
                                                                        .selected_text(format!("{}x", 1usize << selected_factor))
                                                                        .show_ui(ui, |ui| {
                                                                            ui.selectable_value(&mut selected_factor, 0, "1x");
                                                                            ui.selectable_value(&mut selected_factor, 1, "2x");
                                                                            ui.selectable_value(&mut selected_factor, 2, "4x");
                                                                            ui.selectable_value(&mut selected_factor, 3, "8x");
                                                                        })
                                                                        .response;

                                                                    if response.hovered() {
                                                                        hovered_help_title = Some("Oversample");
                                                                        hovered_help_text = Some("Raise the internal sample rate to reduce aliasing.");
                                                                    }

                                                                    if selected_factor != current_factor {
                                                                        current_oversampling_factor_ptr
                                                                            .store(selected_factor, Ordering::Relaxed);
                                                                        println!("Oversampling changed to: {}x", 1usize << selected_factor);
                                                                    }
                                                                },
                                                            );
                                                        });

                                                        ui.add_space(4.0);

                                                        ui.scope(|ui| {
                                                            let visuals = ui.visuals_mut();
                                                            sout_ui::make_combobox_visuals(visuals, Color32::from_hex("#34302e").unwrap());

                                                            ui.style_mut().spacing.button_padding = egui::vec2(8.0, 2.0);
                                                            ui.style_mut().spacing.interact_size.y = 24.0;

                                                            ui.allocate_ui_with_layout(
                                                                egui::vec2(100.0, 24.0),
                                                                egui::Layout::top_down_justified(egui::Align::Min),
                                                                |ui| {
                                                                    ui.set_width(ui.available_width());
                                                                    ui.set_height(ui.available_height());

                                                                    let text_color = Color32::from_hex("#FFEAD0").unwrap();

                                                                    let button = egui::Button::new(
                                                                        RichText::new("Settings").color(text_color),
                                                                    )
                                                                    .min_size(ui.available_size());

                                                                    let response = ui
                                                                        .with_layout(
                                                                            egui::Layout::centered_and_justified(
                                                                                egui::Direction::LeftToRight,
                                                                            ),
                                                                            |ui| ui.add_sized(ui.available_size(), button),
                                                                        )
                                                                        .inner;

                                                                    if response.hovered() {
                                                                        hovered_help_title = Some("Settings");
                                                                        hovered_help_text = Some("Open holy settings. u know.");
                                                                    }

                                                                    if response.clicked() {
                                                                        open_settings_modal_ptr.store(true, Ordering::Relaxed);
                                                                    }
                                                                },
                                                            );
                                                        });
                                                    });
                                                });
                                            });
                                        });

                                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                            ui.add_space(38.0);

                                            ui.vertical(|ui| {
                                                ui.set_width(64.0);
                                                ui.add_space(4.0);
                                                ui.add(
                                                    ParamKnob::for_param(&params.pre_gain, setter)
                                                        .with_diameter(59.9)
                                                        .with_color(Color32::from_hex("#DB9160").unwrap()),
                                                );
                                            });

                                            ui.add_space(24.0);

                                            ui.vertical(|ui| {
                                                ui.set_width(64.0);
                                                ui.add_space(4.0);
                                                ui.add(
                                                    ParamKnob::for_param(&params.post_gain, setter)
                                                        .with_diameter(59.9)
                                                        .with_color(Color32::from_hex("#DB9160").unwrap()),
                                                );
                                            });

                                            ui.add_space(24.0);

                                            ui.vertical(|ui| {
                                                ui.set_width(64.0);
                                                ui.add_space(4.0);
                                                ui.add(ParamKnob::for_param(&params.mix, setter).with_diameter(59.9));
                                            });
                                        });
                                    });
                                });

                                if let Some(mut title) = help_panel_title_ptr.try_lock() {
                                    title.clear();
                                    title.push_str(hovered_help_title.unwrap_or(default_help_title));
                                }
                                if let Some(mut text) = help_panel_text_ptr.try_lock() {
                                    text.clear();
                                    text.push_str(hovered_help_text.unwrap_or(default_help_text));
                                }
                            });
                        });
                    });
            },
        )
    }
}

fn rolling_oscilloscope(
    ui: &mut egui::Ui,
    waveform_buffer: Arc<Mutex<VecDeque<f32>>>,
    current_value: f32,
    top_y: f32,
    bottom_y: f32,
    colored_waveform: bool,
    timebase: usize,
    symmetry_mode: usize,
) {
    ui.vertical(|ui| {
        // ui.label("Output");

        let desired_size = ui.available_size();
        let (rect, _response) = ui.allocate_at_least(desired_size, egui::Sense::hover());

        // bg
        // ui.painter().rect_filled(
        //     rect,
        //     0.0,
        //     egui::Color32::from_hex("#1C1917")
        //         .unwrap()
        //         .gamma_multiply(0.8),
        // );

        ui.painter().rect_filled(
            Rect::from_two_pos(
                Pos2 {
                    x: rect.left(),
                    y: top_y.min(rect.bottom()).max(rect.top()),
                },
                Pos2 {
                    x: rect.right(),
                    y: bottom_y.min(rect.bottom()).max(rect.top()),
                },
            ),
            0.0,
            egui::Color32::from_hex("#FFEAD0").unwrap().gamma_multiply_u8(2),
        );

        let visual_value = |val: f32| -> f32 {
            if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
                val
            } else {
                val.abs()
            }
        };

        let map_y = |val: f32| -> f32 {
            let t = if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
                (visual_value(val) + 1.0) * 0.5
            } else {
                visual_value(val)
            };
            egui::lerp(bottom_y..=top_y, t)
        };

        if let Some(samples) = waveform_buffer.try_lock() {
            if !samples.is_empty() {
                let bar_width = rect.width() / timebase as f32;
                let stroke_width = bar_width.max(1.0);

                for (i, &sample_value) in samples.iter().rev().enumerate() {
                    if !sample_value.is_finite() {
                        continue;
                    }
                    let x = rect.left() + (i as f32 * bar_width);
                    if x > rect.right() {
                        break;
                    }

                    let base_y = if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
                        map_y(0.0)
                    } else {
                        bottom_y
                    }
                    .min(rect.bottom())
                    .max(rect.top());
                    let sample_y = map_y(sample_value).min(rect.bottom()).max(rect.top());
                    let y_top = sample_y.min(base_y);
                    let y_bottom = sample_y.max(base_y);

                    let base_color = egui::Color32::from_hex("#FFEAD0").unwrap();
                    let min_opacity = 0.24;
                    let intensity = sample_value.abs().clamp(0.0, 1.0);
                    let opacity = min_opacity + (intensity * (1.0 - min_opacity));

                    let color = if colored_waveform {
                        if intensity > 0.95 {
                            egui::Color32::from_rgb(255, 50, 50)
                        } else {
                            egui::Color32::from_rgb((intensity * 255.0) as u8, (255.0 - intensity * 100.0) as u8, 0)
                        }
                    } else {
                        base_color.gamma_multiply(opacity.clamp(0.0, 1.0))
                    };

                    ui.painter().line_segment(
                        [egui::pos2(x, y_bottom), egui::pos2(x, y_top)],
                        egui::Stroke::new(stroke_width, color.gamma_multiply(0.8)),
                    );
                }
            }
        }

        let draw_horizontal_line = |y_pos: f32, stroke: egui::Stroke| {
            if y_pos >= rect.top() && y_pos <= rect.bottom() {
                ui.painter()
                    .line_segment([egui::pos2(rect.left(), y_pos), egui::pos2(rect.right(), y_pos)], stroke);
            }
        };

        draw_horizontal_line(map_y(current_value), egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 215, 0)));

        draw_horizontal_line(
            top_y,
            egui::Stroke::new(1.2, egui::Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.5)),
        );

        draw_horizontal_line(
            bottom_y,
            egui::Stroke::new(1.2, egui::Color32::from_hex("#FFEAD0").unwrap().gamma_multiply(0.5)),
        );
    });
}

fn waveform(ui: &mut egui::Ui, waveform_buffer: Arc<Mutex<VecDeque<f32>>>, timebase: usize) {
    ui.vertical(|ui| {
        ui.heading("Output Waveform");

        let desired_size = ui.available_size();
        let (rect, _response) = ui.allocate_at_least(desired_size, egui::Sense::hover());

        ui.painter().rect_filled(rect, 4.0, egui::Color32::from_black_alpha(100));

        if let Some(samples) = waveform_buffer.try_lock() {
            if samples.len() > 1 {
                let mut points: Vec<egui::Pos2> = Vec::with_capacity(samples.len());
                for (i, &sample) in samples.iter().enumerate() {
                    let x = rect.left() + (i as f32 / timebase as f32) * rect.width();
                    let y = rect.center().y - (sample * rect.height() * 0.45);
                    points.push(egui::pos2(x, y));
                }

                ui.painter().add(egui::Shape::line(
                    points,
                    egui::Stroke::new(1.5, egui::Color32::from_rgb(0, 255, 127)),
                ));
            }
        }
    });
}

fn interpolation_mode_label(mode: usize) -> &'static str {
    match mode {
        INTERPOLATION_MODE_LINEAR => "Linear",
        INTERPOLATION_MODE_COSINE => "Cosine",
        INTERPOLATION_MODE_HERMITE => "Hermite",
        _ => "Linear",
    }
}

fn oversampling_algorithm_label(mode: usize) -> &'static str {
    match mode {
        OVERSAMPLING_ALGORITHM_LANCZOS3 => "Lanczos3",
        OVERSAMPLING_ALGORITHM_FLAT_FIR => "Flat FIR",
        _ => "Lanczos3",
    }
}

fn display_mode_label(mode: usize) -> &'static str {
    match mode {
        DISPLAY_MODE_DBFS => "dBFS",
        _ => "Linear",
    }
}

fn display_scope_label(mode: usize) -> &'static str {
    match mode {
        DISPLAY_SCOPE_XY => "X+Y",
        _ => "Y only",
    }
}

fn symmetry_mode_label(mode: usize) -> &'static str {
    match mode {
        SYMMETRY_MODE_ASYMMETRIC => "Asymmetric",
        _ => "Symmetric",
    }
}

fn next_preset_version_name(current_path: &str) -> String {
    let stem = std::path::Path::new(current_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("Preset");

    if let Some((base, version)) = parse_trailing_version(stem) {
        format!("{base} v{}", version + 1)
    } else {
        format!("{stem} v2")
    }
}

fn parse_trailing_version(name: &str) -> Option<(String, usize)> {
    let trimmed = name.trim();
    let (base, version_str) = trimmed.rsplit_once(" v")?;
    let version = version_str.parse().ok()?;
    Some((base.to_string(), version))
}

fn preset_display_name(path_str: &str) -> String {
    std::path::Path::new(path_str)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn relative_preset_segments(path_str: &str, root: &std::path::Path) -> Vec<String> {
    std::path::Path::new(path_str)
        .strip_prefix(root)
        .ok()
        .map(|relative| {
            relative
                .iter()
                .map(|part| part.to_string_lossy().to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn show_preset_tree(
    ui: &mut egui::Ui,
    current_preset: &mut String,
    preset_paths: &[String],
    root: &std::path::Path,
    menu_height: f32,
    und3ath_logo: Option<&TextureHandle>,
) {
    ui.set_max_height(menu_height);
    show_preset_tree_level(ui, current_preset, preset_paths, root, 0, menu_height, und3ath_logo);
}

fn show_preset_tree_level(
    ui: &mut egui::Ui,
    current_preset: &mut String,
    preset_paths: &[String],
    root: &std::path::Path,
    depth: usize,
    menu_height: f32,
    und3ath_logo: Option<&TextureHandle>,
) {
    let mut grouped: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
    let mut direct_files = Vec::new();

    for preset in preset_paths {
        let segments = relative_preset_segments(preset, root);
        if segments.len() <= depth {
            continue;
        }
        if segments.len() == depth + 1 {
            direct_files.push(preset.clone());
        } else {
            grouped
                .entry(segments[depth].clone())
                .or_default()
                .push(preset.clone());
        }
    }

    if depth == 1
        && root
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq("Producers"))
        && grouped.contains_key("UnD3ath")
    {
        if let Some(logo) = und3ath_logo {
            let logo_size = egui::vec2(180.0, 64.0);
            let logo_texture = egui::load::SizedTexture::new(logo.id(), logo_size);
            let logo_response = ui.add(
                egui::Button::image(logo_texture)
                    .frame(false)
                    .min_size(logo_size),
            );
            if logo_response.clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab("https://space.bilibili.com/224632474"));
            }
            ui.separator();
        }
    }

    for preset in direct_files {
        if ui
            .selectable_value(current_preset, preset.clone(), preset_display_name(&preset))
            .clicked()
        {
            ui.close_menu();
        }
    }

    for (folder, presets) in grouped {
        let is_und3ath_folder = depth == 0 && folder == "UnD3ath";
        ui.menu_button(&folder, |ui| {
            ui.set_max_height(menu_height);
            if is_und3ath_folder {
                if let Some(logo) = und3ath_logo {
                    let source_size = egui::load::SizedTexture::from_handle(logo).size;
                    let target_height = 44.0_f32;
                    let aspect_ratio = if source_size.y > 0.0 {
                        source_size.x / source_size.y
                    } else {
                        1.0
                    };
                    let logo_size = egui::vec2(target_height * aspect_ratio, target_height);
                    let logo_texture = egui::load::SizedTexture::new(logo.id(), logo_size);
                    let card_width = ui.available_width().max(logo_size.x + 96.0);
                    let card_height = logo_size.y.max(36.0);
                    let card_response = ui
                        .allocate_ui(egui::vec2(card_width, card_height), |ui| {
                            let rect = ui.max_rect();
                            let response = ui.interact(
                                rect,
                                ui.make_persistent_id("und3ath_producer_card"),
                                egui::Sense::click(),
                            );

                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::Image::from_texture(logo_texture)
                                        .fit_to_exact_size(logo_size),
                                );
                                ui.add_space(6.0);
                                ui.vertical(|ui| {
                                    ui.label(RichText::new("UnD3ath").size(14.0).strong());
                                    ui.label(RichText::new("VIP Presets").size(11.0).weak());
                                });
                            });

                            response
                        })
                        .inner;

                    if card_response.clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab("https://space.bilibili.com/224632474"));
                    }
                    ui.separator();
                }
            }
            show_preset_tree_level(ui, current_preset, &presets, root, depth + 1, menu_height, und3ath_logo);
        });
    }
}

fn generate_segment_shape(
    curve: &mut LookupCurve,
    selected_ids: &[usize],
    kind: SegmentGeneratorKind,
    cycles: usize,
    steps: usize,
    symmetry_mode: usize,
) -> bool {
    if selected_ids.len() != 2 {
        return false;
    }

    let mut selected_knots: Vec<_> = curve
        .knots()
        .iter()
        .copied()
        .filter(|knot| selected_ids.contains(&knot.id))
        .collect();
    if selected_knots.len() != 2 {
        return false;
    }

    selected_knots.sort_by(|a, b| {
        a.position
            .x
            .partial_cmp(&b.position.x)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let left = selected_knots[0];
    let right = selected_knots[1];
    let dx = right.position.x - left.position.x;
    if dx.abs() <= 1.0e-6 {
        return false;
    }

    let mut all_knots = curve.knots().to_vec();
    all_knots.retain(|knot| {
        knot.id == left.id || knot.id == right.id || knot.position.x <= left.position.x || knot.position.x >= right.position.x
    });

    let point_count = match kind {
        SegmentGeneratorKind::Square => 10,
        SegmentGeneratorKind::Stairs => steps.max(2) * 2,
        _ => 16,
    };

    let min_y = if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
        -1.0
    } else {
        0.0
    };
    let cycles = cycles.max(1) as f32;

    let sample_shape = |t: f32| -> f32 {
        match kind {
            SegmentGeneratorKind::Sine => 0.5 - 0.5 * (std::f32::consts::TAU * cycles * t).cos(),
            SegmentGeneratorKind::Triangle => {
                let phase = (t * cycles).fract();
                if phase < 0.5 { phase * 2.0 } else { 2.0 - phase * 2.0 }
            }
            SegmentGeneratorKind::Square => {
                let phase = (t * cycles).fract();
                if phase < 0.5 { 0.0 } else { 1.0 }
            }
            SegmentGeneratorKind::Stairs => {
                let total_steps = steps.max(2) as f32;
                (t * total_steps).floor() / (total_steps - 1.0)
            }
        }
    };

    let mut generated = Vec::new();
    for i in 1..point_count {
        let t = i as f32 / point_count as f32;
        let x = left.position.x + dx * t;
        let shaped = sample_shape(t).clamp(0.0, 1.0);
        let y = (left.position.y + (right.position.y - left.position.y) * shaped).clamp(min_y, 1.0);
        generated.push(bevy_lookup_curve::Knot {
            position: BevyVec2::new(x, y),
            interpolation: match kind {
                SegmentGeneratorKind::Stairs => bevy_lookup_curve::KnotInterpolation::Constant,
                _ => bevy_lookup_curve::KnotInterpolation::Linear,
            },
            ..Default::default()
        });
    }

    if let Some(left_idx) = all_knots.iter().position(|knot| knot.id == left.id) {
        all_knots[left_idx].interpolation = match kind {
            SegmentGeneratorKind::Stairs => bevy_lookup_curve::KnotInterpolation::Constant,
            _ => bevy_lookup_curve::KnotInterpolation::Linear,
        };
    }

    all_knots.extend(generated);
    *curve = LookupCurve::new(all_knots);
    true
}

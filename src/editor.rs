use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::collections::VecDeque;

use bevy_lookup_curve::{editor::LookupCurveEguiEditor, LookupCurve};
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
    capture_plot_state, fs, load_image_from_memory, sample_lut, sync_lut_cache_from_state,
    oversampling::{OVERSAMPLING_ALGORITHM_FLAT_FIR, OVERSAMPLING_ALGORITHM_LANCZOS3},
    param_knob::ParamKnob,
    sout_ui::{self, SoutTheme},
    WaverPluginParams, INTERPOLATION_MODE_COSINE, INTERPOLATION_MODE_HERMITE, INTERPOLATION_MODE_LINEAR,
};

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
    pub help_panel_title: Arc<Mutex<String>>,
    pub help_panel_text: Arc<Mutex<String>>,
    pub msg_modal_title: Arc<Mutex<String>>,
    pub msg_modal_content: Arc<Mutex<String>>,
}

#[derive(Default)]
struct EditorVisualCache {
    fonts_loaded: bool,
    background: Option<TextureHandle>,
    save_icon: Option<TextureHandle>,
}

impl EditorData {
    pub fn editor(
        &mut self,
        params: Arc<WaverPluginParams>,
        latest_input: Arc<AtomicF32>,
        current_resolution: Arc<AtomicUsize>,
        current_timebase: Arc<AtomicUsize>,
        linear_ext: Arc<AtomicBool>,
        current_oversampling_factor: Arc<AtomicUsize>,
        current_interpolation_mode: Arc<AtomicUsize>,
        current_oversampling_algorithm: Arc<AtomicUsize>,
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
        let current_oversampling_factor_ptr = current_oversampling_factor.clone();
        let current_interpolation_mode_ptr = current_interpolation_mode.clone();
        let current_oversampling_algorithm_ptr = current_oversampling_algorithm.clone();
        let open_save_modal_ptr = self.open_save_modal.clone();
        let open_msg_modal_ptr = self.open_msg_modal.clone();
        let open_about_modal_ptr = self.open_about_modal.clone();
        let open_settings_modal_ptr = self.open_settings_modal.clone();
        let help_panel_title_ptr = self.help_panel_title.clone();
        let help_panel_text_ptr = self.help_panel_text.clone();
        let saving_preset_name_ptr = self.saving_preset_name.clone();
        let msg_modal_title_ptr = self.msg_modal_title.clone();
        let msg_modal_content_ptr = self.msg_modal_content.clone();
        let visual_cache = Arc::new(Mutex::new(EditorVisualCache::default()));

        create_egui_editor(
            params.editor_state.clone(),
            (),
            |_, _| {},
            move |ctx, setter, _state| {
                let theme = SoutTheme::new();
                sout_ui::set_theme(ctx, theme);

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

                let (bg_texture, save_texture) = {
                    let mut cache = visual_cache.lock();
                    if !cache.fonts_loaded {
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
                        cache.fonts_loaded = true;
                    }

                    let bg_texture = cache
                        .background
                        .get_or_insert_with(|| load_image("background", include_bytes!("../assets/bg.png")))
                        .clone();
                    let save_texture = cache
                        .save_icon
                        .get_or_insert_with(|| load_image("save", include_bytes!("../assets/save.png")))
                        .clone();

                    (bg_texture, save_texture)
                };

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

                                                        let icon_pos = response.rect.center() + egui::vec2(-1.8, -0.5);
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
                                                                                if let Err(err) = curve.save_to_file(&path) {
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
                                                                                    *params.saved_plot_state.lock() = capture_plot_state(
                                                                                        &curve,
                                                                                        current_resolution_ptr.load(Ordering::Relaxed),
                                                                                        current_timebase_ptr.load(Ordering::Relaxed),
                                                                                        linear_ext_enabled_ptr.load(Ordering::Relaxed),
                                                                                        current_oversampling_factor_ptr.load(Ordering::Relaxed),
                                                                                        colored_waveform_ptr.load(Ordering::Relaxed),
                                                                                        current_interpolation_mode_ptr
                                                                                            .load(Ordering::Relaxed),
                                                                                        current_oversampling_algorithm_ptr
                                                                                            .load(Ordering::Relaxed),
                                                                                    );
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
                                                    ui.set_width(320.0);

                                                    ui.heading("SA Waver");
                                                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                                                    ui.add_space(8.0);

                                                    ui.scope(|ui| {
                                                        let visuals = ui.visuals_mut();
                                                        sout_ui::make_btn_visuals(
                                                            visuals,
                                                            Color32::from_hex("#554e4a").unwrap(),
                                                            Color32::from_hex("#6a625d").unwrap(),
                                                            Color32::from_hex("#FFEAD0").unwrap(),
                                                        );
                                                        ui.style_mut().spacing.button_padding = egui::vec2(10.0, 6.0);

                                                        if ui.button("󰖟 Homepage").clicked() {
                                                            ui.ctx().open_url(egui::OpenUrl::new_tab(
                                                                "https://audio.soout.top/sa_waver",
                                                            ));
                                                        }

                                                        if ui.button(" GitHub").clicked() {
                                                            ui.ctx().open_url(egui::OpenUrl::new_tab(
                                                                "https://github.com/sout233/sa_waver",
                                                            ));
                                                        }
                                                    });

                                                    ui.add_space(8.0);

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
                                                    ui.set_width(340.0);
                                                    ui.heading("Settings");
                                                    ui.add_space(8.0);
                                                    ui.label("Interpolation");
                                                    ui.add_space(6.0);

                                                    ui.scope(|ui| {
                                                        let visuals = ui.visuals_mut();
                                                        sout_ui::make_combobox_visuals(
                                                            visuals,
                                                            Color32::from_hex("#554e4a").unwrap(),
                                                        );
                                                        ui.style_mut().spacing.interact_size.y = 24.0;

                                                        let current_mode = current_interpolation_mode_ptr
                                                            .load(Ordering::Relaxed);
                                                        let mut selected_mode = current_mode;

                                                        egui::ComboBox::from_id_salt("settings_interpolation_selector")
                                                            .width(220.0)
                                                            .selected_text(interpolation_mode_label(current_mode))
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
                                                            current_interpolation_mode_ptr
                                                                .store(selected_mode, Ordering::Relaxed);
                                                            plot_dirty_ptr.store(true, Ordering::Relaxed);
                                                        }
                                                    });

                                                    ui.add_space(12.0);
                                                    ui.label("Oversampling");
                                                    ui.add_space(6.0);

                                                    ui.scope(|ui| {
                                                        let visuals = ui.visuals_mut();
                                                        sout_ui::make_combobox_visuals(
                                                            visuals,
                                                            Color32::from_hex("#554e4a").unwrap(),
                                                        );
                                                        ui.style_mut().spacing.interact_size.y = 24.0;

                                                        let current_algo = current_oversampling_algorithm_ptr
                                                            .load(Ordering::Relaxed);
                                                        let mut selected_algo = current_algo;

                                                        egui::ComboBox::from_id_salt("settings_oversampling_algorithm_selector")
                                                            .width(220.0)
                                                            .selected_text(oversampling_algorithm_label(current_algo))
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
                                                            current_oversampling_algorithm_ptr
                                                                .store(selected_algo, Ordering::Relaxed);
                                                            plot_dirty_ptr.store(true, Ordering::Relaxed);
                                                        }
                                                    });

                                                    ui.add_space(6.0);

                                                    egui::Sides::new().show(
                                                        ui,
                                                        |_ui| {},
                                                        |ui| {
                                                            if ui.button("Close").clicked() {
                                                                open_settings_modal_ptr.store(false, Ordering::Relaxed);
                                                            }
                                                        },
                                                    );
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

                                                let get_filename = |path_str: &str| {
                                                    let path = std::path::Path::new(path_str);
                                                    match path.file_stem() {
                                                        Some(stem) => stem.to_string_lossy().to_string(),
                                                        None => "".to_string(),
                                                    }
                                                };

                                                if let Some(mut current_preset_guard) = current_preset_ptr.try_lock() {
                                                    let previous_preset = current_preset_guard.clone();
                                                    let plot_dirty = plot_dirty_ptr.load(Ordering::Relaxed);
                                                    let preset_label = if plot_dirty {
                                                        format!("{} *", get_filename(&current_preset_guard))
                                                    } else {
                                                        get_filename(&current_preset_guard)
                                                    };

                                                    let response = egui::ComboBox::from_id_salt("preset_selector")
                                                        .selected_text(preset_label)
                                                        .show_ui(ui, |ui| {
                                                            ui.selectable_value(
                                                                &mut *current_preset_guard,
                                                                "./Default.ron".to_string(),
                                                                "Default",
                                                            );

                                                            if let Some(presets) = presets_ptr.try_lock() {
                                                                for preset in presets.iter() {
                                                                    ui.selectable_value(
                                                                        &mut *current_preset_guard,
                                                                        preset.to_string(),
                                                                        get_filename(preset),
                                                                    );
                                                                }
                                                            }
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
                                                            match LookupCurve::load_from_bytes(&preset_data) {
                                                                Ok(c) => {
                                                                    *curve = c;
                                                                    curve_dirty.store(true, Ordering::Relaxed);
                                                                    *params.saved_plot_state.lock() = capture_plot_state(
                                                                        &curve,
                                                                        current_resolution_ptr.load(Ordering::Relaxed),
                                                                        current_timebase_ptr.load(Ordering::Relaxed),
                                                                        linear_ext_enabled_ptr.load(Ordering::Relaxed),
                                                                        current_oversampling_factor_ptr.load(Ordering::Relaxed),
                                                                        colored_waveform_ptr.load(Ordering::Relaxed),
                                                                        current_interpolation_mode_ptr
                                                                            .load(Ordering::Relaxed),
                                                                        current_oversampling_algorithm_ptr
                                                                            .load(Ordering::Relaxed),
                                                                    );
                                                                    plot_dirty_ptr.store(false, Ordering::Relaxed);
                                                                }
                                                                Err(e) => {
                                                                    println!("Failed to load preset: {}", e);
                                                                    *curve = LookupCurve::load_from_bytes(include_bytes!("default.ron"))
                                                                        .unwrap();
                                                                    curve_dirty.store(true, Ordering::Relaxed);
                                                                    *params.saved_plot_state.lock() = capture_plot_state(
                                                                        &curve,
                                                                        current_resolution_ptr.load(Ordering::Relaxed),
                                                                        current_timebase_ptr.load(Ordering::Relaxed),
                                                                        linear_ext_enabled_ptr.load(Ordering::Relaxed),
                                                                        current_oversampling_factor_ptr.load(Ordering::Relaxed),
                                                                        colored_waveform_ptr.load(Ordering::Relaxed),
                                                                        current_interpolation_mode_ptr
                                                                            .load(Ordering::Relaxed),
                                                                        current_oversampling_algorithm_ptr
                                                                            .load(Ordering::Relaxed),
                                                                    );
                                                                    plot_dirty_ptr.store(false, Ordering::Relaxed);
                                                                }
                                                            }
                                                        }
                                                    }

                                                    if response.clicked() {
                                                        let presets = fs::get_presets().unwrap_or_default();
                                                        *presets_ptr.lock() = presets;
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
                            let mut output_y = None;
                            let mut display_output_y = None;
                            let mut top_y = 0.0;
                            let mut bottom_y = 0.0;

                            let colored_waveform = colored_waveform_ptr.load(Ordering::Relaxed);
                            let current_timebase = current_timebase_ptr.load(Ordering::Relaxed);
                            let linear_ext_enabled = linear_ext_enabled_ptr.load(Ordering::Relaxed);

                            egui::Frame::new().inner_margin(12.0).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    if let (Some(mut curve), Some(mut editor_ui)) = (lookup_curve.try_lock(), editor.try_lock()) {
                                        let side_length = ui.available_height().max(350.0);
                                        let square_size = egui::Vec2::splat(side_length);
                                        let should_draw_manual_indicator = linear_ext_enabled && current_t > 1.0;
                                        let sample_for_editor = if should_draw_manual_indicator {
                                            None
                                        } else {
                                            Some(current_t)
                                        };

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

                                                        let curve_to_screen = |curve_x: f32, curve_y: f32| -> Pos2 {
                                                            let canvas_x = (curve_x - editor_ui.offset.x) * editor_ui.editor_size.x
                                                                / editor_ui.scale.x;
                                                            let canvas_y = editor_ui.editor_size.y
                                                                - ((curve_y - editor_ui.offset.y) * editor_ui.editor_size.y
                                                                    / editor_ui.scale.y);

                                                            egui::pos2(curve_rect.left() + canvas_x, curve_rect.top() + canvas_y)
                                                        };

                                                        top_y = curve_to_screen(1.0, 1.0).y;
                                                        bottom_y = curve_to_screen(0.0, 0.0).y;

                                                        if should_draw_manual_indicator {
                                                            if let Some(lut) = lut_cache.try_lock() {
                                                                let curve_y =
                                                                    curve_lookup_with_linear_ext(
                                                                        &lut,
                                                                        current_t,
                                                                        linear_ext_enabled,
                                                                        current_interpolation_mode_ptr
                                                                            .load(Ordering::Relaxed),
                                                                    );
                                                                if !curve_y.is_finite() {
                                                                    display_output_y = None;
                                                                    output_y = None;
                                                                    return;
                                                                }
                                                                let visible_max_x = editor_ui.offset.x + editor_ui.scale.x;
                                                                let visible_min_x = editor_ui.offset.x;
                                                                let indicator_x = current_t.clamp(visible_min_x, visible_max_x);
                                                                let indicator_pos = curve_to_screen(indicator_x, curve_y);
                                                                if !indicator_pos.is_finite() {
                                                                    display_output_y = None;
                                                                    output_y = None;
                                                                    return;
                                                                }

                                                                display_output_y = Some(indicator_pos.y);

                                                                ui.painter().line_segment(
                                                                    [
                                                                        indicator_pos,
                                                                        egui::pos2(curve_rect.right(), indicator_pos.y),
                                                                    ],
                                                                    egui::Stroke::new(1.0, Color32::LIGHT_GREEN),
                                                                );

                                                                ui.painter()
                                                                    .circle_filled(indicator_pos, 3.0, Color32::LIGHT_GREEN);

                                                                if current_t > visible_max_x {
                                                                    let arrow_tip = egui::pos2(curve_rect.right() - 2.0, indicator_pos.y);
                                                                    let arrow_back_top =
                                                                        egui::pos2(curve_rect.right() - 10.0, indicator_pos.y - 5.0);
                                                                    let arrow_back_bottom =
                                                                        egui::pos2(curve_rect.right() - 10.0, indicator_pos.y + 5.0);

                                                                    ui.painter().add(egui::Shape::convex_polygon(
                                                                        vec![arrow_tip, arrow_back_top, arrow_back_bottom],
                                                                        Color32::LIGHT_GREEN,
                                                                        egui::Stroke::NONE,
                                                                    ));
                                                                }
                                                            }
                                                        } else {
                                                            output_y = editor_ui.sample_point;
                                                            display_output_y = editor_ui.sample_point.map(|point| point.y);
                                                        }
                                                    });
                                                });
                                            });
                                    }

                                    // waveform(ui, waveform_buffer.clone());
                                    let current_t = display_output_y.unwrap_or_else(|| output_y.map(|point| point.y).unwrap_or(0.0));

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
                                                current_t,
                                                top_y,
                                                bottom_y,
                                                colored_waveform,
                                                current_timebase,
                                            );
                                        });
                                });

                                ui.add_space(8.0);

                                ui.horizontal(|ui| {
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
                                                                hovered_help_text = Some("Set LUT size for the curve. Higher values reduce lookup error.");
                                                            }

                                                            if selected_val != current_val {
                                                                current_resolution_ptr.store(selected_val, Ordering::Relaxed);

                                                                println!(
                                                                    "Resolution changed to: {}",
                                                                    current_resolution_ptr.load(Ordering::Relaxed)
                                                                );

                                                                curve_dirty.store(true, Ordering::Relaxed);
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
                                                                plot_dirty_ptr.store(true, Ordering::Relaxed);
                                                            }
                                                        },
                                                    );
                                                });
                                            });

                                            ui.add_space(4.0);

                                            ui.vertical(|ui| {
                                                ui.horizontal(|ui| {
                                                    ui.vertical(|ui| {
                                                        ui.label(RichText::new("󰄉 Timebase").size(12.0));

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

                                                                    let current_val = current_timebase_ptr.load(Ordering::Relaxed);
                                                                    let mut selected_val = current_val;

                                                                    let response = egui::ComboBox::from_id_salt("timebase_selector")
                                                                        .width(100.0)
                                                                        .selected_text(format!("{}", selected_val))
                                                                        .show_ui(ui, |ui| {
                                                                            ui.selectable_value(&mut selected_val, 128, "128");
                                                                            ui.selectable_value(&mut selected_val, 256, "256");
                                                                            ui.selectable_value(&mut selected_val, 512, "512");
                                                                            ui.selectable_value(&mut selected_val, 1024, "1024");

                                                                            ui.separator();
                                                                            ui.horizontal(|ui| {
                                                                                ui.label("Custom:");
                                                                                ui.add(
                                                                                    egui::DragValue::new(&mut selected_val)
                                                                                        .clamp_existing_to_range(true)
                                                                                        .range(64..=4096),
                                                                                );
                                                                            });
                                                                        })
                                                                        .response;

                                                                    if response.hovered() {
                                                                        hovered_help_title = Some("Timebase");
                                                                        hovered_help_text = Some("Choose how much output history is shown in the rolling view.");
                                                                    }

                                                                    if selected_val != current_val {
                                                                        current_timebase_ptr.store(selected_val, Ordering::Relaxed);
                                                                        plot_dirty_ptr.store(true, Ordering::Relaxed);

                                                                        println!(
                                                                            "Timebase changed to: {}",
                                                                            current_timebase_ptr.load(Ordering::Relaxed)
                                                                        );
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
                                                                        plot_dirty_ptr.store(true, Ordering::Relaxed);
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
                                                                        plot_dirty_ptr.store(true, Ordering::Relaxed);
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
                                                                        hovered_help_text = Some("Open interpolation and oversampling settings.");
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
    current_t_y: f32,
    top_y: f32,
    bottom_y: f32,
    colored_waveform: bool,
    timebase: usize,
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

        let map_y = |val: f32| -> f32 {
            let t = val.clamp(0.0, 114.0);
            egui::lerp(bottom_y..=top_y, t)
        };

        if let Some(samples) = waveform_buffer.try_lock() {
            if !samples.is_empty() {
                let bar_width = rect.width() / timebase as f32;
                let stroke_width = bar_width.max(1.0);

                for (i, &sample_peak) in samples.iter().rev().enumerate() {
                    if !sample_peak.is_finite() {
                        continue;
                    }
                    let x = rect.left() + (i as f32 * bar_width);
                    if x > rect.right() {
                        break;
                    }

                    let y_top = map_y(sample_peak).min(rect.bottom()).max(rect.top());
                    let y_bottom = bottom_y.min(rect.bottom()).max(rect.top());

                    let base_color = egui::Color32::from_hex("#FFEAD0").unwrap();
                    let min_opacity = 0.24;
                    let opacity = min_opacity + (sample_peak * (1.0 - min_opacity));

                    let color = if colored_waveform {
                        if sample_peak > 0.95 {
                            egui::Color32::from_rgb(255, 50, 50)
                        } else {
                            egui::Color32::from_rgb((sample_peak * 255.0) as u8, (255.0 - sample_peak * 100.0) as u8, 0)
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

        draw_horizontal_line(current_t_y, egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 215, 0)));

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

fn curve_lookup_with_linear_ext(
    lut: &[f32],
    input: f32,
    linear_ext_enabled: bool,
    interpolation_mode: usize,
) -> f32 {
    if lut.is_empty() {
        return 0.0;
    }

    let lut_size = lut.len();
    let abs_input = if linear_ext_enabled {
        input.abs()
    } else {
        input.abs().min(1.0)
    };

    let t = abs_input * (lut_size - 1) as f32;
    let index = t.floor() as usize;
    let fraction = t - index as f32;

    if index >= lut_size - 1 {
        if linear_ext_enabled && lut_size >= 2 {
            let last_val = lut[lut_size - 1];
            let prev_val = lut[lut_size - 2];
            let slope = last_val - prev_val;
            let excess = t - (lut_size - 1) as f32;

            let value = last_val + slope * excess;
            if value.is_finite() { value } else { last_val }
        } else {
            lut[lut_size - 1]
        }
    } else {
        sample_lut(lut, index, fraction, interpolation_mode)
    }
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

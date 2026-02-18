use bevy_lookup_curve::{editor::LookupCurveEguiEditor, LookupCurve};
use fundsp::prelude::*;
use nih_plug::prelude::*;
use nih_plug_egui::egui::epaint::CornerRadiusF32;
use nih_plug_egui::egui::{pos2, Color32, ColorImage, Id, Modal, Pos2, Rect, RichText, TextureHandle, Vec2};
use nih_plug_egui::{create_egui_editor, egui, EguiState};
use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::param_knob::ParamKnob;
use crate::sout_ui::SoutTheme;

mod fs;
mod param_knob;
mod sout_ui;

// const WAVEFORM_SIZE: usize = 512;

pub struct WaverPlugin {
    params: Arc<WaverPluginParams>,
    lookup_curve: Arc<Mutex<LookupCurve>>,
    editor: Arc<Mutex<LookupCurveEguiEditor>>,
    lut_cache: Arc<Mutex<Vec<f32>>>,
    // lut_size: usize,
    waveform_buffer: Arc<Mutex<Vec<f32>>>,
    latest_input: Arc<AtomicF32>,
    input_peak_follower: f32,

    // 降采样计数器
    sample_counter: usize,
    // 当前块的峰值累加器
    current_chunk_peak: f32,
    colored_waveform: Arc<AtomicBool>,

    presets: Arc<Mutex<Vec<String>>>,
    current_preset: Arc<Mutex<String>>,
    saving_preset_name: Arc<Mutex<String>>,

    current_resolution: Arc<AtomicUsize>,
    current_timebase: Arc<AtomicUsize>,

    linear_ext: Arc<AtomicBool>,

    open_save_modal: Arc<AtomicBool>,

    open_msg_modal: Arc<AtomicBool>,
    msg_modal_title: Arc<Mutex<String>>,
    msg_modal_content: Arc<Mutex<String>>,
}

#[derive(Params)]
struct WaverPluginParams {
    // The editor state
    #[persist = "editor_state"]
    editor_state: Arc<EguiState>,

    /// The parameter's ID is used to identify the parameter in the wrappred plugin API. As long as
    /// these IDs remain constant, you can rename and reorder these fields as you wish. The
    /// parameters are exposed to the host in the same order they were defined. In this case, this
    /// gain parameter is stored as linear gain while the values are displayed in decibels.
    #[id = "pre_gain"]
    pub pre_gain: FloatParam,

    #[id = "post_gain"]
    pub post_gain: FloatParam,

    #[id = "mix"]
    pub mix: FloatParam,
}

impl Default for WaverPlugin {
    fn default() -> Self {
        let fun_gain = Shared::new(1.0);
        let _fun_filter_stereo = (pass() * var(&fun_gain)) | (pass() * var(&fun_gain));
        let path = "./assets/test.ron";

        let res = LookupCurve::load_from_file(path);
        let lookup_curve = match res {
            Ok(curve) => curve,
            Err(err) => {
                eprintln!("Failed to load curve: {}", err);
                LookupCurve::load_from_bytes(include_bytes!("default.ron")).unwrap()
            }
        };

        let mut initial_lut = Vec::with_capacity(1024);
        for i in 0..1024 {
            let t = i as f32 / (1024 - 1) as f32;
            initial_lut.push(lookup_curve.lookup(t));
        }

        Self {
            params: Arc::new(WaverPluginParams::default()),
            // fun_gain,
            // fun_filter_stereo: Box::new(fun_filter_stereo),
            lookup_curve: Arc::new(Mutex::new(lookup_curve.clone())),
            editor: Arc::new(Mutex::new(LookupCurveEguiEditor::fitted_to_curve(&lookup_curve))),
            lut_cache: Arc::new(Mutex::new(initial_lut)),
            waveform_buffer: Arc::new(Mutex::new(vec![])),
            latest_input: Arc::new(AtomicF32::new(0.5)),
            input_peak_follower: 0.0,
            sample_counter: 0,
            current_chunk_peak: 0.0,
            colored_waveform: Arc::new(AtomicBool::new(false)),
            presets: Arc::new(Mutex::new(Vec::new())),
            current_preset: Arc::new(Mutex::new(String::from("Default"))),
            current_resolution: Arc::new(AtomicUsize::new(1024)),
            current_timebase: Arc::new(AtomicUsize::new(512)),
            linear_ext: Arc::new(AtomicBool::new(false)),
            open_save_modal: Arc::new(AtomicBool::new(false)),
            open_msg_modal: Arc::new(AtomicBool::new(false)),
            saving_preset_name: Arc::new(Mutex::new(String::new())),
            msg_modal_title: Arc::new(Mutex::new(String::new())),
            msg_modal_content: Arc::new(Mutex::new(String::new())),
        }
    }
}

impl Default for WaverPluginParams {
    fn default() -> Self {
        Self {
            editor_state: EguiState::from_size(1000, 520),

            pre_gain: FloatParam::new(
                "Pre",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-30.0),
                    max: util::db_to_gain(30.0),
                    factor: FloatRange::gain_skew_factor(-30.0, 30.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            post_gain: FloatParam::new(
                "Post",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-30.0),
                    max: util::db_to_gain(30.0),
                    factor: FloatRange::gain_skew_factor(-30.0, 30.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            mix: FloatParam::new("Mix", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_smoother(SmoothingStyle::Linear(50.0))
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_percentage(1))
                .with_string_to_value(formatters::s2v_f32_percentage()),
        }
    }
}


impl Plugin for WaverPlugin {
    const NAME: &'static str = "SA Waver";
    const VENDOR: &'static str = "Sout Audio";
    const URL: &'static str = env!("CARGO_PKG_HOMEPAGE");
    const EMAIL: &'static str = "sout233@163.com";

    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    // The first audio IO layout is used as the default. The other layouts may be selected either
    // explicitly or automatically by the host or the user depending on the plugin API/backend.
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),

        aux_input_ports: &[],
        aux_output_ports: &[],

        // Individual ports and the layout as a whole can be named here. By default these names
        // are generated as needed. This layout will be called 'Stereo', while a layout with
        // only one input and output channel would be called 'Mono'.
        names: PortNames::const_default(),
    }];

    const MIDI_INPUT: MidiConfig = MidiConfig::None;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::None;

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    // If the plugin can send or receive SysEx messages, it can define a type to wrap around those
    // messages here. The type implements the `SysExMessage` trait, which allows conversion to and
    // from plain byte buffers.
    type SysExMessage = ();
    // More advanced plugins can use this to run expensive background tasks. See the field's
    // documentation for more information. `()` means that the plugin does not have any background
    // tasks.
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        _buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        // Resize buffers and perform other potentially expensive initialization operations here.
        // The `reset()` function is always called right after this function. You can remove this
        // function if you do not need it.
        true
    }

    fn reset(&mut self) {
        // Reset buffers and envelopes here. This can be called from the audio thread and may not
        // allocate. You can remove this function if you do not need it.
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, _context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let mut block_max_abs = 0.0f32;
        const DOWNSAMPLE_RATE: usize = 256;
        let linear_ext_enabled = self.linear_ext.load(Ordering::Relaxed);
        let timebase = self.current_timebase.load(Ordering::Relaxed);

        for channel_samples in buffer.iter_samples() {
            let pre_gain = self.params.pre_gain.smoothed.next();
            let post_gain = self.params.post_gain.smoothed.next();
            let mix = self.params.mix.smoothed.next();

            // let lut_size = self.lut_size;
            // let lut_size = self.current_resolution.load(Ordering::Relaxed);

            if let Ok(lut) = self.lut_cache.try_lock() {
                let lut_size = lut.len();
                if let Ok(mut waveform_buf) = self.waveform_buffer.try_lock() {
                    for sample in channel_samples {
                        let input = *sample * pre_gain;

                        let raw_abs_input = input.abs();
                        let abs_input = if linear_ext_enabled {
                            raw_abs_input
                        } else {
                            raw_abs_input.min(1.0)
                        };

                        if abs_input > block_max_abs {
                            block_max_abs = abs_input;
                        }

                        // LUT 映射
                        let sign = input.signum();
                        let t = abs_input * (lut_size - 1) as f32;
                        let index = t.floor() as usize;
                        let fraction = t - index as f32;

                        let curve_val = if index >= lut_size - 1 {
                            if linear_ext_enabled {
                                let last_val = lut[lut_size - 1];
                                let prev_val = lut[lut_size - 2];

                                let slope = last_val - prev_val;

                                let excess = t - (lut_size - 1) as f32;

                                last_val + slope * excess
                            } else {
                                lut[lut_size - 1]
                            }
                        } else {
                            let a = lut[index];
                            let b = lut[index + 1];
                            a + fraction * (b - a)
                        };

                        let wet_signal = curve_val * sign * post_gain;

                        *sample = input * (1.0 - mix) + wet_signal * mix;

                        // peak only
                        let out_abs = (curve_val * sign).abs();
                        if out_abs > self.current_chunk_peak {
                            self.current_chunk_peak = out_abs;
                        }

                        self.sample_counter += 1;

                        if self.sample_counter >= DOWNSAMPLE_RATE {
                            if waveform_buf.len() >= timebase {
                                waveform_buf.remove(0);
                            }
                            waveform_buf.push(self.current_chunk_peak);

                            self.sample_counter = 0;
                            self.current_chunk_peak = 0.0;
                        }
                    }
                }
            }
        }

        if block_max_abs > self.input_peak_follower {
            self.input_peak_follower = block_max_abs;
        } else {
            self.input_peak_follower *= 0.95;
        }

        self.latest_input.store(self.input_peak_follower, Ordering::Relaxed);

        ProcessStatus::Normal
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let params = self.params.clone();
        let lookup_curve = self.lookup_curve.clone();
        let editor = self.editor.clone();
        // let lut_size = self.lut_size;
        let lut_cache = self.lut_cache.clone();
        let waveform_buffer = self.waveform_buffer.clone();
        let latest_input_ptr = self.latest_input.clone();
        let colored_waveform_ptr = self.colored_waveform.clone();
        let presets_ptr = self.presets.clone();
        let current_preset_ptr = self.current_preset.clone();
        let current_resolution_ptr = self.current_resolution.clone();
        let current_timebase_ptr = self.current_timebase.clone();
        let linear_ext_enabled_ptr = self.linear_ext.clone();
        let open_save_modal_ptr = self.open_save_modal.clone();
        let open_msg_modal_ptr = self.open_msg_modal.clone();
        let saving_preset_name_ptr = self.saving_preset_name.clone();
        let msg_modal_title_ptr = self.msg_modal_title.clone();
        let msg_modal_content_ptr = self.msg_modal_content.clone();

        create_egui_editor(
            self.params.editor_state.clone(),
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

                let mut fonts = egui::FontDefinitions::default();
                fonts.font_data.insert(
                    "maple-mono".to_string(),
                    std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../assets/MapleMono-NF-CN-Regular.ttf"))),
                );
                fonts
                    .families
                    .get_mut(&egui::FontFamily::Proportional)
                    .unwrap()
                    .insert(0, "maple-mono".to_string());
                ctx.set_fonts(fonts);

                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(egui::Color32::BLACK).inner_margin(0.0))
                    .show(ctx, |ui| {
                        let bg_texture = load_image("background", include_bytes!("../assets/bg.png"));

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
                                            ui.add_space(2.0);

                                            // ghost_button(ui, "".into());

                                            let save_texture = load_image("save", include_bytes!("../assets/save.png"));
                                            let img_size = egui::vec2(12.0, 12.0);
                                            let img_src = egui::load::SizedTexture::new(save_texture.id(), img_size);

                                            ui.scope(|ui| {
                                                let visuals = ui.visuals_mut();
                                                sout_ui::make_ghost_button_visuals(visuals);

                                                ui.allocate_ui(egui::vec2(24.0, ui.available_height()), |ui| {
                                                    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                                                        ui.add_space(4.0);

                                                        // save button
                                                        if ui.add(egui::Button::image(img_src).min_size(egui::vec2(24.0, 24.0))).clicked() {
                                                            println!("Save clicked");
                                                            open_save_modal_ptr.store(true, Ordering::Relaxed);
                                                        }
                                                    });
                                                });
                                            });

                                            if open_save_modal_ptr.load(Ordering::Relaxed) {
                                                if let Ok(mut name_guard) = saving_preset_name_ptr.lock() {
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
                                                                        if let (Ok(mut title_lock), Ok(mut content_lock)) =
                                                                            (msg_modal_title_ptr.lock(), msg_modal_content_ptr.lock())
                                                                        {
                                                                            *title_lock = title.to_string();
                                                                            *content_lock = message;
                                                                        }
                                                                    };

                                                                    match fs::build_preset_path(&name_guard) {
                                                                        Ok(path) => {
                                                                            if let Ok(curve) = lookup_curve.lock() {
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
                                                if let (Ok(title_guard), Ok(content_guard)) =
                                                    (msg_modal_title_ptr.lock(), msg_modal_content_ptr.lock())
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

                                                if let Ok(mut current_preset_guard) = current_preset_ptr.lock() {
                                                    let previous_preset = current_preset_guard.clone();

                                                    let response = egui::ComboBox::from_id_salt("preset_selector")
                                                        .selected_text(get_filename(&current_preset_guard))
                                                        .show_ui(ui, |ui| {
                                                            ui.selectable_value(
                                                                &mut *current_preset_guard,
                                                                "./Default.ron".to_string(),
                                                                "Default",
                                                            );

                                                            if let Ok(presets) = presets_ptr.lock() {
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

                                                    if *current_preset_guard != previous_preset {
                                                        println!("Preset changed to: {}", *current_preset_guard);

                                                        let preset_data = std::fs::read(format!("{}", *current_preset_guard))
                                                            .unwrap_or_default();
                                                        if let Ok(mut curve) = lookup_curve.lock() {
                                                            match LookupCurve::load_from_bytes(&preset_data) {
                                                                Ok(c) => {
                                                                    *curve = c;
                                                                }
                                                                Err(e) => {
                                                                    println!("Failed to load preset: {}", e);
                                                                    *curve = LookupCurve::load_from_bytes(include_bytes!("default.ron")).unwrap();
                                                                },
                                                            }
                                                            if let Ok(mut lut) = lut_cache.lock() {
                                                                lut.clear();
                                                                let lut_size = current_resolution_ptr.load(Ordering::Relaxed);
                                                                for i in 0..lut_size {
                                                                    let t = i as f32 / (lut_size - 1) as f32;
                                                                    lut.push(curve.lookup(t));
                                                                }
                                                            }
                                                        }
                                                    }

                                                    if response.clicked() {
                                                        let presets = fs::get_presets().unwrap_or_default();
                                                        *presets_ptr.lock().unwrap() = presets;
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
                            let mut top_y = 0.0;
                            let mut bottom_y = 0.0;

                            let colored_waveform = colored_waveform_ptr.load(Ordering::Relaxed);
                            let current_timebase = current_timebase_ptr.load(Ordering::Relaxed);
                            let linear_ext_enabled = linear_ext_enabled_ptr.load(Ordering::Relaxed);

                            egui::Frame::new().inner_margin(12.0).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    if let (Ok(mut curve), Ok(mut editor_ui)) = (lookup_curve.lock(), editor.lock()) {
                                        let side_length = ui.available_height().max(350.0);
                                        let square_size = egui::Vec2::splat(side_length);
                                        output_y = editor_ui.sample_point;
                                        top_y = editor_ui.top_one_point.y;
                                        bottom_y = editor_ui.bottom_zero_point.y;

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
                                                        if editor_ui.ui(ui, &mut curve, Some(current_t)) {
                                                            if let Ok(mut lut) = lut_cache.lock() {
                                                                lut.clear();
                                                                let lut_size = current_resolution_ptr.load(Ordering::Relaxed);
                                                                for i in 0..lut_size {
                                                                    let t = i as f32 / (lut_size - 1) as f32;
                                                                    lut.push(curve.lookup(t));
                                                                }
                                                            }
                                                        }
                                                    });
                                                });
                                            });
                                    }

                                    // waveform(ui, waveform_buffer.clone());
                                    let current_t = if output_y.is_some() { output_y.unwrap().y } else { 0.0 };

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
                                                    ui.add_space(16.0);
                                                    ui.heading("Waver");
                                                    ui.label("by sout audio");
                                                });
                                            });
                                        });

                                    egui::Frame::new().stroke(egui::Stroke::new(1.0, Color32::RED)).show(ui, |ui| {
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

                                                            let _response = egui::ComboBox::from_id_salt("resolution_selector")
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
                                                                });

                                                            if selected_val != current_val {
                                                                current_resolution_ptr.store(selected_val, Ordering::Relaxed);

                                                                println!(
                                                                    "Resolution changed to: {}",
                                                                    current_resolution_ptr.load(Ordering::Relaxed)
                                                                );

                                                                if let Ok(mut lut) = lut_cache.lock() {
                                                                    lut.clear();
                                                                    for i in 0..selected_val {
                                                                        let t = i as f32 / (selected_val - 1) as f32;
                                                                        if let Ok(curve) = lookup_curve.lock() {
                                                                            lut.push(curve.lookup(t));
                                                                        }
                                                                    }
                                                                }
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

                                                            if ui
                                                                .add(egui::Button::new(RichText::new("󰉦 Colorful").color(text_color)))
                                                                .clicked()
                                                            {
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

                                                            let _response = egui::ComboBox::from_id_salt("timebase_selector")
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
                                                                });

                                                            if selected_val != current_val {
                                                                current_timebase_ptr.store(selected_val, Ordering::Relaxed);

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
                                                                    Color32::from_hex("#FFCBA8").unwrap().gamma_multiply(0.8),
                                                                    Color32::from_hex("#2B1100").unwrap(),
                                                                );
                                                            }

                                                            let text_color = if linear_ext_enabled {
                                                                Color32::from_hex("#2B1100").unwrap()
                                                            } else {
                                                                Color32::from_hex("#FFEAD0").unwrap()
                                                            };

                                                            if ui
                                                                .add(
                                                                    egui::Button::new(RichText::new("Linear Ext.").color(text_color))
                                                                        .wrap_mode(egui::TextWrapMode::Extend)
                                                                        .min_size(Vec2::new(0.0, 0.0)),
                                                                )
                                                                .clicked()
                                                            {
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
                                        });

                                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                            ui.add_space(114.0);

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
                            });
                        });
                    });
            },
        )
    }
}

fn load_image_from_memory(image_data: &[u8]) -> Result<ColorImage, Box<dyn Error>> {
    let format = image::ImageFormat::Png;
    let image = image::load_from_memory_with_format(image_data, format)?;
    let size = [image.width() as _, image.height() as _];
    let image_buffer = image.to_rgba8();
    let pixels = image_buffer.as_flat_samples();
    Ok(ColorImage::from_rgba_unmultiplied(size, pixels.as_slice()))
}

fn rolling_oscilloscope(
    ui: &mut egui::Ui,
    waveform_buffer: Arc<Mutex<Vec<f32>>>,
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

        if let Ok(samples) = waveform_buffer.lock() {
            if !samples.is_empty() {
                let bar_width = rect.width() / timebase as f32;
                let stroke_width = bar_width.max(1.0);

                for (i, &sample_peak) in samples.iter().rev().enumerate() {
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

fn waveform(ui: &mut egui::Ui, waveform_buffer: Arc<Mutex<Vec<f32>>>, timebase: usize) {
    ui.vertical(|ui| {
        ui.heading("Output Waveform");

        let desired_size = ui.available_size();
        let (rect, _response) = ui.allocate_at_least(desired_size, egui::Sense::hover());

        ui.painter().rect_filled(rect, 4.0, egui::Color32::from_black_alpha(100));

        if let Ok(samples) = waveform_buffer.lock() {
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

impl ClapPlugin for WaverPlugin {
    const CLAP_ID: &'static str = "top.soout.audio.waver";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A WaveShaper plugin built with love.");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;

    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Distortion];
}

impl Vst3Plugin for WaverPlugin {
    const VST3_CLASS_ID: [u8; 16] = *b"SA_WAVER!!!!!!!!";

    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics, Vst3SubCategory::Distortion];
}

nih_export_vst3!(WaverPlugin);
nih_export_clap!(WaverPlugin);

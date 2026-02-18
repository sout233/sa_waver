use bevy_lookup_curve::{editor::LookupCurveEguiEditor, LookupCurve};
use fundsp::prelude::*;
use nih_plug::prelude::*;
use nih_plug_egui::egui::ColorImage;
use nih_plug_egui::EguiState;
use parking_lot::Mutex;
use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::editor::EditorData;

mod editor;
mod fs;
mod param_knob;
mod sout_ui;

// const WAVEFORM_SIZE: usize = 512;

pub struct WaverPlugin {
    params: Arc<WaverPluginParams>,
    editor_data: EditorData,

    pub linear_ext: Arc<AtomicBool>,
    pub current_resolution: Arc<AtomicUsize>,
    pub current_timebase: Arc<AtomicUsize>,
    // 降采样计数器
    pub sample_counter: usize,
    // 当前块的峰值累加器
    pub current_chunk_peak: f32,
    pub input_peak_follower: f32,
    pub latest_input: Arc<AtomicF32>,
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
            editor_data: EditorData {
                lookup_curve: Arc::new(Mutex::new(lookup_curve.clone())),
                editor: Arc::new(Mutex::new(LookupCurveEguiEditor::fitted_to_curve(&lookup_curve))),
                lut_cache: Arc::new(Mutex::new(initial_lut)),
                waveform_buffer: Arc::new(Mutex::new(vec![])),

                colored_waveform: Arc::new(AtomicBool::new(false)),
                presets: Arc::new(Mutex::new(Vec::new())),
                current_preset: Arc::new(Mutex::new(String::from("Default"))),

                open_save_modal: Arc::new(AtomicBool::new(false)),
                open_msg_modal: Arc::new(AtomicBool::new(false)),
                saving_preset_name: Arc::new(Mutex::new(String::new())),
                msg_modal_title: Arc::new(Mutex::new(String::new())),
                msg_modal_content: Arc::new(Mutex::new(String::new())),
            },
            current_resolution: Arc::new(AtomicUsize::new(1024)),
            current_timebase: Arc::new(AtomicUsize::new(512)),
            linear_ext: Arc::new(AtomicBool::new(false)),
            sample_counter: 0,
            current_chunk_peak: 0.0,
            input_peak_follower: 0.0,
            latest_input: Arc::new(AtomicF32::new(0.5)),
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

            if let Some(lut) = self.editor_data.lut_cache.try_lock() {
                let lut_size = lut.len();
                if let Some(mut waveform_buf) = self.editor_data.waveform_buffer.try_lock() {
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
        self.editor_data.editor(
            self.params.clone(),
            self.latest_input.clone(),
            self.current_resolution.clone(),
            self.current_timebase.clone(),
            self.linear_ext.clone(),
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

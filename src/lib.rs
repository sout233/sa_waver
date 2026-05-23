use bevy_lookup_curve::{editor::LookupCurveEguiEditor, LookupCurve};
use fundsp::prelude::*;
use nih_plug::params::persist::PersistentField;
use nih_plug::prelude::*;
use nih_plug_egui::egui::ColorImage;
use nih_plug_egui::EguiState;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::editor::EditorData;
use crate::oversampling::{ConfigurableOversampler, OversamplingAlgorithm, DEFAULT_OVERSAMPLING_ALGORITHM};

mod editor;
mod fs;
mod oversampling;
mod param_knob;
mod sout_ui;

const MAX_OVERSAMPLING_FACTOR: usize = 3;
const DEFAULT_OVERSAMPLING_FACTOR: usize = 0;
pub const INTERPOLATION_MODE_LINEAR: usize = 0;
pub const INTERPOLATION_MODE_COSINE: usize = 1;
pub const INTERPOLATION_MODE_HERMITE: usize = 2;
pub const DEFAULT_INTERPOLATION_MODE: usize = INTERPOLATION_MODE_LINEAR;

#[derive(Clone, Serialize, Deserialize)]
pub struct PlotStateSnapshot {
    pub curve: LookupCurve,
    pub resolution: usize,
    pub timebase: usize,
    pub linear_ext: bool,
    pub oversampling_factor: usize,
    pub colored_waveform: bool,
    #[serde(default = "default_interpolation_mode")]
    pub interpolation_mode: usize,
    #[serde(default = "default_oversampling_algorithm")]
    pub oversampling_algorithm: usize,
}

pub struct WaverPlugin {
    params: Arc<WaverPluginParams>,
    editor_data: EditorData,

    pub linear_ext: Arc<AtomicBool>,
    pub is_bipolar: Arc<AtomicBool>,

    pub current_resolution: Arc<AtomicUsize>,
    pub current_timebase: Arc<AtomicUsize>,
    pub current_oversampling_factor: Arc<AtomicUsize>,
    pub current_interpolation_mode: Arc<AtomicUsize>,
    pub current_oversampling_algorithm: Arc<AtomicUsize>,
    pub oversampling_times: Arc<AtomicF32>,
    pub oversamplers: Vec<ConfigurableOversampler>,
    pub reported_oversampling_factor: usize,
    pub reported_oversampling_algorithm: usize,
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

    #[persist = "lookup_curve"]
    pub lookup_curve: Arc<Mutex<LookupCurve>>,

    #[persist = "resolution"]
    pub current_resolution: Arc<AtomicUsize>,

    #[persist = "timebase"]
    pub current_timebase: Arc<AtomicUsize>,

    #[persist = "linear_ext"]
    pub linear_ext: Arc<AtomicBool>,

    #[persist = "oversampling_factor"]
    pub current_oversampling_factor: Arc<AtomicUsize>,

    #[persist = "colored_waveform"]
    pub colored_waveform: Arc<AtomicBool>,

    #[persist = "interpolation_mode"]
    pub current_interpolation_mode: Arc<AtomicUsize>,

    #[persist = "oversampling_algorithm"]
    pub current_oversampling_algorithm: Arc<AtomicUsize>,

    #[persist = "current_preset"]
    pub current_preset: Arc<Mutex<String>>,

    #[persist = "saved_plot_state"]
    pub saved_plot_state: Arc<Mutex<PlotStateSnapshot>>,

    #[persist = "plot_dirty"]
    pub plot_dirty: Arc<AtomicBool>,
    pub oversampling_times: Arc<AtomicF32>,

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

        let params = Arc::new(WaverPluginParams::default());
        params.lookup_curve.set(lookup_curve.clone());
        *params.current_preset.lock() = String::from("./Default.ron");
        *params.saved_plot_state.lock() = capture_plot_state(
            &lookup_curve,
            1024,
            512,
            false,
            DEFAULT_OVERSAMPLING_FACTOR,
            false,
            DEFAULT_INTERPOLATION_MODE,
            DEFAULT_OVERSAMPLING_ALGORITHM,
        );
        params.plot_dirty.store(false, Ordering::Relaxed);

        Self {
            params: params.clone(),
            // fun_gain,
            // fun_filter_stereo: Box::new(fun_filter_stereo),
            editor_data: EditorData {
                lookup_curve: params.lookup_curve.clone(),
                curve_dirty: Arc::new(AtomicBool::new(true)),
                editor: Arc::new(Mutex::new(LookupCurveEguiEditor::fitted_to_curve(&lookup_curve))),
                lut_cache: Arc::new(Mutex::new(initial_lut)),
                waveform_buffer: Arc::new(Mutex::new(VecDeque::new())),

                colored_waveform: params.colored_waveform.clone(),
                presets: Arc::new(Mutex::new(Vec::new())),
                current_preset: params.current_preset.clone(),
                plot_dirty: params.plot_dirty.clone(),

                open_save_modal: Arc::new(AtomicBool::new(false)),
                open_msg_modal: Arc::new(AtomicBool::new(false)),
                open_about_modal: Arc::new(AtomicBool::new(false)),
                open_settings_modal: Arc::new(AtomicBool::new(false)),
                saving_preset_name: Arc::new(Mutex::new(String::new())),
                msg_modal_title: Arc::new(Mutex::new(String::new())),
                msg_modal_content: Arc::new(Mutex::new(String::new())),
            },
            current_resolution: params.current_resolution.clone(),
            current_timebase: params.current_timebase.clone(),
            linear_ext: params.linear_ext.clone(),
            is_bipolar: Arc::new(AtomicBool::new(false)),
            current_oversampling_factor: params.current_oversampling_factor.clone(),
            current_interpolation_mode: params.current_interpolation_mode.clone(),
            current_oversampling_algorithm: params.current_oversampling_algorithm.clone(),
            oversampling_times: Arc::new(AtomicF32::new(oversampling_factor_to_times(
                DEFAULT_OVERSAMPLING_FACTOR,
            ) as f32)),
            oversamplers: Vec::new(),
            reported_oversampling_factor: usize::MAX,
            reported_oversampling_algorithm: usize::MAX,
            sample_counter: 0,
            current_chunk_peak: 0.0,
            input_peak_follower: 0.0,
            latest_input: Arc::new(AtomicF32::new(0.5)),
        }
    }
}

impl Default for WaverPluginParams {
    fn default() -> Self {
        let oversampling_times =
            Arc::new(AtomicF32::new(oversampling_factor_to_times(DEFAULT_OVERSAMPLING_FACTOR) as f32));
        let lookup_curve = LookupCurve::load_from_bytes(include_bytes!("default.ron")).unwrap();

        Self {
            editor_state: EguiState::from_size(1040, 520),
            lookup_curve: Arc::new(Mutex::new(lookup_curve.clone())),
            current_resolution: Arc::new(AtomicUsize::new(1024)),
            current_timebase: Arc::new(AtomicUsize::new(512)),
            linear_ext: Arc::new(AtomicBool::new(false)),
            current_oversampling_factor: Arc::new(AtomicUsize::new(DEFAULT_OVERSAMPLING_FACTOR)),
            colored_waveform: Arc::new(AtomicBool::new(false)),
            current_interpolation_mode: Arc::new(AtomicUsize::new(DEFAULT_INTERPOLATION_MODE)),
            current_oversampling_algorithm: Arc::new(AtomicUsize::new(DEFAULT_OVERSAMPLING_ALGORITHM)),
            current_preset: Arc::new(Mutex::new(String::from("./Default.ron"))),
            saved_plot_state: Arc::new(Mutex::new(PlotStateSnapshot {
                curve: lookup_curve.clone(),
                resolution: 1024,
                timebase: 512,
                linear_ext: false,
                oversampling_factor: DEFAULT_OVERSAMPLING_FACTOR,
                colored_waveform: false,
                interpolation_mode: DEFAULT_INTERPOLATION_MODE,
                oversampling_algorithm: DEFAULT_OVERSAMPLING_ALGORITHM,
            })),
            plot_dirty: Arc::new(AtomicBool::new(false)),
            oversampling_times: oversampling_times.clone(),

            pre_gain: FloatParam::new(
                "Pre",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-30.0),
                    max: util::db_to_gain(30.0),
                    factor: FloatRange::gain_skew_factor(-30.0, 30.0),
                },
            )
            .with_smoother(SmoothingStyle::OversamplingAware(
                oversampling_times.clone(),
                &SmoothingStyle::Logarithmic(50.0),
            ))
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
            .with_smoother(SmoothingStyle::OversamplingAware(
                oversampling_times.clone(),
                &SmoothingStyle::Logarithmic(50.0),
            ))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            mix: FloatParam::new("Mix", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_smoother(SmoothingStyle::OversamplingAware(
                    oversampling_times,
                    &SmoothingStyle::Linear(50.0),
                ))
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
        audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        let num_channels = audio_io_layout
            .main_output_channels
            .expect("Plugin was initialized without any outputs")
            .get() as usize;

        self.rebuild_oversamplers(num_channels, buffer_config.max_buffer_size as usize);

        if let Some(oversampler) = self.oversamplers.first() {
            context.set_latency_samples(
                oversampler.latency(self.current_oversampling_factor.load(Ordering::Relaxed)),
            );
        }
        self.reported_oversampling_factor = self.current_oversampling_factor.load(Ordering::Relaxed);
        self.reported_oversampling_algorithm = self.current_oversampling_algorithm.load(Ordering::Relaxed);

        true
    }

    fn reset(&mut self) {
        for oversampler in &mut self.oversamplers {
            oversampler.reset();
        }
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let mut block_max_abs = 0.0f32;
        const DOWNSAMPLE_RATE: usize = 256;
        let linear_ext_enabled = self.linear_ext.load(Ordering::Relaxed);
        let timebase = self.current_timebase.load(Ordering::Relaxed);
        let oversampling_factor = self.current_oversampling_factor.load(Ordering::Relaxed);
        let interpolation_mode = self.current_interpolation_mode.load(Ordering::Relaxed);
        let oversampling_algorithm = self.current_oversampling_algorithm.load(Ordering::Relaxed);
        let resolution = self.current_resolution.load(Ordering::Relaxed);
        let oversampling_times = oversampling_factor_to_times(oversampling_factor);
        self.params
            .oversampling_times
            .store(oversampling_times as f32, Ordering::Relaxed);

        if oversampling_algorithm != self.reported_oversampling_algorithm {
            self.reported_oversampling_algorithm = oversampling_algorithm;
            let num_channels = self.oversamplers.len();
            if num_channels > 0 {
                self.rebuild_oversamplers(num_channels, buffer.samples());
            }
            if let Some(oversampler) = self.oversamplers.first() {
                context.set_latency_samples(oversampler.latency(oversampling_factor));
            }
        }

        if oversampling_factor != self.reported_oversampling_factor {
            self.reported_oversampling_factor = oversampling_factor;
            if let Some(oversampler) = self.oversamplers.first() {
                context.set_latency_samples(oversampler.latency(oversampling_factor));
            }
        }

        sync_lut_cache_from_state(
            &self.params.lookup_curve,
            &self.editor_data.curve_dirty,
            &self.editor_data.lut_cache,
            resolution,
        );

        if let Some(lut) = self.editor_data.lut_cache.try_lock() {
            let lut_size = lut.len();

            for (_, block) in buffer.iter_blocks(buffer.samples()) {
                for (channel_samples, oversampler) in block.into_iter().zip(self.oversamplers.iter_mut()) {
                    oversampler.process(channel_samples, oversampling_factor, |upsampled| {
                        for sample in upsampled {
                            let pre_gain = self.params.pre_gain.smoothed.next();
                            let post_gain = self.params.post_gain.smoothed.next();
                            let mix = self.params.mix.smoothed.next();

                            let dry_signal = *sample;
                            let input = dry_signal * pre_gain;

                            let raw_abs_input = input.abs();
                            let abs_input = if linear_ext_enabled {
                                raw_abs_input
                            } else {
                                raw_abs_input.min(1.0)
                            };

                            if abs_input > block_max_abs {
                                block_max_abs = abs_input;
                            }

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
                                sample_lut(lut.as_slice(), index, fraction, interpolation_mode)
                            };

                            let shaped = curve_val * sign;
                            let wet_signal = shaped * post_gain;
                            *sample = dry_signal * (1.0 - mix) + wet_signal * mix;

                            let out_abs = shaped.abs();
                            if out_abs > self.current_chunk_peak {
                                self.current_chunk_peak = out_abs;
                            }

                            self.sample_counter += 1;
                            if self.sample_counter >= DOWNSAMPLE_RATE * oversampling_times {
                                if let Some(mut waveform_buf) = self.editor_data.waveform_buffer.try_lock() {
                                    if waveform_buf.len() >= timebase {
                                        waveform_buf.pop_front();
                                    }
                                    waveform_buf.push_back(self.current_chunk_peak);
                                }

                                self.sample_counter = 0;
                                self.current_chunk_peak = 0.0;
                            }
                        }
                    });
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
            self.current_oversampling_factor.clone(),
            self.current_interpolation_mode.clone(),
            self.current_oversampling_algorithm.clone(),
        )
    }
}

impl WaverPlugin {
    fn rebuild_oversamplers(&mut self, num_channels: usize, maximum_block_size: usize) {
        let algorithm = OversamplingAlgorithm::from_index(
            self.current_oversampling_algorithm.load(Ordering::Relaxed),
        );
        self.oversamplers = (0..num_channels)
            .map(|_| ConfigurableOversampler::new(maximum_block_size, MAX_OVERSAMPLING_FACTOR, algorithm))
            .collect();
    }
}

pub fn rebuild_lut(curve: &LookupCurve, resolution: usize) -> Vec<f32> {
    let resolution = std::cmp::max(resolution, 2);
    let mut lut = Vec::with_capacity(resolution);
    for i in 0..resolution {
        let t = i as f32 / (resolution - 1) as f32;
        let value = curve.lookup(t);
        lut.push(if value.is_finite() { value } else { 0.0 });
    }
    lut
}

pub fn capture_plot_state(
    curve: &LookupCurve,
    resolution: usize,
    timebase: usize,
    linear_ext: bool,
    oversampling_factor: usize,
    colored_waveform: bool,
    interpolation_mode: usize,
    oversampling_algorithm: usize,
) -> PlotStateSnapshot {
    PlotStateSnapshot {
        curve: curve.clone(),
        resolution,
        timebase,
        linear_ext,
        oversampling_factor,
        colored_waveform,
        interpolation_mode,
        oversampling_algorithm,
    }
}

pub fn plot_state_matches(
    snapshot: &PlotStateSnapshot,
    curve: &LookupCurve,
    resolution: usize,
    timebase: usize,
    linear_ext: bool,
    oversampling_factor: usize,
    colored_waveform: bool,
    interpolation_mode: usize,
    oversampling_algorithm: usize,
) -> bool {
    snapshot.resolution == resolution
        && snapshot.timebase == timebase
        && snapshot.linear_ext == linear_ext
        && snapshot.oversampling_factor == oversampling_factor
        && snapshot.colored_waveform == colored_waveform
        && snapshot.interpolation_mode == interpolation_mode
        && snapshot.oversampling_algorithm == oversampling_algorithm
        && curves_match(&snapshot.curve, curve)
}

fn curves_match(a: &LookupCurve, b: &LookupCurve) -> bool {
    let a_knots = a.knots();
    let b_knots = b.knots();
    if a_knots.len() != b_knots.len() {
        return false;
    }

    a_knots.iter().zip(b_knots.iter()).all(|(left, right)| {
        left.position.x.to_bits() == right.position.x.to_bits()
            && left.position.y.to_bits() == right.position.y.to_bits()
            && std::mem::discriminant(&left.interpolation) == std::mem::discriminant(&right.interpolation)
            && left.left_tangent.slope.to_bits() == right.left_tangent.slope.to_bits()
            && std::mem::discriminant(&left.left_tangent.mode)
                == std::mem::discriminant(&right.left_tangent.mode)
            && option_f32_bits_equal(left.left_tangent.weight, right.left_tangent.weight)
            && left.right_tangent.slope.to_bits() == right.right_tangent.slope.to_bits()
            && std::mem::discriminant(&left.right_tangent.mode)
                == std::mem::discriminant(&right.right_tangent.mode)
            && option_f32_bits_equal(left.right_tangent.weight, right.right_tangent.weight)
    })
}

fn option_f32_bits_equal(a: Option<f32>, b: Option<f32>) -> bool {
    match (a, b) {
        (Some(left), Some(right)) => left.to_bits() == right.to_bits(),
        (None, None) => true,
        _ => false,
    }
}

pub fn sync_lut_cache_from_state(
    curve_state: &Mutex<LookupCurve>,
    dirty_flag: &AtomicBool,
    lut_cache: &Mutex<Vec<f32>>,
    resolution: usize,
) {
    let resolution = std::cmp::max(resolution, 2);
    let needs_sync = dirty_flag.swap(false, Ordering::Relaxed)
        || lut_cache.try_lock().map(|lut| lut.len() != resolution).unwrap_or(false);

    if !needs_sync {
        return;
    }

    if let (Some(curve), Some(mut lut)) = (curve_state.try_lock(), lut_cache.try_lock()) {
        *lut = rebuild_lut(&curve, resolution);
    } else {
        dirty_flag.store(true, Ordering::Relaxed);
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

pub fn sample_lut(lut: &[f32], index: usize, fraction: f32, interpolation_mode: usize) -> f32 {
    if lut.is_empty() {
        return 0.0;
    }

    let idx = std::cmp::Ord::min(index, lut.len().saturating_sub(1));
    let a = lut[idx];
    let b = lut[std::cmp::Ord::min(idx + 1, lut.len() - 1)];
    let t = fraction.clamp(0.0, 1.0);

    match interpolation_mode {
        INTERPOLATION_MODE_LINEAR => {
            let value = a + t * (b - a);
            if value.is_finite() { value } else { a }
        }
        INTERPOLATION_MODE_COSINE => {
            let mu = (1.0 - (t * std::f32::consts::PI).cos()) * 0.5;
            let value = a * (1.0 - mu) + b * mu;
            if value.is_finite() { value } else { a }
        }
        INTERPOLATION_MODE_HERMITE if lut.len() >= 4 && idx < lut.len().saturating_sub(1) => {
            let y0 = lut[idx.saturating_sub(1)];
            let y1 = lut[idx];
            let y2 = lut[idx + 1];
            let y3 = lut[std::cmp::Ord::min(idx + 2, lut.len() - 1)];

            let t2 = t * t;
            let t3 = t2 * t;

            let m1 = 0.5 * (y2 - y0);
            let m2 = 0.5 * (y3 - y1);

            let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
            let h10 = t3 - 2.0 * t2 + t;
            let h01 = -2.0 * t3 + 3.0 * t2;
            let h11 = t3 - t2;

            let value = h00 * y1 + h10 * m1 + h01 * y2 + h11 * m2;
            if value.is_finite() { value } else { y1 }
        }
        _ => {
            let value = a + t * (b - a);
            if value.is_finite() { value } else { a }
        }
    }
}

const fn default_interpolation_mode() -> usize {
    DEFAULT_INTERPOLATION_MODE
}

const fn default_oversampling_algorithm() -> usize {
    DEFAULT_OVERSAMPLING_ALGORITHM
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

const fn oversampling_factor_to_times(factor: usize) -> usize {
    2usize.pow(factor as u32)
}

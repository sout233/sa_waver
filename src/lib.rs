use bevy_lookup_curve::{editor::LookupCurveEguiEditor, KnotInterpolation, LookupCurve, TangentSide};
use bevy_math::Vec2;
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
const DBFS_FLOOR_DB: f32 = -60.0;
pub const INTERPOLATION_MODE_LINEAR: usize = 0;
pub const INTERPOLATION_MODE_COSINE: usize = 1;
pub const INTERPOLATION_MODE_HERMITE: usize = 2;
pub const DEFAULT_INTERPOLATION_MODE: usize = INTERPOLATION_MODE_HERMITE;
pub const SYMMETRY_MODE_SYMMETRIC: usize = 0;
pub const SYMMETRY_MODE_ASYMMETRIC: usize = 1;
pub const DEFAULT_SYMMETRY_MODE: usize = SYMMETRY_MODE_SYMMETRIC;
pub const DISPLAY_MODE_LINEAR: usize = 0;
pub const DISPLAY_MODE_DBFS: usize = 1;
pub const DEFAULT_DISPLAY_MODE: usize = DISPLAY_MODE_LINEAR;
pub const DISPLAY_SCOPE_Y_ONLY: usize = 0;
pub const DISPLAY_SCOPE_XY: usize = 1;
pub const DEFAULT_DISPLAY_SCOPE: usize = DISPLAY_SCOPE_XY;
pub const AUTOMATION_SLOT_COUNT: usize = 8;
pub const AUTOMATION_TARGET_NONE: usize = 0;
pub const AUTOMATION_TARGET_X: usize = 1;
pub const AUTOMATION_TARGET_Y: usize = 2;

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutomationSlotBinding {
    #[serde(default)]
    pub knot_id: usize,
    #[serde(default, alias = "knot_index", skip_serializing)]
    pub legacy_knot_index: usize,
    #[serde(default)]
    pub target: usize,
}

impl Default for AutomationSlotBinding {
    fn default() -> Self {
        Self {
            knot_id: 0,
            legacy_knot_index: 0,
            target: AUTOMATION_TARGET_NONE,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PlotStateSnapshot {
    pub curve: LookupCurve,
    #[serde(default = "default_symmetry_mode")]
    pub symmetry_mode: usize,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PresetFile {
    pub curve: LookupCurve,
    #[serde(default = "default_symmetry_mode")]
    pub symmetry_mode: usize,
}

pub struct WaverPlugin {
    params: Arc<WaverPluginParams>,
    editor_data: EditorData,

    pub linear_ext: Arc<AtomicBool>,
    pub symmetry_mode: Arc<AtomicUsize>,

    pub current_resolution: Arc<AtomicUsize>,
    pub current_timebase: Arc<AtomicUsize>,
    pub current_oversampling_factor: Arc<AtomicUsize>,
    pub current_interpolation_mode: Arc<AtomicUsize>,
    pub current_oversampling_algorithm: Arc<AtomicUsize>,
    pub current_display_mode: Arc<AtomicUsize>,
    pub current_display_scope: Arc<AtomicUsize>,
    pub current_strict_dbfs_ticks: Arc<AtomicBool>,
    pub current_grid_step_x: Arc<AtomicUsize>,
    pub current_grid_step_y: Arc<AtomicUsize>,
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
    pub last_applied_automation_values: [f32; AUTOMATION_SLOT_COUNT],
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

    #[persist = "symmetry_mode"]
    pub symmetry_mode: Arc<AtomicUsize>,

    #[persist = "oversampling_factor"]
    pub current_oversampling_factor: Arc<AtomicUsize>,

    #[persist = "colored_waveform"]
    pub colored_waveform: Arc<AtomicBool>,

    #[persist = "interpolation_mode"]
    pub current_interpolation_mode: Arc<AtomicUsize>,

    #[persist = "oversampling_algorithm"]
    pub current_oversampling_algorithm: Arc<AtomicUsize>,

    #[persist = "display_mode"]
    pub current_display_mode: Arc<AtomicUsize>,

    #[persist = "display_scope"]
    pub current_display_scope: Arc<AtomicUsize>,

    #[persist = "strict_dbfs_ticks"]
    pub current_strict_dbfs_ticks: Arc<AtomicBool>,

    #[persist = "grid_step_x_milli"]
    pub current_grid_step_x: Arc<AtomicUsize>,

    #[persist = "grid_step_y_milli"]
    pub current_grid_step_y: Arc<AtomicUsize>,

    #[persist = "current_preset"]
    pub current_preset: Arc<Mutex<String>>,

    #[persist = "saved_plot_state"]
    pub saved_plot_state: Arc<Mutex<PlotStateSnapshot>>,

    #[persist = "plot_dirty"]
    pub plot_dirty: Arc<AtomicBool>,

    #[persist = "automation_slot_bindings"]
    pub automation_slot_bindings: Arc<Mutex<[AutomationSlotBinding; AUTOMATION_SLOT_COUNT]>>,
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

    #[id = "automation_slot_1"]
    pub automation_slot_1: FloatParam,
    #[id = "automation_slot_2"]
    pub automation_slot_2: FloatParam,
    #[id = "automation_slot_3"]
    pub automation_slot_3: FloatParam,
    #[id = "automation_slot_4"]
    pub automation_slot_4: FloatParam,
    #[id = "automation_slot_5"]
    pub automation_slot_5: FloatParam,
    #[id = "automation_slot_6"]
    pub automation_slot_6: FloatParam,
    #[id = "automation_slot_7"]
    pub automation_slot_7: FloatParam,
    #[id = "automation_slot_8"]
    pub automation_slot_8: FloatParam,
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
            DEFAULT_SYMMETRY_MODE,
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
                settings_tab: Arc::new(AtomicUsize::new(0)),
                segment_generator_kind: Arc::new(AtomicUsize::new(0)),
                segment_generator_cycles: Arc::new(AtomicUsize::new(1)),
                segment_generator_steps: Arc::new(AtomicUsize::new(6)),
                segment_generator_active: Arc::new(AtomicBool::new(false)),
                help_panel_title: Arc::new(Mutex::new(String::from("SA Waver"))),
                help_panel_text: Arc::new(Mutex::new(String::from("by sout audio"))),
                saving_preset_name: Arc::new(Mutex::new(String::new())),
                msg_modal_title: Arc::new(Mutex::new(String::new())),
                msg_modal_content: Arc::new(Mutex::new(String::new())),
                automation_slot_bindings: params.automation_slot_bindings.clone(),
            },
            current_resolution: params.current_resolution.clone(),
            current_timebase: params.current_timebase.clone(),
            linear_ext: params.linear_ext.clone(),
            symmetry_mode: params.symmetry_mode.clone(),
            current_oversampling_factor: params.current_oversampling_factor.clone(),
            current_interpolation_mode: params.current_interpolation_mode.clone(),
            current_oversampling_algorithm: params.current_oversampling_algorithm.clone(),
            current_display_mode: params.current_display_mode.clone(),
            current_display_scope: params.current_display_scope.clone(),
            current_strict_dbfs_ticks: params.current_strict_dbfs_ticks.clone(),
            current_grid_step_x: params.current_grid_step_x.clone(),
            current_grid_step_y: params.current_grid_step_y.clone(),
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
            last_applied_automation_values: current_automation_slot_values(&params),
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
            symmetry_mode: Arc::new(AtomicUsize::new(DEFAULT_SYMMETRY_MODE)),
            current_oversampling_factor: Arc::new(AtomicUsize::new(DEFAULT_OVERSAMPLING_FACTOR)),
            colored_waveform: Arc::new(AtomicBool::new(false)),
            current_interpolation_mode: Arc::new(AtomicUsize::new(DEFAULT_INTERPOLATION_MODE)),
            current_oversampling_algorithm: Arc::new(AtomicUsize::new(DEFAULT_OVERSAMPLING_ALGORITHM)),
            current_display_mode: Arc::new(AtomicUsize::new(DEFAULT_DISPLAY_MODE)),
            current_display_scope: Arc::new(AtomicUsize::new(DEFAULT_DISPLAY_SCOPE)),
            current_strict_dbfs_ticks: Arc::new(AtomicBool::new(false)),
            current_grid_step_x: Arc::new(AtomicUsize::new(100)),
            current_grid_step_y: Arc::new(AtomicUsize::new(100)),
            current_preset: Arc::new(Mutex::new(String::from("./Default.ron"))),
            saved_plot_state: Arc::new(Mutex::new(PlotStateSnapshot {
                curve: lookup_curve.clone(),
                symmetry_mode: DEFAULT_SYMMETRY_MODE,
            })),
            plot_dirty: Arc::new(AtomicBool::new(false)),
            automation_slot_bindings: Arc::new(Mutex::new(
                [AutomationSlotBinding::default(); AUTOMATION_SLOT_COUNT],
            )),
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

            automation_slot_1: automation_slot_param("Automation 1"),
            automation_slot_2: automation_slot_param("Automation 2"),
            automation_slot_3: automation_slot_param("Automation 3"),
            automation_slot_4: automation_slot_param("Automation 4"),
            automation_slot_5: automation_slot_param("Automation 5"),
            automation_slot_6: automation_slot_param("Automation 6"),
            automation_slot_7: automation_slot_param("Automation 7"),
            automation_slot_8: automation_slot_param("Automation 8"),
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
        let mut block_max_signed = 0.0f32;
        const DOWNSAMPLE_RATE: usize = 256;
        let linear_ext_enabled = self.linear_ext.load(Ordering::Relaxed);
        let symmetry_mode = self.symmetry_mode.load(Ordering::Relaxed);
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

        self.sync_automation_slots_to_curve();

        sync_lut_cache_from_state(
            &self.params.lookup_curve,
            &self.editor_data.curve_dirty,
            &self.editor_data.lut_cache,
            resolution,
            symmetry_mode,
        );

        if let Some(lut) = self.editor_data.lut_cache.try_lock() {
            for (_, block) in buffer.iter_blocks(buffer.samples()) {
                for (channel_samples, oversampler) in block.into_iter().zip(self.oversamplers.iter_mut()) {
                    oversampler.process(channel_samples, oversampling_factor, |upsampled| {
                        for sample in upsampled {
                            let pre_gain = self.params.pre_gain.smoothed.next();
                            let post_gain = self.params.post_gain.smoothed.next();
                            let mix = self.params.mix.smoothed.next();

                            let dry_signal = *sample;
                            let input = dry_signal * pre_gain;

                            if input.abs() > block_max_abs {
                                block_max_abs = input.abs();
                                block_max_signed = input;
                            }

                            let shaped = curve_lookup(
                                &lut,
                                input,
                                linear_ext_enabled,
                                interpolation_mode,
                                symmetry_mode,
                                self.current_display_mode.load(Ordering::Relaxed),
                                self.current_display_scope.load(Ordering::Relaxed),
                            );
                            let wet_signal = shaped * post_gain;
                            *sample = dry_signal * (1.0 - mix) + wet_signal * mix;

                            let stored_peak = if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
                                shaped
                            } else {
                                shaped.abs()
                            };
                            if stored_peak.abs() > self.current_chunk_peak.abs() {
                                self.current_chunk_peak = stored_peak;
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

        self.latest_input
            .store(block_max_signed.clamp(-1.5, 1.5), Ordering::Relaxed);

        ProcessStatus::Normal
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        self.editor_data.editor(
            self.params.clone(),
            self.latest_input.clone(),
            self.current_resolution.clone(),
            self.current_timebase.clone(),
            self.linear_ext.clone(),
            self.symmetry_mode.clone(),
            self.current_oversampling_factor.clone(),
            self.current_interpolation_mode.clone(),
            self.current_oversampling_algorithm.clone(),
            self.current_display_mode.clone(),
            self.current_display_scope.clone(),
            self.current_strict_dbfs_ticks.clone(),
            self.current_grid_step_x.clone(),
            self.current_grid_step_y.clone(),
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

    fn sync_automation_slots_to_curve(&mut self) {
        let current_values = current_automation_slot_values(&self.params);
        let changed = current_values
            .iter()
            .zip(self.last_applied_automation_values.iter())
            .any(|(left, right)| left.to_bits() != right.to_bits());

        if !changed {
            return;
        }

        if let Some(mut curve) = self.params.lookup_curve.try_lock() {
            let mut bindings = self
                .params
                .automation_slot_bindings
                .try_lock()
                .map(|bindings| *bindings)
                .unwrap_or([AutomationSlotBinding::default(); AUTOMATION_SLOT_COUNT]);
            if apply_automation_slots_to_curve(
                &mut curve,
                &mut bindings,
                &current_values,
                self.symmetry_mode.load(Ordering::Relaxed),
            ) {
                self.editor_data.curve_dirty.store(true, Ordering::Relaxed);
                self.params.plot_dirty.store(true, Ordering::Relaxed);
            }
            if let Some(mut stored_bindings) = self.params.automation_slot_bindings.try_lock() {
                *stored_bindings = bindings;
            }
            self.last_applied_automation_values = current_values;
        }
    }
}

fn automation_slot_param(name: &str) -> FloatParam {
    FloatParam::new(name, 0.5, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_smoother(SmoothingStyle::Linear(0.0))
        .with_unit("")
        .with_value_to_string(Arc::new(|value| format!("{:.3}", value)))
        .with_string_to_value(Arc::new(|string| string.trim().parse::<f32>().ok()))
}

pub(crate) fn current_automation_slot_values(params: &WaverPluginParams) -> [f32; AUTOMATION_SLOT_COUNT] {
    [
        params.automation_slot_1.modulated_plain_value(),
        params.automation_slot_2.modulated_plain_value(),
        params.automation_slot_3.modulated_plain_value(),
        params.automation_slot_4.modulated_plain_value(),
        params.automation_slot_5.modulated_plain_value(),
        params.automation_slot_6.modulated_plain_value(),
        params.automation_slot_7.modulated_plain_value(),
        params.automation_slot_8.modulated_plain_value(),
    ]
}

pub fn apply_automation_slots_to_curve(
    curve: &mut LookupCurve,
    bindings: &mut [AutomationSlotBinding; AUTOMATION_SLOT_COUNT],
    values: &[f32; AUTOMATION_SLOT_COUNT],
    symmetry_mode: usize,
) -> bool {
    let mut changed = false;

    for slot_index in 0..AUTOMATION_SLOT_COUNT {
        let binding = bindings[slot_index];
        if binding.target == AUTOMATION_TARGET_NONE {
            continue;
        }

        let Some(knot_index) = automation_binding_knot_index(curve, &binding) else {
            continue;
        };

        let original = curve.knots()[knot_index];
        let mut new_position = original.position;
        match binding.target {
            AUTOMATION_TARGET_X => {
                let min_x = curve
                    .prev_knot(knot_index)
                    .map(|knot| knot.position.x + f32::EPSILON)
                    .unwrap_or(if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC { -1.0 } else { 0.0 });
                let max_x = curve
                    .next_knot(knot_index)
                    .map(|knot| knot.position.x - f32::EPSILON)
                    .unwrap_or(1.0);
                let value = values[slot_index];
                let remapped = if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
                    value * 2.0 - 1.0
                } else {
                    value
                };
                new_position.x = remapped.clamp(min_x, max_x);
            }
            AUTOMATION_TARGET_Y => {
                let min_y = if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC { -1.0 } else { 0.0 };
                new_position.y = if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
                    (values[slot_index] * 2.0 - 1.0).clamp(min_y, 1.0)
                } else {
                    values[slot_index].clamp(min_y, 1.0)
                };
            }
            _ => {}
        }

        if new_position.x.to_bits() != original.position.x.to_bits()
            || new_position.y.to_bits() != original.position.y.to_bits()
        {
            curve.modify_knot(
                knot_index,
                bevy_lookup_curve::Knot {
                    position: new_position,
                    ..original
                },
            );
            changed = true;
        }
        bindings[slot_index].knot_id = original.id;
        bindings[slot_index].legacy_knot_index = knot_index;
    }

    changed
}

pub fn automation_binding_knot_index(curve: &LookupCurve, binding: &AutomationSlotBinding) -> Option<usize> {
    if binding.knot_id != 0 {
        curve.knots().iter().position(|knot| knot.id == binding.knot_id)
    } else if binding.legacy_knot_index < curve.knots().len() {
        Some(binding.legacy_knot_index)
    } else {
        None
    }
}

pub fn knot_position_to_automation_value(position: Vec2, target: usize, symmetry_mode: usize) -> f32 {
    match target {
        AUTOMATION_TARGET_X => {
            if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
                ((position.x + 1.0) * 0.5).clamp(0.0, 1.0)
            } else {
                position.x.clamp(0.0, 1.0)
            }
        }
        AUTOMATION_TARGET_Y => {
            if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
                ((position.y + 1.0) * 0.5).clamp(0.0, 1.0)
            } else {
                position.y.clamp(0.0, 1.0)
            }
        }
        _ => 0.5,
    }
}

pub fn infer_symmetry_mode_from_curve(curve: &LookupCurve) -> usize {
    if curve.knots().iter().any(|knot| knot.position.x < 0.0 || knot.position.y < 0.0) {
        SYMMETRY_MODE_ASYMMETRIC
    } else {
        SYMMETRY_MODE_SYMMETRIC
    }
}

pub fn rebuild_lut(curve: &LookupCurve, resolution: usize) -> Vec<f32> {
    rebuild_lut_for_mode(curve, resolution, DEFAULT_SYMMETRY_MODE)
}

pub fn rebuild_lut_for_mode(curve: &LookupCurve, resolution: usize, symmetry_mode: usize) -> Vec<f32> {
    let resolution = std::cmp::max(resolution, 2);
    let mut lut = Vec::with_capacity(resolution);
    for i in 0..resolution {
        let t = if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
            (i as f32 / (resolution - 1) as f32) * 2.0 - 1.0
        } else {
            i as f32 / (resolution - 1) as f32
        };
        let value = curve.lookup(t);
        lut.push(if value.is_finite() { value } else { 0.0 });
    }
    lut
}

pub fn capture_plot_state(
    curve: &LookupCurve,
    symmetry_mode: usize,
) -> PlotStateSnapshot {
    PlotStateSnapshot {
        curve: curve.clone(),
        symmetry_mode,
    }
}

pub fn plot_state_matches(
    snapshot: &PlotStateSnapshot,
    curve: &LookupCurve,
    symmetry_mode: usize,
) -> bool {
    snapshot.symmetry_mode == symmetry_mode
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
    symmetry_mode: usize,
) {
    let resolution = std::cmp::max(resolution, 2);
    let needs_sync = dirty_flag.swap(false, Ordering::Relaxed)
        || lut_cache.try_lock().map(|lut| lut.len() != resolution).unwrap_or(false);

    if !needs_sync {
        return;
    }

    if let (Some(curve), Some(mut lut)) = (curve_state.try_lock(), lut_cache.try_lock()) {
        *lut = rebuild_lut_for_mode(&curve, resolution, symmetry_mode);
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

pub fn curve_lookup_chart(
    lut: &[f32],
    input: f32,
    linear_ext_enabled: bool,
    interpolation_mode: usize,
    symmetry_mode: usize,
) -> f32 {
    if lut.is_empty() {
        return 0.0;
    }

    if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
        let sample_input = if linear_ext_enabled {
            input
        } else {
            input.clamp(-1.0, 1.0)
        };

        let t = ((sample_input + 1.0) * 0.5) * (lut.len() - 1) as f32;
        let clamped_t = t.clamp(0.0, (lut.len() - 1) as f32);
        let index = clamped_t.floor() as usize;
        let fraction = clamped_t - index as f32;

        if t >= (lut.len() - 1) as f32 {
            if linear_ext_enabled && lut.len() >= 2 {
                let last_val = lut[lut.len() - 1];
                let prev_val = lut[lut.len() - 2];
                let step = last_val - prev_val;
                let excess = t - (lut.len() - 1) as f32;
                let linear = last_val + step * excess;
                if linear.is_finite() { linear } else { last_val }
            } else {
                lut[lut.len() - 1]
            }
        } else if t <= 0.0 {
            if linear_ext_enabled && lut.len() >= 2 {
                let first_val = lut[0];
                let next_val = lut[1];
                let step = next_val - first_val;
                let excess = t;
                let linear = first_val + step * excess;
                if linear.is_finite() { linear } else { first_val }
            } else {
                lut[0]
            }
        } else {
            sample_lut(lut, index, fraction, interpolation_mode)
        }
    } else {
        let raw_abs_input = input.abs();
        let abs_input = if linear_ext_enabled {
            raw_abs_input
        } else {
            raw_abs_input.min(1.0)
        };

        let t = abs_input * (lut.len() - 1) as f32;
        let index = t.floor() as usize;
        let fraction = t - index as f32;

        let curve_val = if index >= lut.len() - 1 {
            if linear_ext_enabled && lut.len() >= 2 {
                let last_val = lut[lut.len() - 1];
                let prev_val = lut[lut.len() - 2];
                let slope = last_val - prev_val;
                let excess = t - (lut.len() - 1) as f32;
                let linear = last_val + slope * excess;
                if linear.is_finite() { linear } else { last_val }
            } else {
                lut[lut.len() - 1]
            }
        } else {
            sample_lut(lut, index, fraction, interpolation_mode)
        };

        curve_val * input.signum()
    }
}

pub fn curve_lookup(
    lut: &[f32],
    input: f32,
    linear_ext_enabled: bool,
    interpolation_mode: usize,
    symmetry_mode: usize,
    display_mode: usize,
    display_scope: usize,
) -> f32 {
    if display_mode != DISPLAY_MODE_DBFS {
        return curve_lookup_chart(lut, input, linear_ext_enabled, interpolation_mode, symmetry_mode);
    }

    if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
        let chart_input = if display_scope == DISPLAY_SCOPE_XY {
            signed_audio_to_dbfs_chart(input)
        } else {
            input
        };
        let chart_output =
            curve_lookup_chart(lut, chart_input, linear_ext_enabled, interpolation_mode, symmetry_mode);
        chart_output_to_audio(chart_output, true)
    } else {
        let sign = input.signum();
        let magnitude = input.abs();
        let chart_input = if display_scope == DISPLAY_SCOPE_XY {
            audio_to_dbfs_chart(magnitude)
        } else {
            magnitude
        };
        let chart_output =
            curve_lookup_chart(lut, chart_input, linear_ext_enabled, interpolation_mode, symmetry_mode);
        chart_output_to_audio(chart_output, false) * sign
    }
}

pub fn audio_input_to_chart_input(
    input: f32,
    symmetry_mode: usize,
    display_mode: usize,
    display_scope: usize,
) -> f32 {
    if display_mode != DISPLAY_MODE_DBFS || display_scope != DISPLAY_SCOPE_XY {
        if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
            input
        } else {
            input.abs()
        }
    } else if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
        signed_audio_to_dbfs_chart(input)
    } else {
        audio_to_dbfs_chart(input.abs())
    }
}

pub fn chart_output_to_audio_output(
    chart_output: f32,
    symmetry_mode: usize,
    display_mode: usize,
) -> f32 {
    if display_mode != DISPLAY_MODE_DBFS {
        return if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
            chart_output
        } else {
            chart_output.abs()
        };
    }

    chart_output_to_audio(chart_output, symmetry_mode == SYMMETRY_MODE_ASYMMETRIC)
}

pub fn is_default_linear_curve(curve: &LookupCurve, symmetry_mode: usize) -> bool {
    let knots = curve.knots();
    if symmetry_mode != SYMMETRY_MODE_SYMMETRIC || knots.len() != 2 {
        return false;
    }

    let first = knots[0];
    let second = knots[1];
    positions_close(first.position, Vec2::new(0.0, 0.0))
        && positions_close(second.position, Vec2::new(1.0, 1.0))
        && matches!(first.interpolation, KnotInterpolation::Linear)
        && matches!(second.interpolation, KnotInterpolation::Linear)
}

pub fn build_default_dbfs_curve(symmetry_mode: usize) -> LookupCurve {
    if symmetry_mode == SYMMETRY_MODE_ASYMMETRIC {
        let points = [
            -1.0_f32,
            -0.316_227_76,
            -0.1,
            -0.031_622_776,
            -0.01,
            -0.003_162_277_6,
            -0.001,
            0.0,
            0.001,
            0.003_162_277_6,
            0.01,
            0.031_622_776,
            0.1,
            0.316_227_76,
            1.0,
        ];
        build_dbfs_cubic_curve(&points, true)
    } else {
        let points = [
            0.0_f32,
            0.001,
            0.003_162_277_6,
            0.01,
            0.031_622_776,
            0.1,
            0.316_227_76,
            1.0,
        ];
        build_dbfs_cubic_curve(&points, false)
    }
}

fn audio_to_dbfs_chart(value: f32) -> f32 {
    if value <= 0.0 {
        0.0
    } else if value >= 1.0 {
        value
    } else {
        ((20.0 * value.log10()).max(DBFS_FLOOR_DB) - DBFS_FLOOR_DB) / -DBFS_FLOOR_DB
    }
}

fn dbfs_chart_to_audio(value: f32) -> f32 {
    if value <= 0.0 {
        0.0
    } else if value >= 1.0 {
        value
    } else {
        10.0_f32.powf((value * -DBFS_FLOOR_DB + DBFS_FLOOR_DB) / 20.0)
    }
}

fn signed_audio_to_dbfs_chart(value: f32) -> f32 {
    let sign = value.signum();
    let magnitude = value.abs();
    if magnitude >= 1.0 {
        value
    } else {
        sign * audio_to_dbfs_chart(magnitude)
    }
}

fn chart_output_to_audio(value: f32, bipolar: bool) -> f32 {
    if !bipolar {
        return dbfs_chart_to_audio(value);
    }

    let sign = value.signum();
    let magnitude = value.abs();
    if magnitude >= 1.0 {
        value
    } else {
        sign * dbfs_chart_to_audio(magnitude)
    }
}

fn positions_close(a: Vec2, b: Vec2) -> bool {
    const EPS: f32 = 1.0e-6;
    (a.x - b.x).abs() <= EPS && (a.y - b.y).abs() <= EPS
}

fn build_dbfs_cubic_curve(xs: &[f32], bipolar: bool) -> LookupCurve {
    let points: Vec<Vec2> = xs
        .iter()
        .copied()
        .map(|x| {
            let y = if bipolar {
                signed_audio_to_dbfs_chart(x)
            } else {
                audio_to_dbfs_chart(x)
            };
            Vec2::new(x, y)
        })
        .collect();
    let slopes = monotone_cubic_slopes(&points);

    let knots = points
        .iter()
        .enumerate()
        .map(|(i, point)| {
            let mut knot = bevy_lookup_curve::Knot {
                position: *point,
                interpolation: if i + 1 < points.len() {
                    KnotInterpolation::Cubic
                } else {
                    KnotInterpolation::Linear
                },
                ..Default::default()
            };
            knot = knot.with_tangent_slope(TangentSide::Left, slopes[i]);
            knot = knot.with_tangent_slope(TangentSide::Right, slopes[i]);

            if i > 0 {
                knot = knot.with_tangent_weight(
                    TangentSide::Left,
                    Some(dbfs_segment_weight(points[i - 1].x, points[i].x)),
                );
            }
            if i + 1 < points.len() {
                knot = knot.with_tangent_weight(
                    TangentSide::Right,
                    Some(dbfs_segment_weight(points[i].x, points[i + 1].x)),
                );
            }

            knot
        })
        .collect();

    LookupCurve::new(knots)
}

fn monotone_cubic_slopes(points: &[Vec2]) -> Vec<f32> {
    let n = points.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![0.0];
    }

    let mut h = Vec::with_capacity(n - 1);
    let mut delta = Vec::with_capacity(n - 1);
    for window in points.windows(2) {
        let dx = (window[1].x - window[0].x).max(f32::EPSILON);
        h.push(dx);
        delta.push((window[1].y - window[0].y) / dx);
    }

    let mut slopes = vec![0.0; n];
    if n == 2 {
        slopes[0] = delta[0];
        slopes[1] = delta[0];
        return slopes;
    }

    slopes[0] = monotone_endpoint_slope(h[0], h[1], delta[0], delta[1]);
    slopes[n - 1] = monotone_endpoint_slope(h[n - 2], h[n - 3], delta[n - 2], delta[n - 3]);

    for i in 1..(n - 1) {
        let d0 = delta[i - 1];
        let d1 = delta[i];
        if d0.abs() <= f32::EPSILON || d1.abs() <= f32::EPSILON || d0.signum() != d1.signum() {
            slopes[i] = 0.0;
        } else {
            let w1 = 2.0 * h[i] + h[i - 1];
            let w2 = h[i] + 2.0 * h[i - 1];
            slopes[i] = (w1 + w2) / ((w1 / d0) + (w2 / d1));
        }
    }

    slopes
}

fn monotone_endpoint_slope(h0: f32, h1: f32, d0: f32, d1: f32) -> f32 {
    let mut slope = ((2.0 * h0 + h1) * d0 - h0 * d1) / (h0 + h1).max(f32::EPSILON);
    if slope.signum() != d0.signum() {
        slope = 0.0;
    } else if d0.signum() != d1.signum() && slope.abs() > 3.0 * d0.abs() {
        slope = 3.0 * d0;
    }
    slope
}

fn dbfs_segment_weight(x0: f32, x1: f32) -> f32 {
    let max_x = x0.abs().max(x1.abs());
    if max_x <= 0.01 {
        0.24
    } else if max_x <= 0.1 {
        0.28
    } else {
        1.0 / 3.0
    }
}

pub fn transform_curve_for_symmetry_mode(curve: &LookupCurve, target_mode: usize) -> LookupCurve {
    match target_mode {
        SYMMETRY_MODE_ASYMMETRIC => make_curve_asymmetric(curve),
        _ => make_curve_symmetric(curve),
    }
}

pub fn save_preset_file(path: &str, snapshot: &PlotStateSnapshot) -> Result<(), Box<dyn Error>> {
    let preset = PresetFile {
        curve: snapshot.curve.clone(),
        symmetry_mode: snapshot.symmetry_mode,
    };

    let config = ron::ser::PrettyConfig::new()
        .new_line("\n".to_string())
        .indentor("  ".to_string());
    let serialized = ron::ser::to_string_pretty(&preset, config)?;
    std::fs::write(path, serialized)?;
    Ok(())
}

pub fn load_preset_file(bytes: &[u8]) -> Result<PlotStateSnapshot, Box<dyn Error>> {
    if let Ok(preset) = ron::de::from_bytes::<PresetFile>(bytes) {
        return Ok(PlotStateSnapshot {
            curve: preset.curve,
            symmetry_mode: preset.symmetry_mode,
        });
    }

    let curve = LookupCurve::load_from_bytes(bytes)?;
    Ok(PlotStateSnapshot {
        curve,
        symmetry_mode: default_symmetry_mode(),
    })
}

const fn default_interpolation_mode() -> usize {
    DEFAULT_INTERPOLATION_MODE
}

const fn default_oversampling_algorithm() -> usize {
    DEFAULT_OVERSAMPLING_ALGORITHM
}

const fn default_symmetry_mode() -> usize {
    DEFAULT_SYMMETRY_MODE
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

fn make_curve_asymmetric(curve: &LookupCurve) -> LookupCurve {
    let positive_knots: Vec<_> = curve
        .knots()
        .iter()
        .copied()
        .filter(|knot| knot.position.x >= 0.0)
        .collect();

    if positive_knots.is_empty() {
        return LookupCurve::new(vec![
            bevy_lookup_curve::Knot {
                position: Vec2::new(-1.0, -1.0),
                ..Default::default()
            },
            bevy_lookup_curve::Knot {
                position: Vec2::new(0.0, 0.0),
                ..Default::default()
            },
            bevy_lookup_curve::Knot {
                position: Vec2::new(1.0, 1.0),
                ..Default::default()
            },
        ]);
    }

    let mut mirrored = Vec::with_capacity(positive_knots.len().saturating_sub(1));
    for idx in (1..positive_knots.len()).rev() {
        let mut mirrored_knot = mirror_knot_around_origin(positive_knots[idx]);
        mirrored_knot.interpolation = positive_knots[idx - 1].interpolation;
        mirrored.push(mirrored_knot);
    }

    let mut knots = mirrored;
    knots.extend(positive_knots);
    LookupCurve::new(knots)
}

fn make_curve_symmetric(curve: &LookupCurve) -> LookupCurve {
    let mut positive_knots: Vec<_> = curve
        .knots()
        .iter()
        .copied()
        .filter(|knot| knot.position.x >= 0.0)
        .collect();

    if positive_knots.is_empty() {
        positive_knots.push(bevy_lookup_curve::Knot {
            position: Vec2::new(0.0, 0.0),
            ..Default::default()
        });
        positive_knots.push(bevy_lookup_curve::Knot {
            position: Vec2::new(1.0, 1.0),
            ..Default::default()
        });
    }

    LookupCurve::new(positive_knots)
}

fn mirror_knot_around_origin(knot: bevy_lookup_curve::Knot) -> bevy_lookup_curve::Knot {
    let mut mirrored = bevy_lookup_curve::Knot {
        interpolation: knot.interpolation,
        left_tangent: knot.right_tangent,
        right_tangent: knot.left_tangent,
        ..Default::default()
    };
    mirrored.position = Vec2::new(-knot.position.x, -knot.position.y);
    mirrored
}

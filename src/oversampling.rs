// Adapted from nih-plug's soft_vacuum example oversampler.
// Original copyright (C) 2023-2024 Robbert van der Helm.

use nih_plug::debug::*;
use std::sync::LazyLock;

pub const OVERSAMPLING_ALGORITHM_LANCZOS3: usize = 0;
pub const OVERSAMPLING_ALGORITHM_FLAT_FIR: usize = 1;
pub const DEFAULT_OVERSAMPLING_ALGORITHM: usize = OVERSAMPLING_ALGORITHM_FLAT_FIR;

const LANCZOS3_UPSAMPLING_KERNEL: [f32; 11] = [
    0.02431708,
    -0.0,
    -0.13509491,
    0.0,
    0.6079271,
    1.0,
    0.6079271,
    0.0,
    -0.13509491,
    -0.0,
    0.02431708,
];

const LANCZOS3_DOWNSAMPLING_KERNEL: [f32; 11] = [
    0.01215854,
    -0.0,
    -0.06754746,
    0.0,
    0.30396355,
    0.5,
    0.30396355,
    0.0,
    -0.06754746,
    -0.0,
    0.01215854,
];

const FLAT_FIR_KERNEL_LEN: usize = 95;
const FLAT_FIR_KAISER_BETA: f32 = 5.65;
const FINAL_DOWNSAMPLING_KERNEL_LEN: usize = 255;
const FINAL_DOWNSAMPLING_CUTOFF: f32 = 0.47;
const FINAL_DOWNSAMPLING_KAISER_BETA: f32 = 8.6;

static FLAT_FIR_UPSAMPLING_KERNEL: LazyLock<[f32; FLAT_FIR_KERNEL_LEN]> = LazyLock::new(design_flat_fir_upsampling_kernel);
#[cfg(test)]
static FLAT_FIR_DOWNSAMPLING_KERNEL: LazyLock<[f32; FLAT_FIR_KERNEL_LEN]> =
    LazyLock::new(|| std::array::from_fn(|index| FLAT_FIR_UPSAMPLING_KERNEL[index] * 0.5));
static FINAL_FLAT_FIR_UPSAMPLING_KERNEL: LazyLock<[f32; FINAL_DOWNSAMPLING_KERNEL_LEN]> =
    LazyLock::new(design_final_flat_fir_upsampling_kernel);
static FINAL_FLAT_FIR_DOWNSAMPLING_KERNEL: LazyLock<[f32; FINAL_DOWNSAMPLING_KERNEL_LEN]> =
    LazyLock::new(design_final_flat_fir_downsampling_kernel);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OversamplingAlgorithm {
    Lanczos3,
    FlatFir,
}

impl OversamplingAlgorithm {
    pub fn from_index(index: usize) -> Self {
        match index {
            OVERSAMPLING_ALGORITHM_FLAT_FIR => Self::FlatFir,
            _ => Self::Lanczos3,
        }
    }

    pub fn as_index(self) -> usize {
        match self {
            Self::Lanczos3 => OVERSAMPLING_ALGORITHM_LANCZOS3,
            Self::FlatFir => OVERSAMPLING_ALGORITHM_FLAT_FIR,
        }
    }

    fn kernels(self, stage_number: usize) -> (&'static [f32], &'static [f32]) {
        match self {
            Self::Lanczos3 => (&LANCZOS3_UPSAMPLING_KERNEL, &LANCZOS3_DOWNSAMPLING_KERNEL),
            Self::FlatFir if stage_number == 0 => (
                FINAL_FLAT_FIR_UPSAMPLING_KERNEL.as_slice(),
                FINAL_FLAT_FIR_DOWNSAMPLING_KERNEL.as_slice(),
            ),
            Self::FlatFir => (FLAT_FIR_UPSAMPLING_KERNEL.as_slice(), FINAL_FLAT_FIR_DOWNSAMPLING_KERNEL.as_slice()),
        }
    }

    fn preserves_inserted_input_samples(self, stage_number: usize) -> bool {
        !matches!(self, Self::FlatFir if stage_number == 0)
    }
}

#[derive(Debug)]
pub struct ConfigurableOversampler {
    algorithm: OversamplingAlgorithm,
    stages: Vec<FilterStage>,
    latencies: Vec<u32>,
}

#[derive(Debug, Clone)]
struct FilterStage {
    oversampling_amount: usize,
    upsampling_kernel: &'static [f32],
    downsampling_kernel: &'static [f32],
    upsampling_rb: Vec<f32>,
    upsampling_write_pos: usize,
    additional_upsampling_latency: usize,
    downsampling_rb: Vec<f32>,
    downsampling_write_pos: usize,
    scratch_buffer: Vec<f32>,
    preserves_inserted_input_samples: bool,
}

impl ConfigurableOversampler {
    pub fn new(maximum_block_size: usize, max_factor: usize, algorithm: OversamplingAlgorithm) -> Self {
        let mut stages = Vec::with_capacity(max_factor);
        for stage in 0..max_factor {
            stages.push(FilterStage::new(maximum_block_size, stage, algorithm));
        }

        let latencies = stages
            .iter()
            .map(|stage| stage.effective_latency())
            .scan(0, |total_latency, latency| {
                *total_latency += latency;
                Some(*total_latency)
            })
            .collect();

        Self {
            algorithm,
            stages,
            latencies,
        }
    }

    pub fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.reset();
        }
    }

    pub fn latency(&self, factor: usize) -> u32 {
        if factor == 0 {
            0
        } else {
            self.latencies[factor - 1]
        }
    }

    pub fn algorithm(&self) -> OversamplingAlgorithm {
        self.algorithm
    }

    pub fn process(&mut self, block: &mut [f32], factor: usize, f: impl FnOnce(&mut [f32])) {
        assert!(factor <= self.stages.len());

        if factor == 0 {
            f(block);
            return;
        }

        assert!(
            block.len() <= self.stages[0].scratch_buffer.len() / 2,
            "The block's size exceeds the maximum block size"
        );

        let upsampled = self.upsample_from(block, factor);
        f(upsampled);
        self.downsample_to(block, factor);
    }

    fn upsample_from(&mut self, block: &[f32], factor: usize) -> &mut [f32] {
        assert_ne!(factor, 0);
        assert!(factor <= self.stages.len());

        self.stages[0].upsample_from(block);

        let mut previous_upsampled_block_len = block.len() * 2;
        for to_stage_idx in 1..factor {
            let ([.., from], [to, ..]) = self.stages.split_at_mut(to_stage_idx) else {
                unreachable!()
            };

            to.upsample_from(&from.scratch_buffer[..previous_upsampled_block_len]);
            previous_upsampled_block_len *= 2;
        }

        &mut self.stages[factor - 1].scratch_buffer[..previous_upsampled_block_len]
    }

    fn downsample_to(&mut self, block: &mut [f32], factor: usize) {
        assert_ne!(factor, 0);
        assert!(factor <= self.stages.len());

        let mut next_downsampled_block_len = block.len() * 2usize.pow(factor as u32 - 1);
        for to_stage_idx in (1..factor).rev() {
            let ([.., to], [from, ..]) = self.stages.split_at_mut(to_stage_idx) else {
                unreachable!()
            };

            from.downsample_to(&mut to.scratch_buffer[..next_downsampled_block_len]);
            next_downsampled_block_len /= 2;
        }

        assert_eq!(next_downsampled_block_len, block.len());
        self.stages[0].downsample_to(block);
    }
}

impl FilterStage {
    fn new(maximum_block_size: usize, stage_number: usize, algorithm: OversamplingAlgorithm) -> Self {
        let oversampling_amount = 2usize.pow(stage_number as u32 + 1);
        let (upsampling_kernel, downsampling_kernel) = algorithm.kernels(stage_number);

        assert!(upsampling_kernel.len() % 2 == 1);
        assert!(downsampling_kernel.len() % 2 == 1);

        let upsampling_kernel_latency = upsampling_kernel.len() / 2;
        let downsampling_kernel_latency = downsampling_kernel.len() / 2;
        let uncompensated_stage_latency = upsampling_kernel_latency + downsampling_kernel_latency;
        let additional_delay_required = (-(uncompensated_stage_latency as isize)).rem_euclid(oversampling_amount as isize) as usize;

        Self {
            oversampling_amount,
            upsampling_kernel,
            downsampling_kernel,
            upsampling_rb: vec![0.0; upsampling_kernel.len() + additional_delay_required],
            upsampling_write_pos: 0,
            additional_upsampling_latency: additional_delay_required,
            downsampling_rb: vec![0.0; downsampling_kernel.len()],
            downsampling_write_pos: 0,
            scratch_buffer: vec![0.0; maximum_block_size * oversampling_amount],
            preserves_inserted_input_samples: algorithm.preserves_inserted_input_samples(stage_number),
        }
    }

    fn reset(&mut self) {
        self.upsampling_rb.fill(0.0);
        self.upsampling_write_pos = 0;
        self.downsampling_rb.fill(0.0);
        self.downsampling_write_pos = 0;
    }

    fn effective_latency(&self) -> u32 {
        let uncompensated_stage_latency = (self.upsampling_kernel.len() / 2) + (self.downsampling_kernel.len() / 2);
        let total_stage_latency = uncompensated_stage_latency + self.additional_upsampling_latency;
        let effective_latency = total_stage_latency as f32 / self.oversampling_amount as f32;

        assert!(effective_latency.fract() == 0.0);
        effective_latency as u32
    }

    fn upsample_from(&mut self, block: &[f32]) {
        let output_length = block.len() * 2;
        assert!(output_length <= self.scratch_buffer.len());

        for (input_sample_idx, input_sample) in block.iter().enumerate() {
            let output_sample_idx = input_sample_idx * 2;
            self.scratch_buffer[output_sample_idx] = *input_sample;
            self.scratch_buffer[output_sample_idx + 1] = 0.0;
        }

        let kernel_latency = self.upsampling_kernel.len() / 2;
        let mut direct_read_pos = (self.upsampling_write_pos + kernel_latency) % self.upsampling_rb.len();
        for output_sample_idx in 0..output_length {
            self.upsampling_rb[self.upsampling_write_pos] = self.scratch_buffer[output_sample_idx];

            self.upsampling_write_pos += 1;
            if self.upsampling_write_pos == self.upsampling_rb.len() {
                self.upsampling_write_pos = 0;
            }

            direct_read_pos += 1;
            if direct_read_pos == self.upsampling_rb.len() {
                direct_read_pos = 0;
            }

            self.scratch_buffer[output_sample_idx] =
                if self.preserves_inserted_input_samples && output_sample_idx % 2 == (kernel_latency % 2) {
                    nih_debug_assert_eq!(
                        self.upsampling_rb[(direct_read_pos + self.upsampling_rb.len() - 1) % self.upsampling_rb.len()],
                        0.0
                    );
                    nih_debug_assert_eq!(self.upsampling_rb[(direct_read_pos + 1) % self.upsampling_rb.len()], 0.0);

                    self.upsampling_rb[direct_read_pos]
                } else {
                    convolve_rb(&self.upsampling_rb, self.upsampling_kernel, self.upsampling_write_pos)
                };
        }
    }

    fn downsample_to(&mut self, block: &mut [f32]) {
        let input_length = block.len() * 2;
        assert!(input_length <= self.scratch_buffer.len());

        for input_sample_idx in 0..input_length {
            self.downsampling_rb[self.downsampling_write_pos] = self.scratch_buffer[input_sample_idx];

            self.downsampling_write_pos += 1;
            if self.downsampling_write_pos == self.downsampling_rb.len() {
                self.downsampling_write_pos = 0;
            }

            if input_sample_idx % 2 == 0 {
                let output_sample_idx = input_sample_idx / 2;
                block[output_sample_idx] = convolve_rb(&self.downsampling_rb, self.downsampling_kernel, self.downsampling_write_pos);
            }
        }
    }
}

fn convolve_rb(input_ring_buffer: &[f32], kernel: &[f32], ring_buffer_pos: usize) -> f32 {
    let mut total = 0.0;

    nih_debug_assert!(input_ring_buffer.len() >= kernel.len());

    let num_samples_until_wraparound = std::cmp::Ord::min(input_ring_buffer.len() - ring_buffer_pos, kernel.len());
    for (read_pos_offset, kernel_sample) in kernel.iter().rev().take(num_samples_until_wraparound).enumerate() {
        total += kernel_sample * input_ring_buffer[ring_buffer_pos + read_pos_offset];
    }

    for (read_pos, kernel_sample) in kernel.iter().rev().skip(num_samples_until_wraparound).enumerate() {
        total += kernel_sample * input_ring_buffer[read_pos];
    }

    total
}

fn design_flat_fir_upsampling_kernel() -> [f32; FLAT_FIR_KERNEL_LEN] {
    let center = FLAT_FIR_KERNEL_LEN / 2;
    let mut kernel = [0.0; FLAT_FIR_KERNEL_LEN];

    for (index, sample) in kernel.iter_mut().enumerate() {
        let offset = index as isize - center as isize;
        *sample = if offset == 0 {
            1.0
        } else if offset % 2 == 0 {
            0.0
        } else {
            let offset = offset as f32;
            let ideal_half_band = 2.0 * (std::f32::consts::FRAC_PI_2 * offset).sin() / (std::f32::consts::PI * offset);
            ideal_half_band * kaiser_window(index, FLAT_FIR_KERNEL_LEN, FLAT_FIR_KAISER_BETA)
        };
    }

    let odd_sum: f32 = kernel
        .iter()
        .enumerate()
        .filter_map(|(index, sample)| {
            let offset = index as isize - center as isize;
            (offset != 0 && offset % 2 != 0).then_some(*sample)
        })
        .sum();

    for (index, sample) in kernel.iter_mut().enumerate() {
        let offset = index as isize - center as isize;
        if offset != 0 && offset % 2 != 0 {
            *sample /= odd_sum;
        }
    }

    kernel
}

fn design_final_flat_fir_downsampling_kernel() -> [f32; FINAL_DOWNSAMPLING_KERNEL_LEN] {
    design_lowpass_kernel::<FINAL_DOWNSAMPLING_KERNEL_LEN>(FINAL_DOWNSAMPLING_CUTOFF, FINAL_DOWNSAMPLING_KAISER_BETA)
}

fn design_final_flat_fir_upsampling_kernel() -> [f32; FINAL_DOWNSAMPLING_KERNEL_LEN] {
    let lowpass = design_lowpass_kernel::<FINAL_DOWNSAMPLING_KERNEL_LEN>(FINAL_DOWNSAMPLING_CUTOFF, FINAL_DOWNSAMPLING_KAISER_BETA);

    lowpass.map(|sample| sample * 2.0)
}

fn design_lowpass_kernel<const LEN: usize>(cutoff: f32, beta: f32) -> [f32; LEN] {
    let center = LEN / 2;
    let mut kernel = [0.0; LEN];

    for (index, sample) in kernel.iter_mut().enumerate() {
        let offset = index as isize - center as isize;
        let offset = offset as f32;
        let ideal_lowpass = cutoff * sinc(cutoff * offset);
        *sample = ideal_lowpass * kaiser_window(index, LEN, beta);
    }

    let sum: f32 = kernel.iter().sum();
    for sample in &mut kernel {
        *sample /= sum;
    }

    kernel
}

fn sinc(x: f32) -> f32 {
    if x.abs() <= f32::EPSILON {
        1.0
    } else {
        (std::f32::consts::PI * x).sin() / (std::f32::consts::PI * x)
    }
}

fn kaiser_window(index: usize, len: usize, beta: f32) -> f32 {
    if len <= 1 {
        return 1.0;
    }

    let center = (len - 1) as f32 * 0.5;
    let distance = (index as f32 - center) / center;
    bessel_i0(beta * (1.0 - distance * distance).max(0.0).sqrt()) / bessel_i0(beta)
}

fn bessel_i0(x: f32) -> f32 {
    let y = x * x * 0.25;
    let mut sum = 1.0;
    let mut term = 1.0;

    for order in 1..=32 {
        let order = order as f32;
        term *= y / (order * order);
        sum += term;

        if term <= sum * 1.0e-6 {
            break;
        }
    }

    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SAMPLE_RATE: f32 = 44_100.0;

    #[test]
    fn flat_fir_keeps_audio_band_flat_at_8x() {
        let reference = measured_gain_db(1_000.0, 3);

        for frequency in [15_000.0, 20_000.0] {
            let gain = measured_gain_db(frequency, 3) - reference;
            assert!(
                gain > -0.3 && gain < 0.1,
                "Flat FIR gain at {frequency} Hz should stay close to unity, got {gain:.3} dB"
            );
        }
    }

    #[test]
    fn flat_fir_does_not_boost_near_nyquist_at_8x() {
        let reference = measured_gain_db(1_000.0, 3);

        for frequency in [20_500.0, 21_000.0] {
            let gain = measured_gain_db(frequency, 3) - reference;
            assert!(
                gain <= 0.1,
                "Flat FIR gain at {frequency} Hz should not be boosted, got {gain:.3} dB"
            );
        }
    }

    #[test]
    fn flat_fir_rejects_final_stage_image_and_alias_bands() {
        let passband = frequency_response_db(FINAL_FLAT_FIR_DOWNSAMPLING_KERNEL.as_slice(), 20_000.0);
        let downsampling_stopband = frequency_response_db(FINAL_FLAT_FIR_DOWNSAMPLING_KERNEL.as_slice(), 24_000.0);
        let upsampling_image = frequency_response_db(FINAL_FLAT_FIR_UPSAMPLING_KERNEL.as_slice(), 24_100.0) - 6.020_600_3;

        assert!(passband > -0.3, "20 kHz should remain usable, got {passband:.3} dB");
        assert!(
            downsampling_stopband < -70.0,
            "24 kHz ultrasonic content should be rejected while downsampling, got {downsampling_stopband:.3} dB"
        );
        assert!(
            upsampling_image < -70.0,
            "20 kHz input image around 24.1 kHz should be rejected before shaping, got {upsampling_image:.3} dB"
        );
    }

    #[test]
    fn flat_fir_clipped_signal_does_not_build_high_frequency_tilt_at_8x() {
        let input_gain = db_to_gain(10.0);
        let reference = measured_clipped_fundamental_db(5_000.0, 3, input_gain);

        for frequency in [10_000.0, 15_000.0, 18_000.0, 20_000.0] {
            let gain = measured_clipped_fundamental_db(frequency, 3, input_gain) - reference;
            assert!(
                gain <= 0.75,
                "10 dB clipped signal should not tilt upward at {frequency} Hz, got {gain:.3} dB"
            );
        }
    }

    #[test]
    fn flat_fir_2x_20db_clipped_fundamental_stays_flat() {
        let input_gain = db_to_gain(20.0);
        let reference = measured_clipped_fundamental_db(5_000.0, 1, input_gain);

        for frequency in [10_000.0, 15_000.0, 18_000.0, 20_000.0] {
            let gain = measured_clipped_fundamental_db(frequency, 1, input_gain) - reference;
            assert!(
                gain <= 0.75,
                "20 dB clipped 2x signal fundamental should not tilt upward at {frequency} Hz, got {gain:.3} dB"
            );
        }
    }

    #[test]
    fn flat_fir_has_unity_dc_gain() {
        let upsampling_dc: f32 = FLAT_FIR_UPSAMPLING_KERNEL.iter().sum();
        let downsampling_dc: f32 = FLAT_FIR_DOWNSAMPLING_KERNEL.iter().sum();

        assert!((upsampling_dc - 2.0).abs() < 1.0e-5);
        assert!((downsampling_dc - 1.0).abs() < 1.0e-5);
    }

    fn measured_gain_db(frequency: f32, factor: usize) -> f32 {
        let samples = 65_536;
        let mut signal = vec![0.0; samples];
        let radians_per_sample = std::f32::consts::TAU * frequency / TEST_SAMPLE_RATE;

        for (index, sample) in signal.iter_mut().enumerate() {
            *sample = (radians_per_sample * index as f32).sin();
        }

        let mut oversampler = ConfigurableOversampler::new(samples, factor, OversamplingAlgorithm::FlatFir);
        oversampler.process(&mut signal, factor, |_| {});

        let warmup = oversampler.latency(factor) as usize + 512;
        sine_amplitude(&signal[warmup..], radians_per_sample).max(1.0e-12).log10() * 20.0
    }

    fn measured_clipped_fundamental_db(frequency: f32, factor: usize, input_gain: f32) -> f32 {
        let measurement_len = 65_536;
        let samples = measurement_len * 2;
        let coherent_frequency = coherent_frequency(frequency, measurement_len);
        let radians_per_sample = std::f32::consts::TAU * coherent_frequency / TEST_SAMPLE_RATE;
        let mut signal = vec![0.0; samples];

        for (index, sample) in signal.iter_mut().enumerate() {
            *sample = input_gain * (radians_per_sample * index as f32).sin();
        }

        let mut oversampler = ConfigurableOversampler::new(samples, factor, OversamplingAlgorithm::FlatFir);
        oversampler.process(&mut signal, factor, |upsampled| {
            for sample in upsampled {
                *sample = sample.clamp(-1.0, 1.0);
            }
        });

        let warmup = oversampler.latency(factor) as usize + 512;
        sine_amplitude(&signal[warmup..warmup + measurement_len], radians_per_sample)
            .max(1.0e-12)
            .log10()
            * 20.0
    }

    fn coherent_frequency(frequency: f32, len: usize) -> f32 {
        let bin = (frequency * len as f32 / TEST_SAMPLE_RATE).round().max(1.0);
        bin * TEST_SAMPLE_RATE / len as f32
    }

    fn frequency_response_db(kernel: &[f32], frequency: f32) -> f32 {
        let radians_per_sample = std::f32::consts::PI * frequency / TEST_SAMPLE_RATE;
        let mut real = 0.0;
        let mut imaginary = 0.0;

        for (index, sample) in kernel.iter().enumerate() {
            let phase = -(radians_per_sample * index as f32);
            real += sample * phase.cos();
            imaginary += sample * phase.sin();
        }

        (real * real + imaginary * imaginary).sqrt().max(1.0e-12).log10() * 20.0
    }

    fn db_to_gain(db: f32) -> f32 {
        10.0_f32.powf(db / 20.0)
    }

    fn sine_amplitude(signal: &[f32], radians_per_sample: f32) -> f32 {
        let mut sin_sum = 0.0;
        let mut cos_sum = 0.0;

        for (index, sample) in signal.iter().enumerate() {
            let phase = radians_per_sample * index as f32;
            sin_sum += sample * phase.sin();
            cos_sum += sample * phase.cos();
        }

        2.0 * (sin_sum * sin_sum + cos_sum * cos_sum).sqrt() / signal.len() as f32
    }
}

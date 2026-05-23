// Adapted from nih-plug's soft_vacuum example oversampler.
// Original copyright (C) 2023-2024 Robbert van der Helm.

use nih_plug::debug::*;

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

const LANZCOS3_KERNEL_LATENCY: usize = LANCZOS3_UPSAMPLING_KERNEL.len() / 2;

#[derive(Debug)]
pub struct Lanczos3Oversampler {
    stages: Vec<Lanzcos3Stage>,
    latencies: Vec<u32>,
}

#[derive(Debug, Clone)]
struct Lanzcos3Stage {
    oversampling_amount: usize,
    upsampling_rb: Vec<f32>,
    upsampling_write_pos: usize,
    additional_upsampling_latency: usize,
    downsampling_rb: [f32; LANCZOS3_DOWNSAMPLING_KERNEL.len()],
    downsampling_write_pos: usize,
    scratch_buffer: Vec<f32>,
}

impl Lanczos3Oversampler {
    pub fn new(maximum_block_size: usize, max_factor: usize) -> Self {
        let mut stages = Vec::with_capacity(max_factor);
        for stage in 0..max_factor {
            stages.push(Lanzcos3Stage::new(maximum_block_size, stage));
        }

        let latencies = stages
            .iter()
            .map(|stage| stage.effective_latency())
            .scan(0, |total_latency, latency| {
                *total_latency += latency;
                Some(*total_latency)
            })
            .collect();

        Self { stages, latencies }
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

impl Lanzcos3Stage {
    fn new(maximum_block_size: usize, stage_number: usize) -> Self {
        let oversampling_amount = 2usize.pow(stage_number as u32 + 1);

        assert!(LANCZOS3_UPSAMPLING_KERNEL.len() == LANCZOS3_DOWNSAMPLING_KERNEL.len());
        assert!(LANCZOS3_UPSAMPLING_KERNEL.len() % 2 == 1);

        let uncompensated_stage_latency = LANZCOS3_KERNEL_LATENCY + LANZCOS3_KERNEL_LATENCY;
        let additional_delay_required =
            (-(uncompensated_stage_latency as isize)).rem_euclid(oversampling_amount as isize)
                as usize;

        Self {
            oversampling_amount,
            upsampling_rb: vec![0.0; LANCZOS3_UPSAMPLING_KERNEL.len() + additional_delay_required],
            upsampling_write_pos: 0,
            additional_upsampling_latency: additional_delay_required,
            downsampling_rb: [0.0; LANCZOS3_DOWNSAMPLING_KERNEL.len()],
            downsampling_write_pos: 0,
            scratch_buffer: vec![0.0; maximum_block_size * oversampling_amount],
        }
    }

    fn reset(&mut self) {
        self.upsampling_rb.fill(0.0);
        self.upsampling_write_pos = 0;
        self.downsampling_rb.fill(0.0);
        self.downsampling_write_pos = 0;
    }

    fn effective_latency(&self) -> u32 {
        let uncompensated_stage_latency = LANZCOS3_KERNEL_LATENCY + LANZCOS3_KERNEL_LATENCY;
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

        let mut direct_read_pos =
            (self.upsampling_write_pos + LANZCOS3_KERNEL_LATENCY) % self.upsampling_rb.len();
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
                if output_sample_idx % 2 == (LANZCOS3_KERNEL_LATENCY % 2) {
                    nih_debug_assert_eq!(
                        self.upsampling_rb
                            [(direct_read_pos + self.upsampling_rb.len() - 1) % self.upsampling_rb.len()],
                        0.0
                    );
                    nih_debug_assert_eq!(
                        self.upsampling_rb[(direct_read_pos + 1) % self.upsampling_rb.len()],
                        0.0
                    );

                    self.upsampling_rb[direct_read_pos]
                } else {
                    convolve_rb(
                        &self.upsampling_rb,
                        &LANCZOS3_UPSAMPLING_KERNEL,
                        self.upsampling_write_pos,
                    )
                };
        }
    }

    fn downsample_to(&mut self, block: &mut [f32]) {
        let input_length = block.len() * 2;
        assert!(input_length <= self.scratch_buffer.len());

        for input_sample_idx in 0..input_length {
            self.downsampling_rb[self.downsampling_write_pos] = self.scratch_buffer[input_sample_idx];

            self.downsampling_write_pos += 1;
            if self.downsampling_write_pos == LANCZOS3_DOWNSAMPLING_KERNEL.len() {
                self.downsampling_write_pos = 0;
            }

            if input_sample_idx % 2 == 0 {
                let output_sample_idx = input_sample_idx / 2;
                block[output_sample_idx] = convolve_rb(
                    &self.downsampling_rb,
                    &LANCZOS3_DOWNSAMPLING_KERNEL,
                    self.downsampling_write_pos,
                );
            }
        }
    }
}

fn convolve_rb(input_ring_buffer: &[f32], kernel: &[f32], ring_buffer_pos: usize) -> f32 {
    let mut total = 0.0;

    nih_debug_assert!(input_ring_buffer.len() >= kernel.len());

    let num_samples_until_wraparound = (input_ring_buffer.len() - ring_buffer_pos).min(kernel.len());
    for (read_pos_offset, kernel_sample) in kernel
        .iter()
        .rev()
        .take(num_samples_until_wraparound)
        .enumerate()
    {
        total += kernel_sample * input_ring_buffer[ring_buffer_pos + read_pos_offset];
    }

    for (read_pos, kernel_sample) in kernel
        .iter()
        .rev()
        .skip(num_samples_until_wraparound)
        .enumerate()
    {
        total += kernel_sample * input_ring_buffer[read_pos];
    }

    total
}

// Third-party portions are covered by the MIT notice in docs/THIRD-PARTY-MIT.txt.
//! Per-input sound treatment: channel select, noise gate, compressor,
//! graphic equaliser, gain and delay. Works on interleaved stereo blocks.

use crate::audio::Effects as AudioCfg;
use crate::audio::RATE as AUDIO_RATE;
use std::collections::VecDeque;

pub const EQ_BANDS: [f32; 10] = [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z: [[f32; 2]; 2],
}

impl Biquad {
    /// Peaking filter of the audio cookbook.
    fn peaking(freq: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w = std::f32::consts::TAU * freq / AUDIO_RATE as f32;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * 1.4);
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: -2.0 * c / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: -2.0 * c / a0,
            a2: (1.0 - alpha / a) / a0,
            z: [[0.0; 2]; 2],
        }
    }

    #[inline]
    fn run(&mut self, x: f32, ch: usize) -> f32 {
        let y = self.b0 * x + self.z[ch][0];
        self.z[ch][0] = self.b1 * x - self.a1 * y + self.z[ch][1];
        self.z[ch][1] = self.b2 * x - self.a2 * y;
        y
    }
}

fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn lin_to_db(v: f32) -> f32 {
    20.0 * v.max(1e-6).log10()
}

const DENOISE_SAMPLES: usize = nnnoiseless::DenoiseState::FRAME_SIZE;

struct StereoDenoiser {
    states: [Box<nnnoiseless::DenoiseState<'static>>; 2],
    input: [[f32; DENOISE_SAMPLES]; 2],
    output: [[f32; DENOISE_SAMPLES]; 2],
    position: usize,
}

impl StereoDenoiser {
    fn new() -> Self {
        Self {
            states: std::array::from_fn(|_| nnnoiseless::DenoiseState::new()),
            input: [[0.0; DENOISE_SAMPLES]; 2],
            output: [[0.0; DENOISE_SAMPLES]; 2],
            position: 0,
        }
    }

    fn process(&mut self, block: &mut [f32]) {
        for pair in block.as_chunks_mut::<2>().0 {
            let position = self.position;
            for (channel, sample) in pair.iter_mut().enumerate() {
                self.input[channel][position] = *sample * 32768.0;
                *sample = self.output[channel][position] / 32768.0;
            }
            self.position += 1;
            if self.position == DENOISE_SAMPLES {
                for (channel, state) in self.states.iter_mut().enumerate() {
                    state.process_frame(&mut self.output[channel], &self.input[channel]);
                }
                self.position = 0;
            }
        }
    }
}

#[test]
fn stereo_matrix_uses_both_original_channels() {
    let config = AudioCfg { matrix: [[0.0, 1.0], [0.5, 0.5]], ..Default::default() };
    let mut block = [0.5, -0.25];
    Chain::default().process(&mut block, &config);
    assert_eq!(block, [-0.25, 0.125]);
}

#[test]
fn automatic_gain_raises_a_quiet_signal_on_both_channels() {
    let mut block = vec![0.03; 48_000];
    let cfg = AudioCfg { agc: true, ..Default::default() };
    Chain::default().process(&mut block, &cfg);
    assert!(block[47_998] > 0.10);
    assert_eq!(block[47_998], block[47_999]);
}

/// Filter state of one input.
pub struct Chain {
    denoise: Option<Box<StereoDenoiser>>,
    room_fast: f32,
    room_slow: f32,
    room_gain: f32,
    eq_set: [f32; 10],
    eq: Vec<(usize, Biquad)>,
    gate_level: f32,
    comp_level: f32,
    comp_gain: f32,
    comp_target: f32,
    comp_samples: u8,
    dynamics_level: f32,
    dynamics_gain: f32,
    gate_gain: f32,
    delay: VecDeque<f32>,
}

impl Default for Chain {
    fn default() -> Self {
        Self {
            denoise: None,
            room_fast: 0.0,
            room_slow: 0.0,
            room_gain: 1.0,
            eq_set: [0.0; 10],
            eq: Vec::new(),
            gate_level: 0.0,
            comp_level: 0.0,
            comp_gain: 1.0,
            comp_target: 1.0,
            comp_samples: 0,
            dynamics_level: 0.0,
            dynamics_gain: 1.0,
            gate_gain: 1.0,
            delay: VecDeque::new(),
        }
    }
}

impl Chain {
    fn refresh_eq(&mut self, set: &[f32; 10]) {
        if self.eq_set == *set {
            return;
        }
        self.eq_set = *set;
        self.eq = set
            .iter()
            .enumerate()
            .filter(|(_, g)| g.abs() > 0.01)
            .map(|(i, g)| (i, Biquad::peaking(EQ_BANDS[i], *g)))
            .collect();
    }

    /// Treats one block in place. Nothing set means nothing computed.
    pub fn process(&mut self, block: &mut [f32], cfg: &AudioCfg) {
        if cfg.denoise {
            self.denoise.get_or_insert_with(|| Box::new(StereoDenoiser::new())).process(block);
        } else {
            self.denoise = None;
        }
        if cfg.dereverb || self.room_gain < 0.9999 {
            for pair in block.as_chunks_mut::<2>().0 {
                let level = pair[0].abs().max(pair[1].abs());
                self.room_fast += (level - self.room_fast) * 0.002;
                self.room_slow += (level - self.room_slow) * 0.00012;
                let target = if cfg.dereverb && self.room_slow > 0.0003 {
                    (1.5 * self.room_fast / self.room_slow).clamp(0.35, 1.0)
                } else { 1.0 };
                let slew = if target < self.room_gain { 0.001 } else { 0.007 };
                self.room_gain += (target - self.room_gain) * slew;
                for sample in pair { *sample *= self.room_gain; }
            }
        } else {
            self.room_fast = 0.0;
            self.room_slow = 0.0;
        }
        match cfg.channel {
            1 => block.as_chunks_mut::<2>().0.iter_mut().for_each(|p| p[1] = p[0]),
            2 => block.as_chunks_mut::<2>().0.iter_mut().for_each(|p| p[0] = p[1]),
            3 => block.as_chunks_mut::<2>().0.iter_mut().for_each(|p| p.swap(0, 1)),
            4 => block.as_chunks_mut::<2>().0.iter_mut().for_each(|p| *p = [(p[0] + p[1]) * 0.5; 2]),
            _ => {}
        }
        if cfg.matrix != [[1.0, 0.0], [0.0, 1.0]] {
            for pair in block.as_chunks_mut::<2>().0 {
                let [left, right] = *pair;
                *pair = cfg.matrix.map(|[l, r]| l * left + r * right);
            }
        }
        if cfg.pan.abs() > 0.001 {
            let (left, right) = if cfg.pan > 0.0 { (1.0 - cfg.pan, 1.0) } else { (1.0, 1.0 + cfg.pan) };
            block.as_chunks_mut::<2>().0.iter_mut().for_each(|p| { p[0] *= left; p[1] *= right; });
        }
        if cfg.gate_enabled && cfg.gate_threshold > -89.0 {
            self.gate(block, cfg.gate_threshold);
        }
        if (cfg.compressor_enabled && cfg.comp_ratio > 1.001) || self.compressor_releasing() {
            self.compress(block, cfg);
        }
        self.refresh_eq(if cfg.eq_enabled { &cfg.eq } else { &[0.0; 10] });
        for (_, f) in &mut self.eq {
            for p in block.as_chunks_mut::<2>().0 {
                p[0] = f.run(p[0], 0);
                p[1] = f.run(p[1], 1);
            }
        }
        if cfg.gain_db.abs() > 0.01 {
            let g = db_to_lin(cfg.gain_db);
            block.iter_mut().for_each(|v| *v *= g);
        }
        if cfg.gain_right_db.abs() > 0.01 {
            let gain = db_to_lin(cfg.gain_right_db);
            for pair in block.as_chunks_mut::<2>().0 { pair[1] *= gain; }
        }
        if cfg.agc || cfg.dynamics || (self.dynamics_gain - 1.0).abs() > 0.0001 {
            self.level_dynamics(block, cfg);
        }
        self.apply_delay(block, cfg.delay_ms);
    }

    fn gate(&mut self, block: &mut [f32], threshold_db: f32) {
        let open = db_to_lin(threshold_db);
        for p in block.as_chunks_mut::<2>().0 {
            let level = p[0].abs().max(p[1].abs());
            self.gate_level = if level > self.gate_level { level } else { self.gate_level * 0.9995 };
            let target = if self.gate_level >= open { 1.0 } else { 0.0 };
            self.gate_gain += (target - self.gate_gain) * 0.002;
            p[0] *= self.gate_gain;
            p[1] *= self.gate_gain;
        }
    }

    pub fn compressor_releasing(&self) -> bool {
        self.comp_gain < 0.9999 || self.comp_target < 0.9999
    }

    pub fn settling(&self) -> bool {
        self.compressor_releasing() || (self.dynamics_gain - 1.0).abs() > 0.0001 || self.room_gain < 0.9999
    }

    fn level_dynamics(&mut self, block: &mut [f32], cfg: &AudioCfg) {
        let attack = 1.0 - (-1000.0 / (AUDIO_RATE as f32 * cfg.dynamics_attack_ms)).exp();
        let low = db_to_lin(cfg.dynamics_low);
        let high = db_to_lin(cfg.dynamics_high);
        let goal = db_to_lin(-18.0);
        for pair in block.as_chunks_mut::<2>().0 {
            let peak = pair[0].abs().max(pair[1].abs());
            let speed = if peak > self.dynamics_level { attack } else { 0.0001 };
            self.dynamics_level += (peak - self.dynamics_level) * speed;
            let level = self.dynamics_level.max(0.0001);
            let mut gain = if cfg.agc && level > 0.01 { (goal / level).clamp(0.25, 4.0) } else { 1.0 };
            if cfg.dynamics {
                if level < low { gain *= (level / low).sqrt(); }
                if level * gain > high { gain *= (high / (level * gain)).sqrt(); }
            }
            let slew = if gain < self.dynamics_gain { attack } else { 0.0001 };
            self.dynamics_gain += (gain - self.dynamics_gain) * slew;
            pair[0] *= self.dynamics_gain;
            pair[1] *= self.dynamics_gain;
        }
    }

    fn compress(&mut self, block: &mut [f32], cfg: &AudioCfg) {
        let active = cfg.compressor_enabled && cfg.comp_ratio > 1.001;
        let slope = 1.0 - 1.0 / cfg.comp_ratio;
        if !active { self.comp_target = 1.0; }
        for p in block.as_chunks_mut::<2>().0 {
            let level = p[0].abs().max(p[1].abs());
            let coef = if level > self.comp_level { 0.01 } else { 0.0005 };
            self.comp_level += (level - self.comp_level) * coef;
            if active && self.comp_samples == 0 {
                let over = (lin_to_db(self.comp_level) - cfg.comp_threshold).max(0.0);
                self.comp_target = db_to_lin(-over * slope);
            }
            // Update the gain target every 16 samples and slew the applied gain per sample.
            self.comp_samples = (self.comp_samples + 1) % 16;
            let slew = if self.comp_target < self.comp_gain { 0.01 } else if active { 0.0005 } else { 0.004 };
            self.comp_gain += (self.comp_target - self.comp_gain) * slew;
            p[0] *= self.comp_gain;
            p[1] *= self.comp_gain;
        }
    }

    fn apply_delay(&mut self, block: &mut [f32], ms: f32) {
        let want = ((ms.max(0.0) / 1000.0) * AUDIO_RATE as f32) as usize * 2;
        if want == 0 {
            self.delay.clear();
            return;
        }
        while self.delay.len() < want {
            self.delay.push_back(0.0);
        }
        self.delay.truncate(want);
        for v in block.iter_mut() {
            self.delay.push_back(*v);
            *v = self.delay.pop_front().unwrap_or(0.0);
        }
    }
}

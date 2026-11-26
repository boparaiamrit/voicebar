//! Real-time WASAPI/native microphone capture resampled to 16 kHz mono f32,
//! plus a 16-bar live amplitude ring for the floating waveform visualizer.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub const TARGET_SAMPLE_RATE: u32 = 16_000;
pub const WAVEFORM_BARS: usize = 16;

#[derive(Clone)]
pub struct AudioShared {
    samples: Arc<Mutex<VecDeque<f32>>>,
    bars: Arc<Mutex<[f32; WAVEFORM_BARS]>>,
    peak: Arc<Mutex<f32>>,
}

impl AudioShared {
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(VecDeque::with_capacity(TARGET_SAMPLE_RATE as usize * 4))),
            bars: Arc::new(Mutex::new([0.08; WAVEFORM_BARS])),
            peak: Arc::new(Mutex::new(0.0)),
        }
    }

    pub fn clear(&self) {
        if let Ok(mut q) = self.samples.lock() {
            q.clear();
        }
        if let Ok(mut b) = self.bars.lock() {
            *b = [0.08; WAVEFORM_BARS];
        }
        if let Ok(mut p) = self.peak.lock() {
            *p = 0.0;
        }
    }

    /// Pop exactly `count` samples if available.
    pub fn pop_chunk(&self, count: usize) -> Option<Vec<f32>> {
        let mut q = self.samples.lock().ok()?;
        if q.len() < count {
            return None;
        }
        Some(q.drain(..count).collect())
    }

    /// Drain all remaining samples (used when stopping dictation).
    pub fn drain_all(&self) -> Vec<f32> {
        self.samples
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    }

    pub fn waveform(&self) -> [f32; WAVEFORM_BARS] {
        self.bars
            .lock()
            .map(|b| *b)
            .unwrap_or([0.08; WAVEFORM_BARS])
    }

    fn push_resampled(&self, chunk_16k: &[f32]) {
        if chunk_16k.is_empty() {
            return;
        }
        let mut sum_sq = 0.0f32;
        for &s in chunk_16k {
            sum_sq += s * s;
        }
        let rms = (sum_sq / chunk_16k.len() as f32).sqrt();
        // Perceptually boost normal speech (~0.02..0.25 RMS) into a lively 0.08..1.0 bar height.
        let level = (rms * 7.5).powf(0.75).clamp(0.08, 1.0);

        if let Ok(mut p) = self.peak.lock() {
            *p = *p * 0.55 + level * 0.45;
            let smoothed = *p;
            if let Ok(mut bars) = self.bars.lock() {
                bars.rotate_left(1);
                bars[WAVEFORM_BARS - 1] = smoothed;
            }
        }

        if let Ok(mut q) = self.samples.lock() {
            // Cap buffer at 30 s to prevent unbounded growth if anything stalls.
            const MAX_SAMPLES: usize = TARGET_SAMPLE_RATE as usize * 30;
            if q.len() + chunk_16k.len() > MAX_SAMPLES {
                let to_drop = ((q.len() + chunk_16k.len()) - MAX_SAMPLES).min(q.len());
                q.drain(..to_drop);
            }
            q.extend(chunk_16k.iter().copied());
        }
    }
}

/// Stateful streaming linear resampler from arbitrary device rate -> 16 kHz mono.
struct Resampler {
    step: f64,
    pos: f64,
    last_sample: f32,
    initialized: bool,
}

impl Resampler {
    fn new(in_rate: u32) -> Self {
        Self {
            step: in_rate as f64 / TARGET_SAMPLE_RATE as f64,
            pos: 0.0,
            last_sample: 0.0,
            initialized: false,
        }
    }

    fn process(&mut self, mono: &[f32], out: &mut Vec<f32>) {
        out.clear();
        if mono.is_empty() {
            return;
        }
        if (self.step - 1.0).abs() < 1e-6 {
            out.extend_from_slice(mono);
            return;
        }
        if !self.initialized {
            self.last_sample = mono[0];
            self.initialized = true;
        }

        let len = mono.len() as f64;
        while self.pos < len {
            let idx = self.pos.floor() as isize;
            let frac = (self.pos - idx as f64) as f32;
            let s0 = if idx < 0 {
                self.last_sample
            } else {
                mono[idx as usize]
            };
            let s1 = if (idx + 1) as usize >= mono.len() {
                mono[mono.len() - 1]
            } else {
                mono[(idx + 1) as usize]
            };
            out.push((s0 + (s1 - s0) * frac).clamp(-1.0, 1.0));
            self.pos += self.step;
        }
        self.pos -= len;
        self.last_sample = mono[mono.len() - 1];
    }
}

pub struct MicCapture {
    _stream: Stream,
    pub device_name: String,
}

impl MicCapture {
    pub fn start(shared: AudioShared) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "No default microphone found. Check Windows Sound settings.".to_string())?;
        let device_name = device
            .name()
            .unwrap_or_else(|_| "Default Microphone".to_string());

        let supported = device
            .default_input_config()
            .map_err(|e| format!("Could not read microphone config: {e}"))?;

        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let channels = config.channels as usize;
        let in_rate = config.sample_rate.0;

        let err_fn = |err| eprintln!("voicebar audio stream error: {err}");

        let stream = match sample_format {
            SampleFormat::F32 => {
                let mut resampler = Resampler::new(in_rate);
                let mut mono = Vec::with_capacity(2048);
                let mut out = Vec::with_capacity(1024);
                device.build_input_stream(
                    &config,
                    move |data: &[f32], _| {
                        mix_to_mono_f32(data, channels, &mut mono);
                        resampler.process(&mono, &mut out);
                        shared.push_resampled(&out);
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::I16 => {
                let mut resampler = Resampler::new(in_rate);
                let mut mono = Vec::with_capacity(2048);
                let mut out = Vec::with_capacity(1024);
                device.build_input_stream(
                    &config,
                    move |data: &[i16], _| {
                        mono.clear();
                        for frame in data.chunks(channels.max(1)) {
                            let sum: f32 = frame.iter().map(|&s| s as f32 / 32768.0).sum();
                            mono.push(sum / frame.len() as f32);
                        }
                        resampler.process(&mono, &mut out);
                        shared.push_resampled(&out);
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::U16 => {
                let mut resampler = Resampler::new(in_rate);
                let mut mono = Vec::with_capacity(2048);
                let mut out = Vec::with_capacity(1024);
                device.build_input_stream(
                    &config,
                    move |data: &[u16], _| {
                        mono.clear();
                        for frame in data.chunks(channels.max(1)) {
                            let sum: f32 = frame
                                .iter()
                                .map(|&s| (s as f32 / 32768.0) - 1.0)
                                .sum();
                            mono.push(sum / frame.len() as f32);
                        }
                        resampler.process(&mono, &mut out);
                        shared.push_resampled(&out);
                    },
                    err_fn,
                    None,
                )
            }
            other => {
                return Err(format!("Unsupported microphone sample format: {other:?}"));
            }
        }
        .map_err(|e| format!("Failed to open microphone stream: {e}"))?;

        stream
            .play()
            .map_err(|e| format!("Failed to start microphone capture: {e}"))?;

        Ok(Self {
            _stream: stream,
            device_name,
        })
    }
}

fn mix_to_mono_f32(data: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    if channels <= 1 {
        out.extend_from_slice(data);
        return;
    }
    for frame in data.chunks(channels) {
        let sum: f32 = frame.iter().copied().sum();
        out.push(sum / frame.len() as f32);
    }
}

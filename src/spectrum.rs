//! Live spectrum for the visuals: a 1024-point FFT of the newest output
//! samples, folded into log-spaced bands with fast attack / slow release.
//! Tables are built once; each update is ~20k multiply-adds and no
//! allocation, and it only runs while something on screen uses it.

use crate::engine::Scope;

const N: usize = 1024;
pub const BANDS: usize = 24;

pub struct Spectrum {
    window: Vec<f32>,
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<u16>,
    samples: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    /// Smoothed band levels, 0..1, lowest frequencies first.
    pub bands: [f32; BANDS],
    /// Smoothed overall loudness, 0..1.
    pub energy: f32,
    last_head: usize,
}

impl Default for Spectrum {
    fn default() -> Self {
        let bits = N.trailing_zeros();
        Self {
            window: (0..N).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos()).collect(),
            cos: (0..N / 2).map(|i| (std::f32::consts::TAU * i as f32 / N as f32).cos()).collect(),
            sin: (0..N / 2).map(|i| -(std::f32::consts::TAU * i as f32 / N as f32).sin()).collect(),
            rev: (0..N).map(|i| ((i as u32).reverse_bits() >> (32 - bits)) as u16).collect(),
            samples: vec![0.0; N],
            re: vec![0.0; N],
            im: vec![0.0; N],
            bands: [0.0; BANDS],
            energy: 0.0,
            last_head: usize::MAX,
        }
    }
}

impl Spectrum {
    /// Analyse the newest audio. With `playing` false the bands fall to rest.
    pub fn update(&mut self, scope: &Scope, dt: f32, playing: bool) {
        let mut target = [0.0f32; BANDS];
        let mut loud = 0.0;
        if playing {
            let head = scope.latest(&mut self.samples);
            if head != self.last_head {
                self.last_head = head;
                self.fft();
                let rate = scope.rate.load(std::sync::atomic::Ordering::Relaxed).max(8000) as f32;
                let hz_per_bin = rate / N as f32;
                let (lo, hi) = (45.0f32, 14_000.0f32.min(rate * 0.45));
                for (b, t) in target.iter_mut().enumerate() {
                    let f0 = lo * (hi / lo).powf(b as f32 / BANDS as f32);
                    let f1 = lo * (hi / lo).powf((b + 1) as f32 / BANDS as f32);
                    let (i0, i1) = ((f0 / hz_per_bin) as usize, ((f1 / hz_per_bin) as usize).max((f0 / hz_per_bin) as usize + 1));
                    let mut peak = 0.0f32;
                    for i in i0.max(1)..i1.min(N / 2) {
                        peak = peak.max(self.re[i] * self.re[i] + self.im[i] * self.im[i]);
                    }
                    // dB, normalised for a Hann-windowed full-scale sine.
                    let db = 10.0 * (peak.max(1e-12) / (N as f32 * N as f32 / 16.0)).log10();
                    // Tilt up the highs a little: music has far less energy there.
                    let tilt = 2.5 * b as f32 / BANDS as f32 * 4.0;
                    *t = ((db + tilt + 54.0) / 50.0).clamp(0.0, 1.0);
                }
                loud = (self.samples.iter().map(|s| s * s).sum::<f32>() / N as f32).sqrt();
                loud = (loud * 3.2).min(1.0);
            } else {
                // No new audio this frame (very high refresh): hold.
                target = self.bands;
                loud = self.energy;
            }
        }
        let up = 1.0 - (-dt * 38.0).exp();
        let down = 1.0 - (-dt * 7.0).exp();
        for (b, t) in self.bands.iter_mut().zip(target) {
            *b += (t - *b) * if t > *b { up } else { down };
        }
        self.energy += (loud - self.energy) * if loud > self.energy { up } else { down };
    }

    pub fn resting(&self) -> bool {
        self.energy < 0.002 && self.bands.iter().all(|b| *b < 0.002)
    }

    /// In-place iterative radix-2 FFT of the windowed samples.
    fn fft(&mut self) {
        for i in 0..N {
            let j = self.rev[i] as usize;
            self.re[j] = self.samples[i] * self.window[i];
            self.im[j] = 0.0;
        }
        let mut len = 2;
        while len <= N {
            let half = len / 2;
            let step = N / len;
            for start in (0..N).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * step], self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = self.re[b] * wr - self.im[b] * wi;
                    let ti = self.re[b] * wi + self.im[b] * wr;
                    self.re[b] = self.re[a] - tr;
                    self.im[b] = self.im[a] - ti;
                    self.re[a] += tr;
                    self.im[a] += ti;
                }
            }
            len *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_lights_its_band() {
        let mut s = Spectrum::default();
        let rate = 44_100.0;
        let f = 1000.0;
        for i in 0..N {
            s.samples[i] = (std::f32::consts::TAU * f * i as f32 / rate).sin() * 0.5;
        }
        s.fft();
        let bin = (f / (rate / N as f32)).round() as usize;
        let mag = |i: usize| (s.re[i] * s.re[i] + s.im[i] * s.im[i]).sqrt();
        assert!(mag(bin) > 20.0 * mag(bin + 40), "peak {} vs far {}", mag(bin), mag(bin + 40));
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    #[test]
    fn music_like_signal_gives_visible_bands() {
        let scope = Scope::default();
        let mut s = Spectrum::default();
        for frame in 0..30 {
            for i in 0..735 {
                let t = (frame * 735 + i) as f32 / 44_100.0;
                // Bass + mids + a little noise, around -15 dBFS overall.
                let v = 0.18 * (std::f32::consts::TAU * 80.0 * t).sin()
                    + 0.08 * (std::f32::consts::TAU * 440.0 * t).sin()
                    + 0.03 * (std::f32::consts::TAU * 3000.0 * t).sin();
                scope.push(v);
            }
            s.update(&scope, 1.0 / 60.0, true);
        }
        println!("bands {:?}\nenergy {}", s.bands.map(|b| (b * 100.0).round() / 100.0), s.energy);
        assert!(s.bands.iter().cloned().fold(0.0, f32::max) > 0.4);
        assert!(s.energy > 0.2);
    }
}

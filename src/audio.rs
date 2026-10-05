//! All sound is synthesised here; SIGPIPE ships no audio files (the approach
//! Panzer proved, `games/panzer/src/audio.rs`).
//!
//! - Effects are rendered once at start-up into sample buffers.
//! - One endless source, the stream, plays while data flows: a low hum and a
//!   modem-like chatter of short blips whose rate follows the flow speed, so
//!   fast flow sounds fast and a split stream sounds busier.
//! - Every segment piped plays the next note up a pentatonic scale, so a long
//!   pipe climbs; reaching the target rings a chord.
//!
//! Without an audio device, or with `--mute`, every call does nothing.

use rodio::buffer::SamplesBuffer;
use rodio::{MixerDeviceSink, Source};
use std::f32::consts::TAU;
use std::num::NonZero;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SR: u32 = 44_100;
const SRF: f32 = SR as f32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sfx {
    Place,
    Replace,
    Denied,
    Tick,
    Start,
    Fast,
    Met,
    Loop,
    LoopsBonus,
    Buffer,
    Sink,
    Spill,
    Pass,
    Fail,
    Select,
    /// A rival's junk landing.
    Junk,
}

const ALL_SFX: [Sfx; 16] = [
    Sfx::Place,
    Sfx::Replace,
    Sfx::Denied,
    Sfx::Tick,
    Sfx::Start,
    Sfx::Fast,
    Sfx::Met,
    Sfx::Loop,
    Sfx::LoopsBonus,
    Sfx::Buffer,
    Sfx::Sink,
    Sfx::Spill,
    Sfx::Pass,
    Sfx::Fail,
    Sfx::Select,
    Sfx::Junk,
];

/// Notes for the segments: a pentatonic scale over three octaves.
const SCALE: [f32; 5] = [0.0, 2.0, 4.0, 7.0, 9.0];
const NOTES: usize = 15;

#[derive(Clone, Copy, Default)]
struct Live {
    /// Segments a second while data flows; 0 when it doesn't.
    rate: f32,
    /// Fronts running at once (a tee makes two).
    fronts: u32,
    silent: bool,
}

pub struct Audio {
    // the sink must outlive everything playing on its mixer
    sink: Option<MixerDeviceSink>,
    bank: Vec<SamplesBuffer>,
    notes: Vec<SamplesBuffer>,
    live: Arc<Mutex<Live>>,
    pub muted: bool,
    last: [Option<Instant>; ALL_SFX.len()],
}

impl Audio {
    pub fn new(muted: bool) -> Audio {
        let sink = if muted {
            None
        } else {
            match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(mut s) => {
                    s.log_on_drop(false);
                    Some(s)
                }
                Err(e) => {
                    log::warn!("no audio: {e}");
                    None
                }
            }
        };
        let live = Arc::new(Mutex::new(Live::default()));
        let (bank, notes) = if sink.is_some() {
            (ALL_SFX.iter().map(|&s| buffer(render_sfx(s))).collect(), (0..NOTES).map(|i| buffer(render_note(i))).collect())
        } else {
            (Vec::new(), Vec::new())
        };
        if let Some(s) = &sink {
            s.mixer().add(Stream::new(live.clone()));
        }
        Audio { sink, bank, notes, live, muted, last: [None; ALL_SFX.len()] }
    }

    pub fn play(&mut self, sfx: Sfx) {
        if self.muted {
            return;
        }
        let Some(sink) = &self.sink else { return };
        let i = ALL_SFX.iter().position(|&s| s == sfx).unwrap_or(0);
        let now = Instant::now();
        if self.last[i].is_some_and(|t| now.duration_since(t) < Duration::from_millis(35)) {
            return;
        }
        self.last[i] = Some(now);
        sink.mixer().add(self.bank[i].clone());
    }

    /// The note for the `n`th segment piped (from 1), climbing the scale.
    pub fn segment(&mut self, n: u32) {
        if self.muted {
            return;
        }
        let Some(sink) = &self.sink else { return };
        let i = (n.max(1) as usize - 1).min(NOTES - 1);
        sink.mixer().add(self.notes[i].clone());
    }

    /// What the stream plays: `rate` segments a second (0: quiet), `fronts` at once.
    pub fn set_stream(&mut self, rate: f32, fronts: u32) {
        if let Ok(mut l) = self.live.lock() {
            l.rate = rate;
            l.fronts = fronts;
        }
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        if let Ok(mut l) = self.live.lock() {
            l.silent = self.muted;
        }
    }
}

fn buffer(samples: Vec<f32>) -> SamplesBuffer {
    SamplesBuffer::new(NonZero::new(1).unwrap(), NonZero::new(SR).unwrap(), samples)
}

// ------------------------------------------------------------------ helpers

struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn midi(n: f32) -> f32 {
    440.0 * 2f32.powf((n - 69.0) / 12.0)
}

fn square(ph: f32) -> f32 {
    if ph - ph.floor() < 0.5 { 1.0 } else { -1.0 }
}

fn tri(ph: f32) -> f32 {
    let p = ph - ph.floor();
    4.0 * (p - 0.5).abs() - 1.0
}

/// `dur` seconds of samples from `f(t, noise)`.
fn synth(dur: f32, mut f: impl FnMut(f32, &mut Noise) -> f32) -> Vec<f32> {
    let mut n = Noise(0x1234_5678);
    (0..(dur * SRF) as usize).map(|i| f(i as f32 / SRF, &mut n)).collect()
}

/// A little tune: (midi note, seconds), played on `wave` with a soft attack.
fn tune(notes: &[(f32, f32)], gain: f32, wave: fn(f32) -> f32) -> Vec<f32> {
    let mut out = Vec::new();
    let mut ph = 0.0f32;
    let mut lp = 0.0f32;
    for &(n, d) in notes {
        let len = (d * SRF) as usize;
        for i in 0..len {
            let t = i as f32 / SRF;
            if n > 0.0 {
                ph += midi(n) / SRF;
            }
            let env = (t / 0.008).min(1.0) * ((d - t) / 0.03).clamp(0.0, 1.0);
            let x = if n > 0.0 { wave(ph) * env } else { 0.0 };
            lp += (x - lp) * 0.3;
            out.push(lp * gain);
        }
    }
    out
}

/// Several tunes played together.
fn chord(parts: &[Vec<f32>]) -> Vec<f32> {
    let len = parts.iter().map(Vec::len).max().unwrap_or(0);
    (0..len).map(|i| parts.iter().map(|p| p.get(i).copied().unwrap_or(0.0)).sum()).collect()
}

/// A falling, bubbling liquid: noise through a sweeping resonant filter.
fn gurgle(dur: f32, from: f32, to: f32, gain: f32, seed: u32) -> Vec<f32> {
    let (mut lp, mut bp) = (0.0f32, 0.0f32);
    let mut n = Noise(seed);
    let mut wob = 0.0f32;
    (0..(dur * SRF) as usize)
        .map(|i| {
            let t = i as f32 / SRF;
            let k = t / dur;
            wob += (9.0 + 6.0 * k) / SRF;
            let f = (from + (to - from) * k) * (1.0 + 0.35 * (wob * TAU).sin());
            let c = (TAU * f / SRF).min(0.9);
            // state-variable filter, resonant band-pass
            lp += c * bp;
            let hp = n.next() - lp - 0.12 * bp;
            bp += c * hp;
            let env = (t / 0.02).min(1.0) * (1.0 - k).powf(1.4);
            bp * env * gain
        })
        .collect()
}

fn render_note(i: usize) -> Vec<f32> {
    let n = 64.0 + SCALE[i % 5] + 12.0 * (i / 5) as f32;
    let f = midi(n);
    synth(0.16, |t, _| {
        let env = (t / 0.004).min(1.0) * (-t * 22.0).exp();
        ((TAU * f * t).sin() * 0.7 + (TAU * 2.0 * f * t).sin() * 0.15) * env * 0.22
    })
}

fn render_sfx(s: Sfx) -> Vec<f32> {
    match s {
        // a relay click as the piece seats
        Sfx::Place => synth(0.045, |t, n| {
            let env = (-t * 140.0).exp();
            (n.next() * 0.5 + (TAU * 1900.0 * t).sin() * 0.6) * env * 0.35
        }),
        // rm -f: a short crunch, then the click of the new piece
        Sfx::Replace => {
            let mut v = synth(0.14, |t, n| {
                let env = (t / 0.004).min(1.0) * (-t * 26.0).exp();
                let crush = (n.next() * 8.0).round() / 8.0;
                crush * env * 0.4
            });
            v.extend(render_sfx(Sfx::Place));
            v
        }
        Sfx::Denied => synth(0.13, |t, _| square(95.0 * t) * (1.0 - t / 0.13) * 0.13),
        Sfx::Tick => synth(0.03, |t, _| (TAU * 2600.0 * t).sin() * (-t * 200.0).exp() * 0.3),
        // the process starts writing: a rising chirp with a modem's grit
        Sfx::Start => {
            let mut ph = 0.0f32;
            synth(0.42, |t, n| {
                ph += (300.0 + 2600.0 * (t / 0.42).powi(2)) / SRF;
                let env = (t / 0.02).min(1.0) * (1.0 - t / 0.42);
                (square(ph) * 0.6 + n.next() * 0.15) * env * 0.16
            })
        }
        Sfx::Fast => {
            let mut ph = 0.0f32;
            synth(0.25, |t, n| {
                ph += (500.0 + 1800.0 * t / 0.25) / SRF;
                ((ph * TAU).sin() * 0.4 + n.next() * 0.3) * (1.0 - t / 0.25) * 0.2
            })
        }
        Sfx::Met => chord(&[
            tune(&[(76.0, 0.5)], 0.13, tri),
            tune(&[(0.0, 0.05), (80.0, 0.45)], 0.11, tri),
            tune(&[(0.0, 0.1), (83.0, 0.4)], 0.1, tri),
        ]),
        Sfx::Loop => tune(&[(79.0, 0.06), (83.0, 0.06), (86.0, 0.06), (91.0, 0.16)], 0.2, square),
        Sfx::LoopsBonus => tune(
            &[(72.0, 0.08), (76.0, 0.08), (79.0, 0.08), (84.0, 0.08), (79.0, 0.08), (84.0, 0.08), (88.0, 0.4)],
            0.2,
            square,
        ),
        Sfx::Buffer => gurgle(0.6, 300.0, 700.0, 0.5, 7),
        // /dev/null: a swirl going down the drain
        Sfx::Sink => gurgle(0.9, 900.0, 120.0, 0.55, 11),
        // broken pipe: a hiss and a falling gurgle
        Sfx::Spill => {
            let g = gurgle(1.2, 700.0, 90.0, 0.6, 3);
            let hiss = synth(1.2, |t, n| n.next() * (t / 0.01).min(1.0) * (-t * 3.0).exp() * 0.12);
            chord(&[g, hiss])
        }
        Sfx::Pass => tune(&[(72.0, 0.1), (76.0, 0.1), (79.0, 0.1), (84.0, 0.35)], 0.2, tri),
        Sfx::Fail => tune(&[(64.0, 0.18), (60.0, 0.18), (55.0, 0.18), (48.0, 0.5)], 0.18, square),
        // static: line noise chopped into a stutter
        Sfx::Junk => synth(0.3, |t, n| {
            let gate = if (t * 40.0) as u32 % 2 == 0 { 1.0 } else { 0.3 };
            n.next() * gate * (1.0 - t / 0.3) * 0.3
        }),
        Sfx::Select => tune(&[(84.0, 0.05), (91.0, 0.08)], 0.14, square),
    }
}

// ------------------------------------------------------------------ the stream

/// The sound of data flowing: hum plus chatter, rendered in blocks.
struct Stream {
    live: Arc<Mutex<Live>>,
    buf: Vec<f32>,
    pos: usize,
    noise: Noise,
    hum: f32,
    level: f32,
    lp: f32,
    /// The current blip: frequency, samples left, and its phase.
    blip: (f32, usize, f32),
    gap: usize,
}

const BLOCK: usize = 512;

impl Stream {
    fn new(live: Arc<Mutex<Live>>) -> Stream {
        Stream { live, buf: vec![0.0; BLOCK], pos: BLOCK, noise: Noise(0xBEEF), hum: 0.0, level: 0.0, lp: 0.0, blip: (0.0, 0, 0.0), gap: 0 }
    }

    fn render(&mut self) {
        let live = self.live.lock().map(|l| *l).unwrap_or_default();
        let target = if live.silent || live.rate <= 0.0 { 0.0 } else { 1.0 };
        // blips per second: a dozen per segment, more with more fronts
        let per_s = live.rate * 12.0 * (1.0 + 0.5 * live.fronts.saturating_sub(1) as f32);
        for i in 0..BLOCK {
            self.level += (target - self.level) * 0.0008;
            self.hum += 55.0 / SRF;
            let hum = (self.hum * TAU).sin() * 0.5 + (self.hum * 2.0 * TAU).sin() * 0.25;
            let mut x = hum * 0.05;
            if self.gap > 0 {
                self.gap -= 1;
            } else if per_s > 0.0 {
                const PITCHES: [f32; 4] = [980.0, 1225.0, 1470.0, 1960.0];
                let pick = ((self.noise.next() + 1.0) * 2.0) as usize % 4;
                self.blip = (PITCHES[pick], (SRF * 0.012) as usize, 0.0);
                let jitter = 0.6 + 0.8 * (self.noise.next() + 1.0) / 2.0;
                self.gap = (SRF / per_s * jitter) as usize;
            }
            if self.blip.1 > 0 {
                self.blip.1 -= 1;
                self.blip.2 += self.blip.0 / SRF;
                x += square(self.blip.2) * 0.035;
            }
            self.lp += (x - self.lp) * 0.25;
            self.buf[i] = self.lp * self.level;
        }
    }
}

impl Iterator for Stream {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.buf.len() {
            self.render();
            self.pos = 0;
        }
        self.pos += 1;
        Some(self.buf[self.pos - 1])
    }
}

impl Source for Stream {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        NonZero::new(1).unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        NonZero::new(SR).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_render_and_stay_in_range() {
        for s in ALL_SFX {
            let v = render_sfx(s);
            assert!(!v.is_empty(), "{s:?} is empty");
            let peak = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(v.iter().all(|x| x.is_finite()), "{s:?} not finite");
            assert!(peak > 0.01 && peak <= 1.0, "{s:?} peak {peak}");
        }
        for i in 0..NOTES {
            let peak = render_note(i).iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak > 0.05 && peak <= 1.0, "note {i} peak {peak}");
        }
    }

    #[test]
    fn stream_plays_while_flowing_and_hushes_after() {
        let live = Arc::new(Mutex::new(Live { rate: 3.0, fronts: 2, silent: false }));
        let mut s = Stream::new(live.clone());
        let loud = (0..SR as usize * 3).map(|_| s.next().unwrap()).fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(loud > 0.01 && loud <= 1.0, "peak {loud}");
        live.lock().unwrap().rate = 0.0;
        let quiet: f32 = (0..SR as usize * 3).map(|_| s.next().unwrap()).skip(SR as usize * 2).fold(0.0, |m, x| m.max(x.abs()));
        assert!(quiet < 0.002, "still {quiet} after stopping");
    }
}

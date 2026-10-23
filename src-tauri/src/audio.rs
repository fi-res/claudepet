//! Listens to what the computer is playing (WASAPI loopback on Windows,
//! the PulseAudio/PipeWire sink monitor on Linux) and turns it into a few
//! simple events for the pet: sudden loud sounds, music on/off and beats.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use serde::Serialize;

pub const SAMPLE_RATE: u32 = 22_050;
const HOP: usize = (SAMPLE_RATE / 50) as usize; // 20 ms
const HOPS_PER_SEC: usize = 50;

const SILENCE_DB: f32 = -50.0;
const LOUD_DB: f32 = -18.0;
const LOUD_JUMP_DB: f32 = 15.0;
const LOUD_COOLDOWN: usize = 2 * HOPS_PER_SEC;
/// A loud sound must come out of relative quiet: nothing this close to it in the
/// last 1.5 s. Keeps speech, which pauses only briefly, from startling Clawd.
const LOUD_QUIET_BEFORE: usize = 75;
const LOUD_ABOVE_RECENT_DB: f32 = 20.0;
const BACKGROUND_SECS: f32 = 3.0;
const BASS_CUTOFF_HZ: f32 = 150.0;
const BEAT_RATIO: f32 = 1.6;
const BEAT_MIN_GAP: usize = 13; // ~260 ms, i.e. up to ~230 BPM

// Music vs speech, judged on the last 6 s every 0.5 s. Thresholds were tuned on
// real speech (conversations, voice messages, talking over background music),
// several music genres and white noise.
const WINDOW: usize = 6 * HOPS_PER_SEC;
const JUDGE_EVERY: usize = HOPS_PER_SEC / 2;
const MIN_ACTIVE: f32 = 0.9;
/// Share of 20 ms hops quieter than half their second's average: speech
/// keeps dropping between syllables and words (0.3-0.5), music doesn't (< 0.1).
const MAX_LOW_ENERGY: f32 = 0.2;
/// Loudness envelope std/mean over the window: speech ~1, music ~0.2-0.4.
const MAX_DEPTH: f32 = 0.5;
/// Spectral flatness: 0 for tones, ~0.6 for white noise (rain, fans, hiss).
const MAX_FLATNESS: f32 = 0.25;
const ON_AFTER: usize = 6;
const OFF_AFTER: usize = 6;

const FFT_SIZE: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AudioEvent {
    Loud { db: f32 },
    Music { playing: bool },
    Beat { strength: f32 },
}

/// In-place radix-2 FFT over (re, im) pairs; `buf.len()` must be a power of two.
fn fft(buf: &mut [(f32, f32)]) {
    let n = buf.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            buf.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * std::f32::consts::PI / len as f32;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (angle * k as f32).sin_cos();
                let (re, im) = buf[start + k + len / 2];
                let t = (re * c - im * s, re * s + im * c);
                let u = buf[start + k];
                buf[start + k] = (u.0 + t.0, u.1 + t.1);
                buf[start + k + len / 2] = (u.0 - t.0, u.1 - t.1);
            }
        }
        len <<= 1;
    }
}

pub struct Analyzer {
    pending: Vec<f32>,
    lowpass: f32,
    lowpass_alpha: f32,
    background: f32,
    bass_history: VecDeque<f32>,
    window_fn: Vec<f32>,
    spectrum: Vec<(f32, f32)>,
    /// Per-hop (rms, spectral flatness) for the last `WINDOW` hops.
    history: VecDeque<(f32, f32)>,
    since_judge: usize,
    streak: usize,
    music: bool,
    since_beat: usize,
    since_loud: usize,
}

impl Default for Analyzer {
    fn default() -> Self {
        let dt = 1.0 / SAMPLE_RATE as f32;
        let rc = 1.0 / (2.0 * std::f32::consts::PI * BASS_CUTOFF_HZ);
        let hann = (0..HOP)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (HOP - 1) as f32).cos())
            .collect();
        Self {
            pending: Vec::with_capacity(HOP),
            lowpass: 0.0,
            lowpass_alpha: dt / (rc + dt),
            background: 0.0,
            bass_history: VecDeque::with_capacity(HOPS_PER_SEC),
            window_fn: hann,
            spectrum: vec![(0.0, 0.0); FFT_SIZE],
            history: VecDeque::with_capacity(WINDOW),
            since_judge: 0,
            streak: 0,
            music: false,
            since_beat: BEAT_MIN_GAP,
            since_loud: LOUD_COOLDOWN,
        }
    }
}

fn to_db(rms: f32) -> f32 {
    20.0 * (rms + 1e-9).log10()
}

impl Analyzer {
    /// Feed mono samples at `SAMPLE_RATE`; events are passed to `emit` as they occur.
    pub fn push(&mut self, samples: &[f32], emit: &mut impl FnMut(AudioEvent)) {
        for &s in samples {
            self.pending.push(s);
            if self.pending.len() == HOP {
                self.hop(emit);
                self.pending.clear();
            }
        }
    }

    pub fn is_music(&self) -> bool {
        self.music
    }

    /// Spectral flatness of the pending hop over ~86 Hz..8.6 kHz.
    fn flatness(&mut self) -> f32 {
        for (i, slot) in self.spectrum.iter_mut().enumerate() {
            *slot = (self.pending.get(i).map_or(0.0, |s| s * self.window_fn[i]), 0.0);
        }
        fft(&mut self.spectrum);
        let bins = &self.spectrum[2..200];
        let (mut log_sum, mut sum) = (0.0f32, 0.0f32);
        for &(re, im) in bins {
            let power = re * re + im * im + 1e-12;
            log_sum += power.ln();
            sum += power;
        }
        let n = bins.len() as f32;
        (log_sum / n).exp() / (sum / n)
    }

    /// Whether the last `WINDOW` hops sound like music rather than speech or noise.
    fn sounds_like_music(&self) -> bool {
        let n = self.history.len() as f32;
        let active = self.history.iter().filter(|(rms, _)| to_db(*rms) > SILENCE_DB).count() as f32 / n;
        if active < MIN_ACTIVE {
            return false;
        }
        let mean = self.history.iter().map(|(rms, _)| rms).sum::<f32>() / n;
        let var = self.history.iter().map(|(rms, _)| (rms - mean).powi(2)).sum::<f32>() / n;
        let depth = var.sqrt() / (mean + 1e-9);

        let rms: Vec<f32> = self.history.iter().map(|(rms, _)| *rms).collect();
        let mut low = 0;
        for second in rms.chunks(HOPS_PER_SEC) {
            let avg = second.iter().sum::<f32>() / second.len() as f32;
            low += second.iter().filter(|&&r| r < 0.5 * avg).count();
        }
        let low_energy = low as f32 / n;

        let mut flat: Vec<f32> = self.history.iter().filter(|(r, _)| *r > 0.3 * mean).map(|(_, f)| *f).collect();
        flat.sort_by(f32::total_cmp);
        let flatness = flat.get(flat.len() / 2).copied().unwrap_or(1.0);

        low_energy < MAX_LOW_ENERGY && depth < MAX_DEPTH && flatness < MAX_FLATNESS
    }

    fn hop(&mut self, emit: &mut impl FnMut(AudioEvent)) {
        let mut energy = 0.0;
        let mut bass = 0.0;
        for &s in &self.pending {
            energy += s * s;
            self.lowpass += self.lowpass_alpha * (s - self.lowpass);
            bass += self.lowpass * self.lowpass;
        }
        let rms = (energy / HOP as f32).sqrt();
        let bass = bass / HOP as f32;
        let db = to_db(rms);

        // A sudden jump well above what has been playing lately.
        self.since_loud += 1;
        let recent_peak = self
            .history
            .iter()
            .rev()
            .take(LOUD_QUIET_BEFORE)
            .map(|(rms, _)| *rms)
            .fold(0.0, f32::max);
        if self.since_loud >= LOUD_COOLDOWN
            && db > LOUD_DB
            && db > to_db(self.background) + LOUD_JUMP_DB
            && db > to_db(recent_peak) + LOUD_ABOVE_RECENT_DB
        {
            self.since_loud = 0;
            emit(AudioEvent::Loud { db });
        }
        self.background += (rms - self.background) / (BACKGROUND_SECS * HOPS_PER_SEC as f32);

        // Silent hops skip the FFT; their flatness is never looked at.
        let flatness = if db > SILENCE_DB { self.flatness() } else { 1.0 };
        if self.history.len() == WINDOW {
            self.history.pop_front();
        }
        self.history.push_back((rms, flatness));

        // Hysteresis: the verdict has to hold for a few judgements in a row.
        self.since_judge += 1;
        if self.history.len() == WINDOW && self.since_judge >= JUDGE_EVERY {
            self.since_judge = 0;
            let verdict = self.sounds_like_music();
            self.streak = if verdict != self.music { self.streak + 1 } else { 0 };
            let needed = if self.music { OFF_AFTER } else { ON_AFTER };
            if self.streak >= needed {
                self.streak = 0;
                self.music = verdict;
                emit(AudioEvent::Music { playing: verdict });
            }
        }

        // Beats: bass energy spiking above its average over the last second.
        self.since_beat += 1;
        if !self.bass_history.is_empty() {
            let mean = self.bass_history.iter().sum::<f32>() / self.bass_history.len() as f32;
            if self.music && self.since_beat >= BEAT_MIN_GAP && bass > mean * BEAT_RATIO && bass > 1e-6 {
                self.since_beat = 0;
                emit(AudioEvent::Beat {
                    strength: (bass / mean.max(1e-9)).min(10.0),
                });
            }
        }
        if self.bass_history.len() == HOPS_PER_SEC {
            self.bass_history.pop_front();
        }
        self.bass_history.push_back(bass);
    }
}

/// Runs capture on a background thread for the lifetime of the app.
/// Capture pauses while `enabled` is false and restarts after device errors.
pub fn spawn(enabled: Arc<AtomicBool>, mut emit: impl FnMut(AudioEvent) + Send + 'static) {
    thread::spawn(move || loop {
        if !enabled.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(500));
            continue;
        }
        let mut analyzer = Analyzer::default();
        let result = platform::capture(&enabled, &mut |samples| analyzer.push(samples, &mut emit));
        if analyzer.is_music() {
            emit(AudioEvent::Music { playing: false });
        }
        if let Err(e) = result {
            eprintln!("audio capture stopped: {e}");
            thread::sleep(Duration::from_secs(3));
        }
    });
}

type CaptureResult = Result<(), Box<dyn std::error::Error>>;

#[cfg(target_os = "linux")]
mod platform {
    use super::{CaptureResult, HOP, SAMPLE_RATE};
    use libpulse_binding::{def::BufferAttr, sample, stream::Direction};
    use libpulse_simple_binding::Simple;
    use std::sync::atomic::{AtomicBool, Ordering};

    pub fn capture(enabled: &AtomicBool, sink: &mut dyn FnMut(&[f32])) -> CaptureResult {
        let spec = sample::Spec {
            format: sample::Format::FLOAT32NE,
            channels: 1,
            rate: SAMPLE_RATE,
        };
        let bytes = HOP * std::mem::size_of::<f32>();
        let attr = BufferAttr {
            maxlength: u32::MAX,
            tlength: u32::MAX,
            prebuf: u32::MAX,
            minreq: u32::MAX,
            fragsize: bytes as u32,
        };
        // The monitor of the default sink carries everything that is being played.
        let stream = Simple::new(
            None,
            "Clawd",
            Direction::Record,
            Some("@DEFAULT_MONITOR@"),
            "system audio",
            &spec,
            None,
            Some(&attr),
        )?;
        let mut raw = vec![0u8; bytes];
        let mut samples = vec![0f32; HOP];
        while enabled.load(Ordering::Relaxed) {
            stream.read(&mut raw)?;
            for (out, chunk) in samples.iter_mut().zip(raw.chunks_exact(4)) {
                *out = f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            }
            sink(&samples);
        }
        Ok(())
    }
}

#[cfg(windows)]
mod platform {
    use super::{CaptureResult, SAMPLE_RATE};
    use std::{
        collections::VecDeque,
        sync::atomic::{AtomicBool, Ordering},
        thread,
        time::{Duration, Instant},
    };
    use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

    const POLL: Duration = Duration::from_millis(20);
    const DEVICE_CHECK: Duration = Duration::from_secs(3);

    pub fn capture(enabled: &AtomicBool, sink: &mut dyn FnMut(&[f32])) -> CaptureResult {
        let _ = wasapi::initialize_mta();
        let enumerator = DeviceEnumerator::new()?;
        let device = enumerator.get_default_device(&Direction::Render)?;
        let device_id = device.get_id()?;
        let mut client = device.get_iaudioclient()?;
        let format = WaveFormat::new(32, 32, &SampleType::Float, SAMPLE_RATE as usize, 2, None);
        // Opening a render device for capture gives a loopback stream of what it plays.
        client.initialize_client(
            &format,
            &Direction::Capture,
            &StreamMode::PollingShared {
                autoconvert: true,
                buffer_duration_hns: 2_000_000,
            },
        )?;
        let capture = client.get_audiocaptureclient()?;
        client.start_stream()?;

        let mut raw = VecDeque::new();
        let mut mono = Vec::new();
        let mut last_data = Instant::now();
        let mut last_check = Instant::now();
        while enabled.load(Ordering::Relaxed) {
            thread::sleep(POLL);
            while capture.get_next_packet_size()?.unwrap_or(0) > 0 {
                capture.read_from_device_to_deque(&mut raw)?;
            }
            mono.clear();
            if raw.is_empty() {
                // Loopback delivers nothing while no app is playing; that is silence.
                let gap = last_data.elapsed();
                if gap >= POLL {
                    mono.resize((gap.as_secs_f32() * SAMPLE_RATE as f32) as usize, 0.0);
                    last_data = Instant::now();
                }
            } else {
                let bytes: Vec<u8> = raw.drain(..raw.len() - raw.len() % 8).collect();
                mono.extend(bytes.chunks_exact(8).map(|f| {
                    let l = f32::from_le_bytes([f[0], f[1], f[2], f[3]]);
                    let r = f32::from_le_bytes([f[4], f[5], f[6], f[7]]);
                    (l + r) * 0.5
                }));
                last_data = Instant::now();
            }
            if !mono.is_empty() {
                sink(&mono);
            }
            // Follow the user when they switch the default output (e.g. to headphones).
            if last_check.elapsed() >= DEVICE_CHECK {
                last_check = Instant::now();
                let current = enumerator.get_default_device(&Direction::Render)?.get_id()?;
                if current != device_id {
                    break;
                }
            }
        }
        client.stop_stream()?;
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    use super::CaptureResult;
    use std::sync::atomic::AtomicBool;

    pub fn capture(_enabled: &AtomicBool, _sink: &mut dyn FnMut(&[f32])) -> CaptureResult {
        Err("system audio capture is not supported on this platform".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const SR: f32 = SAMPLE_RATE as f32;

    fn run(analyzer: &mut Analyzer, samples: &[f32]) -> Vec<AudioEvent> {
        let mut events = Vec::new();
        analyzer.push(samples, &mut |e| events.push(e));
        events
    }

    fn silence(secs: f32) -> Vec<f32> {
        vec![0.0; (secs * SR) as usize]
    }

    fn tone(secs: f32, freq: f32, amp: f32) -> Vec<f32> {
        (0..(secs * SR) as usize).map(|i| amp * (2.0 * PI * freq * i as f32 / SR).sin()).collect()
    }

    /// Deterministic pseudo-random numbers in [0, 1).
    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / (1u32 << 24) as f32
    }

    /// A sustained C major chord with a decaying 60 Hz kick on every beat.
    fn song(secs: f32, bpm: f32) -> Vec<f32> {
        let beat = 60.0 / bpm;
        (0..(secs * SR) as usize)
            .map(|i| {
                let t = i as f32 / SR;
                let chord: f32 = [261.6, 329.6, 392.0].iter().map(|f| 0.12 * (2.0 * PI * f * t).sin()).sum();
                let kick = 0.4 * (-(t % beat) * 12.0).exp() * (2.0 * PI * 60.0 * t).sin();
                chord + kick
            })
            .collect()
    }

    /// Voiced "syllables" (140 Hz harmonics) of ~180 ms, short gaps, and pauses between words.
    fn speech(secs: f32) -> Vec<f32> {
        let mut seed = 7;
        let mut out = Vec::with_capacity((secs * SR) as usize);
        while out.len() < (secs * SR) as usize {
            for _ in 0..(3 + (lcg(&mut seed) * 4.0) as usize) {
                let len = ((0.12 + lcg(&mut seed) * 0.12) * SR) as usize;
                let pitch = 120.0 + lcg(&mut seed) * 50.0;
                for i in 0..len {
                    let env = (PI * i as f32 / len as f32).sin().powi(2);
                    let t = out.len() as f32 / SR;
                    let voice: f32 = (1..10).map(|k| (2.0 * PI * pitch * k as f32 * t).sin() / k as f32).sum();
                    out.push(0.15 * env * voice);
                }
                out.extend(silence(0.04 + lcg(&mut seed) * 0.08));
            }
            out.extend(silence(0.25 + lcg(&mut seed) * 0.4));
        }
        out
    }

    fn noise(secs: f32, amp: f32) -> Vec<f32> {
        let mut seed = 1;
        (0..(secs * SR) as usize).map(|_| amp * (lcg(&mut seed) * 2.0 - 1.0)).collect()
    }

    fn music_events(events: &[AudioEvent]) -> Vec<bool> {
        events
            .iter()
            .filter_map(|e| match e {
                AudioEvent::Music { playing } => Some(*playing),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn fft_finds_a_pure_tone() {
        let mut buf: Vec<(f32, f32)> = (0..64).map(|i| ((2.0 * PI * 4.0 * i as f32 / 64.0).cos(), 0.0)).collect();
        fft(&mut buf);
        let peak = (0..32).max_by(|&a, &b| (buf[a].0.hypot(buf[a].1)).total_cmp(&buf[b].0.hypot(buf[b].1))).unwrap();
        assert_eq!(peak, 4);
    }

    #[test]
    fn silence_is_quiet() {
        assert!(run(&mut Analyzer::default(), &silence(10.0)).is_empty());
    }

    #[test]
    fn sudden_bang_is_loud_but_not_music() {
        let mut a = Analyzer::default();
        run(&mut a, &silence(3.0));
        let events = run(&mut a, &tone(0.3, 1000.0, 0.7));
        assert!(matches!(events.as_slice(), [AudioEvent::Loud { .. }]), "{events:?}");
        assert!(run(&mut a, &silence(6.0)).is_empty());
    }

    #[test]
    fn quiet_notification_is_not_loud() {
        let mut a = Analyzer::default();
        run(&mut a, &silence(3.0));
        assert!(run(&mut a, &tone(0.3, 1000.0, 0.05)).is_empty());
    }

    #[test]
    fn song_turns_music_on_with_beats_and_off_after() {
        let mut a = Analyzer::default();
        let mut events = run(&mut a, &song(9.0, 120.0));
        assert_eq!(music_events(&events), [true], "music within 9 s: {events:?}");
        events = run(&mut a, &song(5.0, 120.0));
        let beats = events.iter().filter(|e| matches!(e, AudioEvent::Beat { .. })).count();
        assert!((8..=11).contains(&beats), "beats: {beats}");
        let events = run(&mut a, &silence(5.0));
        assert_eq!(music_events(&events), [false]);
    }

    #[test]
    fn speech_is_not_music() {
        let mut a = Analyzer::default();
        let events = run(&mut a, &speech(30.0));
        assert!(music_events(&events).is_empty(), "{events:?}");
    }

    #[test]
    fn speech_does_not_keep_startling() {
        let mut a = Analyzer::default();
        run(&mut a, &silence(3.0));
        let loud: Vec<f32> = speech(30.0).iter().map(|s| s * 4.0).collect();
        let events = run(&mut a, &loud);
        let bangs = events.iter().filter(|e| matches!(e, AudioEvent::Loud { .. })).count();
        assert!(bangs <= 1, "{events:?}");
    }

    #[test]
    fn steady_noise_is_not_music() {
        let mut a = Analyzer::default();
        let events = run(&mut a, &noise(20.0, 0.2));
        assert!(music_events(&events).is_empty(), "{events:?}");
    }

    #[test]
    fn talking_after_a_song_ends_the_music() {
        let mut a = Analyzer::default();
        run(&mut a, &song(12.0, 100.0));
        assert!(a.is_music());
        let events = run(&mut a, &speech(10.0));
        assert_eq!(music_events(&events), [false]);
    }
}

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
const BACKGROUND_SECS: f32 = 3.0;
const MUSIC_ON_WINDOW: usize = 5 * HOPS_PER_SEC;
const MUSIC_ON_RATIO: f32 = 0.9;
const MUSIC_OFF_WINDOW: usize = 3 * HOPS_PER_SEC;
const MUSIC_OFF_RATIO: f32 = 0.3;
const BASS_CUTOFF_HZ: f32 = 150.0;
const BEAT_RATIO: f32 = 1.6;
const BEAT_MIN_GAP: usize = 13; // ~260 ms, i.e. up to ~230 BPM

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AudioEvent {
    Loud { db: f32 },
    Music { playing: bool },
    Beat { strength: f32 },
}

pub struct Analyzer {
    pending: Vec<f32>,
    lowpass: f32,
    lowpass_alpha: f32,
    background: f32,
    bass_history: VecDeque<f32>,
    active: VecDeque<bool>,
    music: bool,
    since_beat: usize,
    since_loud: usize,
}

impl Default for Analyzer {
    fn default() -> Self {
        let dt = 1.0 / SAMPLE_RATE as f32;
        let rc = 1.0 / (2.0 * std::f32::consts::PI * BASS_CUTOFF_HZ);
        Self {
            pending: Vec::with_capacity(HOP),
            lowpass: 0.0,
            lowpass_alpha: dt / (rc + dt),
            background: 0.0,
            bass_history: VecDeque::with_capacity(HOPS_PER_SEC),
            active: VecDeque::with_capacity(MUSIC_ON_WINDOW),
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
        if self.since_loud >= LOUD_COOLDOWN && db > LOUD_DB && db > to_db(self.background) + LOUD_JUMP_DB {
            self.since_loud = 0;
            emit(AudioEvent::Loud { db });
        }
        self.background += (rms - self.background) / (BACKGROUND_SECS * HOPS_PER_SEC as f32);

        // Music is told apart from short sounds by being (almost) continuous.
        if self.active.len() == MUSIC_ON_WINDOW {
            self.active.pop_front();
        }
        self.active.push_back(db > SILENCE_DB);
        let ratio = |window: usize| {
            let n = window.min(self.active.len());
            self.active.iter().rev().take(n).filter(|&&a| a).count() as f32 / window as f32
        };
        if !self.music && ratio(MUSIC_ON_WINDOW) >= MUSIC_ON_RATIO {
            self.music = true;
            emit(AudioEvent::Music { playing: true });
        } else if self.music && ratio(MUSIC_OFF_WINDOW) < MUSIC_OFF_RATIO {
            self.music = false;
            emit(AudioEvent::Music { playing: false });
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

    fn run(analyzer: &mut Analyzer, samples: &[f32]) -> Vec<AudioEvent> {
        let mut events = Vec::new();
        analyzer.push(samples, &mut |e| events.push(e));
        events
    }

    fn silence(secs: f32) -> Vec<f32> {
        vec![0.0; (secs * SAMPLE_RATE as f32) as usize]
    }

    fn tone(secs: f32, freq: f32, amp: f32) -> Vec<f32> {
        (0..(secs * SAMPLE_RATE as f32) as usize)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SAMPLE_RATE as f32).sin())
            .collect()
    }

    /// Quiet pad with a decaying 60 Hz kick every `60 / bpm` seconds.
    fn song(secs: f32, bpm: f32) -> Vec<f32> {
        let beat = 60.0 / bpm;
        (0..(secs * SAMPLE_RATE as f32) as usize)
            .map(|i| {
                let t = i as f32 / SAMPLE_RATE as f32;
                let since = t % beat;
                let kick = 0.8 * (-since * 25.0).exp() * (2.0 * std::f32::consts::PI * 60.0 * t).sin();
                let pad = 0.05 * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
                kick + pad
            })
            .collect()
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
        let events = run(&mut a, &song(12.0, 120.0));
        let music_on = events.iter().position(|e| *e == AudioEvent::Music { playing: true });
        let on = music_on.expect("music detected");
        let beats = events[on..].iter().filter(|e| matches!(e, AudioEvent::Beat { .. })).count();
        // ~7 seconds of music at 2 beats per second after detection.
        assert!((11..=16).contains(&beats), "beats: {beats}");
        let events = run(&mut a, &silence(4.0));
        assert!(events.contains(&AudioEvent::Music { playing: false }), "{events:?}");
    }
}

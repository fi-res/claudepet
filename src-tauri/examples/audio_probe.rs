//! Runs Clawd's audio analyzer over WAV files and prints what it hears.
//!
//!     cargo run --example audio_probe -- clip1.wav clip2.wav
//!
//! Files must be 16-bit PCM, mono, 22050 Hz, e.g. `sox in.mp3 -r 22050 -c 1 -b 16 out.wav`.

#[allow(dead_code)]
#[path = "../src/audio.rs"]
mod audio;

use audio::{Analyzer, AudioEvent, SAMPLE_RATE};

fn read_wav(path: &str) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let mut pos = 12;
    let mut format = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &bytes[pos + 8..(pos + 8 + len).min(bytes.len())];
        if id == b"fmt " {
            let channels = u16::from_le_bytes([body[2], body[3]]);
            let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            let bits = u16::from_le_bytes([body[14], body[15]]);
            format = Some((channels, rate, bits));
        } else if id == b"data" {
            match format {
                Some((1, SAMPLE_RATE, 16)) => {}
                other => return Err(format!("need mono 16-bit {SAMPLE_RATE} Hz, got {other:?}")),
            }
            return Ok(body.chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0).collect());
        }
        pos += 8 + len + (len & 1);
    }
    Err("no data chunk".into())
}

fn main() {
    for path in std::env::args().skip(1) {
        let samples = match read_wav(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        let mut analyzer = Analyzer::default();
        let mut log = Vec::new();
        let mut beats = 0;
        let mut on_since = None;
        let mut on_total = 0.0;
        for (i, chunk) in samples.chunks(SAMPLE_RATE as usize / 10).enumerate() {
            let t = i as f32 / 10.0;
            analyzer.push(chunk, &mut |e| match e {
                AudioEvent::Music { playing: true } => {
                    on_since = Some(t);
                    log.push(format!("music on @{t:.1}s"));
                }
                AudioEvent::Music { playing: false } => {
                    on_total += t - on_since.take().unwrap_or(t);
                    log.push(format!("off @{t:.1}s"));
                }
                AudioEvent::Loud { .. } => log.push(format!("loud @{t:.1}s")),
                AudioEvent::Beat { .. } => beats += 1,
            });
        }
        let duration = samples.len() as f32 / SAMPLE_RATE as f32;
        if let Some(since) = on_since {
            on_total += duration - since;
        }
        let name = std::path::Path::new(&path).file_stem().unwrap().to_string_lossy();
        println!(
            "{name:28} {duration:5.1}s  music {:3.0}%  beats {beats:3}  {}",
            100.0 * on_total / duration,
            log.join(", ")
        );
    }
}

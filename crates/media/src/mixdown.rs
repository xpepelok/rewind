use std::time::Duration;

use crate::edit::{Mix, Window};

pub const CHANNELS: usize = 2;
const FADE: Duration = Duration::from_millis(10);

fn mute_gain(at: Duration, mutes: &[Window]) -> f32 {
    let fade = FADE.as_secs_f32();
    mutes
        .iter()
        .map(|window| {
            let before = window.start.as_secs_f32() - at.as_secs_f32();
            let after = at.as_secs_f32() - window.end.as_secs_f32();
            (before.max(after) / fade).clamp(0.0, 1.0)
        })
        .fold(1.0, f32::min)
}

pub fn mixdown(
    tracks: &[Vec<f32>],
    rate: u32,
    origin: Duration,
    mix: &Mix,
    mutes: &[Window],
) -> Vec<f32> {
    let length = tracks.iter().map(Vec::len).max().unwrap_or(0);
    let gains: Vec<f32> = (0..tracks.len())
        .map(|index| mix.track_gain(index))
        .collect();
    let mut out = vec![0.0f32; length];
    for (track, gain) in tracks.iter().zip(&gains) {
        if *gain == 0.0 {
            continue;
        }
        for (sample, value) in out.iter_mut().zip(track) {
            *sample += value * gain;
        }
    }
    if !mutes.is_empty() {
        for (frame, samples) in out.chunks_mut(CHANNELS).enumerate() {
            let at = origin + Duration::from_secs_f64(frame as f64 / f64::from(rate));
            let gain = mute_gain(at, mutes);
            if gain < 1.0 {
                samples.iter_mut().for_each(|sample| *sample *= gain);
            }
        }
    }
    out.iter_mut()
        .for_each(|sample| *sample = sample.clamp(-1.0, 1.0));
    out
}

pub fn to_stereo(samples: Vec<f32>, channels: usize) -> Vec<f32> {
    match channels {
        0 => Vec::new(),
        1 => samples.iter().flat_map(|value| [*value, *value]).collect(),
        2 => samples,
        _ => samples
            .chunks_exact(channels)
            .flat_map(|frame| [frame[0], frame[1]])
            .collect(),
    }
}

pub fn resample(samples: Vec<f32>, from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 {
        return samples;
    }
    let frames = samples.len() / CHANNELS;
    let out_frames = (frames as u64 * u64::from(to) / u64::from(from)) as usize;
    let ratio = f64::from(from) / f64::from(to);
    let mut out = Vec::with_capacity(out_frames * CHANNELS);
    for index in 0..out_frames {
        let position = index as f64 * ratio;
        let base = position as usize;
        let fraction = (position - base as f64) as f32;
        let next = (base + 1).min(frames.saturating_sub(1));
        for channel in 0..CHANNELS {
            let a = samples[base * CHANNELS + channel];
            let b = samples[next * CHANNELS + channel];
            out.push(a + (b - a) * fraction);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 1000;

    #[test]
    fn tracks_are_weighted_by_their_faders() {
        let game = vec![0.5; 20];
        let mic = vec![0.25; 20];
        let mix = Mix {
            game: 0.0,
            mic: 2.0,
            ..Mix::default()
        };
        let out = mixdown(&[game, mic], RATE, Duration::ZERO, &mix, &[]);
        assert!(out.iter().all(|sample| (*sample - 0.5).abs() < 1e-6));
    }

    #[test]
    fn muted_windows_are_silent_with_soft_edges() {
        let game = vec![0.5; 2 * RATE as usize];
        let mutes = [Window {
            start: Duration::from_millis(200),
            end: Duration::from_millis(600),
        }];
        let out = mixdown(&[game], RATE, Duration::ZERO, &Mix::default(), &mutes);
        let at = |ms: usize| out[ms * CHANNELS];
        assert_eq!(at(400), 0.0);
        assert_eq!(at(100), 0.5);
        assert!(at(195) > 0.0 && at(195) < 0.5);
        assert_eq!(at(700), 0.5);
    }

    #[test]
    fn mono_and_rate_are_normalized() {
        assert_eq!(to_stereo(vec![0.1, 0.2], 1), vec![0.1, 0.1, 0.2, 0.2]);
        let doubled = resample(vec![0.0, 0.0, 1.0, 1.0], 1, 2);
        assert_eq!(doubled.len(), 8);
        assert!((doubled[2] - 0.5).abs() < 1e-6);
    }
}

const FRAME_SECONDS: f64 = 0.04;
const SEARCH_SECONDS: f64 = 0.012;
const COARSE_STEP: usize = 4;
const WEIGHT_FLOOR: f32 = 1e-3;

fn mono(input: &[f32], channels: usize) -> Vec<f32> {
    input
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

fn similarity(signal: &[f32], a: usize, b: usize, length: usize, step: usize) -> f32 {
    (0..length)
        .step_by(step)
        .map(|i| signal[a + i] * signal[b + i])
        .sum()
}

fn best_offset(
    signal: &[f32],
    nominal: usize,
    target: usize,
    overlap: usize,
    search: usize,
    limit: usize,
) -> usize {
    let low = nominal.saturating_sub(search);
    let high = (nominal + search).min(limit);
    if low >= high {
        return nominal.min(limit);
    }
    let score = |start: usize, step: usize| similarity(signal, start, target, overlap, step);
    let mut best = low;
    let mut best_score = f32::MIN;
    for start in (low..=high).step_by(COARSE_STEP / 2) {
        let value = score(start, COARSE_STEP);
        if value > best_score {
            best_score = value;
            best = start;
        }
    }
    let fine_low = best.saturating_sub(COARSE_STEP).max(low);
    let fine_high = (best + COARSE_STEP).min(high);
    best_score = f32::MIN;
    for start in fine_low..=fine_high {
        let value = score(start, 1);
        if value > best_score {
            best_score = value;
            best = start;
        }
    }
    best
}

pub fn stretch(input: Vec<f32>, channels: usize, rate: u32, speed: f32) -> Vec<f32> {
    let frames_in = input.len() / channels.max(1);
    if channels == 0 || frames_in == 0 || (speed - 1.0).abs() < f32::EPSILON {
        return input;
    }
    let speed = f64::from(speed);
    let frame = ((FRAME_SECONDS * f64::from(rate)) as usize).max(16) & !1;
    let hop = frame / 2;
    let search = (SEARCH_SECONDS * f64::from(rate)) as usize;
    let frames_out = (frames_in as f64 / speed).round() as usize;
    let signal = mono(&input, channels);
    let window: Vec<f32> = (0..frame)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / frame as f32).cos())
        .collect();
    let mut output = vec![0.0f32; (frames_out + frame) * channels];
    let mut weight = vec![0.0f32; frames_out + frame];
    let last_start = frames_in.saturating_sub(frame);
    let mut previous: Option<usize> = None;
    let mut position = 0;
    while position < frames_out {
        let nominal = ((position as f64 * speed) as usize).min(last_start);
        let start = match previous {
            Some(previous) if previous + hop + hop <= frames_in && last_start > 0 => {
                let target = (previous + hop).min(last_start);
                best_offset(
                    &signal,
                    nominal,
                    target,
                    hop.min(frames_in - target),
                    search,
                    last_start,
                )
            }
            _ => nominal,
        };
        let available = frame.min(frames_in - start);
        for i in 0..available {
            let gain = window[i];
            weight[position + i] += gain;
            for channel in 0..channels {
                output[(position + i) * channels + channel] +=
                    input[(start + i) * channels + channel] * gain;
            }
        }
        previous = Some(start);
        position += hop;
    }
    output.truncate(frames_out * channels);
    for (index, sample) in output.iter_mut().enumerate() {
        let gain = weight[index / channels];
        if gain > WEIGHT_FLOOR {
            *sample /= gain;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const TONE: f32 = 440.0;

    fn sine(seconds: f32) -> Vec<f32> {
        (0..(seconds * RATE as f32) as usize)
            .flat_map(|i| {
                let value = (std::f32::consts::TAU * TONE * i as f32 / RATE as f32).sin() * 0.5;
                [value, value]
            })
            .collect()
    }

    fn pitch(samples: &[f32]) -> f32 {
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        let skip = left.len() / 10;
        let body = &left[skip..left.len() - skip];
        let crossings = body
            .windows(2)
            .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
            .count();
        crossings as f32 / (body.len() as f32 / RATE as f32)
    }

    #[test]
    fn faster_audio_is_shorter_but_keeps_its_pitch() {
        let input = sine(2.0);
        let output = stretch(input.clone(), 2, RATE, 2.0);
        assert_eq!(output.len(), input.len() / 2);
        assert!(
            (pitch(&output) - TONE).abs() < TONE * 0.03,
            "{}",
            pitch(&output)
        );
    }

    #[test]
    fn slower_audio_is_longer_but_keeps_its_pitch() {
        let input = sine(1.0);
        let output = stretch(input.clone(), 2, RATE, 0.5);
        assert_eq!(output.len(), input.len() * 2);
        assert!(
            (pitch(&output) - TONE).abs() < TONE * 0.03,
            "{}",
            pitch(&output)
        );
    }

    #[test]
    fn normal_speed_is_untouched() {
        let input = sine(0.1);
        assert_eq!(stretch(input.clone(), 2, RATE, 1.0), input);
    }
}

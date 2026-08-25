//! Linux system-audio capture via PulseAudio/PipeWire monitor sources (`cpal`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tauri::ipc::Channel;

use super::{samples_to_bytes, to_mono_f32, Resampler};

/// Runs the capture loop until the stop flag is set.
pub(crate) fn capture_loop(channel: Channel<Vec<u8>>, stop: Arc<AtomicBool>, to_rate: u32) {
    let host = cpal::default_host();

    // Prefer a device that looks like a monitor of an output device so we
    // capture system audio rather than the microphone.
    let device = preferred_input_device(&host).or_else(|| host.default_input_device());

    let device = match device {
        Some(d) => d,
        None => {
            eprintln!("[screen-audio] no input/monitor device found");
            return;
        }
    };

    let default_config = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[screen-audio] no default input config: {e}");
            return;
        }
    };

    let device_name = device.name().unwrap_or_else(|_| "unknown".into());
    let in_channels = default_config.channels() as usize;
    let in_rate = default_config.sample_rate().0;
    eprintln!(
        "[screen-audio] capturing from: {device_name} ({} ch, {} Hz) -> {} Hz",
        in_channels, in_rate, to_rate
    );

    let resampler = Resampler::new(in_rate, to_rate);

    let stream = build_stream(&device, channel, in_channels, resampler);

    let stream = match stream {
        Some(s) => s,
        None => return,
    };

    if let Err(e) = stream.play() {
        eprintln!("[screen-audio] failed to play stream: {e}");
        return;
    }

    // Keep the stream alive until told to stop. The audio callback runs on the
    // audio thread.
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn build_stream(
    device: &cpal::Device,
    channel: Channel<Vec<u8>>,
    in_channels: usize,
    mut resampler: Resampler,
) -> Option<cpal::Stream> {
    let config = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[screen-audio] default_input_config failed: {e}");
            return None;
        }
    };
    let sample_format = config.sample_format();

    // Out buffer reused across callbacks to avoid reallocating.
    let mut out: Vec<f32> = Vec::with_capacity(4096);

    let result = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config.into(),
            move |data: &[f32], _| {
                let mono = to_mono_f32(data, in_channels);
                resampler.process(&mono, &mut out);
                let _ = channel.send(samples_to_bytes(&out));
            },
            |err| eprintln!("[screen-audio] stream error: {err}"),
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream::<i16, _, _>(
            &config.into(),
            move |data: &[i16], _| {
                let f32: Vec<f32> = data
                    .iter()
                    .map(|s| *s as f32 / i16::MAX as f32)
                    .collect();
                let mono = to_mono_f32(&f32, in_channels);
                resampler.process(&mono, &mut out);
                let _ = channel.send(samples_to_bytes(&out));
            },
            |err| eprintln!("[screen-audio] stream error: {err}"),
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream::<u16, _, _>(
            &config.into(),
            move |data: &[u16], _| {
                let f32: Vec<f32> = data
                    .iter()
                    .map(|s| (*s as i32 - 32768) as f32 / 32768.0)
                    .collect();
                let mono = to_mono_f32(&f32, in_channels);
                resampler.process(&mono, &mut out);
                let _ = channel.send(samples_to_bytes(&out));
            },
            |err| eprintln!("[screen-audio] stream error: {err}"),
            None,
        ),
        other => {
            eprintln!("[screen-audio] unsupported sample format: {other:?}");
            return None;
        }
    };

    match result {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("[screen-audio] failed to build stream: {e}");
            None
        }
    }
}

/// Returns the input device whose name suggests it is an output monitor.
fn preferred_input_device(host: &cpal::Host) -> Option<cpal::Device> {
    let devices: Vec<cpal::Device> = host.input_devices().ok()?.collect();
    if devices.is_empty() {
        return None;
    }
    for device in &devices {
        if let Ok(name) = device.name() {
            let lower = name.to_lowercase();
            if lower.contains("monitor")
                || lower.contains("output")
                || lower.contains("loopback")
            {
                return Some(device.clone());
            }
        }
    }
    devices.last().cloned()
}

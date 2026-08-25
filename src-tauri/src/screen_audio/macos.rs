//! macOS system-audio capture via ScreenCaptureKit.
//!
//! Unlike Windows (WASAPI loopback) and Linux (PulseAudio/PipeWire monitor),
//! macOS gates system-audio capture behind ScreenCaptureKit (`SCStream`), which
//! requires macOS 13+ and the **Screen Recording** permission granted to the
//! app in System Settings → Privacy & Security → Screen Recording.
//!
//! We configure an `SCStream` to capture *only* audio (no video — the screen/
//! window video already comes from `getDisplayMedia`) at 48 kHz mono, and
//! stream the float32 PCM over the binary Tauri channel.

use tauri::ipc::Channel;

use std::sync::{Arc, Mutex};

use screencapturekit::prelude::*;
use screencapturekit::stream::output_type::SCStreamOutputType;

use super::Resampler;

/// Owns a live `SCStream`. Dropping it stops the capture.
pub(crate) struct MacStream {
    stream: SCStream,
}

impl Drop for MacStream {
    fn drop(&mut self) {
        let _ = self.stream.stop_capture();
    }
}

/// SCKit capture sample rate (we request mono float32 at 48 kHz).
const SCKIT_RATE: u32 = 48_000;

/// Starts an audio-only ScreenCaptureKit stream and begins streaming mono f32
/// samples (resampled to `to_rate`) through `channel`.
pub(crate) fn start(channel: Channel<Vec<u8>>, to_rate: u32) -> Result<MacStream, String> {
    // List shareable content to discover the primary display.
    let content = SCShareableContent::get().map_err(|e| format!("SCShareableContent: {e}"))?;
    let displays = content.displays();
    let display = displays
        .first()
        .ok_or_else(|| "no display available for audio capture".to_string())?;

    // Filter on the primary display. ScreenCaptureKit requires *some* content
    // filter; audio is captured system-wide for that display's session.
    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build();

    // Audio-only stream: 48 kHz mono float32. We do not configure any video
    // dimensions, so no video frames are produced — the screen/window video
    // already comes from `getDisplayMedia`.
    let config = SCStreamConfiguration::new()
        .with_captures_audio(true)
        .with_sample_rate(48_000)
        .with_channel_count(1)
        .with_excludes_current_process_audio(true); // avoid capturing our own output

    let mut stream = SCStream::new(&filter, &config);

    // Resample from SCKit's 48 kHz to the frontend's real output rate.
    let resampler = Arc::new(Mutex::new(Resampler::new(SCKIT_RATE, to_rate)));
    let resampler_for_handler = Arc::clone(&resampler);

    stream.add_output_handler(
        move |sample: screencapturekit::cm::CMSampleBuffer, of_type| {
            if of_type != SCStreamOutputType::Audio {
                return;
            }
            // Extract the audio buffer list. SCKit delivers float32 PCM; we
            // asked for mono `with_channel_count(1)`, but defensively downmix
            // any interleaved multi-channel buffer back to mono before sending.
            if let Some(list) = sample.audio_buffer_list() {
                let mut mono: Vec<f32> = Vec::new();
                for i in 0..list.num_buffers() {
                    if let Some(b) = list.get(i) {
                        let channels = b.number_channels as usize;
                        let raw = b.data();
                        if channels == 0 || raw.is_empty() {
                            continue;
                        }
                        let byte_len = raw.len() - (raw.len() % 4);
                        let frames = byte_len / 4 / channels.max(1);
                        for f in 0..frames {
                            let mut sum = 0f32;
                            for ch in 0..channels {
                                let off = (f * channels + ch) * 4;
                                let s = f32::from_le_bytes([
                                    raw[off],
                                    raw[off + 1],
                                    raw[off + 2],
                                    raw[off + 3],
                                ]);
                                sum += s;
                            }
                            mono.push(sum / channels.max(1) as f32);
                        }
                    }
                }
                if !mono.is_empty() {
                    if let Ok(mut resampler) = resampler_for_handler.lock() {
                        let mut out: Vec<f32> = Vec::with_capacity(mono.len());
                        resampler.process(&mono, &mut out);
                        if !out.is_empty() {
                            let _ = channel.send(super::samples_to_bytes(&out));
                        }
                    }
                }
            }
        },
        SCStreamOutputType::Audio,
    );

    let _ = resampler;

    stream.start_capture().map_err(|e| format!("start_capture: {e}"))?;

    Ok(MacStream { stream })
}

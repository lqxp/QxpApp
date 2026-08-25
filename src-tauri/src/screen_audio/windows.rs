//! Windows system-audio capture via WASAPI render-client loopback.
//!
//! Loopback captures whatever the system is playing through the default
//! render device, which is exactly what "share system audio" means. The
//! microphone is a separate device and is never touched here.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::ipc::Channel;

use super::{samples_to_bytes, Resampler};

use windows::Win32::Media::Audio;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

// SubFormat `data1` values for WAVEFORMATEXTENSIBLE.
const SUBTYPE_PCM: u32 = 1;
const SUBTYPE_IEEE_FLOAT: u32 = 3;

/// The decoded sample representation we stream to the frontend (always mono f32).
enum SampleRepr {
    /// Signed 16-bit PCM.
    I16,
    /// Signed 24-bit PCM (packed, 3 bytes per sample).
    I24,
    /// Signed 32-bit integer PCM.
    I32,
    /// 32-bit IEEE float.
    F32,
    /// An unsupported format — drained and dropped (yields silence).
    Unsupported,
}

/// Runs the capture loop until the stop flag is set.
pub(crate) fn capture_loop(channel: Channel<Vec<u8>>, stop: Arc<AtomicBool>, to_rate: u32) {
    // Initialize COM on this thread (required for MMDeviceEnumerator).
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let capture = Capture::new();

    let capture = match capture {
        Some(c) => c,
        None => {
            eprintln!("[screen-audio] failed to initialize WASAPI loopback");
            unsafe {
                CoUninitialize();
            }
            return;
        }
    };

    let sample_rate = capture.sample_rate();
    eprintln!(
        "[screen-audio] capturing system mix via WASAPI loopback ({} Hz -> {} Hz)",
        sample_rate, to_rate
    );

    let mut resampler = Resampler::new(sample_rate, to_rate);
    let mut out: Vec<f32> = Vec::with_capacity(4096);
    let mut resampled: Vec<f32> = Vec::with_capacity(4096);

    while !stop.load(Ordering::SeqCst) {
        capture.read_frames(&mut out);
        if !out.is_empty() {
            resampler.process(&out, &mut resampled);
            if !resampled.is_empty() {
                let _ = channel.send(samples_to_bytes(&resampled));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    unsafe {
        CoUninitialize();
    }
}

/// Owns a WASAPI loopback client.
struct Capture {
    _client: Audio::IAudioClient,
    _capture_client: Audio::IAudioCaptureClient,
    format: *mut Audio::WAVEFORMATEX,
    repr: SampleRepr,
    channels: usize,
    sample_rate: u32,
}

impl Capture {
    fn new() -> Option<Self> {
        unsafe {
            let enumerator: Audio::IMMDeviceEnumerator =
                CoCreateInstance(&Audio::MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device: Audio::IMMDevice = enumerator
                .GetDefaultAudioEndpoint(Audio::eRender, Audio::eConsole)
                .ok()?;

            let client: Audio::IAudioClient = device.Activate(CLSCTX_ALL, None).ok()?;

            // Ask WASAPI for the mix format (device's native sample format).
            let wave_format_ptr: *mut Audio::WAVEFORMATEX = client.GetMixFormat().ok()?;

            // Decode the format representation and channel count up front.
            let (repr, channels) = decode_format(wave_format_ptr);
            let sample_rate = (*wave_format_ptr).nSamplesPerSec;

            // ~100ms of audio.
            const REFTIMES_PER_MILLISEC: i64 = 10_000;
            let buffer_duration = 100 * REFTIMES_PER_MILLISEC;

            if let Err(e) = client.Initialize(
                Audio::AUDCLNT_SHAREMODE_SHARED,
                Audio::AUDCLNT_STREAMFLAGS_LOOPBACK,
                buffer_duration,
                0,
                wave_format_ptr,
                None,
            ) {
                eprintln!("[screen-audio] Initialize failed: {e}");
                CoTaskMemFree(Some(wave_format_ptr as *const core::ffi::c_void));
                return None;
            }

            let capture_client: Audio::IAudioCaptureClient = match client.GetService() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[screen-audio] GetService(IAudioCaptureClient) failed: {e}");
                    CoTaskMemFree(Some(wave_format_ptr as *const core::ffi::c_void));
                    return None;
                }
            };

            Some(Self {
                _client: client,
                _capture_client: capture_client,
                format: wave_format_ptr,
                repr,
                channels,
                sample_rate,
            })
        }
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Reads pending frames, appending mono `f32` samples to `out`.
    fn read_frames(&self, out: &mut Vec<f32>) {
        out.clear();

        let channels = self.channels;

        unsafe {
            let wave_format = &*self.format;
            let bits = wave_format.wBitsPerSample as usize;
            let bytes_per_sample = (bits + 7) / 8;

            loop {
                let packet_length = match self._capture_client.GetNextPacketSize() {
                    Ok(n) => n,
                    Err(_) => break,
                };
                if packet_length == 0 {
                    break;
                }

                let mut data_ptr: *mut u8 = std::ptr::null_mut();
                let mut num_frames = 0u32;
                let mut flags = 0u32;

                if self
                    ._capture_client
                    .GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)
                    .is_err()
                {
                    break;
                }

                let frame_bytes = channels * bytes_per_sample;
                let total_bytes = (num_frames as usize) * frame_bytes;
                let data = std::slice::from_raw_parts(data_ptr, total_bytes);

                match self.repr {
                    SampleRepr::F32 => {
                        for frame in data.chunks_exact(frame_bytes) {
                            let mut sum = 0f32;
                            for ch in 0..channels {
                                let offset = ch * bytes_per_sample;
                                let sample = f32::from_le_bytes([
                                    frame[offset],
                                    frame[offset + 1],
                                    frame[offset + 2],
                                    frame[offset + 3],
                                ]);
                                sum += sample;
                            }
                            out.push(sum / channels as f32);
                        }
                    }
                    SampleRepr::I16 => {
                        for frame in data.chunks_exact(frame_bytes) {
                            let mut sum = 0f32;
                            for ch in 0..channels {
                                let offset = ch * bytes_per_sample;
                                let sample =
                                    i16::from_le_bytes([frame[offset], frame[offset + 1]]);
                                sum += sample as f32 / i16::MAX as f32;
                            }
                            out.push(sum / channels as f32);
                        }
                    }
                    SampleRepr::I24 => {
                        for frame in data.chunks_exact(frame_bytes) {
                            let mut sum = 0f32;
                            for ch in 0..channels {
                                let offset = ch * bytes_per_sample;
                                // Sign-extend 24-bit little-endian to i32.
                                let b0 = frame[offset] as u32;
                                let b1 = frame[offset + 1] as u32;
                                let b2 = frame[offset + 2] as u32;
                                let mut v = (b0 | (b1 << 8) | (b2 << 16)) as i32;
                                if v & 0x0080_0000 != 0 {
                                    v |= !0x00FF_FFFF;
                                }
                                sum += v as f32 / 8_388_607.0;
                            }
                            out.push(sum / channels as f32);
                        }
                    }
                    SampleRepr::I32 => {
                        for frame in data.chunks_exact(frame_bytes) {
                            let mut sum = 0f32;
                            for ch in 0..channels {
                                let offset = ch * bytes_per_sample;
                                let sample = i32::from_le_bytes([
                                    frame[offset],
                                    frame[offset + 1],
                                    frame[offset + 2],
                                    frame[offset + 3],
                                ]);
                                sum += sample as f32 / i32::MAX as f32;
                            }
                            out.push(sum / channels as f32);
                        }
                    }
                    SampleRepr::Unsupported => {
                        // Drained and dropped (silence).
                    }
                }

                let _ = self._capture_client.ReleaseBuffer(num_frames);
            }
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            if !self.format.is_null() {
                CoTaskMemFree(Some(self.format as *const core::ffi::c_void));
            }
        }
    }
}

/// Determines the sample representation and channel count from a WASAPI mix
/// format pointer (which is either a `WAVEFORMATEX` or, when the tag is
/// `WAVE_FORMAT_EXTENSIBLE`, a `WAVEFORMATEXTENSIBLE`).
unsafe fn decode_format(ptr: *mut Audio::WAVEFORMATEX) -> (SampleRepr, usize) {
    if ptr.is_null() {
        return (SampleRepr::Unsupported, 0);
    }
    let wave = &*ptr;
    let channels = wave.nChannels as usize;
    let bits = wave.wBitsPerSample as usize;

    let tag = wave.wFormatTag;
    match tag {
        WAVE_FORMAT_PCM => (pcm_repr(bits), channels),
        WAVE_FORMAT_IEEE_FLOAT => {
            if bits == 32 {
                (SampleRepr::F32, channels)
            } else {
                (SampleRepr::Unsupported, channels)
            }
        }
        WAVE_FORMAT_EXTENSIBLE => {
            // Interpret the trailing portion as WAVEFORMATEXTENSIBLE to read
            // the SubFormat GUID, whose first dword identifies the data type.
            let ext = &*(ptr as *const Audio::WAVEFORMATEXTENSIBLE);
            match ext.SubFormat.data1 {
                SUBTYPE_IEEE_FLOAT => {
                    if bits == 32 {
                        (SampleRepr::F32, channels)
                    } else {
                        (SampleRepr::Unsupported, channels)
                    }
                }
                SUBTYPE_PCM => (pcm_repr(bits), channels),
                _ => (SampleRepr::Unsupported, channels),
            }
        }
        _ => (SampleRepr::Unsupported, channels),
    }
}

fn pcm_repr(bits: usize) -> SampleRepr {
    match bits {
        16 => SampleRepr::I16,
        24 => SampleRepr::I24,
        32 => SampleRepr::I32,
        _ => SampleRepr::Unsupported,
    }
}

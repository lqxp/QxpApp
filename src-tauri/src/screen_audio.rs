//! Native system-audio capture for screen sharing.
//!
//! `getDisplayMedia({ audio: true })` does **not** provide system audio in the
//! WebViews Tauri uses (WebView2 on Windows, WKWebView on macOS, WebKitGTK on
//! Linux). To capture the sound the user's computer is actually playing, we
//! have to reach down to the OS audio layer ourselves:
//!
//! - **Windows**: WASAPI render-client loopback (the system mix is captured as
//!   if it were an input device).
//! - **Linux**: a PulseAudio/PipeWire "monitor" source via `cpal`.
//! - **macOS**: ScreenCaptureKit (`SCStream`), available macOS 13+.
//!
//! Frames are downmixed to mono `f32` and streamed to the frontend over a raw
//! binary Tauri `Channel<Vec<u8>>` (little-endian f32 samples). The frontend
//! feeds those bytes into an `AudioWorklet` backed by a
//! `MediaStreamAudioDestinationNode`, producing a real `MediaStreamTrack` that
//! the WebRTC layer can negotiate like any other audio track.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{
    ipc::Channel,
    plugin::{Builder, TauriPlugin},
    Manager, Runtime, State,
};

/// A single active capture. The platform modules drive it via `Drop`, or an
/// explicit stop that signals the worker (threads on Windows/Linux; an
/// `SCStream` on macOS).
enum CaptureHandle {
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "windows"
    ))]
    Thread {
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    },

    #[cfg(target_os = "macos")]
    Stream(macos::MacStream),
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        match self {
            #[cfg(any(
                target_os = "linux",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd",
                target_os = "windows"
            ))]
            CaptureHandle::Thread { stop, thread } => {
                stop.store(true, Ordering::SeqCst);
                if let Some(t) = thread.take() {
                    let _ = t.join();
                }
            }

            #[cfg(target_os = "macos")]
            CaptureHandle::Stream(_) => {
                // `MacStream` stops itself on drop.
            }
        }
    }
}

/// Shared, managed state holding the single active capture (if any).
struct CaptureRegistry {
    handle: Mutex<Option<CaptureHandle>>,
}

impl CaptureRegistry {
    fn new() -> Self {
        Self {
            handle: Mutex::new(None),
        }
    }
}

/// Starts a system-audio capture, streaming little-endian mono `f32` samples
/// through `channel`.
///
/// Returns `Ok(())` when a capture was started, or an error describing why
/// system audio is unavailable (so the caller can fall back to video-only
/// sharing without breaking it).
#[tauri::command]
async fn start<R: Runtime>(
    app: tauri::AppHandle<R>,
    channel: Channel<Vec<u8>>,
    sample_rate: u32,
    state: State<'_, CaptureRegistry>,
) -> Result<(), String> {
    let _ = app;
    // Stop any previous capture first.
    stop_inner(&state);

    // Sanitize the requested output rate (the frontend's real AudioContext
    // sample rate). Fall back to 48 kHz when out of range.
    let to_rate = if (8000..=192_000).contains(&sample_rate) {
        sample_rate
    } else {
        TARGET_SAMPLE_RATE
    };

    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_channel = channel.clone();

        let thread = std::thread::Builder::new()
            .name("qxchat-screen-audio".into())
            .spawn(move || {
                linux::capture_loop(thread_channel, thread_stop, to_rate);
            })
            .map_err(|e| format!("failed to spawn capture thread: {e}"))?;

        let mut guard = state.handle.lock().unwrap();
        *guard = Some(CaptureHandle::Thread {
            stop,
            thread: Some(thread),
        });
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_channel = channel.clone();

        let thread = std::thread::Builder::new()
            .name("qxchat-screen-audio".into())
            .spawn(move || {
                windows::capture_loop(thread_channel, thread_stop, to_rate);
            })
            .map_err(|e| format!("failed to spawn capture thread: {e}"))?;

        let mut guard = state.handle.lock().unwrap();
        *guard = Some(CaptureHandle::Thread {
            stop,
            thread: Some(thread),
        });
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        let stream = macos::start(channel, to_rate)?;
        let mut guard = state.handle.lock().unwrap();
        *guard = Some(CaptureHandle::Stream(stream));
        return Ok(());
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "windows",
        target_os = "macos"
    )))]
    {
        let _ = channel;
        Err("System audio capture is not supported on this platform.".into())
    }
}

/// Stops any active capture.
#[tauri::command]
async fn stop<R: Runtime>(app: tauri::AppHandle<R>, state: State<'_, CaptureRegistry>) -> Result<(), String> {
    let _ = app;
    stop_inner(&state);
    Ok(())
}

fn stop_inner(state: &CaptureRegistry) {
    let mut guard = state.handle.lock().unwrap();
    *guard = None;
}

/// Initializes the screen-audio plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("screen-audio")
        .invoke_handler(tauri::generate_handler![start, stop])
        .setup(|app, _api| {
            app.manage(CaptureRegistry::new());
            Ok(())
        })
        .build()
}

/// Serializes mono `f32` samples into little-endian bytes.
fn samples_to_bytes(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    bytes
}

/// Downmixes interleaved `channels`-channel `f32` data to mono. Kept simple and
/// allocation-friendly so it runs in the capture thread without glitches.
#[allow(dead_code)]
fn to_mono_f32(input: &[f32], in_channels: usize) -> Vec<f32> {
    if in_channels == 0 {
        return Vec::new();
    }
    let mut mono: Vec<f32> = Vec::with_capacity(input.len() / in_channels);
    for frame in input.chunks_exact(in_channels) {
        let sum: f32 = frame.iter().sum();
        mono.push(sum / in_channels as f32);
    }
    mono
}

/// Target sample rate the frontend always expects.
pub const TARGET_SAMPLE_RATE: u32 = 48_000;

/// High-quality streaming resampler using Catmull-Rom cubic interpolation.
///
/// Continuously resamples a stream of mono `f32` samples from `from_rate` to
/// `to_rate`. It keeps a short history buffer across `process` calls so the
/// interpolation phase is exact and there is no click/discontinuity at chunk
/// boundaries, and it keeps the fractional position precisely to avoid any
/// pitch drift.
#[allow(dead_code)]
pub struct Resampler {
    from_rate: u32,
    to_rate: u32,
    /// Pending source samples not yet emitted (kept small). `start` is the
    /// offset of the first valid sample; everything below it is consumed.
    buf: Vec<f32>,
    start: usize,
    /// Fractional read position within `buf` (source-sample units), relative to
    /// an absolute stream position.
    pos: f64,
}

#[allow(dead_code)]
impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        Self {
            from_rate,
            to_rate: if to_rate == 0 { TARGET_SAMPLE_RATE } else { to_rate },
            buf: Vec::new(),
            start: 0,
            pos: 0.0,
        }
    }

    /// Appends `mono` (at `from_rate`) and emits samples (at `to_rate`) into `out`.
    pub fn process(&mut self, mono: &[f32], out: &mut Vec<f32>) {
        out.clear();
        if mono.is_empty() {
            return;
        }

        if self.from_rate == self.to_rate {
            out.extend_from_slice(mono);
            return;
        }

        let ratio = self.from_rate as f64 / self.to_rate as f64;

        // Compact the buffer once its consumed prefix grows large, to bound
        // memory and keep `start` small.
        self.compact();
        self.buf.extend_from_slice(mono);

        // Produce output samples while we have enough source samples ahead
        // (Catmull-Rom needs the current sample plus one on each side).
        loop {
            let active = self.active();
            if active.len() < 4 {
                break;
            }
            let needed = self.pos.floor() as usize + 3;
            if needed >= active.len() {
                break;
            }

            out.push(cubic_at(active, self.pos));
            self.pos += ratio;

            // Advance the window as whole source samples are consumed.
            while self.pos >= 1.0 && self.start + 1 < self.buf.len() {
                self.start += 1;
                self.pos -= 1.0;
            }
        }
    }

    fn active(&self) -> &[f32] {
        &self.buf[self.start..]
    }

    fn compact(&mut self) {
        if self.start > 0 && self.start * 2 >= self.buf.len() {
            self.buf.drain(..self.start);
            self.start = 0;
        }
    }
}

/// Catmull-Rom cubic interpolation at fractional position `t` (0..1) within
/// `i1`. Falls back to linear interpolation near the stream edges.
fn cubic_at(buf: &[f32], pos: f64) -> f32 {
    let i1 = pos.floor() as usize;
    let t = (pos - i1 as f64) as f32;

    if i1 == 0 {
        // Not enough left neighbours — linear between index 0 and 1.
        let a = buf[0];
        let b = buf[1];
        return a + (b - a) * t;
    }
    if i1 + 2 >= buf.len() {
        // Not enough right neighbours — linear between last two.
        let n = buf.len();
        let a = buf[n - 2];
        let b = buf[n - 1];
        return a + (b - a) * t;
    }

    let x0 = buf[i1 - 1];
    let x1 = buf[i1];
    let x2 = buf[i1 + 1];
    let x3 = buf[i1 + 2];

    let t2 = t * t;
    let t3 = t2 * t;
    0.5
        * (2.0 * x1
            + (-x0 + x2) * t
            + (2.0 * x0 - 5.0 * x1 + 4.0 * x2 - x3) * t2
            + (-x0 + 3.0 * x1 - 3.0 * x2 + x3) * t3)
}

#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
mod linux;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
mod macos;

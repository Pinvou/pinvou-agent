//! Wayland same-session screen capture: the ScreenCast stream (PipeWire) bound to the portal
//! session.
//!
//! The RemoteDesktop session's Start response already carries the ScreenCast stream's PipeWire
//! node id (the absolute-motion coordinate reference); `ScreenCast.OpenPipewireRemote` trades
//! the same session for a private PipeWire connection fd. This module hands that fd to a
//! dedicated PipeWire thread: it subscribes to the node's video stream, converts arriving
//! frames to RGBA, and shares them with the negotiated size, so `capture()` can take frames
//! from the same session — capture no longer goes through xcap's GNOME-Shell/portal
//! Screenshot/wlroots chains (the legacy chain could pop an authorization dialog per capture).
//!
//! Coordinate semantics (see mutter `meta_screen_cast_monitor_stream`'s `transform_position`
//! and KDE's portal-layer implementation that adds the stream origin): input injection
//! coordinates are "stream-local pixels", the global offset being handled inside the
//! compositor/portal layer, so the caller's origin is always (0,0); at scale≠1 KDE's input
//! unit is stream-local logical pixels (its buffer is physical pixels and needs dividing by
//! the scale), while mutter's input unit is buffer pixels — the caller picks the factor per
//! desktop environment.
//!
//! Threading contract: objects are constructed on the computer_use dedicated worker thread;
//! the PipeWire main loop runs on its own thread, shared state exchanged only via
//! `Arc<Mutex>`; `shutdown` stops the stream via the pw channel and joins the thread.

use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pipewire::{
    channel,
    context::ContextRc,
    keys::{MEDIA_CATEGORY, MEDIA_ROLE, MEDIA_TYPE},
    main_loop::MainLoopRc,
    properties,
    spa::{
        param::{
            ParamType,
            format::{FormatProperties, MediaSubtype, MediaType},
            format_utils,
            video::{VideoFormat, VideoInfoRaw},
        },
        pod::{self, Pod, serialize::PodSerializer},
        utils::{Direction, Fraction, Rectangle, SpaTypes},
    },
    stream::{StreamFlags, StreamRc},
};

use super::super::types::ComputerUseError;

/// Budget for waiting for the first frame (format negotiation + first-frame composition).
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(4);
/// Grace period after an input action while waiting for a frame newer than it (with no
/// visual change no new frame arrives; on timeout use the existing frame — content
/// unchanged, the frame is still correct).
const FRESH_FRAME_GRACE: Duration = Duration::from_millis(400);
/// Poll step while waiting for a frame.
const POLL_STEP: Duration = Duration::from_millis(20);

/// A captured frame that is ready (stream-local pixels).
#[derive(Debug, Clone)]
pub(super) struct PortalFrame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub received_at: Instant,
}

#[derive(Default)]
struct SharedCapture {
    frame: Option<PortalFrame>,
    /// Total frames received (diagnostics: distinguishes "stream stopped pushing" from
    /// "the caller did not take").
    frames_received: u64,
    /// Buffer size negotiated by PipeWire (stream-local pixels).
    negotiated: Option<(u32, u32)>,
    /// Latest stream state/error (for diagnostics).
    last_state: Option<String>,
    stopped: bool,
}

/// PipeWire capture receiver: owns the dedicated thread and the shared frame buffer.
pub(super) struct PwCapture {
    shared: Arc<Mutex<SharedCapture>>,
    control: channel::Sender<bool>,
    worker: Option<JoinHandle<()>>,
}

impl PwCapture {
    /// Establish the same-session capture stream from the portal `OpenPipewireRemote` fd.
    /// `node_id` is the PipeWire node id from the Start response's streams.
    pub(super) fn spawn(node_id: u32, fd: OwnedFd) -> Result<Self, ComputerUseError> {
        let shared = Arc::new(Mutex::new(SharedCapture::default()));
        let (control, control_rx) = channel::channel::<bool>();

        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("pinvou-cu-pipewire".to_string())
            .spawn(move || {
                if let Err(error) = run_pw_loop(node_id, fd, &worker_shared, control_rx) {
                    if let Ok(mut shared) = worker_shared.lock() {
                        shared.last_state = Some(format!("pipewire thread exited: {error}"));
                    }
                }
            })
            .map_err(|error| {
                ComputerUseError::unavailable(format!("cannot spawn pipewire thread: {error}"))
            })?;

        Ok(Self {
            shared,
            control,
            worker: Some(worker),
        })
    }

    /// Take the latest frame. When `not_before` is `Some` (an input action just completed),
    /// first wait for a frame newer than it, falling back to the existing frame once the
    /// grace period expires (no visual change means the existing frame IS the current
    /// picture); with no frames at all, wait within the first-frame budget.
    pub(super) fn latest_frame(
        &self,
        not_before: Option<Instant>,
    ) -> Result<PortalFrame, ComputerUseError> {
        let first_deadline = Instant::now() + FIRST_FRAME_TIMEOUT;
        let grace_deadline = not_before.map(|mark| mark + FRESH_FRAME_GRACE);
        loop {
            let shared = self
                .shared
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(frame) = shared.frame.as_ref() {
                let fresh_enough = not_before.map_or(true, |mark| frame.received_at >= mark);
                if fresh_enough {
                    return Ok(frame.clone());
                }
                // A frame exists but is not newer than the action: wait the grace window,
                // then use it.
                if let Some(deadline) = grace_deadline {
                    if Instant::now() > deadline {
                        return Ok(frame.clone());
                    }
                }
            } else {
                if Instant::now() > first_deadline {
                    let state = shared
                        .last_state
                        .clone()
                        .unwrap_or_else(|| "no state reported".to_string());
                    return Err(ComputerUseError::unavailable(format!(
                        "the same-session screencast stream produced no frame within \
                         {FIRST_FRAME_TIMEOUT:?} (frames so far: {}, {state})",
                        shared.frames_received
                    )));
                }
            }
            drop(shared);
            std::thread::sleep(POLL_STEP);
        }
    }

    /// Stop the stream (called on backend drop/session recycling; idempotent).
    ///
    /// Drop the JoinHandle so the thread detaches instead of joining (round-12 review M4):
    /// on the normal path control(false) makes the main loop quit; but if the PipeWire
    /// thread is wedged in the connection phase before control_rx attach (the peer
    /// portal/pipewire is dead), it will never see the quit signal — joining on the worker
    /// thread would pin the whole session backend forever (every later request, including
    /// emergency cleanup, would time out; only an app restart would recover). The cost of
    /// detach is at most one wedged lingering thread (the same trade as the capture probe's
    /// abandoned thread, and bounded) in exchange for a shutdown that never blocks.
    pub(super) fn shutdown(&mut self) {
        self.shared
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stopped = true;
        let _ = self.control.send(false);
        if let Some(worker) = self.worker.take() {
            drop(worker);
        }
    }
}

impl Drop for PwCapture {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The PipeWire thread body: portal fd → context → stream (frames shared).
/// Receiving `false` on `control_rx` stops the stream and exits the main loop.
fn run_pw_loop(
    node_id: u32,
    fd: OwnedFd,
    shared: &Arc<Mutex<SharedCapture>>,
    control_rx: channel::Receiver<bool>,
) -> Result<(), String> {
    pipewire::init();

    let main_loop = MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = ContextRc::new(&main_loop, None).map_err(|e| e.to_string())?;
    // The session-private PipeWire remote: only this portal session's stream objects are exposed.
    let core = context
        .connect_fd_rc(fd, None)
        .map_err(|e| format!("cannot connect to the portal pipewire remote: {e}"))?;

    let stream = StreamRc::new(
        core,
        "pinvou-computer-use",
        properties::properties! {
            *MEDIA_TYPE => "Video",
            *MEDIA_CATEGORY => "Capture",
            *MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| e.to_string())?;

    let listener_shared = Arc::clone(shared);
    let _listener = stream
        .add_local_listener_with_user_data(listener_shared)
        .state_changed(|_, shared, _old, new| {
            if let Ok(mut shared) = shared.lock() {
                shared.last_state = Some(format!("stream state {new:?}"));
            }
        })
        .param_changed(|_, shared, id, param| {
            let Some(param) = param else {
                return;
            };
            if id != ParamType::Format.as_raw() {
                return;
            }
            let parsed = format_utils::parse_format(param)
                .map_err(|e| e.to_string())
                .and_then(|(media, subtype)| {
                    if media == MediaType::Video && subtype == MediaSubtype::Raw {
                        Ok(())
                    } else {
                        Err(format!(
                            "negotiated format is {media:?}/{subtype:?}, not raw video"
                        ))
                    }
                })
                .and_then(|()| {
                    let mut info = VideoInfoRaw::new();
                    info.parse(param)
                        .map(|_| info)
                        .map_err(|e| format!("cannot parse negotiated video format: {e}"))
                })
                .and_then(|info| {
                    // Defensive validation: although the offer pins the format to BGRx, an
                    // anomalous compositor fixating another format would make convert_frame's
                    // channel swap silently output wrong-colored frames — reject outright
                    // during negotiation (fail-closed).
                    if info.format() == VideoFormat::BGRx {
                        Ok(info)
                    } else {
                        Err(format!(
                            "negotiated pixel format is {:?}, expected BGRx",
                            info.format()
                        ))
                    }
                });
            if let Ok(mut shared) = shared.lock() {
                match parsed {
                    Ok(info) => {
                        let size = info.size();
                        shared.negotiated = Some((size.width, size.height));
                    }
                    Err(error) => shared.last_state = Some(error),
                }
            }
        })
        .process(move |stream, shared| {
            let Ok(mut shared) = shared.lock() else {
                return;
            };
            if shared.stopped {
                return;
            }
            let Some(negotiated) = shared.negotiated else {
                return;
            };
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let (width, height) = negotiated;
            let data = &mut datas[0];
            let (offset, chunk_stride) = {
                let chunk = data.chunk();
                (chunk.offset() as usize, chunk.stride())
            };
            // A non-positive stride cannot address buffer rows, and casting
            // a negative i32 to usize would wrap into a huge value (debug
            // overflow panic, release slice panic): record the failure the
            // same way the BGRx rejection reports and skip the frame.
            let stride = match usize::try_from(chunk_stride) {
                Ok(stride) if stride > 0 => stride,
                _ => {
                    shared.last_state =
                        Some(format!("frame has non-positive stride {chunk_stride}"));
                    return;
                }
            };
            let Some(raw) = data.data() else {
                return;
            };
            if offset >= raw.len() {
                return;
            }
            let end = (offset + stride * height as usize).min(raw.len());
            match convert_frame(&raw[offset..end], stride, width, height) {
                Some(rgba) => {
                    shared.frames_received += 1;
                    shared.frame = Some(PortalFrame {
                        rgba,
                        width,
                        height,
                        received_at: Instant::now(),
                    });
                }
                None => {
                    shared.last_state =
                        Some("frame buffer is smaller than the negotiated geometry".to_string());
                }
            }
        })
        .register()
        .map_err(|e| e.to_string())?;

    // Allowed format: fixed BGRx (the shm buffer format of the three major compositors'
    // portal implementations, see mutter screen-cast / kwin screencastbuffer /
    // xdg-desktop-portal-wlr); size and framerate are given as ranges for the compositor to
    // fixate (the negotiated result is the capture resolution and also the input coordinate
    // space of stream-local pixels).
    let obj = pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pod::property!(FormatProperties::VideoFormat, Id, VideoFormat::BGRx),
        pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: 1920,
                height: 1080
            },
            Rectangle {
                width: 1,
                height: 1
            },
            Rectangle {
                width: 16384,
                height: 16384
            }
        ),
        pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: 60, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: 60, denom: 1 }
        ),
    );
    let values =
        PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &pod::Value::Object(obj))
            .map_err(|e| format!("cannot serialize format params: {e}"))?
            .0
            .into_inner();
    let mut params =
        [Pod::from_bytes(&values)
            .ok_or_else(|| "serialized format params are empty".to_string())?];

    stream
        .connect(
            Direction::Input,
            Some(node_id),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| format!("cannot connect the capture stream to node {node_id}: {e}"))?;
    // pw_stream defaults to Inactive: the compositor only starts pushing frames after an
    // explicit activation.
    stream
        .set_active(true)
        .map_err(|e| format!("cannot activate the capture stream: {e}"))?;

    // Stop switch: receiving false stops the stream and exits (backend drop/close path).
    let quit_loop = main_loop.clone();
    let shutdown_shared = Arc::clone(shared);
    let _attached = control_rx.attach(main_loop.loop_(), move |active| {
        if let Err(error) = stream.set_active(active) {
            if let Ok(mut shared) = shutdown_shared.lock() {
                shared.last_state = Some(format!("cannot set stream active={active}: {error}"));
            }
        }
        if !active {
            quit_loop.quit();
        }
    });

    main_loop.run();
    Ok(())
}

/// BGRx ([b,g,r,x]) → RGBA frame copy, honoring stride. Pure function, easy to unit test.
fn convert_frame(raw: &[u8], stride: usize, width: u32, height: u32) -> Option<Vec<u8>> {
    let width = width as usize;
    let height = height as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let row = width * 4;
    if stride < row || raw.len() < stride * (height - 1) + row {
        return None;
    }
    let mut rgba = vec![0u8; row * height];
    for (y, out_row) in rgba.chunks_mut(row).enumerate() {
        let src = &raw[y * stride..y * stride + row];
        for (px, out) in src.chunks_exact(4).zip(out_row.chunks_exact_mut(4)) {
            out[0] = px[2];
            out[1] = px[1];
            out[2] = px[0];
            out[3] = 255;
        }
    }
    Some(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_frame_handles_stride_padding_and_swizzle() {
        // Width 2, height 2, stride 12 (4 bytes of padding per row); BGRx input → RGBA output.
        let mut raw = vec![0u8; 12 * 2];
        // First row, pixel 0: B G R x → expected R G B A.
        raw[0..4].copy_from_slice(&[10, 20, 30, 255]);
        // First row, pixel 1.
        raw[4..8].copy_from_slice(&[40, 50, 60, 0]);
        // Second row, pixel 0 (after the padding).
        raw[12..16].copy_from_slice(&[1, 2, 3, 7]);
        let out = convert_frame(&raw, 12, 2, 2).expect("convert");
        assert_eq!(out.len(), 16);
        assert_eq!(&out[0..4], &[30, 20, 10, 255]);
        assert_eq!(&out[4..8], &[60, 50, 40, 255]);
        assert_eq!(&out[8..12], &[3, 2, 1, 255]);
        assert_eq!(&out[12..16], &[0, 0, 0, 255]);
    }

    #[test]
    fn convert_frame_rejects_short_or_mismatched_buffers() {
        assert!(convert_frame(&[], 8, 1, 1).is_none());
        // stride < row (row = 12 > stride 8 at width 3).
        assert!(convert_frame(&[0u8; 24], 8, 3, 1).is_none());
        // Last row incomplete (len 7 < stride*(h-1)+row = 12).
        assert!(convert_frame(&[0u8; 7], 8, 1, 2).is_none());
        assert!(convert_frame(&[0u8; 8], 8, 0, 1).is_none(), "zero width");
        assert!(convert_frame(&[0u8; 8], 8, 1, 0).is_none(), "zero height");
    }

    #[test]
    fn convert_frame_requires_full_final_row() {
        // Last row incomplete: reject the whole frame rather than silently dropping the
        // trailing row and emitting a broken image.
        let raw = vec![0u8; 8]; // 1 complete row, 2 declared.
        assert!(convert_frame(&raw, 8, 1, 2).is_none());
    }
}

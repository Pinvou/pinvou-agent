//! Wayland 同会话截屏:portal 会话绑定的 ScreenCast 流(PipeWire)。
//!
//! RemoteDesktop 会话在 Start 响应里已经带了 ScreenCast stream 的 PipeWire
//! node id(绝对移动坐标参照);`ScreenCast.OpenPipewireRemote` 用同一会话
//! 换取一条 PipeWire 私有连接 fd。本模块把这条 fd 交给一个专用 PipeWire
//! 线程:订阅该 node 的视频流,把到达帧转成 RGBA 与协商尺寸共享出来,
//! `capture()` 即可从同会话取帧——截屏不再走 xcap 的 GNOME-Shell/portal
//! Screenshot/wlroots 链(旧链可能每次截屏弹授权对话框)。
//!
//! 坐标语义(见 mutter `meta_screen_cast_monitor_stream` 的
//! `transform_position` 与 KDE portal 层补流原点的实现):输入注入坐标是
//! 「流本地像素」,全局偏移由合成器/portal 层内部处理,故调用方 origin
//! 恒为 (0,0);scale≠1 时 KDE 的输入单位是流本地逻辑像素(其缓冲为
//! 物理像素,需除以缩放),mutter 的输入单位是缓冲像素,由调用方按桌面
//! 环境选择倍率。
//!
//! 线程约定:对象在 computer_use 专用 worker 线程构造;PipeWire 主循环在
//! 自有线程运行,共享状态只经 `Arc<Mutex>` 交换;`shutdown` 经 pw channel
//! 停流并 join 线程。

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

/// 等待首帧的预算(格式协商 + 首帧合成)。
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(4);
/// 输入动作后等待比动作新的一帧的宽限(无视觉变化时不会有新帧,超时即用
/// 现有帧——内容未变,帧仍然正确)。
const FRESH_FRAME_GRACE: Duration = Duration::from_millis(400);
/// 等帧轮询步长。
const POLL_STEP: Duration = Duration::from_millis(20);

/// 一帧就绪的截屏(流本地像素)。
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
    /// 累计收到的帧数(诊断:区分"流停推"与"调用方没取")。
    frames_received: u64,
    /// PipeWire 格式协商出的缓冲尺寸(流本地像素)。
    negotiated: Option<(u32, u32)>,
    /// 最近一次流状态/错误(诊断用)。
    last_state: Option<String>,
    stopped: bool,
}

/// PipeWire 截屏接收器:持有专用线程与共享帧缓冲。
pub(super) struct PwCapture {
    shared: Arc<Mutex<SharedCapture>>,
    control: channel::Sender<bool>,
    worker: Option<JoinHandle<()>>,
}

impl PwCapture {
    /// 从 portal `OpenPipewireRemote` 的 fd 建立同会话截屏流。
    /// `node_id` 是 Start 响应 streams 里的 PipeWire node id。
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

    /// 取最新帧。`not_before` 为 `Some` 时(输入动作刚完成)先等比它新的帧,
    /// 宽限超时后回退为现有帧(画面没有视觉变化,现有帧就是当前画面);
    /// 完全没有帧时按首帧预算等待。
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
                // 帧存在但不比动作新:等宽限窗口,过期就用它。
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

    /// PipeWire 协商出的缓冲尺寸(诊断与 KDE 输入倍率换算用)。
    pub(super) fn negotiated_size(&self) -> Option<(u32, u32)> {
        self.shared
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .negotiated
    }

    /// 最近一次流状态(诊断用)。
    pub(super) fn last_state(&self) -> Option<String> {
        self.shared
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .last_state
            .clone()
    }

    /// 停流并 join 线程(backend 析构时调用;幂等)。
    pub(super) fn shutdown(&mut self) {
        self.shared
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stopped = true;
        let _ = self.control.send(false);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for PwCapture {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// PipeWire 线程主体:portal fd → context → stream(帧共享)。
/// `control_rx` 收到 `false` 即停流退出主循环。
fn run_pw_loop(
    node_id: u32,
    fd: OwnedFd,
    shared: &Arc<Mutex<SharedCapture>>,
    control_rx: channel::Receiver<bool>,
) -> Result<(), String> {
    pipewire::init();

    let main_loop = MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = ContextRc::new(&main_loop, None).map_err(|e| e.to_string())?;
    // 会话私有的 PipeWire 远端:只暴露本 portal 会话的流对象。
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
            let (offset, stride) = {
                let chunk = data.chunk();
                (chunk.offset() as usize, chunk.stride() as usize)
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

    // 允许的格式:固定 BGRx(三大合成器 portal 实现的 shm 缓冲格式,见
    // mutter screen-cast / kwin screencastbuffer / xdg-desktop-portal-wlr);
    // 尺寸与帧率给区间,由合成器 fixate(协商结果即截屏分辨率,也是流本地
    // 像素的输入坐标空间)。
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
    // pw_stream 默认Inactive:显式激活后合成器才开始推帧。
    stream
        .set_active(true)
        .map_err(|e| format!("cannot activate the capture stream: {e}"))?;

    // 停流开关:收到 false 即停流退出(backend 析构/close 路径)。
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

/// BGRx([b,g,r,x])→ RGBA 帧拷贝,尊重 stride。纯函数,便于单测。
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
        // 宽 2 高 2,stride 12(每行多 4 字节填充);BGRx 输入 → RGBA 输出。
        let mut raw = vec![0u8; 12 * 2];
        // 第一行像素 0:B G R x → 期望 R G B A。
        raw[0..4].copy_from_slice(&[10, 20, 30, 255]);
        // 第一行像素 1。
        raw[4..8].copy_from_slice(&[40, 50, 60, 0]);
        // 第二行像素 0(在 padding 之后)。
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
        // stride < row(宽 3 时 row=12 > stride 8)。
        assert!(convert_frame(&[0u8; 24], 8, 3, 1).is_none());
        // 最后一行不满(len 7 < stride*(h-1)+row = 12)。
        assert!(convert_frame(&[0u8; 7], 8, 1, 2).is_none());
        assert!(convert_frame(&[0u8; 8], 8, 0, 1).is_none(), "zero width");
        assert!(convert_frame(&[0u8; 8], 8, 1, 0).is_none(), "zero height");
    }

    #[test]
    fn convert_frame_requires_full_final_row() {
        // 最后一行不满:整帧拒绝,不能只截掉尾行静默输出坏图。
        let raw = vec![0u8; 8]; // 1 行完整,声明 2 行。
        assert!(convert_frame(&raw, 8, 1, 2).is_none());
    }
}

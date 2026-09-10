//! macOS ScreenCaptureKit 静态截图适配（macOS 15.2+，被 `macos` 后端在
//! [`available`] 为真时优先使用）。
//!
//! 为什么迁移：xcap 0.9.8 的 macOS 静态截图走 CGWindowListCreateImage——
//! 该 API 在 macOS 15 SDK 已 obsoleted（"Please use ScreenCaptureKit instead"），
//! 且基于它的截屏在 Sequoia+ 会触发周期性"继续允许录屏"系统确认。
//! ScreenCaptureKit 是 Apple 指定的替代，共用同一条 Screen Recording TCC
//! 授权（预检仍用 CGPreflightScreenCaptureAccess，无需改动）。
//!
//! 实现取 [`SCScreenshotManager::captureImageInRect`]：直接吃**全局屏幕点
//! 矩形**（与 `Capture.origin_x/y` 的 CGDisplayBounds 空间一致），显示器枚举、
//! 内容过滤、像素尺寸协商都由系统完成，不需要 ShareableContent → Filter →
//! StreamConfiguration 三步链。返回 CGImage 为 BGRA 预乘 alpha，本模块做
//! BGRA→RGBA 行拷贝（含 stride 处理），alpha 仍由调用方单趟置 255（与
//! xcap 路径一致）。
//!
//! 线程约定：worker 线程没有 Obj-C runloop，completion 回调在 SCK 内部队列
//! 投递，故用 mpsc 通道等待，上限 [`CAPTURE_CALLBACK_TIMEOUT`]；超时按失败
//! 处理，不把 worker 线程钉死。回调交付的 CGImage 只在块执行期内有效，
//! 因此 BGRA→RGBA 转换在块内完成，通道只传 `Vec<u8>`（无 CF/NS 对象跨线程）。
//!
//! 系统版本门：应用最低支持 macOS 11（Cargo.toml），静态截图 API 需要
//! 15.2（见 [`MIN_MACOS_VERSION`] 的注释：类 14.0 引入，类方法 15.2 才有），
//! 之下由调用方回退 xcap 的 CGWindowList 路径。

use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2_core_foundation::{CFRange, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGDataProvider, CGImage};
use objc2_foundation::{NSError, NSProcessInfo};
use objc2_screen_capture_kit::SCScreenshotManager;

use super::super::types::ComputerUseError;

/// `SCScreenshotManager::captureImageInRect` 所需的最低系统版本。
///
/// 注意这不是 14.0：类本身在 macOS 14.0 引入，但 `captureImageInRect:
/// completionHandler:` 是 `API_AVAILABLE(macos(15.2))`（SDK 头文件实测）。
/// 若只按主版本 14 放行，14.0–15.1 上会向已合法注册的类发送不存在的
/// selector → Obj-C 异常无着陆垫 → 进程 abort。之下（含 14.x）走 xcap 的
/// CGWindowList 回退路径。
const MIN_MACOS_VERSION: (isize, isize) = (15, 2);

/// 单次截图完成回调的等待上限。远小于调用方的 150s 请求预算；超时按失败
/// 处理（迟到的回调向已断开的通道发送，静默丢弃）。
const CAPTURE_CALLBACK_TIMEOUT: Duration = Duration::from_secs(10);

/// 运行时判定本系统是否可用 ScreenCaptureKit 静态截图（macOS >= 15.2）。
pub(super) fn available() -> bool {
    let version = NSProcessInfo::processInfo().operatingSystemVersion();
    satisfies_min_version(version.majorVersion, version.minorVersion)
}

/// 版本比较的纯函数形态（便于对 14.x/15.0/15.1 这类无法在本机构造的边界
/// 做单元测试）。字段类型与 NSOperatingSystemVersion（isize）一致。
fn satisfies_min_version(major: isize, minor: isize) -> bool {
    (major, minor) >= MIN_MACOS_VERSION
}

/// 一次截图的原始像素（RGBA，每像素 4 字节，行紧凑无 stride）。
pub(super) struct CapturedScreen {
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// 截取全局屏幕点矩形 `(origin_x, origin_y, width_pts, height_pts)`（左上
/// 原点，可跨/负坐标）。要求 Screen Recording TCC 已授权（调用方预检）。
pub(super) fn capture_region(
    origin_x: i32,
    origin_y: i32,
    width_pts: i32,
    height_pts: i32,
) -> Result<CapturedScreen, ComputerUseError> {
    if width_pts <= 0 || height_pts <= 0 {
        return Err(ComputerUseError::failed(format!(
            "invalid capture region {width_pts}x{height_pts} at ({origin_x}, {origin_y})"
        )));
    }
    let (tx, rx) = mpsc::channel::<Result<CapturedScreen, String>>();
    let block = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        let result = if image.is_null() {
            let detail = if error.is_null() {
                "no image and no error".to_string()
            } else {
                // SAFETY: 回调参数非空且在块执行期内有效；code() 是纯读取，
                // 借用不超过块作用域。
                format!("NSerror code {}", unsafe { (*error).code() })
            };
            Err(format!("ScreenCaptureKit returned no image: {detail}"))
        } else {
            // SAFETY: image 非空且在块执行期内有效（回调参数的强引用语义由
            // Obj-C 运行时保证）；转换不触碰引用计数。
            bgra_image_to_rgba(unsafe { &*image })
        };
        let _ = tx.send(result);
    });
    // SAFETY: block 在调用返回后仍被 SCK 持有（Obj-C block copy 语义），
    // 本地 RcBlock 析构只释放本侧引用；rect 是纯值参数。
    unsafe {
        SCScreenshotManager::captureImageInRect_completionHandler(
            CGRect {
                origin: CGPoint {
                    x: f64::from(origin_x),
                    y: f64::from(origin_y),
                },
                size: CGSize {
                    width: f64::from(width_pts),
                    height: f64::from(height_pts),
                },
            },
            Some(&block),
        );
    }
    match rx.recv_timeout(CAPTURE_CALLBACK_TIMEOUT) {
        Ok(Ok(shot)) => Ok(shot),
        Ok(Err(detail)) => Err(ComputerUseError::failed(format!(
            "ScreenCaptureKit capture failed: {detail}"
        ))),
        Err(_) => Err(ComputerUseError::failed(
            "ScreenCaptureKit capture timed out",
        )),
    }
}

/// CGImage（BGRA 预乘 alpha）→ 紧凑 RGBA。行拷贝处理 stride（源行距
/// bytes_per_row 可能大于 width*4），逐像素交换 R/B 两个通道。
fn bgra_image_to_rgba(image: &CGImage) -> Result<CapturedScreen, String> {
    let (width, height, bytes_per_row, bits_per_pixel) = (
        CGImage::width(Some(image)),
        CGImage::height(Some(image)),
        CGImage::bytes_per_row(Some(image)),
        CGImage::bits_per_pixel(Some(image)),
    );
    if width == 0 || height == 0 {
        return Err(format!("empty CGImage {width}x{height}"));
    }
    if bits_per_pixel != 32 {
        return Err(format!(
            "unexpected CGImage bit depth {bits_per_pixel} (expected 32-bit BGRA)"
        ));
    }
    let provider = CGImage::data_provider(Some(image))
        .ok_or_else(|| "CGImage has no data provider".to_string())?;
    // CGDataProvider::data 拷出独立 CFData，由 CFRetained 智能指针托管
    // +1（drop 自动 release，本函数不得再手动 CFRelease）。
    let data = CGDataProvider::data(Some(&provider))
        .ok_or_else(|| "CGDataProvider::data returned null".to_string())?;
    let expected = bytes_per_row
        .checked_mul(height)
        .ok_or_else(|| "CGImage row stride overflows".to_string())?;
    let len = data.length();
    if len < 0 || (len as usize) < expected {
        return Err(format!(
            "CGImage data too small: {len} bytes, need {expected}"
        ));
    }
    let mut raw = vec![0u8; len as usize];
    // SAFETY: raw 缓冲有 len 字节；range 0..len 全覆盖，bytes 只写该范围。
    unsafe {
        data.bytes(
            CFRange {
                location: 0,
                length: len,
            },
            raw.as_mut_ptr(),
        )
    };
    let mut rgba = vec![0u8; width * height * 4];
    copy_bgra_to_rgba(&mut rgba, &raw, width, bytes_per_row, height);
    Ok(CapturedScreen {
        rgba,
        width,
        height,
    })
}

/// 行拷贝 BGRA→RGBA：`src` 行距 `bytes_per_row`（可大于 `width*4`，stride
/// 填充被丢弃），`dst` 为紧凑 `width*height*4`。
fn copy_bgra_to_rgba(
    dst: &mut [u8],
    src: &[u8],
    width: usize,
    bytes_per_row: usize,
    height: usize,
) {
    for row in 0..height {
        let src_row = &src[row * bytes_per_row..row * bytes_per_row + width * 4];
        let dst_row = &mut dst[row * width * 4..(row + 1) * width * 4];
        for (out, px) in dst_row.chunks_exact_mut(4).zip(src_row.chunks_exact(4)) {
            out[0] = px[2];
            out[1] = px[1];
            out[2] = px[0];
            out[3] = px[3];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_gate_requires_15_2() {
        // captureImageInRect 是 API_AVAILABLE(macos(15.2))：类 14.0 就存在,
        // 但 14.0-15.1 上发送该 selector 会直接 abort——这些版本必须走回退。
        assert!(!satisfies_min_version(14, 0));
        assert!(!satisfies_min_version(14, 4));
        assert!(!satisfies_min_version(15, 1));
        assert!(satisfies_min_version(15, 2));
        assert!(satisfies_min_version(15, 7));
        assert!(satisfies_min_version(16, 0));
        // 本机（能跑测试则必然 >= 11）：available() 必须与真实版本一致。
        let version = NSProcessInfo::processInfo().operatingSystemVersion();
        assert_eq!(
            available(),
            satisfies_min_version(version.majorVersion, version.minorVersion)
        );
    }

    #[test]
    fn bgra_to_rgba_handles_stride_and_channel_swap() {
        // 2x2 图、行距 12（比 2*4 多 4 字节填充）：BGRA→RGBA 且丢弃填充。
        let src = [
            1u8, 2, 3, 255, 4, 5, 6, 255, 0xAA, 0xBB, 0xCC, 0xDD, //
            10, 20, 30, 255, 40, 50, 60, 255, 0x11, 0x22, 0x33, 0x44,
        ];
        let mut dst = vec![0u8; 16];
        copy_bgra_to_rgba(&mut dst, &src, 2, 12, 2);
        assert_eq!(
            dst,
            [
                3, 2, 1, 255, 6, 5, 4, 255, //
                30, 20, 10, 255, 60, 50, 40, 255,
            ]
        );
    }

    #[test]
    #[ignore = "live probe: needs Screen Recording TCC grant on a real desktop"]
    fn live_capture_region_returns_matching_pixel_buffer() {
        if !available() {
            return;
        }
        let shot = capture_region(0, 0, 800, 600).expect("live ScreenCaptureKit capture");
        assert!(shot.width > 0 && shot.height > 0);
        assert_eq!(shot.rgba.len(), shot.width * shot.height * 4);
        // 真实桌面不会是纯黑（菜单栏/壁纸/窗口至少一处非零）。
        assert!(
            shot.rgba.chunks_exact(4).any(|px| px[..3] != [0, 0, 0]),
            "capture is entirely black"
        );
    }
}

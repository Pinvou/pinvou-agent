//! 截图缩放与坐标映射——Computer Use 的头号 bug 源，全部换算集中在本文件。
//!
//! 三层坐标空间：
//! - **截图空间（shot）**：模型看到的 PNG 像素，原点左上。长边 ≤ 1440。
//! - **设备空间（device）**：捕获显示器物理像素，全局坐标（多显示器可有负原点）。
//! - **输入空间（input）**：输入注入 API 的坐标。Windows/X11 == 设备物理像素；
//!   macOS == CGEvent 点 = 设备物理像素 × (1/backing_scale_factor)。
//!
//! `ScaleMap` 每次截图生成一份并随会话保存，后续动作坐标一律按最近一次截图换算。

use xcap::image;

use super::types::{Capture, ComputerUseError};

/// 截图长边上限（像素）。Anthropic 建议长边 ≤1568；取 1440 兼顾细节与 token 成本。
pub const MAX_LONG_EDGE: u32 = 1440;
/// 底座 `image_attach` 的单图硬上限是 5 MB，超限会被**静默跳过**（模型该轮
/// 失去视觉）。PNG 对照片类内容压缩率差，1440px 的噪点截图可以远超 5 MB
/// （评审发现）。编码后超限时按 0.8 步进降分辨率重编码，保住视觉通路，
/// 长边不低于 [`MIN_LONG_EDGE_FLOOR`]。
pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
pub const MIN_LONG_EDGE_FLOOR: u32 = 640;

/// 均匀缩放系数：长边压到 ≤ [`MAX_LONG_EDGE`]，小图不放大。
pub fn scale_factor(width: u32, height: u32) -> f64 {
    let long_edge = width.max(height);
    if long_edge == 0 {
        return 1.0;
    }
    (f64::from(MAX_LONG_EDGE) / f64::from(long_edge)).min(1.0)
}

/// 一次截图的坐标映射表。
#[derive(Debug, Clone)]
pub struct ScaleMap {
    /// 截图（PNG）宽/高，模型坐标空间。
    pub shot_w: u32,
    pub shot_h: u32,
    /// 设备物理像素宽/高。
    pub dev_w: u32,
    pub dev_h: u32,
    /// 显示器原点，**输入坐标空间**（Windows 物理像素；macOS 点）。
    pub origin_x: i32,
    pub origin_y: i32,
    /// 设备物理像素 → 输入坐标倍率（Windows/X11 = 1.0；macOS Retina 2x = 0.5）。
    pub input_scale_x: f64,
    pub input_scale_y: f64,
}

impl ScaleMap {
    /// 从捕获结果构建映射表。`input_scale` 非正/非有限时拒绝（评审发现：
    /// 0 或 NaN 倍率会让后续所有换算除零/产生垃圾坐标——混合 DPI 屏上曾
    /// 静默映射到错误位置）。
    pub fn from_capture(
        shot_w: u32,
        shot_h: u32,
        capture: &Capture,
    ) -> Result<Self, ComputerUseError> {
        let (input_scale_x, input_scale_y) = capture.input_scale();
        for (axis, scale) in [("x", input_scale_x), ("y", input_scale_y)] {
            if !scale.is_finite() || scale <= 0.0 {
                return Err(ComputerUseError::failed(format!(
                    "capture reports non-positive input_scale_{axis} ({scale}); \
                     the capture is not usable for coordinate mapping"
                )));
            }
        }
        Ok(Self {
            shot_w: shot_w.max(1),
            shot_h: shot_h.max(1),
            dev_w: capture.width.max(1),
            dev_h: capture.height.max(1),
            origin_x: capture.origin_x,
            origin_y: capture.origin_y,
            input_scale_x,
            input_scale_y,
        })
    }

    /// 截图相对设备像素的缩放比（结果文本里展示给模型）。
    pub fn factor(&self) -> f64 {
        f64::from(self.shot_w) / f64::from(self.dev_w)
    }

    /// 输入空间原点 ↔ 设备空间原点。
    fn origin_device(&self) -> (f64, f64) {
        (
            f64::from(self.origin_x) / self.input_scale_x,
            f64::from(self.origin_y) / self.input_scale_y,
        )
    }

    /// 截图坐标 → 全局设备物理像素。
    pub fn shot_to_device(&self, x: i64, y: i64) -> (i32, i32) {
        let (ox, oy) = self.origin_device();
        let dx = ox + x as f64 * f64::from(self.dev_w) / f64::from(self.shot_w);
        let dy = oy + y as f64 * f64::from(self.dev_h) / f64::from(self.shot_h);
        (dx.round() as i32, dy.round() as i32)
    }

    /// 截图坐标 → 全局输入注入坐标（发给后端 move/click 的坐标）。
    pub fn shot_to_input(&self, x: i64, y: i64) -> (i32, i32) {
        let ix = f64::from(self.origin_x)
            + x as f64 * f64::from(self.dev_w) * self.input_scale_x / f64::from(self.shot_w);
        let iy = f64::from(self.origin_y)
            + y as f64 * f64::from(self.dev_h) * self.input_scale_y / f64::from(self.shot_h);
        (ix.round() as i32, iy.round() as i32)
    }

    /// 设备物理像素点是否落在截图显示器范围内。混合 DPI 防护：多显示器下
    /// 光标可能在另一块屏上，此时按本映射换算是无意义的垃圾坐标（评审发现
    /// 的 `u32 as` 截断同源问题）——调用方必须先检查本方法。
    pub fn contains_device_point(&self, x: i32, y: i32) -> bool {
        let (ox, oy) = self.origin_device();
        let (fx, fy) = (f64::from(x), f64::from(y));
        ox <= fx && fx < ox + f64::from(self.dev_w) && oy <= fy && fy < oy + f64::from(self.dev_h)
    }

    /// 全局设备物理像素 → 截图坐标（cursor_position 回报给模型用）。
    pub fn device_to_shot(&self, x: i32, y: i32) -> (i64, i64) {
        let (ox, oy) = self.origin_device();
        let sx = (f64::from(x) - ox) * f64::from(self.shot_w) / f64::from(self.dev_w);
        let sy = (f64::from(y) - oy) * f64::from(self.shot_h) / f64::from(self.dev_h);
        (sx.round() as i64, sy.round() as i64)
    }

    /// 全局设备物理像素 → 全局输入坐标（T3 检测拿当前光标的输入坐标用）。
    pub fn device_to_input(&self, x: i32, y: i32) -> (i32, i32) {
        (
            (f64::from(x) * self.input_scale_x).round() as i32,
            (f64::from(y) * self.input_scale_y).round() as i32,
        )
    }

    /// Whether the point `x`/`y` falls inside this monitor's **input-space**
    /// rect `[origin_x, origin_x + dev_w * input_scale_x)` (same for `y`).
    /// Mixed-DPI guard beyond [`Self::contains_device_point`]: on macOS
    /// `cursor_position` reports `points × cursor-monitor scale`, so with
    /// mixed monitor scales the per-monitor device rects overlap (a 2x
    /// built-in at the origin and a 1x external share a device range) and a
    /// cursor in the overlap passes the wrong monitor's device check, after
    /// which `device_to_input` is ~scale-times off. Input/points coordinates
    /// are globally unique per monitor, so input rects stay disjoint. This is
    /// the input-space image of the device rect checked by
    /// `contains_device_point` (which starts at `origin_device`): the lower
    /// bound is the exact integer `origin_x`; the upper bound can be
    /// fractional for non-integer scales, so ceil it and keep the interval
    /// half-open (ceil is the identity on integral bounds, keeping edge
    /// semantics self-consistent with `origin_device`/`shot_to_input`).
    pub fn contains_input_point(&self, x: i32, y: i32) -> bool {
        let (fx, fy) = (f64::from(x), f64::from(y));
        let max_x = (f64::from(self.origin_x) + f64::from(self.dev_w) * self.input_scale_x).ceil();
        let max_y = (f64::from(self.origin_y) + f64::from(self.dev_h) * self.input_scale_y).ceil();
        f64::from(self.origin_x) <= fx && fx < max_x && f64::from(self.origin_y) <= fy && fy < max_y
    }

    /// [`Self::device_to_input`] gated by [`Self::contains_input_point`]:
    /// returns `Some` only when the point passes the input-space containment
    /// check. Callers forwarding macOS `cursor_position` output must prefer
    /// this over the unchecked conversion — on mixed-DPI multi-monitor setups
    /// the raw device point can belong to a different monitor than this map,
    /// and converting it anyway yields coordinates ~scale-times off (T3
    /// screens the wrong location while the injection lands at the real
    /// cursor, i.e. fail-open).
    pub fn device_to_input_checked(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        if self.contains_input_point(x, y) {
            Some(self.device_to_input(x, y))
        } else {
            None
        }
    }

    /// 把模型给的坐标钳制到截图范围内；返回 (x, y, 是否被钳制)。
    /// 模型经常发出越界坐标——钳制并在结果里警告，而不是失败。
    pub fn clamp_shot(&self, x: i64, y: i64) -> (i64, i64, bool) {
        let max_x = i64::from(self.shot_w) - 1;
        let max_y = i64::from(self.shot_h) - 1;
        let cx = x.clamp(0, max_x);
        let cy = y.clamp(0, max_y);
        (cx, cy, cx != x || cy != y)
    }
}

/// 缩放并 PNG 编码后的截图。
pub struct ScaledScreenshot {
    pub png: Vec<u8>,
    pub map: ScaleMap,
}

/// 设备物理像素捕获 → 均匀缩放（长边 ≤1440）→ PNG 编码。
pub fn downscale_and_encode(capture: &Capture) -> Result<ScaledScreenshot, ComputerUseError> {
    // 评审修复：`w * h * 4` 用 checked 乘法——超大尺寸在乘法处显式失败，
    // 而不是依赖 usize 宽度碰运气（32 位目标会回绕成假的小长度）。
    let expected_len = capture
        .width
        .checked_mul(capture.height)
        .and_then(|pixels| pixels.checked_mul(4))
        .map(|pixels| pixels as usize)
        .ok_or_else(|| {
            ComputerUseError::failed(format!(
                "capture {}x{} overflows the addressable rgba buffer size",
                capture.width, capture.height
            ))
        })?;
    if capture.width == 0 || capture.height == 0 || capture.rgba.len() != expected_len {
        return Err(ComputerUseError::failed(format!(
            "capture buffer mismatch: {}x{} expects {expected_len} rgba bytes, got {}",
            capture.width,
            capture.height,
            capture.rgba.len()
        )));
    }
    let factor = scale_factor(capture.width, capture.height);
    let mut shot_w = ((f64::from(capture.width) * factor).round() as u32).max(1);
    let mut shot_h = ((f64::from(capture.height) * factor).round() as u32).max(1);

    let source = image::RgbaImage::from_raw(capture.width, capture.height, capture.rgba.clone())
        .ok_or_else(|| ComputerUseError::failed("capture buffer cannot form an image"))?;
    let mut shot = if factor < 1.0 {
        image::imageops::resize(
            &source,
            shot_w,
            shot_h,
            image::imageops::FilterType::Triangle,
        )
    } else {
        source
    };

    // 编码超过底座 5MB 上限时降分辨率重编码：超限文件会被 image_attach
    // 静默跳过，模型该轮直接失去视觉——宁可细节少一点也不能盲。
    let mut png = encode_png(&shot)?;
    while png.len() > MAX_IMAGE_BYTES {
        let long_edge = shot_w.max(shot_h);
        if long_edge <= MIN_LONG_EDGE_FLOOR {
            break;
        }
        let scale = f64::from((long_edge as f32 * 0.8).round() as u32) / f64::from(long_edge);
        shot_w = ((f64::from(shot_w) * scale).round() as u32).max(1);
        shot_h = ((f64::from(shot_h) * scale).round() as u32).max(1);
        shot =
            image::imageops::resize(&shot, shot_w, shot_h, image::imageops::FilterType::Triangle);
        png = encode_png(&shot)?;
    }

    Ok(ScaledScreenshot {
        png,
        map: ScaleMap::from_capture(shot_w, shot_h, capture)?,
    })
}

fn encode_png(shot: &image::RgbaImage) -> Result<Vec<u8>, ComputerUseError> {
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(shot.clone())
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|error| ComputerUseError::failed(format!("png encode: {error}")))?;
    Ok(png.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(w: u32, h: u32, ox: i32, oy: i32, sx: f64, sy: f64) -> Capture {
        Capture {
            rgba: vec![0u8; w as usize * h as usize * 4],
            width: w,
            height: h,
            origin_x: ox,
            origin_y: oy,
            input_scale_x: sx,
            input_scale_y: sy,
        }
    }

    /// 测试助手：合法捕获 → 解包映射表。
    fn map(shot_w: u32, shot_h: u32, cap: &Capture) -> ScaleMap {
        ScaleMap::from_capture(shot_w, shot_h, cap).expect("valid capture map")
    }

    /// 评审修复回归：非正/非有限 input_scale 必须被拒绝。
    #[test]
    fn from_capture_rejects_non_positive_input_scale() {
        for (sx, sy) in [(0.0, 1.0), (1.0, 0.0), (-0.5, 1.0), (1.0, -1.0)] {
            let error = ScaleMap::from_capture(10, 10, &capture(100, 100, 0, 0, sx, sy))
                .expect_err("non-positive scale must be rejected");
            assert!(error.to_string().contains("input_scale"), "{error}");
        }
        let error = ScaleMap::from_capture(10, 10, &capture(100, 100, 0, 0, f64::NAN, 1.0))
            .expect_err("NaN scale must be rejected");
        assert!(error.to_string().contains("input_scale"), "{error}");
        // 合法倍率仍通过。
        assert!(ScaleMap::from_capture(10, 10, &capture(100, 100, 0, 0, 0.5, 0.5)).is_ok());
    }

    /// 评审修复回归：设备点必须落在截图显示器内才允许映射（混合 DPI 防护）。
    #[test]
    fn contains_device_point_bounds_multi_monitor() {
        let map = map(1440, 810, &capture(2560, 1440, -2560, 0, 1.0, 1.0));
        assert!(map.contains_device_point(-2560, 0));
        assert!(map.contains_device_point(-1, 1439));
        // 右边缘开区间：越界一点都算出界。
        assert!(!map.contains_device_point(0, 0));
        assert!(!map.contains_device_point(-2561, 0));
        assert!(!map.contains_device_point(-100, 1440));
        assert!(!map.contains_device_point(-100, -1));
    }

    /// Mixed-DPI overlap regression (M1-class setups): a 2x Retina built-in
    /// at the origin (points 1440x900 = physical 2880x1800, hence
    /// input_scale 0.5) plus a 1x external at point-origin 1440 (physical
    /// 1920x1080). The device rects overlap in [1440, 2880) while the input
    /// rects stay disjoint, so only the input-space check can tell which
    /// monitor the cursor is on.
    #[test]
    fn contains_input_point_disambiguates_mixed_dpi_overlap() {
        let builtin = map(1440, 900, &capture(2880, 1800, 0, 0, 0.5, 0.5));
        let external = map(1440, 810, &capture(1920, 1080, 1440, 0, 1.0, 1.0));

        // Cursor on the external screen at points (1500, 400) reports device
        // (1500, 400): both device-space checks pass, but only the external
        // map must accept the input-space check (the old path would map
        // through the built-in map to input ~750, i.e. ~2x off).
        assert!(builtin.contains_device_point(1500, 400));
        assert!(external.contains_device_point(1500, 400));
        assert!(!builtin.contains_input_point(1500, 400));
        assert_eq!(builtin.device_to_input_checked(1500, 400), None);
        assert!(external.contains_input_point(1500, 400));
        assert_eq!(
            external.device_to_input_checked(1500, 400),
            Some((1500, 400))
        );

        // Input-rect edges stay half-open on the built-in map (input width
        // 2880 * 0.5 = 1440).
        assert!(builtin.contains_input_point(1439, 400));
        assert!(!builtin.contains_input_point(1440, 400));

        // A cursor genuinely on the built-in screen (points (700, 400) report
        // device (1400, 800) at 2x) still maps through the built-in map.
        assert!(builtin.contains_input_point(1400, 800));
        assert!(!external.contains_input_point(1400, 800));
        assert_eq!(builtin.device_to_input_checked(1400, 800), Some((700, 400)));
    }

    /// At unit input scale the input rect equals the device rect, so the
    /// input-space check must agree with `contains_device_point` everywhere.
    #[test]
    fn contains_input_point_matches_device_check_at_unit_scale() {
        let map = map(1440, 810, &capture(2560, 1440, 0, 0, 1.0, 1.0));
        for x in [-1, 0, 1, 1279, 2559, 2560] {
            for y in [-1, 0, 1, 719, 1439, 1440] {
                assert_eq!(
                    map.contains_input_point(x, y),
                    map.contains_device_point(x, y),
                    "unit-scale disagreement at ({x},{y})"
                );
                let expected = if map.contains_device_point(x, y) {
                    Some((x, y))
                } else {
                    None
                };
                assert_eq!(map.device_to_input_checked(x, y), expected);
            }
        }
    }

    /// Negative-origin multi-monitor (Windows virtual desktop): the
    /// input-space check mirrors the existing negative-origin device test.
    #[test]
    fn contains_input_point_bounds_negative_origin_multi_monitor() {
        let map = map(1440, 810, &capture(2560, 1440, -2560, 0, 1.0, 1.0));
        assert!(map.contains_input_point(-2560, 0));
        assert!(map.contains_input_point(-1, 1439));
        // Right edge is half-open: one pixel out is already outside.
        assert!(!map.contains_input_point(0, 0));
        assert!(!map.contains_input_point(-2561, 0));
        assert!(!map.contains_input_point(-100, 1440));
        assert!(!map.contains_input_point(-100, -1));
        assert_eq!(map.device_to_input_checked(-2560, 0), Some((-2560, 0)));
        assert_eq!(map.device_to_input_checked(0, 0), None);
    }

    #[test]
    fn scale_factor_only_downscales_long_edge() {
        assert_eq!(scale_factor(1280, 720), 1.0);
        assert_eq!(scale_factor(1440, 900), 1.0);
        let f = scale_factor(2880, 1800);
        assert!((f - 0.5).abs() < 1e-9);
        let f = scale_factor(2560, 1440);
        assert!((f - 1440.0 / 2560.0).abs() < 1e-9);
        // 竖屏以高为长边。
        let f = scale_factor(1080, 2400);
        assert!((f - 0.6).abs() < 1e-9);
    }

    #[test]
    fn shot_device_input_round_trip_windows_style() {
        // Windows：物理像素 2560x1440，input_scale = 1，无缩放截图。
        let map = map(1440, 810, &capture(2560, 1440, 0, 0, 1.0, 1.0));
        let (dx, dy) = map.shot_to_device(720, 405);
        assert_eq!((dx, dy), (1280, 720));
        let (ix, iy) = map.shot_to_input(720, 405);
        assert_eq!((ix, iy), (1280, 720));
        // 设备 → 截图回环。
        let (sx, sy) = map.device_to_shot(dx, dy);
        assert_eq!((sx, sy), (720, 405));
    }

    #[test]
    fn shot_input_mapping_macos_retina_2x() {
        // macOS Retina：捕获 3024x1964 物理像素，CGEvent 点是物理像素/2，
        // xcap 报告的原点也是点。
        let cap = capture(3024, 1964, 0, 0, 0.5, 0.5);
        let shot_w = (3024.0 * scale_factor(3024, 1964)).round() as u32;
        let shot_h = (1964.0 * scale_factor(3024, 1964)).round() as u32;
        let map = map(shot_w, shot_h, &cap);
        // 截图中心 → 设备中心（物理像素）→ 输入点 = 物理像素/2。
        let (dx, dy) = map.shot_to_device(i64::from(shot_w) / 2, i64::from(shot_h) / 2);
        // 截图像素中心按缩放比还原（整数中心带来 ≤1px 的舍入）。
        assert!((dx - 1512).abs() <= 1 && (dy - 982).abs() <= 1);
        let (ix, iy) = map.shot_to_input(i64::from(shot_w) / 2, i64::from(shot_h) / 2);
        assert!((ix - 756).abs() <= 1 && (iy - 491).abs() <= 1);
        // 输入坐标 ≈ 设备物理像素 / 2（核心 Retina 契约；双重舍入允许 1px 误差）。
        assert!((ix * 2 - dx).abs() <= 1);
        assert!((iy * 2 - dy).abs() <= 1);
        // device → input 同一倍率（±1px 舍入）。
        let (dix, diy) = map.device_to_input(dx, dy);
        assert!((dix - ix).abs() <= 1 && (diy - iy).abs() <= 1);
    }

    #[test]
    fn multi_monitor_negative_origin_is_preserved() {
        // 主屏右侧的副屏：Windows 虚拟桌面原点可为负。
        let map = map(1440, 810, &capture(2560, 1440, -2560, 0, 1.0, 1.0));
        let (dx, dy) = map.shot_to_device(0, 0);
        assert_eq!((dx, dy), (-2560, 0));
        let (ix, iy) = map.shot_to_input(0, 0);
        assert_eq!((ix, iy), (-2560, 0));
        let (sx, sy) = map.device_to_shot(dx, dy);
        assert_eq!((sx, sy), (0, 0));
    }

    #[test]
    fn non_unit_scale_factor_round_trips_within_one_pixel() {
        // 奇数尺寸 + 非 1 缩放比：往返误差 ≤ 1 设备像素。
        let map = map(1113, 627, &capture(1983, 1117, 0, 0, 1.0, 1.0));
        for (sx, sy) in [(0, 0), (500, 300), (1112, 626), (1, 625)] {
            let (dx, dy) = map.shot_to_device(sx, sy);
            let (rx, ry) = map.device_to_shot(dx, dy);
            assert!((rx - sx).abs() <= 1, "x drift {sx} -> {rx}");
            assert!((ry - sy).abs() <= 1, "y drift {sy} -> {ry}");
        }
    }

    #[test]
    fn clamp_shot_bounds_and_flags() {
        let map = map(1440, 810, &capture(2560, 1440, 0, 0, 1.0, 1.0));
        assert_eq!(map.clamp_shot(100, 100), (100, 100, false));
        assert_eq!(map.clamp_shot(-5, 100), (0, 100, true));
        assert_eq!(map.clamp_shot(100, 900), (100, 809, true));
        assert_eq!(map.clamp_shot(9999, -1), (1439, 0, true));
    }

    #[test]
    fn downscale_and_encode_produces_png_and_map() {
        // 构造非纯色像素，验证编码路径真实工作。
        let mut cap = capture(2000, 1000, 0, 0, 1.0, 1.0);
        for (i, byte) in cap.rgba.iter_mut().enumerate() {
            *byte = (i % 251) as u8;
        }
        let scaled = downscale_and_encode(&cap).map_err(|e| e.to_string());
        let scaled = match scaled {
            Ok(s) => s,
            Err(e) => panic!("encode failed: {e}"),
        };
        assert_eq!(scaled.map.shot_w, 1440);
        assert_eq!(scaled.map.shot_h, 720);
        assert!(scaled.png.len() > 8);
        assert_eq!(&scaled.png[..4], b"\x89PNG");
        // 无缩放时尺寸不变。
        let small = downscale_and_encode(&capture(64, 32, 0, 0, 1.0, 1.0));
        assert!(small.is_ok());
        let small = match small {
            Ok(s) => s,
            Err(_) => unreachable!(),
        };
        assert_eq!((small.map.shot_w, small.map.shot_h), (64, 32));
    }

    #[test]
    fn oversized_png_downgrades_resolution_instead_of_losing_vision() {
        // 噪点 RGBA 产生远超 5MB 的 PNG（照片类内容的最坏情况）。
        let w = 2560u32;
        let h = 1440u32;
        let mut cap = Capture {
            rgba: vec![0u8; (w * h * 4) as usize],
            width: w,
            height: h,
            origin_x: 0,
            origin_y: 0,
            input_scale_x: 1.0,
            input_scale_y: 1.0,
        };
        // 线性同余噪声：不可压缩，逼出真实的大 PNG。
        let mut state = 123_456_789u32;
        for byte in cap.rgba.iter_mut() {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            *byte = (state >> 16) as u8;
        }
        let scaled = downscale_and_encode(&cap).expect("encode");
        assert!(
            scaled.png.len() <= MAX_IMAGE_BYTES,
            "encoded {} bytes must fit the foundation cap {MAX_IMAGE_BYTES}",
            scaled.png.len()
        );
        assert_eq!(&scaled.png[..4], b"\x89PNG");
        assert!(scaled.map.shot_w >= MIN_LONG_EDGE_FLOOR / 2);
    }

    #[test]
    fn downscale_and_encode_rejects_bad_buffer() {
        let bad = Capture {
            rgba: vec![0u8; 10],
            width: 100,
            height: 100,
            origin_x: 0,
            origin_y: 0,
            input_scale_x: 1.0,
            input_scale_y: 1.0,
        };
        assert!(downscale_and_encode(&bad).is_err());
    }

    /// 评审修复回归：尺寸乘法用 checked——溢出显式失败而非回绕成假的小长度。
    #[test]
    fn downscale_and_encode_rejects_overflowing_dimensions() {
        let huge = Capture {
            rgba: vec![0u8; 16],
            width: u32::MAX,
            height: u32::MAX,
            origin_x: 0,
            origin_y: 0,
            input_scale_x: 1.0,
            input_scale_y: 1.0,
        };
        let error = match downscale_and_encode(&huge) {
            Err(error) => error,
            Ok(scaled) => panic!("overflow must be rejected, got {} bytes", scaled.png.len()),
        };
        assert!(error.to_string().contains("overflows"), "{error}");
    }
}

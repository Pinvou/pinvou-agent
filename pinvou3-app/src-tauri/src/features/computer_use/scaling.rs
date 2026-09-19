//! Screenshot scaling and coordinate mapping — the number-one bug source of Computer Use;
//! all conversions are centralized in this file.
//!
//! Three coordinate spaces:
//! - **Screenshot space (shot)**: the PNG pixels the model sees, origin at the top-left.
//!   Long edge ≤ 1440.
//! - **Device space (device)**: physical pixels of the captured monitor, global coordinates
//!   (multi-monitor origins can be negative).
//! - **Input space (input)**: coordinates of the input injection APIs. Windows/X11 ==
//!   device physical pixels; macOS == CGEvent points = device physical pixels ×
//!   (1/backing_scale_factor).
//!
//! A `ScaleMap` is generated per screenshot and kept with the session; subsequent action
//! coordinates are always converted against the most recent screenshot; the conversion is
//! bidirectional shot space ↔ input space (`shot_to_input` / `input_to_shot`), and cursor
//! positions are reported by the backend directly in input coordinates.

use xcap::image;
use xcap::image::ImageEncoder as _;

use super::types::{Capture, ComputerUseError};

/// Screenshot long-edge cap (pixels). Anthropic recommends a long edge ≤1568; 1440 balances
/// detail against token cost.
pub const MAX_LONG_EDGE: u32 = 1440;
/// The foundation `image_attach` hard cap per image is 5 MB; over-limit images are
/// **silently skipped** (the model loses vision for that turn). PNG compresses photo-like
/// content poorly, and a noisy 1440px screenshot can far exceed 5 MB. When
/// the encoded size exceeds the cap, re-encode at 0.8 resolution steps to keep the visual
/// channel alive, with the long edge never below [`MIN_LONG_EDGE_FLOOR`].
pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
pub const MIN_LONG_EDGE_FLOOR: u32 = 640;

/// Uniform scale factor: squeeze the long edge to ≤ [`MAX_LONG_EDGE`], never upscale small
/// images.
pub fn scale_factor(width: u32, height: u32) -> f64 {
    let long_edge = width.max(height);
    if long_edge == 0 {
        return 1.0;
    }
    (f64::from(MAX_LONG_EDGE) / f64::from(long_edge)).min(1.0)
}

/// Coordinate map for one screenshot.
#[derive(Debug, Clone)]
pub struct ScaleMap {
    /// Screenshot (PNG) width/height, the model's coordinate space.
    pub shot_w: u32,
    pub shot_h: u32,
    /// Device physical pixel width/height.
    pub dev_w: u32,
    pub dev_h: u32,
    /// Monitor origin, in **input coordinate space** (Windows physical pixels; macOS points).
    pub origin_x: i32,
    pub origin_y: i32,
    /// Device physical pixels → input coordinate scale (Windows/X11 = 1.0; macOS Retina 2x = 0.5).
    pub input_scale_x: f64,
    pub input_scale_y: f64,
}

impl ScaleMap {
    /// Build the map from a capture result. Rejects a non-positive/non-finite `input_scale`:
    /// a 0 or NaN scale makes every later conversion divide by zero or produce garbage
    /// coordinates.
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

    /// Screenshot-to-device-pixel scale ratio on the x axis (shown to the model in result
    /// text). Screenshot scaling is uniform, so this matches [`Self::factor_y`] up to
    /// rounding; compare both when a non-uniform ratio would mislead.
    pub fn factor(&self) -> f64 {
        self.factor_x()
    }

    /// Screenshot-to-device scale ratio per axis (`shot_w/dev_w` and `shot_h/dev_h`).
    pub fn factor_x(&self) -> f64 {
        f64::from(self.shot_w) / f64::from(self.dev_w)
    }

    pub fn factor_y(&self) -> f64 {
        f64::from(self.shot_h) / f64::from(self.dev_h)
    }

    /// Screenshot coordinates → global input injection coordinates (what is sent to the
    /// backend's move/click).
    pub fn shot_to_input(&self, x: i64, y: i64) -> (i32, i32) {
        let ix = f64::from(self.origin_x)
            + x as f64 * f64::from(self.dev_w) * self.input_scale_x / f64::from(self.shot_w);
        let iy = f64::from(self.origin_y)
            + y as f64 * f64::from(self.dev_h) * self.input_scale_y / f64::from(self.shot_h);
        (ix.round() as i32, iy.round() as i32)
    }

    /// Whether the point `x`/`y` — already expressed in the **input space**
    /// (as `cursor_position` reports it) — falls inside this monitor's
    /// input-space rect `[origin_x, origin_x + dev_w * input_scale_x)`
    /// (same for `y`, half-open). Input/points coordinates are globally
    /// unique per monitor (unlike device pixels, whose rects overlap when
    /// monitor scales differ), so per-monitor input rects are disjoint and
    /// this containment test is exact: the point is on this map's monitor
    /// or it is not.
    pub fn contains_input_point(&self, x: i32, y: i32) -> bool {
        let (fx, fy) = (f64::from(x), f64::from(y));
        let max_x = (f64::from(self.origin_x) + f64::from(self.dev_w) * self.input_scale_x).ceil();
        let max_y = (f64::from(self.origin_y) + f64::from(self.dev_h) * self.input_scale_y).ceil();
        f64::from(self.origin_x) <= fx && fx < max_x && f64::from(self.origin_y) <= fy && fy < max_y
    }

    /// Global input coordinates → screenshot coordinates (what cursor_position reports back
    /// to the model). The inverse of [`Self::shot_to_input`]:
    /// `(ix - origin_x) * shot_w / (dev_w * input_scale_x)`, computed in f64 then rounded.
    /// A nonzero denominator is guaranteed by [`Self::from_capture`] (input_scale must be
    /// > 0 and finite; dev dimensions have a floor of 1).
    ///
    /// The result is clamped to `[0, shot-1]`: when capturing with
    /// downsample ≥2× (4K/5K screens → 1440 long edge), the last input pixel admitted by the
    /// containment test rounds up to exactly `shot_w` (e.g. at 3840→1440, 3839 →
    /// 1439.625 → 1440) — the model would receive an out-of-range coordinate and the next
    /// click would trigger a spurious "outside the screenshot" warning. The legal domain of
    /// screenshot coordinates is `[0, shot-1]` anyway, so clamping loses no information.
    pub fn input_to_shot(&self, ix: i32, iy: i32) -> (i64, i64) {
        let sx = (f64::from(ix) - f64::from(self.origin_x)) * f64::from(self.shot_w)
            / (f64::from(self.dev_w) * self.input_scale_x);
        let sy = (f64::from(iy) - f64::from(self.origin_y)) * f64::from(self.shot_h)
            / (f64::from(self.dev_h) * self.input_scale_y);
        (
            (sx.round() as i64).clamp(0, i64::from(self.shot_w) - 1),
            (sy.round() as i64).clamp(0, i64::from(self.shot_h) - 1),
        )
    }

    /// Clamp model-provided coordinates into the screenshot range; returns (x, y, clamped).
    /// Models frequently emit out-of-range coordinates — clamp and warn in the result
    /// instead of failing.
    pub fn clamp_shot(&self, x: i64, y: i64) -> (i64, i64, bool) {
        let max_x = i64::from(self.shot_w) - 1;
        let max_y = i64::from(self.shot_h) - 1;
        let cx = x.clamp(0, max_x);
        let cy = y.clamp(0, max_y);
        (cx, cy, cx != x || cy != y)
    }
}

/// The screenshot after scaling and PNG encoding.
pub struct ScaledScreenshot {
    pub png: Vec<u8>,
    pub map: ScaleMap,
}

/// Device physical pixel capture → uniform scale (long edge ≤1440) → PNG encode.
pub fn downscale_and_encode(capture: &Capture) -> Result<ScaledScreenshot, ComputerUseError> {
    // `w * h * 4` uses checked multiplication — oversized dimensions fail
    // explicitly at the multiplication instead of relying on usize width luck
    // (32-bit targets would wrap into a fake small length).
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

    // Resize reads straight from the borrowed capture buffer (no copy); only the
    // no-downscale path needs an owned image, because the encode-budget loop below
    // reassigns `shot` with fresh owned buffers.
    let mut shot: image::RgbaImage = if factor < 1.0 {
        let source =
            image::ImageBuffer::from_raw(capture.width, capture.height, capture.rgba.as_slice())
                .ok_or_else(|| ComputerUseError::failed("capture buffer cannot form an image"))?;
        image::imageops::resize(
            &source,
            shot_w,
            shot_h,
            image::imageops::FilterType::Triangle,
        )
    } else {
        image::RgbaImage::from_raw(capture.width, capture.height, capture.rgba.clone())
            .ok_or_else(|| ComputerUseError::failed("capture buffer cannot form an image"))?
    };

    // When the encoding exceeds the foundation's 5MB cap, re-encode at a lower resolution:
    // an over-cap file is silently skipped by image_attach and the model loses vision for
    // that turn outright — less detail is preferable to blindness.
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
    // Encode straight from the pixel slice: DynamicImage::write_to selects this same
    // encoder for RGBA8, so wrapping the buffer in a DynamicImage would clone the whole
    // image for no benefit.
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            shot.as_raw(),
            shot.width(),
            shot.height(),
            image::ExtendedColorType::Rgba8,
        )
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

    /// Test helper: valid capture → unwrap the map.
    fn map(shot_w: u32, shot_h: u32, cap: &Capture) -> ScaleMap {
        ScaleMap::from_capture(shot_w, shot_h, cap).expect("valid capture map")
    }

    /// A non-positive/non-finite input_scale must be rejected.
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
        // Legitimate scales still pass.
        assert!(ScaleMap::from_capture(10, 10, &capture(100, 100, 0, 0, 0.5, 0.5)).is_ok());
    }

    /// Mixed-DPI overlap regression (M1-class setups): a 2x Retina built-in
    /// at the origin (points 1440x900 = physical 2880x1800, hence
    /// input_scale 0.5) plus a 1x external at point-origin 1440 (physical
    /// 1920x1080). The device rects overlap in [1440, 2880) while the input
    /// rects stay disjoint, so only the input-space containment can tell
    /// which monitor the cursor is on.
    #[test]
    fn contains_input_point_disambiguates_mixed_dpi_overlap() {
        let builtin = map(1440, 900, &capture(2880, 1800, 0, 0, 0.5, 0.5));
        let external = map(1440, 810, &capture(1920, 1080, 1440, 0, 1.0, 1.0));

        // Cursor on the external screen at points (1500, 400): only the
        // external map accepts it (the built-in input rect is [0, 1440);
        // the old device-space path would have mapped the cursor through
        // the built-in map to input ~750, i.e. ~2x off).
        assert!(!builtin.contains_input_point(1500, 400));
        assert!(external.contains_input_point(1500, 400));

        // Input-rect edges stay half-open on the built-in map (input width
        // 2880 * 0.5 = 1440).
        assert!(builtin.contains_input_point(1439, 400));
        assert!(!builtin.contains_input_point(1440, 400));

        // A cursor genuinely on the built-in screen (points (700, 400),
        // device (1400, 800) at 2x) is accepted by the built-in map and
        // rejected by the external one.
        assert!(builtin.contains_input_point(1400, 800));
        assert!(!external.contains_input_point(1400, 800));
    }

    /// Negative-origin multi-monitor (Windows virtual desktop): the
    /// input-space check bounds the monitor rect with a half-open right
    /// edge, mirroring the pre-input-space device test.
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
    }

    /// Non-integer input scale: `input_to_shot` is the algebraic inverse of
    /// `shot_to_input` (dev 200x200 device px, input_scale 0.5 → the input
    /// rect is [0, 100), shot 144x144). Double rounding can shift the round
    /// trip by at most one shot pixel; well-conditioned points (and every
    /// multiple of 36, where the intermediate values are exact) invert
    /// exactly.
    #[test]
    fn input_to_shot_inverts_shot_to_input_on_non_integer_scale() {
        let map = map(144, 144, &capture(200, 200, 0, 0, 0.5, 0.5));
        for x in [0, 1, 36, 37, 71, 72, 105, 143] {
            let (ix, iy) = map.shot_to_input(x, x);
            assert_eq!(
                map.input_to_shot(ix, iy),
                (x, x),
                "shot_to_input -> input_to_shot must invert at {x}"
            );
        }
        for x in 0..=144 {
            let (ix, iy) = map.shot_to_input(x, x);
            let (rx, ry) = map.input_to_shot(ix, iy);
            assert!(
                (rx - x).abs() <= 1 && (ry - x).abs() <= 1,
                "round-trip drift beyond one shot pixel at {x}"
            );
        }
    }

    /// `contains_input_point` boundaries on the same non-integer-scale map
    /// (dev 200x200 at input_scale 0.5 → input rect [0, 100) x [0, 100)).
    #[test]
    fn contains_input_point_boundaries_on_non_integer_scale() {
        let map = map(144, 144, &capture(200, 200, 0, 0, 0.5, 0.5));
        assert!(map.contains_input_point(0, 0));
        assert!(map.contains_input_point(99, 99));
        assert!(map.contains_input_point(50, 30));
        // Half-open edges: input width/height is exactly 100.
        assert!(!map.contains_input_point(100, 50));
        assert!(!map.contains_input_point(50, 100));
        assert!(!map.contains_input_point(-1, 50));
        assert!(!map.contains_input_point(50, -1));
    }

    #[test]
    fn scale_factor_only_downscales_long_edge() {
        assert_eq!(scale_factor(1280, 720), 1.0);
        assert_eq!(scale_factor(1440, 900), 1.0);
        let f = scale_factor(2880, 1800);
        assert!((f - 0.5).abs() < 1e-9);
        let f = scale_factor(2560, 1440);
        assert!((f - 1440.0 / 2560.0).abs() < 1e-9);
        // Portrait: the height is the long edge.
        let f = scale_factor(1080, 2400);
        assert!((f - 0.6).abs() < 1e-9);
    }

    #[test]
    fn shot_input_round_trip_windows_style() {
        // Windows: physical pixels 2560x1440, input_scale = 1, unscaled screenshot.
        let map = map(1440, 810, &capture(2560, 1440, 0, 0, 1.0, 1.0));
        let (ix, iy) = map.shot_to_input(720, 405);
        assert_eq!((ix, iy), (1280, 720));
        // Input → screenshot round trip.
        let (sx, sy) = map.input_to_shot(ix, iy);
        assert_eq!((sx, sy), (720, 405));
    }

    #[test]
    fn shot_input_mapping_macos_retina_2x() {
        // macOS Retina: capture 3024x1964 physical pixels, CGEvent points are physical
        // pixels / 2, and the origin xcap reports is also in points.
        let cap = capture(3024, 1964, 0, 0, 0.5, 0.5);
        let shot_w = (3024.0 * scale_factor(3024, 1964)).round() as u32;
        let shot_h = (1964.0 * scale_factor(3024, 1964)).round() as u32;
        let map = map(shot_w, shot_h, &cap);
        // Screenshot center → input point = physical pixels / 2.
        let (ix, iy) = map.shot_to_input(i64::from(shot_w) / 2, i64::from(shot_h) / 2);
        assert!((ix - 756).abs() <= 1 && (iy - 491).abs() <= 1);
        // Input → screenshot round trip (1px error allowed from division rounding).
        let (sx, sy) = map.input_to_shot(ix, iy);
        assert!((sx - i64::from(shot_w) / 2).abs() <= 1);
        assert!((sy - i64::from(shot_h) / 2).abs() <= 1);
    }

    #[test]
    fn multi_monitor_negative_origin_is_preserved() {
        // Secondary screen to the right of the primary: the Windows virtual desktop origin
        // can be negative.
        let map = map(1440, 810, &capture(2560, 1440, -2560, 0, 1.0, 1.0));
        let (ix, iy) = map.shot_to_input(0, 0);
        assert_eq!((ix, iy), (-2560, 0));
        let (sx, sy) = map.input_to_shot(ix, iy);
        assert_eq!((sx, sy), (0, 0));
    }

    #[test]
    fn non_unit_scale_factor_round_trips_within_one_pixel() {
        // Odd dimensions + non-1 scale factor: round-trip error ≤ 1 input pixel.
        let map = map(1113, 627, &capture(1983, 1117, 0, 0, 1.0, 1.0));
        for (sx, sy) in [(0, 0), (500, 300), (1112, 626), (1, 625)] {
            let (ix, iy) = map.shot_to_input(sx, sy);
            let (rx, ry) = map.input_to_shot(ix, iy);
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
        // Construct non-uniform-color pixels to verify the encode path really works.
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
        // Unchanged dimensions when no scaling applies.
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
        // Noisy RGBA produces a PNG far over 5MB (worst case for photo-like content).
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
        // Linear congruential noise: incompressible, forces a genuinely large PNG.
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
        // Real termination condition: the downgrade ladder starts at the long-edge cap and
        // multiplies by 0.8 per step, so four steps land at/below MIN_LONG_EDGE_FLOOR where
        // the loop stops — the final long edge is always inside
        // [0.8^4 * MAX_LONG_EDGE, MAX_LONG_EDGE] and the encoded PNG fits the cap.
        let long_edge = scaled.map.shot_w.max(scaled.map.shot_h);
        let ladder_floor = (f64::from(MAX_LONG_EDGE) * 0.8f64.powi(4)).round() as u32;
        assert!(
            (ladder_floor..=MAX_LONG_EDGE).contains(&long_edge),
            "long edge {long_edge} outside the termination window [{ladder_floor}, {MAX_LONG_EDGE}]"
        );
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

    /// Dimension multiplication is checked — overflow fails explicitly
    /// rather than wrapping into a fake small length.
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

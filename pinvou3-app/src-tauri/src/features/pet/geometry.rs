//! 桌宠窗口的纯几何计算:尺寸推导、缩放钳制、锚点定位、坐标迁移。
//!
//! 本模块刻意只容纳**纯函数**(无 `&WebviewWindow` / `&AppHandle` / 异步 / IO),
//! 便于独立单测与跨平台数学复用。窗口生命周期操作(建窗、置顶、resize、状态
//! 持久化)仍留在 [`super::pet_window`],后者通过 `use super::geometry::*` 复用
//! 本模块的纯函数。
//!
//! `pet_window_effective_size` 调 [`super::platform::effective_window_size`] 获取
//! 各平台运行时生效尺寸(Linux 上夹到 WebKitGTK 最小内容尺寸),该函数本身仍是纯函数。

use serde::{Deserialize, Serialize};

pub const PET_LABEL: &str = "pet";

const PET_FRAME_W: f64 = 192.0;
const PET_FRAME_H: f64 = 208.0;
const PET_HORIZONTAL_PADDING: f64 = 48.0;
const PET_VERTICAL_PADDING: f64 = 16.0;
const PET_COMPACT_MIN_H: f64 = 165.0;
const PET_ACTIVITY_WINDOW_W: f64 = 350.0;
const PET_ACTIVITY_DEFAULT_H: f64 = 112.0;
const PET_ACTIVITY_MIN_H: f64 = 48.0;
const PET_ACTIVITY_MAX_H: f64 = 260.0;
const PET_ACTIVITY_GAP_H: f64 = 12.0;
/// 人物可见区距窗口底边的距离,与 pet.css(.pet-root padding-bottom)及
/// JS 拖拽物理(pet-interaction PET_BOTTOM_PADDING)三方必须一致。
const PET_CHARACTER_BOTTOM: f64 = 8.0;
const MIN_SCALE: f64 = 0.5;
const MAX_SCALE: f64 = 1.2;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PetVerticalAlignment {
    Top,
    #[default]
    Bottom,
}

impl PetVerticalAlignment {
    pub(crate) fn from_str(value: &str) -> Self {
        if value == "top" {
            Self::Top
        } else {
            Self::Bottom
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

pub fn clamp_scale(s: f64) -> f64 {
    if !s.is_finite() {
        return 1.0;
    }
    s.clamp(MIN_SCALE, MAX_SCALE)
}

pub(crate) fn pet_window_logical_size(
    scale: f64,
    activity_visible: bool,
    activity_height: Option<f64>,
) -> (f64, f64) {
    let scale = clamp_scale(scale);
    let compact_width = PET_FRAME_W * scale + PET_HORIZONTAL_PADDING;
    if activity_visible {
        let content_height = activity_content_height(activity_height);
        (
            compact_width.max(PET_ACTIVITY_WINDOW_W),
            PET_FRAME_H * scale + content_height + PET_ACTIVITY_GAP_H,
        )
    } else {
        (
            compact_width,
            (PET_FRAME_H * scale + PET_VERTICAL_PADDING).max(PET_COMPACT_MIN_H),
        )
    }
}

/// 运行时生效尺寸。统一内存设备实测:WebKitGTK 给 webview 的最小内容尺寸约 200x200,
/// GTK 不允许窗口小于内容最小值——紧凑桌伴请求 144x165 实得 200x200,
/// min_inner_size hint 也放不开。定位数学若按请求尺寸算,窗口底/右边会凸出
/// 预期边界,拖拽物理一开边界钳制人物就被顶走("点击上移")。Linux 上把假定
/// 尺寸夹到同一下限让数学与真实窗口一致;其他平台请求尺寸如实生效,不动。
pub(crate) fn pet_window_effective_size(
    scale: f64,
    activity_visible: bool,
    activity_height: Option<f64>,
) -> (f64, f64) {
    let size = pet_window_logical_size(scale, activity_visible, activity_height);
    super::platform::effective_window_size(size)
}

pub(crate) fn activity_content_height(activity_height: Option<f64>) -> f64 {
    let measured = activity_height.unwrap_or(PET_ACTIVITY_DEFAULT_H);
    if measured.is_finite() {
        measured.clamp(PET_ACTIVITY_MIN_H, PET_ACTIVITY_MAX_H)
    } else {
        PET_ACTIVITY_DEFAULT_H
    }
}

pub(crate) fn scale_resize_required(current: f64, next: f64) -> bool {
    (current - next).abs() > 1e-9
}

/// 点 (cx,cy) 是否落在任一显示器矩形内。恢复保存位置前用窗口中心点判定——
/// 显示器可能被拔掉/换分辨率,落在"不存在的屏"上的宠物等于消失。
pub fn point_on_any_monitor(cx: i32, cy: i32, monitors: &[(i32, i32, u32, u32)]) -> bool {
    monitors
        .iter()
        .any(|&(x, y, w, h)| super::point_in_rect(cx, cy, x, y, w as i32, h as i32))
}

pub(crate) fn legacy_frame_position_to_client(
    saved: (i32, i32),
    observed_inner: (i32, i32),
    observed_outer: (i32, i32),
) -> Option<(i32, i32)> {
    let dx = observed_inner.0.checked_sub(observed_outer.0)?;
    let dy = observed_inner.1.checked_sub(observed_outer.1)?;
    if !(0..=128).contains(&dx) || !(0..=128).contains(&dy) {
        return None;
    }
    Some((saved.0.saturating_add(dx), saved.1.saturating_add(dy)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScaleAnchor {
    BottomCenter,
    BottomLeft,
    BottomRight,
    TopCenter,
    TopLeft,
    TopRight,
}

pub(crate) fn resized_position(
    position: (i32, i32),
    old_size: (u32, u32),
    new_size: (u32, u32),
    anchor: ScaleAnchor,
    work_area: Option<(i32, i32, u32, u32)>,
) -> (i32, i32) {
    let (mut x, mut y) = position;
    match anchor {
        ScaleAnchor::BottomCenter => {
            x += (old_size.0 as i32 - new_size.0 as i32) / 2;
            y += old_size.1 as i32 - new_size.1 as i32;
        }
        ScaleAnchor::BottomLeft => {
            y += old_size.1 as i32 - new_size.1 as i32;
        }
        ScaleAnchor::BottomRight => {
            x += old_size.0 as i32 - new_size.0 as i32;
            y += old_size.1 as i32 - new_size.1 as i32;
        }
        ScaleAnchor::TopCenter => {
            x += (old_size.0 as i32 - new_size.0 as i32) / 2;
        }
        ScaleAnchor::TopLeft => {}
        ScaleAnchor::TopRight => {
            x += old_size.0 as i32 - new_size.0 as i32;
        }
    }
    if let Some((left, top, width, height)) = work_area {
        let right = left + width as i32;
        let bottom = top + height as i32;
        let max_x = (right - new_size.0 as i32).max(left);
        let max_y = (bottom - new_size.1 as i32).max(top);
        x = x.clamp(left, max_x);
        y = y.clamp(top, max_y);
    }
    (x, y)
}

pub(crate) fn edge_anchor(
    position: (i32, i32),
    size: (u32, u32),
    work_area: Option<(i32, i32, u32, u32)>,
    vertical_alignment: PetVerticalAlignment,
) -> ScaleAnchor {
    let Some((left, _, width, _)) = work_area else {
        return match vertical_alignment {
            PetVerticalAlignment::Top => ScaleAnchor::TopCenter,
            PetVerticalAlignment::Bottom => ScaleAnchor::BottomCenter,
        };
    };
    let window_center = position.0 as i64 + size.0 as i64 / 2;
    let monitor_center = left as i64 + width as i64 / 2;
    match (window_center <= monitor_center, vertical_alignment) {
        (true, PetVerticalAlignment::Top) => ScaleAnchor::TopLeft,
        (false, PetVerticalAlignment::Top) => ScaleAnchor::TopRight,
        (true, PetVerticalAlignment::Bottom) => ScaleAnchor::BottomLeft,
        (false, PetVerticalAlignment::Bottom) => ScaleAnchor::BottomRight,
    }
}

/// [`super::pet_window::PetWindowState`] serde default 用的最小缩放。
pub(super) fn default_scale() -> f64 {
    MIN_SCALE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_on_any_monitor_basic() {
        let monitors = [(0, 0, 1920, 1080), (1920, 0, 2560, 1440)];
        assert!(point_on_any_monitor(100, 100, &monitors));
        assert!(point_on_any_monitor(3000, 500, &monitors)); // 第二屏
        assert!(!point_on_any_monitor(-50, 100, &monitors)); // 屏外左
        assert!(!point_on_any_monitor(100, 2000, &monitors)); // 屏外下
        assert!(!point_on_any_monitor(0, 0, &[])); // 拿不到显示器 → 不信任保存位置
    }

    #[test]
    fn legacy_frame_position_migrates_only_with_valid_observation() {
        assert_eq!(
            legacy_frame_position_to_client((815, 489), (815, 526), (815, 489)),
            Some((815, 526))
        );
        assert_eq!(
            legacy_frame_position_to_client((815, 489), (815, 900), (815, 489)),
            None,
            "异常 frame inset 不得把旧坐标永久标成 client"
        );
        assert_eq!(
            legacy_frame_position_to_client((815, 489), (815, 480), (815, 489)),
            None,
            "负 inset 是陈旧观测，不得参与迁移"
        );
        assert_eq!(
            legacy_frame_position_to_client((100, 200), (10, 10), (10, 10)),
            Some((100, 200)),
            "无边框平台的零 inset 必须保持坐标不变"
        );
    }

    #[test]
    fn scale_clamped() {
        assert_eq!(clamp_scale(0.1), 0.5);
        assert_eq!(clamp_scale(9.0), 1.2);
        assert_eq!(clamp_scale(1.2), 1.2);
        assert_eq!(clamp_scale(f64::NAN), 1.0);
    }

    #[test]
    fn pet_window_size_keeps_activity_cards_readable_at_every_pet_scale() {
        assert_eq!(pet_window_logical_size(0.5, false, None), (144.0, 165.0));
        assert_eq!(pet_window_logical_size(0.5, true, None), (350.0, 228.0));
        assert_eq!(pet_window_logical_size(1.0, true, None), (350.0, 332.0));
        assert_eq!(pet_window_logical_size(1.2, true, None), (350.0, 373.6));
    }

    #[test]
    fn effective_size_floors_to_webview_min_on_linux_only() {
        assert_eq!(
            pet_window_effective_size(0.5, false, None),
            super::super::platform::effective_window_size((144.0, 165.0))
        );
        // 高于下限的尺寸各平台一致。
        assert_eq!(pet_window_effective_size(1.0, true, None), (350.0, 332.0));
    }

    #[test]
    fn unchanged_scale_does_not_race_activity_resize() {
        assert!(!scale_resize_required(0.5, 0.5));
        assert!(!scale_resize_required(0.5, 0.5 + 1e-10));
        assert!(scale_resize_required(0.5, 0.6));
    }

    #[test]
    fn pet_window_size_uses_measured_activity_height() {
        assert_eq!(pet_window_logical_size(1.0, false, None), (240.0, 224.0));
        assert_eq!(
            pet_window_logical_size(1.0, true, Some(64.0)),
            (350.0, 284.0)
        );
        assert_eq!(
            pet_window_logical_size(1.0, true, Some(180.0)),
            (350.0, 400.0)
        );
        assert_eq!(
            pet_window_logical_size(1.0, true, Some(999.0)),
            (350.0, 480.0)
        );
    }

    #[test]
    fn resize_position_preserves_selected_anchor_and_work_area() {
        assert_eq!(
            resized_position(
                (100, 200),
                (240, 330),
                (480, 660),
                ScaleAnchor::TopLeft,
                Some((0, 0, 1920, 1080)),
            ),
            (100, 200)
        );
        assert_eq!(
            resized_position(
                (1700, 800),
                (240, 330),
                (480, 660),
                ScaleAnchor::TopLeft,
                Some((0, 0, 1920, 1080)),
            ),
            (1440, 420)
        );
        assert_eq!(
            resized_position(
                (100, 200),
                (240, 330),
                (480, 660),
                ScaleAnchor::BottomCenter,
                None,
            ),
            (-20, -130)
        );
        assert_eq!(
            resized_position(
                (100, 200),
                (144, 165),
                (350, 228),
                ScaleAnchor::BottomLeft,
                None,
            ),
            (100, 137)
        );
        assert_eq!(
            resized_position(
                (1000, 200),
                (144, 165),
                (350, 228),
                ScaleAnchor::BottomRight,
                None,
            ),
            (794, 137)
        );
    }

    #[test]
    fn activity_window_anchor_tracks_the_screen_half() {
        let work_area = Some((100, 0, 1200, 800));
        assert_eq!(
            edge_anchor(
                (120, 20),
                (144, 165),
                work_area,
                PetVerticalAlignment::Bottom,
            ),
            ScaleAnchor::BottomLeft
        );
        assert_eq!(
            edge_anchor(
                (1000, 20),
                (144, 165),
                work_area,
                PetVerticalAlignment::Bottom,
            ),
            ScaleAnchor::BottomRight
        );
        assert_eq!(
            edge_anchor((120, 20), (144, 165), work_area, PetVerticalAlignment::Top,),
            ScaleAnchor::TopLeft
        );
    }
}

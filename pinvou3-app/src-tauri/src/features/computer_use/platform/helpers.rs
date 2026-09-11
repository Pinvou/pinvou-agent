//! Pure helpers shared by the per-OS backends (hoisted in the ninth review
//! round: the per-platform copies had drifted — Linux truncated names in a
//! different order, macOS scroll did not clamp, and drag interpolation was
//! inlined twice). No OS handles here: everything is a plain function so it
//! stays unit-testable on every target.

use super::super::types::ScrollDirection;

/// Overflow guard on scroll clicks per call: enigo multiplies the click
/// count by `WHEEL_DELTA` (120) internally, so an unclamped count overflows
/// i32 (debug panic / release wrap-around). Matches the tool layer's
/// `MAX_SCROLL_AMOUNT` parse cap, which is the effective limit — this one
/// only bounds the backend math. Windows used to clamp here; macOS did not
/// clamp at all — both go through this now.
pub(crate) const MAX_SCROLL_CLICKS: u32 = 100;

/// Scroll direction → enigo (axis, signed clicks). enigo convention:
/// Vertical positive is down / negative up, Horizontal positive is right /
/// negative left.
pub(crate) fn map_scroll(direction: ScrollDirection, clicks: u32) -> (enigo::Axis, i32) {
    let clicks = i32::try_from(clicks.min(MAX_SCROLL_CLICKS)).unwrap_or(i32::MAX);
    match direction {
        ScrollDirection::Up => (enigo::Axis::Vertical, -clicks),
        ScrollDirection::Down => (enigo::Axis::Vertical, clicks),
        ScrollDirection::Left => (enigo::Axis::Horizontal, -clicks),
        ScrollDirection::Right => (enigo::Axis::Horizontal, clicks),
    }
}

/// Linear interpolation waypoints for a drag (includes the endpoint, excludes
/// the start) so the target receives enough motion events even when the
/// compositor coalesces absolute moves.
pub(crate) fn drag_waypoints(from: (i32, i32), to: (i32, i32), steps: usize) -> Vec<(i32, i32)> {
    let steps = steps.max(1);
    (1..=steps)
        .map(|i| {
            let t = i as f64 / steps as f64;
            let x = f64::from(from.0) + f64::from(to.0 - from.0) * t;
            let y = f64::from(from.1) + f64::from(to.1 - from.1) * t;
            (x.round() as i32, y.round() as i32)
        })
        .collect()
}

/// Element-name sanitizer for model-facing single-line tree text: quotes are
/// straightened, every control character (newlines, tabs, C0/C1) folds to a
/// space, the result is truncated and trimmed. Mapping before truncation
/// guarantees a single clean line; the Linux copy used to truncate first,
/// which let a cut point preserve a line break.
pub(crate) fn sanitize_name(name: &str, max_chars: usize) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() {
                ' '
            } else if c == '"' {
                '\''
            } else {
                c
            }
        })
        .take(max_chars)
        .collect();
    cleaned.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_waypoints_interpolates_and_excludes_start() {
        let pts = drag_waypoints((0, 0), (100, 50), 4);
        assert_eq!(pts, vec![(25, 13), (50, 25), (75, 38), (100, 50)]);
        assert_eq!(drag_waypoints((7, 7), (9, 9), 0).len(), 1);
        assert_eq!(drag_waypoints((7, 7), (9, 9), 0)[0], (9, 9));
    }

    #[test]
    fn map_scroll_clamps_and_signs() {
        assert_eq!(
            map_scroll(ScrollDirection::Up, 3),
            (enigo::Axis::Vertical, -3)
        );
        assert_eq!(
            map_scroll(ScrollDirection::Right, 3),
            (enigo::Axis::Horizontal, 3)
        );
        assert_eq!(
            map_scroll(ScrollDirection::Down, 1_000),
            (enigo::Axis::Vertical, MAX_SCROLL_CLICKS as i32)
        );
    }

    #[test]
    fn sanitize_name_folds_controls_and_trims() {
        assert_eq!(sanitize_name("a\"b\nc\td\u{1}e ", 20), "a'b c d e");
        assert_eq!(sanitize_name("abcdef", 3), "abc");
        assert_eq!(sanitize_name("  \n x", 10), "x");
    }
}

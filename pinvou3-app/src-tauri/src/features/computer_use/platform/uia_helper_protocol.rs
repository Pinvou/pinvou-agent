//! The hidden-desktop UIA helper's wire protocol — the platform-independent
//! half of [`super::uia_helper`].
//!
//! The helper is this same executable re-spawned with [`HELPER_ARG`] on the
//! hidden desktop (inside the session's kill-on-close job), talking NDJSON
//! over inherited stdin/stdout. Parent and child share these types so the
//! framing cannot drift: requests are [`Request`] lines, responses are
//! [`Response`] lines, and the very first line the helper writes is
//! [`READY_LINE`].
//!
//! The `ElementInfo`-shaped payload ([`WireElement`]) deliberately carries
//! the two fields the real `ElementInfo` serializes with `#[serde(skip)]`
//! (`name_screening_hit` / raw binding): the T3 forced-confirmation chain on
//! the parent side reads them, and they are computed in the helper where the
//! raw (attacker-controlled) name still exists — exactly the same boundary
//! the physical backend draws. The raw name itself never crosses the pipe;
//! the wire carries the same display-truncated `name` the in-process path
//! would produce.

use serde::{Deserialize, Serialize};

/// The argv marker that turns this executable into a UIA helper for one
/// hidden-desktop session (see `run_helper_loop`). Deliberately long and
/// namespaced: an accidental collision with any other argv contract would
/// silently turn an app launch into a pipe server.
pub const HELPER_ARG: &str = "--pinvou-computer-use-uia-helper";

/// The first line the helper writes after COM init. The parent waits for it
/// (bounded) instead of racing the first request against UIA initialization.
pub const READY_LINE: &str = "{\"ready\":true}";

/// One query from the backend. `id` is echoed verbatim; the protocol is
/// strictly serial (one outstanding request), so the id is a sanity check,
/// not a demux key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub op: Op,
}

/// The query kinds. Field names are short (`d`/`n`/`x`/`y`) — these cross the
/// pipe on every keyboard action (`focus` rides the confirm preview path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Serialize the hidden desktop's UIA tree with the physical stack's
    /// exact line format and budgets.
    Tree {
        #[serde(rename = "d")]
        max_depth: u32,
        #[serde(rename = "n")]
        max_nodes: u32,
    },
    /// The element at a virtual-screen-global screen point.
    AtPoint {
        #[serde(rename = "x")]
        x: i32,
        #[serde(rename = "y")]
        y: i32,
    },
    /// The focused control of the hidden desktop's foreground window.
    Focus,
}

/// One response line. `ok: false` is a SOFT verdict (the helper ran and
/// answers why): the parent treats transport failures (timeout, broken
/// pipe, bad framing) and this verdict identically — drop the helper and
/// degrade.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<WireElement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn tree(id: u64, tree: String) -> Self {
        Response {
            id,
            ok: true,
            tree: Some(tree),
            element: None,
            error: None,
        }
    }

    pub fn element(id: u64, element: Option<WireElement>) -> Self {
        Response {
            id,
            ok: true,
            tree: None,
            element,
            error: None,
        }
    }

    pub fn error(id: u64, error: String) -> Self {
        Response {
            id,
            ok: false,
            tree: None,
            element: None,
            error: Some(error),
        }
    }
}

/// The `ElementInfo` wire shape — see the module doc for why the skipped
/// fields travel explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireElement {
    pub role: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub secure: bool,
    /// The T3 screening verdict on the RAW name (computed in the helper).
    pub screening_hit: bool,
    /// The keyed (name, role) binding hash (computed in the helper).
    pub binding: u64,
}

/// Whether this process was started AS a helper (checked before any app
/// initialization in `run`).
pub fn is_helper_invocation() -> bool {
    std::env::args().any(|arg| arg == HELPER_ARG)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_request_round_trips_with_short_field_names() {
        let req = Request {
            id: 7,
            op: Op::Tree {
                max_depth: 8,
                max_nodes: 400,
            },
        };
        let line = serde_json::to_string(&req).expect("serialize");
        assert!(line.contains("\"op\":\"tree\""), "{line}");
        assert!(
            line.contains("\"d\":8") && line.contains("\"n\":400"),
            "{line}"
        );
        let back: Request = serde_json::from_str(&line).expect("deserialize");
        assert_eq!(back, req);
    }

    #[test]
    fn at_point_and_focus_ops_round_trip() {
        let at = Request {
            id: 1,
            op: Op::AtPoint { x: -1920, y: 1079 },
        };
        let line = serde_json::to_string(&at).expect("serialize");
        let back: Request = serde_json::from_str(&line).expect("deserialize");
        assert_eq!(back, at);
        assert!(line.contains("\"x\":-1920"), "{line}");

        let focus = Request {
            id: 2,
            op: Op::Focus,
        };
        let line = serde_json::to_string(&focus).expect("serialize");
        let back: Request = serde_json::from_str(&line).expect("deserialize");
        assert_eq!(back, focus);
    }

    #[test]
    fn response_variants_round_trip_and_skip_empty_fields() {
        let tree = Response::tree(3, "[0] Window \"x\" (0,0,1,1)".to_string());
        let line = serde_json::to_string(&tree).expect("serialize");
        assert!(line.contains("\"tree\":\""), "{line}");
        assert!(!line.contains("element"), "{line}");
        let back: Response = serde_json::from_str(&line).expect("deserialize");
        assert_eq!(back, tree);

        let err = Response::error(4, "no focus".to_string());
        let line = serde_json::to_string(&err).expect("serialize");
        assert!(
            line.contains("\"ok\":false") && line.contains("no focus"),
            "{line}"
        );
        let back: Response = serde_json::from_str(&line).expect("deserialize");
        assert_eq!(back, err);

        let el = Response::element(
            5,
            Some(WireElement {
                role: "Edit".to_string(),
                name: "Password".to_string(),
                x: 0,
                y: 0,
                width: 80,
                height: 20,
                secure: true,
                screening_hit: false,
                binding: 42,
            }),
        );
        let line = serde_json::to_string(&el).expect("serialize");
        // The screening fields MUST travel (serde(skip) in ElementInfo must
        // not silently drop them on this wire).
        assert!(line.contains("\"screening_hit\":false"), "{line}");
        assert!(line.contains("\"binding\":42"), "{line}");
        assert!(line.contains("\"secure\":true"), "{line}");
        let back: Response = serde_json::from_str(&line).expect("deserialize");
        assert_eq!(back, el);
    }

    #[test]
    fn ready_line_is_exact_json_true() {
        let parsed: serde_json::Value =
            serde_json::from_str(READY_LINE).expect("ready line must be valid JSON");
        assert_eq!(parsed, serde_json::json!({"ready": true}));
    }

    #[test]
    fn helper_arg_detection_scans_all_arguments() {
        // The real check reads std::env::args; this pins the constant shape
        // the detection and the spawner must agree on.
        assert!(HELPER_ARG.starts_with("--pinvou-"));
        assert!(HELPER_ARG.len() > 24, "long namespaced marker");
        assert_ne!(HELPER_ARG, std::env::args().next().unwrap_or_default());
    }
}

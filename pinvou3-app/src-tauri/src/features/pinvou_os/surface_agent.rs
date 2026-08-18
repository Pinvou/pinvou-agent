use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::model::{CapabilityContract, Interruptibility, ResourceClass};

pub const SURFACE_AGENT_ID: &str = "agent:surface";
pub const SURFACE_OBSERVE_CAPABILITY_ID: &str = "surface.observe";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceAgentState {
    Idle,
    Ready,
    Degraded,
    Unavailable,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceSource {
    /// Pinvou 自绘界面：结构化状态本身就是真相，不需要截图。
    KnownSurface,
    Accessibility,
    WindowManager,
    /// 只知道像素区域存在；语义尚未经过 OCR/VLM 兜底解析。
    OpaqueCapture,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceRole {
    Window,
    Dialog,
    Button,
    TextField,
    Text,
    Checkbox,
    Image,
    List,
    ListItem,
    Menu,
    MenuItem,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceAction {
    Activate,
    Focus,
    SetValue,
    Toggle,
    Scroll,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceNodeObservation {
    pub node_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub role: SurfaceRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<SurfaceBounds>,
    pub visible: bool,
    pub enabled: bool,
    pub focused: bool,
    #[serde(default)]
    pub actions: BTreeSet<SurfaceAction>,
    pub source: SurfaceSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceWindowObservation {
    pub window_id: String,
    pub application_id: String,
    pub title: String,
    pub bounds: SurfaceBounds,
    pub z_order: i32,
    pub focused: bool,
    pub source: SurfaceSource,
    #[serde(default)]
    pub nodes: Vec<SurfaceNodeObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceObservationInput {
    pub observation_id: String,
    pub observed_at_ms: i64,
    #[serde(default)]
    pub accessibility_available: bool,
    #[serde(default)]
    pub screen_capture_available: bool,
    #[serde(default)]
    pub windows: Vec<SurfaceWindowObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceFact {
    pub subject: String,
    pub predicate: String,
    pub value: Value,
    pub evidence_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceScene {
    pub agent_id: String,
    pub state: SurfaceAgentState,
    pub revision: u64,
    pub observation_id: String,
    pub observed_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused_window_id: Option<String>,
    pub windows: Vec<SurfaceWindowObservation>,
    pub facts: Vec<SurfaceFact>,
    pub limitation_codes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceObservationError {
    message: String,
}

impl SurfaceObservationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SurfaceObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for SurfaceObservationError {}

#[derive(Debug, Clone)]
pub struct SurfaceAgent {
    state: SurfaceAgentState,
    revision: u64,
    last_scene: Option<SurfaceScene>,
}

impl Default for SurfaceAgent {
    fn default() -> Self {
        Self {
            state: SurfaceAgentState::Idle,
            revision: 0,
            last_scene: None,
        }
    }
}

impl SurfaceAgent {
    pub fn state(&self) -> SurfaceAgentState {
        self.state
    }

    pub fn last_scene(&self) -> Option<&SurfaceScene> {
        self.last_scene.as_ref()
    }

    /// 把各平台 Provider 提交的窗口/可访问性观测归一为稳定场景事实。
    /// 本原子能力不主动截屏；OpaqueCapture 只标记语义缺口，留给昂贵兜底层。
    pub fn observe(
        &mut self,
        input: SurfaceObservationInput,
    ) -> Result<SurfaceScene, SurfaceObservationError> {
        if let Err(error) = validate_observation(&input) {
            self.state = SurfaceAgentState::Failed;
            return Err(error);
        }

        let mut windows = input.windows;
        windows.sort_by(|left, right| {
            right
                .z_order
                .cmp(&left.z_order)
                .then_with(|| left.window_id.cmp(&right.window_id))
        });
        for window in &mut windows {
            window
                .nodes
                .sort_by(|left, right| left.node_id.cmp(&right.node_id));
        }

        let focused_window_id = windows
            .iter()
            .find(|window| window.focused)
            .map(|window| window.window_id.clone());
        let semantic_nodes = windows
            .iter()
            .flat_map(|window| &window.nodes)
            .filter(|node| {
                matches!(
                    node.source,
                    SurfaceSource::KnownSurface | SurfaceSource::Accessibility
                )
            })
            .count();
        let has_opaque_content = windows.iter().any(|window| {
            window.source == SurfaceSource::OpaqueCapture
                || window
                    .nodes
                    .iter()
                    .any(|node| node.source == SurfaceSource::OpaqueCapture)
        });

        let mut limitation_codes = Vec::new();
        let state = if windows.is_empty()
            && !input.accessibility_available
            && !input.screen_capture_available
        {
            limitation_codes.push("no_surface_provider_available".to_string());
            SurfaceAgentState::Unavailable
        } else if semantic_nodes == 0 {
            limitation_codes.push("semantic_scene_unavailable".to_string());
            SurfaceAgentState::Degraded
        } else {
            if has_opaque_content {
                limitation_codes.push("opaque_regions_present".to_string());
            }
            SurfaceAgentState::Ready
        };

        let mut facts = Vec::new();
        if let Some(window_id) = &focused_window_id {
            facts.push(SurfaceFact {
                subject: "desktop".to_string(),
                predicate: "focused_window".to_string(),
                value: Value::String(window_id.clone()),
                evidence_ref: input.observation_id.clone(),
            });
        }
        for window in &windows {
            facts.push(SurfaceFact {
                subject: format!("window:{}", window.window_id),
                predicate: "window_state".to_string(),
                value: json!({
                    "applicationId": window.application_id,
                    "title": window.title,
                    "focused": window.focused,
                    "zOrder": window.z_order,
                    "bounds": window.bounds,
                    "source": window.source,
                }),
                evidence_ref: input.observation_id.clone(),
            });
            for node in &window.nodes {
                facts.push(SurfaceFact {
                    subject: format!("surface_node:{}:{}", window.window_id, node.node_id),
                    predicate: "node_state".to_string(),
                    value: json!({
                        "windowId": window.window_id,
                        "parentId": node.parent_id,
                        "role": node.role,
                        "label": node.label,
                        "value": node.value,
                        "bounds": node.bounds,
                        "visible": node.visible,
                        "enabled": node.enabled,
                        "focused": node.focused,
                        "actions": node.actions,
                        "source": node.source,
                    }),
                    evidence_ref: format!(
                        "{}#{}:{}",
                        input.observation_id, window.window_id, node.node_id
                    ),
                });
            }
        }

        self.revision = self.revision.saturating_add(1);
        self.state = state;
        let scene = SurfaceScene {
            agent_id: SURFACE_AGENT_ID.to_string(),
            state,
            revision: self.revision,
            observation_id: input.observation_id,
            observed_at_ms: input.observed_at_ms,
            focused_window_id,
            windows,
            facts,
            limitation_codes,
        };
        self.last_scene = Some(scene.clone());
        Ok(scene)
    }
}

pub fn surface_observe_contract() -> CapabilityContract {
    CapabilityContract {
        capability_id: SURFACE_OBSERVE_CAPABILITY_ID.to_string(),
        version: 1,
        summary: "把窗口、Pinvou Surface IR 与可访问性观测归一为结构化场景事实".to_string(),
        input_schema: json!({
            "type": "object",
            "required": ["observationId", "observedAtMs", "windows"],
            "properties": {
                "observationId": { "type": "string", "minLength": 1 },
                "observedAtMs": { "type": "integer" },
                "accessibilityAvailable": { "type": "boolean" },
                "screenCaptureAvailable": { "type": "boolean" },
                "windows": { "type": "array" }
            }
        }),
        output_schema: json!({
            "type": "object",
            "required": ["agentId", "state", "revision", "windows", "facts", "limitationCodes"],
            "properties": {
                "state": { "enum": ["idle", "ready", "degraded", "unavailable", "failed"] },
                "revision": { "type": "integer", "minimum": 1 },
                "windows": { "type": "array" },
                "facts": { "type": "array" },
                "limitationCodes": { "type": "array", "items": { "type": "string" } }
            }
        }),
        preconditions: vec!["at_least_one_surface_provider_initialized".to_string()],
        permissions: vec!["screen_read".to_string()],
        side_effects: vec!["surface_projection_changed".to_string()],
        resource_class: ResourceClass::Light,
        interruptibility: Interruptibility::Immediate,
        idempotent: false,
    }
}

fn validate_observation(input: &SurfaceObservationInput) -> Result<(), SurfaceObservationError> {
    if input.observation_id.trim().is_empty() {
        return Err(SurfaceObservationError::new(
            "surface observation id must not be empty",
        ));
    }
    if input.observed_at_ms < 0 {
        return Err(SurfaceObservationError::new(
            "surface observation timestamp must be non-negative",
        ));
    }

    let mut window_ids = BTreeSet::new();
    let mut focused_windows = 0_usize;
    let mut focused_nodes = 0_usize;
    for window in &input.windows {
        if window.window_id.trim().is_empty() || window.application_id.trim().is_empty() {
            return Err(SurfaceObservationError::new(
                "surface window id and application id must not be empty",
            ));
        }
        if !window_ids.insert(window.window_id.as_str()) {
            return Err(SurfaceObservationError::new(format!(
                "duplicate surface window id {}",
                window.window_id
            )));
        }
        validate_bounds(window.bounds, "window")?;
        focused_windows += usize::from(window.focused);

        let node_ids = window
            .nodes
            .iter()
            .map(|node| node.node_id.as_str())
            .collect::<BTreeSet<_>>();
        if node_ids.len() != window.nodes.len() || node_ids.contains("") {
            return Err(SurfaceObservationError::new(format!(
                "window {} contains empty or duplicate node ids",
                window.window_id
            )));
        }
        let mut seen = BTreeMap::new();
        for node in &window.nodes {
            if let Some(bounds) = node.bounds {
                validate_bounds(bounds, "surface node")?;
            }
            if node
                .parent_id
                .as_ref()
                .is_some_and(|parent_id| !node_ids.contains(parent_id.as_str()))
            {
                return Err(SurfaceObservationError::new(format!(
                    "surface node {} references an unknown parent",
                    node.node_id
                )));
            }
            if node.parent_id.as_deref() == Some(node.node_id.as_str()) {
                return Err(SurfaceObservationError::new(format!(
                    "surface node {} cannot be its own parent",
                    node.node_id
                )));
            }
            seen.insert(node.node_id.as_str(), node.parent_id.as_deref());
            focused_nodes += usize::from(node.focused);
        }
        for node in &window.nodes {
            let mut ancestors = BTreeSet::new();
            let mut cursor = node.parent_id.as_deref();
            while let Some(parent_id) = cursor {
                if !ancestors.insert(parent_id) {
                    return Err(SurfaceObservationError::new(format!(
                        "surface node {} has a parent cycle",
                        node.node_id
                    )));
                }
                cursor = seen.get(parent_id).copied().flatten();
            }
        }
    }
    if focused_windows > 1 || focused_nodes > 1 {
        return Err(SurfaceObservationError::new(
            "surface observation has multiple focused targets",
        ));
    }
    Ok(())
}

fn validate_bounds(bounds: SurfaceBounds, kind: &str) -> Result<(), SurfaceObservationError> {
    if !bounds.x.is_finite()
        || !bounds.y.is_finite()
        || !bounds.width.is_finite()
        || !bounds.height.is_finite()
        || bounds.width < 0.0
        || bounds.height < 0.0
    {
        return Err(SurfaceObservationError::new(format!(
            "{kind} bounds must be finite and non-negative"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> SurfaceBounds {
        SurfaceBounds {
            x: 0.0,
            y: 0.0,
            width: 1280.0,
            height: 800.0,
        }
    }

    #[test]
    fn known_surface_becomes_ready_without_pixel_capture() {
        let mut agent = SurfaceAgent::default();
        let scene = agent
            .observe(SurfaceObservationInput {
                observation_id: "obs-1".to_string(),
                observed_at_ms: 10,
                accessibility_available: false,
                screen_capture_available: false,
                windows: vec![SurfaceWindowObservation {
                    window_id: "pinvou".to_string(),
                    application_id: "pinvou.os".to_string(),
                    title: "Pinvou".to_string(),
                    bounds: bounds(),
                    z_order: 9,
                    focused: true,
                    source: SurfaceSource::KnownSurface,
                    nodes: vec![SurfaceNodeObservation {
                        node_id: "microphone".to_string(),
                        parent_id: None,
                        role: SurfaceRole::Button,
                        label: Some("麦克风".to_string()),
                        value: None,
                        bounds: Some(bounds()),
                        visible: true,
                        enabled: true,
                        focused: false,
                        actions: BTreeSet::from([SurfaceAction::Activate]),
                        source: SurfaceSource::KnownSurface,
                    }],
                }],
            })
            .expect("known surface observation should be valid");

        assert_eq!(scene.state, SurfaceAgentState::Ready);
        assert_eq!(scene.focused_window_id.as_deref(), Some("pinvou"));
        assert_eq!(scene.facts.len(), 3);
        assert!(scene.limitation_codes.is_empty());
        assert_eq!(agent.state(), SurfaceAgentState::Ready);
    }

    #[test]
    fn unavailable_and_invalid_inputs_have_explicit_state() {
        let mut agent = SurfaceAgent::default();
        let scene = agent
            .observe(SurfaceObservationInput {
                observation_id: "obs-empty".to_string(),
                observed_at_ms: 10,
                accessibility_available: false,
                screen_capture_available: false,
                windows: Vec::new(),
            })
            .expect("empty provider result is a valid unavailable scene");
        assert_eq!(scene.state, SurfaceAgentState::Unavailable);

        let error = agent.observe(SurfaceObservationInput {
            observation_id: String::new(),
            observed_at_ms: 10,
            accessibility_available: false,
            screen_capture_available: false,
            windows: Vec::new(),
        });
        assert!(error.is_err());
        assert_eq!(agent.state(), SurfaceAgentState::Failed);
        assert_eq!(agent.last_scene().map(|scene| scene.revision), Some(1));
    }
}

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::model::{CapabilityContract, Interruptibility, ResourceClass};

pub const POLICY_AGENT_ID: &str = "agent:policy";
pub const POLICY_AUTHORIZE_CAPABILITY_ID: &str = "policy.authorize";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAgentState {
    Ready,
    Evaluating,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    ReadOnly,
    LocalMutation,
    ExternalCommunication,
    CredentialAccess,
    DeviceControl,
    Destructive,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRisk {
    Routine,
    Elevated,
    Critical,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationDecision {
    Allow,
    Deny,
    RequireConfirmation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionGrant {
    pub permission_id: String,
    /// 空集合代表该权限不限制目标 scope；否则使用点分层级前缀匹配。
    #[serde(default)]
    pub scope_prefixes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct UserBoundary {
    /// 空集合代表不额外限制 scope；非空时拟议目标必须命中其中一个前缀。
    #[serde(default)]
    pub allowed_scope_prefixes: Vec<String>,
    #[serde(default)]
    pub denied_capability_ids: BTreeSet<String>,
    #[serde(default)]
    pub denied_effects: BTreeSet<PolicyEffect>,
    #[serde(default)]
    pub always_confirm_effects: BTreeSet<PolicyEffect>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProposedAction {
    pub action_id: String,
    pub actor_id: String,
    pub capability_id: String,
    pub target_scope: String,
    #[serde(default)]
    pub required_permissions: BTreeSet<String>,
    #[serde(default)]
    pub effects: BTreeSet<PolicyEffect>,
    pub risk: PolicyRisk,
    pub reversible: bool,
    /// 只能由内核 Device/Governor 事实计算；动作提出者不能自报或隐瞒。
    #[serde(default)]
    pub(crate) safety_invariant_violations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActionConfirmation {
    /// 确认必须绑定完整动作内容，不能只凭可复用 action_id 放行被篡改的目标或副作用。
    pub action: ProposedAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PolicyEvaluationInput {
    pub now_ms: i64,
    pub action: ProposedAction,
    /// 以下 Authority 数据只能由内核 AuthorityStore 构造，不能成为 renderer/Agent 输入。
    #[serde(default)]
    pub(crate) permission_grants: Vec<PermissionGrant>,
    #[serde(default)]
    pub(crate) explicitly_denied_permissions: BTreeSet<String>,
    #[serde(default)]
    pub(crate) confirmations: Vec<ActionConfirmation>,
    #[serde(default)]
    pub(crate) user_boundary: UserBoundary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PolicyReason {
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationConstraint {
    pub action_id: String,
    pub decision: AuthorizationDecision,
    pub reasons: Vec<PolicyReason>,
    pub effective_permissions: BTreeSet<String>,
    pub obligations: BTreeSet<String>,
    pub evaluated_at_ms: i64,
    pub policy_revision: u64,
}

#[derive(Debug, Clone)]
pub struct PolicyAgent {
    state: PolicyAgentState,
    policy_revision: u64,
    evaluated_count: u64,
    last_decision: Option<AuthorizationConstraint>,
}

impl Default for PolicyAgent {
    fn default() -> Self {
        Self {
            state: PolicyAgentState::Ready,
            policy_revision: 1,
            evaluated_count: 0,
            last_decision: None,
        }
    }
}

impl PolicyAgent {
    pub fn state(&self) -> PolicyAgentState {
        self.state
    }

    pub fn evaluated_count(&self) -> u64 {
        self.evaluated_count
    }

    pub fn last_decision(&self) -> Option<&AuthorizationConstraint> {
        self.last_decision.as_ref()
    }

    /// 单调策略：硬安全/显式边界的拒绝优先级最高；确认只能解除确认门，
    /// 不能覆盖权限缺失、显式拒绝或安全不变量。
    pub fn evaluate(&mut self, input: PolicyEvaluationInput) -> AuthorizationConstraint {
        self.state = PolicyAgentState::Evaluating;
        let mut deny_reasons = Vec::new();
        let mut confirmation_reasons = Vec::new();
        let mut obligations = BTreeSet::from(["append_authorization_audit".to_string()]);

        let action = &input.action;
        if input.now_ms < 0
            || action.action_id.trim().is_empty()
            || action.actor_id.trim().is_empty()
            || action.capability_id.trim().is_empty()
            || action.target_scope.trim().is_empty()
            || action
                .required_permissions
                .iter()
                .any(|permission| permission.trim().is_empty())
        {
            deny_reasons.push(reason(
                "invalid_action_contract",
                "动作、执行者、能力和目标 scope 必须明确",
            ));
        }
        for invariant in &action.safety_invariant_violations {
            deny_reasons.push(reason(
                "safety_invariant_violation",
                format!("不可绕过的安全不变量被触发：{invariant}"),
            ));
        }
        if input
            .user_boundary
            .denied_capability_ids
            .contains(&action.capability_id)
        {
            deny_reasons.push(reason(
                "capability_outside_user_boundary",
                "用户边界明确禁止该能力",
            ));
        }
        if !input.user_boundary.allowed_scope_prefixes.is_empty()
            && !input
                .user_boundary
                .allowed_scope_prefixes
                .iter()
                .any(|prefix| scope_matches(&action.target_scope, prefix))
        {
            deny_reasons.push(reason(
                "target_outside_user_boundary",
                "动作目标不在用户允许的 scope 内",
            ));
        }
        for effect in action
            .effects
            .intersection(&input.user_boundary.denied_effects)
        {
            deny_reasons.push(reason(
                "effect_outside_user_boundary",
                format!("用户边界禁止 {effect:?} 副作用"),
            ));
        }

        let mut effective_permissions = BTreeSet::new();
        for permission in &action.required_permissions {
            if input.explicitly_denied_permissions.contains(permission) {
                deny_reasons.push(reason(
                    "permission_explicitly_denied",
                    format!("权限 {permission} 已被显式拒绝"),
                ));
                continue;
            }
            let granted = input.permission_grants.iter().any(|grant| {
                grant.permission_id == *permission
                    && grant
                        .expires_at_ms
                        .is_none_or(|expires_at_ms| expires_at_ms > input.now_ms)
                    && (grant.scope_prefixes.is_empty()
                        || grant
                            .scope_prefixes
                            .iter()
                            .any(|prefix| scope_matches(&action.target_scope, prefix)))
            });
            if granted {
                effective_permissions.insert(permission.clone());
            } else {
                deny_reasons.push(reason(
                    "permission_not_granted",
                    format!("权限 {permission} 未授权、已过期或 scope 不匹配"),
                ));
            }
        }

        let already_confirmed = input.confirmations.iter().any(|confirmation| {
            confirmation.action == *action
                && confirmation
                    .expires_at_ms
                    .is_none_or(|expires_at_ms| expires_at_ms > input.now_ms)
        });
        let needs_confirmation = action.risk != PolicyRisk::Routine
            || !action.reversible
            || action
                .effects
                .contains(&PolicyEffect::ExternalCommunication)
            || action.effects.contains(&PolicyEffect::CredentialAccess)
            || action.effects.contains(&PolicyEffect::Destructive)
            || action
                .effects
                .iter()
                .any(|effect| input.user_boundary.always_confirm_effects.contains(effect));
        if needs_confirmation && !already_confirmed {
            confirmation_reasons.push(reason(
                "explicit_user_confirmation_required",
                "风险、不可逆或越出本机的副作用需要绑定 action_id 的确认",
            ));
            obligations.insert(format!("confirm_action:{}", action.action_id));
        }
        if action.effects.contains(&PolicyEffect::CredentialAccess) {
            obligations.insert("redact_credentials_from_logs".to_string());
        }
        if action.effects.contains(&PolicyEffect::DeviceControl) {
            obligations.insert("verify_device_state_after_action".to_string());
        }

        let (decision, reasons) = if !deny_reasons.is_empty() {
            (AuthorizationDecision::Deny, deny_reasons)
        } else if !confirmation_reasons.is_empty() {
            (
                AuthorizationDecision::RequireConfirmation,
                confirmation_reasons,
            )
        } else {
            (
                AuthorizationDecision::Allow,
                vec![reason(
                    "policy_rules_satisfied",
                    "权限、安全不变量和用户边界均满足",
                )],
            )
        };

        self.evaluated_count = self.evaluated_count.saturating_add(1);
        self.state = PolicyAgentState::Ready;
        let constraint = AuthorizationConstraint {
            action_id: action.action_id.clone(),
            decision,
            reasons,
            effective_permissions,
            obligations,
            evaluated_at_ms: input.now_ms,
            policy_revision: self.policy_revision,
        };
        self.last_decision = Some(constraint.clone());
        constraint
    }
}

pub fn policy_authorize_contract() -> CapabilityContract {
    CapabilityContract {
        capability_id: POLICY_AUTHORIZE_CAPABILITY_ID.to_string(),
        version: 1,
        summary: "把权限、安全不变量和用户边界投影为 allow/deny/require_confirmation 约束"
            .to_string(),
        input_schema: json!({
            "type": "object",
            "required": ["nowMs", "action"],
            "properties": {
                "nowMs": { "type": "integer" },
                "action": { "type": "object" }
            }
        }),
        output_schema: json!({
            "type": "object",
            "required": ["actionId", "decision", "reasons", "obligations", "policyRevision"],
            "properties": {
                "decision": { "enum": ["allow", "deny", "require_confirmation"] },
                "reasons": { "type": "array" },
                "effectivePermissions": { "type": "array" },
                "obligations": { "type": "array" }
            }
        }),
        preconditions: Vec::new(),
        permissions: Vec::new(),
        side_effects: vec!["authorization_audit".to_string()],
        resource_class: ResourceClass::Light,
        interruptibility: Interruptibility::Immediate,
        idempotent: false,
    }
}

fn reason(code: impl Into<String>, detail: impl Into<String>) -> PolicyReason {
    PolicyReason {
        code: code.into(),
        detail: detail.into(),
    }
}

fn scope_matches(scope: &str, prefix: &str) -> bool {
    let scope = scope.trim();
    let prefix = prefix.trim().trim_end_matches('.');
    !prefix.is_empty()
        && (scope == prefix
            || scope
                .strip_prefix(prefix)
                .is_some_and(|remainder| remainder.starts_with('.')))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(action: ProposedAction) -> PolicyEvaluationInput {
        PolicyEvaluationInput {
            now_ms: 100,
            action,
            permission_grants: vec![PermissionGrant {
                permission_id: "screen.read".to_string(),
                scope_prefixes: vec!["desktop".to_string()],
                expires_at_ms: Some(200),
            }],
            explicitly_denied_permissions: BTreeSet::new(),
            confirmations: Vec::new(),
            user_boundary: UserBoundary {
                allowed_scope_prefixes: vec!["desktop".to_string()],
                ..UserBoundary::default()
            },
        }
    }

    fn read_action() -> ProposedAction {
        ProposedAction {
            action_id: "action-read".to_string(),
            actor_id: "agent:screen-observer".to_string(),
            capability_id: "screen.observe".to_string(),
            target_scope: "desktop.primary".to_string(),
            required_permissions: BTreeSet::from(["screen.read".to_string()]),
            effects: BTreeSet::from([PolicyEffect::ReadOnly]),
            risk: PolicyRisk::Routine,
            reversible: true,
            safety_invariant_violations: Vec::new(),
        }
    }

    #[test]
    fn routine_in_scope_action_is_allowed() {
        let mut agent = PolicyAgent::default();
        let decision = agent.evaluate(input(read_action()));
        assert_eq!(decision.decision, AuthorizationDecision::Allow);
        assert_eq!(agent.state(), PolicyAgentState::Ready);
        assert_eq!(agent.evaluated_count(), 1);
    }

    #[test]
    fn external_effect_requires_action_bound_confirmation() {
        let mut agent = PolicyAgent::default();
        let mut action = read_action();
        action.action_id = "action-send".to_string();
        action.effects = BTreeSet::from([PolicyEffect::ExternalCommunication]);
        action.risk = PolicyRisk::Elevated;
        let mut request = input(action);

        let first = agent.evaluate(request.clone());
        assert_eq!(first.decision, AuthorizationDecision::RequireConfirmation);
        request.confirmations.push(ActionConfirmation {
            action: request.action.clone(),
            expires_at_ms: Some(200),
        });
        let confirmed = agent.evaluate(request);
        assert_eq!(confirmed.decision, AuthorizationDecision::Allow);
    }

    #[test]
    fn confirmation_cannot_be_reused_after_action_content_changes() {
        let mut agent = PolicyAgent::default();
        let mut action = read_action();
        action.action_id = "action-send".to_string();
        action.effects = BTreeSet::from([PolicyEffect::ExternalCommunication]);
        let confirmation = ActionConfirmation {
            action: action.clone(),
            expires_at_ms: Some(200),
        };
        action.target_scope = "desktop.secondary".to_string();
        let mut request = input(action);
        request.confirmations.push(confirmation);

        let decision = agent.evaluate(request);
        assert_eq!(
            decision.decision,
            AuthorizationDecision::RequireConfirmation
        );
    }

    #[test]
    fn confirmation_never_overrides_hard_safety_or_permission_denial() {
        let mut agent = PolicyAgent::default();
        let mut action = read_action();
        action.safety_invariant_violations = vec!["device.temperature.max".to_string()];
        let mut request = input(action);
        request.confirmations.push(ActionConfirmation {
            action: request.action.clone(),
            expires_at_ms: None,
        });
        request
            .explicitly_denied_permissions
            .insert("screen.read".to_string());

        let decision = agent.evaluate(request);
        assert_eq!(decision.decision, AuthorizationDecision::Deny);
        assert!(decision
            .reasons
            .iter()
            .any(|reason| reason.code == "safety_invariant_violation"));
        assert!(decision
            .reasons
            .iter()
            .any(|reason| reason.code == "permission_explicitly_denied"));
    }
}

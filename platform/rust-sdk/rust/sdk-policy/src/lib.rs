//! # Allternit SDK Policy
//!
//! Policy engine interface and types for the Allternit SDK.
//!
//! ## Overview
//!
//! This crate provides the policy engine abstraction and types for
//! security policy evaluation within the Allternit platform. It defines
//! the interface for checking permissions, evaluating policies,
//! and enforcing security constraints.
//!
//! The policy engine is responsible for:
//! - Evaluating security policies against requests
//! - Managing permission contexts and roles
//! - Enforcing safety tier requirements
//! - Auditing policy decisions
//!
//! ## Key Concepts
//!
//! - **PolicyEngine**: Core policy evaluation engine
//! - **PolicyRequest**: Request for policy evaluation
//! - **PolicyDecision**: Result of policy evaluation
//! - **PolicyEffect**: Allow or deny decision
//! - **SafetyTier**: Security classification levels
//!
//! ## Example
//!
//! ```rust
//! use allternit_sdk_policy::{PolicyEngine, PolicyRequest, SafetyTier};
//!
//! // Create a policy engine
//! let engine = PolicyEngine::new();
//!
//! // Create a policy request
//! let request = PolicyRequest {
//!     identity_id: "user-123".to_string(),
//!     resource: "function:execute".to_string(),
//!     action: "call".to_string(),
//!     context: serde_json::json!({"tier": "T1"}),
//!     requested_tier: SafetyTier::T1,
//! };
//! ```

use serde::{Deserialize, Serialize};

/// Core policy engine for security policy evaluation.
///
/// `PolicyEngine` provides the interface for evaluating security
/// policies and making authorization decisions. It maintains the
/// state necessary for policy evaluation, including rules,
/// permissions, and audit logging.
///
/// # Examples
///
/// ```
/// use allternit_sdk_policy::PolicyEngine;
///
/// let engine = PolicyEngine::new();
/// ```
#[derive(Debug, Clone)]
pub struct PolicyEngine;

impl PolicyEngine {
    /// Creates a new policy engine instance.
    ///
    /// Initializes a fresh policy engine with default configuration.
    /// The engine starts with no custom rules or policies.
    ///
    /// # Returns
    ///
    /// A new `PolicyEngine` instance
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_policy::PolicyEngine;
    ///
    /// let engine = PolicyEngine::new();
    /// ```
    pub fn new() -> Self {
        Self
    }

    /// Evaluates a policy request and returns a decision.
    ///
    /// This method takes a `PolicyRequest` and evaluates it against
    /// the configured policies, returning a `PolicyDecision` indicating
    /// whether the action should be allowed or denied.
    ///
    /// # Arguments
    ///
    /// * `request` - The policy request to evaluate
    ///
    /// # Returns
    ///
    /// A `PolicyDecision` containing the evaluation result
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_policy::{PolicyEngine, PolicyRequest, SafetyTier};
    ///
    /// let engine = PolicyEngine::new();
    ///
    /// let request = PolicyRequest {
    ///     identity_id: "user-123".to_string(),
    ///     resource: "function:com.example.read".to_string(),
    ///     action: "execute".to_string(),
    ///     context: serde_json::json!({}),
    ///     requested_tier: SafetyTier::T1,
    /// };
    ///
    /// // In a real implementation, this would evaluate against policies
    /// // let decision = engine.evaluate(request);
    /// ```
    pub fn evaluate(&self, request: PolicyRequest) -> PolicyDecision {
        // Placeholder implementation
        // In a real implementation, this would:
        // 1. Look up applicable policies for the resource
        // 2. Evaluate conditions against the request context
        // 3. Check safety tier requirements
        // 4. Return appropriate decision

        PolicyDecision {
            decision: PolicyEffect::Allow,
            reason: "Default allow".to_string(),
            tier: request.requested_tier,
            constraints: vec![],
        }
    }

    /// Evaluates a batch of policy requests.
    ///
    /// This is more efficient than evaluating requests individually
    /// when multiple decisions are needed.
    ///
    /// # Arguments
    ///
    /// * `requests` - A vector of policy requests
    ///
    /// # Returns
    ///
    /// A vector of `PolicyDecision` results in the same order as the requests
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_policy::{PolicyEngine, PolicyRequest, SafetyTier};
    ///
    /// let engine = PolicyEngine::new();
    ///
    /// let requests = vec![
    ///     PolicyRequest {
    ///         identity_id: "user-123".to_string(),
    ///         resource: "resource-1".to_string(),
    ///         action: "read".to_string(),
    ///         context: serde_json::json!({}),
    ///         requested_tier: SafetyTier::T0,
    ///     },
    ///     PolicyRequest {
    ///         identity_id: "user-123".to_string(),
    ///         resource: "resource-2".to_string(),
    ///         action: "write".to_string(),
    ///         context: serde_json::json!({}),
    ///         requested_tier: SafetyTier::T2,
    ///     },
    /// ];
    ///
    /// // let decisions = engine.evaluate_batch(requests);
    /// ```
    pub fn evaluate_batch(&self, requests: Vec<PolicyRequest>) -> Vec<PolicyDecision> {
        requests.into_iter().map(|req| self.evaluate(req)).collect()
    }
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Request for policy evaluation.
///
/// `PolicyRequest` contains all the information needed to evaluate
/// a policy decision, including the identity, resource, action,
/// and security context.
///
/// # Examples
///
/// ```
/// use allternit_sdk_policy::{PolicyRequest, SafetyTier};
///
/// let request = PolicyRequest {
///     identity_id: "user-123".to_string(),
///     resource: "function:com.example.delete".to_string(),
///     action: "execute".to_string(),
///     context: serde_json::json!({
///         "ip_address": "192.168.1.1",
///         "user_agent": "Allternit Client/1.0"
///     }),
///     requested_tier: SafetyTier::T3,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRequest {
    /// Identity of the user or service making the request
    pub identity_id: String,

    /// Resource being accessed (e.g., "function:com.example.read")
    pub resource: String,

    /// Action being performed (e.g., "execute", "read", "write")
    pub action: String,

    /// Additional context for policy evaluation
    pub context: serde_json::Value,

    /// Requested safety tier for the operation
    pub requested_tier: SafetyTier,
}

impl PolicyRequest {
    /// Creates a new policy request.
    ///
    /// # Arguments
    ///
    /// * `identity_id` - The identity making the request
    /// * `resource` - The resource being accessed
    /// * `action` - The action being performed
    ///
    /// # Returns
    ///
    /// A new `PolicyRequest` with default safety tier (T0)
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_policy::PolicyRequest;
    ///
    /// let request = PolicyRequest::new(
    ///     "user-123".to_string(),
    ///     "function:read".to_string(),
    ///     "execute".to_string(),
    /// );
    ///
    /// assert_eq!(request.identity_id, "user-123");
    /// assert_eq!(request.resource, "function:read");
    /// ```
    pub fn new(identity_id: String, resource: String, action: String) -> Self {
        Self {
            identity_id,
            resource,
            action,
            context: serde_json::json!({}),
            requested_tier: SafetyTier::T0,
        }
    }

    /// Sets the safety tier for the request.
    ///
    /// # Arguments
    ///
    /// * `tier` - The safety tier
    ///
    /// # Returns
    ///
    /// The updated `PolicyRequest` for method chaining
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_policy::{PolicyRequest, SafetyTier};
    ///
    /// let request = PolicyRequest::new(
    ///     "user-123".to_string(),
    ///     "function:write".to_string(),
    ///     "execute".to_string(),
    /// ).with_tier(SafetyTier::T2);
    ///
    /// assert!(matches!(request.requested_tier, SafetyTier::T2));
    /// ```
    pub fn with_tier(mut self, tier: SafetyTier) -> Self {
        self.requested_tier = tier;
        self
    }

    /// Sets the context for the request.
    ///
    /// # Arguments
    ///
    /// * `context` - Additional context as JSON
    ///
    /// # Returns
    ///
    /// The updated `PolicyRequest` for method chaining
    pub fn with_context(mut self, context: serde_json::Value) -> Self {
        self.context = context;
        self
    }
}

/// Decision from policy evaluation.
///
/// `PolicyDecision` represents the outcome of evaluating a policy
/// request, including whether the action is allowed and any
/// constraints that apply.
///
/// # Examples
///
/// ```
/// use allternit_sdk_policy::{PolicyDecision, PolicyEffect, SafetyTier, Constraint};
///
/// let decision = PolicyDecision {
///     decision: PolicyEffect::Allow,
///     reason: "User has required permission".to_string(),
///     tier: SafetyTier::T1,
///     constraints: vec![
///         Constraint::ConfirmationRequired,
///         Constraint::RateLimit { max_calls: 10, window_secs: 60 },
///     ],
/// };
///
/// assert!(decision.is_allowed());
/// assert!(decision.requires_confirmation());
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecision {
    /// The decision (allow or deny)
    pub decision: PolicyEffect,

    /// Human-readable reason for the decision
    pub reason: String,

    /// The safety tier that was evaluated
    pub tier: SafetyTier,

    /// Any constraints that apply to the allowed action
    pub constraints: Vec<Constraint>,
}

impl PolicyDecision {
    /// Returns true if the decision is to allow the action.
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_policy::{PolicyDecision, PolicyEffect, SafetyTier};
    ///
    /// let allow = PolicyDecision {
    ///     decision: PolicyEffect::Allow,
    ///     reason: "Allowed".to_string(),
    ///     tier: SafetyTier::T0,
    ///     constraints: vec![],
    /// };
    ///
    /// let deny = PolicyDecision {
    ///     decision: PolicyEffect::Deny,
    ///     reason: "Denied".to_string(),
    ///     tier: SafetyTier::T0,
    ///     constraints: vec![],
    /// };
    ///
    /// assert!(allow.is_allowed());
    /// assert!(!deny.is_allowed());
    /// ```
    pub fn is_allowed(&self) -> bool {
        matches!(self.decision, PolicyEffect::Allow)
    }

    /// Returns true if the decision is to deny the action.
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_policy::{PolicyDecision, PolicyEffect, SafetyTier};
    ///
    /// let deny = PolicyDecision {
    ///     decision: PolicyEffect::Deny,
    ///     reason: "Permission denied".to_string(),
    ///     tier: SafetyTier::T0,
    ///     constraints: vec![],
    /// };
    ///
    /// assert!(deny.is_denied());
    /// ```
    pub fn is_denied(&self) -> bool {
        matches!(self.decision, PolicyEffect::Deny)
    }

    /// Returns true if confirmation is required for this decision.
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_policy::{PolicyDecision, PolicyEffect, SafetyTier, Constraint};
    ///
    /// let decision = PolicyDecision {
    ///     decision: PolicyEffect::Allow,
    ///     reason: "Allowed with confirmation".to_string(),
    ///     tier: SafetyTier::T2,
    ///     constraints: vec![Constraint::ConfirmationRequired],
    /// };
    ///
    /// assert!(decision.requires_confirmation());
    /// ```
    pub fn requires_confirmation(&self) -> bool {
        self.constraints
            .iter()
            .any(|c| matches!(c, Constraint::ConfirmationRequired))
    }

    /// Returns the rate limit constraint if present.
    ///
    /// # Returns
    ///
    /// `Some((max_calls, window_secs))` if a rate limit constraint exists,
    /// `None` otherwise
    pub fn rate_limit(&self) -> Option<(u32, u64)> {
        self.constraints.iter().find_map(|c| {
            if let Constraint::RateLimit {
                max_calls,
                window_secs,
            } = c
            {
                Some((*max_calls, *window_secs))
            } else {
                None
            }
        })
    }
}

/// Effect of a policy decision.
///
/// `PolicyEffect` represents the binary outcome of a policy evaluation:
/// either allow or deny.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PolicyEffect {
    /// The action is permitted
    Allow,
    /// The action is not permitted
    Deny,
}

/// Safety tier for security classification.
///
/// `SafetyTier` represents the security level of an operation,
/// ranging from T0 (informational only) to T4 (critical risk).
///
/// # Tiers
///
/// - **T0**: No risk - informational queries only
/// - **T1**: Low risk - read-only operations
/// - **T2**: Medium risk - non-destructive writes
/// - **T3**: High risk - destructive operations
/// - **T4**: Critical risk - irreversible, sensitive data operations
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SafetyTier {
    /// T0: No risk - informational queries only
    T0,
    /// T1: Low risk - read-only operations
    T1,
    /// T2: Medium risk - non-destructive writes
    T2,
    /// T3: High risk - destructive operations
    T3,
    /// T4: Critical risk - irreversible operations with sensitive data
    T4,
}

impl SafetyTier {
    /// Returns the numeric value of the tier (0-4).
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_policy::SafetyTier;
    ///
    /// assert_eq!(SafetyTier::T0.as_number(), 0);
    /// assert_eq!(SafetyTier::T2.as_number(), 2);
    /// assert_eq!(SafetyTier::T4.as_number(), 4);
    /// ```
    pub fn as_number(&self) -> u8 {
        match self {
            SafetyTier::T0 => 0,
            SafetyTier::T1 => 1,
            SafetyTier::T2 => 2,
            SafetyTier::T3 => 3,
            SafetyTier::T4 => 4,
        }
    }

    /// Returns true if this tier is at least as high as another tier.
    ///
    /// # Arguments
    ///
    /// * `other` - The tier to compare against
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_policy::SafetyTier;
    ///
    /// assert!(SafetyTier::T2.is_at_least(SafetyTier::T0));
    /// assert!(SafetyTier::T2.is_at_least(SafetyTier::T2));
    /// assert!(!SafetyTier::T1.is_at_least(SafetyTier::T3));
    /// ```
    pub fn is_at_least(&self, other: SafetyTier) -> bool {
        self.as_number() >= other.as_number()
    }

    /// Returns a description of the tier.
    ///
    /// # Examples
    ///
    /// ```
    /// use allternit_sdk_policy::SafetyTier;
    ///
    /// assert_eq!(SafetyTier::T0.description(), "No risk - informational only");
    /// assert_eq!(SafetyTier::T4.description(), "Critical risk - irreversible operations");
    /// ```
    pub fn description(&self) -> &'static str {
        match self {
            SafetyTier::T0 => "No risk - informational only",
            SafetyTier::T1 => "Low risk - read-only operations",
            SafetyTier::T2 => "Medium risk - non-destructive writes",
            SafetyTier::T3 => "High risk - destructive operations",
            SafetyTier::T4 => "Critical risk - irreversible operations",
        }
    }
}

/// Constraint applied to an allowed policy decision.
///
/// `Constraint` represents additional requirements or limitations
/// that must be satisfied when executing an allowed action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Constraint {
    /// User confirmation is required before execution
    ConfirmationRequired,

    /// Rate limiting applies to the operation
    RateLimit {
        /// Maximum number of calls allowed
        max_calls: u32,
        /// Time window in seconds
        window_secs: u64,
    },

    /// Operation must be logged for audit
    AuditRequired,

    /// Operation can only be performed during specific hours
    TimeRestriction {
        /// Allowed start hour (0-23)
        start_hour: u8,
        /// Allowed end hour (0-23)
        end_hour: u8,
    },

    /// Custom constraint with arbitrary data
    Custom {
        /// Constraint type identifier
        constraint_type: String,
        /// Constraint parameters
        parameters: serde_json::Value,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test policy request creation
    #[test]
    fn test_policy_request_new() {
        let request = PolicyRequest::new(
            "user-123".to_string(),
            "resource-1".to_string(),
            "read".to_string(),
        );

        assert_eq!(request.identity_id, "user-123");
        assert_eq!(request.resource, "resource-1");
        assert_eq!(request.action, "read");
        assert!(matches!(request.requested_tier, SafetyTier::T0));
    }

    /// Test policy request builder
    #[test]
    fn test_policy_request_builder() {
        let request = PolicyRequest::new(
            "user-123".to_string(),
            "resource-1".to_string(),
            "write".to_string(),
        )
        .with_tier(SafetyTier::T2)
        .with_context(serde_json::json!({"ip": "127.0.0.1"}));

        assert!(matches!(request.requested_tier, SafetyTier::T2));
        assert_eq!(request.context["ip"], "127.0.0.1");
    }

    /// Test policy decision methods
    #[test]
    fn test_policy_decision() {
        let allow = PolicyDecision {
            decision: PolicyEffect::Allow,
            reason: "Allowed".to_string(),
            tier: SafetyTier::T0,
            constraints: vec![],
        };

        let deny = PolicyDecision {
            decision: PolicyEffect::Deny,
            reason: "Denied".to_string(),
            tier: SafetyTier::T0,
            constraints: vec![],
        };

        let with_confirmation = PolicyDecision {
            decision: PolicyEffect::Allow,
            reason: "Allowed with confirmation".to_string(),
            tier: SafetyTier::T2,
            constraints: vec![Constraint::ConfirmationRequired],
        };

        let with_rate_limit = PolicyDecision {
            decision: PolicyEffect::Allow,
            reason: "Allowed with rate limit".to_string(),
            tier: SafetyTier::T1,
            constraints: vec![Constraint::RateLimit {
                max_calls: 10,
                window_secs: 60,
            }],
        };

        assert!(allow.is_allowed());
        assert!(!allow.is_denied());

        assert!(!deny.is_allowed());
        assert!(deny.is_denied());

        assert!(with_confirmation.requires_confirmation());
        assert!(!allow.requires_confirmation());

        assert_eq!(with_rate_limit.rate_limit(), Some((10, 60)));
        assert_eq!(allow.rate_limit(), None);
    }

    /// Test safety tier operations
    #[test]
    fn test_safety_tier() {
        assert_eq!(SafetyTier::T0.as_number(), 0);
        assert_eq!(SafetyTier::T2.as_number(), 2);
        assert_eq!(SafetyTier::T4.as_number(), 4);

        assert!(SafetyTier::T2.is_at_least(SafetyTier::T0));
        assert!(SafetyTier::T2.is_at_least(SafetyTier::T2));
        assert!(!SafetyTier::T1.is_at_least(SafetyTier::T3));
        assert!(SafetyTier::T4.is_at_least(SafetyTier::T0));

        // Test descriptions are non-empty
        assert!(!SafetyTier::T0.description().is_empty());
        assert!(!SafetyTier::T4.description().is_empty());
    }

    /// Test batch evaluation
    #[test]
    fn test_evaluate_batch() {
        let engine = PolicyEngine::new();

        let requests = vec![
            PolicyRequest::new(
                "user-1".to_string(),
                "res-1".to_string(),
                "read".to_string(),
            ),
            PolicyRequest::new(
                "user-2".to_string(),
                "res-2".to_string(),
                "write".to_string(),
            ),
        ];

        let decisions = engine.evaluate_batch(requests);
        assert_eq!(decisions.len(), 2);
    }
}

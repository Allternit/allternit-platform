#![allow(clippy::redundant_closure_call)]
#![allow(clippy::needless_lifetimes)]
#![allow(clippy::match_single_binding)]
#![allow(clippy::clone_on_copy)]

#[doc = "`AbiEnvelopeV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AbiEnvelopeV1 {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: AbiEnvelopeV1SchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`AbiEnvelopeV1SchemaId`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct AbiEnvelopeV1SchemaId(::std::string::String);
impl ::std::ops::Deref for AbiEnvelopeV1SchemaId {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<AbiEnvelopeV1SchemaId> for ::std::string::String {
    fn from(value: AbiEnvelopeV1SchemaId) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for AbiEnvelopeV1SchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^allternit\\.kernel\\.[A-Za-z]+V\\d+$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^allternit\\.kernel\\.[A-Za-z]+V\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for AbiEnvelopeV1SchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AbiEnvelopeV1SchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for AbiEnvelopeV1SchemaId {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`AbiVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct AbiVersion(::std::string::String);
impl ::std::ops::Deref for AbiVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<AbiVersion> for ::std::string::String {
    fn from(value: AbiVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for AbiVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for AbiVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AbiVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for AbiVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ActionReceiptV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ActionReceiptV1 {
    pub action_id: Id,
    pub chain: ReceiptChainV1,
    pub effect_class: ActionReceiptV1EffectClass,
    pub envelope: ActionReceiptV1Envelope,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub external_ref: ::std::option::Option<::std::string::String>,
    pub idempotency_key: ActionReceiptV1IdempotencyKey,
    pub policy_decision_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub result_hash: ::std::option::Option<ContentHash>,
    pub status: ActionReceiptV1Status,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub target: ::std::option::Option<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub tool_receipt_id: ::std::option::Option<Id>,
}
#[doc = "`ActionReceiptV1EffectClass`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ActionReceiptV1EffectClass {
    #[serde(rename = "NONE")]
    None,
    #[serde(rename = "READ")]
    Read,
    #[serde(rename = "WORKSPACE_WRITE")]
    WorkspaceWrite,
    #[serde(rename = "EXECUTE")]
    Execute,
    #[serde(rename = "NETWORK")]
    Network,
    #[serde(rename = "EXTERNAL_WRITE")]
    ExternalWrite,
    #[serde(rename = "FINANCIAL")]
    Financial,
    #[serde(rename = "PUBLISH")]
    Publish,
    #[serde(rename = "PERMISSION_CHANGE")]
    PermissionChange,
}
impl ::std::fmt::Display for ActionReceiptV1EffectClass {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::None => f.write_str("NONE"),
            Self::Read => f.write_str("READ"),
            Self::WorkspaceWrite => f.write_str("WORKSPACE_WRITE"),
            Self::Execute => f.write_str("EXECUTE"),
            Self::Network => f.write_str("NETWORK"),
            Self::ExternalWrite => f.write_str("EXTERNAL_WRITE"),
            Self::Financial => f.write_str("FINANCIAL"),
            Self::Publish => f.write_str("PUBLISH"),
            Self::PermissionChange => f.write_str("PERMISSION_CHANGE"),
        }
    }
}
impl ::std::str::FromStr for ActionReceiptV1EffectClass {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "NONE" => Ok(Self::None),
            "READ" => Ok(Self::Read),
            "WORKSPACE_WRITE" => Ok(Self::WorkspaceWrite),
            "EXECUTE" => Ok(Self::Execute),
            "NETWORK" => Ok(Self::Network),
            "EXTERNAL_WRITE" => Ok(Self::ExternalWrite),
            "FINANCIAL" => Ok(Self::Financial),
            "PUBLISH" => Ok(Self::Publish),
            "PERMISSION_CHANGE" => Ok(Self::PermissionChange),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ActionReceiptV1EffectClass {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ActionReceiptV1EffectClass {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ActionReceiptV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ActionReceiptV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: ActionReceiptV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`ActionReceiptV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ActionReceiptV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.ActionReceiptV1")]
    AllternitKernelActionReceiptV1,
}
impl ::std::fmt::Display for ActionReceiptV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelActionReceiptV1 => f.write_str("allternit.kernel.ActionReceiptV1"),
        }
    }
}
impl ::std::str::FromStr for ActionReceiptV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.ActionReceiptV1" => Ok(Self::AllternitKernelActionReceiptV1),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ActionReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ActionReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ActionReceiptV1IdempotencyKey`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ActionReceiptV1IdempotencyKey(::std::string::String);
impl ::std::ops::Deref for ActionReceiptV1IdempotencyKey {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ActionReceiptV1IdempotencyKey> for ::std::string::String {
    fn from(value: ActionReceiptV1IdempotencyKey) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ActionReceiptV1IdempotencyKey {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 8usize {
            return Err("shorter than 8 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ActionReceiptV1IdempotencyKey {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ActionReceiptV1IdempotencyKey {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ActionReceiptV1IdempotencyKey {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ActionReceiptV1Status`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ActionReceiptV1Status {
    #[serde(rename = "INTENDED")]
    Intended,
    #[serde(rename = "COMMITTED")]
    Committed,
    #[serde(rename = "FAILED")]
    Failed,
    #[serde(rename = "COMPENSATED")]
    Compensated,
    #[serde(rename = "UNKNOWN")]
    Unknown,
}
impl ::std::fmt::Display for ActionReceiptV1Status {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Intended => f.write_str("INTENDED"),
            Self::Committed => f.write_str("COMMITTED"),
            Self::Failed => f.write_str("FAILED"),
            Self::Compensated => f.write_str("COMPENSATED"),
            Self::Unknown => f.write_str("UNKNOWN"),
        }
    }
}
impl ::std::str::FromStr for ActionReceiptV1Status {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "INTENDED" => Ok(Self::Intended),
            "COMMITTED" => Ok(Self::Committed),
            "FAILED" => Ok(Self::Failed),
            "COMPENSATED" => Ok(Self::Compensated),
            "UNKNOWN" => Ok(Self::Unknown),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ActionReceiptV1Status {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ActionReceiptV1Status {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AdmissionDecisionV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AdmissionDecisionV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub cap_in_use: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub cap_limit: ::std::option::Option<u64>,
    pub decision: AdmissionDecisionV1Decision,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub probe_id: ::std::option::Option<Id>,
    pub reason_codes: ::std::vec::Vec<::std::string::String>,
    pub request_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub retry_after_ms: ::std::option::Option<u64>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: AdmissionDecisionV1SchemaVersion,
    pub spawn_cap_key: ::std::string::String,
}
#[doc = "`AdmissionDecisionV1Decision`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AdmissionDecisionV1Decision {
    #[serde(rename = "ADMIT")]
    Admit,
    #[serde(rename = "QUEUE")]
    Queue,
    #[serde(rename = "REJECT")]
    Reject,
}
impl ::std::fmt::Display for AdmissionDecisionV1Decision {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Admit => f.write_str("ADMIT"),
            Self::Queue => f.write_str("QUEUE"),
            Self::Reject => f.write_str("REJECT"),
        }
    }
}
impl ::std::str::FromStr for AdmissionDecisionV1Decision {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "ADMIT" => Ok(Self::Admit),
            "QUEUE" => Ok(Self::Queue),
            "REJECT" => Ok(Self::Reject),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AdmissionDecisionV1Decision {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AdmissionDecisionV1Decision {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AdmissionDecisionV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct AdmissionDecisionV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for AdmissionDecisionV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<AdmissionDecisionV1SchemaVersion> for ::std::string::String {
    fn from(value: AdmissionDecisionV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for AdmissionDecisionV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for AdmissionDecisionV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AdmissionDecisionV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for AdmissionDecisionV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`AdmissionRequestV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AdmissionRequestV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub budget: ::std::option::Option<ResourceBudget>,
    pub executor_class: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub node_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub priority: ::std::option::Option<i64>,
    pub request_id: Id,
    pub requested_scope: ::std::vec::Vec<ResourceRef>,
    pub run_id: Id,
    pub schema_id: ::serde_json::Value,
    pub schema_version: AdmissionRequestV1SchemaVersion,
}
#[doc = "`AdmissionRequestV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct AdmissionRequestV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for AdmissionRequestV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<AdmissionRequestV1SchemaVersion> for ::std::string::String {
    fn from(value: AdmissionRequestV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for AdmissionRequestV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for AdmissionRequestV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AdmissionRequestV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for AdmissionRequestV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`AgentBundleManifestV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AgentBundleManifestV1 {
    pub bundle_ref: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub name: AgentBundleManifestV1Name,
    pub profiles: ::std::vec::Vec<AgentBundleManifestV1ProfilesItem>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: AgentBundleManifestV1SchemaVersion,
    pub stable: bool,
    pub version: AgentBundleManifestV1Version,
}
#[doc = "`AgentBundleManifestV1Name`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct AgentBundleManifestV1Name(::std::string::String);
impl ::std::ops::Deref for AgentBundleManifestV1Name {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<AgentBundleManifestV1Name> for ::std::string::String {
    fn from(value: AgentBundleManifestV1Name) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for AgentBundleManifestV1Name {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^[a-z0-9][a-z0-9-]*$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[a-z0-9][a-z0-9-]*$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for AgentBundleManifestV1Name {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AgentBundleManifestV1Name {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for AgentBundleManifestV1Name {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`AgentBundleManifestV1ProfilesItem`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AgentBundleManifestV1ProfilesItem {
    #[serde(rename = "read-only")]
    ReadOnly,
    #[serde(rename = "code-safe")]
    CodeSafe,
    #[serde(rename = "code-write")]
    CodeWrite,
}
impl ::std::fmt::Display for AgentBundleManifestV1ProfilesItem {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::ReadOnly => f.write_str("read-only"),
            Self::CodeSafe => f.write_str("code-safe"),
            Self::CodeWrite => f.write_str("code-write"),
        }
    }
}
impl ::std::str::FromStr for AgentBundleManifestV1ProfilesItem {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "read-only" => Ok(Self::ReadOnly),
            "code-safe" => Ok(Self::CodeSafe),
            "code-write" => Ok(Self::CodeWrite),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AgentBundleManifestV1ProfilesItem {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AgentBundleManifestV1ProfilesItem {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AgentBundleManifestV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct AgentBundleManifestV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for AgentBundleManifestV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<AgentBundleManifestV1SchemaVersion> for ::std::string::String {
    fn from(value: AgentBundleManifestV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for AgentBundleManifestV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for AgentBundleManifestV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AgentBundleManifestV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for AgentBundleManifestV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`AgentBundleManifestV1Version`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct AgentBundleManifestV1Version(::std::string::String);
impl ::std::ops::Deref for AgentBundleManifestV1Version {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<AgentBundleManifestV1Version> for ::std::string::String {
    fn from(value: AgentBundleManifestV1Version) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for AgentBundleManifestV1Version {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^v\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^v\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for AgentBundleManifestV1Version {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AgentBundleManifestV1Version {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for AgentBundleManifestV1Version {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`AgentStateV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AgentStateV1 {
    pub acceptance: ::std::vec::Vec<::serde_json::Map<::std::string::String, ::serde_json::Value>>,
    pub active_packs: ::std::vec::Vec<AgentStateV1ActivePacksItem>,
    pub budgets: ResourceBudget,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub candidate_targets:
        ::std::vec::Vec<::serde_json::Map<::std::string::String, ::serde_json::Value>>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub command_history: ::std::vec::Vec<Id>,
    pub completion: AgentStateV1Completion,
    pub constraints: ::std::vec::Vec<Id>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub context_index: ::std::vec::Vec<Id>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub diagnostics: ::std::vec::Vec<::serde_json::Map<::std::string::String, ::serde_json::Value>>,
    pub environment: AgentStateV1Environment,
    pub evidence: ::std::vec::Vec<EvidenceRef>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub generated_artifacts: ::std::vec::Vec<ArtifactRef>,
    pub graph_cursor: AgentStateV1GraphCursor,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub hypotheses: ::std::vec::Vec<AgentStateV1HypothesesItem>,
    pub identity: AgentStateV1Identity,
    pub instruction_predicates: ::std::vec::Vec<AgentStateV1InstructionPredicatesItem>,
    #[serde(default, skip_serializing_if = "::serde_json::Map::is_empty")]
    pub model_state: ::serde_json::Map<::std::string::String, ::serde_json::Value>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub mutations: ::std::vec::Vec<Id>,
    pub permissions: AgentStateV1Permissions,
    pub plan: ::serde_json::Map<::std::string::String, ::serde_json::Value>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub relevant_context: ::std::vec::Vec<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub repository:
        ::std::option::Option<::serde_json::Map<::std::string::String, ::serde_json::Value>>,
    pub retrieval_snapshot_refs: ::std::vec::Vec<AgentStateV1RetrievalSnapshotRefsItem>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: AgentStateV1SchemaVersion,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub selected_targets: ::std::vec::Vec<ResourceRef>,
    pub sensitivity_labels: ::std::vec::Vec<AgentStateV1SensitivityLabelsItem>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub subgoals: ::std::vec::Vec<::serde_json::Map<::std::string::String, ::serde_json::Value>>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub unresolved_failures:
        ::std::vec::Vec<::serde_json::Map<::std::string::String, ::serde_json::Value>>,
    pub user_intent: AgentStateV1UserIntent,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub user_visible_status: ::std::option::Option<::std::string::String>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub verification: ::std::vec::Vec<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_ref: ::std::option::Option<StateVersionRef>,
}
#[doc = "`AgentStateV1ActivePacksItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AgentStateV1ActivePacksItem {
    pub pack_id: Id,
    pub pack_version: SemVer,
}
#[doc = "`AgentStateV1Completion`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AgentStateV1Completion {
    pub coverage: ::std::vec::Vec<AgentStateV1CompletionCoverageItem>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub latest_decision_ref: ::std::option::Option<Id>,
}
#[doc = "`AgentStateV1CompletionCoverageItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AgentStateV1CompletionCoverageItem {
    pub criterion_id: Id,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub evidence_refs: ::std::vec::Vec<EvidenceRef>,
    pub satisfied: bool,
}
#[doc = "`AgentStateV1Environment`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AgentStateV1Environment {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub fingerprint: ::std::option::Option<ContentHash>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub languages: ::std::vec::Vec<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub repo_id: ::std::option::Option<::std::string::String>,
    pub workspace_root: ::std::string::String,
}
#[doc = "`AgentStateV1GraphCursor`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AgentStateV1GraphCursor {
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub completed_nodes: ::std::vec::Vec<Id>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub failed_nodes: ::std::vec::Vec<Id>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub frontier: ::std::vec::Vec<Id>,
    pub graph_id: Id,
    pub graph_version: u64,
}
#[doc = "`AgentStateV1HypothesesItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AgentStateV1HypothesesItem {
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub evidence_refs: ::std::vec::Vec<EvidenceRef>,
    pub hypothesis_id: Id,
    pub statement: ::std::string::String,
    pub status: AgentStateV1HypothesesItemStatus,
}
#[doc = "`AgentStateV1HypothesesItemStatus`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AgentStateV1HypothesesItemStatus {
    #[serde(rename = "OPEN")]
    Open,
    #[serde(rename = "SUPPORTED")]
    Supported,
    #[serde(rename = "REFUTED")]
    Refuted,
    #[serde(rename = "ABANDONED")]
    Abandoned,
}
impl ::std::fmt::Display for AgentStateV1HypothesesItemStatus {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Open => f.write_str("OPEN"),
            Self::Supported => f.write_str("SUPPORTED"),
            Self::Refuted => f.write_str("REFUTED"),
            Self::Abandoned => f.write_str("ABANDONED"),
        }
    }
}
impl ::std::str::FromStr for AgentStateV1HypothesesItemStatus {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "OPEN" => Ok(Self::Open),
            "SUPPORTED" => Ok(Self::Supported),
            "REFUTED" => Ok(Self::Refuted),
            "ABANDONED" => Ok(Self::Abandoned),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AgentStateV1HypothesesItemStatus {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AgentStateV1HypothesesItemStatus {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AgentStateV1Identity`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AgentStateV1Identity {
    pub run_id: Id,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
}
#[doc = "`AgentStateV1InstructionPredicatesItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AgentStateV1InstructionPredicatesItem {
    pub condition: ::std::string::String,
    pub instruction_ref: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub pinned: ::std::option::Option<bool>,
    pub predicate_id: Id,
}
#[doc = "`AgentStateV1Permissions`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AgentStateV1Permissions {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub authority_profile: ::std::option::Option<AgentStateV1PermissionsAuthorityProfile>,
    pub grant_ids: ::std::vec::Vec<Id>,
}
#[doc = "`AgentStateV1PermissionsAuthorityProfile`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AgentStateV1PermissionsAuthorityProfile {
    #[serde(rename = "read-only")]
    ReadOnly,
    #[serde(rename = "code-safe")]
    CodeSafe,
    #[serde(rename = "code-write")]
    CodeWrite,
}
impl ::std::fmt::Display for AgentStateV1PermissionsAuthorityProfile {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::ReadOnly => f.write_str("read-only"),
            Self::CodeSafe => f.write_str("code-safe"),
            Self::CodeWrite => f.write_str("code-write"),
        }
    }
}
impl ::std::str::FromStr for AgentStateV1PermissionsAuthorityProfile {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "read-only" => Ok(Self::ReadOnly),
            "code-safe" => Ok(Self::CodeSafe),
            "code-write" => Ok(Self::CodeWrite),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AgentStateV1PermissionsAuthorityProfile {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AgentStateV1PermissionsAuthorityProfile {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AgentStateV1RetrievalSnapshotRefsItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AgentStateV1RetrievalSnapshotRefsItem {
    pub fingerprint: ContentHash,
    pub snapshot_id: Id,
}
#[doc = "`AgentStateV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct AgentStateV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for AgentStateV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<AgentStateV1SchemaVersion> for ::std::string::String {
    fn from(value: AgentStateV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for AgentStateV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for AgentStateV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AgentStateV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for AgentStateV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`AgentStateV1SensitivityLabelsItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AgentStateV1SensitivityLabelsItem {
    pub class: SensitivityClass,
    pub resource: ResourceRef,
}
#[doc = "`AgentStateV1UserIntent`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AgentStateV1UserIntent {
    pub objective: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub task_ir_ref: ::std::option::Option<Id>,
}
#[doc = "Codegen bundle of Allternit Kernel ABI 1.0.0. Not normative; see schemas/."]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(transparent)]
pub struct AllternitKernelAbi(pub ::serde_json::Value);
impl ::std::ops::Deref for AllternitKernelAbi {
    type Target = ::serde_json::Value;
    fn deref(&self) -> &::serde_json::Value {
        &self.0
    }
}
impl ::std::convert::From<AllternitKernelAbi> for ::serde_json::Value {
    fn from(value: AllternitKernelAbi) -> Self {
        value.0
    }
}
impl ::std::convert::From<::serde_json::Value> for AllternitKernelAbi {
    fn from(value: ::serde_json::Value) -> Self {
        Self(value)
    }
}
#[doc = "`ArtifactRecordV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRecordV1 {
    pub artifact_id: Id,
    pub content_hash: ContentHash,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub kind: ArtifactRecordV1Kind,
    #[serde(default, skip_serializing_if = "::serde_json::Map::is_empty")]
    pub metadata: ::serde_json::Map<::std::string::String, ::serde_json::Value>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub payload_ref: ::std::option::Option<::std::string::String>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: ArtifactRecordV1SchemaVersion,
    pub scope_fingerprint: ContentHash,
}
#[doc = "`ArtifactRecordV1Kind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ArtifactRecordV1Kind {
    #[serde(rename = "decision_readout_profile")]
    DecisionReadoutProfile,
    #[serde(rename = "decision_head")]
    DecisionHead,
    #[serde(rename = "calibration_manifest")]
    CalibrationManifest,
    #[serde(rename = "adapter")]
    Adapter,
    #[serde(rename = "routing_profile")]
    RoutingProfile,
}
impl ::std::fmt::Display for ArtifactRecordV1Kind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::DecisionReadoutProfile => f.write_str("decision_readout_profile"),
            Self::DecisionHead => f.write_str("decision_head"),
            Self::CalibrationManifest => f.write_str("calibration_manifest"),
            Self::Adapter => f.write_str("adapter"),
            Self::RoutingProfile => f.write_str("routing_profile"),
        }
    }
}
impl ::std::str::FromStr for ArtifactRecordV1Kind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "decision_readout_profile" => Ok(Self::DecisionReadoutProfile),
            "decision_head" => Ok(Self::DecisionHead),
            "calibration_manifest" => Ok(Self::CalibrationManifest),
            "adapter" => Ok(Self::Adapter),
            "routing_profile" => Ok(Self::RoutingProfile),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ArtifactRecordV1Kind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ArtifactRecordV1Kind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ArtifactRecordV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ArtifactRecordV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for ArtifactRecordV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ArtifactRecordV1SchemaVersion> for ::std::string::String {
    fn from(value: ArtifactRecordV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ArtifactRecordV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ArtifactRecordV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ArtifactRecordV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ArtifactRecordV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ArtifactRef`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    pub artifact_id: Id,
    pub content_hash: ContentHash,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub mime_type: ::std::option::Option<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub uri: ::std::option::Option<::std::string::String>,
}
#[doc = "`AttentionPolicyV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AttentionPolicyV1 {
    pub channels: ::std::vec::Vec<::std::string::String>,
    pub dedupe_window_seconds: u64,
    pub defaults_version: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub escalate_after_seconds: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub interrupt_quiet_hours_at: ::std::option::Option<AttentionPolicyV1InterruptQuietHoursAt>,
    pub max_per_day: u64,
    pub max_per_hour: u64,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub quiet_hours:
        ::std::option::Option<::serde_json::Map<::std::string::String, ::serde_json::Value>>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: AttentionPolicyV1SchemaVersion,
}
#[doc = "`AttentionPolicyV1InterruptQuietHoursAt`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AttentionPolicyV1InterruptQuietHoursAt {
    #[serde(rename = "critical")]
    Critical,
    #[serde(rename = "never")]
    Never,
}
impl ::std::fmt::Display for AttentionPolicyV1InterruptQuietHoursAt {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Critical => f.write_str("critical"),
            Self::Never => f.write_str("never"),
        }
    }
}
impl ::std::str::FromStr for AttentionPolicyV1InterruptQuietHoursAt {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "critical" => Ok(Self::Critical),
            "never" => Ok(Self::Never),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AttentionPolicyV1InterruptQuietHoursAt {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AttentionPolicyV1InterruptQuietHoursAt {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AttentionPolicyV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct AttentionPolicyV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for AttentionPolicyV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<AttentionPolicyV1SchemaVersion> for ::std::string::String {
    fn from(value: AttentionPolicyV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for AttentionPolicyV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for AttentionPolicyV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AttentionPolicyV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for AttentionPolicyV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`AttentionRequestV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AttentionRequestV1 {
    pub attention_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub campaign_id: ::std::option::Option<Id>,
    pub dedupe_key: AttentionRequestV1DedupeKey,
    pub envelope: AttentionRequestV1Envelope,
    pub evidence_refs: ::std::vec::Vec<EvidenceRef>,
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub expected_loss: ::std::option::Option<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub expires_at: ::std::option::Option<Timestamp>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub kind: AttentionRequestV1Kind,
    pub on_expiry: AttentionRequestV1OnExpiry,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub policy_decision_id: ::std::option::Option<Id>,
    pub reason: AttentionRequestV1Reason,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub requested_channel: ::std::option::Option<::std::string::String>,
    pub response_options: ::std::vec::Vec<AttentionRequestV1ResponseOptionsItem>,
    pub run_id: Id,
    pub status: AttentionRequestV1Status,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub summary: ::std::option::Option<::std::string::String>,
    pub topic: ::std::string::String,
    pub urgency: AttentionRequestV1Urgency,
}
#[doc = "`AttentionRequestV1DedupeKey`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct AttentionRequestV1DedupeKey(::std::string::String);
impl ::std::ops::Deref for AttentionRequestV1DedupeKey {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<AttentionRequestV1DedupeKey> for ::std::string::String {
    fn from(value: AttentionRequestV1DedupeKey) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for AttentionRequestV1DedupeKey {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for AttentionRequestV1DedupeKey {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AttentionRequestV1DedupeKey {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for AttentionRequestV1DedupeKey {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`AttentionRequestV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AttentionRequestV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: AttentionRequestV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`AttentionRequestV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AttentionRequestV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.AttentionRequestV1")]
    AllternitKernelAttentionRequestV1,
}
impl ::std::fmt::Display for AttentionRequestV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelAttentionRequestV1 => {
                f.write_str("allternit.kernel.AttentionRequestV1")
            }
        }
    }
}
impl ::std::str::FromStr for AttentionRequestV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.AttentionRequestV1" => Ok(Self::AllternitKernelAttentionRequestV1),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AttentionRequestV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AttentionRequestV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AttentionRequestV1Kind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AttentionRequestV1Kind {
    #[serde(rename = "approval")]
    Approval,
    #[serde(rename = "informational")]
    Informational,
}
impl ::std::fmt::Display for AttentionRequestV1Kind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Approval => f.write_str("approval"),
            Self::Informational => f.write_str("informational"),
        }
    }
}
impl ::std::str::FromStr for AttentionRequestV1Kind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "approval" => Ok(Self::Approval),
            "informational" => Ok(Self::Informational),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AttentionRequestV1Kind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AttentionRequestV1Kind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AttentionRequestV1OnExpiry`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AttentionRequestV1OnExpiry {
    #[serde(rename = "reject")]
    Reject,
    #[serde(rename = "none")]
    None,
}
impl ::std::fmt::Display for AttentionRequestV1OnExpiry {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Reject => f.write_str("reject"),
            Self::None => f.write_str("none"),
        }
    }
}
impl ::std::str::FromStr for AttentionRequestV1OnExpiry {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "reject" => Ok(Self::Reject),
            "none" => Ok(Self::None),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AttentionRequestV1OnExpiry {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AttentionRequestV1OnExpiry {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AttentionRequestV1Reason`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AttentionRequestV1Reason {
    #[serde(rename = "approval_required")]
    ApprovalRequired,
    #[serde(rename = "clarification_needed")]
    ClarificationNeeded,
    #[serde(rename = "budget_exhausted")]
    BudgetExhausted,
    #[serde(rename = "verification_inconclusive")]
    VerificationInconclusive,
    #[serde(rename = "low_confidence")]
    LowConfidence,
    #[serde(rename = "unsafe_resume")]
    UnsafeResume,
    #[serde(rename = "review_requested")]
    ReviewRequested,
}
impl ::std::fmt::Display for AttentionRequestV1Reason {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::ApprovalRequired => f.write_str("approval_required"),
            Self::ClarificationNeeded => f.write_str("clarification_needed"),
            Self::BudgetExhausted => f.write_str("budget_exhausted"),
            Self::VerificationInconclusive => f.write_str("verification_inconclusive"),
            Self::LowConfidence => f.write_str("low_confidence"),
            Self::UnsafeResume => f.write_str("unsafe_resume"),
            Self::ReviewRequested => f.write_str("review_requested"),
        }
    }
}
impl ::std::str::FromStr for AttentionRequestV1Reason {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "approval_required" => Ok(Self::ApprovalRequired),
            "clarification_needed" => Ok(Self::ClarificationNeeded),
            "budget_exhausted" => Ok(Self::BudgetExhausted),
            "verification_inconclusive" => Ok(Self::VerificationInconclusive),
            "low_confidence" => Ok(Self::LowConfidence),
            "unsafe_resume" => Ok(Self::UnsafeResume),
            "review_requested" => Ok(Self::ReviewRequested),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AttentionRequestV1Reason {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AttentionRequestV1Reason {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AttentionRequestV1ResponseOptionsItem`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AttentionRequestV1ResponseOptionsItem {
    #[serde(rename = "approval")]
    Approval,
    #[serde(rename = "rejection")]
    Rejection,
    #[serde(rename = "data")]
    Data,
    #[serde(rename = "message")]
    Message,
}
impl ::std::fmt::Display for AttentionRequestV1ResponseOptionsItem {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Approval => f.write_str("approval"),
            Self::Rejection => f.write_str("rejection"),
            Self::Data => f.write_str("data"),
            Self::Message => f.write_str("message"),
        }
    }
}
impl ::std::str::FromStr for AttentionRequestV1ResponseOptionsItem {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "approval" => Ok(Self::Approval),
            "rejection" => Ok(Self::Rejection),
            "data" => Ok(Self::Data),
            "message" => Ok(Self::Message),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AttentionRequestV1ResponseOptionsItem {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AttentionRequestV1ResponseOptionsItem {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AttentionRequestV1Status`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AttentionRequestV1Status {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "resolved")]
    Resolved,
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "withdrawn")]
    Withdrawn,
}
impl ::std::fmt::Display for AttentionRequestV1Status {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Open => f.write_str("open"),
            Self::Resolved => f.write_str("resolved"),
            Self::Expired => f.write_str("expired"),
            Self::Withdrawn => f.write_str("withdrawn"),
        }
    }
}
impl ::std::str::FromStr for AttentionRequestV1Status {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "open" => Ok(Self::Open),
            "resolved" => Ok(Self::Resolved),
            "expired" => Ok(Self::Expired),
            "withdrawn" => Ok(Self::Withdrawn),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AttentionRequestV1Status {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AttentionRequestV1Status {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AttentionRequestV1Urgency`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AttentionRequestV1Urgency {
    #[serde(rename = "low")]
    Low,
    #[serde(rename = "normal")]
    Normal,
    #[serde(rename = "high")]
    High,
    #[serde(rename = "critical")]
    Critical,
}
impl ::std::fmt::Display for AttentionRequestV1Urgency {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Low => f.write_str("low"),
            Self::Normal => f.write_str("normal"),
            Self::High => f.write_str("high"),
            Self::Critical => f.write_str("critical"),
        }
    }
}
impl ::std::str::FromStr for AttentionRequestV1Urgency {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "low" => Ok(Self::Low),
            "normal" => Ok(Self::Normal),
            "high" => Ok(Self::High),
            "critical" => Ok(Self::Critical),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AttentionRequestV1Urgency {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AttentionRequestV1Urgency {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AttentionResolutionV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AttentionResolutionV1 {
    pub attention_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub outcome: AttentionResolutionV1Outcome,
    pub receipt_id: Id,
    pub resolved_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub resolved_by: ::std::option::Option<Id>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: AttentionResolutionV1SchemaVersion,
    #[serde(rename = "type")]
    pub type_: AttentionResolutionV1Type,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub value: ::std::option::Option<::serde_json::Value>,
}
#[doc = "`AttentionResolutionV1Outcome`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AttentionResolutionV1Outcome {
    #[serde(rename = "approved")]
    Approved,
    #[serde(rename = "rejected")]
    Rejected,
    #[serde(rename = "answered")]
    Answered,
    #[serde(rename = "none")]
    None,
}
impl ::std::fmt::Display for AttentionResolutionV1Outcome {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Approved => f.write_str("approved"),
            Self::Rejected => f.write_str("rejected"),
            Self::Answered => f.write_str("answered"),
            Self::None => f.write_str("none"),
        }
    }
}
impl ::std::str::FromStr for AttentionResolutionV1Outcome {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            "answered" => Ok(Self::Answered),
            "none" => Ok(Self::None),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AttentionResolutionV1Outcome {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AttentionResolutionV1Outcome {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AttentionResolutionV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct AttentionResolutionV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for AttentionResolutionV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<AttentionResolutionV1SchemaVersion> for ::std::string::String {
    fn from(value: AttentionResolutionV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for AttentionResolutionV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for AttentionResolutionV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AttentionResolutionV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for AttentionResolutionV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`AttentionResolutionV1Type`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AttentionResolutionV1Type {
    #[serde(rename = "approval")]
    Approval,
    #[serde(rename = "rejection")]
    Rejection,
    #[serde(rename = "data")]
    Data,
    #[serde(rename = "message")]
    Message,
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "withdrawn")]
    Withdrawn,
}
impl ::std::fmt::Display for AttentionResolutionV1Type {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Approval => f.write_str("approval"),
            Self::Rejection => f.write_str("rejection"),
            Self::Data => f.write_str("data"),
            Self::Message => f.write_str("message"),
            Self::Expired => f.write_str("expired"),
            Self::Withdrawn => f.write_str("withdrawn"),
        }
    }
}
impl ::std::str::FromStr for AttentionResolutionV1Type {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "approval" => Ok(Self::Approval),
            "rejection" => Ok(Self::Rejection),
            "data" => Ok(Self::Data),
            "message" => Ok(Self::Message),
            "expired" => Ok(Self::Expired),
            "withdrawn" => Ok(Self::Withdrawn),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AttentionResolutionV1Type {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AttentionResolutionV1Type {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AuthorityProfileV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AuthorityProfileV1 {
    pub approval_requirements: ::std::vec::Vec<AuthorityProfileV1ApprovalRequirementsItem>,
    pub capabilities: ::std::vec::Vec<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub defaults_version: ::std::option::Option<::std::string::String>,
    pub denied_capabilities: ::std::vec::Vec<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub filesystem_scope: ::std::vec::Vec<ResourceRef>,
    pub network_policy_id: Id,
    pub profile_id: AuthorityProfileV1ProfileId,
    pub profile_version: ::std::num::NonZeroU64,
    pub schema_id: ::serde_json::Value,
    pub schema_version: AuthorityProfileV1SchemaVersion,
}
#[doc = "`AuthorityProfileV1ApprovalRequirementsItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AuthorityProfileV1ApprovalRequirementsItem {
    pub action_class: ::std::string::String,
    pub requires: AuthorityProfileV1ApprovalRequirementsItemRequires,
}
#[doc = "`AuthorityProfileV1ApprovalRequirementsItemRequires`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AuthorityProfileV1ApprovalRequirementsItemRequires {
    #[serde(rename = "ASK")]
    Ask,
    #[serde(rename = "DENY")]
    Deny,
}
impl ::std::fmt::Display for AuthorityProfileV1ApprovalRequirementsItemRequires {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Ask => f.write_str("ASK"),
            Self::Deny => f.write_str("DENY"),
        }
    }
}
impl ::std::str::FromStr for AuthorityProfileV1ApprovalRequirementsItemRequires {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "ASK" => Ok(Self::Ask),
            "DENY" => Ok(Self::Deny),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AuthorityProfileV1ApprovalRequirementsItemRequires {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String>
    for AuthorityProfileV1ApprovalRequirementsItemRequires
{
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AuthorityProfileV1ProfileId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum AuthorityProfileV1ProfileId {
    #[serde(rename = "read-only")]
    ReadOnly,
    #[serde(rename = "code-safe")]
    CodeSafe,
    #[serde(rename = "code-write")]
    CodeWrite,
}
impl ::std::fmt::Display for AuthorityProfileV1ProfileId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::ReadOnly => f.write_str("read-only"),
            Self::CodeSafe => f.write_str("code-safe"),
            Self::CodeWrite => f.write_str("code-write"),
        }
    }
}
impl ::std::str::FromStr for AuthorityProfileV1ProfileId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "read-only" => Ok(Self::ReadOnly),
            "code-safe" => Ok(Self::CodeSafe),
            "code-write" => Ok(Self::CodeWrite),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AuthorityProfileV1ProfileId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AuthorityProfileV1ProfileId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`AuthorityProfileV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct AuthorityProfileV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for AuthorityProfileV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<AuthorityProfileV1SchemaVersion> for ::std::string::String {
    fn from(value: AuthorityProfileV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for AuthorityProfileV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for AuthorityProfileV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AuthorityProfileV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for AuthorityProfileV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`CampaignV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CampaignV1 {
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub active_run_ids: ::std::vec::Vec<Id>,
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub agent_state_ref: ::std::option::Option<StateVersionRef>,
    pub attention_policy: AttentionPolicyV1,
    pub budget: CampaignV1Budget,
    pub campaign_id: Id,
    pub completion_criteria: CompletionPolicyRef,
    pub defaults_version: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub graph_template: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub last_wake: ::std::option::Option<Timestamp>,
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub next_wake: ::std::option::Option<Timestamp>,
    pub objective: CampaignV1Objective,
    pub owner: Id,
    pub scheduling: CampaignV1Scheduling,
    pub schema_id: ::serde_json::Value,
    pub schema_version: CampaignV1SchemaVersion,
    pub status: CampaignV1Status,
    pub wake_policy: WakePolicyV1,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`CampaignV1Budget`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CampaignV1Budget {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub max_cost_units: ::std::option::Option<f64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub max_runs: ::std::option::Option<u64>,
    pub on_exhaustion: CampaignV1BudgetOnExhaustion,
    pub per_run: ResourceBudget,
}
#[doc = "`CampaignV1BudgetOnExhaustion`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CampaignV1BudgetOnExhaustion {
    #[serde(rename = "REQUEST_ATTENTION")]
    RequestAttention,
    #[serde(rename = "STOP")]
    Stop,
}
impl ::std::fmt::Display for CampaignV1BudgetOnExhaustion {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::RequestAttention => f.write_str("REQUEST_ATTENTION"),
            Self::Stop => f.write_str("STOP"),
        }
    }
}
impl ::std::str::FromStr for CampaignV1BudgetOnExhaustion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "REQUEST_ATTENTION" => Ok(Self::RequestAttention),
            "STOP" => Ok(Self::Stop),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CampaignV1BudgetOnExhaustion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CampaignV1BudgetOnExhaustion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`CampaignV1Objective`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CampaignV1Objective(::std::string::String);
impl ::std::ops::Deref for CampaignV1Objective {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CampaignV1Objective> for ::std::string::String {
    fn from(value: CampaignV1Objective) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CampaignV1Objective {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CampaignV1Objective {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CampaignV1Objective {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CampaignV1Objective {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`CampaignV1Scheduling`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CampaignV1Scheduling {
    #[serde(rename = "disabled_pending_golive")]
    DisabledPendingGolive,
    #[serde(rename = "enabled")]
    Enabled,
}
impl ::std::fmt::Display for CampaignV1Scheduling {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::DisabledPendingGolive => f.write_str("disabled_pending_golive"),
            Self::Enabled => f.write_str("enabled"),
        }
    }
}
impl ::std::str::FromStr for CampaignV1Scheduling {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "disabled_pending_golive" => Ok(Self::DisabledPendingGolive),
            "enabled" => Ok(Self::Enabled),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CampaignV1Scheduling {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CampaignV1Scheduling {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`CampaignV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CampaignV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for CampaignV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CampaignV1SchemaVersion> for ::std::string::String {
    fn from(value: CampaignV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CampaignV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CampaignV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CampaignV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CampaignV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`CampaignV1Status`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CampaignV1Status {
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "sleeping")]
    Sleeping,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "needs_attention")]
    NeedsAttention,
    #[serde(rename = "paused")]
    Paused,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
}
impl ::std::fmt::Display for CampaignV1Status {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Active => f.write_str("active"),
            Self::Sleeping => f.write_str("sleeping"),
            Self::Running => f.write_str("running"),
            Self::NeedsAttention => f.write_str("needs_attention"),
            Self::Paused => f.write_str("paused"),
            Self::Completed => f.write_str("completed"),
            Self::Failed => f.write_str("failed"),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}
impl ::std::str::FromStr for CampaignV1Status {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "active" => Ok(Self::Active),
            "sleeping" => Ok(Self::Sleeping),
            "running" => Ok(Self::Running),
            "needs_attention" => Ok(Self::NeedsAttention),
            "paused" => Ok(Self::Paused),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CampaignV1Status {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CampaignV1Status {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`Candidate`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub candidate_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub is_unknown: ::std::option::Option<bool>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub label: ::std::option::Option<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub payload: ::std::option::Option<::serde_json::Value>,
}
#[doc = "`CapabilityGrant`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CapabilityGrant {
    pub capability: ::std::string::String,
    pub expires_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub grant_id: Id,
    #[serde(default, skip_serializing_if = "::serde_json::Map::is_empty")]
    pub limits: ::serde_json::Map<::std::string::String, ::serde_json::Value>,
    pub resources: ::std::vec::Vec<ResourceRef>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub task_id: ::std::option::Option<Id>,
}
#[doc = "`CapabilityId`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CapabilityId(::std::string::String);
impl ::std::ops::Deref for CapabilityId {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CapabilityId> for ::std::string::String {
    fn from(value: CapabilityId) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CapabilityId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^cap(\\.[a-z0-9_]+)+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^cap(\\.[a-z0-9_]+)+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CapabilityId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CapabilityId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CapabilityId {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`CapabilityRequestV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRequestV1 {
    pub budget: ResourceBudget,
    pub capability: CapabilityId,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub language: ::std::option::Option<::std::string::String>,
    pub latency_class: CapabilityRequestV1LatencyClass,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub min_context_tokens: ::std::option::Option<u64>,
    pub modality: CapabilityRequestV1Modality,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub quality_floor: ::std::option::Option<f64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub required_output_schema: ::std::option::Option<::std::string::String>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: CapabilityRequestV1SchemaVersion,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub state_transfer_preference:
        ::std::option::Option<CapabilityRequestV1StateTransferPreference>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub task_shape: ::std::option::Option<::std::string::String>,
    pub trust_requirement: TrustClass,
}
#[doc = "`CapabilityRequestV1LatencyClass`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CapabilityRequestV1LatencyClass {
    #[serde(rename = "INTERACTIVE")]
    Interactive,
    #[serde(rename = "NORMAL")]
    Normal,
    #[serde(rename = "BACKGROUND")]
    Background,
}
impl ::std::fmt::Display for CapabilityRequestV1LatencyClass {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Interactive => f.write_str("INTERACTIVE"),
            Self::Normal => f.write_str("NORMAL"),
            Self::Background => f.write_str("BACKGROUND"),
        }
    }
}
impl ::std::str::FromStr for CapabilityRequestV1LatencyClass {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "INTERACTIVE" => Ok(Self::Interactive),
            "NORMAL" => Ok(Self::Normal),
            "BACKGROUND" => Ok(Self::Background),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CapabilityRequestV1LatencyClass {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CapabilityRequestV1LatencyClass {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`CapabilityRequestV1Modality`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CapabilityRequestV1Modality {
    #[serde(rename = "TEXT")]
    Text,
    #[serde(rename = "CODE")]
    Code,
    #[serde(rename = "IMAGE")]
    Image,
    #[serde(rename = "MULTI")]
    Multi,
}
impl ::std::fmt::Display for CapabilityRequestV1Modality {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Text => f.write_str("TEXT"),
            Self::Code => f.write_str("CODE"),
            Self::Image => f.write_str("IMAGE"),
            Self::Multi => f.write_str("MULTI"),
        }
    }
}
impl ::std::str::FromStr for CapabilityRequestV1Modality {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "TEXT" => Ok(Self::Text),
            "CODE" => Ok(Self::Code),
            "IMAGE" => Ok(Self::Image),
            "MULTI" => Ok(Self::Multi),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CapabilityRequestV1Modality {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CapabilityRequestV1Modality {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`CapabilityRequestV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CapabilityRequestV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for CapabilityRequestV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CapabilityRequestV1SchemaVersion> for ::std::string::String {
    fn from(value: CapabilityRequestV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CapabilityRequestV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CapabilityRequestV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CapabilityRequestV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CapabilityRequestV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`CapabilityRequestV1StateTransferPreference`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CapabilityRequestV1StateTransferPreference {
    #[serde(rename = "NATIVE_KV")]
    NativeKv,
    #[serde(rename = "TRANSLATED_KV")]
    TranslatedKv,
    #[serde(rename = "PREFIX")]
    Prefix,
    #[serde(rename = "SEMANTIC")]
    Semantic,
}
impl ::std::fmt::Display for CapabilityRequestV1StateTransferPreference {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::NativeKv => f.write_str("NATIVE_KV"),
            Self::TranslatedKv => f.write_str("TRANSLATED_KV"),
            Self::Prefix => f.write_str("PREFIX"),
            Self::Semantic => f.write_str("SEMANTIC"),
        }
    }
}
impl ::std::str::FromStr for CapabilityRequestV1StateTransferPreference {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "NATIVE_KV" => Ok(Self::NativeKv),
            "TRANSLATED_KV" => Ok(Self::TranslatedKv),
            "PREFIX" => Ok(Self::Prefix),
            "SEMANTIC" => Ok(Self::Semantic),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CapabilityRequestV1StateTransferPreference {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CapabilityRequestV1StateTransferPreference {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`CassetteV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CassetteV1 {
    pub abi_version: AbiVersion,
    pub cassette_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub created_at: ::std::option::Option<Timestamp>,
    pub entries: ::std::vec::Vec<CassetteV1EntriesItem>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub graph_id: Id,
    pub graph_version: u64,
    pub run_id: Id,
    pub run_receipt_hash: ContentHash,
    pub schema_id: ::serde_json::Value,
    pub schema_version: CassetteV1SchemaVersion,
}
#[doc = "`CassetteV1EntriesItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CassetteV1EntriesItem {
    pub boundary: CassetteV1EntriesItemBoundary,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub branch_taken: ::std::option::Option<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub effectful: ::std::option::Option<bool>,
    pub node_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub primitive_id: ::std::option::Option<PrimitiveId>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub receipt_ids: ::std::vec::Vec<Id>,
    pub recorded_result_hash: ContentHash,
    pub request_hash: ContentHash,
    pub result_ref: Id,
    pub seq: u64,
}
#[doc = "`CassetteV1EntriesItemBoundary`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CassetteV1EntriesItemBoundary {
    #[serde(rename = "DECISION")]
    Decision,
    #[serde(rename = "TOOL")]
    Tool,
    #[serde(rename = "CAPABILITY")]
    Capability,
    #[serde(rename = "POLICY")]
    Policy,
    #[serde(rename = "VERIFICATION")]
    Verification,
    #[serde(rename = "MUTATION")]
    Mutation,
    #[serde(rename = "WAKE")]
    Wake,
    #[serde(rename = "ATTENTION")]
    Attention,
}
impl ::std::fmt::Display for CassetteV1EntriesItemBoundary {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Decision => f.write_str("DECISION"),
            Self::Tool => f.write_str("TOOL"),
            Self::Capability => f.write_str("CAPABILITY"),
            Self::Policy => f.write_str("POLICY"),
            Self::Verification => f.write_str("VERIFICATION"),
            Self::Mutation => f.write_str("MUTATION"),
            Self::Wake => f.write_str("WAKE"),
            Self::Attention => f.write_str("ATTENTION"),
        }
    }
}
impl ::std::str::FromStr for CassetteV1EntriesItemBoundary {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "DECISION" => Ok(Self::Decision),
            "TOOL" => Ok(Self::Tool),
            "CAPABILITY" => Ok(Self::Capability),
            "POLICY" => Ok(Self::Policy),
            "VERIFICATION" => Ok(Self::Verification),
            "MUTATION" => Ok(Self::Mutation),
            "WAKE" => Ok(Self::Wake),
            "ATTENTION" => Ok(Self::Attention),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CassetteV1EntriesItemBoundary {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CassetteV1EntriesItemBoundary {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`CassetteV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CassetteV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for CassetteV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CassetteV1SchemaVersion> for ::std::string::String {
    fn from(value: CassetteV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CassetteV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CassetteV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CassetteV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CassetteV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`CognitiveStateDescriptorV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CognitiveStateDescriptorV1 {
    pub context_fingerprint: ContentHash,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub model_ref: Id,
    pub model_revision: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub position_encoding: ::std::option::Option<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub quality_profile:
        ::std::option::Option<::serde_json::Map<::std::string::String, ::serde_json::Value>>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: CognitiveStateDescriptorV1SchemaVersion,
    pub state_id: Id,
    pub state_type: CognitiveStateDescriptorV1StateType,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub storage_ref: ::std::option::Option<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub token_span: ::std::option::Option<u64>,
}
#[doc = "`CognitiveStateDescriptorV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CognitiveStateDescriptorV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for CognitiveStateDescriptorV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CognitiveStateDescriptorV1SchemaVersion> for ::std::string::String {
    fn from(value: CognitiveStateDescriptorV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CognitiveStateDescriptorV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CognitiveStateDescriptorV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CognitiveStateDescriptorV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CognitiveStateDescriptorV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`CognitiveStateDescriptorV1StateType`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CognitiveStateDescriptorV1StateType {
    #[serde(rename = "NATIVE_KV")]
    NativeKv,
    #[serde(rename = "TRANSLATED_KV")]
    TranslatedKv,
    #[serde(rename = "PREFIX_CACHE")]
    PrefixCache,
    #[serde(rename = "LATENT")]
    Latent,
    #[serde(rename = "NONE")]
    None,
}
impl ::std::fmt::Display for CognitiveStateDescriptorV1StateType {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::NativeKv => f.write_str("NATIVE_KV"),
            Self::TranslatedKv => f.write_str("TRANSLATED_KV"),
            Self::PrefixCache => f.write_str("PREFIX_CACHE"),
            Self::Latent => f.write_str("LATENT"),
            Self::None => f.write_str("NONE"),
        }
    }
}
impl ::std::str::FromStr for CognitiveStateDescriptorV1StateType {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "NATIVE_KV" => Ok(Self::NativeKv),
            "TRANSLATED_KV" => Ok(Self::TranslatedKv),
            "PREFIX_CACHE" => Ok(Self::PrefixCache),
            "LATENT" => Ok(Self::Latent),
            "NONE" => Ok(Self::None),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CognitiveStateDescriptorV1StateType {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CognitiveStateDescriptorV1StateType {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`CompletionCriterionV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CompletionCriterionV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub blocking_default: ::std::option::Option<bool>,
    pub built_in: bool,
    pub criterion_id: CompletionCriterionV1CriterionId,
    pub criterion_version: ::std::num::NonZeroU64,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub deprecated: ::std::option::Option<bool>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub description: ::std::option::Option<::std::string::String>,
    pub evidence_kinds: ::std::vec::Vec<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: CompletionCriterionV1SchemaVersion,
    pub verifier_kind: CompletionCriterionV1VerifierKind,
    pub verifier_semantics: CompletionCriterionV1VerifierSemantics,
}
#[doc = "`CompletionCriterionV1CriterionId`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CompletionCriterionV1CriterionId(::std::string::String);
impl ::std::ops::Deref for CompletionCriterionV1CriterionId {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CompletionCriterionV1CriterionId> for ::std::string::String {
    fn from(value: CompletionCriterionV1CriterionId) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CompletionCriterionV1CriterionId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^[a-z][a-z0-9_]*$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[a-z][a-z0-9_]*$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CompletionCriterionV1CriterionId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CompletionCriterionV1CriterionId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CompletionCriterionV1CriterionId {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`CompletionCriterionV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CompletionCriterionV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for CompletionCriterionV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CompletionCriterionV1SchemaVersion> for ::std::string::String {
    fn from(value: CompletionCriterionV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CompletionCriterionV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CompletionCriterionV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CompletionCriterionV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CompletionCriterionV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`CompletionCriterionV1VerifierKind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CompletionCriterionV1VerifierKind {
    #[serde(rename = "parse")]
    Parse,
    #[serde(rename = "format")]
    Format,
    #[serde(rename = "lint")]
    Lint,
    #[serde(rename = "typecheck")]
    Typecheck,
    #[serde(rename = "unit_test")]
    UnitTest,
    #[serde(rename = "integration_test")]
    IntegrationTest,
    #[serde(rename = "build")]
    Build,
    #[serde(rename = "semantic_rule")]
    SemanticRule,
    #[serde(rename = "requirement")]
    Requirement,
    #[serde(rename = "security")]
    Security,
    #[serde(rename = "regression")]
    Regression,
    #[serde(rename = "diff_review")]
    DiffReview,
    #[serde(rename = "human")]
    Human,
}
impl ::std::fmt::Display for CompletionCriterionV1VerifierKind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Parse => f.write_str("parse"),
            Self::Format => f.write_str("format"),
            Self::Lint => f.write_str("lint"),
            Self::Typecheck => f.write_str("typecheck"),
            Self::UnitTest => f.write_str("unit_test"),
            Self::IntegrationTest => f.write_str("integration_test"),
            Self::Build => f.write_str("build"),
            Self::SemanticRule => f.write_str("semantic_rule"),
            Self::Requirement => f.write_str("requirement"),
            Self::Security => f.write_str("security"),
            Self::Regression => f.write_str("regression"),
            Self::DiffReview => f.write_str("diff_review"),
            Self::Human => f.write_str("human"),
        }
    }
}
impl ::std::str::FromStr for CompletionCriterionV1VerifierKind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "parse" => Ok(Self::Parse),
            "format" => Ok(Self::Format),
            "lint" => Ok(Self::Lint),
            "typecheck" => Ok(Self::Typecheck),
            "unit_test" => Ok(Self::UnitTest),
            "integration_test" => Ok(Self::IntegrationTest),
            "build" => Ok(Self::Build),
            "semantic_rule" => Ok(Self::SemanticRule),
            "requirement" => Ok(Self::Requirement),
            "security" => Ok(Self::Security),
            "regression" => Ok(Self::Regression),
            "diff_review" => Ok(Self::DiffReview),
            "human" => Ok(Self::Human),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CompletionCriterionV1VerifierKind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CompletionCriterionV1VerifierKind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`CompletionCriterionV1VerifierSemantics`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CompletionCriterionV1VerifierSemantics(::std::string::String);
impl ::std::ops::Deref for CompletionCriterionV1VerifierSemantics {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CompletionCriterionV1VerifierSemantics> for ::std::string::String {
    fn from(value: CompletionCriterionV1VerifierSemantics) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CompletionCriterionV1VerifierSemantics {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CompletionCriterionV1VerifierSemantics {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CompletionCriterionV1VerifierSemantics {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CompletionCriterionV1VerifierSemantics {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`CompletionDecisionV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CompletionDecisionV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub attention_id: ::std::option::Option<Id>,
    pub criteria_results: ::std::vec::Vec<CompletionDecisionV1CriteriaResultsItem>,
    pub decided_by: ::serde_json::Value,
    pub decision_id: Id,
    pub envelope: CompletionDecisionV1Envelope,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub outcome: CompletionDecisionV1Outcome,
    pub policy: CompletionPolicyRef,
    pub predicate_results: CompletionDecisionV1PredicateResults,
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub proposal_id: ::std::option::Option<Id>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub verifier_ids: ::std::vec::Vec<Id>,
}
#[doc = "`CompletionDecisionV1CriteriaResultsItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CompletionDecisionV1CriteriaResultsItem {
    pub criterion_id: ::std::string::String,
    pub receipt_refs: ::std::vec::Vec<Id>,
    pub result: CompletionDecisionV1CriteriaResultsItemResult,
}
#[doc = "`CompletionDecisionV1CriteriaResultsItemResult`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CompletionDecisionV1CriteriaResultsItemResult {
    #[serde(rename = "PASS")]
    Pass,
    #[serde(rename = "FAIL")]
    Fail,
    #[serde(rename = "INCONCLUSIVE")]
    Inconclusive,
    #[serde(rename = "ERROR")]
    Error,
}
impl ::std::fmt::Display for CompletionDecisionV1CriteriaResultsItemResult {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Pass => f.write_str("PASS"),
            Self::Fail => f.write_str("FAIL"),
            Self::Inconclusive => f.write_str("INCONCLUSIVE"),
            Self::Error => f.write_str("ERROR"),
        }
    }
}
impl ::std::str::FromStr for CompletionDecisionV1CriteriaResultsItemResult {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "PASS" => Ok(Self::Pass),
            "FAIL" => Ok(Self::Fail),
            "INCONCLUSIVE" => Ok(Self::Inconclusive),
            "ERROR" => Ok(Self::Error),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CompletionDecisionV1CriteriaResultsItemResult {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String>
    for CompletionDecisionV1CriteriaResultsItemResult
{
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`CompletionDecisionV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CompletionDecisionV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: CompletionDecisionV1EnvelopeProducer,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: CompletionDecisionV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`CompletionDecisionV1EnvelopeProducer`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CompletionDecisionV1EnvelopeProducer {
    pub component_id: Id,
    pub component_version: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub kind: CompletionDecisionV1EnvelopeProducerKind,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub logical_class: ::std::option::Option<::std::string::String>,
}
#[doc = "`CompletionDecisionV1EnvelopeProducerKind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CompletionDecisionV1EnvelopeProducerKind {
    #[serde(rename = "RUNTIME")]
    Runtime,
    #[serde(rename = "VERIFIER")]
    Verifier,
}
impl ::std::fmt::Display for CompletionDecisionV1EnvelopeProducerKind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Runtime => f.write_str("RUNTIME"),
            Self::Verifier => f.write_str("VERIFIER"),
        }
    }
}
impl ::std::str::FromStr for CompletionDecisionV1EnvelopeProducerKind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "RUNTIME" => Ok(Self::Runtime),
            "VERIFIER" => Ok(Self::Verifier),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CompletionDecisionV1EnvelopeProducerKind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CompletionDecisionV1EnvelopeProducerKind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`CompletionDecisionV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CompletionDecisionV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.CompletionDecisionV1")]
    AllternitKernelCompletionDecisionV1,
}
impl ::std::fmt::Display for CompletionDecisionV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelCompletionDecisionV1 => {
                f.write_str("allternit.kernel.CompletionDecisionV1")
            }
        }
    }
}
impl ::std::str::FromStr for CompletionDecisionV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.CompletionDecisionV1" => {
                Ok(Self::AllternitKernelCompletionDecisionV1)
            }
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CompletionDecisionV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CompletionDecisionV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`CompletionDecisionV1Outcome`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CompletionDecisionV1Outcome {
    #[serde(rename = "COMPLETE")]
    Complete,
    #[serde(rename = "PARTIAL")]
    Partial,
    #[serde(rename = "CONTINUE")]
    Continue,
    #[serde(rename = "ESCALATE")]
    Escalate,
    #[serde(rename = "FAIL")]
    Fail,
}
impl ::std::fmt::Display for CompletionDecisionV1Outcome {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Complete => f.write_str("COMPLETE"),
            Self::Partial => f.write_str("PARTIAL"),
            Self::Continue => f.write_str("CONTINUE"),
            Self::Escalate => f.write_str("ESCALATE"),
            Self::Fail => f.write_str("FAIL"),
        }
    }
}
impl ::std::str::FromStr for CompletionDecisionV1Outcome {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "COMPLETE" => Ok(Self::Complete),
            "PARTIAL" => Ok(Self::Partial),
            "CONTINUE" => Ok(Self::Continue),
            "ESCALATE" => Ok(Self::Escalate),
            "FAIL" => Ok(Self::Fail),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CompletionDecisionV1Outcome {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CompletionDecisionV1Outcome {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`CompletionDecisionV1PredicateResults`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CompletionDecisionV1PredicateResults {
    pub acceptance_criteria_satisfied: bool,
    pub completion_evidence_durable: bool,
    pub no_unresolved_blocking_failures: bool,
    pub policy_obligations_satisfied: bool,
    pub required_artifacts_exist: bool,
    pub required_verifications_pass: bool,
}
#[doc = "`CompletionPolicyRef`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CompletionPolicyRef {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub policy_id: Id,
    pub policy_version: ::std::num::NonZeroU64,
}
#[doc = "`CompletionPolicyV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CompletionPolicyV1 {
    pub allow_partial: bool,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub l2882_predicate: ::serde_json::Value,
    pub policy_id: Id,
    pub policy_version: ::std::num::NonZeroU64,
    pub require: ::std::vec::Vec<CompletionPolicyV1RequireItem>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: CompletionPolicyV1SchemaVersion,
    pub task_type: CompletionPolicyV1TaskType,
}
#[doc = "`CompletionPolicyV1RequireItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CompletionPolicyV1RequireItem {
    pub blocking: bool,
    pub criterion_id: ::std::string::String,
    pub criterion_version: ::std::num::NonZeroU64,
    #[serde(default, skip_serializing_if = "::serde_json::Map::is_empty")]
    pub params: ::serde_json::Map<::std::string::String, ::serde_json::Value>,
}
#[doc = "`CompletionPolicyV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CompletionPolicyV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for CompletionPolicyV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CompletionPolicyV1SchemaVersion> for ::std::string::String {
    fn from(value: CompletionPolicyV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CompletionPolicyV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CompletionPolicyV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CompletionPolicyV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CompletionPolicyV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`CompletionPolicyV1TaskType`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CompletionPolicyV1TaskType(::std::string::String);
impl ::std::ops::Deref for CompletionPolicyV1TaskType {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CompletionPolicyV1TaskType> for ::std::string::String {
    fn from(value: CompletionPolicyV1TaskType) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CompletionPolicyV1TaskType {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^[A-Z][A-Z0-9_]*$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[A-Z][A-Z0-9_]*$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CompletionPolicyV1TaskType {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CompletionPolicyV1TaskType {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CompletionPolicyV1TaskType {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`CompletionProposalV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CompletionProposalV1 {
    pub claimed_criteria: ::std::vec::Vec<CompletionProposalV1ClaimedCriteriaItem>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub confidence: ::std::option::Option<f64>,
    pub envelope: CompletionProposalV1Envelope,
    pub evidence_refs: ::std::vec::Vec<EvidenceRef>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub proposal_id: Id,
    pub proposer_node_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub proposer_role: ::std::option::Option<CompletionProposalV1ProposerRole>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub rationale: ::std::option::Option<::std::string::String>,
}
#[doc = "`CompletionProposalV1ClaimedCriteriaItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CompletionProposalV1ClaimedCriteriaItem {
    pub claim: CompletionProposalV1ClaimedCriteriaItemClaim,
    pub criterion_id: ::std::string::String,
}
#[doc = "`CompletionProposalV1ClaimedCriteriaItemClaim`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CompletionProposalV1ClaimedCriteriaItemClaim {
    #[serde(rename = "SATISFIED")]
    Satisfied,
    #[serde(rename = "UNSATISFIED")]
    Unsatisfied,
    #[serde(rename = "UNKNOWN")]
    Unknown,
}
impl ::std::fmt::Display for CompletionProposalV1ClaimedCriteriaItemClaim {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Satisfied => f.write_str("SATISFIED"),
            Self::Unsatisfied => f.write_str("UNSATISFIED"),
            Self::Unknown => f.write_str("UNKNOWN"),
        }
    }
}
impl ::std::str::FromStr for CompletionProposalV1ClaimedCriteriaItemClaim {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "SATISFIED" => Ok(Self::Satisfied),
            "UNSATISFIED" => Ok(Self::Unsatisfied),
            "UNKNOWN" => Ok(Self::Unknown),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CompletionProposalV1ClaimedCriteriaItemClaim {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String>
    for CompletionProposalV1ClaimedCriteriaItemClaim
{
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`CompletionProposalV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CompletionProposalV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: CompletionProposalV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`CompletionProposalV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CompletionProposalV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.CompletionProposalV1")]
    AllternitKernelCompletionProposalV1,
}
impl ::std::fmt::Display for CompletionProposalV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelCompletionProposalV1 => {
                f.write_str("allternit.kernel.CompletionProposalV1")
            }
        }
    }
}
impl ::std::str::FromStr for CompletionProposalV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.CompletionProposalV1" => {
                Ok(Self::AllternitKernelCompletionProposalV1)
            }
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CompletionProposalV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CompletionProposalV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`CompletionProposalV1ProposerRole`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum CompletionProposalV1ProposerRole {
    S0,
    S1,
    S2,
    S3,
}
impl ::std::fmt::Display for CompletionProposalV1ProposerRole {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::S0 => f.write_str("S0"),
            Self::S1 => f.write_str("S1"),
            Self::S2 => f.write_str("S2"),
            Self::S3 => f.write_str("S3"),
        }
    }
}
impl ::std::str::FromStr for CompletionProposalV1ProposerRole {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "S0" => Ok(Self::S0),
            "S1" => Ok(Self::S1),
            "S2" => Ok(Self::S2),
            "S3" => Ok(Self::S3),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for CompletionProposalV1ProposerRole {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CompletionProposalV1ProposerRole {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ComponentIdentity`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ComponentIdentity {
    pub component_id: Id,
    pub component_version: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub kind: ComponentIdentityKind,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub logical_class: ::std::option::Option<::std::string::String>,
}
#[doc = "`ComponentIdentityKind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ComponentIdentityKind {
    #[serde(rename = "RUNTIME")]
    Runtime,
    #[serde(rename = "PRIMITIVE")]
    Primitive,
    #[serde(rename = "DECISION_BACKEND")]
    DecisionBackend,
    #[serde(rename = "MODEL_COMPONENT")]
    ModelComponent,
    #[serde(rename = "TOOL")]
    Tool,
    #[serde(rename = "VERIFIER")]
    Verifier,
    #[serde(rename = "POLICY_ENGINE")]
    PolicyEngine,
    #[serde(rename = "HUMAN")]
    Human,
    #[serde(rename = "ADAPTER")]
    Adapter,
}
impl ::std::fmt::Display for ComponentIdentityKind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Runtime => f.write_str("RUNTIME"),
            Self::Primitive => f.write_str("PRIMITIVE"),
            Self::DecisionBackend => f.write_str("DECISION_BACKEND"),
            Self::ModelComponent => f.write_str("MODEL_COMPONENT"),
            Self::Tool => f.write_str("TOOL"),
            Self::Verifier => f.write_str("VERIFIER"),
            Self::PolicyEngine => f.write_str("POLICY_ENGINE"),
            Self::Human => f.write_str("HUMAN"),
            Self::Adapter => f.write_str("ADAPTER"),
        }
    }
}
impl ::std::str::FromStr for ComponentIdentityKind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "RUNTIME" => Ok(Self::Runtime),
            "PRIMITIVE" => Ok(Self::Primitive),
            "DECISION_BACKEND" => Ok(Self::DecisionBackend),
            "MODEL_COMPONENT" => Ok(Self::ModelComponent),
            "TOOL" => Ok(Self::Tool),
            "VERIFIER" => Ok(Self::Verifier),
            "POLICY_ENGINE" => Ok(Self::PolicyEngine),
            "HUMAN" => Ok(Self::Human),
            "ADAPTER" => Ok(Self::Adapter),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ComponentIdentityKind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ComponentIdentityKind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`CompositeModelBundleV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CompositeModelBundleV1 {
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub adapters: ::std::vec::Vec<Id>,
    pub bundle_version: SemVer,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub calibration_manifest_ids: ::std::vec::Vec<Id>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub completion_policies: ::std::vec::Vec<CompletionPolicyRef>,
    pub compute_graphs: ::std::vec::Vec<Id>,
    pub context_policy: Id,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub decision_backends: ::std::vec::Vec<Id>,
    pub decision_banks: ::std::vec::Vec<Id>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub decision_head_ids: ::std::vec::Vec<Id>,
    pub deployment_profiles: ::std::vec::Vec<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub eval_profile: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(
        default,
        skip_serializing_if = ":: std :: collections :: HashMap::is_empty"
    )]
    pub hashes: ::std::collections::HashMap<::std::string::String, ContentHash>,
    pub logical_model_id: LogicalModelId,
    pub model_components: ::std::vec::Vec<Id>,
    pub primitive_packs: ::std::vec::Vec<Id>,
    pub required_abi_versions: ::std::vec::Vec<AbiVersion>,
    pub router_policy: Id,
    pub schema_id: ::serde_json::Value,
    pub schema_version: CompositeModelBundleV1SchemaVersion,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub shared_backbone_id: ::std::option::Option<Id>,
    pub signatures: ::std::vec::Vec<ReceiptSignature>,
    pub verification_policy: Id,
}
#[doc = "`CompositeModelBundleV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CompositeModelBundleV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for CompositeModelBundleV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CompositeModelBundleV1SchemaVersion> for ::std::string::String {
    fn from(value: CompositeModelBundleV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CompositeModelBundleV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CompositeModelBundleV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CompositeModelBundleV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CompositeModelBundleV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ComputeGraphIrv1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ComputeGraphIrv1 {
    pub completion_nodes: ::std::vec::Vec<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub completion_policy: ::std::option::Option<CompletionPolicyRef>,
    pub edges: ::std::vec::Vec<GraphEdgeV1>,
    pub entry_nodes: ::std::vec::Vec<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub failure_policy: ComputeGraphIrv1FailurePolicy,
    pub global_budget: ResourceBudget,
    pub graph_id: Id,
    pub graph_version: u32,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub invariants: ::std::vec::Vec<::std::string::String>,
    pub nodes: ::std::vec::Vec<GraphNodeV1>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: ComputeGraphIrv1SchemaVersion,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub task_id: ::std::option::Option<Id>,
    pub task_type: ComputeGraphIrv1TaskType,
}
#[doc = "`ComputeGraphIrv1FailurePolicy`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ComputeGraphIrv1FailurePolicy {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub max_total_retries: ::std::option::Option<u64>,
    pub on_unhandled: ComputeGraphIrv1FailurePolicyOnUnhandled,
}
#[doc = "`ComputeGraphIrv1FailurePolicyOnUnhandled`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ComputeGraphIrv1FailurePolicyOnUnhandled {
    #[serde(rename = "FAIL")]
    Fail,
    #[serde(rename = "ESCALATE")]
    Escalate,
    #[serde(rename = "NEEDS_HUMAN")]
    NeedsHuman,
}
impl ::std::fmt::Display for ComputeGraphIrv1FailurePolicyOnUnhandled {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Fail => f.write_str("FAIL"),
            Self::Escalate => f.write_str("ESCALATE"),
            Self::NeedsHuman => f.write_str("NEEDS_HUMAN"),
        }
    }
}
impl ::std::str::FromStr for ComputeGraphIrv1FailurePolicyOnUnhandled {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "FAIL" => Ok(Self::Fail),
            "ESCALATE" => Ok(Self::Escalate),
            "NEEDS_HUMAN" => Ok(Self::NeedsHuman),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ComputeGraphIrv1FailurePolicyOnUnhandled {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ComputeGraphIrv1FailurePolicyOnUnhandled {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ComputeGraphIrv1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ComputeGraphIrv1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for ComputeGraphIrv1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ComputeGraphIrv1SchemaVersion> for ::std::string::String {
    fn from(value: ComputeGraphIrv1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ComputeGraphIrv1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ComputeGraphIrv1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ComputeGraphIrv1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ComputeGraphIrv1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ComputeGraphIrv1TaskType`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ComputeGraphIrv1TaskType(::std::string::String);
impl ::std::ops::Deref for ComputeGraphIrv1TaskType {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ComputeGraphIrv1TaskType> for ::std::string::String {
    fn from(value: ComputeGraphIrv1TaskType) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ComputeGraphIrv1TaskType {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^[A-Z][A-Z0-9_]*$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[A-Z][A-Z0-9_]*$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ComputeGraphIrv1TaskType {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ComputeGraphIrv1TaskType {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ComputeGraphIrv1TaskType {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ContentHash`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ContentHash(::std::string::String);
impl ::std::ops::Deref for ContentHash {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ContentHash> for ::std::string::String {
    fn from(value: ContentHash) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ContentHash {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^sha256:[0-9a-f]{64}$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^sha256:[0-9a-f]{64}$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ContentHash {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ContentHash {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ContentHash {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ContextChunkV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ContextChunkV1 {
    pub chunk_id: Id,
    pub content_hash: ContentHash,
    pub content_ref: ::std::string::String,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub dependency_distance: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub freshness: ::std::option::Option<f64>,
    pub kind: ContextChunkV1Kind,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub pinned_conditions: ::std::vec::Vec<Id>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: ContextChunkV1SchemaVersion,
    pub sensitivity: SensitivityClass,
    pub source_ref: Provenance,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub supersedes: ::std::vec::Vec<Id>,
    #[serde(
        default,
        skip_serializing_if = ":: std :: collections :: HashMap::is_empty"
    )]
    pub token_estimates: ::std::collections::HashMap<::std::string::String, u64>,
    pub trust_class: TrustClass,
}
#[doc = "`ContextChunkV1Kind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ContextChunkV1Kind {
    #[serde(rename = "USER_TURN")]
    UserTurn,
    #[serde(rename = "FILE")]
    File,
    #[serde(rename = "SYMBOL")]
    Symbol,
    #[serde(rename = "TOOL_INPUT")]
    ToolInput,
    #[serde(rename = "TOOL_OUTPUT")]
    ToolOutput,
    #[serde(rename = "DIAGNOSTIC")]
    Diagnostic,
    #[serde(rename = "PLAN")]
    Plan,
    #[serde(rename = "INSTRUCTION")]
    Instruction,
    #[serde(rename = "DIFF")]
    Diff,
    #[serde(rename = "SUMMARY")]
    Summary,
    #[serde(rename = "RECEIPT")]
    Receipt,
    #[serde(rename = "MEMORY")]
    Memory,
    #[serde(rename = "OTHER")]
    Other,
}
impl ::std::fmt::Display for ContextChunkV1Kind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::UserTurn => f.write_str("USER_TURN"),
            Self::File => f.write_str("FILE"),
            Self::Symbol => f.write_str("SYMBOL"),
            Self::ToolInput => f.write_str("TOOL_INPUT"),
            Self::ToolOutput => f.write_str("TOOL_OUTPUT"),
            Self::Diagnostic => f.write_str("DIAGNOSTIC"),
            Self::Plan => f.write_str("PLAN"),
            Self::Instruction => f.write_str("INSTRUCTION"),
            Self::Diff => f.write_str("DIFF"),
            Self::Summary => f.write_str("SUMMARY"),
            Self::Receipt => f.write_str("RECEIPT"),
            Self::Memory => f.write_str("MEMORY"),
            Self::Other => f.write_str("OTHER"),
        }
    }
}
impl ::std::str::FromStr for ContextChunkV1Kind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "USER_TURN" => Ok(Self::UserTurn),
            "FILE" => Ok(Self::File),
            "SYMBOL" => Ok(Self::Symbol),
            "TOOL_INPUT" => Ok(Self::ToolInput),
            "TOOL_OUTPUT" => Ok(Self::ToolOutput),
            "DIAGNOSTIC" => Ok(Self::Diagnostic),
            "PLAN" => Ok(Self::Plan),
            "INSTRUCTION" => Ok(Self::Instruction),
            "DIFF" => Ok(Self::Diff),
            "SUMMARY" => Ok(Self::Summary),
            "RECEIPT" => Ok(Self::Receipt),
            "MEMORY" => Ok(Self::Memory),
            "OTHER" => Ok(Self::Other),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ContextChunkV1Kind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ContextChunkV1Kind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ContextChunkV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ContextChunkV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for ContextChunkV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ContextChunkV1SchemaVersion> for ::std::string::String {
    fn from(value: ContextChunkV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ContextChunkV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ContextChunkV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ContextChunkV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ContextChunkV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ContextProjectionV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ContextProjectionV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub budget_tokens: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub cache_strategy: ::std::option::Option<ContextProjectionV1CacheStrategy>,
    pub compiled_tokens_estimate: u64,
    pub context_fingerprint: ContentHash,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub instruction_fragments: ::std::vec::Vec<Id>,
    pub projection_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub retrieval_snapshot_id: ::std::option::Option<Id>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: ContextProjectionV1SchemaVersion,
    pub selected: ::std::vec::Vec<ContextVisibilityDecisionV1>,
    pub target_capability: CapabilityId,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub target_model_family: ::std::option::Option<::std::string::String>,
}
#[doc = "`ContextProjectionV1CacheStrategy`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ContextProjectionV1CacheStrategy {
    #[serde(rename = "REUSE")]
    Reuse,
    #[serde(rename = "EXTEND")]
    Extend,
    #[serde(rename = "REBUILD")]
    Rebuild,
    #[serde(rename = "SEMANTIC_RECONSTRUCT")]
    SemanticReconstruct,
}
impl ::std::fmt::Display for ContextProjectionV1CacheStrategy {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Reuse => f.write_str("REUSE"),
            Self::Extend => f.write_str("EXTEND"),
            Self::Rebuild => f.write_str("REBUILD"),
            Self::SemanticReconstruct => f.write_str("SEMANTIC_RECONSTRUCT"),
        }
    }
}
impl ::std::str::FromStr for ContextProjectionV1CacheStrategy {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "REUSE" => Ok(Self::Reuse),
            "EXTEND" => Ok(Self::Extend),
            "REBUILD" => Ok(Self::Rebuild),
            "SEMANTIC_RECONSTRUCT" => Ok(Self::SemanticReconstruct),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ContextProjectionV1CacheStrategy {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ContextProjectionV1CacheStrategy {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ContextProjectionV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ContextProjectionV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for ContextProjectionV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ContextProjectionV1SchemaVersion> for ::std::string::String {
    fn from(value: ContextProjectionV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ContextProjectionV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ContextProjectionV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ContextProjectionV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ContextProjectionV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ContextVisibilityDecisionV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ContextVisibilityDecisionV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub assembly_order: ::std::option::Option<u64>,
    pub chunk_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub compression_method: ::std::option::Option<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub estimated_tokens: ::std::option::Option<u64>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub evidence_pointers: ::std::vec::Vec<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub reason_code: ::std::string::String,
    pub relevance: f64,
    pub schema_id: ::serde_json::Value,
    pub schema_version: ContextVisibilityDecisionV1SchemaVersion,
    pub visibility: ContextVisibilityDecisionV1Visibility,
}
#[doc = "`ContextVisibilityDecisionV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ContextVisibilityDecisionV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for ContextVisibilityDecisionV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ContextVisibilityDecisionV1SchemaVersion> for ::std::string::String {
    fn from(value: ContextVisibilityDecisionV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ContextVisibilityDecisionV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ContextVisibilityDecisionV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ContextVisibilityDecisionV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ContextVisibilityDecisionV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ContextVisibilityDecisionV1Visibility`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ContextVisibilityDecisionV1Visibility {
    #[serde(rename = "HIDE")]
    Hide,
    #[serde(rename = "SHORT")]
    Short,
    #[serde(rename = "LONG")]
    Long,
    #[serde(rename = "FULL")]
    Full,
}
impl ::std::fmt::Display for ContextVisibilityDecisionV1Visibility {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Hide => f.write_str("HIDE"),
            Self::Short => f.write_str("SHORT"),
            Self::Long => f.write_str("LONG"),
            Self::Full => f.write_str("FULL"),
        }
    }
}
impl ::std::str::FromStr for ContextVisibilityDecisionV1Visibility {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "HIDE" => Ok(Self::Hide),
            "SHORT" => Ok(Self::Short),
            "LONG" => Ok(Self::Long),
            "FULL" => Ok(Self::Full),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ContextVisibilityDecisionV1Visibility {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ContextVisibilityDecisionV1Visibility {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionBackendProfileV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DecisionBackendProfileV1 {
    pub backend_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub calibration_profile: ::std::option::Option<DecisionBackendProfileV1CalibrationProfile>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub candidate_cacheability: ::std::option::Option<bool>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub candidate_count_curve:
        ::std::vec::Vec<::serde_json::Map<::std::string::String, ::serde_json::Value>>,
    pub decision_modes: ::std::vec::Vec<DecisionBackendProfileV1DecisionModesItem>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub deployment_unit_id: ::std::option::Option<Id>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub domain_heads: ::std::vec::Vec<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub fine_tune_support: ::std::option::Option<bool>,
    pub kind: DecisionBackendProfileV1Kind,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub latency_class: ::std::option::Option<DecisionBackendProfileV1LatencyClass>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub max_candidates: ::std::option::Option<::std::num::NonZeroU64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub max_state_tokens: ::std::option::Option<u64>,
    pub probability_semantics: DecisionBackendProfileV1ProbabilitySemantics,
    pub schema_id: ::serde_json::Value,
    pub schema_version: DecisionBackendProfileV1SchemaVersion,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub state_cacheability: ::std::option::Option<bool>,
    pub trust_location: DecisionBackendProfileV1TrustLocation,
}
#[doc = "`DecisionBackendProfileV1CalibrationProfile`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct DecisionBackendProfileV1CalibrationProfile {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub bins: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub brier: ::std::option::Option<f64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub ece: ::std::option::Option<f64>,
}
#[doc = "`DecisionBackendProfileV1DecisionModesItem`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionBackendProfileV1DecisionModesItem {
    #[serde(rename = "BELIEF")]
    Belief,
    #[serde(rename = "CHOICE")]
    Choice,
    #[serde(rename = "SCORE")]
    Score,
    #[serde(rename = "RANK")]
    Rank,
    #[serde(rename = "SUBSET")]
    Subset,
    #[serde(rename = "ESTIMATE")]
    Estimate,
    #[serde(rename = "GATE")]
    Gate,
    #[serde(rename = "VERIFY")]
    Verify,
    #[serde(rename = "PAIR_SCORE")]
    PairScore,
}
impl ::std::fmt::Display for DecisionBackendProfileV1DecisionModesItem {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Belief => f.write_str("BELIEF"),
            Self::Choice => f.write_str("CHOICE"),
            Self::Score => f.write_str("SCORE"),
            Self::Rank => f.write_str("RANK"),
            Self::Subset => f.write_str("SUBSET"),
            Self::Estimate => f.write_str("ESTIMATE"),
            Self::Gate => f.write_str("GATE"),
            Self::Verify => f.write_str("VERIFY"),
            Self::PairScore => f.write_str("PAIR_SCORE"),
        }
    }
}
impl ::std::str::FromStr for DecisionBackendProfileV1DecisionModesItem {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "BELIEF" => Ok(Self::Belief),
            "CHOICE" => Ok(Self::Choice),
            "SCORE" => Ok(Self::Score),
            "RANK" => Ok(Self::Rank),
            "SUBSET" => Ok(Self::Subset),
            "ESTIMATE" => Ok(Self::Estimate),
            "GATE" => Ok(Self::Gate),
            "VERIFY" => Ok(Self::Verify),
            "PAIR_SCORE" => Ok(Self::PairScore),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionBackendProfileV1DecisionModesItem {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionBackendProfileV1DecisionModesItem {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionBackendProfileV1Kind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionBackendProfileV1Kind {
    #[serde(rename = "rules")]
    Rules,
    #[serde(rename = "schema_encoder")]
    SchemaEncoder,
    #[serde(rename = "contrastive_ranker")]
    ContrastiveRanker,
    #[serde(rename = "decoder_readout")]
    DecoderReadout,
    #[serde(rename = "specialist")]
    Specialist,
    #[serde(rename = "remote_api")]
    RemoteApi,
}
impl ::std::fmt::Display for DecisionBackendProfileV1Kind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Rules => f.write_str("rules"),
            Self::SchemaEncoder => f.write_str("schema_encoder"),
            Self::ContrastiveRanker => f.write_str("contrastive_ranker"),
            Self::DecoderReadout => f.write_str("decoder_readout"),
            Self::Specialist => f.write_str("specialist"),
            Self::RemoteApi => f.write_str("remote_api"),
        }
    }
}
impl ::std::str::FromStr for DecisionBackendProfileV1Kind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "rules" => Ok(Self::Rules),
            "schema_encoder" => Ok(Self::SchemaEncoder),
            "contrastive_ranker" => Ok(Self::ContrastiveRanker),
            "decoder_readout" => Ok(Self::DecoderReadout),
            "specialist" => Ok(Self::Specialist),
            "remote_api" => Ok(Self::RemoteApi),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionBackendProfileV1Kind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionBackendProfileV1Kind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionBackendProfileV1LatencyClass`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionBackendProfileV1LatencyClass {
    #[serde(rename = "REALTIME")]
    Realtime,
    #[serde(rename = "INTERACTIVE")]
    Interactive,
    #[serde(rename = "BACKGROUND")]
    Background,
    #[serde(rename = "BATCH")]
    Batch,
}
impl ::std::fmt::Display for DecisionBackendProfileV1LatencyClass {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Realtime => f.write_str("REALTIME"),
            Self::Interactive => f.write_str("INTERACTIVE"),
            Self::Background => f.write_str("BACKGROUND"),
            Self::Batch => f.write_str("BATCH"),
        }
    }
}
impl ::std::str::FromStr for DecisionBackendProfileV1LatencyClass {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "REALTIME" => Ok(Self::Realtime),
            "INTERACTIVE" => Ok(Self::Interactive),
            "BACKGROUND" => Ok(Self::Background),
            "BATCH" => Ok(Self::Batch),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionBackendProfileV1LatencyClass {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionBackendProfileV1LatencyClass {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionBackendProfileV1ProbabilitySemantics`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionBackendProfileV1ProbabilitySemantics {
    #[serde(rename = "CALIBRATED")]
    Calibrated,
    #[serde(rename = "RELATIVE_SET")]
    RelativeSet,
    #[serde(rename = "UNCALIBRATED")]
    Uncalibrated,
}
impl ::std::fmt::Display for DecisionBackendProfileV1ProbabilitySemantics {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Calibrated => f.write_str("CALIBRATED"),
            Self::RelativeSet => f.write_str("RELATIVE_SET"),
            Self::Uncalibrated => f.write_str("UNCALIBRATED"),
        }
    }
}
impl ::std::str::FromStr for DecisionBackendProfileV1ProbabilitySemantics {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "CALIBRATED" => Ok(Self::Calibrated),
            "RELATIVE_SET" => Ok(Self::RelativeSet),
            "UNCALIBRATED" => Ok(Self::Uncalibrated),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionBackendProfileV1ProbabilitySemantics {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String>
    for DecisionBackendProfileV1ProbabilitySemantics
{
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionBackendProfileV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct DecisionBackendProfileV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for DecisionBackendProfileV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<DecisionBackendProfileV1SchemaVersion> for ::std::string::String {
    fn from(value: DecisionBackendProfileV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for DecisionBackendProfileV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for DecisionBackendProfileV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionBackendProfileV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for DecisionBackendProfileV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`DecisionBackendProfileV1TrustLocation`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionBackendProfileV1TrustLocation {
    #[serde(rename = "LOCAL")]
    Local,
    #[serde(rename = "PRIVATE_REMOTE")]
    PrivateRemote,
    #[serde(rename = "PUBLIC_REMOTE")]
    PublicRemote,
}
impl ::std::fmt::Display for DecisionBackendProfileV1TrustLocation {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Local => f.write_str("LOCAL"),
            Self::PrivateRemote => f.write_str("PRIVATE_REMOTE"),
            Self::PublicRemote => f.write_str("PUBLIC_REMOTE"),
        }
    }
}
impl ::std::str::FromStr for DecisionBackendProfileV1TrustLocation {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "LOCAL" => Ok(Self::Local),
            "PRIVATE_REMOTE" => Ok(Self::PrivateRemote),
            "PUBLIC_REMOTE" => Ok(Self::PublicRemote),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionBackendProfileV1TrustLocation {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionBackendProfileV1TrustLocation {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionCalibrationManifestV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DecisionCalibrationManifestV1 {
    #[serde(default, skip_serializing_if = "::serde_json::Map::is_empty")]
    pub coverage_region: ::serde_json::Map<::std::string::String, ::serde_json::Value>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub created_at: ::std::option::Option<Timestamp>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub gate: DecisionCalibrationManifestV1Gate,
    pub held_out: DecisionCalibrationManifestV1HeldOut,
    pub manifest_id: Id,
    pub metrics: DecisionCalibrationManifestV1Metrics,
    pub primitive_id: PrimitiveId,
    pub schema_id: ::serde_json::Value,
    pub schema_version: DecisionCalibrationManifestV1SchemaVersion,
    pub scope: DecisionCalibrationManifestV1Scope,
    pub scope_fingerprint: ContentHash,
}
#[doc = "`DecisionCalibrationManifestV1Gate`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DecisionCalibrationManifestV1Gate {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub agreement_with_other_model_used: ::std::option::Option<::serde_json::Value>,
    pub auto_act_error_max: ::serde_json::Value,
    pub ece_max: ::serde_json::Value,
    pub passed: bool,
    pub reversible_only: ::serde_json::Value,
}
#[doc = "`DecisionCalibrationManifestV1HeldOut`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DecisionCalibrationManifestV1HeldOut {
    pub auto_act_error_rate: f64,
    pub auto_act_error_upper_bound: f64,
    pub auto_act_n: u64,
    pub ci_level: f64,
    pub ci_method: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub dataset_ref: ::std::option::Option<Id>,
    pub ece_upper_bound: f64,
    pub n: ::std::num::NonZeroU64,
}
#[doc = "`DecisionCalibrationManifestV1Metrics`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DecisionCalibrationManifestV1Metrics {
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub accuracy: ::std::option::Option<f64>,
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub brier: ::std::option::Option<f64>,
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub coverage_at_risk: ::std::option::Option<f64>,
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub ece: ::std::option::Option<f64>,
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub f1: ::std::option::Option<f64>,
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub flip_sensitivity: ::std::option::Option<f64>,
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub nll: ::std::option::Option<f64>,
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub order_sensitivity: ::std::option::Option<f64>,
}
#[doc = "`DecisionCalibrationManifestV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct DecisionCalibrationManifestV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for DecisionCalibrationManifestV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<DecisionCalibrationManifestV1SchemaVersion> for ::std::string::String {
    fn from(value: DecisionCalibrationManifestV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for DecisionCalibrationManifestV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for DecisionCalibrationManifestV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionCalibrationManifestV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for DecisionCalibrationManifestV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`DecisionCalibrationManifestV1Scope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DecisionCalibrationManifestV1Scope {
    pub backend_id: Id,
    pub candidate_schema_hash: ContentHash,
    pub candidate_set_hash: ContentHash,
    pub model_ref: Id,
    pub model_revision: ::std::string::String,
    pub quantization: ::std::string::String,
    pub question_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub readout_point: ::std::option::Option<::std::string::String>,
    pub runtime_backend: ::std::string::String,
    pub threshold_profile: Id,
    pub tokenizer_id: ::std::string::String,
}
#[doc = "`DecisionDeploymentUnitV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DecisionDeploymentUnitV1 {
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub calibration_artifact_id: ::std::option::Option<Id>,
    pub candidate_set_hash: ContentHash,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub model_ref: Id,
    pub model_revision: ::std::string::String,
    pub quantization: ::std::string::String,
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub readout_point: ::std::option::Option<::std::string::String>,
    pub runtime_backend: ::std::string::String,
    pub schema_hash: ContentHash,
    pub schema_id: ::serde_json::Value,
    pub schema_version: DecisionDeploymentUnitV1SchemaVersion,
    pub threshold_profile: Id,
    pub tokenizer_id: ::std::string::String,
}
#[doc = "`DecisionDeploymentUnitV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct DecisionDeploymentUnitV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for DecisionDeploymentUnitV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<DecisionDeploymentUnitV1SchemaVersion> for ::std::string::String {
    fn from(value: DecisionDeploymentUnitV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for DecisionDeploymentUnitV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for DecisionDeploymentUnitV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionDeploymentUnitV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for DecisionDeploymentUnitV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`DecisionReadoutProfileV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DecisionReadoutProfileV1 {
    pub backend_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub calibration_artifact_id: ::std::option::Option<Id>,
    pub calibration_level: DecisionReadoutProfileV1CalibrationLevel,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub candidate_set_hash: ::std::option::Option<ContentHash>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub confidence_semantics: ::std::option::Option<DecisionReadoutProfileV1ConfidenceSemantics>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub early_exit_supported: ::std::option::Option<bool>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub head_artifact_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub max_options: ::std::option::Option<::std::num::NonZeroU64>,
    pub profile_id: Id,
    pub question_id: Id,
    pub readout_kind: DecisionReadoutProfileV1ReadoutKind,
    pub schema_hash: ContentHash,
    pub schema_id: ::serde_json::Value,
    pub schema_version: DecisionReadoutProfileV1SchemaVersion,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub source_layer: ::std::option::Option<u64>,
}
#[doc = "`DecisionReadoutProfileV1CalibrationLevel`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionReadoutProfileV1CalibrationLevel {
    #[serde(rename = "RAW")]
    Raw,
    L0,
    L1,
    L2,
    #[serde(rename = "AUTO")]
    Auto,
}
impl ::std::fmt::Display for DecisionReadoutProfileV1CalibrationLevel {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Raw => f.write_str("RAW"),
            Self::L0 => f.write_str("L0"),
            Self::L1 => f.write_str("L1"),
            Self::L2 => f.write_str("L2"),
            Self::Auto => f.write_str("AUTO"),
        }
    }
}
impl ::std::str::FromStr for DecisionReadoutProfileV1CalibrationLevel {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "RAW" => Ok(Self::Raw),
            "L0" => Ok(Self::L0),
            "L1" => Ok(Self::L1),
            "L2" => Ok(Self::L2),
            "AUTO" => Ok(Self::Auto),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionReadoutProfileV1CalibrationLevel {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionReadoutProfileV1CalibrationLevel {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionReadoutProfileV1ConfidenceSemantics`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionReadoutProfileV1ConfidenceSemantics {
    #[serde(rename = "CALIBRATED")]
    Calibrated,
    #[serde(rename = "RELATIVE_SET")]
    RelativeSet,
    #[serde(rename = "UNCALIBRATED")]
    Uncalibrated,
}
impl ::std::fmt::Display for DecisionReadoutProfileV1ConfidenceSemantics {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Calibrated => f.write_str("CALIBRATED"),
            Self::RelativeSet => f.write_str("RELATIVE_SET"),
            Self::Uncalibrated => f.write_str("UNCALIBRATED"),
        }
    }
}
impl ::std::str::FromStr for DecisionReadoutProfileV1ConfidenceSemantics {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "CALIBRATED" => Ok(Self::Calibrated),
            "RELATIVE_SET" => Ok(Self::RelativeSet),
            "UNCALIBRATED" => Ok(Self::Uncalibrated),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionReadoutProfileV1ConfidenceSemantics {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String>
    for DecisionReadoutProfileV1ConfidenceSemantics
{
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionReadoutProfileV1ReadoutKind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionReadoutProfileV1ReadoutKind {
    #[serde(rename = "RAW_LOGIT")]
    RawLogit,
    #[serde(rename = "DEBIASED_LOGIT")]
    DebiasedLogit,
    #[serde(rename = "CALIBRATED_LOGIT")]
    CalibratedLogit,
    #[serde(rename = "HIDDEN_STATE_HEAD")]
    HiddenStateHead,
    #[serde(rename = "CONTRASTIVE")]
    Contrastive,
    #[serde(rename = "SCHEMA_ENCODER")]
    SchemaEncoder,
}
impl ::std::fmt::Display for DecisionReadoutProfileV1ReadoutKind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::RawLogit => f.write_str("RAW_LOGIT"),
            Self::DebiasedLogit => f.write_str("DEBIASED_LOGIT"),
            Self::CalibratedLogit => f.write_str("CALIBRATED_LOGIT"),
            Self::HiddenStateHead => f.write_str("HIDDEN_STATE_HEAD"),
            Self::Contrastive => f.write_str("CONTRASTIVE"),
            Self::SchemaEncoder => f.write_str("SCHEMA_ENCODER"),
        }
    }
}
impl ::std::str::FromStr for DecisionReadoutProfileV1ReadoutKind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "RAW_LOGIT" => Ok(Self::RawLogit),
            "DEBIASED_LOGIT" => Ok(Self::DebiasedLogit),
            "CALIBRATED_LOGIT" => Ok(Self::CalibratedLogit),
            "HIDDEN_STATE_HEAD" => Ok(Self::HiddenStateHead),
            "CONTRASTIVE" => Ok(Self::Contrastive),
            "SCHEMA_ENCODER" => Ok(Self::SchemaEncoder),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionReadoutProfileV1ReadoutKind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionReadoutProfileV1ReadoutKind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionReadoutProfileV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct DecisionReadoutProfileV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for DecisionReadoutProfileV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<DecisionReadoutProfileV1SchemaVersion> for ::std::string::String {
    fn from(value: DecisionReadoutProfileV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for DecisionReadoutProfileV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for DecisionReadoutProfileV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionReadoutProfileV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for DecisionReadoutProfileV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`DecisionRequestV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequestV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub calibration_domain: ::std::option::Option<::std::string::String>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub candidates: ::std::vec::Vec<Candidate>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub constraints: ::std::vec::Vec<::serde_json::Map<::std::string::String, ::serde_json::Value>>,
    pub decision_bank_id: Id,
    pub envelope: DecisionRequestV1Envelope,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub instructions: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub latency_class: ::std::option::Option<DecisionRequestV1LatencyClass>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub max_latency_ms: ::std::option::Option<u64>,
    pub operation: DecisionRequestV1Operation,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub question_id: ::std::option::Option<Id>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub scale: ::std::vec::Vec<::std::string::String>,
    pub state_projection_ref: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub threshold_profile_id: ::std::option::Option<Id>,
}
#[doc = "`DecisionRequestV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequestV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: DecisionRequestV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`DecisionRequestV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionRequestV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.DecisionRequestV1")]
    AllternitKernelDecisionRequestV1,
}
impl ::std::fmt::Display for DecisionRequestV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelDecisionRequestV1 => {
                f.write_str("allternit.kernel.DecisionRequestV1")
            }
        }
    }
}
impl ::std::str::FromStr for DecisionRequestV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.DecisionRequestV1" => Ok(Self::AllternitKernelDecisionRequestV1),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionRequestV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionRequestV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionRequestV1LatencyClass`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionRequestV1LatencyClass {
    #[serde(rename = "REALTIME")]
    Realtime,
    #[serde(rename = "INTERACTIVE")]
    Interactive,
    #[serde(rename = "BACKGROUND")]
    Background,
    #[serde(rename = "BATCH")]
    Batch,
}
impl ::std::fmt::Display for DecisionRequestV1LatencyClass {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Realtime => f.write_str("REALTIME"),
            Self::Interactive => f.write_str("INTERACTIVE"),
            Self::Background => f.write_str("BACKGROUND"),
            Self::Batch => f.write_str("BATCH"),
        }
    }
}
impl ::std::str::FromStr for DecisionRequestV1LatencyClass {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "REALTIME" => Ok(Self::Realtime),
            "INTERACTIVE" => Ok(Self::Interactive),
            "BACKGROUND" => Ok(Self::Background),
            "BATCH" => Ok(Self::Batch),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionRequestV1LatencyClass {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionRequestV1LatencyClass {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionRequestV1Operation`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionRequestV1Operation {
    #[serde(rename = "BELIEF")]
    Belief,
    #[serde(rename = "CHOICE")]
    Choice,
    #[serde(rename = "SCORE")]
    Score,
    #[serde(rename = "RANK")]
    Rank,
    #[serde(rename = "SUBSET")]
    Subset,
    #[serde(rename = "ESTIMATE")]
    Estimate,
    #[serde(rename = "GATE")]
    Gate,
    #[serde(rename = "VERIFY")]
    Verify,
    #[serde(rename = "PAIR_SCORE")]
    PairScore,
}
impl ::std::fmt::Display for DecisionRequestV1Operation {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Belief => f.write_str("BELIEF"),
            Self::Choice => f.write_str("CHOICE"),
            Self::Score => f.write_str("SCORE"),
            Self::Rank => f.write_str("RANK"),
            Self::Subset => f.write_str("SUBSET"),
            Self::Estimate => f.write_str("ESTIMATE"),
            Self::Gate => f.write_str("GATE"),
            Self::Verify => f.write_str("VERIFY"),
            Self::PairScore => f.write_str("PAIR_SCORE"),
        }
    }
}
impl ::std::str::FromStr for DecisionRequestV1Operation {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "BELIEF" => Ok(Self::Belief),
            "CHOICE" => Ok(Self::Choice),
            "SCORE" => Ok(Self::Score),
            "RANK" => Ok(Self::Rank),
            "SUBSET" => Ok(Self::Subset),
            "ESTIMATE" => Ok(Self::Estimate),
            "GATE" => Ok(Self::Gate),
            "VERIFY" => Ok(Self::Verify),
            "PAIR_SCORE" => Ok(Self::PairScore),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionRequestV1Operation {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionRequestV1Operation {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionResultV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DecisionResultV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub abstained: ::std::option::Option<bool>,
    pub answer: ::serde_json::Value,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub backend_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub calibration_fingerprint: ::std::option::Option<ContentHash>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub calibration_id: ::std::option::Option<Id>,
    pub calibration_level_served: DecisionResultV1CalibrationLevelServed,
    pub confidence: f64,
    pub confidence_semantics: DecisionResultV1ConfidenceSemantics,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub constraint_violations:
        ::std::vec::Vec<::serde_json::Map<::std::string::String, ::serde_json::Value>>,
    pub envelope: DecisionResultV1Envelope,
    pub evidence_refs: ::std::vec::Vec<EvidenceRef>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub feasible: ::std::option::Option<bool>,
    pub latency_ms: f64,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub model_impl: ::std::option::Option<ComponentIdentity>,
    pub operation: DecisionResultV1Operation,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub probabilities:
        ::std::option::Option<::std::collections::HashMap<::std::string::String, f64>>,
    pub threshold_action: DecisionResultV1ThresholdAction,
}
#[doc = "`DecisionResultV1CalibrationLevelServed`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionResultV1CalibrationLevelServed {
    #[serde(rename = "RAW")]
    Raw,
    L0,
    L1,
    L2,
    #[serde(rename = "NONE")]
    None,
}
impl ::std::fmt::Display for DecisionResultV1CalibrationLevelServed {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Raw => f.write_str("RAW"),
            Self::L0 => f.write_str("L0"),
            Self::L1 => f.write_str("L1"),
            Self::L2 => f.write_str("L2"),
            Self::None => f.write_str("NONE"),
        }
    }
}
impl ::std::str::FromStr for DecisionResultV1CalibrationLevelServed {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "RAW" => Ok(Self::Raw),
            "L0" => Ok(Self::L0),
            "L1" => Ok(Self::L1),
            "L2" => Ok(Self::L2),
            "NONE" => Ok(Self::None),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionResultV1CalibrationLevelServed {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionResultV1CalibrationLevelServed {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionResultV1ConfidenceSemantics`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionResultV1ConfidenceSemantics {
    #[serde(rename = "CALIBRATED")]
    Calibrated,
    #[serde(rename = "RELATIVE_SET")]
    RelativeSet,
    #[serde(rename = "UNCALIBRATED")]
    Uncalibrated,
}
impl ::std::fmt::Display for DecisionResultV1ConfidenceSemantics {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Calibrated => f.write_str("CALIBRATED"),
            Self::RelativeSet => f.write_str("RELATIVE_SET"),
            Self::Uncalibrated => f.write_str("UNCALIBRATED"),
        }
    }
}
impl ::std::str::FromStr for DecisionResultV1ConfidenceSemantics {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "CALIBRATED" => Ok(Self::Calibrated),
            "RELATIVE_SET" => Ok(Self::RelativeSet),
            "UNCALIBRATED" => Ok(Self::Uncalibrated),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionResultV1ConfidenceSemantics {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionResultV1ConfidenceSemantics {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionResultV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DecisionResultV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: DecisionResultV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`DecisionResultV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionResultV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.DecisionResultV1")]
    AllternitKernelDecisionResultV1,
}
impl ::std::fmt::Display for DecisionResultV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelDecisionResultV1 => {
                f.write_str("allternit.kernel.DecisionResultV1")
            }
        }
    }
}
impl ::std::str::FromStr for DecisionResultV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.DecisionResultV1" => Ok(Self::AllternitKernelDecisionResultV1),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionResultV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionResultV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionResultV1Operation`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionResultV1Operation {
    #[serde(rename = "BELIEF")]
    Belief,
    #[serde(rename = "CHOICE")]
    Choice,
    #[serde(rename = "SCORE")]
    Score,
    #[serde(rename = "RANK")]
    Rank,
    #[serde(rename = "SUBSET")]
    Subset,
    #[serde(rename = "ESTIMATE")]
    Estimate,
    #[serde(rename = "GATE")]
    Gate,
    #[serde(rename = "VERIFY")]
    Verify,
    #[serde(rename = "PAIR_SCORE")]
    PairScore,
}
impl ::std::fmt::Display for DecisionResultV1Operation {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Belief => f.write_str("BELIEF"),
            Self::Choice => f.write_str("CHOICE"),
            Self::Score => f.write_str("SCORE"),
            Self::Rank => f.write_str("RANK"),
            Self::Subset => f.write_str("SUBSET"),
            Self::Estimate => f.write_str("ESTIMATE"),
            Self::Gate => f.write_str("GATE"),
            Self::Verify => f.write_str("VERIFY"),
            Self::PairScore => f.write_str("PAIR_SCORE"),
        }
    }
}
impl ::std::str::FromStr for DecisionResultV1Operation {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "BELIEF" => Ok(Self::Belief),
            "CHOICE" => Ok(Self::Choice),
            "SCORE" => Ok(Self::Score),
            "RANK" => Ok(Self::Rank),
            "SUBSET" => Ok(Self::Subset),
            "ESTIMATE" => Ok(Self::Estimate),
            "GATE" => Ok(Self::Gate),
            "VERIFY" => Ok(Self::Verify),
            "PAIR_SCORE" => Ok(Self::PairScore),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionResultV1Operation {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionResultV1Operation {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DecisionResultV1ThresholdAction`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DecisionResultV1ThresholdAction {
    #[serde(rename = "AUTO")]
    Auto,
    #[serde(rename = "REVIEW")]
    Review,
    #[serde(rename = "ESCALATE")]
    Escalate,
    #[serde(rename = "REJECT")]
    Reject,
}
impl ::std::fmt::Display for DecisionResultV1ThresholdAction {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Auto => f.write_str("AUTO"),
            Self::Review => f.write_str("REVIEW"),
            Self::Escalate => f.write_str("ESCALATE"),
            Self::Reject => f.write_str("REJECT"),
        }
    }
}
impl ::std::str::FromStr for DecisionResultV1ThresholdAction {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "AUTO" => Ok(Self::Auto),
            "REVIEW" => Ok(Self::Review),
            "ESCALATE" => Ok(Self::Escalate),
            "REJECT" => Ok(Self::Reject),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DecisionResultV1ThresholdAction {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DecisionResultV1ThresholdAction {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DivergenceReportV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DivergenceReportV1 {
    pub cassette_id: Id,
    pub divergences: ::std::vec::Vec<DivergenceReportV1DivergencesItem>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub replay_run_id: Id,
    pub report_id: Id,
    pub schema_id: ::serde_json::Value,
    pub schema_version: DivergenceReportV1SchemaVersion,
    pub verdict: DivergenceReportV1Verdict,
}
#[doc = "`DivergenceReportV1DivergencesItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DivergenceReportV1DivergencesItem {
    pub expected: bool,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub explanation: ::std::option::Option<::std::string::String>,
    pub kind: DivergenceReportV1DivergencesItemKind,
    pub node_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub recorded: ::std::option::Option<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub replayed: ::std::option::Option<::std::string::String>,
    pub seq: u64,
}
#[doc = "`DivergenceReportV1DivergencesItemKind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DivergenceReportV1DivergencesItemKind {
    #[serde(rename = "BRANCH")]
    Branch,
    #[serde(rename = "RESULT_HASH")]
    ResultHash,
    #[serde(rename = "NODE_ORDER")]
    NodeOrder,
    #[serde(rename = "MISSING_ENTRY")]
    MissingEntry,
    #[serde(rename = "EXTRA_ENTRY")]
    ExtraEntry,
    #[serde(rename = "POLICY_OUTCOME")]
    PolicyOutcome,
    #[serde(rename = "BACKEND_RESOLUTION")]
    BackendResolution,
}
impl ::std::fmt::Display for DivergenceReportV1DivergencesItemKind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Branch => f.write_str("BRANCH"),
            Self::ResultHash => f.write_str("RESULT_HASH"),
            Self::NodeOrder => f.write_str("NODE_ORDER"),
            Self::MissingEntry => f.write_str("MISSING_ENTRY"),
            Self::ExtraEntry => f.write_str("EXTRA_ENTRY"),
            Self::PolicyOutcome => f.write_str("POLICY_OUTCOME"),
            Self::BackendResolution => f.write_str("BACKEND_RESOLUTION"),
        }
    }
}
impl ::std::str::FromStr for DivergenceReportV1DivergencesItemKind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "BRANCH" => Ok(Self::Branch),
            "RESULT_HASH" => Ok(Self::ResultHash),
            "NODE_ORDER" => Ok(Self::NodeOrder),
            "MISSING_ENTRY" => Ok(Self::MissingEntry),
            "EXTRA_ENTRY" => Ok(Self::ExtraEntry),
            "POLICY_OUTCOME" => Ok(Self::PolicyOutcome),
            "BACKEND_RESOLUTION" => Ok(Self::BackendResolution),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DivergenceReportV1DivergencesItemKind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DivergenceReportV1DivergencesItemKind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`DivergenceReportV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct DivergenceReportV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for DivergenceReportV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<DivergenceReportV1SchemaVersion> for ::std::string::String {
    fn from(value: DivergenceReportV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for DivergenceReportV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for DivergenceReportV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DivergenceReportV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for DivergenceReportV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`DivergenceReportV1Verdict`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum DivergenceReportV1Verdict {
    #[serde(rename = "IDENTICAL")]
    Identical,
    #[serde(rename = "EXPECTED_DIVERGENCE")]
    ExpectedDivergence,
    #[serde(rename = "UNEXPECTED_DIVERGENCE")]
    UnexpectedDivergence,
}
impl ::std::fmt::Display for DivergenceReportV1Verdict {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Identical => f.write_str("IDENTICAL"),
            Self::ExpectedDivergence => f.write_str("EXPECTED_DIVERGENCE"),
            Self::UnexpectedDivergence => f.write_str("UNEXPECTED_DIVERGENCE"),
        }
    }
}
impl ::std::str::FromStr for DivergenceReportV1Verdict {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "IDENTICAL" => Ok(Self::Identical),
            "EXPECTED_DIVERGENCE" => Ok(Self::ExpectedDivergence),
            "UNEXPECTED_DIVERGENCE" => Ok(Self::UnexpectedDivergence),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for DivergenceReportV1Verdict {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for DivergenceReportV1Verdict {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ErrorV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ErrorV1 {
    pub code: ErrorV1Code,
    #[serde(default, skip_serializing_if = "::serde_json::Map::is_empty")]
    pub details: ::serde_json::Map<::std::string::String, ::serde_json::Value>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub family: ErrorV1Family,
    pub message: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub receipt_id: ::std::option::Option<Id>,
    pub retryable: bool,
}
#[doc = "`ErrorV1Code`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ErrorV1Code(::std::string::String);
impl ::std::ops::Deref for ErrorV1Code {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ErrorV1Code> for ::std::string::String {
    fn from(value: ErrorV1Code) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ErrorV1Code {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^ERR_[A-Z0-9_]+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^ERR_[A-Z0-9_]+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ErrorV1Code {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ErrorV1Code {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ErrorV1Code {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ErrorV1Family`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ErrorV1Family {
    #[serde(rename = "INPUT")]
    Input,
    #[serde(rename = "STATE")]
    State,
    #[serde(rename = "CONTEXT")]
    Context,
    #[serde(rename = "ROUTING")]
    Routing,
    #[serde(rename = "DECISION")]
    Decision,
    #[serde(rename = "TOOL")]
    Tool,
    #[serde(rename = "POLICY")]
    Policy,
    #[serde(rename = "MUTATION")]
    Mutation,
    #[serde(rename = "VERIFY")]
    Verify,
    #[serde(rename = "TRANSFER")]
    Transfer,
    #[serde(rename = "BUDGET")]
    Budget,
    #[serde(rename = "SYSTEM")]
    System,
}
impl ::std::fmt::Display for ErrorV1Family {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Input => f.write_str("INPUT"),
            Self::State => f.write_str("STATE"),
            Self::Context => f.write_str("CONTEXT"),
            Self::Routing => f.write_str("ROUTING"),
            Self::Decision => f.write_str("DECISION"),
            Self::Tool => f.write_str("TOOL"),
            Self::Policy => f.write_str("POLICY"),
            Self::Mutation => f.write_str("MUTATION"),
            Self::Verify => f.write_str("VERIFY"),
            Self::Transfer => f.write_str("TRANSFER"),
            Self::Budget => f.write_str("BUDGET"),
            Self::System => f.write_str("SYSTEM"),
        }
    }
}
impl ::std::str::FromStr for ErrorV1Family {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "INPUT" => Ok(Self::Input),
            "STATE" => Ok(Self::State),
            "CONTEXT" => Ok(Self::Context),
            "ROUTING" => Ok(Self::Routing),
            "DECISION" => Ok(Self::Decision),
            "TOOL" => Ok(Self::Tool),
            "POLICY" => Ok(Self::Policy),
            "MUTATION" => Ok(Self::Mutation),
            "VERIFY" => Ok(Self::Verify),
            "TRANSFER" => Ok(Self::Transfer),
            "BUDGET" => Ok(Self::Budget),
            "SYSTEM" => Ok(Self::System),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ErrorV1Family {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ErrorV1Family {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`EvidenceRef`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub claim: ::std::option::Option<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub content_hash: ::std::option::Option<ContentHash>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub kind: EvidenceRefKind,
    #[serde(rename = "ref")]
    pub ref_: Id,
}
#[doc = "`EvidenceRefKind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum EvidenceRefKind {
    #[serde(rename = "STATE")]
    State,
    #[serde(rename = "FILE")]
    File,
    #[serde(rename = "SYMBOL")]
    Symbol,
    #[serde(rename = "TOOL_RECEIPT")]
    ToolReceipt,
    #[serde(rename = "ACTION_RECEIPT")]
    ActionReceipt,
    #[serde(rename = "MUTATION_RECEIPT")]
    MutationReceipt,
    #[serde(rename = "VERIFICATION_RECEIPT")]
    VerificationReceipt,
    #[serde(rename = "POLICY_RECEIPT")]
    PolicyReceipt,
    #[serde(rename = "SPAWN_RECEIPT")]
    SpawnReceipt,
    #[serde(rename = "NODE_OUTPUT")]
    NodeOutput,
    #[serde(rename = "TRACE")]
    Trace,
    #[serde(rename = "HUMAN")]
    Human,
    #[serde(rename = "ATTENTION_RESOLUTION")]
    AttentionResolution,
    #[serde(rename = "DECISION_RESULT")]
    DecisionResult,
}
impl ::std::fmt::Display for EvidenceRefKind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::State => f.write_str("STATE"),
            Self::File => f.write_str("FILE"),
            Self::Symbol => f.write_str("SYMBOL"),
            Self::ToolReceipt => f.write_str("TOOL_RECEIPT"),
            Self::ActionReceipt => f.write_str("ACTION_RECEIPT"),
            Self::MutationReceipt => f.write_str("MUTATION_RECEIPT"),
            Self::VerificationReceipt => f.write_str("VERIFICATION_RECEIPT"),
            Self::PolicyReceipt => f.write_str("POLICY_RECEIPT"),
            Self::SpawnReceipt => f.write_str("SPAWN_RECEIPT"),
            Self::NodeOutput => f.write_str("NODE_OUTPUT"),
            Self::Trace => f.write_str("TRACE"),
            Self::Human => f.write_str("HUMAN"),
            Self::AttentionResolution => f.write_str("ATTENTION_RESOLUTION"),
            Self::DecisionResult => f.write_str("DECISION_RESULT"),
        }
    }
}
impl ::std::str::FromStr for EvidenceRefKind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "STATE" => Ok(Self::State),
            "FILE" => Ok(Self::File),
            "SYMBOL" => Ok(Self::Symbol),
            "TOOL_RECEIPT" => Ok(Self::ToolReceipt),
            "ACTION_RECEIPT" => Ok(Self::ActionReceipt),
            "MUTATION_RECEIPT" => Ok(Self::MutationReceipt),
            "VERIFICATION_RECEIPT" => Ok(Self::VerificationReceipt),
            "POLICY_RECEIPT" => Ok(Self::PolicyReceipt),
            "SPAWN_RECEIPT" => Ok(Self::SpawnReceipt),
            "NODE_OUTPUT" => Ok(Self::NodeOutput),
            "TRACE" => Ok(Self::Trace),
            "HUMAN" => Ok(Self::Human),
            "ATTENTION_RESOLUTION" => Ok(Self::AttentionResolution),
            "DECISION_RESULT" => Ok(Self::DecisionResult),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for EvidenceRefKind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for EvidenceRefKind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ExecutionEnvironmentV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ExecutionEnvironmentV1 {
    pub allowed_env_keys: ::std::vec::Vec<ExecutionEnvironmentV1AllowedEnvKeysItem>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub compiled_by: ::std::option::Option<ComponentIdentity>,
    pub cwd: ::std::string::String,
    pub environment_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub filesystem_scope: ::std::vec::Vec<ResourceRef>,
    pub inherit_process_env: ::serde_json::Value,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub network_policy_id: ::std::option::Option<Id>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: ExecutionEnvironmentV1SchemaVersion,
    pub secret_refs: ::std::vec::Vec<Id>,
    pub tmp_scope: ::std::string::String,
}
#[doc = "`ExecutionEnvironmentV1AllowedEnvKeysItem`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ExecutionEnvironmentV1AllowedEnvKeysItem(::std::string::String);
impl ::std::ops::Deref for ExecutionEnvironmentV1AllowedEnvKeysItem {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ExecutionEnvironmentV1AllowedEnvKeysItem> for ::std::string::String {
    fn from(value: ExecutionEnvironmentV1AllowedEnvKeysItem) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ExecutionEnvironmentV1AllowedEnvKeysItem {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^[A-Z_][A-Z0-9_]*$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[A-Z_][A-Z0-9_]*$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ExecutionEnvironmentV1AllowedEnvKeysItem {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ExecutionEnvironmentV1AllowedEnvKeysItem {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ExecutionEnvironmentV1AllowedEnvKeysItem {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ExecutionEnvironmentV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ExecutionEnvironmentV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for ExecutionEnvironmentV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ExecutionEnvironmentV1SchemaVersion> for ::std::string::String {
    fn from(value: ExecutionEnvironmentV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ExecutionEnvironmentV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ExecutionEnvironmentV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ExecutionEnvironmentV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ExecutionEnvironmentV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ExecutionPlanV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPlanV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub backbone_id: ::std::option::Option<Id>,
    pub backend_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub cache_strategy: ::std::option::Option<ExecutionPlanV1CacheStrategy>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub calibration_level: ::std::option::Option<ExecutionPlanV1CalibrationLevel>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub calibration_manifest_id: ::std::option::Option<Id>,
    pub capability_id: CapabilityId,
    pub cognitive_role: ExecutionPlanV1CognitiveRole,
    pub confidence_floor: f64,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub context_projection_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub cost_budget: ::std::option::Option<f64>,
    pub execution_mode: ExecutionPlanV1ExecutionMode,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub expected_forward_fraction: ::std::option::Option<f64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub fallback_chain: ::std::vec::Vec<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub latency_budget_ms: ::std::option::Option<f64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub layer_start: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub layer_stop: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub memory_budget_mb: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub model_revision: ::std::option::Option<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    pub plan_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub quantization: ::std::option::Option<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub readout_head_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub residency_requirement: ::std::option::Option<ExecutionPlanV1ResidencyRequirement>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub runtime: ::std::option::Option<::std::string::String>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: ExecutionPlanV1SchemaVersion,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub state_transfer_strategy: ::std::option::Option<TransferMode>,
}
#[doc = "`ExecutionPlanV1CacheStrategy`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ExecutionPlanV1CacheStrategy {
    #[serde(rename = "REUSE")]
    Reuse,
    #[serde(rename = "EXTEND")]
    Extend,
    #[serde(rename = "REBUILD")]
    Rebuild,
    #[serde(rename = "SEMANTIC_RECONSTRUCT")]
    SemanticReconstruct,
}
impl ::std::fmt::Display for ExecutionPlanV1CacheStrategy {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Reuse => f.write_str("REUSE"),
            Self::Extend => f.write_str("EXTEND"),
            Self::Rebuild => f.write_str("REBUILD"),
            Self::SemanticReconstruct => f.write_str("SEMANTIC_RECONSTRUCT"),
        }
    }
}
impl ::std::str::FromStr for ExecutionPlanV1CacheStrategy {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "REUSE" => Ok(Self::Reuse),
            "EXTEND" => Ok(Self::Extend),
            "REBUILD" => Ok(Self::Rebuild),
            "SEMANTIC_RECONSTRUCT" => Ok(Self::SemanticReconstruct),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ExecutionPlanV1CacheStrategy {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ExecutionPlanV1CacheStrategy {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ExecutionPlanV1CalibrationLevel`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ExecutionPlanV1CalibrationLevel {
    #[serde(rename = "RAW")]
    Raw,
    L0,
    L1,
    L2,
}
impl ::std::fmt::Display for ExecutionPlanV1CalibrationLevel {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Raw => f.write_str("RAW"),
            Self::L0 => f.write_str("L0"),
            Self::L1 => f.write_str("L1"),
            Self::L2 => f.write_str("L2"),
        }
    }
}
impl ::std::str::FromStr for ExecutionPlanV1CalibrationLevel {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "RAW" => Ok(Self::Raw),
            "L0" => Ok(Self::L0),
            "L1" => Ok(Self::L1),
            "L2" => Ok(Self::L2),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ExecutionPlanV1CalibrationLevel {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ExecutionPlanV1CalibrationLevel {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ExecutionPlanV1CognitiveRole`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ExecutionPlanV1CognitiveRole {
    S0,
    S1,
    S2,
    S3,
}
impl ::std::fmt::Display for ExecutionPlanV1CognitiveRole {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::S0 => f.write_str("S0"),
            Self::S1 => f.write_str("S1"),
            Self::S2 => f.write_str("S2"),
            Self::S3 => f.write_str("S3"),
        }
    }
}
impl ::std::str::FromStr for ExecutionPlanV1CognitiveRole {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "S0" => Ok(Self::S0),
            "S1" => Ok(Self::S1),
            "S2" => Ok(Self::S2),
            "S3" => Ok(Self::S3),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ExecutionPlanV1CognitiveRole {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ExecutionPlanV1CognitiveRole {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ExecutionPlanV1ExecutionMode`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ExecutionPlanV1ExecutionMode {
    #[serde(rename = "M0.DETERMINISTIC")]
    M0Deterministic,
    #[serde(rename = "M1.LOGIT_READOUT")]
    M1LogitReadout,
    #[serde(rename = "M2.CALIBRATED_READOUT")]
    M2CalibratedReadout,
    #[serde(rename = "M3.HIDDEN_HEAD")]
    M3HiddenHead,
    #[serde(rename = "M4.DEDICATED_DECIDER")]
    M4DedicatedDecider,
    #[serde(rename = "M5.GENERATIVE")]
    M5Generative,
    #[serde(rename = "M6.DEEP_SOLVER")]
    M6DeepSolver,
}
impl ::std::fmt::Display for ExecutionPlanV1ExecutionMode {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::M0Deterministic => f.write_str("M0.DETERMINISTIC"),
            Self::M1LogitReadout => f.write_str("M1.LOGIT_READOUT"),
            Self::M2CalibratedReadout => f.write_str("M2.CALIBRATED_READOUT"),
            Self::M3HiddenHead => f.write_str("M3.HIDDEN_HEAD"),
            Self::M4DedicatedDecider => f.write_str("M4.DEDICATED_DECIDER"),
            Self::M5Generative => f.write_str("M5.GENERATIVE"),
            Self::M6DeepSolver => f.write_str("M6.DEEP_SOLVER"),
        }
    }
}
impl ::std::str::FromStr for ExecutionPlanV1ExecutionMode {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "M0.DETERMINISTIC" => Ok(Self::M0Deterministic),
            "M1.LOGIT_READOUT" => Ok(Self::M1LogitReadout),
            "M2.CALIBRATED_READOUT" => Ok(Self::M2CalibratedReadout),
            "M3.HIDDEN_HEAD" => Ok(Self::M3HiddenHead),
            "M4.DEDICATED_DECIDER" => Ok(Self::M4DedicatedDecider),
            "M5.GENERATIVE" => Ok(Self::M5Generative),
            "M6.DEEP_SOLVER" => Ok(Self::M6DeepSolver),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ExecutionPlanV1ExecutionMode {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ExecutionPlanV1ExecutionMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ExecutionPlanV1ResidencyRequirement`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ExecutionPlanV1ResidencyRequirement {
    #[serde(rename = "PINNED")]
    Pinned,
    #[serde(rename = "HOT")]
    Hot,
    #[serde(rename = "WARM")]
    Warm,
    #[serde(rename = "COLD")]
    Cold,
    #[serde(rename = "REMOTE")]
    Remote,
}
impl ::std::fmt::Display for ExecutionPlanV1ResidencyRequirement {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Pinned => f.write_str("PINNED"),
            Self::Hot => f.write_str("HOT"),
            Self::Warm => f.write_str("WARM"),
            Self::Cold => f.write_str("COLD"),
            Self::Remote => f.write_str("REMOTE"),
        }
    }
}
impl ::std::str::FromStr for ExecutionPlanV1ResidencyRequirement {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "PINNED" => Ok(Self::Pinned),
            "HOT" => Ok(Self::Hot),
            "WARM" => Ok(Self::Warm),
            "COLD" => Ok(Self::Cold),
            "REMOTE" => Ok(Self::Remote),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ExecutionPlanV1ResidencyRequirement {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ExecutionPlanV1ResidencyRequirement {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ExecutionPlanV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ExecutionPlanV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for ExecutionPlanV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ExecutionPlanV1SchemaVersion> for ::std::string::String {
    fn from(value: ExecutionPlanV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ExecutionPlanV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ExecutionPlanV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ExecutionPlanV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ExecutionPlanV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ExecutorProbeV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ExecutorProbeV1 {
    pub auth_state: ExecutorProbeV1AuthState,
    pub available: bool,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub capacity: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub error: ::std::option::Option<ErrorV1>,
    pub executor_class: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub in_use: ::std::option::Option<u64>,
    pub probe_id: Id,
    pub probed_at: Timestamp,
    pub schema_id: ::serde_json::Value,
    pub schema_version: ExecutorProbeV1SchemaVersion,
}
#[doc = "`ExecutorProbeV1AuthState`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ExecutorProbeV1AuthState {
    #[serde(rename = "AUTHENTICATED")]
    Authenticated,
    #[serde(rename = "UNAUTHENTICATED")]
    Unauthenticated,
    #[serde(rename = "EXPIRED")]
    Expired,
    #[serde(rename = "UNKNOWN")]
    Unknown,
}
impl ::std::fmt::Display for ExecutorProbeV1AuthState {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Authenticated => f.write_str("AUTHENTICATED"),
            Self::Unauthenticated => f.write_str("UNAUTHENTICATED"),
            Self::Expired => f.write_str("EXPIRED"),
            Self::Unknown => f.write_str("UNKNOWN"),
        }
    }
}
impl ::std::str::FromStr for ExecutorProbeV1AuthState {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "AUTHENTICATED" => Ok(Self::Authenticated),
            "UNAUTHENTICATED" => Ok(Self::Unauthenticated),
            "EXPIRED" => Ok(Self::Expired),
            "UNKNOWN" => Ok(Self::Unknown),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ExecutorProbeV1AuthState {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ExecutorProbeV1AuthState {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ExecutorProbeV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ExecutorProbeV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for ExecutorProbeV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ExecutorProbeV1SchemaVersion> for ::std::string::String {
    fn from(value: ExecutorProbeV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ExecutorProbeV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ExecutorProbeV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ExecutorProbeV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ExecutorProbeV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ExtensionMap`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(transparent)]
pub struct ExtensionMap(pub ::serde_json::Map<::std::string::String, ::serde_json::Value>);
impl ::std::ops::Deref for ExtensionMap {
    type Target = ::serde_json::Map<::std::string::String, ::serde_json::Value>;
    fn deref(&self) -> &::serde_json::Map<::std::string::String, ::serde_json::Value> {
        &self.0
    }
}
impl ::std::convert::From<ExtensionMap>
    for ::serde_json::Map<::std::string::String, ::serde_json::Value>
{
    fn from(value: ExtensionMap) -> Self {
        value.0
    }
}
impl ::std::convert::From<::serde_json::Map<::std::string::String, ::serde_json::Value>>
    for ExtensionMap
{
    fn from(value: ::serde_json::Map<::std::string::String, ::serde_json::Value>) -> Self {
        Self(value)
    }
}
#[doc = "`GraphEdgeV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct GraphEdgeV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub edge_kind: ::std::option::Option<GraphEdgeV1EdgeKind>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub from: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub priority: ::std::option::Option<i64>,
    pub to: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub when: ::std::option::Option<::std::string::String>,
}
#[doc = "`GraphEdgeV1EdgeKind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum GraphEdgeV1EdgeKind {
    #[serde(rename = "NORMAL")]
    Normal,
    #[serde(rename = "FAILURE")]
    Failure,
    #[serde(rename = "LOOP")]
    Loop,
}
impl ::std::fmt::Display for GraphEdgeV1EdgeKind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Normal => f.write_str("NORMAL"),
            Self::Failure => f.write_str("FAILURE"),
            Self::Loop => f.write_str("LOOP"),
        }
    }
}
impl ::std::str::FromStr for GraphEdgeV1EdgeKind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "NORMAL" => Ok(Self::Normal),
            "FAILURE" => Ok(Self::Failure),
            "LOOP" => Ok(Self::Loop),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for GraphEdgeV1EdgeKind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for GraphEdgeV1EdgeKind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`GraphNodeV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct GraphNodeV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub allowed_modes: ::std::option::Option<::std::vec::Vec<GraphNodeV1AllowedModesItem>>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub budget: ::std::option::Option<ResourceBudget>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub capability_request: ::std::option::Option<CapabilityRequestV1>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub cognitive_role: ::std::option::Option<GraphNodeV1CognitiveRole>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub context_projection_id: ::std::option::Option<Id>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub evidence_required: ::std::vec::Vec<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub inputs: ::std::vec::Vec<::std::string::String>,
    pub lock_scope: ::std::vec::Vec<ResourceRef>,
    pub node_id: Id,
    pub node_kind: GraphNodeV1NodeKind,
    pub on_failure: OnFailure,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub output_refs: ::std::vec::Vec<Id>,
    pub outputs: ::std::vec::Vec<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parallel_group: ::std::option::Option<::std::string::String>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub postconditions: ::std::vec::Vec<::std::string::String>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub preconditions: ::std::vec::Vec<::std::string::String>,
    pub primitive_id: PrimitiveId,
    pub read_set: ::std::vec::Vec<ResourceRef>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub retry_policy: ::std::option::Option<RetryPolicy>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub rollback: ::std::option::Option<PrimitiveId>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub timeout_ms: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub verifier: ::std::option::Option<GraphNodeV1Verifier>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub wait_gate: ::std::option::Option<WaitGateV1>,
    pub write_set: ::std::vec::Vec<ResourceRef>,
}
#[doc = "`GraphNodeV1AllowedModesItem`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum GraphNodeV1AllowedModesItem {
    #[serde(rename = "M0.DETERMINISTIC")]
    M0Deterministic,
    #[serde(rename = "M1.LOGIT_READOUT")]
    M1LogitReadout,
    #[serde(rename = "M2.CALIBRATED_READOUT")]
    M2CalibratedReadout,
    #[serde(rename = "M3.HIDDEN_HEAD")]
    M3HiddenHead,
    #[serde(rename = "M4.DEDICATED_DECIDER")]
    M4DedicatedDecider,
    #[serde(rename = "M5.GENERATIVE")]
    M5Generative,
    #[serde(rename = "M6.DEEP_SOLVER")]
    M6DeepSolver,
}
impl ::std::fmt::Display for GraphNodeV1AllowedModesItem {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::M0Deterministic => f.write_str("M0.DETERMINISTIC"),
            Self::M1LogitReadout => f.write_str("M1.LOGIT_READOUT"),
            Self::M2CalibratedReadout => f.write_str("M2.CALIBRATED_READOUT"),
            Self::M3HiddenHead => f.write_str("M3.HIDDEN_HEAD"),
            Self::M4DedicatedDecider => f.write_str("M4.DEDICATED_DECIDER"),
            Self::M5Generative => f.write_str("M5.GENERATIVE"),
            Self::M6DeepSolver => f.write_str("M6.DEEP_SOLVER"),
        }
    }
}
impl ::std::str::FromStr for GraphNodeV1AllowedModesItem {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "M0.DETERMINISTIC" => Ok(Self::M0Deterministic),
            "M1.LOGIT_READOUT" => Ok(Self::M1LogitReadout),
            "M2.CALIBRATED_READOUT" => Ok(Self::M2CalibratedReadout),
            "M3.HIDDEN_HEAD" => Ok(Self::M3HiddenHead),
            "M4.DEDICATED_DECIDER" => Ok(Self::M4DedicatedDecider),
            "M5.GENERATIVE" => Ok(Self::M5Generative),
            "M6.DEEP_SOLVER" => Ok(Self::M6DeepSolver),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for GraphNodeV1AllowedModesItem {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for GraphNodeV1AllowedModesItem {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`GraphNodeV1CognitiveRole`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum GraphNodeV1CognitiveRole {
    S0,
    S1,
    S2,
    S3,
}
impl ::std::fmt::Display for GraphNodeV1CognitiveRole {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::S0 => f.write_str("S0"),
            Self::S1 => f.write_str("S1"),
            Self::S2 => f.write_str("S2"),
            Self::S3 => f.write_str("S3"),
        }
    }
}
impl ::std::str::FromStr for GraphNodeV1CognitiveRole {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "S0" => Ok(Self::S0),
            "S1" => Ok(Self::S1),
            "S2" => Ok(Self::S2),
            "S3" => Ok(Self::S3),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for GraphNodeV1CognitiveRole {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for GraphNodeV1CognitiveRole {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`GraphNodeV1NodeKind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum GraphNodeV1NodeKind {
    #[serde(rename = "COMPUTE")]
    Compute,
    #[serde(rename = "POLICY")]
    Policy,
    #[serde(rename = "VERIFY")]
    Verify,
    #[serde(rename = "WAIT")]
    Wait,
    #[serde(rename = "CONTROL")]
    Control,
}
impl ::std::fmt::Display for GraphNodeV1NodeKind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Compute => f.write_str("COMPUTE"),
            Self::Policy => f.write_str("POLICY"),
            Self::Verify => f.write_str("VERIFY"),
            Self::Wait => f.write_str("WAIT"),
            Self::Control => f.write_str("CONTROL"),
        }
    }
}
impl ::std::str::FromStr for GraphNodeV1NodeKind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "COMPUTE" => Ok(Self::Compute),
            "POLICY" => Ok(Self::Policy),
            "VERIFY" => Ok(Self::Verify),
            "WAIT" => Ok(Self::Wait),
            "CONTROL" => Ok(Self::Control),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for GraphNodeV1NodeKind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for GraphNodeV1NodeKind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`GraphNodeV1Verifier`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum GraphNodeV1Verifier {
    #[serde(rename = "PARSE")]
    Parse,
    #[serde(rename = "FORMAT")]
    Format,
    #[serde(rename = "LINT")]
    Lint,
    #[serde(rename = "TYPECHECK")]
    Typecheck,
    #[serde(rename = "UNIT_TEST")]
    UnitTest,
    #[serde(rename = "INTEGRATION_TEST")]
    IntegrationTest,
    #[serde(rename = "BUILD")]
    Build,
    #[serde(rename = "SEMANTIC_RULE")]
    SemanticRule,
    #[serde(rename = "REQUIREMENT")]
    Requirement,
    #[serde(rename = "SECURITY")]
    Security,
    #[serde(rename = "REGRESSION")]
    Regression,
    #[serde(rename = "DIFF_REVIEW")]
    DiffReview,
    #[serde(rename = "HUMAN")]
    Human,
}
impl ::std::fmt::Display for GraphNodeV1Verifier {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Parse => f.write_str("PARSE"),
            Self::Format => f.write_str("FORMAT"),
            Self::Lint => f.write_str("LINT"),
            Self::Typecheck => f.write_str("TYPECHECK"),
            Self::UnitTest => f.write_str("UNIT_TEST"),
            Self::IntegrationTest => f.write_str("INTEGRATION_TEST"),
            Self::Build => f.write_str("BUILD"),
            Self::SemanticRule => f.write_str("SEMANTIC_RULE"),
            Self::Requirement => f.write_str("REQUIREMENT"),
            Self::Security => f.write_str("SECURITY"),
            Self::Regression => f.write_str("REGRESSION"),
            Self::DiffReview => f.write_str("DIFF_REVIEW"),
            Self::Human => f.write_str("HUMAN"),
        }
    }
}
impl ::std::str::FromStr for GraphNodeV1Verifier {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "PARSE" => Ok(Self::Parse),
            "FORMAT" => Ok(Self::Format),
            "LINT" => Ok(Self::Lint),
            "TYPECHECK" => Ok(Self::Typecheck),
            "UNIT_TEST" => Ok(Self::UnitTest),
            "INTEGRATION_TEST" => Ok(Self::IntegrationTest),
            "BUILD" => Ok(Self::Build),
            "SEMANTIC_RULE" => Ok(Self::SemanticRule),
            "REQUIREMENT" => Ok(Self::Requirement),
            "SECURITY" => Ok(Self::Security),
            "REGRESSION" => Ok(Self::Regression),
            "DIFF_REVIEW" => Ok(Self::DiffReview),
            "HUMAN" => Ok(Self::Human),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for GraphNodeV1Verifier {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for GraphNodeV1Verifier {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`Id`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct Id(::std::string::String);
impl ::std::ops::Deref for Id {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<Id> for ::std::string::String {
    fn from(value: Id) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for Id {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^[A-Za-z0-9][A-Za-z0-9._:@-]{0,199}$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[A-Za-z0-9][A-Za-z0-9._:@-]{0,199}$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for Id {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for Id {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for Id {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`LeaseV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LeaseV1 {
    pub acquired_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub defaults_version: ::std::option::Option<::std::string::String>,
    pub expires_after_ms: ::std::num::NonZeroU64,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub heartbeat_at: Timestamp,
    pub heartbeat_interval_ms: ::std::num::NonZeroU64,
    pub holder: Id,
    pub host: ::std::string::String,
    pub lease_id: Id,
    pub process_or_session: ::std::string::String,
    pub reclaim_policy: LeaseV1ReclaimPolicy,
    pub schema_id: ::serde_json::Value,
    pub schema_version: LeaseV1SchemaVersion,
    pub scope: ::std::vec::Vec<ResourceRef>,
    pub state_version: StateVersion,
}
#[doc = "`LeaseV1ReclaimPolicy`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum LeaseV1ReclaimPolicy {
    #[serde(rename = "REQUEUE")]
    Requeue,
    #[serde(rename = "RESUME_FROM_CHECKPOINT")]
    ResumeFromCheckpoint,
    #[serde(rename = "FAIL_NODE")]
    FailNode,
    #[serde(rename = "NEEDS_HUMAN")]
    NeedsHuman,
}
impl ::std::fmt::Display for LeaseV1ReclaimPolicy {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Requeue => f.write_str("REQUEUE"),
            Self::ResumeFromCheckpoint => f.write_str("RESUME_FROM_CHECKPOINT"),
            Self::FailNode => f.write_str("FAIL_NODE"),
            Self::NeedsHuman => f.write_str("NEEDS_HUMAN"),
        }
    }
}
impl ::std::str::FromStr for LeaseV1ReclaimPolicy {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "REQUEUE" => Ok(Self::Requeue),
            "RESUME_FROM_CHECKPOINT" => Ok(Self::ResumeFromCheckpoint),
            "FAIL_NODE" => Ok(Self::FailNode),
            "NEEDS_HUMAN" => Ok(Self::NeedsHuman),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for LeaseV1ReclaimPolicy {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for LeaseV1ReclaimPolicy {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`LeaseV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct LeaseV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for LeaseV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<LeaseV1SchemaVersion> for ::std::string::String {
    fn from(value: LeaseV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for LeaseV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for LeaseV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for LeaseV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for LeaseV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`LifecycleState`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum LifecycleState {
    #[serde(rename = "DECLARED")]
    Declared,
    #[serde(rename = "ADMITTED")]
    Admitted,
    #[serde(rename = "READY")]
    Ready,
    #[serde(rename = "LEASED")]
    Leased,
    #[serde(rename = "SPAWNED")]
    Spawned,
    #[serde(rename = "RUNNING")]
    Running,
    #[serde(rename = "OUTPUT_READY")]
    OutputReady,
    #[serde(rename = "VERIFYING")]
    Verifying,
    #[serde(rename = "COMMITTED")]
    Committed,
    #[serde(rename = "CONTINUE")]
    Continue,
    #[serde(rename = "REPLAN")]
    Replan,
    #[serde(rename = "NEEDS_HUMAN")]
    NeedsHuman,
    #[serde(rename = "WAITING")]
    Waiting,
    #[serde(rename = "CANCELLING")]
    Cancelling,
    #[serde(rename = "CLOSED")]
    Closed,
}
impl ::std::fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Declared => f.write_str("DECLARED"),
            Self::Admitted => f.write_str("ADMITTED"),
            Self::Ready => f.write_str("READY"),
            Self::Leased => f.write_str("LEASED"),
            Self::Spawned => f.write_str("SPAWNED"),
            Self::Running => f.write_str("RUNNING"),
            Self::OutputReady => f.write_str("OUTPUT_READY"),
            Self::Verifying => f.write_str("VERIFYING"),
            Self::Committed => f.write_str("COMMITTED"),
            Self::Continue => f.write_str("CONTINUE"),
            Self::Replan => f.write_str("REPLAN"),
            Self::NeedsHuman => f.write_str("NEEDS_HUMAN"),
            Self::Waiting => f.write_str("WAITING"),
            Self::Cancelling => f.write_str("CANCELLING"),
            Self::Closed => f.write_str("CLOSED"),
        }
    }
}
impl ::std::str::FromStr for LifecycleState {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "DECLARED" => Ok(Self::Declared),
            "ADMITTED" => Ok(Self::Admitted),
            "READY" => Ok(Self::Ready),
            "LEASED" => Ok(Self::Leased),
            "SPAWNED" => Ok(Self::Spawned),
            "RUNNING" => Ok(Self::Running),
            "OUTPUT_READY" => Ok(Self::OutputReady),
            "VERIFYING" => Ok(Self::Verifying),
            "COMMITTED" => Ok(Self::Committed),
            "CONTINUE" => Ok(Self::Continue),
            "REPLAN" => Ok(Self::Replan),
            "NEEDS_HUMAN" => Ok(Self::NeedsHuman),
            "WAITING" => Ok(Self::Waiting),
            "CANCELLING" => Ok(Self::Cancelling),
            "CLOSED" => Ok(Self::Closed),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for LifecycleState {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for LifecycleState {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`LogicalModelId`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct LogicalModelId(::std::string::String);
impl ::std::ops::Deref for LogicalModelId {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<LogicalModelId> for ::std::string::String {
    fn from(value: LogicalModelId) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for LogicalModelId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^[a-z0-9][a-z0-9-]*-v\\d+-[a-z0-9-]+$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[a-z0-9][a-z0-9-]*-v\\d+-[a-z0-9-]+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for LogicalModelId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for LogicalModelId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for LogicalModelId {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ModelCapabilityProfileV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ModelCapabilityProfileV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub benchmark_profile: ::std::option::Option<Id>,
    pub capabilities: ::std::vec::Vec<ModelCapabilityProfileV1CapabilitiesItem>,
    pub context_limit: u64,
    pub early_exit_support: bool,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub hardware_backends: ::std::vec::Vec<::std::string::String>,
    pub hidden_state_access: bool,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub locality: ::std::option::Option<ModelCapabilityProfileV1Locality>,
    pub logits_access: bool,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub memory_footprint_mb: ::std::option::Option<u64>,
    pub model_id: Id,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub output_schemas: ::std::vec::Vec<::std::string::String>,
    pub residency: ModelCapabilityProfileV1Residency,
    pub schema_id: ::serde_json::Value,
    pub schema_version: ModelCapabilityProfileV1SchemaVersion,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub state_transfer: ::std::vec::Vec<Id>,
    pub trust_classes: ::std::vec::Vec<TrustClass>,
}
#[doc = "`ModelCapabilityProfileV1CapabilitiesItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ModelCapabilityProfileV1CapabilitiesItem {
    pub capability: CapabilityId,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub measured_range:
        ::std::option::Option<::serde_json::Map<::std::string::String, ::serde_json::Value>>,
    pub score: f64,
}
#[doc = "`ModelCapabilityProfileV1Locality`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ModelCapabilityProfileV1Locality {
    #[serde(rename = "LOCAL")]
    Local,
    #[serde(rename = "PRIVATE_REMOTE")]
    PrivateRemote,
    #[serde(rename = "PUBLIC_REMOTE")]
    PublicRemote,
}
impl ::std::fmt::Display for ModelCapabilityProfileV1Locality {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Local => f.write_str("LOCAL"),
            Self::PrivateRemote => f.write_str("PRIVATE_REMOTE"),
            Self::PublicRemote => f.write_str("PUBLIC_REMOTE"),
        }
    }
}
impl ::std::str::FromStr for ModelCapabilityProfileV1Locality {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "LOCAL" => Ok(Self::Local),
            "PRIVATE_REMOTE" => Ok(Self::PrivateRemote),
            "PUBLIC_REMOTE" => Ok(Self::PublicRemote),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ModelCapabilityProfileV1Locality {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ModelCapabilityProfileV1Locality {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ModelCapabilityProfileV1Residency`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ModelCapabilityProfileV1Residency {
    #[serde(rename = "PINNED")]
    Pinned,
    #[serde(rename = "HOT")]
    Hot,
    #[serde(rename = "WARM")]
    Warm,
    #[serde(rename = "COLD")]
    Cold,
    #[serde(rename = "REMOTE")]
    Remote,
}
impl ::std::fmt::Display for ModelCapabilityProfileV1Residency {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Pinned => f.write_str("PINNED"),
            Self::Hot => f.write_str("HOT"),
            Self::Warm => f.write_str("WARM"),
            Self::Cold => f.write_str("COLD"),
            Self::Remote => f.write_str("REMOTE"),
        }
    }
}
impl ::std::str::FromStr for ModelCapabilityProfileV1Residency {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "PINNED" => Ok(Self::Pinned),
            "HOT" => Ok(Self::Hot),
            "WARM" => Ok(Self::Warm),
            "COLD" => Ok(Self::Cold),
            "REMOTE" => Ok(Self::Remote),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ModelCapabilityProfileV1Residency {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ModelCapabilityProfileV1Residency {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ModelCapabilityProfileV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ModelCapabilityProfileV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for ModelCapabilityProfileV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ModelCapabilityProfileV1SchemaVersion> for ::std::string::String {
    fn from(value: ModelCapabilityProfileV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ModelCapabilityProfileV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ModelCapabilityProfileV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ModelCapabilityProfileV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ModelCapabilityProfileV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ModelPoolEntryV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ModelPoolEntryV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub backbone_id: ::std::option::Option<Id>,
    pub backend_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub calibration_manifest_id: ::std::option::Option<Id>,
    pub capabilities: ::std::vec::Vec<CapabilityId>,
    pub cognitive_roles: ::std::vec::Vec<ModelPoolEntryV1CognitiveRolesItem>,
    pub confidence_estimate: f64,
    pub cost: f64,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub latency_ms: f64,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub layer_stop: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub load_latency_ms: ::std::option::Option<f64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub memory_mb: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub model_revision: ::std::option::Option<::std::string::String>,
    pub modes: ::std::vec::Vec<ModelPoolEntryV1ModesItem>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub quantization: ::std::option::Option<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub readout_head_id: ::std::option::Option<Id>,
    pub residency: ModelPoolEntryV1Residency,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub runtime: ::std::option::Option<::std::string::String>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: ModelPoolEntryV1SchemaVersion,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub trust_tags: ::std::option::Option<::std::vec::Vec<::std::string::String>>,
}
#[doc = "`ModelPoolEntryV1CognitiveRolesItem`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ModelPoolEntryV1CognitiveRolesItem {
    S0,
    S1,
    S2,
    S3,
}
impl ::std::fmt::Display for ModelPoolEntryV1CognitiveRolesItem {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::S0 => f.write_str("S0"),
            Self::S1 => f.write_str("S1"),
            Self::S2 => f.write_str("S2"),
            Self::S3 => f.write_str("S3"),
        }
    }
}
impl ::std::str::FromStr for ModelPoolEntryV1CognitiveRolesItem {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "S0" => Ok(Self::S0),
            "S1" => Ok(Self::S1),
            "S2" => Ok(Self::S2),
            "S3" => Ok(Self::S3),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ModelPoolEntryV1CognitiveRolesItem {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ModelPoolEntryV1CognitiveRolesItem {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ModelPoolEntryV1ModesItem`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ModelPoolEntryV1ModesItem {
    #[serde(rename = "M0.DETERMINISTIC")]
    M0Deterministic,
    #[serde(rename = "M1.LOGIT_READOUT")]
    M1LogitReadout,
    #[serde(rename = "M2.CALIBRATED_READOUT")]
    M2CalibratedReadout,
    #[serde(rename = "M3.HIDDEN_HEAD")]
    M3HiddenHead,
    #[serde(rename = "M4.DEDICATED_DECIDER")]
    M4DedicatedDecider,
    #[serde(rename = "M5.GENERATIVE")]
    M5Generative,
    #[serde(rename = "M6.DEEP_SOLVER")]
    M6DeepSolver,
}
impl ::std::fmt::Display for ModelPoolEntryV1ModesItem {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::M0Deterministic => f.write_str("M0.DETERMINISTIC"),
            Self::M1LogitReadout => f.write_str("M1.LOGIT_READOUT"),
            Self::M2CalibratedReadout => f.write_str("M2.CALIBRATED_READOUT"),
            Self::M3HiddenHead => f.write_str("M3.HIDDEN_HEAD"),
            Self::M4DedicatedDecider => f.write_str("M4.DEDICATED_DECIDER"),
            Self::M5Generative => f.write_str("M5.GENERATIVE"),
            Self::M6DeepSolver => f.write_str("M6.DEEP_SOLVER"),
        }
    }
}
impl ::std::str::FromStr for ModelPoolEntryV1ModesItem {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "M0.DETERMINISTIC" => Ok(Self::M0Deterministic),
            "M1.LOGIT_READOUT" => Ok(Self::M1LogitReadout),
            "M2.CALIBRATED_READOUT" => Ok(Self::M2CalibratedReadout),
            "M3.HIDDEN_HEAD" => Ok(Self::M3HiddenHead),
            "M4.DEDICATED_DECIDER" => Ok(Self::M4DedicatedDecider),
            "M5.GENERATIVE" => Ok(Self::M5Generative),
            "M6.DEEP_SOLVER" => Ok(Self::M6DeepSolver),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ModelPoolEntryV1ModesItem {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ModelPoolEntryV1ModesItem {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ModelPoolEntryV1Residency`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ModelPoolEntryV1Residency {
    #[serde(rename = "PINNED")]
    Pinned,
    #[serde(rename = "HOT")]
    Hot,
    #[serde(rename = "WARM")]
    Warm,
    #[serde(rename = "COLD")]
    Cold,
    #[serde(rename = "REMOTE")]
    Remote,
}
impl ::std::fmt::Display for ModelPoolEntryV1Residency {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Pinned => f.write_str("PINNED"),
            Self::Hot => f.write_str("HOT"),
            Self::Warm => f.write_str("WARM"),
            Self::Cold => f.write_str("COLD"),
            Self::Remote => f.write_str("REMOTE"),
        }
    }
}
impl ::std::str::FromStr for ModelPoolEntryV1Residency {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "PINNED" => Ok(Self::Pinned),
            "HOT" => Ok(Self::Hot),
            "WARM" => Ok(Self::Warm),
            "COLD" => Ok(Self::Cold),
            "REMOTE" => Ok(Self::Remote),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ModelPoolEntryV1Residency {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ModelPoolEntryV1Residency {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ModelPoolEntryV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ModelPoolEntryV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for ModelPoolEntryV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ModelPoolEntryV1SchemaVersion> for ::std::string::String {
    fn from(value: ModelPoolEntryV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ModelPoolEntryV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ModelPoolEntryV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ModelPoolEntryV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ModelPoolEntryV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`MutationReceiptV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct MutationReceiptV1 {
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub affected_resources: ::std::vec::Vec<ResourceRef>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub after_hash: ::std::option::Option<ContentHash>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub before_hash: ::std::option::Option<ContentHash>,
    pub canonical_change: UnifiedDiffV1,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub changed_symbols: ::std::vec::Vec<::std::string::String>,
    pub envelope: MutationReceiptV1Envelope,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub mutation_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub rollback_ref: ::std::option::Option<Id>,
    pub state_version_after: StateVersion,
    pub state_version_before: StateVersion,
    pub status: MutationReceiptV1Status,
    pub structural_parse: MutationReceiptV1StructuralParse,
    pub target: ResourceRef,
}
#[doc = "`MutationReceiptV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct MutationReceiptV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: MutationReceiptV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`MutationReceiptV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum MutationReceiptV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.MutationReceiptV1")]
    AllternitKernelMutationReceiptV1,
}
impl ::std::fmt::Display for MutationReceiptV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelMutationReceiptV1 => {
                f.write_str("allternit.kernel.MutationReceiptV1")
            }
        }
    }
}
impl ::std::str::FromStr for MutationReceiptV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.MutationReceiptV1" => Ok(Self::AllternitKernelMutationReceiptV1),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for MutationReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for MutationReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`MutationReceiptV1Status`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum MutationReceiptV1Status {
    #[serde(rename = "APPLIED")]
    Applied,
    #[serde(rename = "REJECTED")]
    Rejected,
    #[serde(rename = "ROLLED_BACK")]
    RolledBack,
    #[serde(rename = "FAILED")]
    Failed,
}
impl ::std::fmt::Display for MutationReceiptV1Status {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Applied => f.write_str("APPLIED"),
            Self::Rejected => f.write_str("REJECTED"),
            Self::RolledBack => f.write_str("ROLLED_BACK"),
            Self::Failed => f.write_str("FAILED"),
        }
    }
}
impl ::std::str::FromStr for MutationReceiptV1Status {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "APPLIED" => Ok(Self::Applied),
            "REJECTED" => Ok(Self::Rejected),
            "ROLLED_BACK" => Ok(Self::RolledBack),
            "FAILED" => Ok(Self::Failed),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for MutationReceiptV1Status {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for MutationReceiptV1Status {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`MutationReceiptV1StructuralParse`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum MutationReceiptV1StructuralParse {
    #[serde(rename = "PASS")]
    Pass,
    #[serde(rename = "FAIL")]
    Fail,
    #[serde(rename = "NOT_APPLICABLE")]
    NotApplicable,
}
impl ::std::fmt::Display for MutationReceiptV1StructuralParse {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Pass => f.write_str("PASS"),
            Self::Fail => f.write_str("FAIL"),
            Self::NotApplicable => f.write_str("NOT_APPLICABLE"),
        }
    }
}
impl ::std::str::FromStr for MutationReceiptV1StructuralParse {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "PASS" => Ok(Self::Pass),
            "FAIL" => Ok(Self::Fail),
            "NOT_APPLICABLE" => Ok(Self::NotApplicable),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for MutationReceiptV1StructuralParse {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for MutationReceiptV1StructuralParse {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`MutationRequestV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct MutationRequestV1 {
    pub canonical_change: UnifiedDiffV1,
    pub envelope: MutationRequestV1Envelope,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub expected_base_hash: ::std::option::Option<ContentHash>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub invariants: ::std::vec::Vec<::std::string::String>,
    pub mutation_type: MutationRequestV1MutationType,
    pub policy_decision_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub rollback_plan: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub source_ref: ::std::option::Option<Id>,
    pub source_representation: MutationRequestV1SourceRepresentation,
    pub target: ResourceRef,
    pub write_set: ::std::vec::Vec<ResourceRef>,
}
#[doc = "`MutationRequestV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct MutationRequestV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: MutationRequestV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`MutationRequestV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum MutationRequestV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.MutationRequestV1")]
    AllternitKernelMutationRequestV1,
}
impl ::std::fmt::Display for MutationRequestV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelMutationRequestV1 => {
                f.write_str("allternit.kernel.MutationRequestV1")
            }
        }
    }
}
impl ::std::str::FromStr for MutationRequestV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.MutationRequestV1" => Ok(Self::AllternitKernelMutationRequestV1),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for MutationRequestV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for MutationRequestV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`MutationRequestV1MutationType`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum MutationRequestV1MutationType {
    #[serde(rename = "CREATE")]
    Create,
    #[serde(rename = "DELETE")]
    Delete,
    #[serde(rename = "MOVE")]
    Move,
    #[serde(rename = "RENAME")]
    Rename,
    #[serde(rename = "REPLACE_RANGE")]
    ReplaceRange,
    #[serde(rename = "REPLACE_SYMBOL")]
    ReplaceSymbol,
    #[serde(rename = "INSERT_NODE")]
    InsertNode,
    #[serde(rename = "DELETE_NODE")]
    DeleteNode,
    #[serde(rename = "APPLY_PATCH")]
    ApplyPatch,
    #[serde(rename = "CODEMOD")]
    Codemod,
}
impl ::std::fmt::Display for MutationRequestV1MutationType {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Create => f.write_str("CREATE"),
            Self::Delete => f.write_str("DELETE"),
            Self::Move => f.write_str("MOVE"),
            Self::Rename => f.write_str("RENAME"),
            Self::ReplaceRange => f.write_str("REPLACE_RANGE"),
            Self::ReplaceSymbol => f.write_str("REPLACE_SYMBOL"),
            Self::InsertNode => f.write_str("INSERT_NODE"),
            Self::DeleteNode => f.write_str("DELETE_NODE"),
            Self::ApplyPatch => f.write_str("APPLY_PATCH"),
            Self::Codemod => f.write_str("CODEMOD"),
        }
    }
}
impl ::std::str::FromStr for MutationRequestV1MutationType {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "CREATE" => Ok(Self::Create),
            "DELETE" => Ok(Self::Delete),
            "MOVE" => Ok(Self::Move),
            "RENAME" => Ok(Self::Rename),
            "REPLACE_RANGE" => Ok(Self::ReplaceRange),
            "REPLACE_SYMBOL" => Ok(Self::ReplaceSymbol),
            "INSERT_NODE" => Ok(Self::InsertNode),
            "DELETE_NODE" => Ok(Self::DeleteNode),
            "APPLY_PATCH" => Ok(Self::ApplyPatch),
            "CODEMOD" => Ok(Self::Codemod),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for MutationRequestV1MutationType {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for MutationRequestV1MutationType {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`MutationRequestV1SourceRepresentation`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum MutationRequestV1SourceRepresentation {
    #[serde(rename = "UNIFIED_DIFF")]
    UnifiedDiff,
    #[serde(rename = "SEARCH_REPLACE")]
    SearchReplace,
}
impl ::std::fmt::Display for MutationRequestV1SourceRepresentation {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::UnifiedDiff => f.write_str("UNIFIED_DIFF"),
            Self::SearchReplace => f.write_str("SEARCH_REPLACE"),
        }
    }
}
impl ::std::str::FromStr for MutationRequestV1SourceRepresentation {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "UNIFIED_DIFF" => Ok(Self::UnifiedDiff),
            "SEARCH_REPLACE" => Ok(Self::SearchReplace),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for MutationRequestV1SourceRepresentation {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for MutationRequestV1SourceRepresentation {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`NetworkAccessPolicyV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct NetworkAccessPolicyV1 {
    pub defaults_version: ::std::string::String,
    pub dns_rebind_policy: ::serde_json::Value,
    pub domains: ::std::vec::Vec<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub network_default: NetworkAccessPolicyV1NetworkDefault,
    pub policy_id: Id,
    pub private_ip_policy: ::serde_json::Value,
    pub redirect_policy: NetworkAccessPolicyV1RedirectPolicy,
    pub schema_id: ::serde_json::Value,
    pub schema_version: NetworkAccessPolicyV1SchemaVersion,
}
#[doc = "`NetworkAccessPolicyV1NetworkDefault`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum NetworkAccessPolicyV1NetworkDefault {
    #[serde(rename = "DENY")]
    Deny,
    #[serde(rename = "ALLOWLIST")]
    Allowlist,
}
impl ::std::fmt::Display for NetworkAccessPolicyV1NetworkDefault {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Deny => f.write_str("DENY"),
            Self::Allowlist => f.write_str("ALLOWLIST"),
        }
    }
}
impl ::std::str::FromStr for NetworkAccessPolicyV1NetworkDefault {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "DENY" => Ok(Self::Deny),
            "ALLOWLIST" => Ok(Self::Allowlist),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for NetworkAccessPolicyV1NetworkDefault {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for NetworkAccessPolicyV1NetworkDefault {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`NetworkAccessPolicyV1RedirectPolicy`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct NetworkAccessPolicyV1RedirectPolicy {
    pub max_redirects: u64,
    pub scope: NetworkAccessPolicyV1RedirectPolicyScope,
}
#[doc = "`NetworkAccessPolicyV1RedirectPolicyScope`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum NetworkAccessPolicyV1RedirectPolicyScope {
    #[serde(rename = "SAME_DOMAIN")]
    SameDomain,
    #[serde(rename = "ALLOWLIST")]
    Allowlist,
}
impl ::std::fmt::Display for NetworkAccessPolicyV1RedirectPolicyScope {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::SameDomain => f.write_str("SAME_DOMAIN"),
            Self::Allowlist => f.write_str("ALLOWLIST"),
        }
    }
}
impl ::std::str::FromStr for NetworkAccessPolicyV1RedirectPolicyScope {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "SAME_DOMAIN" => Ok(Self::SameDomain),
            "ALLOWLIST" => Ok(Self::Allowlist),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for NetworkAccessPolicyV1RedirectPolicyScope {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for NetworkAccessPolicyV1RedirectPolicyScope {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`NetworkAccessPolicyV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct NetworkAccessPolicyV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for NetworkAccessPolicyV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<NetworkAccessPolicyV1SchemaVersion> for ::std::string::String {
    fn from(value: NetworkAccessPolicyV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for NetworkAccessPolicyV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for NetworkAccessPolicyV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for NetworkAccessPolicyV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for NetworkAccessPolicyV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`NodeOutputV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct NodeOutputV1 {
    pub artifact_id: Id,
    pub created_at: Timestamp,
    pub dag_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub hash: ContentHash,
    pub mime_type: ::std::string::String,
    pub node_id: Id,
    pub producer: ComponentIdentity,
    pub receipt_ref: Id,
    pub schema_id: ::serde_json::Value,
    pub schema_version: NodeOutputV1SchemaVersion,
    pub sensitivity: SensitivityClass,
    pub state_version: StateVersion,
    pub trust_class: TrustClass,
    pub verification_status: NodeOutputV1VerificationStatus,
}
#[doc = "`NodeOutputV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct NodeOutputV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for NodeOutputV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<NodeOutputV1SchemaVersion> for ::std::string::String {
    fn from(value: NodeOutputV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for NodeOutputV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for NodeOutputV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for NodeOutputV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for NodeOutputV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`NodeOutputV1VerificationStatus`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum NodeOutputV1VerificationStatus {
    #[serde(rename = "UNVERIFIED")]
    Unverified,
    #[serde(rename = "PASS")]
    Pass,
    #[serde(rename = "FAIL")]
    Fail,
    #[serde(rename = "INCONCLUSIVE")]
    Inconclusive,
    #[serde(rename = "ERROR")]
    Error,
}
impl ::std::fmt::Display for NodeOutputV1VerificationStatus {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Unverified => f.write_str("UNVERIFIED"),
            Self::Pass => f.write_str("PASS"),
            Self::Fail => f.write_str("FAIL"),
            Self::Inconclusive => f.write_str("INCONCLUSIVE"),
            Self::Error => f.write_str("ERROR"),
        }
    }
}
impl ::std::str::FromStr for NodeOutputV1VerificationStatus {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "UNVERIFIED" => Ok(Self::Unverified),
            "PASS" => Ok(Self::Pass),
            "FAIL" => Ok(Self::Fail),
            "INCONCLUSIVE" => Ok(Self::Inconclusive),
            "ERROR" => Ok(Self::Error),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for NodeOutputV1VerificationStatus {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for NodeOutputV1VerificationStatus {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`OnFailure`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct OnFailure {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub strategy: OnFailureStrategy,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub target: ::std::option::Option<Id>,
}
#[doc = "`OnFailureStrategy`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum OnFailureStrategy {
    #[serde(rename = "RETRY")]
    Retry,
    #[serde(rename = "FALLBACK")]
    Fallback,
    #[serde(rename = "ROLLBACK")]
    Rollback,
    #[serde(rename = "ESCALATE")]
    Escalate,
    #[serde(rename = "FAIL")]
    Fail,
}
impl ::std::fmt::Display for OnFailureStrategy {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Retry => f.write_str("RETRY"),
            Self::Fallback => f.write_str("FALLBACK"),
            Self::Rollback => f.write_str("ROLLBACK"),
            Self::Escalate => f.write_str("ESCALATE"),
            Self::Fail => f.write_str("FAIL"),
        }
    }
}
impl ::std::str::FromStr for OnFailureStrategy {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "RETRY" => Ok(Self::Retry),
            "FALLBACK" => Ok(Self::Fallback),
            "ROLLBACK" => Ok(Self::Rollback),
            "ESCALATE" => Ok(Self::Escalate),
            "FAIL" => Ok(Self::Fail),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for OnFailureStrategy {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for OnFailureStrategy {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PolicyCheckV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PolicyCheckV1 {
    pub check_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub cognitive_risk: ::std::option::Option<f64>,
    pub data_classes: ::std::vec::Vec<SensitivityClass>,
    pub envelope: PolicyCheckV1Envelope,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub external_effects: ::std::vec::Vec<PolicyCheckV1ExternalEffectsItem>,
    pub human_approval_state: PolicyCheckV1HumanApprovalState,
    pub irreversible_class: PolicyCheckV1IrreversibleClass,
    pub principal: Id,
    pub proposed_action: PolicyCheckV1ProposedAction,
    pub resources: ::std::vec::Vec<ResourceRef>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub spend: ::std::option::Option<ResourceBudget>,
    pub task_id: Id,
}
#[doc = "`PolicyCheckV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PolicyCheckV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: PolicyCheckV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`PolicyCheckV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PolicyCheckV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.PolicyCheckV1")]
    AllternitKernelPolicyCheckV1,
}
impl ::std::fmt::Display for PolicyCheckV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelPolicyCheckV1 => f.write_str("allternit.kernel.PolicyCheckV1"),
        }
    }
}
impl ::std::str::FromStr for PolicyCheckV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.PolicyCheckV1" => Ok(Self::AllternitKernelPolicyCheckV1),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PolicyCheckV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PolicyCheckV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PolicyCheckV1ExternalEffectsItem`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PolicyCheckV1ExternalEffectsItem {
    #[serde(rename = "NONE")]
    None,
    #[serde(rename = "READ")]
    Read,
    #[serde(rename = "WORKSPACE_WRITE")]
    WorkspaceWrite,
    #[serde(rename = "EXECUTE")]
    Execute,
    #[serde(rename = "NETWORK")]
    Network,
    #[serde(rename = "EXTERNAL_WRITE")]
    ExternalWrite,
    #[serde(rename = "FINANCIAL")]
    Financial,
    #[serde(rename = "PUBLISH")]
    Publish,
    #[serde(rename = "PERMISSION_CHANGE")]
    PermissionChange,
}
impl ::std::fmt::Display for PolicyCheckV1ExternalEffectsItem {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::None => f.write_str("NONE"),
            Self::Read => f.write_str("READ"),
            Self::WorkspaceWrite => f.write_str("WORKSPACE_WRITE"),
            Self::Execute => f.write_str("EXECUTE"),
            Self::Network => f.write_str("NETWORK"),
            Self::ExternalWrite => f.write_str("EXTERNAL_WRITE"),
            Self::Financial => f.write_str("FINANCIAL"),
            Self::Publish => f.write_str("PUBLISH"),
            Self::PermissionChange => f.write_str("PERMISSION_CHANGE"),
        }
    }
}
impl ::std::str::FromStr for PolicyCheckV1ExternalEffectsItem {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "NONE" => Ok(Self::None),
            "READ" => Ok(Self::Read),
            "WORKSPACE_WRITE" => Ok(Self::WorkspaceWrite),
            "EXECUTE" => Ok(Self::Execute),
            "NETWORK" => Ok(Self::Network),
            "EXTERNAL_WRITE" => Ok(Self::ExternalWrite),
            "FINANCIAL" => Ok(Self::Financial),
            "PUBLISH" => Ok(Self::Publish),
            "PERMISSION_CHANGE" => Ok(Self::PermissionChange),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PolicyCheckV1ExternalEffectsItem {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PolicyCheckV1ExternalEffectsItem {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PolicyCheckV1HumanApprovalState`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PolicyCheckV1HumanApprovalState {
    #[serde(rename = "NOT_REQUIRED")]
    NotRequired,
    #[serde(rename = "REQUIRED_PENDING")]
    RequiredPending,
    #[serde(rename = "APPROVED")]
    Approved,
    #[serde(rename = "REJECTED")]
    Rejected,
    #[serde(rename = "EXPIRED")]
    Expired,
}
impl ::std::fmt::Display for PolicyCheckV1HumanApprovalState {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::NotRequired => f.write_str("NOT_REQUIRED"),
            Self::RequiredPending => f.write_str("REQUIRED_PENDING"),
            Self::Approved => f.write_str("APPROVED"),
            Self::Rejected => f.write_str("REJECTED"),
            Self::Expired => f.write_str("EXPIRED"),
        }
    }
}
impl ::std::str::FromStr for PolicyCheckV1HumanApprovalState {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "NOT_REQUIRED" => Ok(Self::NotRequired),
            "REQUIRED_PENDING" => Ok(Self::RequiredPending),
            "APPROVED" => Ok(Self::Approved),
            "REJECTED" => Ok(Self::Rejected),
            "EXPIRED" => Ok(Self::Expired),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PolicyCheckV1HumanApprovalState {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PolicyCheckV1HumanApprovalState {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PolicyCheckV1IrreversibleClass`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PolicyCheckV1IrreversibleClass {
    #[serde(rename = "REVERSIBLE")]
    Reversible,
    #[serde(rename = "COMPENSATABLE")]
    Compensatable,
    #[serde(rename = "IRREVERSIBLE")]
    Irreversible,
}
impl ::std::fmt::Display for PolicyCheckV1IrreversibleClass {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Reversible => f.write_str("REVERSIBLE"),
            Self::Compensatable => f.write_str("COMPENSATABLE"),
            Self::Irreversible => f.write_str("IRREVERSIBLE"),
        }
    }
}
impl ::std::str::FromStr for PolicyCheckV1IrreversibleClass {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "REVERSIBLE" => Ok(Self::Reversible),
            "COMPENSATABLE" => Ok(Self::Compensatable),
            "IRREVERSIBLE" => Ok(Self::Irreversible),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PolicyCheckV1IrreversibleClass {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PolicyCheckV1IrreversibleClass {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PolicyCheckV1ProposedAction`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PolicyCheckV1ProposedAction {
    pub action_class: PolicyCheckV1ProposedActionActionClass,
    pub primitive_id: PrimitiveId,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub tool_id: ::std::option::Option<Id>,
}
#[doc = "`PolicyCheckV1ProposedActionActionClass`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PolicyCheckV1ProposedActionActionClass {
    #[serde(rename = "READ")]
    Read,
    #[serde(rename = "WRITE")]
    Write,
    #[serde(rename = "EXECUTE")]
    Execute,
    #[serde(rename = "NETWORK")]
    Network,
    #[serde(rename = "FINANCIAL")]
    Financial,
    #[serde(rename = "PUBLISH")]
    Publish,
    #[serde(rename = "PERMISSION_CHANGE")]
    PermissionChange,
    #[serde(rename = "SPAWN")]
    Spawn,
}
impl ::std::fmt::Display for PolicyCheckV1ProposedActionActionClass {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Read => f.write_str("READ"),
            Self::Write => f.write_str("WRITE"),
            Self::Execute => f.write_str("EXECUTE"),
            Self::Network => f.write_str("NETWORK"),
            Self::Financial => f.write_str("FINANCIAL"),
            Self::Publish => f.write_str("PUBLISH"),
            Self::PermissionChange => f.write_str("PERMISSION_CHANGE"),
            Self::Spawn => f.write_str("SPAWN"),
        }
    }
}
impl ::std::str::FromStr for PolicyCheckV1ProposedActionActionClass {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "READ" => Ok(Self::Read),
            "WRITE" => Ok(Self::Write),
            "EXECUTE" => Ok(Self::Execute),
            "NETWORK" => Ok(Self::Network),
            "FINANCIAL" => Ok(Self::Financial),
            "PUBLISH" => Ok(Self::Publish),
            "PERMISSION_CHANGE" => Ok(Self::PermissionChange),
            "SPAWN" => Ok(Self::Spawn),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PolicyCheckV1ProposedActionActionClass {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PolicyCheckV1ProposedActionActionClass {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PolicyDecisionV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PolicyDecisionV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub attention_id: ::std::option::Option<Id>,
    pub check_id: Id,
    pub decision: PolicyDecisionV1Decision,
    pub envelope: PolicyDecisionV1Envelope,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub expires_at: ::std::option::Option<Timestamp>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub grants: ::std::vec::Vec<CapabilityGrant>,
    pub hard_policy: bool,
    pub limits: ::std::vec::Vec<::serde_json::Map<::std::string::String, ::serde_json::Value>>,
    pub precedence: PolicyDecisionV1Precedence,
    pub reason_codes: ::std::vec::Vec<::std::string::String>,
    pub receipt_id: Id,
}
#[doc = "`PolicyDecisionV1Decision`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PolicyDecisionV1Decision {
    #[serde(rename = "ALLOW")]
    Allow,
    #[serde(rename = "ALLOW_WITH_LIMITS")]
    AllowWithLimits,
    #[serde(rename = "ASK")]
    Ask,
    #[serde(rename = "DENY")]
    Deny,
}
impl ::std::fmt::Display for PolicyDecisionV1Decision {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Allow => f.write_str("ALLOW"),
            Self::AllowWithLimits => f.write_str("ALLOW_WITH_LIMITS"),
            Self::Ask => f.write_str("ASK"),
            Self::Deny => f.write_str("DENY"),
        }
    }
}
impl ::std::str::FromStr for PolicyDecisionV1Decision {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "ALLOW" => Ok(Self::Allow),
            "ALLOW_WITH_LIMITS" => Ok(Self::AllowWithLimits),
            "ASK" => Ok(Self::Ask),
            "DENY" => Ok(Self::Deny),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PolicyDecisionV1Decision {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PolicyDecisionV1Decision {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PolicyDecisionV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PolicyDecisionV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: PolicyDecisionV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`PolicyDecisionV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PolicyDecisionV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.PolicyDecisionV1")]
    AllternitKernelPolicyDecisionV1,
}
impl ::std::fmt::Display for PolicyDecisionV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelPolicyDecisionV1 => {
                f.write_str("allternit.kernel.PolicyDecisionV1")
            }
        }
    }
}
impl ::std::str::FromStr for PolicyDecisionV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.PolicyDecisionV1" => Ok(Self::AllternitKernelPolicyDecisionV1),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PolicyDecisionV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PolicyDecisionV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PolicyDecisionV1Precedence`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PolicyDecisionV1Precedence {
    #[serde(rename = "HARD_DENY")]
    HardDeny,
    #[serde(rename = "HARD_ALLOW")]
    HardAllow,
    #[serde(rename = "SOFT")]
    Soft,
    #[serde(rename = "COGNITIVE")]
    Cognitive,
    #[serde(rename = "FAIL_CLOSED")]
    FailClosed,
}
impl ::std::fmt::Display for PolicyDecisionV1Precedence {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::HardDeny => f.write_str("HARD_DENY"),
            Self::HardAllow => f.write_str("HARD_ALLOW"),
            Self::Soft => f.write_str("SOFT"),
            Self::Cognitive => f.write_str("COGNITIVE"),
            Self::FailClosed => f.write_str("FAIL_CLOSED"),
        }
    }
}
impl ::std::str::FromStr for PolicyDecisionV1Precedence {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "HARD_DENY" => Ok(Self::HardDeny),
            "HARD_ALLOW" => Ok(Self::HardAllow),
            "SOFT" => Ok(Self::Soft),
            "COGNITIVE" => Ok(Self::Cognitive),
            "FAIL_CLOSED" => Ok(Self::FailClosed),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PolicyDecisionV1Precedence {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PolicyDecisionV1Precedence {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PolicyReceiptV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PolicyReceiptV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub attention_id: ::std::option::Option<Id>,
    pub chain: ReceiptChainV1,
    pub check_id: Id,
    pub decision: PolicyReceiptV1Decision,
    pub decision_id: Id,
    pub envelope: PolicyReceiptV1Envelope,
    pub evaluated_rules: ::std::vec::Vec<PolicyReceiptV1EvaluatedRulesItem>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub grant_ids: ::std::vec::Vec<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub judge_ref: ::std::option::Option<Id>,
    pub precedence: PolicyReceiptV1Precedence,
    pub reason_codes: ::std::vec::Vec<::std::string::String>,
}
#[doc = "`PolicyReceiptV1Decision`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PolicyReceiptV1Decision {
    #[serde(rename = "ALLOW")]
    Allow,
    #[serde(rename = "ALLOW_WITH_LIMITS")]
    AllowWithLimits,
    #[serde(rename = "ASK")]
    Ask,
    #[serde(rename = "DENY")]
    Deny,
}
impl ::std::fmt::Display for PolicyReceiptV1Decision {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Allow => f.write_str("ALLOW"),
            Self::AllowWithLimits => f.write_str("ALLOW_WITH_LIMITS"),
            Self::Ask => f.write_str("ASK"),
            Self::Deny => f.write_str("DENY"),
        }
    }
}
impl ::std::str::FromStr for PolicyReceiptV1Decision {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "ALLOW" => Ok(Self::Allow),
            "ALLOW_WITH_LIMITS" => Ok(Self::AllowWithLimits),
            "ASK" => Ok(Self::Ask),
            "DENY" => Ok(Self::Deny),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PolicyReceiptV1Decision {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PolicyReceiptV1Decision {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PolicyReceiptV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PolicyReceiptV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: PolicyReceiptV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`PolicyReceiptV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PolicyReceiptV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.PolicyReceiptV1")]
    AllternitKernelPolicyReceiptV1,
}
impl ::std::fmt::Display for PolicyReceiptV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelPolicyReceiptV1 => f.write_str("allternit.kernel.PolicyReceiptV1"),
        }
    }
}
impl ::std::str::FromStr for PolicyReceiptV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.PolicyReceiptV1" => Ok(Self::AllternitKernelPolicyReceiptV1),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PolicyReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PolicyReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PolicyReceiptV1EvaluatedRulesItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PolicyReceiptV1EvaluatedRulesItem {
    pub outcome: PolicyReceiptV1EvaluatedRulesItemOutcome,
    pub rule_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub rule_version: ::std::option::Option<::std::string::String>,
}
#[doc = "`PolicyReceiptV1EvaluatedRulesItemOutcome`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PolicyReceiptV1EvaluatedRulesItemOutcome {
    #[serde(rename = "MATCH")]
    Match,
    #[serde(rename = "NO_MATCH")]
    NoMatch,
    #[serde(rename = "ERROR")]
    Error,
}
impl ::std::fmt::Display for PolicyReceiptV1EvaluatedRulesItemOutcome {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Match => f.write_str("MATCH"),
            Self::NoMatch => f.write_str("NO_MATCH"),
            Self::Error => f.write_str("ERROR"),
        }
    }
}
impl ::std::str::FromStr for PolicyReceiptV1EvaluatedRulesItemOutcome {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "MATCH" => Ok(Self::Match),
            "NO_MATCH" => Ok(Self::NoMatch),
            "ERROR" => Ok(Self::Error),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PolicyReceiptV1EvaluatedRulesItemOutcome {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PolicyReceiptV1EvaluatedRulesItemOutcome {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PolicyReceiptV1Precedence`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PolicyReceiptV1Precedence {
    #[serde(rename = "HARD_DENY")]
    HardDeny,
    #[serde(rename = "HARD_ALLOW")]
    HardAllow,
    #[serde(rename = "SOFT")]
    Soft,
    #[serde(rename = "COGNITIVE")]
    Cognitive,
    #[serde(rename = "FAIL_CLOSED")]
    FailClosed,
}
impl ::std::fmt::Display for PolicyReceiptV1Precedence {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::HardDeny => f.write_str("HARD_DENY"),
            Self::HardAllow => f.write_str("HARD_ALLOW"),
            Self::Soft => f.write_str("SOFT"),
            Self::Cognitive => f.write_str("COGNITIVE"),
            Self::FailClosed => f.write_str("FAIL_CLOSED"),
        }
    }
}
impl ::std::str::FromStr for PolicyReceiptV1Precedence {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "HARD_DENY" => Ok(Self::HardDeny),
            "HARD_ALLOW" => Ok(Self::HardAllow),
            "SOFT" => Ok(Self::Soft),
            "COGNITIVE" => Ok(Self::Cognitive),
            "FAIL_CLOSED" => Ok(Self::FailClosed),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PolicyReceiptV1Precedence {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PolicyReceiptV1Precedence {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PrimitiveDescriptorV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PrimitiveDescriptorV1 {
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub aliases: ::std::vec::Vec<PrimitiveDescriptorV1AliasesItem>,
    pub authority: PrimitiveDescriptorV1Authority,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub capability_requirements: ::std::vec::Vec<CapabilityId>,
    pub class: ::std::vec::Vec<PrimitiveDescriptorV1ClassItem>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub default_role: ::std::option::Option<PrimitiveDescriptorV1DefaultRole>,
    pub determinism: PrimitiveDescriptorV1Determinism,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub eval_ref: ::std::option::Option<Id>,
    pub evidence_required: ::std::vec::Vec<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub failure_modes: ::std::vec::Vec<::std::string::String>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub fallback: ::std::vec::Vec<PrimitiveId>,
    pub idempotency: PrimitiveDescriptorV1Idempotency,
    pub input_schema: ::std::string::String,
    pub output_schema: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub plugin_id: ::std::option::Option<Id>,
    pub primitive_id: PrimitiveId,
    pub read_set: ::std::vec::Vec<ResourceRef>,
    pub reversibility: PrimitiveDescriptorV1Reversibility,
    pub schema_id: ::serde_json::Value,
    pub schema_version: PrimitiveDescriptorV1SchemaVersion,
    pub side_effect_class: PrimitiveDescriptorV1SideEffectClass,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub timeout_ms: ::std::option::Option<u64>,
    pub write_set: ::std::vec::Vec<ResourceRef>,
}
#[doc = "`PrimitiveDescriptorV1AliasesItem`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct PrimitiveDescriptorV1AliasesItem(::std::string::String);
impl ::std::ops::Deref for PrimitiveDescriptorV1AliasesItem {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<PrimitiveDescriptorV1AliasesItem> for ::std::string::String {
    fn from(value: PrimitiveDescriptorV1AliasesItem) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for PrimitiveDescriptorV1AliasesItem {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^[A-Z][A-Z0-9_]*$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[A-Z][A-Z0-9_]*$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveDescriptorV1AliasesItem {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveDescriptorV1AliasesItem {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for PrimitiveDescriptorV1AliasesItem {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`PrimitiveDescriptorV1Authority`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PrimitiveDescriptorV1Authority {
    #[serde(rename = "PROPOSE")]
    Propose,
    #[serde(rename = "DECIDE")]
    Decide,
    #[serde(rename = "VERIFY")]
    Verify,
    #[serde(rename = "AUTHORIZE")]
    Authorize,
}
impl ::std::fmt::Display for PrimitiveDescriptorV1Authority {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Propose => f.write_str("PROPOSE"),
            Self::Decide => f.write_str("DECIDE"),
            Self::Verify => f.write_str("VERIFY"),
            Self::Authorize => f.write_str("AUTHORIZE"),
        }
    }
}
impl ::std::str::FromStr for PrimitiveDescriptorV1Authority {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "PROPOSE" => Ok(Self::Propose),
            "DECIDE" => Ok(Self::Decide),
            "VERIFY" => Ok(Self::Verify),
            "AUTHORIZE" => Ok(Self::Authorize),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveDescriptorV1Authority {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveDescriptorV1Authority {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PrimitiveDescriptorV1ClassItem`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PrimitiveDescriptorV1ClassItem {
    D,
    S,
    G,
    V,
    P,
    M,
    R,
}
impl ::std::fmt::Display for PrimitiveDescriptorV1ClassItem {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::D => f.write_str("D"),
            Self::S => f.write_str("S"),
            Self::G => f.write_str("G"),
            Self::V => f.write_str("V"),
            Self::P => f.write_str("P"),
            Self::M => f.write_str("M"),
            Self::R => f.write_str("R"),
        }
    }
}
impl ::std::str::FromStr for PrimitiveDescriptorV1ClassItem {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "D" => Ok(Self::D),
            "S" => Ok(Self::S),
            "G" => Ok(Self::G),
            "V" => Ok(Self::V),
            "P" => Ok(Self::P),
            "M" => Ok(Self::M),
            "R" => Ok(Self::R),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveDescriptorV1ClassItem {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveDescriptorV1ClassItem {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PrimitiveDescriptorV1DefaultRole`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PrimitiveDescriptorV1DefaultRole {
    S0,
    S1,
    S2,
    S3,
}
impl ::std::fmt::Display for PrimitiveDescriptorV1DefaultRole {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::S0 => f.write_str("S0"),
            Self::S1 => f.write_str("S1"),
            Self::S2 => f.write_str("S2"),
            Self::S3 => f.write_str("S3"),
        }
    }
}
impl ::std::str::FromStr for PrimitiveDescriptorV1DefaultRole {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "S0" => Ok(Self::S0),
            "S1" => Ok(Self::S1),
            "S2" => Ok(Self::S2),
            "S3" => Ok(Self::S3),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveDescriptorV1DefaultRole {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveDescriptorV1DefaultRole {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PrimitiveDescriptorV1Determinism`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PrimitiveDescriptorV1Determinism {
    #[serde(rename = "DETERMINISTIC")]
    Deterministic,
    #[serde(rename = "PROBABILISTIC")]
    Probabilistic,
    #[serde(rename = "GENERATIVE")]
    Generative,
}
impl ::std::fmt::Display for PrimitiveDescriptorV1Determinism {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Deterministic => f.write_str("DETERMINISTIC"),
            Self::Probabilistic => f.write_str("PROBABILISTIC"),
            Self::Generative => f.write_str("GENERATIVE"),
        }
    }
}
impl ::std::str::FromStr for PrimitiveDescriptorV1Determinism {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "DETERMINISTIC" => Ok(Self::Deterministic),
            "PROBABILISTIC" => Ok(Self::Probabilistic),
            "GENERATIVE" => Ok(Self::Generative),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveDescriptorV1Determinism {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveDescriptorV1Determinism {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PrimitiveDescriptorV1Idempotency`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PrimitiveDescriptorV1Idempotency {
    #[serde(rename = "IDEMPOTENT")]
    Idempotent,
    #[serde(rename = "DEDUPLICATED")]
    Deduplicated,
    #[serde(rename = "NON_IDEMPOTENT")]
    NonIdempotent,
}
impl ::std::fmt::Display for PrimitiveDescriptorV1Idempotency {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Idempotent => f.write_str("IDEMPOTENT"),
            Self::Deduplicated => f.write_str("DEDUPLICATED"),
            Self::NonIdempotent => f.write_str("NON_IDEMPOTENT"),
        }
    }
}
impl ::std::str::FromStr for PrimitiveDescriptorV1Idempotency {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "IDEMPOTENT" => Ok(Self::Idempotent),
            "DEDUPLICATED" => Ok(Self::Deduplicated),
            "NON_IDEMPOTENT" => Ok(Self::NonIdempotent),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveDescriptorV1Idempotency {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveDescriptorV1Idempotency {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PrimitiveDescriptorV1Reversibility`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PrimitiveDescriptorV1Reversibility {
    #[serde(rename = "REVERSIBLE")]
    Reversible,
    #[serde(rename = "COMPENSATABLE")]
    Compensatable,
    #[serde(rename = "IRREVERSIBLE")]
    Irreversible,
}
impl ::std::fmt::Display for PrimitiveDescriptorV1Reversibility {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Reversible => f.write_str("REVERSIBLE"),
            Self::Compensatable => f.write_str("COMPENSATABLE"),
            Self::Irreversible => f.write_str("IRREVERSIBLE"),
        }
    }
}
impl ::std::str::FromStr for PrimitiveDescriptorV1Reversibility {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "REVERSIBLE" => Ok(Self::Reversible),
            "COMPENSATABLE" => Ok(Self::Compensatable),
            "IRREVERSIBLE" => Ok(Self::Irreversible),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveDescriptorV1Reversibility {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveDescriptorV1Reversibility {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PrimitiveDescriptorV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct PrimitiveDescriptorV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for PrimitiveDescriptorV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<PrimitiveDescriptorV1SchemaVersion> for ::std::string::String {
    fn from(value: PrimitiveDescriptorV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for PrimitiveDescriptorV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveDescriptorV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveDescriptorV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for PrimitiveDescriptorV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`PrimitiveDescriptorV1SideEffectClass`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PrimitiveDescriptorV1SideEffectClass {
    #[serde(rename = "NONE")]
    None,
    #[serde(rename = "READ")]
    Read,
    #[serde(rename = "WORKSPACE_WRITE")]
    WorkspaceWrite,
    #[serde(rename = "EXECUTE")]
    Execute,
    #[serde(rename = "NETWORK")]
    Network,
    #[serde(rename = "EXTERNAL_WRITE")]
    ExternalWrite,
    #[serde(rename = "FINANCIAL")]
    Financial,
    #[serde(rename = "PUBLISH")]
    Publish,
    #[serde(rename = "PERMISSION_CHANGE")]
    PermissionChange,
}
impl ::std::fmt::Display for PrimitiveDescriptorV1SideEffectClass {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::None => f.write_str("NONE"),
            Self::Read => f.write_str("READ"),
            Self::WorkspaceWrite => f.write_str("WORKSPACE_WRITE"),
            Self::Execute => f.write_str("EXECUTE"),
            Self::Network => f.write_str("NETWORK"),
            Self::ExternalWrite => f.write_str("EXTERNAL_WRITE"),
            Self::Financial => f.write_str("FINANCIAL"),
            Self::Publish => f.write_str("PUBLISH"),
            Self::PermissionChange => f.write_str("PERMISSION_CHANGE"),
        }
    }
}
impl ::std::str::FromStr for PrimitiveDescriptorV1SideEffectClass {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "NONE" => Ok(Self::None),
            "READ" => Ok(Self::Read),
            "WORKSPACE_WRITE" => Ok(Self::WorkspaceWrite),
            "EXECUTE" => Ok(Self::Execute),
            "NETWORK" => Ok(Self::Network),
            "EXTERNAL_WRITE" => Ok(Self::ExternalWrite),
            "FINANCIAL" => Ok(Self::Financial),
            "PUBLISH" => Ok(Self::Publish),
            "PERMISSION_CHANGE" => Ok(Self::PermissionChange),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveDescriptorV1SideEffectClass {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveDescriptorV1SideEffectClass {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PrimitiveId`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct PrimitiveId(::std::string::String);
impl ::std::ops::Deref for PrimitiveId {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<PrimitiveId> for ::std::string::String {
    fn from(value: PrimitiveId) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for PrimitiveId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> = ::std::sync::LazyLock::new(
            || {
                :: regress :: Regex :: new ("^(obs|dec|ctx|plan|tool|mut|ver|ctl|route|state|policy|bg|mem)(\\.[a-z0-9_]+)+$") . unwrap ()
            },
        );
        if PATTERN.find(value).is_none() {
            return Err ("doesn't match pattern \"^(obs|dec|ctx|plan|tool|mut|ver|ctl|route|state|policy|bg|mem)(\\.[a-z0-9_]+)+$\"" . into ()) ;
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for PrimitiveId {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`PrimitiveMaturityV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PrimitiveMaturityV1 {
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub active_profile_id: ::std::option::Option<Id>,
    pub authority_stage: PrimitiveMaturityV1AuthorityStage,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub calibration_manifest_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub drift_status: ::std::option::Option<PrimitiveMaturityV1DriftStatus>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub fallback_profile_id: ::std::option::Option<Id>,
    pub labels: u64,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub last_fit_at: ::std::option::Option<Timestamp>,
    pub observations: u64,
    pub primitive_id: PrimitiveId,
    pub schema_id: ::serde_json::Value,
    pub schema_version: PrimitiveMaturityV1SchemaVersion,
    pub stage: PrimitiveMaturityV1Stage,
}
#[doc = "`PrimitiveMaturityV1AuthorityStage`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PrimitiveMaturityV1AuthorityStage {
    #[serde(rename = "SHADOW")]
    Shadow,
    #[serde(rename = "AUTO_ACT_REVERSIBLE")]
    AutoActReversible,
    #[serde(rename = "AUTO_ACT_EXPANDED")]
    AutoActExpanded,
}
impl ::std::fmt::Display for PrimitiveMaturityV1AuthorityStage {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Shadow => f.write_str("SHADOW"),
            Self::AutoActReversible => f.write_str("AUTO_ACT_REVERSIBLE"),
            Self::AutoActExpanded => f.write_str("AUTO_ACT_EXPANDED"),
        }
    }
}
impl ::std::str::FromStr for PrimitiveMaturityV1AuthorityStage {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "SHADOW" => Ok(Self::Shadow),
            "AUTO_ACT_REVERSIBLE" => Ok(Self::AutoActReversible),
            "AUTO_ACT_EXPANDED" => Ok(Self::AutoActExpanded),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveMaturityV1AuthorityStage {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveMaturityV1AuthorityStage {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PrimitiveMaturityV1DriftStatus`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PrimitiveMaturityV1DriftStatus {
    #[serde(rename = "UNKNOWN")]
    Unknown,
    #[serde(rename = "STABLE")]
    Stable,
    #[serde(rename = "WATCH")]
    Watch,
    #[serde(rename = "INVALIDATE")]
    Invalidate,
}
impl ::std::fmt::Display for PrimitiveMaturityV1DriftStatus {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Unknown => f.write_str("UNKNOWN"),
            Self::Stable => f.write_str("STABLE"),
            Self::Watch => f.write_str("WATCH"),
            Self::Invalidate => f.write_str("INVALIDATE"),
        }
    }
}
impl ::std::str::FromStr for PrimitiveMaturityV1DriftStatus {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "UNKNOWN" => Ok(Self::Unknown),
            "STABLE" => Ok(Self::Stable),
            "WATCH" => Ok(Self::Watch),
            "INVALIDATE" => Ok(Self::Invalidate),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveMaturityV1DriftStatus {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveMaturityV1DriftStatus {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PrimitiveMaturityV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct PrimitiveMaturityV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for PrimitiveMaturityV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<PrimitiveMaturityV1SchemaVersion> for ::std::string::String {
    fn from(value: PrimitiveMaturityV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for PrimitiveMaturityV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveMaturityV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveMaturityV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for PrimitiveMaturityV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`PrimitiveMaturityV1Stage`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PrimitiveMaturityV1Stage {
    #[serde(rename = "UNTRAINED")]
    Untrained,
    #[serde(rename = "BOOTSTRAP_L0")]
    BootstrapL0,
    #[serde(rename = "CALIBRATED_L1")]
    CalibratedL1,
    #[serde(rename = "SPECIALIZED_L2")]
    SpecializedL2,
    #[serde(rename = "MONITORED_PRODUCTION")]
    MonitoredProduction,
    #[serde(rename = "INVALIDATED")]
    Invalidated,
}
impl ::std::fmt::Display for PrimitiveMaturityV1Stage {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Untrained => f.write_str("UNTRAINED"),
            Self::BootstrapL0 => f.write_str("BOOTSTRAP_L0"),
            Self::CalibratedL1 => f.write_str("CALIBRATED_L1"),
            Self::SpecializedL2 => f.write_str("SPECIALIZED_L2"),
            Self::MonitoredProduction => f.write_str("MONITORED_PRODUCTION"),
            Self::Invalidated => f.write_str("INVALIDATED"),
        }
    }
}
impl ::std::str::FromStr for PrimitiveMaturityV1Stage {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "UNTRAINED" => Ok(Self::Untrained),
            "BOOTSTRAP_L0" => Ok(Self::BootstrapL0),
            "CALIBRATED_L1" => Ok(Self::CalibratedL1),
            "SPECIALIZED_L2" => Ok(Self::SpecializedL2),
            "MONITORED_PRODUCTION" => Ok(Self::MonitoredProduction),
            "INVALIDATED" => Ok(Self::Invalidated),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveMaturityV1Stage {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveMaturityV1Stage {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PrimitiveRegistryV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PrimitiveRegistryV1 {
    #[serde(default, skip_serializing_if = "::serde_json::Map::is_empty")]
    pub class_legend: ::serde_json::Map<::std::string::String, ::serde_json::Value>,
    pub count: u64,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub primitives: ::std::vec::Vec<PrimitiveRegistryV1PrimitivesItem>,
    pub registry_version: SemVer,
    pub schema_id: ::serde_json::Value,
    pub schema_version: PrimitiveRegistryV1SchemaVersion,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub source: ::std::option::Option<::std::string::String>,
}
#[doc = "`PrimitiveRegistryV1PrimitivesItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PrimitiveRegistryV1PrimitivesItem {
    pub alias: PrimitiveRegistryV1PrimitivesItemAlias,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub class: ::std::option::Option<::std::vec::Vec<PrimitiveRegistryV1PrimitivesItemClassItem>>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub extra_aliases: ::std::vec::Vec<PrimitiveRegistryV1PrimitivesItemExtraAliasesItem>,
    pub family: ::std::string::String,
    pub id: PrimitiveId,
    pub ledger_rows: ::std::vec::Vec<PrimitiveRegistryV1PrimitivesItemLedgerRowsItem>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub note: ::std::option::Option<::std::string::String>,
}
#[doc = "`PrimitiveRegistryV1PrimitivesItemAlias`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct PrimitiveRegistryV1PrimitivesItemAlias(::std::string::String);
impl ::std::ops::Deref for PrimitiveRegistryV1PrimitivesItemAlias {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<PrimitiveRegistryV1PrimitivesItemAlias> for ::std::string::String {
    fn from(value: PrimitiveRegistryV1PrimitivesItemAlias) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for PrimitiveRegistryV1PrimitivesItemAlias {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^[A-Z][A-Z0-9_]*$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[A-Z][A-Z0-9_]*$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveRegistryV1PrimitivesItemAlias {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveRegistryV1PrimitivesItemAlias {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for PrimitiveRegistryV1PrimitivesItemAlias {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`PrimitiveRegistryV1PrimitivesItemClassItem`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum PrimitiveRegistryV1PrimitivesItemClassItem {
    D,
    S,
    G,
    V,
    P,
    M,
    R,
}
impl ::std::fmt::Display for PrimitiveRegistryV1PrimitivesItemClassItem {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::D => f.write_str("D"),
            Self::S => f.write_str("S"),
            Self::G => f.write_str("G"),
            Self::V => f.write_str("V"),
            Self::P => f.write_str("P"),
            Self::M => f.write_str("M"),
            Self::R => f.write_str("R"),
        }
    }
}
impl ::std::str::FromStr for PrimitiveRegistryV1PrimitivesItemClassItem {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "D" => Ok(Self::D),
            "S" => Ok(Self::S),
            "G" => Ok(Self::G),
            "V" => Ok(Self::V),
            "P" => Ok(Self::P),
            "M" => Ok(Self::M),
            "R" => Ok(Self::R),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveRegistryV1PrimitivesItemClassItem {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveRegistryV1PrimitivesItemClassItem {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`PrimitiveRegistryV1PrimitivesItemExtraAliasesItem`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct PrimitiveRegistryV1PrimitivesItemExtraAliasesItem(::std::string::String);
impl ::std::ops::Deref for PrimitiveRegistryV1PrimitivesItemExtraAliasesItem {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<PrimitiveRegistryV1PrimitivesItemExtraAliasesItem>
    for ::std::string::String
{
    fn from(value: PrimitiveRegistryV1PrimitivesItemExtraAliasesItem) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for PrimitiveRegistryV1PrimitivesItemExtraAliasesItem {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^[A-Z][A-Z0-9_]*$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[A-Z][A-Z0-9_]*$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveRegistryV1PrimitivesItemExtraAliasesItem {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String>
    for PrimitiveRegistryV1PrimitivesItemExtraAliasesItem
{
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for PrimitiveRegistryV1PrimitivesItemExtraAliasesItem {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`PrimitiveRegistryV1PrimitivesItemLedgerRowsItem`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct PrimitiveRegistryV1PrimitivesItemLedgerRowsItem(::std::string::String);
impl ::std::ops::Deref for PrimitiveRegistryV1PrimitivesItemLedgerRowsItem {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<PrimitiveRegistryV1PrimitivesItemLedgerRowsItem>
    for ::std::string::String
{
    fn from(value: PrimitiveRegistryV1PrimitivesItemLedgerRowsItem) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for PrimitiveRegistryV1PrimitivesItemLedgerRowsItem {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^CL-\\d{3}$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^CL-\\d{3}$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveRegistryV1PrimitivesItemLedgerRowsItem {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String>
    for PrimitiveRegistryV1PrimitivesItemLedgerRowsItem
{
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for PrimitiveRegistryV1PrimitivesItemLedgerRowsItem {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`PrimitiveRegistryV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct PrimitiveRegistryV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for PrimitiveRegistryV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<PrimitiveRegistryV1SchemaVersion> for ::std::string::String {
    fn from(value: PrimitiveRegistryV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for PrimitiveRegistryV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for PrimitiveRegistryV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for PrimitiveRegistryV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for PrimitiveRegistryV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`Provenance`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub content_hash: ::std::option::Option<ContentHash>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub observed_at: ::std::option::Option<Timestamp>,
    pub source_id: ::std::string::String,
    pub source_type: ProvenanceSourceType,
    pub trust_class: TrustClass,
}
#[doc = "`ProvenanceSourceType`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ProvenanceSourceType {
    #[serde(rename = "USER")]
    User,
    #[serde(rename = "FILE")]
    File,
    #[serde(rename = "TOOL")]
    Tool,
    #[serde(rename = "MODEL")]
    Model,
    #[serde(rename = "POLICY")]
    Policy,
    #[serde(rename = "SYSTEM")]
    System,
    #[serde(rename = "CACHE")]
    Cache,
    #[serde(rename = "RETRIEVAL")]
    Retrieval,
}
impl ::std::fmt::Display for ProvenanceSourceType {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::User => f.write_str("USER"),
            Self::File => f.write_str("FILE"),
            Self::Tool => f.write_str("TOOL"),
            Self::Model => f.write_str("MODEL"),
            Self::Policy => f.write_str("POLICY"),
            Self::System => f.write_str("SYSTEM"),
            Self::Cache => f.write_str("CACHE"),
            Self::Retrieval => f.write_str("RETRIEVAL"),
        }
    }
}
impl ::std::str::FromStr for ProvenanceSourceType {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "USER" => Ok(Self::User),
            "FILE" => Ok(Self::File),
            "TOOL" => Ok(Self::Tool),
            "MODEL" => Ok(Self::Model),
            "POLICY" => Ok(Self::Policy),
            "SYSTEM" => Ok(Self::System),
            "CACHE" => Ok(Self::Cache),
            "RETRIEVAL" => Ok(Self::Retrieval),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ProvenanceSourceType {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ProvenanceSourceType {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ReceiptChainV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ReceiptChainV1 {
    pub canonicalization: ::serde_json::Value,
    pub content_hash: ContentHash,
    pub domain: ::serde_json::Value,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub idempotency_key: ::std::option::Option<::std::string::String>,
    pub index: u64,
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub prev_hash: ::std::option::Option<ContentHash>,
    pub receipt_id: Id,
    pub run_id: Id,
    pub schema_id: ReceiptChainV1SchemaId,
    pub schema_version: SemVer,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub signature: ::std::option::Option<ReceiptSignature>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub supersedes: ::std::option::Option<Id>,
}
#[doc = "`ReceiptChainV1SchemaId`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ReceiptChainV1SchemaId(::std::string::String);
impl ::std::ops::Deref for ReceiptChainV1SchemaId {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ReceiptChainV1SchemaId> for ::std::string::String {
    fn from(value: ReceiptChainV1SchemaId) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ReceiptChainV1SchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^allternit\\.kernel\\.[A-Za-z]+V\\d+$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^allternit\\.kernel\\.[A-Za-z]+V\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ReceiptChainV1SchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ReceiptChainV1SchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ReceiptChainV1SchemaId {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ReceiptSignature`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ReceiptSignature {
    pub alg: ::serde_json::Value,
    pub domain: ::serde_json::Value,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub jwks_url: ::std::option::Option<::std::string::String>,
    pub key_id: ::std::string::String,
    pub value: ::std::string::String,
}
#[doc = "`RecoveryDecisionV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct RecoveryDecisionV1 {
    pub action: RecoveryDecisionV1Action,
    pub attempt: u64,
    pub changed: ::std::vec::Vec<RecoveryDecisionV1ChangedItem>,
    pub envelope: RecoveryDecisionV1Envelope,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub failure_class_ref: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub hard_limit_reached: ::std::option::Option<bool>,
}
#[doc = "`RecoveryDecisionV1Action`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum RecoveryDecisionV1Action {
    #[serde(rename = "RETRY")]
    Retry,
    #[serde(rename = "WAIT")]
    Wait,
    #[serde(rename = "CHANGE_ARGUMENT")]
    ChangeArgument,
    #[serde(rename = "CHANGE_TOOL")]
    ChangeTool,
    #[serde(rename = "ESCALATE")]
    Escalate,
    #[serde(rename = "FAIL")]
    Fail,
}
impl ::std::fmt::Display for RecoveryDecisionV1Action {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Retry => f.write_str("RETRY"),
            Self::Wait => f.write_str("WAIT"),
            Self::ChangeArgument => f.write_str("CHANGE_ARGUMENT"),
            Self::ChangeTool => f.write_str("CHANGE_TOOL"),
            Self::Escalate => f.write_str("ESCALATE"),
            Self::Fail => f.write_str("FAIL"),
        }
    }
}
impl ::std::str::FromStr for RecoveryDecisionV1Action {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "RETRY" => Ok(Self::Retry),
            "WAIT" => Ok(Self::Wait),
            "CHANGE_ARGUMENT" => Ok(Self::ChangeArgument),
            "CHANGE_TOOL" => Ok(Self::ChangeTool),
            "ESCALATE" => Ok(Self::Escalate),
            "FAIL" => Ok(Self::Fail),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for RecoveryDecisionV1Action {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for RecoveryDecisionV1Action {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`RecoveryDecisionV1ChangedItem`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum RecoveryDecisionV1ChangedItem {
    #[serde(rename = "EVIDENCE")]
    Evidence,
    #[serde(rename = "STRATEGY")]
    Strategy,
    #[serde(rename = "CONTEXT")]
    Context,
    #[serde(rename = "MODEL")]
    Model,
}
impl ::std::fmt::Display for RecoveryDecisionV1ChangedItem {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Evidence => f.write_str("EVIDENCE"),
            Self::Strategy => f.write_str("STRATEGY"),
            Self::Context => f.write_str("CONTEXT"),
            Self::Model => f.write_str("MODEL"),
        }
    }
}
impl ::std::str::FromStr for RecoveryDecisionV1ChangedItem {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "EVIDENCE" => Ok(Self::Evidence),
            "STRATEGY" => Ok(Self::Strategy),
            "CONTEXT" => Ok(Self::Context),
            "MODEL" => Ok(Self::Model),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for RecoveryDecisionV1ChangedItem {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for RecoveryDecisionV1ChangedItem {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`RecoveryDecisionV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct RecoveryDecisionV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: RecoveryDecisionV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`RecoveryDecisionV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum RecoveryDecisionV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.RecoveryDecisionV1")]
    AllternitKernelRecoveryDecisionV1,
}
impl ::std::fmt::Display for RecoveryDecisionV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelRecoveryDecisionV1 => {
                f.write_str("allternit.kernel.RecoveryDecisionV1")
            }
        }
    }
}
impl ::std::str::FromStr for RecoveryDecisionV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.RecoveryDecisionV1" => Ok(Self::AllternitKernelRecoveryDecisionV1),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for RecoveryDecisionV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for RecoveryDecisionV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ResourceBudget`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct ResourceBudget {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub max_attempts: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub max_cost_units: ::std::option::Option<f64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub max_spawns: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub max_tokens: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub max_tool_calls: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub max_wall_ms: ::std::option::Option<u64>,
}
#[doc = "`ResourceRef`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ResourceRef(::std::string::String);
impl ::std::ops::Deref for ResourceRef {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ResourceRef> for ::std::string::String {
    fn from(value: ResourceRef) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ResourceRef {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ResourceRef {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ResourceRef {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ResourceRef {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`RetryPolicy`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub backoff_ms: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub max_attempts: u64,
    pub must_change: ::std::vec::Vec<RetryPolicyMustChangeItem>,
}
#[doc = "`RetryPolicyMustChangeItem`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum RetryPolicyMustChangeItem {
    #[serde(rename = "EVIDENCE")]
    Evidence,
    #[serde(rename = "STRATEGY")]
    Strategy,
    #[serde(rename = "CONTEXT")]
    Context,
    #[serde(rename = "MODEL")]
    Model,
}
impl ::std::fmt::Display for RetryPolicyMustChangeItem {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Evidence => f.write_str("EVIDENCE"),
            Self::Strategy => f.write_str("STRATEGY"),
            Self::Context => f.write_str("CONTEXT"),
            Self::Model => f.write_str("MODEL"),
        }
    }
}
impl ::std::str::FromStr for RetryPolicyMustChangeItem {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "EVIDENCE" => Ok(Self::Evidence),
            "STRATEGY" => Ok(Self::Strategy),
            "CONTEXT" => Ok(Self::Context),
            "MODEL" => Ok(Self::Model),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for RetryPolicyMustChangeItem {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for RetryPolicyMustChangeItem {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`RunReceiptV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct RunReceiptV1 {
    pub acceptance_results: ::std::vec::Vec<RunReceiptV1AcceptanceResultsItem>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub action_receipts: ::std::vec::Vec<Id>,
    pub chain: ReceiptChainV1,
    #[serde(deserialize_with = "::std::option::Option::deserialize")]
    pub completion_decision_id: ::std::option::Option<Id>,
    pub completion_status: RunReceiptV1CompletionStatus,
    pub envelope: RunReceiptV1Envelope,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub final_state_version: StateVersion,
    pub final_work_state_version: StateVersion,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    pub implementation_resolution: ::std::vec::Vec<RunReceiptV1ImplementationResolutionItem>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub models_used: ::std::vec::Vec<ComponentIdentity>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub mutation_receipts: ::std::vec::Vec<Id>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub policy_receipts: ::std::vec::Vec<Id>,
    pub replay_fingerprint: ContentHash,
    pub retries: u64,
    pub router_telemetry: RunReceiptV1RouterTelemetry,
    pub task_id: Id,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub tool_receipts: ::std::vec::Vec<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub total_cost_units: ::std::option::Option<f64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub total_latency_ms: ::std::option::Option<f64>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub unresolved_items: ::std::vec::Vec<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub user_result_ref: ::std::option::Option<Id>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub verification_receipts: ::std::vec::Vec<Id>,
}
#[doc = "`RunReceiptV1AcceptanceResultsItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct RunReceiptV1AcceptanceResultsItem {
    pub criterion_id: Id,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub evidence_refs: ::std::vec::Vec<EvidenceRef>,
    pub satisfied: bool,
}
#[doc = "`RunReceiptV1CompletionStatus`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum RunReceiptV1CompletionStatus {
    #[serde(rename = "COMPLETE")]
    Complete,
    #[serde(rename = "PARTIAL")]
    Partial,
    #[serde(rename = "FAILED")]
    Failed,
    #[serde(rename = "CANCELLED")]
    Cancelled,
    #[serde(rename = "WAITING_APPROVAL")]
    WaitingApproval,
}
impl ::std::fmt::Display for RunReceiptV1CompletionStatus {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Complete => f.write_str("COMPLETE"),
            Self::Partial => f.write_str("PARTIAL"),
            Self::Failed => f.write_str("FAILED"),
            Self::Cancelled => f.write_str("CANCELLED"),
            Self::WaitingApproval => f.write_str("WAITING_APPROVAL"),
        }
    }
}
impl ::std::str::FromStr for RunReceiptV1CompletionStatus {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "COMPLETE" => Ok(Self::Complete),
            "PARTIAL" => Ok(Self::Partial),
            "FAILED" => Ok(Self::Failed),
            "CANCELLED" => Ok(Self::Cancelled),
            "WAITING_APPROVAL" => Ok(Self::WaitingApproval),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for RunReceiptV1CompletionStatus {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for RunReceiptV1CompletionStatus {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`RunReceiptV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct RunReceiptV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: RunReceiptV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`RunReceiptV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum RunReceiptV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.RunReceiptV1")]
    AllternitKernelRunReceiptV1,
}
impl ::std::fmt::Display for RunReceiptV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelRunReceiptV1 => f.write_str("allternit.kernel.RunReceiptV1"),
        }
    }
}
impl ::std::str::FromStr for RunReceiptV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.RunReceiptV1" => Ok(Self::AllternitKernelRunReceiptV1),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for RunReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for RunReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`RunReceiptV1ImplementationResolutionItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct RunReceiptV1ImplementationResolutionItem {
    pub backend_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub calibration_level:
        ::std::option::Option<RunReceiptV1ImplementationResolutionItemCalibrationLevel>,
    pub execution_mode: RunReceiptV1ImplementationResolutionItemExecutionMode,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub layer_stop: ::std::option::Option<u64>,
    pub node_id: Id,
}
#[doc = "`RunReceiptV1ImplementationResolutionItemCalibrationLevel`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum RunReceiptV1ImplementationResolutionItemCalibrationLevel {
    #[serde(rename = "RAW")]
    Raw,
    L0,
    L1,
    L2,
}
impl ::std::fmt::Display for RunReceiptV1ImplementationResolutionItemCalibrationLevel {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Raw => f.write_str("RAW"),
            Self::L0 => f.write_str("L0"),
            Self::L1 => f.write_str("L1"),
            Self::L2 => f.write_str("L2"),
        }
    }
}
impl ::std::str::FromStr for RunReceiptV1ImplementationResolutionItemCalibrationLevel {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "RAW" => Ok(Self::Raw),
            "L0" => Ok(Self::L0),
            "L1" => Ok(Self::L1),
            "L2" => Ok(Self::L2),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for RunReceiptV1ImplementationResolutionItemCalibrationLevel {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String>
    for RunReceiptV1ImplementationResolutionItemCalibrationLevel
{
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`RunReceiptV1ImplementationResolutionItemExecutionMode`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum RunReceiptV1ImplementationResolutionItemExecutionMode {
    #[serde(rename = "M0.DETERMINISTIC")]
    M0Deterministic,
    #[serde(rename = "M1.LOGIT_READOUT")]
    M1LogitReadout,
    #[serde(rename = "M2.CALIBRATED_READOUT")]
    M2CalibratedReadout,
    #[serde(rename = "M3.HIDDEN_HEAD")]
    M3HiddenHead,
    #[serde(rename = "M4.DEDICATED_DECIDER")]
    M4DedicatedDecider,
    #[serde(rename = "M5.GENERATIVE")]
    M5Generative,
    #[serde(rename = "M6.DEEP_SOLVER")]
    M6DeepSolver,
}
impl ::std::fmt::Display for RunReceiptV1ImplementationResolutionItemExecutionMode {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::M0Deterministic => f.write_str("M0.DETERMINISTIC"),
            Self::M1LogitReadout => f.write_str("M1.LOGIT_READOUT"),
            Self::M2CalibratedReadout => f.write_str("M2.CALIBRATED_READOUT"),
            Self::M3HiddenHead => f.write_str("M3.HIDDEN_HEAD"),
            Self::M4DedicatedDecider => f.write_str("M4.DEDICATED_DECIDER"),
            Self::M5Generative => f.write_str("M5.GENERATIVE"),
            Self::M6DeepSolver => f.write_str("M6.DEEP_SOLVER"),
        }
    }
}
impl ::std::str::FromStr for RunReceiptV1ImplementationResolutionItemExecutionMode {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "M0.DETERMINISTIC" => Ok(Self::M0Deterministic),
            "M1.LOGIT_READOUT" => Ok(Self::M1LogitReadout),
            "M2.CALIBRATED_READOUT" => Ok(Self::M2CalibratedReadout),
            "M3.HIDDEN_HEAD" => Ok(Self::M3HiddenHead),
            "M4.DEDICATED_DECIDER" => Ok(Self::M4DedicatedDecider),
            "M5.GENERATIVE" => Ok(Self::M5Generative),
            "M6.DEEP_SOLVER" => Ok(Self::M6DeepSolver),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for RunReceiptV1ImplementationResolutionItemExecutionMode {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String>
    for RunReceiptV1ImplementationResolutionItemExecutionMode
{
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`RunReceiptV1RouterTelemetry`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct RunReceiptV1RouterTelemetry {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub context_build_tokens: ::std::option::Option<f64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub effective_route_cost: ::std::option::Option<f64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub prefill_tokens: ::std::option::Option<f64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub provider_trust_class: ::std::option::Option<TrustClass>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub reuse_mode: ::std::option::Option<TransferMode>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub transfer_latency_ms: ::std::option::Option<f64>,
}
#[doc = "`RunStatus`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum RunStatus {
    #[serde(rename = "accepted")]
    Accepted,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "needs_attention")]
    NeedsAttention,
    #[serde(rename = "paused")]
    Paused,
    #[serde(rename = "cancelling")]
    Cancelling,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "partial")]
    Partial,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
}
impl ::std::fmt::Display for RunStatus {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Accepted => f.write_str("accepted"),
            Self::Running => f.write_str("running"),
            Self::Waiting => f.write_str("waiting"),
            Self::NeedsAttention => f.write_str("needs_attention"),
            Self::Paused => f.write_str("paused"),
            Self::Cancelling => f.write_str("cancelling"),
            Self::Completed => f.write_str("completed"),
            Self::Partial => f.write_str("partial"),
            Self::Failed => f.write_str("failed"),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}
impl ::std::str::FromStr for RunStatus {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "accepted" => Ok(Self::Accepted),
            "running" => Ok(Self::Running),
            "waiting" => Ok(Self::Waiting),
            "needs_attention" => Ok(Self::NeedsAttention),
            "paused" => Ok(Self::Paused),
            "cancelling" => Ok(Self::Cancelling),
            "completed" => Ok(Self::Completed),
            "partial" => Ok(Self::Partial),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for RunStatus {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for RunStatus {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`SemVer`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct SemVer(::std::string::String);
impl ::std::ops::Deref for SemVer {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<SemVer> for ::std::string::String {
    fn from(value: SemVer) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for SemVer {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^\\d+\\.\\d+\\.\\d+(-[0-9A-Za-z.]+)?$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^\\d+\\.\\d+\\.\\d+(-[0-9A-Za-z.]+)?$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for SemVer {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for SemVer {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for SemVer {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`SensitivityClass`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum SensitivityClass {
    #[serde(rename = "PUBLIC")]
    Public,
    #[serde(rename = "INTERNAL")]
    Internal,
    #[serde(rename = "RESTRICTED")]
    Restricted,
    #[serde(rename = "SECRET")]
    Secret,
}
impl ::std::fmt::Display for SensitivityClass {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Public => f.write_str("PUBLIC"),
            Self::Internal => f.write_str("INTERNAL"),
            Self::Restricted => f.write_str("RESTRICTED"),
            Self::Secret => f.write_str("SECRET"),
        }
    }
}
impl ::std::str::FromStr for SensitivityClass {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "PUBLIC" => Ok(Self::Public),
            "INTERNAL" => Ok(Self::Internal),
            "RESTRICTED" => Ok(Self::Restricted),
            "SECRET" => Ok(Self::Secret),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for SensitivityClass {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for SensitivityClass {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`SpawnReceiptV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct SpawnReceiptV1 {
    pub admission_decision_id: Id,
    pub authority_profile: SpawnReceiptV1AuthorityProfile,
    pub chain: ReceiptChainV1,
    pub envelope: SpawnReceiptV1Envelope,
    pub environment_id: Id,
    pub executor_class: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub lease_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub network_policy_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub policy_gate_injected: ::std::option::Option<::serde_json::Value>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub process_ref: ::std::option::Option<::std::string::String>,
    pub spawn_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub subgoal_id: ::std::option::Option<Id>,
}
#[doc = "`SpawnReceiptV1AuthorityProfile`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum SpawnReceiptV1AuthorityProfile {
    #[serde(rename = "read-only")]
    ReadOnly,
    #[serde(rename = "code-safe")]
    CodeSafe,
    #[serde(rename = "code-write")]
    CodeWrite,
}
impl ::std::fmt::Display for SpawnReceiptV1AuthorityProfile {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::ReadOnly => f.write_str("read-only"),
            Self::CodeSafe => f.write_str("code-safe"),
            Self::CodeWrite => f.write_str("code-write"),
        }
    }
}
impl ::std::str::FromStr for SpawnReceiptV1AuthorityProfile {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "read-only" => Ok(Self::ReadOnly),
            "code-safe" => Ok(Self::CodeSafe),
            "code-write" => Ok(Self::CodeWrite),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for SpawnReceiptV1AuthorityProfile {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for SpawnReceiptV1AuthorityProfile {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`SpawnReceiptV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct SpawnReceiptV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: SpawnReceiptV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`SpawnReceiptV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum SpawnReceiptV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.SpawnReceiptV1")]
    AllternitKernelSpawnReceiptV1,
}
impl ::std::fmt::Display for SpawnReceiptV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelSpawnReceiptV1 => f.write_str("allternit.kernel.SpawnReceiptV1"),
        }
    }
}
impl ::std::str::FromStr for SpawnReceiptV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.SpawnReceiptV1" => Ok(Self::AllternitKernelSpawnReceiptV1),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for SpawnReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for SpawnReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`StateTransferReceiptV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct StateTransferReceiptV1 {
    pub envelope: StateTransferReceiptV1Envelope,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub fallback_reason: ::std::option::Option<::std::string::String>,
    pub fallback_used: bool,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub fidelity_estimate: ::std::option::Option<f64>,
    pub latency_ms: f64,
    pub mode: TransferMode,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub target_state_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub validation_ref: ::std::option::Option<Id>,
}
#[doc = "`StateTransferReceiptV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct StateTransferReceiptV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: StateTransferReceiptV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`StateTransferReceiptV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum StateTransferReceiptV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.StateTransferReceiptV1")]
    AllternitKernelStateTransferReceiptV1,
}
impl ::std::fmt::Display for StateTransferReceiptV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelStateTransferReceiptV1 => {
                f.write_str("allternit.kernel.StateTransferReceiptV1")
            }
        }
    }
}
impl ::std::str::FromStr for StateTransferReceiptV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.StateTransferReceiptV1" => {
                Ok(Self::AllternitKernelStateTransferReceiptV1)
            }
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for StateTransferReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for StateTransferReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`StateTransferRequestV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct StateTransferRequestV1 {
    pub envelope: StateTransferRequestV1Envelope,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub max_latency_ms: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub max_quality_loss: ::std::option::Option<f64>,
    pub required_context_fingerprint: ContentHash,
    pub source_state_id: Id,
    pub target_model_ref: Id,
}
#[doc = "`StateTransferRequestV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct StateTransferRequestV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: StateTransferRequestV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`StateTransferRequestV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum StateTransferRequestV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.StateTransferRequestV1")]
    AllternitKernelStateTransferRequestV1,
}
impl ::std::fmt::Display for StateTransferRequestV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelStateTransferRequestV1 => {
                f.write_str("allternit.kernel.StateTransferRequestV1")
            }
        }
    }
}
impl ::std::str::FromStr for StateTransferRequestV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.StateTransferRequestV1" => {
                Ok(Self::AllternitKernelStateTransferRequestV1)
            }
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for StateTransferRequestV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for StateTransferRequestV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`StateVersion`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(transparent)]
pub struct StateVersion(pub u64);
impl ::std::ops::Deref for StateVersion {
    type Target = u64;
    fn deref(&self) -> &u64 {
        &self.0
    }
}
impl ::std::convert::From<StateVersion> for u64 {
    fn from(value: StateVersion) -> Self {
        value.0
    }
}
impl ::std::convert::From<u64> for StateVersion {
    fn from(value: u64) -> Self {
        Self(value)
    }
}
impl ::std::fmt::Display for StateVersion {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        self.0.fmt(f)
    }
}
impl ::std::str::FromStr for StateVersion {
    type Err = <u64 as ::std::str::FromStr>::Err;
    fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
        Ok(Self(value.parse()?))
    }
}
impl ::std::convert::TryFrom<&str> for StateVersion {
    type Error = <u64 as ::std::str::FromStr>::Err;
    fn try_from(value: &str) -> ::std::result::Result<Self, Self::Error> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<String> for StateVersion {
    type Error = <u64 as ::std::str::FromStr>::Err;
    fn try_from(value: String) -> ::std::result::Result<Self, Self::Error> {
        value.parse()
    }
}
#[doc = "`StateVersionRef`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct StateVersionRef {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub run_id: Id,
    pub state_version: StateVersion,
    pub store: StateVersionRefStore,
}
#[doc = "`StateVersionRefStore`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum StateVersionRefStore {
    #[serde(rename = "AGENT_STATE")]
    AgentState,
    #[serde(rename = "WORK_LEDGER")]
    WorkLedger,
}
impl ::std::fmt::Display for StateVersionRefStore {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AgentState => f.write_str("AGENT_STATE"),
            Self::WorkLedger => f.write_str("WORK_LEDGER"),
        }
    }
}
impl ::std::str::FromStr for StateVersionRefStore {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "AGENT_STATE" => Ok(Self::AgentState),
            "WORK_LEDGER" => Ok(Self::WorkLedger),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for StateVersionRefStore {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for StateVersionRefStore {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`TaskIrv1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct TaskIrv1 {
    pub acceptance_criteria: ::std::vec::Vec<TaskIrv1AcceptanceCriteriaItem>,
    pub allowed_scope: ::std::vec::Vec<ResourceRef>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub ambiguities: ::std::vec::Vec<TaskIrv1AmbiguitiesItem>,
    pub completion_policy: CompletionPolicyRef,
    pub constraints: ::std::vec::Vec<TaskIrv1ConstraintsItem>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub cost_budget: ::std::option::Option<ResourceBudget>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub forbidden_scope: ::std::vec::Vec<ResourceRef>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub freshness_requirement: ::std::option::Option<::std::string::String>,
    pub interaction_mode: TaskIrv1InteractionMode,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub latency_budget_ms: ::std::option::Option<u64>,
    pub objective: TaskIrv1Objective,
    pub privacy_class: TrustClass,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub requested_outputs: ::std::vec::Vec<TaskIrv1RequestedOutputsItem>,
    pub risk_class: TaskIrv1RiskClass,
    pub schema_id: ::serde_json::Value,
    pub schema_version: TaskIrv1SchemaVersion,
    pub task_id: Id,
    pub task_type: TaskIrv1TaskType,
}
#[doc = "`TaskIrv1AcceptanceCriteriaItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct TaskIrv1AcceptanceCriteriaItem {
    pub criterion_id: Id,
    pub origin: TaskIrv1AcceptanceCriteriaItemOrigin,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub required: ::std::option::Option<bool>,
    pub text: ::std::string::String,
}
#[doc = "`TaskIrv1AcceptanceCriteriaItemOrigin`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum TaskIrv1AcceptanceCriteriaItemOrigin {
    #[serde(rename = "EXPLICIT")]
    Explicit,
    #[serde(rename = "INFERRED")]
    Inferred,
}
impl ::std::fmt::Display for TaskIrv1AcceptanceCriteriaItemOrigin {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Explicit => f.write_str("EXPLICIT"),
            Self::Inferred => f.write_str("INFERRED"),
        }
    }
}
impl ::std::str::FromStr for TaskIrv1AcceptanceCriteriaItemOrigin {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "EXPLICIT" => Ok(Self::Explicit),
            "INFERRED" => Ok(Self::Inferred),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for TaskIrv1AcceptanceCriteriaItemOrigin {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TaskIrv1AcceptanceCriteriaItemOrigin {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`TaskIrv1AmbiguitiesItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct TaskIrv1AmbiguitiesItem {
    pub ambiguity_id: Id,
    pub resolution: TaskIrv1AmbiguitiesItemResolution,
    pub text: ::std::string::String,
}
#[doc = "`TaskIrv1AmbiguitiesItemResolution`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum TaskIrv1AmbiguitiesItemResolution {
    #[serde(rename = "UNRESOLVED")]
    Unresolved,
    #[serde(rename = "DEFAULTED")]
    Defaulted,
    #[serde(rename = "CLARIFIED")]
    Clarified,
    #[serde(rename = "ESCALATED")]
    Escalated,
}
impl ::std::fmt::Display for TaskIrv1AmbiguitiesItemResolution {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Unresolved => f.write_str("UNRESOLVED"),
            Self::Defaulted => f.write_str("DEFAULTED"),
            Self::Clarified => f.write_str("CLARIFIED"),
            Self::Escalated => f.write_str("ESCALATED"),
        }
    }
}
impl ::std::str::FromStr for TaskIrv1AmbiguitiesItemResolution {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "UNRESOLVED" => Ok(Self::Unresolved),
            "DEFAULTED" => Ok(Self::Defaulted),
            "CLARIFIED" => Ok(Self::Clarified),
            "ESCALATED" => Ok(Self::Escalated),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for TaskIrv1AmbiguitiesItemResolution {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TaskIrv1AmbiguitiesItemResolution {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`TaskIrv1ConstraintsItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct TaskIrv1ConstraintsItem {
    pub constraint_id: Id,
    pub hardness: TaskIrv1ConstraintsItemHardness,
    pub text: ::std::string::String,
}
#[doc = "`TaskIrv1ConstraintsItemHardness`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum TaskIrv1ConstraintsItemHardness {
    #[serde(rename = "HARD")]
    Hard,
    #[serde(rename = "SOFT")]
    Soft,
}
impl ::std::fmt::Display for TaskIrv1ConstraintsItemHardness {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Hard => f.write_str("HARD"),
            Self::Soft => f.write_str("SOFT"),
        }
    }
}
impl ::std::str::FromStr for TaskIrv1ConstraintsItemHardness {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "HARD" => Ok(Self::Hard),
            "SOFT" => Ok(Self::Soft),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for TaskIrv1ConstraintsItemHardness {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TaskIrv1ConstraintsItemHardness {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`TaskIrv1InteractionMode`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum TaskIrv1InteractionMode {
    #[serde(rename = "AUTONOMOUS")]
    Autonomous,
    #[serde(rename = "APPROVAL_GATED")]
    ApprovalGated,
    #[serde(rename = "RECOMMEND_ONLY")]
    RecommendOnly,
}
impl ::std::fmt::Display for TaskIrv1InteractionMode {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Autonomous => f.write_str("AUTONOMOUS"),
            Self::ApprovalGated => f.write_str("APPROVAL_GATED"),
            Self::RecommendOnly => f.write_str("RECOMMEND_ONLY"),
        }
    }
}
impl ::std::str::FromStr for TaskIrv1InteractionMode {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "AUTONOMOUS" => Ok(Self::Autonomous),
            "APPROVAL_GATED" => Ok(Self::ApprovalGated),
            "RECOMMEND_ONLY" => Ok(Self::RecommendOnly),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for TaskIrv1InteractionMode {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TaskIrv1InteractionMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`TaskIrv1Objective`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct TaskIrv1Objective(::std::string::String);
impl ::std::ops::Deref for TaskIrv1Objective {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<TaskIrv1Objective> for ::std::string::String {
    fn from(value: TaskIrv1Objective) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for TaskIrv1Objective {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for TaskIrv1Objective {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TaskIrv1Objective {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for TaskIrv1Objective {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`TaskIrv1RequestedOutputsItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct TaskIrv1RequestedOutputsItem {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub description: ::std::option::Option<::std::string::String>,
    pub kind: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub required: ::std::option::Option<bool>,
}
#[doc = "`TaskIrv1RiskClass`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum TaskIrv1RiskClass {
    #[serde(rename = "LOW")]
    Low,
    #[serde(rename = "MEDIUM")]
    Medium,
    #[serde(rename = "HIGH")]
    High,
    #[serde(rename = "CRITICAL")]
    Critical,
    #[serde(rename = "UNKNOWN")]
    Unknown,
}
impl ::std::fmt::Display for TaskIrv1RiskClass {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Low => f.write_str("LOW"),
            Self::Medium => f.write_str("MEDIUM"),
            Self::High => f.write_str("HIGH"),
            Self::Critical => f.write_str("CRITICAL"),
            Self::Unknown => f.write_str("UNKNOWN"),
        }
    }
}
impl ::std::str::FromStr for TaskIrv1RiskClass {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "LOW" => Ok(Self::Low),
            "MEDIUM" => Ok(Self::Medium),
            "HIGH" => Ok(Self::High),
            "CRITICAL" => Ok(Self::Critical),
            "UNKNOWN" => Ok(Self::Unknown),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for TaskIrv1RiskClass {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TaskIrv1RiskClass {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`TaskIrv1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct TaskIrv1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for TaskIrv1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<TaskIrv1SchemaVersion> for ::std::string::String {
    fn from(value: TaskIrv1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for TaskIrv1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for TaskIrv1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TaskIrv1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for TaskIrv1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`TaskIrv1TaskType`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct TaskIrv1TaskType(::std::string::String);
impl ::std::ops::Deref for TaskIrv1TaskType {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<TaskIrv1TaskType> for ::std::string::String {
    fn from(value: TaskIrv1TaskType) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for TaskIrv1TaskType {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^[A-Z][A-Z0-9_]*$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[A-Z][A-Z0-9_]*$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for TaskIrv1TaskType {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TaskIrv1TaskType {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for TaskIrv1TaskType {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`Timestamp`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
#[serde(transparent)]
pub struct Timestamp(pub ::std::string::String);
impl ::std::ops::Deref for Timestamp {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<Timestamp> for ::std::string::String {
    fn from(value: Timestamp) -> Self {
        value.0
    }
}
impl ::std::convert::From<::std::string::String> for Timestamp {
    fn from(value: ::std::string::String) -> Self {
        Self(value)
    }
}
impl ::std::fmt::Display for Timestamp {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        self.0.fmt(f)
    }
}
impl ::std::str::FromStr for Timestamp {
    type Err = ::std::convert::Infallible;
    fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
        Ok(Self(value.to_string()))
    }
}
#[doc = "`ToolInvocationV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ToolInvocationV1 {
    pub argument_provenance:
        ::std::collections::HashMap<::std::string::String, ToolInvocationV1ArgumentProvenanceValue>,
    pub arguments: ::serde_json::Map<::std::string::String, ::serde_json::Value>,
    pub effect_class: ToolInvocationV1EffectClass,
    pub envelope: ToolInvocationV1Envelope,
    pub environment_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub idempotency_key: ::std::option::Option<ToolInvocationV1IdempotencyKey>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub invocation_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub network_policy_id: ::std::option::Option<Id>,
    pub operation_id: ::std::string::String,
    pub policy_decision_id: Id,
    pub read_set: ::std::vec::Vec<ResourceRef>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub sandbox_ref: ::std::option::Option<Id>,
    pub timeout_ms: ::std::num::NonZeroU64,
    pub tool_id: Id,
    pub write_set: ::std::vec::Vec<ResourceRef>,
}
#[doc = "`ToolInvocationV1ArgumentProvenanceValue`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ToolInvocationV1ArgumentProvenanceValue {
    #[serde(rename = "STATE")]
    State,
    #[serde(rename = "RETRIEVAL")]
    Retrieval,
    #[serde(rename = "POLICY")]
    Policy,
    #[serde(rename = "GENERATED")]
    Generated,
    #[serde(rename = "USER")]
    User,
}
impl ::std::fmt::Display for ToolInvocationV1ArgumentProvenanceValue {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::State => f.write_str("STATE"),
            Self::Retrieval => f.write_str("RETRIEVAL"),
            Self::Policy => f.write_str("POLICY"),
            Self::Generated => f.write_str("GENERATED"),
            Self::User => f.write_str("USER"),
        }
    }
}
impl ::std::str::FromStr for ToolInvocationV1ArgumentProvenanceValue {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "STATE" => Ok(Self::State),
            "RETRIEVAL" => Ok(Self::Retrieval),
            "POLICY" => Ok(Self::Policy),
            "GENERATED" => Ok(Self::Generated),
            "USER" => Ok(Self::User),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ToolInvocationV1ArgumentProvenanceValue {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ToolInvocationV1ArgumentProvenanceValue {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ToolInvocationV1EffectClass`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ToolInvocationV1EffectClass {
    #[serde(rename = "NONE")]
    None,
    #[serde(rename = "READ")]
    Read,
    #[serde(rename = "WORKSPACE_WRITE")]
    WorkspaceWrite,
    #[serde(rename = "EXECUTE")]
    Execute,
    #[serde(rename = "NETWORK")]
    Network,
    #[serde(rename = "EXTERNAL_WRITE")]
    ExternalWrite,
    #[serde(rename = "FINANCIAL")]
    Financial,
    #[serde(rename = "PUBLISH")]
    Publish,
    #[serde(rename = "PERMISSION_CHANGE")]
    PermissionChange,
}
impl ::std::fmt::Display for ToolInvocationV1EffectClass {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::None => f.write_str("NONE"),
            Self::Read => f.write_str("READ"),
            Self::WorkspaceWrite => f.write_str("WORKSPACE_WRITE"),
            Self::Execute => f.write_str("EXECUTE"),
            Self::Network => f.write_str("NETWORK"),
            Self::ExternalWrite => f.write_str("EXTERNAL_WRITE"),
            Self::Financial => f.write_str("FINANCIAL"),
            Self::Publish => f.write_str("PUBLISH"),
            Self::PermissionChange => f.write_str("PERMISSION_CHANGE"),
        }
    }
}
impl ::std::str::FromStr for ToolInvocationV1EffectClass {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "NONE" => Ok(Self::None),
            "READ" => Ok(Self::Read),
            "WORKSPACE_WRITE" => Ok(Self::WorkspaceWrite),
            "EXECUTE" => Ok(Self::Execute),
            "NETWORK" => Ok(Self::Network),
            "EXTERNAL_WRITE" => Ok(Self::ExternalWrite),
            "FINANCIAL" => Ok(Self::Financial),
            "PUBLISH" => Ok(Self::Publish),
            "PERMISSION_CHANGE" => Ok(Self::PermissionChange),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ToolInvocationV1EffectClass {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ToolInvocationV1EffectClass {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ToolInvocationV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ToolInvocationV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: ToolInvocationV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`ToolInvocationV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ToolInvocationV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.ToolInvocationV1")]
    AllternitKernelToolInvocationV1,
}
impl ::std::fmt::Display for ToolInvocationV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelToolInvocationV1 => {
                f.write_str("allternit.kernel.ToolInvocationV1")
            }
        }
    }
}
impl ::std::str::FromStr for ToolInvocationV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.ToolInvocationV1" => Ok(Self::AllternitKernelToolInvocationV1),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ToolInvocationV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ToolInvocationV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ToolInvocationV1IdempotencyKey`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct ToolInvocationV1IdempotencyKey(::std::string::String);
impl ::std::ops::Deref for ToolInvocationV1IdempotencyKey {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<ToolInvocationV1IdempotencyKey> for ::std::string::String {
    fn from(value: ToolInvocationV1IdempotencyKey) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for ToolInvocationV1IdempotencyKey {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 8usize {
            return Err("shorter than 8 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for ToolInvocationV1IdempotencyKey {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ToolInvocationV1IdempotencyKey {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for ToolInvocationV1IdempotencyKey {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`ToolReceiptV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ToolReceiptV1 {
    pub content_hashes: ::std::vec::Vec<ContentHash>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub diagnostics: ::std::vec::Vec<::serde_json::Map<::std::string::String, ::serde_json::Value>>,
    pub envelope: ToolReceiptV1Envelope,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub error: ::std::option::Option<ErrorV1>,
    pub exit_class: ToolReceiptV1ExitClass,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub exit_code: ::std::option::Option<i64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub finished_at: Timestamp,
    pub invocation_id: Id,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub observed_reads: ::std::vec::Vec<ResourceRef>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub observed_writes: ::std::vec::Vec<ResourceRef>,
    pub operation_id: ::std::string::String,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub outputs: ::std::vec::Vec<ArtifactRef>,
    pub policy_receipt_id: Id,
    pub started_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub stderr_ref: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub stdout_ref: ::std::option::Option<Id>,
    pub tool_id: Id,
}
#[doc = "`ToolReceiptV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ToolReceiptV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: ToolReceiptV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`ToolReceiptV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ToolReceiptV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.ToolReceiptV1")]
    AllternitKernelToolReceiptV1,
}
impl ::std::fmt::Display for ToolReceiptV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelToolReceiptV1 => f.write_str("allternit.kernel.ToolReceiptV1"),
        }
    }
}
impl ::std::str::FromStr for ToolReceiptV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.ToolReceiptV1" => Ok(Self::AllternitKernelToolReceiptV1),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ToolReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ToolReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`ToolReceiptV1ExitClass`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum ToolReceiptV1ExitClass {
    #[serde(rename = "SUCCESS")]
    Success,
    #[serde(rename = "FAILURE")]
    Failure,
    #[serde(rename = "PARTIAL")]
    Partial,
    #[serde(rename = "TIMEOUT")]
    Timeout,
    #[serde(rename = "DENIED")]
    Denied,
    #[serde(rename = "CANCELLED")]
    Cancelled,
}
impl ::std::fmt::Display for ToolReceiptV1ExitClass {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Success => f.write_str("SUCCESS"),
            Self::Failure => f.write_str("FAILURE"),
            Self::Partial => f.write_str("PARTIAL"),
            Self::Timeout => f.write_str("TIMEOUT"),
            Self::Denied => f.write_str("DENIED"),
            Self::Cancelled => f.write_str("CANCELLED"),
        }
    }
}
impl ::std::str::FromStr for ToolReceiptV1ExitClass {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "SUCCESS" => Ok(Self::Success),
            "FAILURE" => Ok(Self::Failure),
            "PARTIAL" => Ok(Self::Partial),
            "TIMEOUT" => Ok(Self::Timeout),
            "DENIED" => Ok(Self::Denied),
            "CANCELLED" => Ok(Self::Cancelled),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ToolReceiptV1ExitClass {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ToolReceiptV1ExitClass {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`TraceEventV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct TraceEventV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub backend_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub cache_hit: ::std::option::Option<bool>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub calibration_level: ::std::option::Option<TraceEventV1CalibrationLevel>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub capability_id: ::std::option::Option<CapabilityId>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub cognitive_role: ::std::option::Option<TraceEventV1CognitiveRole>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub confidence: ::std::option::Option<f64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub cost_units: ::std::option::Option<f64>,
    pub envelope: TraceEventV1Envelope,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub error: ::std::option::Option<ErrorV1>,
    pub event_type: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub execution_mode: ::std::option::Option<TraceEventV1ExecutionMode>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub input_tokens: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub kv_transfer_mode: ::std::option::Option<TransferMode>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub latency_ms: ::std::option::Option<f64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub layer_stop: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub memory_bytes: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub model_ref: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub output_tokens: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub primitive_id: ::std::option::Option<PrimitiveId>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub receipt_refs: ::std::vec::Vec<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub state_version_after: ::std::option::Option<StateVersion>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub state_version_before: ::std::option::Option<StateVersion>,
    pub status: TraceEventV1Status,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub tool_id: ::std::option::Option<Id>,
}
#[doc = "`TraceEventV1CalibrationLevel`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum TraceEventV1CalibrationLevel {
    #[serde(rename = "RAW")]
    Raw,
    L0,
    L1,
    L2,
}
impl ::std::fmt::Display for TraceEventV1CalibrationLevel {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Raw => f.write_str("RAW"),
            Self::L0 => f.write_str("L0"),
            Self::L1 => f.write_str("L1"),
            Self::L2 => f.write_str("L2"),
        }
    }
}
impl ::std::str::FromStr for TraceEventV1CalibrationLevel {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "RAW" => Ok(Self::Raw),
            "L0" => Ok(Self::L0),
            "L1" => Ok(Self::L1),
            "L2" => Ok(Self::L2),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for TraceEventV1CalibrationLevel {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TraceEventV1CalibrationLevel {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`TraceEventV1CognitiveRole`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum TraceEventV1CognitiveRole {
    S0,
    S1,
    S2,
    S3,
}
impl ::std::fmt::Display for TraceEventV1CognitiveRole {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::S0 => f.write_str("S0"),
            Self::S1 => f.write_str("S1"),
            Self::S2 => f.write_str("S2"),
            Self::S3 => f.write_str("S3"),
        }
    }
}
impl ::std::str::FromStr for TraceEventV1CognitiveRole {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "S0" => Ok(Self::S0),
            "S1" => Ok(Self::S1),
            "S2" => Ok(Self::S2),
            "S3" => Ok(Self::S3),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for TraceEventV1CognitiveRole {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TraceEventV1CognitiveRole {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`TraceEventV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct TraceEventV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: TraceEventV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`TraceEventV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum TraceEventV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.TraceEventV1")]
    AllternitKernelTraceEventV1,
}
impl ::std::fmt::Display for TraceEventV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelTraceEventV1 => f.write_str("allternit.kernel.TraceEventV1"),
        }
    }
}
impl ::std::str::FromStr for TraceEventV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.TraceEventV1" => Ok(Self::AllternitKernelTraceEventV1),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for TraceEventV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TraceEventV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`TraceEventV1ExecutionMode`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum TraceEventV1ExecutionMode {
    #[serde(rename = "M0.DETERMINISTIC")]
    M0Deterministic,
    #[serde(rename = "M1.LOGIT_READOUT")]
    M1LogitReadout,
    #[serde(rename = "M2.CALIBRATED_READOUT")]
    M2CalibratedReadout,
    #[serde(rename = "M3.HIDDEN_HEAD")]
    M3HiddenHead,
    #[serde(rename = "M4.DEDICATED_DECIDER")]
    M4DedicatedDecider,
    #[serde(rename = "M5.GENERATIVE")]
    M5Generative,
    #[serde(rename = "M6.DEEP_SOLVER")]
    M6DeepSolver,
}
impl ::std::fmt::Display for TraceEventV1ExecutionMode {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::M0Deterministic => f.write_str("M0.DETERMINISTIC"),
            Self::M1LogitReadout => f.write_str("M1.LOGIT_READOUT"),
            Self::M2CalibratedReadout => f.write_str("M2.CALIBRATED_READOUT"),
            Self::M3HiddenHead => f.write_str("M3.HIDDEN_HEAD"),
            Self::M4DedicatedDecider => f.write_str("M4.DEDICATED_DECIDER"),
            Self::M5Generative => f.write_str("M5.GENERATIVE"),
            Self::M6DeepSolver => f.write_str("M6.DEEP_SOLVER"),
        }
    }
}
impl ::std::str::FromStr for TraceEventV1ExecutionMode {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "M0.DETERMINISTIC" => Ok(Self::M0Deterministic),
            "M1.LOGIT_READOUT" => Ok(Self::M1LogitReadout),
            "M2.CALIBRATED_READOUT" => Ok(Self::M2CalibratedReadout),
            "M3.HIDDEN_HEAD" => Ok(Self::M3HiddenHead),
            "M4.DEDICATED_DECIDER" => Ok(Self::M4DedicatedDecider),
            "M5.GENERATIVE" => Ok(Self::M5Generative),
            "M6.DEEP_SOLVER" => Ok(Self::M6DeepSolver),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for TraceEventV1ExecutionMode {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TraceEventV1ExecutionMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`TraceEventV1Status`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum TraceEventV1Status {
    #[serde(rename = "SUCCEEDED")]
    Succeeded,
    #[serde(rename = "FAILED")]
    Failed,
    #[serde(rename = "SKIPPED")]
    Skipped,
    #[serde(rename = "STARTED")]
    Started,
}
impl ::std::fmt::Display for TraceEventV1Status {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Succeeded => f.write_str("SUCCEEDED"),
            Self::Failed => f.write_str("FAILED"),
            Self::Skipped => f.write_str("SKIPPED"),
            Self::Started => f.write_str("STARTED"),
        }
    }
}
impl ::std::str::FromStr for TraceEventV1Status {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "SUCCEEDED" => Ok(Self::Succeeded),
            "FAILED" => Ok(Self::Failed),
            "SKIPPED" => Ok(Self::Skipped),
            "STARTED" => Ok(Self::Started),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for TraceEventV1Status {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TraceEventV1Status {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`TransferMode`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum TransferMode {
    #[serde(rename = "IDENTITY")]
    Identity,
    #[serde(rename = "PREFIX_REUSE")]
    PrefixReuse,
    #[serde(rename = "TRANSLATED_KV")]
    TranslatedKv,
    #[serde(rename = "SEMANTIC_REBUILD")]
    SemanticRebuild,
}
impl ::std::fmt::Display for TransferMode {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Identity => f.write_str("IDENTITY"),
            Self::PrefixReuse => f.write_str("PREFIX_REUSE"),
            Self::TranslatedKv => f.write_str("TRANSLATED_KV"),
            Self::SemanticRebuild => f.write_str("SEMANTIC_REBUILD"),
        }
    }
}
impl ::std::str::FromStr for TransferMode {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "IDENTITY" => Ok(Self::Identity),
            "PREFIX_REUSE" => Ok(Self::PrefixReuse),
            "TRANSLATED_KV" => Ok(Self::TranslatedKv),
            "SEMANTIC_REBUILD" => Ok(Self::SemanticRebuild),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for TransferMode {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TransferMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`TrustClass`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum TrustClass {
    #[serde(rename = "PUBLIC")]
    Public,
    #[serde(rename = "INTERNAL")]
    Internal,
    #[serde(rename = "RESTRICTED")]
    Restricted,
    #[serde(rename = "SECRET")]
    Secret,
    #[serde(rename = "UNTRUSTED")]
    Untrusted,
}
impl ::std::fmt::Display for TrustClass {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Public => f.write_str("PUBLIC"),
            Self::Internal => f.write_str("INTERNAL"),
            Self::Restricted => f.write_str("RESTRICTED"),
            Self::Secret => f.write_str("SECRET"),
            Self::Untrusted => f.write_str("UNTRUSTED"),
        }
    }
}
impl ::std::str::FromStr for TrustClass {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "PUBLIC" => Ok(Self::Public),
            "INTERNAL" => Ok(Self::Internal),
            "RESTRICTED" => Ok(Self::Restricted),
            "SECRET" => Ok(Self::Secret),
            "UNTRUSTED" => Ok(Self::Untrusted),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for TrustClass {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for TrustClass {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`UnifiedDiffV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct UnifiedDiffV1 {
    pub base_hashes: ::std::collections::HashMap<::std::string::String, ContentHash>,
    pub diff_hash: ContentHash,
    pub diff_ref: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub files: ::std::vec::Vec<::std::string::String>,
    pub format: ::serde_json::Value,
}
#[doc = "`VerificationReceiptV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct VerificationReceiptV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub confidence: ::std::option::Option<f64>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub criteria_results: ::std::vec::Vec<VerificationReceiptV1CriteriaResultsItem>,
    pub deterministic: bool,
    pub envelope: VerificationReceiptV1Envelope,
    pub evidence_refs: ::std::vec::Vec<EvidenceRef>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub findings: ::std::vec::Vec<::serde_json::Map<::std::string::String, ::serde_json::Value>>,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub generated_followups: ::std::vec::Vec<Id>,
    pub result: VerificationReceiptV1Result,
    pub retryable: bool,
    pub subject_ref: Id,
    pub verification_id: Id,
    pub verifier: VerificationReceiptV1Verifier,
}
#[doc = "`VerificationReceiptV1CriteriaResultsItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct VerificationReceiptV1CriteriaResultsItem {
    pub criterion_id: Id,
    pub result: VerificationReceiptV1CriteriaResultsItemResult,
}
#[doc = "`VerificationReceiptV1CriteriaResultsItemResult`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum VerificationReceiptV1CriteriaResultsItemResult {
    #[serde(rename = "PASS")]
    Pass,
    #[serde(rename = "FAIL")]
    Fail,
    #[serde(rename = "INCONCLUSIVE")]
    Inconclusive,
    #[serde(rename = "ERROR")]
    Error,
}
impl ::std::fmt::Display for VerificationReceiptV1CriteriaResultsItemResult {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Pass => f.write_str("PASS"),
            Self::Fail => f.write_str("FAIL"),
            Self::Inconclusive => f.write_str("INCONCLUSIVE"),
            Self::Error => f.write_str("ERROR"),
        }
    }
}
impl ::std::str::FromStr for VerificationReceiptV1CriteriaResultsItemResult {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "PASS" => Ok(Self::Pass),
            "FAIL" => Ok(Self::Fail),
            "INCONCLUSIVE" => Ok(Self::Inconclusive),
            "ERROR" => Ok(Self::Error),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for VerificationReceiptV1CriteriaResultsItemResult {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String>
    for VerificationReceiptV1CriteriaResultsItemResult
{
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`VerificationReceiptV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct VerificationReceiptV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: VerificationReceiptV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`VerificationReceiptV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum VerificationReceiptV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.VerificationReceiptV1")]
    AllternitKernelVerificationReceiptV1,
}
impl ::std::fmt::Display for VerificationReceiptV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelVerificationReceiptV1 => {
                f.write_str("allternit.kernel.VerificationReceiptV1")
            }
        }
    }
}
impl ::std::str::FromStr for VerificationReceiptV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.VerificationReceiptV1" => {
                Ok(Self::AllternitKernelVerificationReceiptV1)
            }
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for VerificationReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for VerificationReceiptV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`VerificationReceiptV1Result`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum VerificationReceiptV1Result {
    #[serde(rename = "PASS")]
    Pass,
    #[serde(rename = "FAIL")]
    Fail,
    #[serde(rename = "INCONCLUSIVE")]
    Inconclusive,
    #[serde(rename = "ERROR")]
    Error,
}
impl ::std::fmt::Display for VerificationReceiptV1Result {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Pass => f.write_str("PASS"),
            Self::Fail => f.write_str("FAIL"),
            Self::Inconclusive => f.write_str("INCONCLUSIVE"),
            Self::Error => f.write_str("ERROR"),
        }
    }
}
impl ::std::str::FromStr for VerificationReceiptV1Result {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "PASS" => Ok(Self::Pass),
            "FAIL" => Ok(Self::Fail),
            "INCONCLUSIVE" => Ok(Self::Inconclusive),
            "ERROR" => Ok(Self::Error),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for VerificationReceiptV1Result {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for VerificationReceiptV1Result {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`VerificationReceiptV1Verifier`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum VerificationReceiptV1Verifier {
    #[serde(rename = "PARSE")]
    Parse,
    #[serde(rename = "FORMAT")]
    Format,
    #[serde(rename = "LINT")]
    Lint,
    #[serde(rename = "TYPECHECK")]
    Typecheck,
    #[serde(rename = "UNIT_TEST")]
    UnitTest,
    #[serde(rename = "INTEGRATION_TEST")]
    IntegrationTest,
    #[serde(rename = "BUILD")]
    Build,
    #[serde(rename = "SEMANTIC_RULE")]
    SemanticRule,
    #[serde(rename = "REQUIREMENT")]
    Requirement,
    #[serde(rename = "SECURITY")]
    Security,
    #[serde(rename = "REGRESSION")]
    Regression,
    #[serde(rename = "DIFF_REVIEW")]
    DiffReview,
    #[serde(rename = "HUMAN")]
    Human,
}
impl ::std::fmt::Display for VerificationReceiptV1Verifier {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Parse => f.write_str("PARSE"),
            Self::Format => f.write_str("FORMAT"),
            Self::Lint => f.write_str("LINT"),
            Self::Typecheck => f.write_str("TYPECHECK"),
            Self::UnitTest => f.write_str("UNIT_TEST"),
            Self::IntegrationTest => f.write_str("INTEGRATION_TEST"),
            Self::Build => f.write_str("BUILD"),
            Self::SemanticRule => f.write_str("SEMANTIC_RULE"),
            Self::Requirement => f.write_str("REQUIREMENT"),
            Self::Security => f.write_str("SECURITY"),
            Self::Regression => f.write_str("REGRESSION"),
            Self::DiffReview => f.write_str("DIFF_REVIEW"),
            Self::Human => f.write_str("HUMAN"),
        }
    }
}
impl ::std::str::FromStr for VerificationReceiptV1Verifier {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "PARSE" => Ok(Self::Parse),
            "FORMAT" => Ok(Self::Format),
            "LINT" => Ok(Self::Lint),
            "TYPECHECK" => Ok(Self::Typecheck),
            "UNIT_TEST" => Ok(Self::UnitTest),
            "INTEGRATION_TEST" => Ok(Self::IntegrationTest),
            "BUILD" => Ok(Self::Build),
            "SEMANTIC_RULE" => Ok(Self::SemanticRule),
            "REQUIREMENT" => Ok(Self::Requirement),
            "SECURITY" => Ok(Self::Security),
            "REGRESSION" => Ok(Self::Regression),
            "DIFF_REVIEW" => Ok(Self::DiffReview),
            "HUMAN" => Ok(Self::Human),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for VerificationReceiptV1Verifier {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for VerificationReceiptV1Verifier {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`VerificationRequestV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct VerificationRequestV1 {
    pub criteria: ::std::vec::Vec<Id>,
    pub envelope: VerificationRequestV1Envelope,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub evidence_inputs: ::std::vec::Vec<EvidenceRef>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub required_confidence: ::std::option::Option<f64>,
    pub subject: VerificationRequestV1Subject,
    pub verifier: VerificationRequestV1Verifier,
}
#[doc = "`VerificationRequestV1Envelope`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct VerificationRequestV1Envelope {
    pub abi_version: AbiVersion,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub graph_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub node_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub parent_trace_id: ::std::option::Option<Id>,
    pub producer: ComponentIdentity,
    pub provenance: ::std::vec::Vec<Provenance>,
    pub run_id: Id,
    pub schema_id: VerificationRequestV1EnvelopeSchemaId,
    pub schema_version: SemVer,
    pub session_id: Id,
    pub state_version: StateVersion,
    pub task_id: Id,
    pub trace_id: Id,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub work_state_version: ::std::option::Option<StateVersion>,
}
#[doc = "`VerificationRequestV1EnvelopeSchemaId`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum VerificationRequestV1EnvelopeSchemaId {
    #[serde(rename = "allternit.kernel.VerificationRequestV1")]
    AllternitKernelVerificationRequestV1,
}
impl ::std::fmt::Display for VerificationRequestV1EnvelopeSchemaId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AllternitKernelVerificationRequestV1 => {
                f.write_str("allternit.kernel.VerificationRequestV1")
            }
        }
    }
}
impl ::std::str::FromStr for VerificationRequestV1EnvelopeSchemaId {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "allternit.kernel.VerificationRequestV1" => {
                Ok(Self::AllternitKernelVerificationRequestV1)
            }
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for VerificationRequestV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for VerificationRequestV1EnvelopeSchemaId {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`VerificationRequestV1Subject`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct VerificationRequestV1Subject {
    pub kind: VerificationRequestV1SubjectKind,
    #[serde(rename = "ref")]
    pub ref_: Id,
}
#[doc = "`VerificationRequestV1SubjectKind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum VerificationRequestV1SubjectKind {
    #[serde(rename = "ARTIFACT")]
    Artifact,
    #[serde(rename = "MUTATION_RECEIPT")]
    MutationReceipt,
    #[serde(rename = "CLAIM")]
    Claim,
    #[serde(rename = "COMPLETION_PROPOSAL")]
    CompletionProposal,
}
impl ::std::fmt::Display for VerificationRequestV1SubjectKind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Artifact => f.write_str("ARTIFACT"),
            Self::MutationReceipt => f.write_str("MUTATION_RECEIPT"),
            Self::Claim => f.write_str("CLAIM"),
            Self::CompletionProposal => f.write_str("COMPLETION_PROPOSAL"),
        }
    }
}
impl ::std::str::FromStr for VerificationRequestV1SubjectKind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "ARTIFACT" => Ok(Self::Artifact),
            "MUTATION_RECEIPT" => Ok(Self::MutationReceipt),
            "CLAIM" => Ok(Self::Claim),
            "COMPLETION_PROPOSAL" => Ok(Self::CompletionProposal),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for VerificationRequestV1SubjectKind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for VerificationRequestV1SubjectKind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`VerificationRequestV1Verifier`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum VerificationRequestV1Verifier {
    #[serde(rename = "PARSE")]
    Parse,
    #[serde(rename = "FORMAT")]
    Format,
    #[serde(rename = "LINT")]
    Lint,
    #[serde(rename = "TYPECHECK")]
    Typecheck,
    #[serde(rename = "UNIT_TEST")]
    UnitTest,
    #[serde(rename = "INTEGRATION_TEST")]
    IntegrationTest,
    #[serde(rename = "BUILD")]
    Build,
    #[serde(rename = "SEMANTIC_RULE")]
    SemanticRule,
    #[serde(rename = "REQUIREMENT")]
    Requirement,
    #[serde(rename = "SECURITY")]
    Security,
    #[serde(rename = "REGRESSION")]
    Regression,
    #[serde(rename = "DIFF_REVIEW")]
    DiffReview,
    #[serde(rename = "HUMAN")]
    Human,
}
impl ::std::fmt::Display for VerificationRequestV1Verifier {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Parse => f.write_str("PARSE"),
            Self::Format => f.write_str("FORMAT"),
            Self::Lint => f.write_str("LINT"),
            Self::Typecheck => f.write_str("TYPECHECK"),
            Self::UnitTest => f.write_str("UNIT_TEST"),
            Self::IntegrationTest => f.write_str("INTEGRATION_TEST"),
            Self::Build => f.write_str("BUILD"),
            Self::SemanticRule => f.write_str("SEMANTIC_RULE"),
            Self::Requirement => f.write_str("REQUIREMENT"),
            Self::Security => f.write_str("SECURITY"),
            Self::Regression => f.write_str("REGRESSION"),
            Self::DiffReview => f.write_str("DIFF_REVIEW"),
            Self::Human => f.write_str("HUMAN"),
        }
    }
}
impl ::std::str::FromStr for VerificationRequestV1Verifier {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "PARSE" => Ok(Self::Parse),
            "FORMAT" => Ok(Self::Format),
            "LINT" => Ok(Self::Lint),
            "TYPECHECK" => Ok(Self::Typecheck),
            "UNIT_TEST" => Ok(Self::UnitTest),
            "INTEGRATION_TEST" => Ok(Self::IntegrationTest),
            "BUILD" => Ok(Self::Build),
            "SEMANTIC_RULE" => Ok(Self::SemanticRule),
            "REQUIREMENT" => Ok(Self::Requirement),
            "SECURITY" => Ok(Self::Security),
            "REGRESSION" => Ok(Self::Regression),
            "DIFF_REVIEW" => Ok(Self::DiffReview),
            "HUMAN" => Ok(Self::Human),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for VerificationRequestV1Verifier {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for VerificationRequestV1Verifier {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`WaitGateV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct WaitGateV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub attention_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub expires_at: ::std::option::Option<Timestamp>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub gate_id: Id,
    pub kind: WaitGateV1Kind,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub not_before: ::std::option::Option<Timestamp>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub on_expiry: ::std::option::Option<WaitGateV1OnExpiry>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: WaitGateV1SchemaVersion,
    pub status: WaitGateV1Status,
    pub wake_key: WaitGateV1WakeKey,
}
#[doc = "`WaitGateV1Kind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum WaitGateV1Kind {
    #[serde(rename = "TIMER")]
    Timer,
    #[serde(rename = "EVENT")]
    Event,
    #[serde(rename = "DEPENDENCY")]
    Dependency,
    #[serde(rename = "MANUAL")]
    Manual,
    #[serde(rename = "ATTENTION")]
    Attention,
}
impl ::std::fmt::Display for WaitGateV1Kind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Timer => f.write_str("TIMER"),
            Self::Event => f.write_str("EVENT"),
            Self::Dependency => f.write_str("DEPENDENCY"),
            Self::Manual => f.write_str("MANUAL"),
            Self::Attention => f.write_str("ATTENTION"),
        }
    }
}
impl ::std::str::FromStr for WaitGateV1Kind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "TIMER" => Ok(Self::Timer),
            "EVENT" => Ok(Self::Event),
            "DEPENDENCY" => Ok(Self::Dependency),
            "MANUAL" => Ok(Self::Manual),
            "ATTENTION" => Ok(Self::Attention),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for WaitGateV1Kind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WaitGateV1Kind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`WaitGateV1OnExpiry`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum WaitGateV1OnExpiry {
    #[serde(rename = "FAIL_NODE")]
    FailNode,
    #[serde(rename = "CONTINUE")]
    Continue,
    #[serde(rename = "NEEDS_HUMAN")]
    NeedsHuman,
}
impl ::std::fmt::Display for WaitGateV1OnExpiry {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::FailNode => f.write_str("FAIL_NODE"),
            Self::Continue => f.write_str("CONTINUE"),
            Self::NeedsHuman => f.write_str("NEEDS_HUMAN"),
        }
    }
}
impl ::std::str::FromStr for WaitGateV1OnExpiry {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "FAIL_NODE" => Ok(Self::FailNode),
            "CONTINUE" => Ok(Self::Continue),
            "NEEDS_HUMAN" => Ok(Self::NeedsHuman),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for WaitGateV1OnExpiry {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WaitGateV1OnExpiry {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`WaitGateV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct WaitGateV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for WaitGateV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<WaitGateV1SchemaVersion> for ::std::string::String {
    fn from(value: WaitGateV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for WaitGateV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for WaitGateV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WaitGateV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for WaitGateV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`WaitGateV1Status`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum WaitGateV1Status {
    #[serde(rename = "ARMED")]
    Armed,
    #[serde(rename = "FIRED")]
    Fired,
    #[serde(rename = "EXPIRED")]
    Expired,
    #[serde(rename = "CANCELLED")]
    Cancelled,
}
impl ::std::fmt::Display for WaitGateV1Status {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Armed => f.write_str("ARMED"),
            Self::Fired => f.write_str("FIRED"),
            Self::Expired => f.write_str("EXPIRED"),
            Self::Cancelled => f.write_str("CANCELLED"),
        }
    }
}
impl ::std::str::FromStr for WaitGateV1Status {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "ARMED" => Ok(Self::Armed),
            "FIRED" => Ok(Self::Fired),
            "EXPIRED" => Ok(Self::Expired),
            "CANCELLED" => Ok(Self::Cancelled),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for WaitGateV1Status {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WaitGateV1Status {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`WaitGateV1WakeKey`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct WaitGateV1WakeKey(::std::string::String);
impl ::std::ops::Deref for WaitGateV1WakeKey {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<WaitGateV1WakeKey> for ::std::string::String {
    fn from(value: WaitGateV1WakeKey) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for WaitGateV1WakeKey {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for WaitGateV1WakeKey {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WaitGateV1WakeKey {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for WaitGateV1WakeKey {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`WakeEventV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct WakeEventV1 {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub aggregated_count: ::std::option::Option<::std::num::NonZeroU64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub fired_at: Timestamp,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub scheduling_enabled: ::std::option::Option<bool>,
    pub schema_id: ::serde_json::Value,
    pub schema_version: WakeEventV1SchemaVersion,
    pub source: WakeEventV1Source,
    pub targets: ::std::vec::Vec<WakeEventV1TargetsItem>,
    pub wake_id: Id,
    pub wake_key: ::std::string::String,
}
#[doc = "`WakeEventV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct WakeEventV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for WakeEventV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<WakeEventV1SchemaVersion> for ::std::string::String {
    fn from(value: WakeEventV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for WakeEventV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for WakeEventV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WakeEventV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for WakeEventV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`WakeEventV1Source`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum WakeEventV1Source {
    #[serde(rename = "TIMER")]
    Timer,
    #[serde(rename = "EVENT")]
    Event,
    #[serde(rename = "DEPENDENCY")]
    Dependency,
    #[serde(rename = "MANUAL")]
    Manual,
    #[serde(rename = "ATTENTION")]
    Attention,
}
impl ::std::fmt::Display for WakeEventV1Source {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Timer => f.write_str("TIMER"),
            Self::Event => f.write_str("EVENT"),
            Self::Dependency => f.write_str("DEPENDENCY"),
            Self::Manual => f.write_str("MANUAL"),
            Self::Attention => f.write_str("ATTENTION"),
        }
    }
}
impl ::std::str::FromStr for WakeEventV1Source {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "TIMER" => Ok(Self::Timer),
            "EVENT" => Ok(Self::Event),
            "DEPENDENCY" => Ok(Self::Dependency),
            "MANUAL" => Ok(Self::Manual),
            "ATTENTION" => Ok(Self::Attention),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for WakeEventV1Source {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WakeEventV1Source {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`WakeEventV1TargetsItem`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct WakeEventV1TargetsItem {
    pub id: Id,
    pub kind: WakeEventV1TargetsItemKind,
}
#[doc = "`WakeEventV1TargetsItemKind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum WakeEventV1TargetsItemKind {
    #[serde(rename = "WAIT_GATE")]
    WaitGate,
    #[serde(rename = "CAMPAIGN")]
    Campaign,
}
impl ::std::fmt::Display for WakeEventV1TargetsItemKind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::WaitGate => f.write_str("WAIT_GATE"),
            Self::Campaign => f.write_str("CAMPAIGN"),
        }
    }
}
impl ::std::str::FromStr for WakeEventV1TargetsItemKind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "WAIT_GATE" => Ok(Self::WaitGate),
            "CAMPAIGN" => Ok(Self::Campaign),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for WakeEventV1TargetsItemKind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WakeEventV1TargetsItemKind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`WakePolicyV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct WakePolicyV1 {
    pub defaults_version: ::std::string::String,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub max_wakes_per_day: u64,
    pub min_interval_seconds: u64,
    pub schema_id: ::serde_json::Value,
    pub schema_version: WakePolicyV1SchemaVersion,
    pub skip_if_run_active: bool,
    pub triggers: ::std::vec::Vec<WakeTrigger>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub until: ::std::option::Option<Timestamp>,
}
#[doc = "`WakePolicyV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct WakePolicyV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for WakePolicyV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<WakePolicyV1SchemaVersion> for ::std::string::String {
    fn from(value: WakePolicyV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for WakePolicyV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for WakePolicyV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WakePolicyV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for WakePolicyV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`WakeTrigger`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct WakeTrigger {
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub at: ::std::option::Option<Timestamp>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub dependency_ref: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub event_type: ::std::option::Option<::std::string::String>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub interval_seconds: ::std::option::Option<::std::num::NonZeroU64>,
    pub kind: WakeTriggerKind,
}
#[doc = "`WakeTriggerKind`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum WakeTriggerKind {
    #[serde(rename = "TIMER")]
    Timer,
    #[serde(rename = "EVENT")]
    Event,
    #[serde(rename = "DEPENDENCY")]
    Dependency,
    #[serde(rename = "MANUAL")]
    Manual,
    #[serde(rename = "ATTENTION_RESOLVED")]
    AttentionResolved,
}
impl ::std::fmt::Display for WakeTriggerKind {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Timer => f.write_str("TIMER"),
            Self::Event => f.write_str("EVENT"),
            Self::Dependency => f.write_str("DEPENDENCY"),
            Self::Manual => f.write_str("MANUAL"),
            Self::AttentionResolved => f.write_str("ATTENTION_RESOLVED"),
        }
    }
}
impl ::std::str::FromStr for WakeTriggerKind {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "TIMER" => Ok(Self::Timer),
            "EVENT" => Ok(Self::Event),
            "DEPENDENCY" => Ok(Self::Dependency),
            "MANUAL" => Ok(Self::Manual),
            "ATTENTION_RESOLVED" => Ok(Self::AttentionResolved),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for WakeTriggerKind {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WakeTriggerKind {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`WorkNodeLifecycleV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct WorkNodeLifecycleV1 {
    pub agent_state_ref: StateVersionRef,
    pub attempt: u64,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub attention_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub close_outcome: ::std::option::Option<WorkNodeLifecycleV1CloseOutcome>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub lease_id: ::std::option::Option<Id>,
    pub node_id: Id,
    pub run_id: Id,
    pub schema_id: ::serde_json::Value,
    pub schema_version: WorkNodeLifecycleV1SchemaVersion,
    pub state: LifecycleState,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub updated_at: ::std::option::Option<Timestamp>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub wait_gate_id: ::std::option::Option<Id>,
    pub work_state_version: StateVersion,
}
#[doc = "`WorkNodeLifecycleV1CloseOutcome`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum WorkNodeLifecycleV1CloseOutcome {
    #[serde(rename = "COMMITTED")]
    Committed,
    #[serde(rename = "PARTIAL")]
    Partial,
    #[serde(rename = "FAILED")]
    Failed,
    #[serde(rename = "CANCELLED")]
    Cancelled,
}
impl ::std::fmt::Display for WorkNodeLifecycleV1CloseOutcome {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Committed => f.write_str("COMMITTED"),
            Self::Partial => f.write_str("PARTIAL"),
            Self::Failed => f.write_str("FAILED"),
            Self::Cancelled => f.write_str("CANCELLED"),
        }
    }
}
impl ::std::str::FromStr for WorkNodeLifecycleV1CloseOutcome {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "COMMITTED" => Ok(Self::Committed),
            "PARTIAL" => Ok(Self::Partial),
            "FAILED" => Ok(Self::Failed),
            "CANCELLED" => Ok(Self::Cancelled),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for WorkNodeLifecycleV1CloseOutcome {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WorkNodeLifecycleV1CloseOutcome {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`WorkNodeLifecycleV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct WorkNodeLifecycleV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for WorkNodeLifecycleV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<WorkNodeLifecycleV1SchemaVersion> for ::std::string::String {
    fn from(value: WorkNodeLifecycleV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for WorkNodeLifecycleV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for WorkNodeLifecycleV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WorkNodeLifecycleV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for WorkNodeLifecycleV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`WorkNodeTransitionV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct WorkNodeTransitionV1 {
    pub actor: WorkNodeTransitionV1Actor,
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub evidence_refs: ::std::vec::Vec<EvidenceRef>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub from: LifecycleState,
    pub guard: WorkNodeTransitionV1Guard,
    pub node_id: Id,
    pub run_id: Id,
    pub schema_id: ::serde_json::Value,
    pub schema_version: WorkNodeTransitionV1SchemaVersion,
    pub to: LifecycleState,
    pub work_state_version_after: StateVersion,
    pub work_state_version_before: StateVersion,
}
#[doc = "`WorkNodeTransitionV1Actor`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum WorkNodeTransitionV1Actor {
    #[serde(rename = "SYSTEM")]
    System,
    #[serde(rename = "EXECUTOR")]
    Executor,
    #[serde(rename = "HUMAN")]
    Human,
}
impl ::std::fmt::Display for WorkNodeTransitionV1Actor {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::System => f.write_str("SYSTEM"),
            Self::Executor => f.write_str("EXECUTOR"),
            Self::Human => f.write_str("HUMAN"),
        }
    }
}
impl ::std::str::FromStr for WorkNodeTransitionV1Actor {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "SYSTEM" => Ok(Self::System),
            "EXECUTOR" => Ok(Self::Executor),
            "HUMAN" => Ok(Self::Human),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for WorkNodeTransitionV1Actor {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WorkNodeTransitionV1Actor {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`WorkNodeTransitionV1Guard`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum WorkNodeTransitionV1Guard {
    #[serde(rename = "ADMISSION_OK")]
    AdmissionOk,
    #[serde(rename = "DEPS_MET")]
    DepsMet,
    #[serde(rename = "LEASE_ACQUIRED")]
    LeaseAcquired,
    #[serde(rename = "SPAWN_OK")]
    SpawnOk,
    #[serde(rename = "HEARTBEAT")]
    Heartbeat,
    #[serde(rename = "OUTPUT_RECORDED")]
    OutputRecorded,
    #[serde(rename = "VERIFY_PASS")]
    VerifyPass,
    #[serde(rename = "VERIFY_FAIL")]
    VerifyFail,
    #[serde(rename = "COMPLETION_DECIDED")]
    CompletionDecided,
    #[serde(rename = "REPLAN_ACCEPTED")]
    ReplanAccepted,
    #[serde(rename = "ATTENTION_OPENED")]
    AttentionOpened,
    #[serde(rename = "ATTENTION_RESOLVED")]
    AttentionResolved,
    #[serde(rename = "WAIT_GATE_ARMED")]
    WaitGateArmed,
    #[serde(rename = "WAKE")]
    Wake,
    #[serde(rename = "LEASE_EXPIRED")]
    LeaseExpired,
    #[serde(rename = "BUDGET_STOP")]
    BudgetStop,
    #[serde(rename = "CANCEL_REQUESTED")]
    CancelRequested,
    #[serde(rename = "EFFECTS_SETTLED")]
    EffectsSettled,
    #[serde(rename = "UNRECOVERABLE")]
    Unrecoverable,
}
impl ::std::fmt::Display for WorkNodeTransitionV1Guard {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::AdmissionOk => f.write_str("ADMISSION_OK"),
            Self::DepsMet => f.write_str("DEPS_MET"),
            Self::LeaseAcquired => f.write_str("LEASE_ACQUIRED"),
            Self::SpawnOk => f.write_str("SPAWN_OK"),
            Self::Heartbeat => f.write_str("HEARTBEAT"),
            Self::OutputRecorded => f.write_str("OUTPUT_RECORDED"),
            Self::VerifyPass => f.write_str("VERIFY_PASS"),
            Self::VerifyFail => f.write_str("VERIFY_FAIL"),
            Self::CompletionDecided => f.write_str("COMPLETION_DECIDED"),
            Self::ReplanAccepted => f.write_str("REPLAN_ACCEPTED"),
            Self::AttentionOpened => f.write_str("ATTENTION_OPENED"),
            Self::AttentionResolved => f.write_str("ATTENTION_RESOLVED"),
            Self::WaitGateArmed => f.write_str("WAIT_GATE_ARMED"),
            Self::Wake => f.write_str("WAKE"),
            Self::LeaseExpired => f.write_str("LEASE_EXPIRED"),
            Self::BudgetStop => f.write_str("BUDGET_STOP"),
            Self::CancelRequested => f.write_str("CANCEL_REQUESTED"),
            Self::EffectsSettled => f.write_str("EFFECTS_SETTLED"),
            Self::Unrecoverable => f.write_str("UNRECOVERABLE"),
        }
    }
}
impl ::std::str::FromStr for WorkNodeTransitionV1Guard {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "ADMISSION_OK" => Ok(Self::AdmissionOk),
            "DEPS_MET" => Ok(Self::DepsMet),
            "LEASE_ACQUIRED" => Ok(Self::LeaseAcquired),
            "SPAWN_OK" => Ok(Self::SpawnOk),
            "HEARTBEAT" => Ok(Self::Heartbeat),
            "OUTPUT_RECORDED" => Ok(Self::OutputRecorded),
            "VERIFY_PASS" => Ok(Self::VerifyPass),
            "VERIFY_FAIL" => Ok(Self::VerifyFail),
            "COMPLETION_DECIDED" => Ok(Self::CompletionDecided),
            "REPLAN_ACCEPTED" => Ok(Self::ReplanAccepted),
            "ATTENTION_OPENED" => Ok(Self::AttentionOpened),
            "ATTENTION_RESOLVED" => Ok(Self::AttentionResolved),
            "WAIT_GATE_ARMED" => Ok(Self::WaitGateArmed),
            "WAKE" => Ok(Self::Wake),
            "LEASE_EXPIRED" => Ok(Self::LeaseExpired),
            "BUDGET_STOP" => Ok(Self::BudgetStop),
            "CANCEL_REQUESTED" => Ok(Self::CancelRequested),
            "EFFECTS_SETTLED" => Ok(Self::EffectsSettled),
            "UNRECOVERABLE" => Ok(Self::Unrecoverable),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for WorkNodeTransitionV1Guard {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WorkNodeTransitionV1Guard {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`WorkNodeTransitionV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct WorkNodeTransitionV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for WorkNodeTransitionV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<WorkNodeTransitionV1SchemaVersion> for ::std::string::String {
    fn from(value: WorkNodeTransitionV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for WorkNodeTransitionV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for WorkNodeTransitionV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WorkNodeTransitionV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for WorkNodeTransitionV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`WorkRunRecordV1`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct WorkRunRecordV1 {
    pub agent_state_ref: StateVersionRef,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub attempts: ::std::option::Option<u64>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub billable_stopped: ::std::option::Option<bool>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub budget_usage: ::std::option::Option<ResourceBudget>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub campaign_id: ::std::option::Option<Id>,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub extensions: ::std::option::Option<ExtensionMap>,
    pub resolved: WorkRunRecordV1Resolved,
    pub run_id: Id,
    pub schema_id: ::serde_json::Value,
    pub schema_version: WorkRunRecordV1SchemaVersion,
    pub status: RunStatus,
    pub work_state_version: StateVersion,
}
#[doc = "`WorkRunRecordV1Resolved`"]
#[derive(:: serde :: Deserialize, :: serde :: Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct WorkRunRecordV1Resolved {
    pub agent: WorkRunRecordV1ResolvedAgent,
    pub authority_profile: WorkRunRecordV1ResolvedAuthorityProfile,
    pub budget: ResourceBudget,
    pub completion_policy: CompletionPolicyRef,
    pub defaults_version: ::std::string::String,
    pub logical_model_id: LogicalModelId,
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    pub workspace: ::std::option::Option<::std::string::String>,
}
#[doc = "`WorkRunRecordV1ResolvedAgent`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct WorkRunRecordV1ResolvedAgent(::std::string::String);
impl ::std::ops::Deref for WorkRunRecordV1ResolvedAgent {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<WorkRunRecordV1ResolvedAgent> for ::std::string::String {
    fn from(value: WorkRunRecordV1ResolvedAgent) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for WorkRunRecordV1ResolvedAgent {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| {
                ::regress::Regex::new("^[a-z0-9][a-z0-9-]*@v\\d+$").unwrap()
            });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[a-z0-9][a-z0-9-]*@v\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for WorkRunRecordV1ResolvedAgent {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WorkRunRecordV1ResolvedAgent {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for WorkRunRecordV1ResolvedAgent {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = "`WorkRunRecordV1ResolvedAuthorityProfile`"]
#[derive(
    :: serde :: Deserialize,
    :: serde :: Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
)]
pub enum WorkRunRecordV1ResolvedAuthorityProfile {
    #[serde(rename = "read-only")]
    ReadOnly,
    #[serde(rename = "code-safe")]
    CodeSafe,
    #[serde(rename = "code-write")]
    CodeWrite,
}
impl ::std::fmt::Display for WorkRunRecordV1ResolvedAuthorityProfile {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::ReadOnly => f.write_str("read-only"),
            Self::CodeSafe => f.write_str("code-safe"),
            Self::CodeWrite => f.write_str("code-write"),
        }
    }
}
impl ::std::str::FromStr for WorkRunRecordV1ResolvedAuthorityProfile {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "read-only" => Ok(Self::ReadOnly),
            "code-safe" => Ok(Self::CodeSafe),
            "code-write" => Ok(Self::CodeWrite),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for WorkRunRecordV1ResolvedAuthorityProfile {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WorkRunRecordV1ResolvedAuthorityProfile {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
#[doc = "`WorkRunRecordV1SchemaVersion`"]
#[derive(:: serde :: Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct WorkRunRecordV1SchemaVersion(::std::string::String);
impl ::std::ops::Deref for WorkRunRecordV1SchemaVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<WorkRunRecordV1SchemaVersion> for ::std::string::String {
    fn from(value: WorkRunRecordV1SchemaVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for WorkRunRecordV1SchemaVersion {
    type Err = self::error::ConversionError;
    fn from_str(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> =
            ::std::sync::LazyLock::new(|| ::regress::Regex::new("^1\\.\\d+\\.\\d+$").unwrap());
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^1\\.\\d+\\.\\d+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for WorkRunRecordV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(value: &str) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for WorkRunRecordV1SchemaVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for WorkRunRecordV1SchemaVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
#[doc = " Error types."]
pub mod error {
    #[doc = r" Error from a `TryFrom` or `FromStr` implementation."]
    pub struct ConversionError(::std::borrow::Cow<'static, str>);
    impl ::std::error::Error for ConversionError {}
    impl ::std::fmt::Display for ConversionError {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
            ::std::fmt::Display::fmt(&self.0, f)
        }
    }
    impl ::std::fmt::Debug for ConversionError {
        fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
            ::std::fmt::Debug::fmt(&self.0, f)
        }
    }
    impl From<&'static str> for ConversionError {
        fn from(value: &'static str) -> Self {
            Self(value.into())
        }
    }
    impl From<String> for ConversionError {
        fn from(value: String) -> Self {
            Self(value.into())
        }
    }
}

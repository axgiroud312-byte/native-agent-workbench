use std::{error::Error, fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! local_ids {
    ($($name:ident),+ $(,)?) => {$ (
        #[doc = "A stable local identity, allocated once and preserved across serialization."]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self { Self(Uuid::new_v4()) }
            pub const fn from_uuid(value: Uuid) -> Self { Self(value) }
            pub const fn as_uuid(&self) -> &Uuid { &self.0 }
        }

        impl Default for $name {
            fn default() -> Self { Self::new() }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(value: &str) -> Result<Self, Self::Err> { value.parse().map(Self) }
        }
    )+};
}

local_ids!(
    ProjectId,
    AgentId,
    SessionId,
    RunId,
    MessageId,
    ContentBlockId,
    ToolCallId,
    RequestId,
    AttachmentId,
    AttachmentBlobId,
    FileReferenceId,
    FileChangeId,
    PlanId,
    PlanStepId,
    QueuedInputId,
    SubmissionId,
    ResponseAttemptId,
);

macro_rules! agent_ids {
    ($($name:ident),+ $(,)?) => {$ (
        #[doc = "An opaque Agent-provided identifier; never interchangeable with a local ID."]
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self { Self(value.into()) }
            pub fn as_str(&self) -> &str { &self.0 }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    )+};
}

agent_ids!(
    ExternalSessionId,
    ExternalRunId,
    ExternalMessageId,
    ExternalToolCallId,
    ExternalRequestId,
    ExternalSubmissionId,
    AgentOptionId,
    AgentCommandId,
    AgentFieldId,
);

/// Monotonic per-session connection generation, persisted before starting a connection.
/// Zero means no connection has been attempted. Never reuse a generation after restart.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ConnectionEpoch(u64);

impl ConnectionEpoch {
    pub const INITIAL: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u64 {
        self.0
    }
    pub const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

/// UTC milliseconds since the Unix epoch; supplied by the owner rather than read in models.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(i64);

impl Timestamp {
    pub const fn new(unix_millis: i64) -> Self {
        Self(unix_millis)
    }

    pub const fn unix_millis(self) -> i64 {
        self.0
    }
}

/// Required owner for messages, tools, plans, file changes and interaction requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RunContext {
    pub session_id: SessionId,
    pub run_id: RunId,
}

impl RunContext {
    pub const fn new(session_id: SessionId, run_id: RunId) -> Self {
        Self { session_id, run_id }
    }
}

/// Version of the serialized business schema; separate from crate, DB and ACP versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct ModelVersion;

impl ModelVersion {
    pub const CURRENT: Self = Self;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnsupportedModelVersion(u16);

impl UnsupportedModelVersion {
    pub const fn version(self) -> u16 {
        self.0
    }
}

impl fmt::Display for UnsupportedModelVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unsupported business model version {}", self.0)
    }
}

impl Error for UnsupportedModelVersion {}

impl TryFrom<u16> for ModelVersion {
    type Error = UnsupportedModelVersion;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self),
            other => Err(UnsupportedModelVersion(other)),
        }
    }
}

impl From<ModelVersion> for u16 {
    fn from(_: ModelVersion) -> Self {
        1
    }
}

/// Wrap an independently stored or exchanged business record at the serialization boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Versioned<T> {
    pub model_version: ModelVersion,
    pub record: T,
}

impl<T> Versioned<T> {
    pub fn new(record: T) -> Self {
        Self {
            model_version: ModelVersion::CURRENT,
            record,
        }
    }
}

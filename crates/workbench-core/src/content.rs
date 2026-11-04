use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AgentCommandId, AttachmentBlobId, AttachmentId, ContentBlockId, ExternalMessageId,
    FileChangeId, FileReferenceId, MessageId, PlanId, ProjectId, RunContext, SessionId,
    SubmissionId, Timestamp, ToolCallId,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Agent,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessageState {
    Streaming,
    Complete,
    Interrupted { reason: String },
}

/// Run-owned content; user submission receipt is held by InputSubmission, not inferred here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Message {
    pub id: MessageId,
    pub context: RunContext,
    pub external_id: Option<ExternalMessageId>,
    pub role: MessageRole,
    pub created_at: Timestamp,
    pub state: MessageState,
    pub submission_id: Option<SubmissionId>,
    pub blocks: Vec<ContentBlock>,
}

impl Message {
    pub fn new(
        id: MessageId,
        context: RunContext,
        role: MessageRole,
        created_at: Timestamp,
    ) -> Self {
        Self {
            id,
            context,
            external_id: None,
            role,
            created_at,
            state: MessageState::Streaming,
            submission_id: None,
            blocks: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ContentBlock {
    pub id: ContentBlockId,
    pub content: ContentKind,
}

impl ContentBlock {
    pub fn new(id: ContentBlockId, content: ContentKind) -> Self {
        Self { id, content }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextFormat {
    Plain,
    Markdown,
}

/// Structural references point to records owned by the same session/run as the message.
/// Unsupported payloads remain diagnostic content and never become executable UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentKind {
    Text {
        text: String,
        format: TextFormat,
    },
    Image {
        attachment_id: AttachmentId,
        alt: Option<String>,
    },
    Attachment {
        attachment_id: AttachmentId,
    },
    FileReference {
        reference: FileReference,
    },
    AgentCommand {
        command_id: AgentCommandId,
        arguments: String,
    },
    ToolCall {
        tool_call_id: ToolCallId,
    },
    Plan {
        plan_id: PlanId,
    },
    FileChange {
        file_change_id: FileChangeId,
    },
    Unsupported {
        type_name: String,
        payload: Value,
    },
}

/// Stable file selection identity retained through drafts, retries and messages.
/// Path is a location, not a session ID; file contents remain owned by the user project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FileReference {
    pub id: FileReferenceId,
    pub project_id: ProjectId,
    pub path: PathBuf,
    pub lines: Option<LineRange>,
}

impl FileReference {
    pub fn new(id: FileReferenceId, project_id: ProjectId, path: PathBuf) -> Self {
        Self {
            id,
            project_id,
            path,
            lines: None,
        }
    }
}

/// One-based inclusive source lines. Construction rejects empty/reversed ranges.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LineRangeRecord", into = "LineRangeRecord")]
pub struct LineRange {
    start: u32,
    end: u32,
}

#[derive(Serialize, Deserialize)]
struct LineRangeRecord {
    start: u32,
    end: u32,
}

impl LineRange {
    pub fn new(start: u32, end: u32) -> Result<Self, &'static str> {
        if start == 0 || end < start {
            return Err("lines must be one-based and ordered");
        }
        Ok(Self { start, end })
    }
    pub const fn start(self) -> u32 {
        self.start
    }
    pub const fn end(self) -> u32 {
        self.end
    }
}

impl TryFrom<LineRangeRecord> for LineRange {
    type Error = &'static str;
    fn try_from(value: LineRangeRecord) -> Result<Self, Self::Error> {
        Self::new(value.start, value.end)
    }
}

impl From<LineRange> for LineRangeRecord {
    fn from(value: LineRange) -> Self {
        Self {
            start: value.start,
            end: value.end,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttachmentKind {
    Image {
        width: Option<u32>,
        height: Option<u32>,
    },
    File,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttachmentSource {
    Clipboard,
    ImportedFile { original_path: PathBuf },
    AgentReported,
}

/// Storage maps an opaque blob ID into app data. Core never stores arbitrary deletion paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttachmentStorage {
    Pending,
    Managed { blob_id: AttachmentBlobId },
    Failed { reason: String },
    Missing { reason: String },
}

/// Session-owned metadata, referenced by drafts, queue snapshots, messages and tools.
/// Blob ownership does not transfer to a user file merely because it was imported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Attachment {
    pub id: AttachmentId,
    pub session_id: SessionId,
    pub created_at: Timestamp,
    pub name: String,
    pub media_type: String,
    pub byte_length: Option<u64>,
    pub kind: AttachmentKind,
    pub source: AttachmentSource,
    pub storage: AttachmentStorage,
}

impl Attachment {
    pub fn new(
        id: AttachmentId,
        session_id: SessionId,
        created_at: Timestamp,
        name: impl Into<String>,
        media_type: impl Into<String>,
        kind: AttachmentKind,
        source: AttachmentSource,
    ) -> Self {
        Self {
            id,
            session_id,
            created_at,
            name: name.into(),
            media_type: media_type.into(),
            byte_length: None,
            kind,
            source,
            storage: AttachmentStorage::Pending,
        }
    }
}

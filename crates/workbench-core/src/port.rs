use crate::*;
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};

/// No Send requirement: SDKs using a local executor can implement the same narrow port.
pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, BusinessError>> + 'a>>;

/// App queues incoming facts, assigns host sequence, commits and applies the reducer.
pub trait RuntimeEventSink {
    fn emit(&self, event: EventEnvelope) -> PortFuture<'_, ()>;
}

/// Must return control asynchronously. Never wait for user input in an SDK dispatch callback.
/// Different sessions may execute concurrently; no global in-flight prompt lock is implied.
/// App/adapter enqueue and BEGIN each session's operations in committed JournalRecord.sequence
/// order, then RuntimeEffect list order. Preserve Submit-before-Stop at protocol dispatch.
/// Poll/start later Cancel/response operations without awaiting a long prompt or user answer;
/// a pending prompt future must not serialize the session's control operations behind its end.
/// Completion remains a typed EventSink fact, not permission to dispatch the next effect.
/// ReconcileClosingResources proves ownership and cleans/verifies the specified abandoned
/// epoch; it must never kill unrelated processes or infer release merely from app restart.
pub trait RuntimePort {
    fn execute<'a>(
        &'a self,
        effect: RuntimeEffect,
        sink: &'a dyn RuntimeEventSink,
    ) -> PortFuture<'a, ()>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StorageCommit {
    pub expected_sequence: u64,
    pub record: JournalRecord,
    /// Matching post-reducer checkpoint. Adapter may encode normalized rows instead.
    pub checkpoint: Versioned<WorkbenchState>,
}
impl StorageCommit {
    pub fn new(expected_sequence: u64, record: JournalRecord, checkpoint: WorkbenchState) -> Self {
        Self {
            expected_sequence,
            record,
            checkpoint: Versioned::new(checkpoint),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RestoreData {
    pub checkpoint: Option<Versioned<WorkbenchState>>,
    pub tail: Vec<JournalRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HistoryQuery {
    pub session_id: SessionId,
    pub before_sequence: Option<u64>,
    pub limit: u32,
}
impl HistoryQuery {
    pub fn new(session_id: SessionId, before_sequence: Option<u64>, limit: u32) -> Self {
        Self {
            session_id,
            before_sequence,
            limit,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HistoryPage {
    pub records: Vec<JournalRecord>,
    pub next_before_sequence: Option<u64>,
}

/// Transaction commit is compare-and-swap on host sequence: journal and checkpoint atomically.
/// Only after success may app publish next state and deliver RuntimeEffects. Errors propagate.
pub trait StoragePort {
    fn read_preferences(&self) -> PortFuture<'_, Option<Versioned<WorkspacePreferences>>>;
    fn write_preferences(&self, preferences: Versioned<WorkspacePreferences>)
    -> PortFuture<'_, ()>;
    fn restore(&self) -> PortFuture<'_, RestoreData>;
    fn commit(&self, batch: StorageCommit) -> PortFuture<'_, ()>;
    fn history(&self, query: HistoryQuery) -> PortFuture<'_, HistoryPage>;
    /// Blob keys resolve exclusively within application data, never a project path.
    fn write_attachment(&self, id: AttachmentBlobId, bytes: Vec<u8>) -> PortFuture<'_, ()>;
    fn read_attachment(&self, id: AttachmentBlobId) -> PortFuture<'_, Vec<u8>>;
    /// Adapter must recheck every retained reference transactionally before collection.
    fn collect_attachment(&self, id: AttachmentBlobId) -> PortFuture<'_, ()>;
}

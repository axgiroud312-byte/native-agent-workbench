use crate::{
    database::{Database, restored, storage_error},
    default_data_directory,
};
use futures_channel::oneshot;
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
};
use workbench_core::*;

type Operation = Box<dyn FnOnce(&mut Database) + Send>;
enum Job {
    Run(Operation),
    Shutdown(oneshot::Sender<()>),
}

/// Cloneable async handle. SQLite, migration and attachment filesystem work stays on its thread.
#[derive(Clone)]
pub struct SqliteStorage {
    sender: mpsc::Sender<Job>,
    root: PathBuf,
}

impl SqliteStorage {
    pub fn open_default() -> PortFuture<'static, Self> {
        match default_data_directory() {
            Ok(root) => Self::open_at(root),
            Err(e) => Box::pin(async move { Err(e) }),
        }
    }
    /// Explicit application-data override for isolated tests or app configuration.
    pub fn open_at(root: PathBuf) -> PortFuture<'static, Self> {
        let (sender, receiver) = mpsc::channel::<Job>();
        let (ready, opened) = oneshot::channel();
        let started = std::thread::Builder::new()
            .name("workbench-storage".into())
            .spawn(move || {
                let mut database = match Database::open(root) {
                    Ok(database) => database,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                if ready.send(Ok(database.root.clone())).is_err() {
                    return;
                }
                while let Ok(job) = receiver.recv() {
                    match job {
                        Job::Run(operation) => operation(&mut database),
                        Job::Shutdown(done) => {
                            drop(database);
                            let _ = done.send(());
                            return;
                        }
                    }
                }
            });
        Box::pin(async move {
            started.map_err(storage_error)?;
            let root = opened
                .await
                .map_err(|_| storage_error("database worker stopped during open"))??;
            Ok(Self { sender, root })
        })
    }
    pub fn data_directory(&self) -> &Path {
        &self.root
    }
    pub(crate) fn call<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Database) -> Result<T, BusinessError> + Send + 'static,
    ) -> PortFuture<'static, T> {
        let (reply, response) = oneshot::channel();
        let queued = self.sender.send(Job::Run(Box::new(move |db| {
            let _ = reply.send(operation(db));
        })));
        Box::pin(async move {
            queued.map_err(|_| storage_error("database worker is closed"))?;
            response
                .await
                .map_err(|_| storage_error("database worker stopped before replying"))?
        })
    }
    /// Waits until SQLite is closed; useful before releasing an isolated test data directory.
    /// Cloned handles become closed too. Already queued transactions finish first.
    pub fn shutdown(self) -> PortFuture<'static, ()> {
        let (reply, response) = oneshot::channel();
        let queued = self.sender.send(Job::Shutdown(reply));
        Box::pin(async move {
            queued.map_err(|_| storage_error("database worker is closed"))?;
            response
                .await
                .map_err(|_| storage_error("database worker stopped before closing"))
        })
    }
    pub fn attachment_reference_count(&self, id: AttachmentBlobId) -> PortFuture<'static, u64> {
        self.call(move |db| db.reference_count(id))
    }
}

impl StoragePort for SqliteStorage {
    fn read_preferences(&self) -> PortFuture<'_, Option<Versioned<WorkspacePreferences>>> {
        self.call(|db| db.preferences())
    }
    fn write_preferences(
        &self,
        preferences: Versioned<WorkspacePreferences>,
    ) -> PortFuture<'_, ()> {
        self.call(move |db| db.save_preferences(preferences))
    }
    fn restore(&self) -> PortFuture<'_, RestoreData> {
        self.call(|db| restored(&db.connection))
    }
    fn commit(&self, batch: StorageCommit) -> PortFuture<'_, ()> {
        self.call(move |db| db.commit(batch))
    }
    fn history(&self, query: HistoryQuery) -> PortFuture<'_, HistoryPage> {
        self.call(move |db| db.history(query))
    }
    fn write_attachment(&self, id: AttachmentBlobId, bytes: Vec<u8>) -> PortFuture<'_, ()> {
        self.call(move |db| db.write_blob(id, bytes))
    }
    fn read_attachment(&self, id: AttachmentBlobId) -> PortFuture<'_, Vec<u8>> {
        self.call(move |db| db.read_blob(id))
    }
    fn collect_attachment(&self, id: AttachmentBlobId) -> PortFuture<'_, ()> {
        self.call(move |db| db.collect_blob(id))
    }
}

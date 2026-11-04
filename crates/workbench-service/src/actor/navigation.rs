use super::*;
use std::time::Duration;

const RELEASE_DEADLINE: Duration = Duration::from_secs(15);

/// An outstanding application intent, not a second connection/Run state machine.
pub(super) struct Removal {
    pub expected: BTreeMap<SessionId, ConnectionEpoch>,
    pub deadline: tokio::time::Instant,
    pub replies: Vec<Reply<()>>,
    command_id: CommandId,
}

impl Actor {
    pub(super) async fn check_directory(&mut self, id: ProjectId) -> Result<(), BusinessError> {
        let project = self
            .frame
            .workbench
            .projects
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| fail(ErrorKind::NotFound, "project is absent"))?;
        let result = self
            .storage
            .validate_project_directory(id)
            .await
            .map(|_| ());
        self.frame
            .project_directories
            .retain(|entry| entry.project_id != id);
        self.frame
            .project_directories
            .push(ProjectDirectorySnapshot {
                project_id: id,
                directory: project.directory,
                checked_at: now(),
                error: result.clone().err(),
            });
        self.publish();
        result
    }
    pub(super) async fn select_project(
        &mut self,
        id: Option<ProjectId>,
    ) -> Result<u64, BusinessError> {
        self.accept()?;
        if let Some(id) = id {
            let _ = self.check_directory(id).await;
        }
        let mut preferences = self.frame.preferences.clone();
        preferences.selected_project_id = id;
        if preferences.selected_session_id.is_some_and(|session| {
            self.frame
                .workbench
                .sessions
                .iter()
                .find(|s| s.id == session)
                .is_none_or(|s| Some(s.project_id) != id)
        }) {
            preferences.selected_session_id = None;
        }
        let previous = self.frame.selection.generation;
        let changed = self.frame.preferences.selected_project_id != id;
        self.store_preferences(self.frame.preferences_revision, preferences)
            .await?;
        if changed && self.frame.selection.generation == previous {
            self.frame.selection.generation += 1;
            self.publish();
        }
        Ok(self.frame.selection.generation)
    }
    pub(super) async fn create_session(
        &mut self,
        project_id: ProjectId,
        agent_id: AgentId,
        title: String,
    ) -> Result<SessionId, BusinessError> {
        self.accept()?;
        self.project_admission(project_id)?;
        self.check_directory(project_id).await?;
        let agent = self.agent(agent_id)?;
        if agent.launch.is_none() {
            return Err(fail(
                ErrorKind::NotReady,
                "configure this Agent before creating a connected session",
            ));
        }
        if let AgentDiscovery::Incompatible { reason } = &agent.discovery {
            return Err(BusinessError::new(
                ErrorKind::Unsupported,
                reason,
                RetryAdvice::AfterCorrection,
            ));
        }
        let title = if title.trim().is_empty() {
            "新会话".to_owned()
        } else {
            title
        };
        let session = SessionRecord::new(SessionId::new(), project_id, agent_id, title, now());
        let id = session.id();
        self.apply_checked(CommandEnvelope::new(
            CommandId::new(),
            now(),
            Scope::default(),
            Command::CreateSession { session },
        ))
        .await?;
        // No optional selection query is allowed to swallow this persisted connection intent.
        if let Err(error) = self
            .apply_checked(CommandEnvelope::new(
                CommandId::new(),
                now(),
                Scope::session(id, ConnectionEpoch::INITIAL),
                Command::Connect { resume: false },
            ))
            .await
        {
            self.record_error(error);
        }
        let mut preferences = self.frame.preferences.clone();
        preferences.selected_project_id = Some(project_id);
        preferences.selected_session_id = Some(id);
        if let Err(error) = self
            .store_preferences(self.frame.preferences_revision, preferences)
            .await
        {
            self.record_error(error);
        }
        Ok(id)
    }
    pub(super) async fn relocate(
        &mut self,
        id: ProjectId,
        directory: PathBuf,
    ) -> Result<Project, BusinessError> {
        self.accept()?;
        self.project_admission(id)?;
        if !self.frame.workbench.projects.iter().any(|p| p.id == id) {
            return Err(fail(ErrorKind::NotFound, "project is absent"));
        }
        for summary in self
            .frame
            .workbench
            .sessions
            .iter()
            .filter(|s| s.project_id == id)
        {
            let session = self
                .storage
                .session(summary.id)
                .await?
                .ok_or_else(|| fail(ErrorKind::NotFound, "project session is absent"))?;
            if self.session_removing(summary.id)
                || !session.resources_released
                || session.closing_handoff.is_some()
            {
                return Err(fail(
                    ErrorKind::Busy,
                    "release every connection in this project before explicitly changing its directory",
                ));
            }
        }
        let update = self
            .storage
            .relocate_project(CommandId::new(), id, directory, now())
            .await?;
        let rejection = update.decisions.iter().find_map(|d| {
            if let ApplyDisposition::Rejected(error) = &d.disposition {
                Some(error.clone())
            } else {
                None
            }
        });
        self.committed(update).await?;
        if let Some(error) = rejection {
            return Err(error);
        }
        self.check_directory(id).await?;
        self.frame
            .workbench
            .projects
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| fail(ErrorKind::NotFound, "relocated project is absent"))
    }
    pub(super) async fn command_admission(
        &mut self,
        command: &CommandEnvelope,
    ) -> Result<(), BusinessError> {
        match &command.command {
            Command::CreateSession { session } => {
                self.project_admission(session.project_id())?;
                self.check_directory(session.project_id()).await?;
            }
            Command::PutProject { project } => self.project_admission(project.id)?,
            Command::DeleteProject { project_id } => {
                self.project_admission(*project_id)?;
                if self
                    .frame
                    .workbench
                    .sessions
                    .iter()
                    .any(|s| s.project_id == *project_id && self.session_removing(s.id))
                {
                    return Err(fail(
                        ErrorKind::Busy,
                        "a project session removal is already pending",
                    ));
                }
            }
            Command::Connect { .. }
            | Command::CloseSession
            | Command::DeleteSession
            | Command::ReconcileClosingResources { .. } => {
                if let Some(id) = command.scope.session_id {
                    if self.session_removing(id) {
                        return Err(fail(ErrorKind::Busy, "this session is being removed"));
                    }
                    if matches!(command.command, Command::Connect { .. }) {
                        let session = self
                            .storage
                            .session(id)
                            .await?
                            .ok_or_else(|| fail(ErrorKind::NotFound, "session is absent"))?;
                        self.project_admission(session.record.project_id())?;
                        self.check_directory(session.record.project_id()).await?;
                    }
                }
            }
            Command::Restart if !self.removals.is_empty() => {
                return Err(fail(ErrorKind::Busy, "session removals are still pending"));
            }
            _ => {}
        }
        Ok(())
    }
    fn project_admission(&self, id: ProjectId) -> Result<(), BusinessError> {
        if self.removals.contains_key(&RemovalTarget::Project(id)) {
            return Err(fail(ErrorKind::Busy, "project removal is pending"));
        }
        Ok(())
    }
    fn session_removing(&self, id: SessionId) -> bool {
        self.removals
            .values()
            .any(|removal| removal.expected.contains_key(&id))
    }
    async fn apply_checked(&mut self, command: CommandEnvelope) -> Result<(), BusinessError> {
        match self.commit_command(command).await?.0 {
            CommandStatus::Applied => Ok(()),
            CommandStatus::Rejected { error } => Err(error),
        }
    }
    pub(super) async fn commit_command(
        &mut self,
        command: CommandEnvelope,
    ) -> Result<(CommandStatus, bool), BusinessError> {
        let id = command.id;
        let update = self.storage.execute(command).await?;
        let decision = update.decisions.last().map(|d| d.disposition.clone());
        self.committed(update).await?;
        match decision {
            Some(ApplyDisposition::Applied | ApplyDisposition::Stale) => {
                Ok((CommandStatus::Applied, true))
            }
            Some(ApplyDisposition::Rejected(error)) => {
                Ok((CommandStatus::Rejected { error }, true))
            }
            Some(ApplyDisposition::Duplicate) => self
                .storage
                .command_result(id)
                .await?
                .map(|result| (result.status, false))
                .ok_or_else(|| {
                    fail(
                        ErrorKind::Storage,
                        "duplicate command has no persisted result",
                    )
                }),
            None => Err(fail(
                ErrorKind::Storage,
                "committed command decision is missing",
            )),
        }
    }
    pub(super) async fn remove(&mut self, target: RemovalTarget, reply: Reply<()>) {
        if let Err(error) = self.accept() {
            self.reply(reply, Err(error));
            return;
        }
        if let Some(removal) = self.removals.get_mut(&target) {
            removal.replies.push(reply);
            return;
        }
        let result=async {
            let ids=match target {
                RemovalTarget::Session(id)=>vec![id],
                RemovalTarget::Project(id)=>{
                    if !self.frame.workbench.projects.iter().any(|p|p.id==id){return Err(fail(ErrorKind::NotFound,"project is absent"));}
                    self.frame.workbench.sessions.iter().filter(|s|s.project_id==id).map(|s|s.id).collect()
                },
            };
            let mut sessions=Vec::new();
            for id in ids {
                if self.session_removing(id){return Err(fail(ErrorKind::Busy,"an overlapping removal is already pending"));}
                let session=self.storage.session(id).await?.ok_or_else(||fail(ErrorKind::NotFound,"session is absent"))?;
                self.project_admission(session.record.project_id())?;
                if session.closing_handoff.is_some(){return Err(BusinessError::new(ErrorKind::ResourceUnavailable,"old connection resources require authoritative reconciliation before removal",RetryAdvice::ReconcileFirst));}
                sessions.push(session);
            }
            Ok(sessions)
        }.await;
        let sessions = match result {
            Ok(sessions) => sessions,
            Err(error) => {
                self.reply(reply, Err(error));
                return;
            }
        };
        let expected = sessions
            .iter()
            .map(|s| (s.record.id(), s.record.connection.epoch))
            .collect();
        let waiting_sessions = sessions
            .iter()
            .filter(|s| !s.resources_released)
            .map(|s| s.record.id())
            .collect();
        self.removals.insert(
            target,
            Removal {
                expected,
                deadline: tokio::time::Instant::now() + RELEASE_DEADLINE,
                replies: vec![reply],
                command_id: CommandId::new(),
            },
        );
        self.frame.pending_removals.push(RemovalSnapshot {
            target,
            waiting_sessions,
        });
        self.publish();
        for session in sessions {
            // A never-connected/released local record needs no fabricated Runtime route.
            if session.resources_released || session.closing {
                continue;
            }
            let command = CommandEnvelope::new(
                CommandId::new(),
                now(),
                Scope::session(session.record.id(), session.record.connection.epoch),
                Command::CloseSession,
            );
            if let Err(error) = self.apply_checked(command).await {
                self.finish_removal(target, Err(error));
                return;
            }
        }
    }
    fn finish_removal(&mut self, target: RemovalTarget, result: Result<(), BusinessError>) {
        let removal = self.removals.remove(&target);
        self.frame
            .pending_removals
            .retain(|entry| entry.target != target);
        if let Err(error) = &result {
            self.frame.last_error = Some(error.clone());
        }
        self.publish();
        if let Some(removal) = removal {
            for reply in removal.replies {
                let _ = reply.send(result.clone());
            }
        }
    }
    pub(super) async fn advance_removals(&mut self) {
        let targets: Vec<_> = self.removals.keys().copied().collect();
        for target in targets {
            let expected = self.removals[&target].expected.clone();
            let result = async {
                let mut waiting = Vec::new();
                for (id, epoch) in &expected {
                    let session =
                        self.storage.session(*id).await?.ok_or_else(|| {
                            fail(ErrorKind::NotFound, "removal target disappeared")
                        })?;
                    if session.record.connection.epoch != *epoch {
                        return Err(fail(
                            ErrorKind::StaleIdentity,
                            "connection generation changed during removal; record retained",
                        ));
                    }
                    if session.closing_handoff.is_some() {
                        return Err(BusinessError::new(
                            ErrorKind::ResourceUnavailable,
                            "resource release needs authoritative reconciliation",
                            RetryAdvice::ReconcileFirst,
                        ));
                    }
                    if !session.resources_released {
                        waiting.push(*id);
                    }
                }
                Ok(waiting)
            }
            .await;
            let waiting = match result {
                Ok(waiting) => waiting,
                Err(error) => {
                    self.finish_removal(target, Err(error));
                    continue;
                }
            };
            if !waiting.is_empty() {
                if tokio::time::Instant::now() >= self.removals[&target].deadline {
                    self.finish_removal(target,Err(BusinessError::new(ErrorKind::ResourceUnavailable,"connection release was not confirmed before the deadline; records retained",RetryAdvice::ReconcileFirst)));
                } else if let Some(entry) = self
                    .frame
                    .pending_removals
                    .iter_mut()
                    .find(|entry| entry.target == target)
                {
                    if entry.waiting_sessions != waiting {
                        entry.waiting_sessions = waiting;
                        self.publish();
                    }
                }
                continue;
            }
            let id = self.removals[&target].command_id;
            let (scope, command) = match target {
                RemovalTarget::Session(session_id) => (
                    Scope::session(session_id, expected[&session_id]),
                    Command::DeleteSession,
                ),
                RemovalTarget::Project(project_id) => {
                    (Scope::default(), Command::DeleteProject { project_id })
                }
            };
            let result = self
                .apply_checked(CommandEnvelope::new(id, now(), scope, command))
                .await;
            self.finish_removal(target, result);
        }
    }
}

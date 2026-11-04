use workbench_core::*;

#[test]
fn transient_authentication_has_connection_identity_and_opaque_official_interaction() {
    let agent = AgentId::new();
    let session = SessionId::new();
    let epoch = ConnectionEpoch::new(7);
    let agent_scope = AuthenticationScope::agent(agent, epoch);
    let scope = AuthenticationScope::session(agent, session, epoch);
    assert_eq!(agent_scope.session_id(), None);
    assert_eq!(scope.session_id(), Some(session));
    assert_eq!(scope.agent_id(), agent);
    assert_eq!(scope.epoch(), epoch);
    assert_ne!(agent_scope, scope);
    assert_ne!(
        scope,
        AuthenticationScope::session(agent, session, epoch.next().unwrap())
    );
    let method = AuthenticationMethod::new(
        AuthenticationMethodId::new("agent-supplied-method"),
        "官方认证",
        AuthenticationMethodKind::AgentManaged,
        AuthenticationMethodAvailability::Available,
    );
    let request = AuthenticationRequest::new(
        RequestId::new(),
        scope,
        AuthenticationAction::Authenticate {
            method_id: method.id.clone(),
        },
    );
    let methods = AuthenticationNotice::new(
        scope,
        AuthenticationUpdate::Methods {
            methods: vec![method.clone()],
        },
    )
    .with_request(request.id);
    assert!(matches!(
        methods.update,
        AuthenticationUpdate::Methods { .. }
    ));
    let id = AuthenticationInteractionId::new();
    let interaction =
        AuthenticationInteraction::new(id, AuthenticationInteractionKind::OfficialUrl);
    let notice = AuthenticationNotice::new(
        scope,
        AuthenticationUpdate::InteractionRequired { interaction },
    )
    .with_request(request.id);
    assert_eq!(notice.request_id, Some(request.id));
    let open = AuthenticationRequest::new(
        RequestId::new(),
        scope,
        AuthenticationAction::Interact {
            interaction_id: id,
            action: AuthenticationInteractionAction::OpenOfficialFlow,
        },
    );
    assert!(
        matches!(open.action, AuthenticationAction::Interact { interaction_id, .. } if interaction_id == id)
    );
    assert_eq!(format!("{id:?}"), "AuthenticationInteractionId(..)");
    let secret_method = AuthenticationMethod::new(
        AuthenticationMethodId::new("secret-method"),
        "Agent 报告的方法",
        AuthenticationMethodKind::AgentManaged,
        AuthenticationMethodAvailability::Unavailable {
            reason: AuthenticationUnavailableReason::SecretEntryUnsupported,
        },
    );
    assert!(matches!(
        secret_method.availability,
        AuthenticationMethodAvailability::Unavailable { .. }
    ));
    let closed = AuthenticationNotice::new(
        scope,
        AuthenticationUpdate::InteractionClosed {
            interaction_id: id,
            reason: AuthenticationInteractionEnd::Expired,
        },
    );
    assert!(matches!(
        closed.update,
        AuthenticationUpdate::InteractionClosed {
            reason: AuthenticationInteractionEnd::Expired,
            ..
        }
    ));
}

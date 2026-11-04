use crate::{error, mapping::Mapper, now};
use agent_client_protocol::{self as acp, v1};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use workbench_core::*;

enum Kind {
    Permission,
    Question { fields: Vec<QuestionField> },
}
struct LiveRequest {
    context: RequestContext,
    kind: Kind,
    responder: v1::Responder,
}
#[derive(Default)]
pub(crate) struct Responders {
    live: BTreeMap<RequestId, LiveRequest>,
}
impl Responders {
    pub fn receive(
        &mut self,
        mapper: &mut Mapper,
        method: &str,
        params: Value,
        responder: v1::Responder,
    ) -> Result<(), BusinessError> {
        if method != "session/request_permission" && method != "elicitation/create" {
            let _ = responder.respond_with_error(acp::Error::method_not_found());
            return Ok(());
        }
        let Some(scope) = mapper.run_scope() else {
            let _ = responder.respond_with_error(acp::Error::request_cancelled());
            return mapper.facts.emit(mapper.session_scope(),Event::Error {error:error(ErrorKind::Unsupported,"authentication or request-scoped interaction requires the application's non-persistent authentication interface",RetryAdvice::AfterCorrection)},None);
        };
        if params["sessionId"].as_str() != mapper.external.as_deref() {
            let _ = responder.respond_with_error(acp::Error::invalid_params());
            return Err(error(
                ErrorKind::StaleIdentity,
                "interactive request has a different session scope",
                RetryAdvice::Never,
            ));
        }
        mapper.accepted()?;
        let mut context = RequestContext::new(
            RequestId::new(),
            RunContext::new(scope.session_id.unwrap(), scope.run_id.unwrap()),
            scope.epoch.unwrap(),
            now(),
        );
        // Keep the SDK's original typed ID in the responder; this string is diagnostic only.
        context.external_id = serde_json::to_string(responder.raw_id())
            .ok()
            .map(ExternalRequestId::new);
        if let Some(tool) = params.get("toolCall") {
            mapper.tool(tool)?;
        }
        let remote_tool = params["toolCall"]["toolCallId"]
            .as_str()
            .or_else(|| params["toolCallId"].as_str());
        if let Some(remote) = remote_tool {
            context.tool_call_id = mapper.tools.get(remote).map(|tool| tool.id);
        }
        let scoped = scope.with_request(context.id);
        let kind = if method == "session/request_permission" {
            let options = params["options"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|option| {
                    let id = option["optionId"].as_str()?;
                    let decision = match option["kind"].as_str() {
                        Some("allow_once") => PermissionDecisionKind::AllowOnce,
                        Some("allow_always") => PermissionDecisionKind::AllowAlways,
                        Some("reject_once") => PermissionDecisionKind::RejectOnce,
                        Some("reject_always") => PermissionDecisionKind::RejectAlways,
                        _ => PermissionDecisionKind::AgentDefined,
                    };
                    Some(PermissionOption::new(
                        CapabilityOption::new(
                            AgentOptionId::new(id),
                            option["name"].as_str().unwrap_or(id),
                        ),
                        decision,
                    ))
                })
                .collect();
            let operation = params["toolCall"]["title"]
                .as_str()
                .unwrap_or("Agent operation");
            let mut request = PermissionRequest::new(context.clone(), operation, options);
            if let Some(locations) = params["toolCall"]["locations"].as_array() {
                request.scope.resources = locations
                    .iter()
                    .filter_map(|location| {
                        location["path"]
                            .as_str()
                            .map(|path| PermissionResource::File { path: path.into() })
                    })
                    .collect();
            }
            mapper.emit(scoped, Event::Permission { request })?;
            Kind::Permission
        } else {
            if params["mode"].as_str().is_some_and(|mode| mode != "form") {
                let _ = responder.respond_with_error(acp::Error::invalid_params());
                return mapper.unsupported("unsupported_elicitation_mode", &params);
            }
            let fields = fields(&params["requestedSchema"]);
            let mut request = QuestionRequest::new(context.clone(), fields.clone());
            request.title = params["message"].as_str().map(str::to_owned);
            mapper.emit(scoped, Event::Question { request })?;
            Kind::Question { fields }
        };
        self.live.insert(
            context.id,
            LiveRequest {
                context,
                kind,
                responder,
            },
        );
        Ok(())
    }
    pub fn cancel(&mut self, ids: &[RequestId]) {
        for id in ids {
            if let Some(request) = self.live.remove(id) {
                let value = match request.kind {
                    Kind::Permission => json!({"outcome":{"outcome":"cancelled"}}),
                    Kind::Question { .. } => json!({"action":"cancel"}),
                };
                let _ = request.responder.respond(value);
            }
        }
    }
    pub fn cancel_all(&mut self) {
        let ids = self.live.keys().copied().collect::<Vec<_>>();
        self.cancel(&ids);
    }
    pub fn withdrawn(&mut self, mapper: &Mapper) -> Result<(), BusinessError> {
        let ids = self
            .live
            .iter()
            .filter(|(_, r)| r.responder.cancellation().is_cancelled())
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in &ids {
            let request = &self.live[id];
            mapper.emit(
                mapper
                    .session_scope()
                    .with_run(request.context.run.run_id)
                    .with_request(*id),
                Event::RequestInvalidated {
                    reason: RequestInvalidationReason::AgentWithdrawn,
                },
            )?;
        }
        self.cancel(&ids);
        Ok(())
    }
    pub fn permission(
        &mut self,
        mapper: &Mapper,
        context: &RequestContext,
        response: PermissionResponse,
        attempt_id: ResponseAttemptId,
    ) -> Result<(), BusinessError> {
        let value = match response {
            PermissionResponse::Selected { option_id } => {
                json!({"outcome":{"outcome":"selected","optionId":option_id.as_str()}})
            }
            PermissionResponse::Cancelled => json!({"outcome":{"outcome":"cancelled"}}),
        };
        let delivery = self.deliver(context, value, false);
        mapper.emit(
            mapper
                .session_scope()
                .with_run(context.run.run_id)
                .with_request(context.id),
            Event::PermissionDelivery {
                attempt_id,
                delivery,
            },
        )
    }
    pub fn question(
        &mut self,
        mapper: &Mapper,
        context: &RequestContext,
        response: QuestionResponse,
        attempt_id: ResponseAttemptId,
    ) -> Result<(), BusinessError> {
        let value = match response {
            QuestionResponse::Declined => json!({"action":"decline"}),
            QuestionResponse::Cancelled => json!({"action":"cancel"}),
            QuestionResponse::Answered { answers } => {
                let Some(live) = self.live.get(&context.id) else {
                    return self.missing_question(mapper, context, attempt_id);
                };
                let Kind::Question { fields } = &live.kind else {
                    return self.missing_question(mapper, context, attempt_id);
                };
                if fields.iter().any(|f| f.is_secret) {
                    return mapper.emit(mapper.session_scope().with_run(context.run.run_id).with_request(context.id),Event::QuestionDelivery {attempt_id,delivery:ResponseDelivery::RejectedLocally {reason:"secret values require the non-persistent authentication interface".into()}});
                }
                let mut content = serde_json::Map::new();
                for answer in answers {
                    let value = match answer.value {
                        QuestionAnswerValue::Text { text } => Value::String(text),
                        QuestionAnswerValue::Choices { option_ids, custom } => {
                            let mut choices = option_ids
                                .iter()
                                .map(|id| Value::String(id.as_str().into()))
                                .collect::<Vec<_>>();
                            if let Some(custom) = custom {
                                choices.push(Value::String(custom));
                            }
                            if fields.iter().any(|f| {
                                f.id == answer.field_id
                                    && matches!(f.kind, QuestionFieldKind::SingleChoice { .. })
                            }) {
                                choices
                                    .into_iter()
                                    .next()
                                    .unwrap_or(Value::String(String::new()))
                            } else {
                                Value::Array(choices)
                            }
                        }
                    };
                    content.insert(answer.field_id.as_str().into(), value);
                }
                json!({"action":"accept","content":content})
            }
        };
        let delivery = self.deliver(context, value, true);
        mapper.emit(
            mapper
                .session_scope()
                .with_run(context.run.run_id)
                .with_request(context.id),
            Event::QuestionDelivery {
                attempt_id,
                delivery,
            },
        )
    }
    fn missing_question(
        &self,
        mapper: &Mapper,
        context: &RequestContext,
        attempt_id: ResponseAttemptId,
    ) -> Result<(), BusinessError> {
        mapper.emit(
            mapper
                .session_scope()
                .with_run(context.run.run_id)
                .with_request(context.id),
            Event::QuestionDelivery {
                attempt_id,
                delivery: ResponseDelivery::Unknown {
                    reason: "original live question responder is unavailable".into(),
                },
            },
        )
    }
    fn deliver(
        &mut self,
        context: &RequestContext,
        value: Value,
        question: bool,
    ) -> ResponseDelivery {
        let Some(request) = self.live.get(&context.id) else {
            return ResponseDelivery::Unknown {
                reason: "original live responder is unavailable".into(),
            };
        };
        if request.context != *context
            || matches!(request.kind, Kind::Question { .. }) != question
            || request.responder.cancellation().is_cancelled()
        {
            return ResponseDelivery::Unknown {
                reason: "original responder is expired or belongs to another scope".into(),
            };
        }
        let request = self.live.remove(&context.id).unwrap();
        match request.responder.respond(value) {
            Ok(()) => ResponseDelivery::Submitted,
            Err(_) => ResponseDelivery::Unknown {
                reason: "response delivery to the SDK failed".into(),
            },
        }
    }
}

fn fields(schema: &Value) -> Vec<QuestionField> {
    let mut fields = Vec::new();
    for (key, value) in schema["properties"].as_object().into_iter().flatten() {
        let unsupported = value.get("pattern").is_some()
            || value.get("format").is_some()
            || value.get("minLength").is_some()
            || value.get("maxLength").is_some()
            || value.get("minItems").is_some()
            || value.get("maxItems").is_some();
        let options = choices(value);
        let kind = match value["type"].as_str() {
            Some("string") if !unsupported && !options.is_empty() => {
                QuestionFieldKind::SingleChoice {
                    options,
                    allows_custom: false,
                }
            }
            Some("string") if !unsupported => QuestionFieldKind::Text { multiline: false },
            Some("array")
                if !unsupported
                    && value["items"]["type"] == "string"
                    && !choices(&value["items"]).is_empty() =>
            {
                QuestionFieldKind::MultipleChoice {
                    options: choices(&value["items"]),
                    allows_custom: false,
                }
            }
            _ => QuestionFieldKind::Unsupported {
                type_name: value["type"].as_str().unwrap_or("unknown").into(),
                schema: value.clone(),
            },
        };
        let mut field = QuestionField::new(
            AgentFieldId::new(key),
            value["title"].as_str().unwrap_or(key),
            kind,
        );
        field.required = schema["required"]
            .as_array()
            .is_some_and(|required| required.iter().any(|item| item.as_str() == Some(key)));
        field.is_secret = value["_meta"]["codex"]["isSecret"]
            .as_bool()
            .unwrap_or(false);
        fields.push(field);
    }
    for required in schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !fields.iter().any(|f| f.id.as_str() == required) {
            fields.push(QuestionField::new(
                AgentFieldId::new(required),
                required,
                QuestionFieldKind::Unsupported {
                    type_name: "missing_required_schema".into(),
                    schema: Value::Null,
                },
            ));
        }
    }
    fields
}
fn choices(value: &Value) -> Vec<CapabilityOption> {
    if let Some(options) = value["oneOf"].as_array() {
        return options
            .iter()
            .filter_map(|option| {
                option["const"].as_str().map(|id| {
                    CapabilityOption::new(
                        AgentOptionId::new(id),
                        option["title"].as_str().unwrap_or(id),
                    )
                })
            })
            .collect();
    }
    value["enum"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(ix, item)| {
            item.as_str().map(|id| {
                CapabilityOption::new(
                    AgentOptionId::new(id),
                    value["enumNames"][ix].as_str().unwrap_or(id),
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn constraints_and_required_unknown_fields_do_not_become_valid_text() {
        let parsed = fields(
            &json!({"type":"object","required":["number","missing"],"properties":{"number":{"type":"integer"},"bounded":{"type":"string","maxLength":3},"secret":{"type":"string","_meta":{"codex":{"isSecret":true}}},"choice":{"type":"string","oneOf":[{"const":"actual-value","title":"visible label"}]}}}),
        );
        assert!(
            parsed
                .iter()
                .filter(|field| ["number", "bounded", "missing"].contains(&field.id.as_str()))
                .all(|field| matches!(field.kind, QuestionFieldKind::Unsupported { .. }))
        );
        assert!(
            parsed
                .iter()
                .any(|field| field.id.as_str() == "secret" && field.is_secret)
        );
        assert!(parsed.iter().any(|field|matches!(&field.kind,QuestionFieldKind::SingleChoice {options,..} if options[0].id.as_str()=="actual-value")));
    }
}

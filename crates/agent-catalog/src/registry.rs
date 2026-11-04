use crate::*;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::Path, time::Duration};
use tokio::fs;
use workbench_core::{RequestId, Timestamp};

const MAX_BYTES: usize = 4 * 1024 * 1024;
#[derive(Clone, Default)]
pub(crate) struct Validators {
    etag: Option<String>,
    modified: Option<String>,
}
pub(crate) struct Response {
    status: u16,
    etag: Option<String>,
    modified: Option<String>,
    max_age: u64,
    body: Vec<u8>,
}
pub(crate) trait RegistryTransport: Send + Sync {
    fn fetch(&self, validators: Validators) -> BoxFuture<'_, Result<Response, CatalogError>>;
}
pub(crate) struct HttpRegistry(pub reqwest::Client);
impl RegistryTransport for HttpRegistry {
    fn fetch(&self, validators: Validators) -> BoxFuture<'_, Result<Response, CatalogError>> {
        Box::pin(async move {
            let mut request = self
                .0
                .get(OFFICIAL_REGISTRY_URL)
                .header(reqwest::header::ACCEPT, "application/json");
            if let Some(etag) = validators.etag {
                request = request.header(reqwest::header::IF_NONE_MATCH, etag);
            }
            if let Some(modified) = validators.modified {
                request = request.header(reqwest::header::IF_MODIFIED_SINCE, modified);
            }
            let mut response = request.send().await.map_err(|e| {
                failure(
                    if e.is_timeout() {
                        CatalogErrorKind::Timeout
                    } else {
                        CatalogErrorKind::Network
                    },
                    "official Registry request failed",
                    true,
                )
            })?;
            let status = response.status().as_u16();
            if status != 200 && status != 304 {
                return Err(failure(
                    CatalogErrorKind::Network,
                    format!("official Registry returned HTTP {status}"),
                    true,
                ));
            }
            let header = |key: reqwest::header::HeaderName| {
                response
                    .headers()
                    .get(key)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            };
            let etag = header(reqwest::header::ETAG);
            let modified = header(reqwest::header::LAST_MODIFIED);
            let max_age = header(reqwest::header::CACHE_CONTROL)
                .as_deref()
                .and_then(|value| {
                    value
                        .split(',')
                        .find_map(|part| part.trim().strip_prefix("max-age=")?.parse::<u64>().ok())
                })
                .unwrap_or(300)
                .min(86400);
            if response
                .content_length()
                .is_some_and(|size| size > MAX_BYTES as u64)
            {
                return Err(failure(
                    CatalogErrorKind::InvalidRegistry,
                    "Registry payload exceeds the supported metadata size",
                    false,
                ));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| {
                failure(
                    CatalogErrorKind::Network,
                    "Registry response was interrupted",
                    true,
                )
            })? {
                if body.len().saturating_add(chunk.len()) > MAX_BYTES {
                    return Err(failure(
                        CatalogErrorKind::InvalidRegistry,
                        "Registry payload exceeds the supported metadata size",
                        false,
                    ));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(Response {
                status,
                etag,
                modified,
                max_age,
                body,
            })
        })
    }
}
#[derive(Serialize, Deserialize)]
struct Cache {
    version: u16,
    source_url: String,
    format_version: String,
    fetched_at: Timestamp,
    checked_at: Timestamp,
    expires_at: Timestamp,
    etag: Option<String>,
    modified: Option<String>,
    payload: Value,
}
#[derive(Clone)]
pub(crate) struct RegistryData {
    pub evidence: RegistryEvidence,
    pub agents: Vec<RegistryAgent>,
}

pub(crate) async fn load(
    transport: &dyn RegistryTransport,
    root: &Path,
    policy: RefreshPolicy,
) -> RegistryData {
    let at = now();
    let path = root.join("agent-catalog").join("registry-v1.json");
    let mut cached = fs::read(&path)
        .await
        .ok()
        .filter(|bytes| bytes.len() <= MAX_BYTES + 65536)
        .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
        .filter(|cache| {
            cache.version == 1
                && cache.source_url == OFFICIAL_REGISTRY_URL
                && parse(&cache.payload).is_ok()
        });
    if policy == RefreshPolicy::PreferCache
        && cached.as_ref().is_some_and(|cache| cache.expires_at >= at)
    {
        return from_cache(cached.as_ref().unwrap(), at, false, None);
    }
    let validators = cached
        .as_ref()
        .map(|cache| Validators {
            etag: cache.etag.clone(),
            modified: cache.modified.clone(),
        })
        .unwrap_or_default();
    let fetched = transport.fetch(validators).await;
    let result: Result<(), CatalogError> = match fetched {
        Ok(response) => {
            if response.status == 304 {
                match cached.as_mut() {
                    Some(cache) => {
                        cache.checked_at = at;
                        cache.expires_at = expires(at, response.max_age);
                        if response.etag.is_some() {
                            cache.etag = response.etag;
                        }
                        if response.modified.is_some() {
                            cache.modified = response.modified;
                        }
                        let problem = save(&path, cache).await.err().map(|e| e.message);
                        let mut result = from_cache(cache, at, false, problem);
                        result.evidence.state = RegistryState::Live;
                        return result;
                    }
                    None => Err(failure(
                        CatalogErrorKind::InvalidRegistry,
                        "Registry returned 304 without a usable local cache",
                        true,
                    )),
                }
            } else {
                match serde_json::from_slice::<Value>(&response.body)
                    .ok()
                    .and_then(|payload| parse(&payload).ok().map(|agents| (payload, agents)))
                {
                    Some((payload, agents)) => {
                        let cache = Cache {
                            version: 1,
                            source_url: OFFICIAL_REGISTRY_URL.into(),
                            format_version: payload["version"].as_str().unwrap().into(),
                            fetched_at: at,
                            checked_at: at,
                            expires_at: expires(at, response.max_age),
                            etag: response.etag,
                            modified: response.modified,
                            payload,
                        };
                        let problem = save(&path, &cache).await.err().map(|e| e.message);
                        return RegistryData {
                            evidence: RegistryEvidence {
                                source_url: OFFICIAL_REGISTRY_URL.into(),
                                format_version: Some(cache.format_version),
                                fetched_at: Some(at),
                                checked_at: at,
                                expires_at: Some(cache.expires_at),
                                state: RegistryState::Live,
                                problem,
                            },
                            agents,
                        };
                    }
                    None => Err(failure(
                        CatalogErrorKind::InvalidRegistry,
                        "official Registry has an unsupported or invalid schema",
                        true,
                    )),
                }
            }
        }
        Err(error) => Err(error),
    };
    let error = result.unwrap_err();
    if let Some(cache) = cached.as_ref() {
        return from_cache(
            cache,
            at,
            matches!(
                error.kind,
                CatalogErrorKind::Network | CatalogErrorKind::Timeout
            ),
            Some(error.message),
        );
    }
    RegistryData {
        evidence: RegistryEvidence {
            source_url: OFFICIAL_REGISTRY_URL.into(),
            format_version: None,
            fetched_at: None,
            checked_at: at,
            expires_at: None,
            state: RegistryState::Unavailable {
                reason: error.message,
            },
            problem: None,
        },
        agents: Vec::new(),
    }
}
fn from_cache(
    cache: &Cache,
    at: Timestamp,
    offline: bool,
    problem: Option<String>,
) -> RegistryData {
    RegistryData {
        evidence: RegistryEvidence {
            source_url: cache.source_url.clone(),
            format_version: Some(cache.format_version.clone()),
            fetched_at: Some(cache.fetched_at),
            checked_at: at,
            expires_at: Some(cache.expires_at),
            state: RegistryState::Cached {
                stale: cache.expires_at < at,
                offline,
            },
            problem,
        },
        agents: parse(&cache.payload).unwrap_or_default(),
    }
}
fn expires(at: Timestamp, max_age: u64) -> Timestamp {
    Timestamp::new(
        at.unix_millis()
            .saturating_add(Duration::from_secs(max_age.min(86400)).as_millis() as i64),
    )
}
async fn save(path: &Path, cache: &Cache) -> Result<(), CatalogError> {
    let parent = path
        .parent()
        .ok_or_else(|| failure(CatalogErrorKind::Cache, "invalid cache directory", false))?;
    fs::create_dir_all(parent).await.map_err(|_| {
        failure(
            CatalogErrorKind::Cache,
            "could not create Registry cache directory",
            true,
        )
    })?;
    let bytes = serde_json::to_vec(cache).map_err(|_| {
        failure(
            CatalogErrorKind::Cache,
            "could not encode Registry cache",
            false,
        )
    })?;
    let temporary = parent.join(format!(".registry-{}.tmp", RequestId::new()));
    fs::write(&temporary, bytes).await.map_err(|_| {
        failure(
            CatalogErrorKind::Cache,
            "could not write Registry cache",
            true,
        )
    })?;
    if fs::rename(&temporary, path).await.is_err() {
        let _ = fs::remove_file(&temporary).await;
        return Err(failure(
            CatalogErrorKind::Cache,
            "could not atomically publish Registry cache",
            true,
        ));
    }
    Ok(())
}
fn parse(payload: &Value) -> Result<Vec<RegistryAgent>, CatalogError> {
    let version = payload["version"]
        .as_str()
        .filter(|version| version.split('.').next() == Some("1"))
        .ok_or_else(|| {
            failure(
                CatalogErrorKind::InvalidRegistry,
                "unsupported Registry format version",
                false,
            )
        })?;
    let _ = version;
    let agents = payload["agents"].as_array().ok_or_else(|| {
        failure(
            CatalogErrorKind::InvalidRegistry,
            "Registry agents must be an array",
            false,
        )
    })?;
    let mut parsed = Vec::new();
    let mut identities = std::collections::BTreeSet::new();
    for agent in agents {
        let (Some(id), Some(name), Some(version)) = (
            agent["id"].as_str(),
            agent["name"].as_str(),
            agent["version"].as_str(),
        ) else {
            return Err(failure(
                CatalogErrorKind::InvalidRegistry,
                "Registry Agent identity/name/version is missing",
                true,
            ));
        };
        if !identities.insert(id.to_owned()) {
            return Err(failure(
                CatalogErrorKind::InvalidRegistry,
                "Registry repeats an Agent identity",
                false,
            ));
        }
        let npx = agent["distribution"]["npx"]["package"].as_str();
        let supported = id == CODEX_REGISTRY_ID
            && version == CODEX_ADAPTER_VERSION
            && npx == Some("@agentclientprotocol/codex-acp@2.5.0")
            && cfg!(target_os = "windows")
            && matches!(std::env::consts::ARCH, "x86_64" | "aarch64");
        let mut windows = Vec::new();
        if supported {
            windows.push("npx".into());
        }
        if let Some(binaries) = agent["distribution"]["binary"].as_object() {
            for platform in binaries
                .keys()
                .filter(|key| key.starts_with("windows-") || key.starts_with("win32-"))
            {
                windows.push(format!("binary:{platform}"));
            }
        }
        let installation = if supported {
            Installability::Supported
        } else {
            Installability::Unavailable {
                reason: if id != CODEX_REGISTRY_ID {
                    "current Runtime only validates Codex ACP; configure other Agents manually"
                        .into()
                } else if version != CODEX_ADAPTER_VERSION {
                    format!(
                        "adapter {version} has not been validated; supported combination is 2.5.0 / Codex 0.166.0"
                    )
                } else {
                    "this distribution or Windows architecture has not been validated".into()
                },
            }
        };
        parsed.push(RegistryAgent {
            registry_id: id.into(),
            name: name.into(),
            version: version.into(),
            description: agent["description"].as_str().map(str::to_owned),
            repository: agent["repository"].as_str().map(str::to_owned),
            license: agent["license"].as_str().map(str::to_owned),
            windows_distributions: windows,
            installation,
        });
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TestDirectory;
    use std::{collections::VecDeque, sync::Mutex};
    struct Transport {
        responses: Mutex<VecDeque<Result<Response, CatalogError>>>,
    }
    impl RegistryTransport for Transport {
        fn fetch(&self, validators: Validators) -> BoxFuture<'_, Result<Response, CatalogError>> {
            let response = self.responses.lock().unwrap().pop_front().unwrap();
            if response
                .as_ref()
                .is_ok_and(|response| response.status == 304)
            {
                assert_eq!(validators.etag.as_deref(), Some("registry-fixture"));
            }
            Box::pin(async move { response })
        }
    }
    #[tokio::test]
    async fn cache_survives_offline_invalid_schema_and_conditional_refresh() {
        let root = TestDirectory::new();
        let payload = serde_json::json!({"version":"1.0.0","agents":[{"id":"codex-acp","name":"Codex","version":"2.5.0","distribution":{"npx":{"package":"@agentclientprotocol/codex-acp@2.5.0"}}}]});
        let transport = Transport {
            responses: Mutex::new(VecDeque::from([
                Ok(Response {
                    status: 200,
                    etag: Some("registry-fixture".into()),
                    modified: None,
                    max_age: 0,
                    body: serde_json::to_vec(&payload).unwrap(),
                }),
                Err(failure(CatalogErrorKind::Network, "offline fixture", true)),
                Ok(Response {
                    status: 200,
                    etag: None,
                    modified: None,
                    max_age: 0,
                    body: br#"{"version":"9.0.0","agents":[]}"#.to_vec(),
                }),
                Ok(Response {
                    status: 304,
                    etag: None,
                    modified: None,
                    max_age: 300,
                    body: Vec::new(),
                }),
            ])),
        };
        let first = load(&transport, root.path(), RefreshPolicy::Revalidate).await;
        assert_eq!(first.evidence.state, RegistryState::Live);
        assert_eq!(first.agents.len(), 1);
        let bytes = fs::read(root.path().join("agent-catalog/registry-v1.json"))
            .await
            .unwrap();
        let offline = load(&transport, root.path(), RefreshPolicy::Revalidate).await;
        assert!(matches!(
            offline.evidence.state,
            RegistryState::Cached { offline: true, .. }
        ));
        assert_eq!(offline.agents, first.agents);
        let invalid = load(&transport, root.path(), RefreshPolicy::Revalidate).await;
        assert!(matches!(
            invalid.evidence.state,
            RegistryState::Cached { offline: false, .. }
        ));
        assert_eq!(invalid.agents, first.agents);
        assert_eq!(
            fs::read(root.path().join("agent-catalog/registry-v1.json"))
                .await
                .unwrap(),
            bytes
        );
        let revalidated = load(&transport, root.path(), RefreshPolicy::Revalidate).await;
        assert_eq!(revalidated.evidence.state, RegistryState::Live);
        assert_eq!(revalidated.evidence.fetched_at, first.evidence.fetched_at);
        assert_eq!(revalidated.agents, first.agents);
        let cached = load(&transport, root.path(), RefreshPolicy::PreferCache).await;
        assert!(matches!(
            cached.evidence.state,
            RegistryState::Cached {
                stale: false,
                offline: false
            }
        ));
    }
}

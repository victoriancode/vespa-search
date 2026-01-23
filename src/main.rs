use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{sse::Event, sse::KeepAlive, sse::Sse, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    collections::HashMap,
    convert::Infallible,
    error::Error,
    path::{Path as StdPath, PathBuf},
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use tokio::{
    fs,
    io::AsyncWriteExt,
    process::Command,
    sync::{broadcast, RwLock},
};
use tokio_stream::wrappers::BroadcastStream;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info, warn};
use uuid::Uuid;

const EMBEDDING_DIM: usize = 768;
const HF_DEFAULT_MODEL: &str = "sentence-transformers/all-mpnet-base-v2";
const HF_DEFAULT_MAX_CHARS: usize = 4000;
const HF_DEFAULT_BASE_URL: &str = "https://router.huggingface.co/hf-inference/models";
const HF_DEFAULT_MAX_RETRIES: usize = 3;
const HF_DEFAULT_BACKOFF_MS: u64 = 500;
const HF_DEFAULT_BACKOFF_MAX_MS: u64 = 8000;
const HF_DEFAULT_SUMMARY_MODEL: &str = "sshleifer/distilbart-cnn-12-6";
const HF_DEFAULT_SUMMARY_MAX_CHARS: usize = 6000;

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RepoRecord {
    id: String,
    repo_url: String,
    owner: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct GitHubRepo {
    name: String,
    default_branch: String,
}

#[derive(Debug, Deserialize)]
struct GitHubRepoState {
    repo_id: String,
    repo_url: String,
    owner: String,
    name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RepoRequest {
    repo_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RepoResponse {
    id: String,
    repo_url: String,
    owner: String,
    name: String,
    path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StatusResponse {
    status: String,
    message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct IngestEvent {
    repo_id: String,
    status: String,
    message: Option<String>,
    timestamp: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct SearchRequest {
    query: String,
    repo_filter: Option<String>,
    search_mode: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SearchResult {
    repo_id: String,
    file_path: String,
    line_start: usize,
    line_end: usize,
    snippet: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct SummaryEntry {
    version: u32,
    created_at: i64,
    summary: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct SummaryStore {
    entries: Vec<SummaryEntry>,
}

impl SummaryStore {
    fn latest(&self) -> Option<&SummaryEntry> {
        self.entries.last()
    }

    fn next_version(&self) -> u32 {
        self.entries.last().map(|entry| entry.version + 1).unwrap_or(1)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct WikiResponse {
    summary: String,
    history: Vec<SummaryEntry>,
}

#[derive(Debug, Serialize)]
struct VespaPut {
    fields: VespaFields,
}

#[derive(Debug, Serialize)]
struct VespaFields {
    repo_id: String,
    repo_url: String,
    repo_name: String,
    repo_owner: String,
    commit_sha: String,
    branch: String,
    file_path: String,
    language: String,
    license_spdx: String,
    chunk_id: String,
    chunk_hash: String,
    line_start: i32,
    line_end: i32,
    symbol_names: Vec<String>,
    content: String,
    content_sha: String,
    embedding: VespaEmbedding,
    last_indexed_at: i64,
}

#[derive(Debug, Serialize)]
struct VespaEmbedding {
    values: Vec<f32>,
}

#[derive(Clone)]
struct AppState {
    registry_path: PathBuf,
    repos_path: PathBuf,
    registry: Arc<RwLock<Vec<RepoRecord>>>,
    status_tx: broadcast::Sender<IngestEvent>,
    github_org: Option<String>,
    github_token: Option<String>,
    huggingface_token: Option<String>,
    huggingface_model: String,
    huggingface_max_chars: usize,
    huggingface_base_url: String,
    huggingface_max_retries: usize,
    huggingface_backoff_ms: u64,
    huggingface_backoff_max_ms: u64,
    huggingface_summary_model: String,
    huggingface_summary_max_chars: usize,
    vespa_endpoint: String,
    vespa_document_endpoint: String,
    vespa_cluster: String,
    vespa_namespace: String,
    vespa_document_type: String,
    http_client: reqwest::Client,
    hf_client: reqwest::Client,
}

#[derive(Error, Debug)]
enum AppError {
    #[error("invalid repo url")]
    InvalidRepoUrl,
    #[error("repo not found")]
    RepoNotFound,
    #[error("config error: {0}")]
    Config(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("vespa request error: {0}")]
    VespaRequest(#[from] reqwest::Error),
    #[error("vespa rejected request: {0}")]
    VespaRejected(String),
    #[error("github error: {0}")]
    GitHub(String),
    #[error("huggingface error: {0}")]
    HuggingFace(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let status = match self {
            AppError::InvalidRepoUrl => StatusCode::BAD_REQUEST,
            AppError::RepoNotFound => StatusCode::NOT_FOUND,
            AppError::Config(_) | AppError::Io(_) | AppError::Serde(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            AppError::VespaRequest(_)
            | AppError::VespaRejected(_)
            | AppError::GitHub(_)
            | AppError::HuggingFace(_) => {
                StatusCode::BAD_GATEWAY
            }
        };
        let body = Json(serde_json::json!({"error": self.to_string()}));
        (status, body).into_response()
    }
}

fn normalize_pem(value: &str) -> String {
    value.replace("\\n", "\n")
}

fn read_pem_from_path(path: &PathBuf, label: &str) -> Result<String, AppError> {
    std::fs::read_to_string(path).map_err(|err| {
        AppError::Config(format!(
            "failed to read {label} at {}: {err}",
            path.display()
        ))
    })
}

fn load_pem_from_env_or_path(
    value_env: &str,
    path_env: &str,
    default_path: Option<PathBuf>,
    label: &str,
) -> Result<(Option<String>, String), AppError> {
    if let Ok(value) = std::env::var(value_env) {
        if value.contains("-----BEGIN") {
            return Ok((Some(value), value_env.to_string()));
        }
        let path = PathBuf::from(value);
        return Ok((
            Some(read_pem_from_path(&path, label)?),
            value_env.to_string(),
        ));
    }

    if let Ok(path) = std::env::var(path_env) {
        return Ok((
            Some(read_pem_from_path(&PathBuf::from(path), label)?),
            path_env.to_string(),
        ));
    }

    if let Some(path) = default_path {
        return Ok((
            Some(read_pem_from_path(&path, label)?),
            "default path".into(),
        ));
    }

    Ok((None, "missing".into()))
}

fn build_http_client() -> Result<reqwest::Client, AppError> {
    let ca_default = PathBuf::from("vespa/application/security/clients.pem");
    let (ca_cert, ca_source) = load_pem_from_env_or_path(
        "VESPA_CA_CERT",
        "VESPA_CA_CERT_PATH",
        Some(ca_default),
        "Vespa CA cert",
    )?;
    let ca_cert = ca_cert.ok_or_else(|| AppError::Config("missing Vespa CA cert".into()))?;

    let (cert, cert_source) = load_pem_from_env_or_path(
        "VESPA_CLIENT_CERT",
        "VESPA_CLIENT_CERT_PATH",
        Some(PathBuf::from("vespa/application/security/client.pem")),
        "Vespa client cert",
    )?;
    let (key, key_source) = load_pem_from_env_or_path(
        "VESPA_CLIENT_KEY",
        "VESPA_CLIENT_KEY_PATH",
        Some(PathBuf::from("vespa/application/security/client.key")),
        "Vespa client key",
    )?;

    let mut builder = reqwest::Client::builder();

    info!(
        "vespa tls sources: ca={}, cert={}, key={}",
        ca_source, cert_source, key_source
    );

    let ca_cert = normalize_pem(&ca_cert);
    let ca = reqwest::Certificate::from_pem(ca_cert.as_bytes())
        .map_err(|err| AppError::Config(format!("invalid Vespa CA cert: {err}")))?;
    builder = builder.add_root_certificate(ca);

    match (cert, key) {
        (None, None) => builder
            .build()
            .map_err(|err| AppError::Config(format!("failed to build HTTP client: {err:?}"))),
        (Some(cert), Some(key)) => {
            let cert = normalize_pem(&cert);
            let key = normalize_pem(&key);
            let identity = reqwest::Identity::from_pkcs8_pem(cert.as_bytes(), key.as_bytes())
                .map_err(|err| AppError::Config(format!("invalid Vespa client cert/key: {err}")))?;
            builder
                .identity(identity)
                .build()
                .map_err(|err| AppError::Config(format!("failed to build HTTP client: {err:?}")))
        }
        _ => Err(AppError::Config(
            "both VESPA_CLIENT_CERT and VESPA_CLIENT_KEY must be set for mTLS".into(),
        )),
    }
}

fn build_hf_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|err| AppError::Config(format!("failed to build HuggingFace client: {err}")))
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let base_path = std::env::current_dir()?;
    let data_root = std::env::var("DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let default_data = PathBuf::from("/data");
            if default_data.is_dir() {
                default_data
            } else {
                base_path.clone()
            }
        });
    let registry_path = data_root.join("data/registry.json");
    let repos_path = data_root.join("repos");
    let vespa_endpoint = std::env::var("VESPA_ENDPOINT").unwrap_or_default();
    let vespa_document_endpoint =
        std::env::var("VESPA_DOCUMENT_ENDPOINT").unwrap_or_else(|_| vespa_endpoint.clone());
    let vespa_cluster =
        std::env::var("VESPA_CLUSTER").unwrap_or_else(|_| "codesearch".into());
    let vespa_namespace = std::env::var("VESPA_NAMESPACE").unwrap_or_else(|_| "codesearch".into());
    let vespa_document_type =
        std::env::var("VESPA_DOCUMENT_TYPE").unwrap_or_else(|_| "codesearch".into());
    let github_org = std::env::var("GITHUB_ORG").ok();
    let github_token = std::env::var("GITHUB_TOKEN").ok();
    let huggingface_token = std::env::var("HUGGINGFACE_TOKEN")
        .or_else(|_| std::env::var("HF_API_TOKEN"))
        .ok();
    let huggingface_model =
        std::env::var("HUGGINGFACE_EMBEDDING_MODEL").unwrap_or_else(|_| HF_DEFAULT_MODEL.into());
    let huggingface_max_chars = std::env::var("HUGGINGFACE_EMBEDDING_MAX_CHARS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(HF_DEFAULT_MAX_CHARS);
    let huggingface_base_url = std::env::var("HUGGINGFACE_EMBEDDING_BASE_URL")
        .unwrap_or_else(|_| HF_DEFAULT_BASE_URL.into());
    let huggingface_max_retries = std::env::var("HUGGINGFACE_EMBEDDING_MAX_RETRIES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(HF_DEFAULT_MAX_RETRIES);
    let huggingface_backoff_ms = std::env::var("HUGGINGFACE_EMBEDDING_BACKOFF_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(HF_DEFAULT_BACKOFF_MS);
    let huggingface_backoff_max_ms = std::env::var("HUGGINGFACE_EMBEDDING_BACKOFF_MAX_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(HF_DEFAULT_BACKOFF_MAX_MS);
    let huggingface_summary_model =
        std::env::var("HUGGINGFACE_SUMMARY_MODEL").unwrap_or_else(|_| HF_DEFAULT_SUMMARY_MODEL.into());
    let huggingface_summary_max_chars = std::env::var("HUGGINGFACE_SUMMARY_MAX_CHARS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(HF_DEFAULT_SUMMARY_MAX_CHARS);

    fs::create_dir_all(registry_path.parent().unwrap()).await?;
    fs::create_dir_all(&repos_path).await?;

    let registry = load_registry(&registry_path).await.unwrap_or_default();
    let (status_tx, _status_rx) = broadcast::channel(200);

    let state = AppState {
        registry_path,
        repos_path,
        registry: Arc::new(RwLock::new(registry)),
        status_tx,
        github_org,
        github_token,
        huggingface_token,
        huggingface_model,
        huggingface_max_chars,
        huggingface_base_url,
        huggingface_max_retries,
        huggingface_backoff_ms,
        huggingface_backoff_max_ms,
        huggingface_summary_model,
        huggingface_summary_max_chars,
        vespa_endpoint,
        vespa_document_endpoint,
        vespa_cluster,
        vespa_namespace,
        vespa_document_type,
        http_client: build_http_client()?,
        hf_client: build_hf_client()?,
    };

    if let Err(err) = sync_registry_from_github(&state).await {
        warn!("failed to bootstrap registry from GitHub: {err}");
    }

    let app = Router::new()
        .route("/repos", post(create_repo).get(list_repos))
        .route("/repos/:id/index", post(index_repo))
        .route("/repos/:id/status", get(repo_status))
        .route("/repos/:id/events", get(repo_events))
        .route("/repos/:id/wiki", get(repo_wiki))
        .route("/repos/:id/wiki/summary", post(update_repo_summary))
        .route("/search", post(search))
        .with_state(state)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );

    let port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3001);
    let listen_address = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&listen_address).await?;
    info!("backend listening on {}", listen_address);
    axum::serve(listener, app).await.map_err(AppError::Io)?;
    Ok(())
}

async fn create_repo(
    State(state): State<AppState>,
    Json(payload): Json<RepoRequest>,
) -> Result<Json<RepoResponse>, AppError> {
    let (owner, name) = parse_repo_url(&payload.repo_url)?;
    let id = Uuid::new_v4().to_string();

    let record = RepoRecord {
        id: id.clone(),
        repo_url: payload.repo_url.clone(),
        owner: owner.clone(),
        name: name.clone(),
    };

    {
        let mut registry = state.registry.write().await;
        registry.push(record.clone());
        save_registry(&state.registry_path, &registry).await?;
    }

    let repo_path = state.repos_path.join(&owner).join(&name);

    Ok(Json(RepoResponse {
        id,
        repo_url: payload.repo_url,
        owner,
        name,
        path: repo_path.to_string_lossy().to_string(),
    }))
}

async fn list_repos(State(state): State<AppState>) -> Result<Json<Vec<RepoRecord>>, AppError> {
    let registry = state.registry.read().await;
    Ok(Json(registry.clone()))
}

async fn index_repo(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<StatusResponse>, AppError> {
    let record = find_repo_by_id(&state, &id).await?;

    let repo_path = state.repos_path.join(&record.owner).join(&record.name);
    let vv_path = repo_path.join("vv");

    write_status(
        &state,
        &vv_path,
        &record.id,
        "in_progress",
        Some("Ingestion queued".into()),
    )
    .await?;
    let state_clone = state.clone();
    let record_clone = record.clone();
    let repo_path_clone = repo_path.clone();
    let vv_path_clone = vv_path.clone();
    tokio::spawn(async move {
        let state_for_ingest = state_clone.clone();
        let vv_path_for_ingest = vv_path_clone.clone();
        if let Err(err) =
            ingest_repo(state_for_ingest, record_clone, repo_path_clone, vv_path_for_ingest).await
        {
            error!("ingestion failed for repo {}: {}", record.id, err);
            let _ = write_status(
                &state_clone,
                &vv_path_clone,
                &record.id,
                "error",
                Some(err.to_string()),
            )
            .await;
        }
    });

    Ok(Json(StatusResponse {
        status: "in_progress".into(),
        message: Some("Ingestion started".into()),
    }))
}

async fn repo_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<StatusResponse>, AppError> {
    let record = find_repo_by_id(&state, &id).await?;
    let vv_path = state
        .repos_path
        .join(&record.owner)
        .join(&record.name)
        .join("vv");
    read_status(&vv_path).await
}

async fn repo_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let repo_id = id.clone();
    let stream = BroadcastStream::new(state.status_tx.subscribe()).filter_map(move |result| {
        let repo_id = repo_id.clone();
        async move {
            match result {
                Ok(event) if event.repo_id == repo_id => {
                    let payload = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
                    Some(Ok(Event::default().event("status").data(payload)))
                }
                Ok(_) => None,
                Err(_) => None,
            }
        }
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

async fn repo_wiki(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<WikiResponse>, AppError> {
    let record = find_repo_by_id(&state, &id).await?;
    let vv_path = state
        .repos_path
        .join(&record.owner)
        .join(&record.name)
        .join("vv");

    let store = read_summary_store(&vv_path).await.unwrap_or_default();
    if let Some(latest) = store.latest() {
        let mut history = store.entries.clone();
        history.reverse();
        return Ok(Json(WikiResponse {
            summary: latest.summary.clone(),
            history,
        }));
    }

    let wiki_path = vv_path.join("wiki/index.md");
    let fallback = fs::read_to_string(wiki_path)
        .await
        .unwrap_or_else(|_| "# CodeWiki\n\nWiki content is not yet available.".to_string());
    Ok(Json(WikiResponse {
        summary: fallback,
        history: Vec::new(),
    }))
}

async fn update_repo_summary(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<WikiResponse>, AppError> {
    let record = find_repo_by_id(&state, &id).await?;
    let repo_path = state.repos_path.join(&record.owner).join(&record.name);
    let vv_path = repo_path.join("vv");
    let store = generate_repo_summary(&state, &record, &repo_path, &vv_path).await?;
    let mut history = store.entries.clone();
    history.reverse();
    let summary = store
        .latest()
        .map(|entry| entry.summary.clone())
        .unwrap_or_else(|| "Summary not available.".into());
    Ok(Json(WikiResponse { summary, history }))
}

async fn search(
    State(state): State<AppState>,
    Json(payload): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, AppError> {
    let query = payload.query.trim();
    if query.is_empty() {
        return Ok(Json(SearchResponse { results: vec![] }));
    }

    let search_mode = resolve_search_mode(payload.search_mode.as_deref());
    let yql = build_search_yql(payload.repo_filter.as_deref(), search_mode);
    let search_url = vespa_search_url(&state)?;
    let mut body = serde_json::json!({
        "yql": yql,
        "hits": 10,
        "query": query,
    });

    if let Some(repo_id) = payload
        .repo_filter
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(object) = body.as_object_mut() {
            let quoted = format!("\"{}\"", escape_yql_string(repo_id));
            object.insert("repo_id".to_string(), quoted.into());
        }
    }

    if let Some(profile) = search_mode.profile_name() {
        let query_embedding = VespaEmbedding {
            values: embed_text(&state, query).await?,
        };
        let embedding_value = serde_json::to_value(&query_embedding)?;
        if let Some(object) = body.as_object_mut() {
            object.insert("ranking.profile".to_string(), profile.into());
            object.insert(
                "input.query(query_embedding)".to_string(),
                embedding_value,
            );
        }
    }

    let response = state.http_client.post(search_url).json(&body).send().await?;

    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::VespaRejected(body));
    }

    let body: serde_json::Value = response.json().await?;
    let mut results = Vec::new();
    if let Some(children) = body.pointer("/root/children").and_then(|v| v.as_array()) {
        for child in children {
            let fields = match child.get("fields") {
                Some(fields) => fields,
                None => continue,
            };
            let repo_id = fields
                .get("repo_id")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let file_path = fields
                .get("file_path")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let line_start = fields
                .get("line_start")
                .and_then(|value| value.as_i64())
                .unwrap_or(1)
                .max(1) as usize;
            let line_end = fields
                .get("line_end")
                .and_then(|value| value.as_i64())
                .unwrap_or(line_start as i64)
                .max(1) as usize;
            let content = fields
                .get("content")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let snippet = build_snippet(content);

            results.push(SearchResult {
                repo_id,
                file_path,
                line_start,
                line_end,
                snippet,
            });
        }
    }

    Ok(Json(SearchResponse { results }))
}

async fn load_registry(path: &StdPath) -> Result<Vec<RepoRecord>, AppError> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let contents = fs::read(path).await?;
    let registry = serde_json::from_slice(&contents)?;
    Ok(registry)
}

async fn save_registry(path: &StdPath, registry: &[RepoRecord]) -> Result<(), AppError> {
    let contents = serde_json::to_vec_pretty(registry)?;
    fs::write(path, contents).await?;
    Ok(())
}

async fn list_github_org_repos(state: &AppState, org: &str) -> Result<Vec<GitHubRepo>, AppError> {
    let mut page = 1usize;
    let mut repos = Vec::new();

    loop {
        let url = format!("https://api.github.com/orgs/{org}/repos?per_page=100&page={page}");
        let mut request = state
            .http_client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "vespa-code-search");
        if let Some(token) = state.github_token.as_deref() {
            request = request.header("Authorization", format!("token {token}"));
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::GitHub(format!(
                "failed to list GitHub repos for {org}: {status} {body}"
            )));
        }

        let page_repos: Vec<GitHubRepo> = response.json().await?;
        let page_count = page_repos.len();
        repos.extend(page_repos);
        if page_count < 100 {
            break;
        }
        page += 1;
    }

    Ok(repos)
}

async fn fetch_github_repo_state(
    state: &AppState,
    org: &str,
    repo: &GitHubRepo,
) -> Result<Option<RepoRecord>, AppError> {
    let branch = if repo.default_branch.is_empty() {
        "main"
    } else {
        repo.default_branch.as_str()
    };
    let url = format!(
        "https://raw.githubusercontent.com/{org}/{}/{}/.vv/state.json",
        repo.name, branch
    );
    let mut request = state
        .http_client
        .get(&url)
        .header("User-Agent", "vespa-code-search");
    if let Some(token) = state.github_token.as_deref() {
        request = request.header("Authorization", format!("token {token}"));
    }
    let response = request
        .send()
        .await
        .map_err(|err| AppError::HuggingFace(err.to_string()))?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::GitHub(format!(
            "failed to fetch .vv state from {url}: {status} {body}"
        )));
    }

    let payload = match response.json::<GitHubRepoState>().await {
        Ok(payload) => payload,
        Err(err) => {
            warn!("failed to parse .vv state from {url}: {err}");
            return Ok(None);
        }
    };
    if payload.repo_id.is_empty() {
        return Ok(None);
    }

    Ok(Some(RepoRecord {
        id: payload.repo_id,
        repo_url: payload.repo_url,
        owner: payload.owner,
        name: payload.name,
    }))
}

async fn sync_registry_from_github(state: &AppState) -> Result<usize, AppError> {
    let org = match state.github_org.as_deref() {
        Some(org) => org,
        None => return Ok(0),
    };

    let repos = list_github_org_repos(state, org).await?;
    let mut records = Vec::new();
    for repo in repos {
        if !repo.name.ends_with("-vv-search") {
            continue;
        }
        match fetch_github_repo_state(state, org, &repo).await {
            Ok(Some(record)) => records.push(record),
            Ok(None) => {}
            Err(err) => warn!("failed to read vv state for {}: {}", repo.name, err),
        }
    }

    if records.is_empty() {
        return Ok(0);
    }

    let mut registry = state.registry.write().await;
    let mut index = HashMap::new();
    for (idx, record) in registry.iter().enumerate() {
        index.insert(record.id.clone(), idx);
    }

    let mut changes = 0usize;
    for record in records {
        if let Some(&idx) = index.get(&record.id) {
            let existing = &mut registry[idx];
            if existing.repo_url != record.repo_url
                || existing.owner != record.owner
                || existing.name != record.name
            {
                *existing = record;
                changes += 1;
            }
        } else {
            index.insert(record.id.clone(), registry.len());
            registry.push(record);
            changes += 1;
        }
    }

    if changes > 0 {
        save_registry(&state.registry_path, &registry).await?;
    }

    Ok(changes)
}

async fn find_repo_by_id(state: &AppState, id: &str) -> Result<RepoRecord, AppError> {
    {
        let registry = state.registry.read().await;
        if let Some(record) = registry.iter().find(|repo| repo.id == id) {
            return Ok(record.clone());
        }
    }

    if state.github_org.is_some() {
        if let Err(err) = sync_registry_from_github(state).await {
            warn!("failed to refresh registry from GitHub: {err}");
        }
        let registry = state.registry.read().await;
        if let Some(record) = registry.iter().find(|repo| repo.id == id) {
            return Ok(record.clone());
        }
    }

    Err(AppError::RepoNotFound)
}

async fn write_status(
    state: &AppState,
    vv_path: &StdPath,
    repo_id: &str,
    status: &str,
    message: Option<String>,
) -> Result<(), AppError> {
    fs::create_dir_all(vv_path).await?;
    let payload = StatusResponse {
        status: status.into(),
        message: message.clone(),
    };
    fs::write(
        vv_path.join("status.json"),
        serde_json::to_vec_pretty(&payload)?,
    )
    .await?;
    let _ = state.status_tx.send(IngestEvent {
        repo_id: repo_id.to_string(),
        status: status.to_string(),
        message,
        timestamp: Utc::now().timestamp_millis(),
    });
    Ok(())
}

async fn read_status(vv_path: &StdPath) -> Result<Json<StatusResponse>, AppError> {
    let path = vv_path.join("status.json");
    if fs::metadata(&path).await.is_err() {
        let chunks_path = vv_path.join("chunks.jsonl");
        if let Ok(metadata) = fs::metadata(&chunks_path).await {
            if metadata.len() > 0 {
                return Ok(Json(StatusResponse {
                    status: "complete".into(),
                    message: Some("Ingestion complete (status recovered).".into()),
                }));
            }
        }

        let wiki_path = vv_path.join("wiki/index.md");
        if fs::metadata(&wiki_path).await.is_ok() {
            return Ok(Json(StatusResponse {
                status: "unknown".into(),
                message: Some(
                    "Ingestion artifacts found, but status is unavailable. Re-run ingestion to refresh."
                        .into(),
                ),
            }));
        }

        return Ok(Json(StatusResponse {
            status: "unknown".into(),
            message: Some(
                "Status not available on this instance. Re-run ingestion if needed.".into(),
            ),
        }));
    }

    let data = fs::read(path).await?;
    let mut status: StatusResponse = serde_json::from_slice(&data)?;
    if status.message.is_none() {
        status.message = Some(match status.status.as_str() {
            "complete" => "Ingestion complete.".into(),
            "in_progress" => "Ingestion in progress.".into(),
            "error" => "Ingestion failed. Check backend logs.".into(),
            _ => "Status unavailable. Re-run ingestion if needed.".into(),
        });
    }
    Ok(Json(status))
}

async fn read_summary_store(vv_path: &StdPath) -> Result<SummaryStore, AppError> {
    let summary_path = vv_path.join("wiki/summary.json");
    if fs::metadata(&summary_path).await.is_err() {
        return Ok(SummaryStore::default());
    }
    let data = fs::read(&summary_path).await?;
    let store = serde_json::from_slice::<SummaryStore>(&data)?;
    Ok(store)
}

async fn write_summary_store(vv_path: &StdPath, store: &SummaryStore) -> Result<(), AppError> {
    let summary_path = vv_path.join("wiki/summary.json");
    fs::create_dir_all(summary_path.parent().unwrap()).await?;
    let data = serde_json::to_vec_pretty(store)?;
    fs::write(summary_path, data).await?;
    Ok(())
}

async fn run_git_command(
    cwd: Option<&StdPath>,
    args: &[&str],
) -> Result<std::process::Output, AppError> {
    let mut command = Command::new("git");
    command.env("GIT_TERMINAL_PROMPT", "0");
    if let Some(path) = cwd {
        command.arg("-C").arg(path);
    }
    command.args(args);
    command.output().await.map_err(AppError::Io)
}

async fn ensure_github_repo(
    state: &AppState,
    org: &str,
    token: &str,
    repo_name: &str,
) -> Result<(), AppError> {
    let response = state
        .http_client
        .post(format!("https://api.github.com/orgs/{org}/repos"))
        .header("Authorization", format!("token {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "vespa-code-search")
        .json(&serde_json::json!({
            "name": repo_name,
            "private": false,
        }))
        .send()
        .await?;

    if response.status().is_success() {
        return Ok(());
    }

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if status == StatusCode::UNPROCESSABLE_ENTITY && body.contains("name already exists") {
        return Ok(());
    }

    Err(AppError::GitHub(format!(
        "failed to create GitHub repo {org}/{repo_name}: {status} {body}"
    )))
}

async fn mirror_repo_to_github(
    state: &AppState,
    record: &RepoRecord,
    repo_path: &StdPath,
) -> Result<(), AppError> {
    let org = state.github_org.as_deref().ok_or_else(|| {
        AppError::Config("GITHUB_ORG is required for repo mirroring".into())
    })?;
    let token = state.github_token.as_deref().ok_or_else(|| {
        AppError::Config("GITHUB_TOKEN is required for repo mirroring".into())
    })?;
    let mirror_name = format!("{}-vv-search", record.name);

    ensure_github_repo(state, org, token, &mirror_name).await?;

    let remote_url = format!(
        "https://x-access-token:{}@github.com/{}/{}.git",
        token, org, mirror_name
    );

    let _ = run_git_command(Some(repo_path), &["remote", "remove", "mirror"]).await;
    let output = run_git_command(
        Some(repo_path),
        &["remote", "add", "mirror", &remote_url],
    )
    .await?;
    if !output.status.success() {
        return Err(AppError::GitHub(
            "failed to add mirror remote for GitHub".into(),
        ));
    }

    let output = run_git_command(Some(repo_path), &["push", "--mirror", "mirror"]).await?;
    if !output.status.success() {
        return Err(AppError::GitHub(
            "failed to push mirror to GitHub".into(),
        ));
    }

    Ok(())
}

async fn write_vv_state(repo_path: &StdPath, record: &RepoRecord) -> Result<PathBuf, AppError> {
    let vv_path = repo_path.join(".vv");
    fs::create_dir_all(&vv_path).await?;
    let payload = serde_json::json!({
        "repo_id": record.id,
        "repo_url": record.repo_url,
        "owner": record.owner,
        "name": record.name,
        "mirror_repo": format!("{}-vv-search", record.name),
        "updated_at": Utc::now().to_rfc3339(),
    });
    let state_path = vv_path.join("state.json");
    fs::write(&state_path, serde_json::to_vec_pretty(&payload)?).await?;
    Ok(state_path)
}

async fn commit_vv_state(repo_path: &StdPath, state_path: &StdPath) -> Result<(), AppError> {
    let _ = run_git_command(Some(repo_path), &["config", "user.email", "vv-search@users.noreply.github.com"]).await?;
    let _ = run_git_command(Some(repo_path), &["config", "user.name", "vv-search"]).await?;

    let state_path_str = state_path.to_string_lossy();
    let output = run_git_command(
        Some(repo_path),
        &["add", "-f", state_path_str.as_ref()],
    )
    .await?;
    if !output.status.success() {
        return Err(AppError::GitHub(
            "failed to stage .vv state file".into(),
        ));
    }

    let diff_output = run_git_command(Some(repo_path), &["diff", "--cached", "--quiet"]).await?;
    if diff_output.status.code() == Some(0) {
        return Ok(());
    }
    if diff_output.status.code() != Some(1) {
        return Err(AppError::GitHub(
            "failed to inspect staged changes for .vv state".into(),
        ));
    }

    let output = run_git_command(
        Some(repo_path),
        &["commit", "-m", "chore: update vv state", "--", state_path_str.as_ref()],
    )
    .await?;
    if !output.status.success() {
        return Err(AppError::GitHub(
            "failed to commit .vv state file".into(),
        ));
    }

    Ok(())
}

async fn ingest_repo(
    state: AppState,
    record: RepoRecord,
    repo_path: PathBuf,
    vv_path: PathBuf,
) -> Result<(), AppError> {
    write_status(
        &state,
        &vv_path,
        &record.id,
        "in_progress",
        Some("Cloning repository".into()),
    )
    .await?;

    if repo_path.exists() && !repo_path.join(".git").exists() {
        if is_dir_empty(&repo_path).await? {
            fs::remove_dir(&repo_path).await?;
        } else if dir_contains_only_vv(&repo_path).await? {
            warn!(
                "repo path {} contains only vv artifacts, removing for re-clone",
                repo_path.display()
            );
            fs::remove_dir_all(&vv_path).await.ok();
            if is_dir_empty(&repo_path).await? {
                fs::remove_dir(&repo_path).await?;
            }
        }

        if repo_path.exists() {
            write_status(
                &state,
                &vv_path,
                &record.id,
                "error",
                Some("Repo path exists but is not a git repository".into()),
            )
            .await?;
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "repo path exists but is not a git repository",
            )));
        }
    }

    if !repo_path.exists() {
        fs::create_dir_all(repo_path.parent().unwrap()).await?;
        let repo_path_str = repo_path.to_string_lossy();
        let output = run_git_command(
            None,
            &["clone", &record.repo_url, repo_path_str.as_ref()],
        )
        .await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = format!("Git clone failed: {}", stderr.trim());
            write_status(&state, &vv_path, &record.id, "error", Some(message)).await?;
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "git clone failed",
            )));
        }
    }

    let vv_state_path = write_vv_state(&repo_path, &record).await?;
    commit_vv_state(&repo_path, &vv_state_path).await?;

    write_status(
        &state,
        &vv_path,
        &record.id,
        "mirroring",
        Some("Mirroring repository to GitHub".into()),
    )
    .await?;
    mirror_repo_to_github(&state, &record, &repo_path).await?;

    fs::create_dir_all(&vv_path).await?;
    fs::create_dir_all(vv_path.join("vectors")).await?;
    fs::create_dir_all(vv_path.join("wiki")).await?;

    let manifest = serde_json::json!({
        "repo_url": record.repo_url,
        "owner": record.owner,
        "name": record.name,
        "indexed_at": Utc::now().to_rfc3339(),
    });
    fs::write(
        vv_path.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )
    .await?;
    fs::write(vv_path.join("chunks.jsonl"), "").await?;

    let wiki_content = format!(
        "# CodeWiki for {}/{}\n\nThis is a placeholder wiki generated during ingestion.\n",
        record.owner, record.name
    );
    fs::write(vv_path.join("wiki/index.md"), wiki_content).await?;

    write_status(
        &state,
        &vv_path,
        &record.id,
        "indexing",
        Some("Feeding documents to Vespa".into()),
    )
    .await?;
    let indexed = feed_repo_to_vespa(&state, &record, &repo_path, &vv_path).await?;
    info!(
        "vespa feed completed for repo {} ({} documents)",
        record.id, indexed
    );

    write_status(
        &state,
        &vv_path,
        &record.id,
        "summarizing",
        Some("Generating repository summary".into()),
    )
    .await?;
    if let Err(err) = generate_repo_summary(&state, &record, &repo_path, &vv_path).await {
        warn!(
            "failed to generate summary for repo {}: {}",
            record.id, err
        );
    }

    write_status(
        &state,
        &vv_path,
        &record.id,
        "complete",
        Some("Ingestion complete".into()),
    )
    .await?;

    Ok(())
}

async fn feed_repo_to_vespa(
    state: &AppState,
    record: &RepoRecord,
    repo_path: &StdPath,
    vv_path: &StdPath,
) -> Result<usize, AppError> {
    const MAX_CONTENT_BYTES: usize = 200_000;

    let files = list_repo_files(repo_path).await?;
    let mut indexed = 0usize;

    let chunks_path = vv_path.join("chunks.jsonl");
    let mut chunks_file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&chunks_path)
        .await?;

    for file_path in files {
        let absolute_path = repo_path.join(&file_path);
        let content_bytes = match fs::read(&absolute_path).await {
            Ok(bytes) => bytes,
            Err(err) => {
                error!(
                    "skipping file {} due to read error: {}",
                    file_path.display(),
                    err
                );
                continue;
            }
        };

        if content_bytes.is_empty()
            || content_bytes.len() > MAX_CONTENT_BYTES
            || content_bytes.iter().any(|byte| *byte == 0)
        {
            continue;
        }

        let content_lossy = String::from_utf8_lossy(&content_bytes);
        let content = sanitize_vespa_content(&content_lossy);
        if content.trim().is_empty() {
            continue;
        }
        let line_end = content.lines().count().max(1) as i32;
        let content_sha = sha256_hex(content.as_bytes());
        let chunk_id = sha256_hex(format!("{}:{}", record.id, file_path.display()).as_bytes());
        let chunk_hash = sha256_hex(content.as_bytes());
        let language = guess_language(&file_path);
        let last_indexed_at = Utc::now().timestamp_millis();
        let chunk_id_for_chunk = chunk_id.clone();
        let content_sha_for_chunk = content_sha.clone();
        let embedding_values =
            embed_content_with_cache(state, vv_path, &content, &content_sha).await?;

        let doc_id = format!("{}-{}", record.id, chunk_id);
        let put = VespaPut {
            fields: VespaFields {
                repo_id: record.id.clone(),
                repo_url: record.repo_url.clone(),
                repo_name: record.name.clone(),
                repo_owner: record.owner.clone(),
                commit_sha: "unknown".to_string(),
                branch: "main".to_string(),
                file_path: file_path.to_string_lossy().to_string(),
                language,
                license_spdx: "unknown".to_string(),
                chunk_id,
                chunk_hash,
                line_start: 1,
                line_end,
                symbol_names: Vec::new(),
                content,
                content_sha,
                embedding: VespaEmbedding {
                    values: embedding_values,
                },
                last_indexed_at,
            },
        };
        let body_bytes = serde_json::to_vec(&put)?;
        let document_url = vespa_document_url(state, &doc_id)?;
        let response = state
            .http_client
            .post(document_url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body_bytes.clone())
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let preview_len = body_bytes.len().min(1024);
            let preview = String::from_utf8_lossy(&body_bytes[..preview_len]);
            let response_preview: String = body.chars().take(1024).collect();
            error!(
                "vespa feed rejected (status {}), request preview: {}, response: {}",
                status, preview, response_preview
            );
            return Err(AppError::VespaRejected(body));
        }

        let chunk_entry = serde_json::json!({
            "repo_id": record.id.clone(),
            "file_path": file_path.to_string_lossy(),
            "chunk_id": chunk_id_for_chunk,
            "line_start": 1,
            "line_end": line_end,
            "content_sha": content_sha_for_chunk,
        });
        let serialized = serde_json::to_string(&chunk_entry)?;
        chunks_file.write_all(serialized.as_bytes()).await?;
        chunks_file.write_all(b"\n").await?;
        indexed += 1;
    }

    Ok(indexed)
}

async fn is_dir_empty(path: &StdPath) -> Result<bool, AppError> {
    let mut entries = fs::read_dir(path).await?;
    Ok(entries.next_entry().await?.is_none())
}

async fn dir_contains_only_vv(path: &StdPath) -> Result<bool, AppError> {
    let mut entries = fs::read_dir(path).await?;
    let mut saw_entry = false;
    while let Some(entry) = entries.next_entry().await? {
        saw_entry = true;
        let name = entry.file_name();
        if name != "vv" && name != ".vv" {
            return Ok(false);
        }
    }
    Ok(saw_entry)
}

async fn list_repo_files(repo_path: &StdPath) -> Result<Vec<PathBuf>, AppError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("ls-files")
        .output()
        .await;

    if let Ok(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let files = stdout
                .lines()
                .filter(|line| *line != ".vv" && !line.starts_with(".vv/"))
                .map(PathBuf::from)
                .collect();
            return Ok(files);
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(
            "git ls-files failed for {}: {}",
            repo_path.display(),
            stderr.trim()
        );
    } else if let Err(err) = output {
        warn!("git ls-files failed for {}: {}", repo_path.display(), err);
    }

    walk_repo_files(repo_path).await
}

async fn walk_repo_files(repo_path: &StdPath) -> Result<Vec<PathBuf>, AppError> {
    let mut files = Vec::new();
    let mut stack = vec![repo_path.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let mut entries = fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;
            let name = entry.file_name();
            let name = name.to_string_lossy();

            if file_type.is_dir() {
                if should_skip_dir(&name) {
                    continue;
                }
                stack.push(path);
                continue;
            }

            if file_type.is_file() {
                let relative = path.strip_prefix(repo_path).unwrap_or(&path);
                if !relative.as_os_str().is_empty() {
                    files.push(relative.to_path_buf());
                }
            }
        }
    }

    Ok(files)
}

fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "vv"
            | ".vv"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".next"
            | ".venv"
            | "venv"
            | "__pycache__"
    )
}

fn vespa_document_url(state: &AppState, doc_id: &str) -> Result<String, AppError> {
    if state.vespa_document_endpoint.trim().is_empty() {
        return Err(AppError::Config(
            "VESPA_DOCUMENT_ENDPOINT or VESPA_ENDPOINT must be set".into(),
        ));
    }
    Ok(format!(
        "{}/document/v1/{}/{}/docid/{}",
        state.vespa_document_endpoint.trim_end_matches('/'),
        state.vespa_namespace,
        state.vespa_document_type,
        urlencoding::encode(doc_id)
    ))
}

fn vespa_search_url(state: &AppState) -> Result<String, AppError> {
    if state.vespa_endpoint.trim().is_empty() {
        return Err(AppError::Config(
            "VESPA_ENDPOINT must be set".into(),
        ));
    }
    Ok(format!(
        "{}/search/",
        state.vespa_endpoint.trim_end_matches('/')
    ))
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn sanitize_vespa_content(input: &str) -> String {
    input
        .chars()
        .filter(|ch| match ch {
            '\n' | '\r' | '\t' => true,
            _ => !ch.is_control(),
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
enum SearchMode {
    Hybrid,
    Semantic,
    Bm25,
}

impl SearchMode {
    fn profile_name(self) -> Option<&'static str> {
        match self {
            SearchMode::Hybrid => Some("hybrid"),
            SearchMode::Semantic => Some("semantic"),
            SearchMode::Bm25 => None,
        }
    }
}

fn resolve_search_mode(value: Option<&str>) -> SearchMode {
    let mode = value.unwrap_or("bm25").trim().to_lowercase();
    match mode.as_str() {
        "semantic" => SearchMode::Semantic,
        "bm25" => SearchMode::Bm25,
        _ => SearchMode::Hybrid,
    }
}

fn truncate_for_embedding<'a>(input: &'a str, max_chars: usize) -> Cow<'a, str> {
    if input.chars().count() <= max_chars {
        return Cow::Borrowed(input);
    }
    Cow::Owned(input.chars().take(max_chars).collect())
}

fn truncate_for_summary<'a>(input: &'a str, max_chars: usize) -> Cow<'a, str> {
    truncate_for_embedding(input, max_chars)
}

fn normalize_embedding(mut values: Vec<f32>) -> Vec<f32> {
    if values.len() == EMBEDDING_DIM {
        return values;
    }
    warn!(
        "embedding dimension mismatch: got {}, expected {}",
        values.len(),
        EMBEDDING_DIM
    );
    if values.len() > EMBEDDING_DIM {
        values.truncate(EMBEDDING_DIM);
    } else {
        values.resize(EMBEDDING_DIM, 0.0);
    }
    values
}

fn parse_hf_embedding(value: serde_json::Value) -> Result<Vec<f32>, AppError> {
    match value {
        serde_json::Value::Array(values) => {
            if values.is_empty() {
                return Err(AppError::HuggingFace(
                    "empty embedding response".into(),
                ));
            }
            if values[0].is_number() {
                let mut embedding = Vec::with_capacity(values.len());
                for value in values {
                    let number = value.as_f64().ok_or_else(|| {
                        AppError::HuggingFace("invalid embedding value".into())
                    })?;
                    embedding.push(number as f32);
                }
                return Ok(embedding);
            }

            if values[0].is_array() {
                let mut summed: Vec<f32> = Vec::new();
                let mut count = 0usize;
                for row in values {
                    let row_values = row.as_array().ok_or_else(|| {
                        AppError::HuggingFace("invalid embedding row".into())
                    })?;
                    if summed.is_empty() {
                        summed = vec![0.0; row_values.len()];
                    }
                    for (index, value) in row_values.iter().enumerate() {
                        let number = value.as_f64().ok_or_else(|| {
                            AppError::HuggingFace("invalid embedding value".into())
                        })?;
                        if index < summed.len() {
                            summed[index] += number as f32;
                        }
                    }
                    count += 1;
                }
                if count == 0 {
                    return Err(AppError::HuggingFace(
                        "empty embedding response".into(),
                    ));
                }
                for value in &mut summed {
                    *value /= count as f32;
                }
                return Ok(summed);
            }

            Err(AppError::HuggingFace(
                "unsupported embedding response format".into(),
            ))
        }
        serde_json::Value::Object(map) => {
            if let Some(error) = map.get("error").and_then(|value| value.as_str()) {
                return Err(AppError::HuggingFace(error.to_string()));
            }
            Err(AppError::HuggingFace(
                "unexpected embedding response".into(),
            ))
        }
        _ => Err(AppError::HuggingFace(
            "unexpected embedding response".into(),
        )),
    }
}

async fn fetch_hf_embedding(state: &AppState, text: &str) -> Result<Vec<f32>, AppError> {
    let base_url = state.huggingface_base_url.trim_end_matches('/');
    let url = format!(
        "{}/{}/pipeline/feature-extraction",
        base_url, state.huggingface_model
    );
    let payload = serde_json::json!({
        "inputs": text,
        "options": { "wait_for_model": true }
    });

    let max_retries = state.huggingface_max_retries;
    let mut backoff = Duration::from_millis(state.huggingface_backoff_ms);
    let backoff_max = Duration::from_millis(state.huggingface_backoff_max_ms);

    for attempt in 0..=max_retries {
        let mut request = state.hf_client.post(&url).json(&payload);
        if let Some(token) = state.huggingface_token.as_deref() {
            request = request.bearer_auth(token);
        }

        match request.send().await {
            Ok(response) => {
                if response.status().is_success() {
                    let value: serde_json::Value = response
                        .json()
                        .await
                        .map_err(|err| AppError::HuggingFace(err.to_string()))?;
                    let embedding = parse_hf_embedding(value)?;
                    return Ok(normalize_embedding(embedding));
                }

                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                if attempt < max_retries && should_retry_status(status) {
                    warn!(
                        "huggingface embedding request failed with {status}; retrying in {:?} (attempt {}/{})",
                        backoff,
                        attempt + 1,
                        max_retries
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(backoff_max);
                    continue;
                }

                return Err(AppError::HuggingFace(format!(
                    "embedding request failed: {status} {body}"
                )));
            }
            Err(err) => {
                let detail = format_reqwest_error(&err);
                if attempt < max_retries {
                    warn!(
                        "huggingface embedding request failed to send: {detail}; retrying in {:?} (attempt {}/{})",
                        backoff,
                        attempt + 1,
                        max_retries
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(backoff_max);
                    continue;
                }

                return Err(AppError::HuggingFace(format!(
                    "embedding request failed to send: {detail}"
                )));
            }
        }
    }

    Err(AppError::HuggingFace(
        "embedding request exhausted retries".into(),
    ))
}

fn should_retry_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT
        || status.is_server_error()
}

fn format_reqwest_error(err: &reqwest::Error) -> String {
    let mut parts = Vec::new();
    if let Some(url) = err.url() {
        parts.push(format!("url={url}"));
    }
    if err.is_timeout() {
        parts.push("timeout".into());
    }
    if err.is_connect() {
        parts.push("connect".into());
    }
    if err.is_request() {
        parts.push("request".into());
    }
    if err.is_status() {
        parts.push("status".into());
    }

    let mut chain = Vec::new();
    let mut source = err.source();
    while let Some(err) = source {
        chain.push(err.to_string());
        source = err.source();
    }
    if !chain.is_empty() {
        parts.push(format!("source={}", chain.join(": ")));
    }

    if parts.is_empty() {
        err.to_string()
    } else {
        format!("{err} ({})", parts.join(", "))
    }
}

async fn embed_text(state: &AppState, text: &str) -> Result<Vec<f32>, AppError> {
    let truncated = truncate_for_embedding(text, state.huggingface_max_chars);
    fetch_hf_embedding(state, truncated.as_ref()).await
}

async fn embed_content_with_cache(
    state: &AppState,
    vv_path: &StdPath,
    content: &str,
    content_sha: &str,
) -> Result<Vec<f32>, AppError> {
    let vectors_path = vv_path.join("vectors");
    fs::create_dir_all(&vectors_path).await?;
    let cache_path = vectors_path.join(format!("{content_sha}.json"));
    if let Ok(data) = fs::read(&cache_path).await {
        if let Ok(values) = serde_json::from_slice::<Vec<f32>>(&data) {
            if values.len() == EMBEDDING_DIM {
                return Ok(values);
            }
            warn!(
                "cached embedding dimension mismatch for {} (got {}, expected {})",
                cache_path.display(),
                values.len(),
                EMBEDDING_DIM
            );
        }
    }

    let embedding = embed_text(state, content).await?;
    if let Ok(serialized) = serde_json::to_vec(&embedding) {
        if let Err(err) = fs::write(&cache_path, serialized).await {
            warn!("failed to cache embedding at {}: {err}", cache_path.display());
        }
    }
    Ok(embedding)
}

async fn read_repo_readme(repo_path: &StdPath) -> Option<String> {
    let candidates = [
        "README.md",
        "README.mdx",
        "README.txt",
        "README",
        "readme.md",
        "readme.txt",
        "readme",
    ];
    for name in candidates {
        let path = repo_path.join(name);
        if fs::metadata(&path).await.is_ok() {
            if let Ok(content) = fs::read_to_string(&path).await {
                return Some(content);
            }
        }
    }
    None
}

async fn build_repo_summary_input(
    state: &AppState,
    record: &RepoRecord,
    repo_path: &StdPath,
) -> Result<String, AppError> {
    let files = list_repo_files(repo_path).await?;
    let mut language_counts: HashMap<String, usize> = HashMap::new();
    let mut file_lines = Vec::new();
    for file in &files {
        let language = guess_language(file);
        *language_counts.entry(language).or_insert(0) += 1;
        if file_lines.len() < 120 {
            file_lines.push(format!("- {}", file.to_string_lossy()));
        }
    }

    let mut languages: Vec<(String, usize)> = language_counts.into_iter().collect();
    languages.sort_by(|a, b| b.1.cmp(&a.1));
    let language_summary = languages
        .into_iter()
        .take(8)
        .map(|(lang, count)| format!("{lang} ({count})"))
        .collect::<Vec<_>>()
        .join(", ");

    let mut input = String::new();
    input.push_str(&format!("Repository: {}/{}\n", record.owner, record.name));
    if !language_summary.is_empty() {
        input.push_str(&format!("\nLanguages: {language_summary}\n"));
    }
    if !file_lines.is_empty() {
        input.push_str("\nFile tree (first 200 files):\n");
        input.push_str(&file_lines.join("\n"));
        input.push('\n');
    }
    let summary_limit = state.huggingface_summary_max_chars.min(6000);
    if let Some(readme) = read_repo_readme(repo_path).await {
        let cleaned = sanitize_vespa_content(readme.as_str());
        let excerpt = truncate_for_summary(&cleaned, summary_limit / 2);
        input.push_str("\nREADME excerpt:\n");
        input.push_str(excerpt.as_ref());
        input.push('\n');
    }

    Ok(truncate_for_summary(&input, summary_limit).into_owned())
}

fn parse_hf_summary(value: serde_json::Value) -> Result<String, AppError> {
    match value {
        serde_json::Value::Array(values) => {
            let first = values.get(0).ok_or_else(|| {
                AppError::HuggingFace("empty summary response".into())
            })?;
            if let Some(summary) = first
                .get("summary_text")
                .and_then(|value| value.as_str())
            {
                return Ok(summary.to_string());
            }
            Err(AppError::HuggingFace(
                "missing summary_text in response".into(),
            ))
        }
        serde_json::Value::Object(map) => {
            if let Some(error) = map.get("error").and_then(|value| value.as_str()) {
                return Err(AppError::HuggingFace(error.to_string()));
            }
            if let Some(summary) = map
                .get("summary_text")
                .and_then(|value| value.as_str())
            {
                return Ok(summary.to_string());
            }
            Err(AppError::HuggingFace(
                "unexpected summary response".into(),
            ))
        }
        _ => Err(AppError::HuggingFace(
            "unexpected summary response".into(),
        )),
    }
}

async fn fetch_hf_summary(state: &AppState, text: &str) -> Result<String, AppError> {
    let base_url = state.huggingface_base_url.trim_end_matches('/');
    let url = format!(
        "{}/{}/pipeline/summarization",
        base_url, state.huggingface_summary_model
    );
    let payload = serde_json::json!({
        "inputs": text,
        "parameters": {
            "max_length": 220,
            "min_length": 80,
            "do_sample": false,
            "truncation": true
        },
        "options": { "wait_for_model": true }
    });

    let max_retries = state.huggingface_max_retries;
    let mut backoff = Duration::from_millis(state.huggingface_backoff_ms);
    let backoff_max = Duration::from_millis(state.huggingface_backoff_max_ms);

    for attempt in 0..=max_retries {
        let mut request = state.hf_client.post(&url).json(&payload);
        if let Some(token) = state.huggingface_token.as_deref() {
            request = request.bearer_auth(token);
        }

        match request.send().await {
            Ok(response) => {
                if response.status().is_success() {
                    let value: serde_json::Value = response
                        .json()
                        .await
                        .map_err(|err| AppError::HuggingFace(err.to_string()))?;
                    return parse_hf_summary(value);
                }

                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                if attempt < max_retries && should_retry_status(status) {
                    warn!(
                        "huggingface summary request failed with {status}; retrying in {:?} (attempt {}/{})",
                        backoff,
                        attempt + 1,
                        max_retries
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(backoff_max);
                    continue;
                }

                return Err(AppError::HuggingFace(format!(
                    "summary request failed: {status} {body}"
                )));
            }
            Err(err) => {
                let detail = format_reqwest_error(&err);
                if attempt < max_retries {
                    warn!(
                        "huggingface summary request failed to send: {detail}; retrying in {:?} (attempt {}/{})",
                        backoff,
                        attempt + 1,
                        max_retries
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(backoff_max);
                    continue;
                }

                return Err(AppError::HuggingFace(format!(
                    "summary request failed to send: {detail}"
                )));
            }
        }
    }

    Err(AppError::HuggingFace(
        "summary request exhausted retries".into(),
    ))
}

async fn generate_repo_summary(
    state: &AppState,
    record: &RepoRecord,
    repo_path: &StdPath,
    vv_path: &StdPath,
) -> Result<SummaryStore, AppError> {
    let input = build_repo_summary_input(state, record, repo_path).await?;
    let summary = match fetch_hf_summary(state, input.as_ref()).await {
        Ok(summary) => summary,
        Err(AppError::HuggingFace(message))
            if message.contains("index out of range")
                || message.contains("Bad Request") =>
        {
            let shorter = truncate_for_summary(input.as_ref(), 2000);
            fetch_hf_summary(state, shorter.as_ref()).await?
        }
        Err(err) => return Err(err),
    };
    let mut store = read_summary_store(vv_path).await.unwrap_or_default();
    let entry = SummaryEntry {
        version: store.next_version(),
        created_at: Utc::now().timestamp_millis(),
        summary: summary.clone(),
    };
    store.entries.push(entry);
    write_summary_store(vv_path, &store).await?;
    let _ = fs::write(vv_path.join("wiki/index.md"), summary).await;
    Ok(store)
}

fn build_search_yql(repo_filter: Option<&str>, mode: SearchMode) -> String {
    let mut clauses = Vec::new();
    if matches!(mode, SearchMode::Hybrid | SearchMode::Semantic) {
        clauses.push("{targetHits:100}nearestNeighbor(embedding, query_embedding)".to_string());
    }
    if matches!(mode, SearchMode::Hybrid | SearchMode::Bm25) {
        clauses.push("userInput(@query)".to_string());
    }

    let mut clause = if clauses.len() == 1 {
        clauses[0].clone()
    } else {
        format!("({})", clauses.join(" or "))
    };

    if repo_filter
        .and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        })
        .is_some()
    {
        clause.push_str(" and repo_id = @repo_id");
    }
    format!(
        "select repo_id, file_path, line_start, line_end, content from sources * where {};",
        clause
    )
}

fn escape_yql_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\"', "\\\"")
}

fn build_snippet(content: &str) -> String {
    const MAX_CHARS: usize = 400;
    let trimmed = content.trim();
    let mut chars = trimmed.chars();
    let snippet: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        let mut limited = snippet;
        limited.push_str("...");
        limited
    } else {
        snippet
    }
}

fn guess_language(path: &StdPath) -> String {
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    match extension {
        "rs" => "rust",
        "ts" => "typescript",
        "tsx" => "typescript",
        "js" => "javascript",
        "jsx" => "javascript",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "rb" => "ruby",
        "md" => "markdown",
        "json" => "json",
        "yml" | "yaml" => "yaml",
        _ => "unknown",
    }
    .to_string()
}

fn parse_repo_url(repo_url: &str) -> Result<(String, String), AppError> {
    let trimmed = repo_url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git");

    let cleaned = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("http://github.com/"))
        .or_else(|| trimmed.strip_prefix("git@github.com:"))
        .ok_or(AppError::InvalidRepoUrl)?;

    let mut parts = cleaned.split('/');
    let owner = parts.next().ok_or(AppError::InvalidRepoUrl)?;
    let name = parts.next().ok_or(AppError::InvalidRepoUrl)?;
    Ok((owner.to_string(), name.to_string()))
}

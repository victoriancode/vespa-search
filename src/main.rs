use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path as StdPath, PathBuf},
    sync::Arc,
};
use thiserror::Error;
use tokio::{fs, io::AsyncWriteExt, process::Command, sync::RwLock};
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RepoRecord {
    id: String,
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

#[derive(Debug, Serialize, Deserialize)]
struct SearchRequest {
    query: String,
    repo_filter: Option<String>,
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

#[derive(Debug, Serialize, Deserialize)]
struct WikiResponse {
    content: String,
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
    vespa_endpoint: String,
    vespa_document_endpoint: String,
    vespa_cluster: String,
    vespa_namespace: String,
    vespa_document_type: String,
    http_client: reqwest::Client,
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
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let status = match self {
            AppError::InvalidRepoUrl => StatusCode::BAD_REQUEST,
            AppError::RepoNotFound => StatusCode::NOT_FOUND,
            AppError::Config(_) | AppError::Io(_) | AppError::Serde(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            AppError::VespaRequest(_) | AppError::VespaRejected(_) => StatusCode::BAD_GATEWAY,
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
    let vespa_endpoint =
        std::env::var("VESPA_ENDPOINT").unwrap_or_else(|_| "http://localhost:8080".into());
    let vespa_document_endpoint = std::env::var("VESPA_DOCUMENT_ENDPOINT")
        .unwrap_or_else(|_| vespa_endpoint.clone());
    let vespa_cluster =
        std::env::var("VESPA_CLUSTER").unwrap_or_else(|_| "codesearch".into());
    let vespa_namespace = std::env::var("VESPA_NAMESPACE").unwrap_or_else(|_| "codesearch".into());
    let vespa_document_type =
        std::env::var("VESPA_DOCUMENT_TYPE").unwrap_or_else(|_| "codesearch".into());

    fs::create_dir_all(registry_path.parent().unwrap()).await?;
    fs::create_dir_all(&repos_path).await?;

    let registry = load_registry(&registry_path).await.unwrap_or_default();

    let state = AppState {
        registry_path,
        repos_path,
        registry: Arc::new(RwLock::new(registry)),
        vespa_endpoint,
        vespa_document_endpoint,
        vespa_cluster,
        vespa_namespace,
        vespa_document_type,
        http_client: build_http_client()?,
    };

    let app = Router::new()
        .route("/repos", post(create_repo).get(list_repos))
        .route("/repos/:id/index", post(index_repo))
        .route("/repos/:id/status", get(repo_status))
        .route("/repos/:id/wiki", get(repo_wiki))
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
    let record = {
        let registry = state.registry.read().await;
        registry
            .iter()
            .find(|repo| repo.id == id)
            .cloned()
            .ok_or(AppError::RepoNotFound)?
    };

    let repo_path = state.repos_path.join(&record.owner).join(&record.name);
    let vv_path = repo_path.join("vv");

    write_status(&vv_path, "in_progress", Some("Ingestion queued".into())).await?;
    let state_clone = state.clone();
    let record_clone = record.clone();
    let repo_path_clone = repo_path.clone();
    let vv_path_clone = vv_path.clone();
    tokio::spawn(async move {
        if let Err(err) =
            ingest_repo(state_clone, record_clone, repo_path_clone, vv_path_clone).await
        {
            error!("ingestion failed for repo {}: {}", record.id, err);
            let _ = write_status(&vv_path, "error", Some(err.to_string())).await;
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
    let record = {
        let registry = state.registry.read().await;
        registry
            .iter()
            .find(|repo| repo.id == id)
            .cloned()
            .ok_or(AppError::RepoNotFound)?
    };
    let vv_path = state
        .repos_path
        .join(&record.owner)
        .join(&record.name)
        .join("vv");
    read_status(&vv_path).await
}

async fn repo_wiki(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<WikiResponse>, AppError> {
    let record = {
        let registry = state.registry.read().await;
        registry
            .iter()
            .find(|repo| repo.id == id)
            .cloned()
            .ok_or(AppError::RepoNotFound)?
    };
    let wiki_path = state
        .repos_path
        .join(&record.owner)
        .join(&record.name)
        .join("vv/wiki/index.md");

    let content = fs::read_to_string(wiki_path)
        .await
        .unwrap_or_else(|_| "# CodeWiki\n\nWiki content is not yet available.".to_string());
    Ok(Json(WikiResponse { content }))
}

async fn search(
    State(state): State<AppState>,
    Json(payload): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, AppError> {
    let query = payload.query.trim();
    if query.is_empty() {
        return Ok(Json(SearchResponse { results: vec![] }));
    }

    let yql = build_search_yql(query, payload.repo_filter.as_deref());

    let response = state
        .http_client
        .post(vespa_search_url(&state))
        .json(&serde_json::json!({
            "yql": yql,
            "hits": 10,
        }))
        .send()
        .await?;

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

async fn write_status(
    vv_path: &StdPath,
    status: &str,
    message: Option<String>,
) -> Result<(), AppError> {
    fs::create_dir_all(vv_path).await?;
    let payload = StatusResponse {
        status: status.into(),
        message,
    };
    fs::write(
        vv_path.join("status.json"),
        serde_json::to_vec_pretty(&payload)?,
    )
    .await?;
    Ok(())
}

async fn read_status(vv_path: &StdPath) -> Result<Json<StatusResponse>, AppError> {
    let path = vv_path.join("status.json");
    if !path.exists() {
        return Ok(Json(StatusResponse {
            status: "unknown".into(),
            message: None,
        }));
    }
    let data = fs::read(path).await?;
    let status = serde_json::from_slice(&data)?;
    Ok(Json(status))
}

async fn ingest_repo(
    state: AppState,
    record: RepoRecord,
    repo_path: PathBuf,
    vv_path: PathBuf,
) -> Result<(), AppError> {
    write_status(&vv_path, "in_progress", Some("Cloning repository".into())).await?;

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
                &vv_path,
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
        let status = Command::new("git")
            .arg("clone")
            .arg(&record.repo_url)
            .arg(&repo_path)
            .status()
            .await
            .map_err(AppError::Io)?;

        if !status.success() {
            write_status(&vv_path, "error", Some("Git clone failed".into())).await?;
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "git clone failed",
            )));
        }
    }

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
        &vv_path,
        "indexing",
        Some("Feeding documents to Vespa".into()),
    )
    .await?;
    let indexed = feed_repo_to_vespa(&state, &record, &repo_path, &vv_path).await?;
    info!(
        "vespa feed completed for repo {} ({} documents)",
        record.id, indexed
    );
    write_status(&vv_path, "complete", Some("Ingestion complete".into())).await?;

    Ok(())
}

async fn feed_repo_to_vespa(
    state: &AppState,
    record: &RepoRecord,
    repo_path: &StdPath,
    vv_path: &StdPath,
) -> Result<usize, AppError> {
    const MAX_CONTENT_BYTES: usize = 200_000;
    const EMBEDDING_DIM: usize = 768;

    let files = list_repo_files(repo_path).await?;
    let embedding = vec![0.0_f32; EMBEDDING_DIM];
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

        let content = String::from_utf8_lossy(&content_bytes).to_string();
        let line_end = content.lines().count().max(1) as i32;
        let content_sha = sha256_hex(&content_bytes);
        let chunk_id = sha256_hex(format!("{}:{}", record.id, file_path.display()).as_bytes());
        let chunk_hash = sha256_hex(content.as_bytes());
        let language = guess_language(&file_path);
        let last_indexed_at = Utc::now().timestamp_millis();
        let chunk_id_for_chunk = chunk_id.clone();
        let content_sha_for_chunk = content_sha.clone();

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
                    values: embedding.clone(),
                },
                last_indexed_at,
            },
        };
        let body_bytes = serde_json::to_vec(&put)?;
        let response = state
            .http_client
            .post(vespa_document_url(state, &doc_id))
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
        if entry.file_name() != "vv" {
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
            return Ok(stdout.lines().map(PathBuf::from).collect());
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

fn vespa_document_url(state: &AppState, doc_id: &str) -> String {
    format!(
        "{}/document/v1/{}/{}/docid/{}",
        state.vespa_document_endpoint.trim_end_matches('/'),
        state.vespa_namespace,
        state.vespa_document_type,
        urlencoding::encode(doc_id)
    )
}

fn vespa_search_url(state: &AppState) -> String {
    format!("{}/search/", state.vespa_endpoint.trim_end_matches('/'))
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn build_search_yql(query: &str, repo_filter: Option<&str>) -> String {
    let mut clause = format!("content contains \"{}\"", escape_yql_string(query));
    if let Some(repo_id) = repo_filter.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }) {
        clause.push_str(" and repo_id contains \"");
        clause.push_str(&escape_yql_string(repo_id));
        clause.push('"');
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
    if trimmed.len() <= MAX_CHARS {
        return trimmed.to_string();
    }
    let mut snippet = trimmed[..MAX_CHARS].to_string();
    snippet.push_str("...");
    snippet
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

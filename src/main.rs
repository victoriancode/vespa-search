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
use tracing::{error, info};
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

#[derive(Clone)]
struct AppState {
    registry_path: PathBuf,
    repos_path: PathBuf,
    registry: Arc<RwLock<Vec<RepoRecord>>>,
    vespa_endpoint: String,
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

fn build_http_client() -> Result<reqwest::Client, AppError> {
    let cert = std::env::var("VESPA_CLIENT_CERT").ok();
    let key = std::env::var("VESPA_CLIENT_KEY").ok();
    let ca_cert = if let Ok(ca_cert) = std::env::var("VESPA_CA_CERT") {
        ca_cert
    } else {
        let ca_cert_path = std::env::var("VESPA_CA_CERT_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("vespa/application/security/clients.pem"));
        std::fs::read_to_string(&ca_cert_path).map_err(|err| {
            AppError::Config(format!(
                "failed to read Vespa CA cert at {}: {err}",
                ca_cert_path.display()
            ))
        })?
    };

    let mut builder = reqwest::Client::builder();

    let ca_cert = normalize_pem(&ca_cert);
    let ca = reqwest::Certificate::from_pem(ca_cert.as_bytes())
        .map_err(|err| AppError::Config(format!("invalid Vespa CA cert: {err}")))?;
    builder = builder.add_root_certificate(ca);

    match (cert, key) {
        (None, None) => builder
            .build()
            .map_err(|err| AppError::Config(format!("failed to build HTTP client: {err}"))),
        (Some(cert), Some(key)) => {
            let cert = normalize_pem(&cert);
            let key = normalize_pem(&key);
            let mut identity_pem = Vec::with_capacity(cert.len() + key.len() + 2);
            identity_pem.extend_from_slice(cert.as_bytes());
            identity_pem.extend_from_slice(b"\n");
            identity_pem.extend_from_slice(key.as_bytes());
            let identity = reqwest::Identity::from_pem(&identity_pem)
                .map_err(|err| AppError::Config(format!("invalid Vespa client cert/key: {err}")))?;
            builder
                .identity(identity)
                .build()
                .map_err(|err| AppError::Config(format!("failed to build HTTP client: {err}")))
        }
        _ => Err(AppError::Config(
            "both VESPA_CLIENT_CERT and VESPA_CLIENT_KEY must be set for mTLS".into(),
        )),
    }
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

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
    let vespa_namespace =
        std::env::var("VESPA_NAMESPACE").unwrap_or_else(|_| "codesearch".into());
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

    write_status(&vv_path, "in_progress", Some("Cloning repository".into())).await?;

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
    fs::write(vv_path.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?).await?;
    fs::write(vv_path.join("chunks.jsonl"), "").await?;

    let wiki_content = format!(
        "# CodeWiki for {}/{}\n\nThis is a placeholder wiki generated during ingestion.\n",
        record.owner, record.name
    );
    fs::write(vv_path.join("wiki/index.md"), wiki_content).await?;

    write_status(&vv_path, "indexing", Some("Feeding documents to Vespa".into())).await?;
    let indexed = feed_repo_to_vespa(&state, &record, &repo_path, &vv_path).await?;
    info!(
        "vespa feed completed for repo {} ({} documents)",
        record.id, indexed
    );
    write_status(&vv_path, "complete", Some("Ingestion complete".into())).await?;

    Ok(Json(StatusResponse {
        status: "complete".into(),
        message: Some("Ingestion complete".into()),
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

    let content = fs::read_to_string(wiki_path).await.unwrap_or_else(|_| {
        "# CodeWiki\n\nWiki content is not yet available.".to_string()
    });
    Ok(Json(WikiResponse { content }))
}

async fn search(Json(payload): Json<SearchRequest>) -> Result<Json<SearchResponse>, AppError> {
    let _ = payload;
    Ok(Json(SearchResponse { results: vec![] }))
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
        let line_end = content.lines().count().max(1) as i64;
        let content_sha = sha256_hex(&content_bytes);
        let chunk_id = sha256_hex(format!("{}:{}", record.id, file_path.display()).as_bytes());
        let chunk_hash = sha256_hex(content.as_bytes());
        let language = guess_language(&file_path);
        let last_indexed_at = Utc::now().timestamp_millis();

        let fields = serde_json::json!({
            "repo_id": record.id,
            "repo_url": record.repo_url,
            "repo_name": record.name,
            "repo_owner": record.owner,
            "commit_sha": "unknown",
            "branch": "main",
            "file_path": file_path.to_string_lossy(),
            "language": language,
            "license_spdx": "unknown",
            "chunk_id": chunk_id,
            "chunk_hash": chunk_hash,
            "line_start": 1,
            "line_end": line_end,
            "symbol_names": Vec::<String>::new(),
            "content": content,
            "content_sha": content_sha,
            "embedding": { "values": embedding.clone() },
            "last_indexed_at": last_indexed_at,
        });

        let doc_id = format!("{}-{}", record.id, chunk_id);
        let response = state
            .http_client
            .put(vespa_document_url(state, &doc_id))
            .json(&serde_json::json!({ "fields": fields }))
            .send()
            .await?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::VespaRejected(body));
        }

        let chunk_entry = serde_json::json!({
            "repo_id": record.id,
            "file_path": file_path.to_string_lossy(),
            "chunk_id": chunk_id,
            "line_start": 1,
            "line_end": line_end,
            "content_sha": content_sha,
        });
        let serialized = serde_json::to_string(&chunk_entry)?;
        chunks_file.write_all(serialized.as_bytes()).await?;
        chunks_file.write_all(b"\n").await?;
        indexed += 1;
    }

    Ok(indexed)
}

async fn list_repo_files(repo_path: &StdPath) -> Result<Vec<PathBuf>, AppError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("ls-files")
        .output()
        .await
        .map_err(AppError::Io)?;

    if !output.status.success() {
        return Err(AppError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            "git ls-files failed",
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().map(PathBuf::from).collect())
}

fn vespa_document_url(state: &AppState, doc_id: &str) -> String {
    format!(
        "{}/document/v1/{}/{}/docid/{}",
        state.vespa_endpoint,
        state.vespa_namespace,
        state.vespa_document_type,
        urlencoding::encode(doc_id)
    )
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
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

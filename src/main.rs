use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path as StdPath, PathBuf},
    sync::Arc,
};
use thiserror::Error;
use tokio::{fs, process::Command, sync::RwLock};
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
}

#[derive(Error, Debug)]
enum AppError {
    #[error("invalid repo url")]
    InvalidRepoUrl,
    #[error("repo not found")]
    RepoNotFound,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let status = match self {
            AppError::InvalidRepoUrl => StatusCode::BAD_REQUEST,
            AppError::RepoNotFound => StatusCode::NOT_FOUND,
            AppError::Io(_) | AppError::Serde(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = Json(serde_json::json!({"error": self.to_string()}));
        (status, body).into_response()
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

    fs::create_dir_all(registry_path.parent().unwrap()).await?;
    fs::create_dir_all(&repos_path).await?;

    let registry = load_registry(&registry_path).await.unwrap_or_default();

    let state = AppState {
        registry_path,
        repos_path,
        registry: Arc::new(RwLock::new(registry)),
    };

    let app = Router::new()
        .route("/repos", post(create_repo).get(list_repos))
        .route("/repos/:id/index", post(index_repo))
        .route("/repos/:id/status", get(repo_status))
        .route("/repos/:id/wiki", get(repo_wiki))
        .route("/search", post(search))
        .with_state(state)
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any));

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

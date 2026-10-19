//! Single-owner management API. No OpenCode or cloud credentials are needed.
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    store::{ImportMetadata, Search, Store},
    MAX_UPLOAD,
};

#[derive(Clone)]
struct App {
    store: Store,
    token_hash: Arc<[u8; 32]>,
}

pub struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        // Internal errors stay in the service log, never in a browser response.
        tracing::error!(%error, "Knowledge request failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Knowledge operation failed; check service logs".into(),
        )
    }
}
fn bad(message: impl ToString) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, message.to_string())
}
fn missing() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "Source or section not found".into())
}

pub fn router(store: Store, token: &str) -> anyhow::Result<Router> {
    anyhow::ensure!(
        token.len() >= 24 && token.is_ascii() && !token.chars().any(char::is_whitespace),
        "JARVIS_KNOWLEDGE_TOKEN must have at least 24 non-whitespace ASCII characters"
    );
    let app = App {
        store,
        token_hash: Arc::new(Sha256::digest(token.as_bytes()).into()),
    };
    let tools_store = app.store.clone();
    let mcp: StreamableHttpService<crate::mcp::KnowledgeTools, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(crate::mcp::KnowledgeTools::new(tools_store.clone())),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_legacy_session_mode(false)
                .with_json_response(true)
                .enforce_origin_validation(),
        );
    Ok(Router::new()
        .nest_service("/mcp", mcp)
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/documents", get(documents).post(import))
        .route("/v1/documents/{id}", axum::routing::delete(delete))
        .route("/v1/documents/{id}/sections", get(sections))
        .route("/v1/documents/{id}/original", get(original))
        .route("/v1/jobs/{id}", get(job))
        .route("/v1/search", post(search))
        .route("/v1/sections/{id}", get(read))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD + 64 * 1024))
        .layer(middleware::from_fn_with_state(app.clone(), authenticate))
        .with_state(app))
}

async fn authenticate(State(app): State<App>, request: Request, next: Next) -> Response {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or("");
    let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let different = hash
        .iter()
        .zip(app.token_hash.iter())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    if different != 0 {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            Json(json!({"error":"Authentication required"})),
        )
            .into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
}

async fn capabilities(State(app): State<App>) -> Result<Json<serde_json::Value>, ApiError> {
    sqlx::query("SELECT 1")
        .execute(&app.store.pool)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(Json(
        json!({"api_version":1,"service":"jarvis-knowledge","single_owner":true,
        "keyword_search":true,"semantic_search":false,"memory":false,"mcp":true,
        "generation":false,"formats":["txt","md","pdf"],"ocr":false,"max_upload_bytes":MAX_UPLOAD}),
    ))
}

async fn import(
    State(app): State<App>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<crate::store::Job>), ApiError> {
    let mut metadata = None;
    let mut upload = None;
    while let Some(field) = multipart.next_field().await.map_err(bad)? {
        match field.name().unwrap_or("") {
            "metadata" if metadata.is_none() => {
                let bytes = field.bytes().await.map_err(bad)?;
                if bytes.len() > 4096 {
                    return Err(bad("Metadata is too large"));
                }
                metadata = Some(serde_json::from_slice::<ImportMetadata>(&bytes).map_err(bad)?);
            }
            "file" if upload.is_none() => {
                let filename = field
                    .file_name()
                    .ok_or_else(|| bad("File needs a filename"))?
                    .to_string();
                crate::extract::extension(&filename).map_err(bad)?;
                let bytes = field.bytes().await.map_err(bad)?;
                if bytes.is_empty() || bytes.len() > MAX_UPLOAD {
                    return Err(bad("File must contain 1–10 MiB of data"));
                }
                upload = Some((filename, bytes));
            }
            _ => {
                return Err(bad(
                    "Expected exactly one metadata field and one file field",
                ))
            }
        }
    }
    let metadata = metadata.ok_or_else(|| bad("Missing metadata"))?;
    metadata.validate().map_err(bad)?;
    let (filename, bytes) = upload.ok_or_else(|| bad("Missing file"))?;
    if filename.len() > 500 || filename.contains('\0') {
        return Err(bad("Invalid filename"));
    }
    let job = app.store.enqueue(metadata, &filename, &bytes).await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn documents(State(app): State<App>) -> Result<Json<Vec<crate::store::Document>>, ApiError> {
    Ok(Json(app.store.documents().await?))
}
async fn job(
    State(app): State<App>,
    Path(id): Path<Uuid>,
) -> Result<Json<crate::store::Job>, ApiError> {
    Ok(Json(app.store.job(id).await?.ok_or_else(missing)?))
}
async fn sections(
    State(app): State<App>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<crate::store::SourceSection>>, ApiError> {
    Ok(Json(app.store.sections(id).await?))
}
async fn search(
    State(app): State<App>,
    Json(query): Json<Search>,
) -> Result<Json<serde_json::Value>, ApiError> {
    query.validate().map_err(bad)?;
    let hits = app.store.search(&query).await?;
    Ok(Json(json!({"method":"keyword","query":query,"hits":hits,
        "note":"Ranks measure lexical relevance, not confidence. No model has answered this question."})))
}
#[derive(Deserialize)]
struct Read {
    #[serde(default)]
    surrounding_sections: i32,
}
async fn read(
    State(app): State<App>,
    Path(id): Path<Uuid>,
    Query(query): Query<Read>,
) -> Result<Json<Vec<crate::store::SourceSection>>, ApiError> {
    if !(0..=2).contains(&query.surrounding_sections) {
        return Err(bad("surrounding_sections must be between 0 and 2"));
    }
    let sections = app.store.read(id, query.surrounding_sections).await?;
    if sections.is_empty() {
        return Err(missing());
    }
    Ok(Json(sections))
}
async fn original(State(app): State<App>, Path(id): Path<Uuid>) -> Result<Response, ApiError> {
    let (filename, bytes) = app.store.original(id).await?.ok_or_else(missing)?;
    let ext = crate::extract::extension(&filename).map_err(bad)?;
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{id}.{ext}\"")
            .parse()
            .unwrap(),
    );
    Ok(response)
}
async fn delete(State(app): State<App>, Path(id): Path<Uuid>) -> Result<StatusCode, ApiError> {
    if app.store.delete(id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(missing())
    }
}

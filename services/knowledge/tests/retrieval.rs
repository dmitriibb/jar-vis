//! Real PostgreSQL verification. Creates/drops only a uniquely named test DB.
//! Run with: cargo test --test retrieval -- --ignored --nocapture
use std::{path::PathBuf, time::Duration};

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use jarvis_knowledge::{
    api, extract,
    store::{ImportMetadata, Job, Search, Store},
};
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, Executor};
use tower::ServiceExt;
use uuid::Uuid;

const TOKEN: &str = "test-owner-token-at-least-24-characters";
const RULES: &[u8] = include_bytes!("fixtures/harbor-rules.md");

fn metadata(key: &str) -> ImportMetadata {
    ImportMetadata {
        source_key: key.into(),
        title: "Harbor Traders rules".into(),
        game: Some("Harbor Traders".into()),
        edition: Some("base".into()),
    }
}
fn search(query: &str) -> Search {
    Search {
        query: query.into(),
        game: None,
        edition: None,
        document_id: None,
        limit: 5,
    }
}

async fn finished(store: &Store, id: Uuid) -> Job {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let job = store.job(id).await.unwrap().unwrap();
            if job.state == "succeeded" || job.state == "failed" {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("indexing completed")
}

#[tokio::test]
async fn api_rejects_unauthenticated_requests_before_database_access() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/unused")
        .unwrap();
    let store = Store {
        pool,
        root: PathBuf::from("unused"),
    };
    let router = api::router(store.clone(), TOKEN).unwrap();
    for (path, token) in [
        ("/v1/documents", None),
        ("/mcp", None),
        ("/mcp", Some("wrong")),
    ] {
        let mut builder = Request::builder().uri(path);
        if let Some(token) = token {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        let response = router
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert!(api::router(store, "short").is_err());
}

#[tokio::test]
#[ignore = "requires the development PostgreSQL container; creates an isolated test database"]
async fn import_search_provenance_replacement_restart_and_delete() {
    if let Err(error) = dotenvy::from_path(".env") {
        assert!(error.not_found(), "Could not load .env");
    }
    let admin_url = std::env::var("JARVIS_KNOWLEDGE_TEST_DATABASE_URL")
        .or_else(|_| std::env::var("JARVIS_KNOWLEDGE_DATABASE_URL"))
        .unwrap_or_else(|_| {
            "postgres://jarvis:jarvis-dev-only@127.0.0.1:55432/jarvis_knowledge".into()
        });
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url)
        .await
        .expect("start docker compose up -d --wait first");
    let name = format!("jarvis_test_{}", Uuid::new_v4().simple());
    admin
        .execute(format!("CREATE DATABASE {name}").as_str())
        .await
        .unwrap();
    let mut url = reqwest::Url::parse(&admin_url).unwrap();
    url.set_path(&name);
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::connect(url.as_str(), temporary.path().into())
        .await
        .unwrap();
    let router = api::router(store.clone(), TOKEN).unwrap();
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/capabilities")
                .header("Authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let first = store
        .enqueue(metadata("harbor/base"), "rules.md", RULES)
        .await
        .unwrap();
    let again = store
        .enqueue(metadata("harbor/base"), "renamed.md", RULES)
        .await
        .unwrap();
    assert_eq!(
        first.id, again.id,
        "duplicate import must reuse the same job"
    );
    // Mimic a process that died after claiming a job. Worker startup must recover it.
    sqlx::query("UPDATE ingestion_jobs SET state='running' WHERE id=$1")
        .bind(first.id)
        .execute(&store.pool)
        .await
        .unwrap();
    let worker_store = store.clone();
    let worker = tokio::spawn(async move { worker_store.worker().await });
    let first = finished(&store, first.id).await;
    assert_eq!(first.state, "succeeded", "{:?}", first.error);
    assert_eq!(store.documents().await.unwrap().len(), 1);
    assert!(store
        .worker()
        .await
        .unwrap_err()
        .to_string()
        .contains("Another knowledge worker"));

    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/questions.json")).unwrap();
    for case in cases {
        let hits = store
            .search(&search(case["query"].as_str().unwrap()))
            .await
            .unwrap();
        if case["expected_heading"].is_null() {
            assert!(hits.is_empty());
        } else {
            assert_eq!(
                hits[0].section.heading,
                case["expected_heading"].as_str().unwrap()
            );
            assert!(hits[0]
                .section
                .text
                .contains(case["expected_text"].as_str().unwrap()));
        }
    }
    let hits = store.search(&search("trade spice")).await.unwrap();
    let section = &hits[0].section;
    assert_eq!(section.document_id, first.document_id);
    assert_eq!(section.job_id, first.id);
    assert_eq!(section.source_key, "harbor/base");
    assert_eq!(store.read(section.id, 1).await.unwrap().len(), 3);
    // Exercise the real MCP HTTP transport and generated tool schemas, not just
    // the store. This is the same endpoint OpenCode connects to.
    let rpc = |id: i32, method: &str, params: Value| {
        Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("Host", "127.0.0.1:47831")
            .header("Authorization", format!("Bearer {TOKEN}"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-03-26")
            .body(Body::from(
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string(),
            ))
            .unwrap()
    };
    let response = router.clone().oneshot(rpc(1, "initialize", json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"jarvis-test","version":"1"}}))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = router
        .clone()
        .oneshot(rpc(2, "tools/list", json!({})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 100000).await.unwrap()).unwrap();
    assert_eq!(value["result"]["tools"].as_array().unwrap().len(), 2);
    let response = router
        .clone()
        .oneshot(rpc(
            3,
            "tools/call",
            json!({"name":"search_knowledge","arguments":{"query":"trade spice"}}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 100000).await.unwrap()).unwrap();
    assert_ne!(value["result"]["isError"], json!(true));
    assert!(value["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("three copper coins"));
    assert_eq!(
        store.original(first.document_id).await.unwrap().unwrap().1,
        RULES
    );
    let mut filtered = search("trade spice");
    filtered.edition = Some("expansion".into());
    assert!(store.search(&filtered).await.unwrap().is_empty());

    // Multipart route uses exactly the same durable ingestion path as the CLI.
    let boundary = "jarvis-test-boundary";
    let body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"rules.md\"\r\nContent-Type: text/markdown\r\n\r\n{}\r\n--{boundary}--\r\n",
        serde_json::to_string(&metadata("harbor/http")).unwrap(), std::str::from_utf8(RULES).unwrap());
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/documents")
                .header("Authorization", format!("Bearer {TOKEN}"))
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = to_bytes(response.into_body(), 10000).await.unwrap();
    let http_job: Job = serde_json::from_slice(&body).unwrap();
    assert_eq!(finished(&store, http_job.id).await.state, "succeeded");
    store.delete(http_job.document_id).await.unwrap();

    // PDF fixture is a tiny, deterministic, two-page fictional rulebook.
    let pdf = include_bytes!("fixtures/harbor-rules.pdf");
    let pages = extract::extract("rules.pdf", pdf).unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[1].page, Some(2));
    let pdf_job = store
        .enqueue(metadata("harbor/pdf"), "rules.pdf", pdf)
        .await
        .unwrap();
    assert_eq!(finished(&store, pdf_job.id).await.state, "succeeded");
    let mut pdf_search = search("lighthouse");
    pdf_search.document_id = Some(pdf_job.document_id);
    let hits = store.search(&pdf_search).await.unwrap();
    assert_eq!(hits[0].section.page, Some(2));
    assert_eq!(
        store
            .original(pdf_job.document_id)
            .await
            .unwrap()
            .unwrap()
            .1,
        pdf
    );

    // Broken replacement cannot destroy the previous searchable version or its metadata.
    let mut replacement_meta = metadata("harbor/base");
    replacement_meta.title = "Broken replacement".into();
    let broken = store
        .enqueue(replacement_meta, "rules.pdf", b"broken PDF")
        .await
        .unwrap();
    assert_eq!(finished(&store, broken.id).await.state, "failed");
    assert!(!store
        .search(&search("trade spice"))
        .await
        .unwrap()
        .is_empty());
    let doc = store
        .documents()
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.id == first.document_id)
        .unwrap();
    assert_eq!(doc.active_job_id, Some(first.id));
    assert_eq!(doc.title, "Harbor Traders rules");
    assert_eq!(doc.latest_state, "failed");

    let replaced = store
        .enqueue(
            metadata("harbor/base"),
            "rules.md",
            b"# Scoring\nThe lighthouse scores eleven victory points.",
        )
        .await
        .unwrap();
    assert_eq!(finished(&store, replaced.id).await.state, "succeeded");
    assert!(
        store.read(section.id, 0).await.unwrap().is_empty(),
        "replaced citations must not resolve to new text"
    );
    let mut base_search = search("trade spice");
    base_search.document_id = Some(first.document_id);
    assert!(store.search(&base_search).await.unwrap().is_empty());

    // Missing evidence is an empty result, not a generated guess.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/search")
                .header("Authorization", format!("Bearer {TOKEN}"))
                .header("Content-Type", "application/json")
                .body(Body::from(json!({"query":"dragon combat"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
    assert_eq!(value["hits"], json!([]));

    // A fresh client sees persistence; deletion removes chunks, jobs, and originals.
    let fresh = Store::connect(url.as_str(), temporary.path().into())
        .await
        .unwrap();
    assert_eq!(fresh.documents().await.unwrap().len(), 2);
    for document in fresh.documents().await.unwrap() {
        assert!(fresh.delete(document.id).await.unwrap());
        assert!(!temporary.path().join(document.id.to_string()).exists());
        assert!(fresh.sections(document.id).await.unwrap().is_empty());
    }
    assert!(fresh
        .search(&search("lighthouse"))
        .await
        .unwrap()
        .is_empty());
    assert!(fresh.job(first.id).await.unwrap().is_none());
    assert!(!fresh.delete(first.document_id).await.unwrap());
    worker.abort();
    let _ = worker.await;
    fresh.pool.close().await;
    store.pool.close().await;
    admin
        .execute(format!("DROP DATABASE {name} WITH (FORCE)").as_str())
        .await
        .unwrap();
    admin.close().await;
    println!("Verified migration/pgvector, auth, Markdown/PDF import, restart recovery, worker ownership, deduplication, keyword fixtures, provenance, filters, replacement, persistence, HTTP, and deletion");
}

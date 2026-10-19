use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use jarvis_knowledge::store::{ImportMetadata, Job, Search, Store};
use reqwest::{multipart, Client, Method};
use serde_json::Value;
use uuid::Uuid;

#[derive(Parser)]
#[command(about = "Inspect Jarvis knowledge without an agent or cloud model")]
struct Args {
    #[arg(
        long,
        env = "JARVIS_KNOWLEDGE_URL",
        default_value = "http://127.0.0.1:47831",
        global = true
    )]
    url: String,
    /// Shared owner token. Prefer .env to shell arguments/history.
    #[arg(
        long,
        env = "JARVIS_KNOWLEDGE_TOKEN",
        hide_env_values = true,
        global = true
    )]
    token: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve {
        #[arg(long, env = "JARVIS_KNOWLEDGE_DATABASE_URL", hide_env_values = true)]
        database_url: String,
        #[arg(
            long,
            env = "JARVIS_KNOWLEDGE_DATA_DIR",
            default_value = ".data/originals"
        )]
        data_dir: PathBuf,
        #[arg(long, env = "JARVIS_KNOWLEDGE_BIND", default_value = "127.0.0.1:47831")]
        bind: SocketAddr,
    },
    Capabilities,
    Import {
        file: PathBuf,
        #[arg(long)]
        source_key: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        game: Option<String>,
        #[arg(long)]
        edition: Option<String>,
        #[arg(long)]
        wait: bool,
    },
    Documents,
    Job {
        id: Uuid,
    },
    Sections {
        document_id: Uuid,
    },
    Read {
        section_id: Uuid,
        #[arg(long, default_value_t = 0)]
        surrounding: i32,
    },
    Search {
        query: String,
        #[arg(long)]
        game: Option<String>,
        #[arg(long)]
        edition: Option<String>,
        #[arg(long)]
        document_id: Option<Uuid>,
        #[arg(long, default_value_t = 5)]
        limit: i64,
    },
    Original {
        document_id: Uuid,
        #[arg(long)]
        output: PathBuf,
    },
    Delete {
        document_id: Uuid,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load only this working directory's configuration, preserving process overrides.
    match dotenvy::from_path(".env") {
        Ok(()) => {}
        Err(dotenvy::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => bail!("Could not load .env; check its syntax and permissions"),
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse();
    let token = args
        .token
        .context("Set JARVIS_KNOWLEDGE_TOKEN in .env (see .env.example)")?;
    if let Command::Serve {
        database_url,
        data_dir,
        bind,
    } = args.command
    {
        let store = Store::connect(&database_url, data_dir).await?;
        let app = jarvis_knowledge::api::router(store.clone(), &token)?;
        let listener = tokio::net::TcpListener::bind(bind).await?;
        tracing::info!(%bind, "Knowledge service listening; no OpenCode, Jarvis Server, or model required");
        let worker = store.worker();
        tokio::select! {
            result = worker => result.context("Ingestion worker stopped")?,
            result = axum::serve(listener, app).with_graceful_shutdown(shutdown()) => result?,
        }
        return Ok(());
    }
    let url = reqwest::Url::parse(&args.url)?;
    anyhow::ensure!(
        url.scheme() == "http" || url.scheme() == "https",
        "Expected an HTTP(S) service URL"
    );
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let request = |method, path: &str| {
        client
            .request(
                method,
                format!("{}{}", args.url.trim_end_matches('/'), path),
            )
            .bearer_auth(&token)
    };
    let response = match args.command {
        Command::Capabilities => request(Method::GET, "/v1/capabilities").send().await?,
        Command::Documents => request(Method::GET, "/v1/documents").send().await?,
        Command::Job { id } => {
            request(Method::GET, &format!("/v1/jobs/{id}"))
                .send()
                .await?
        }
        Command::Sections { document_id } => {
            request(
                Method::GET,
                &format!("/v1/documents/{document_id}/sections"),
            )
            .send()
            .await?
        }
        Command::Read {
            section_id,
            surrounding,
        } => {
            request(
                Method::GET,
                &format!("/v1/sections/{section_id}?surrounding_sections={surrounding}"),
            )
            .send()
            .await?
        }
        Command::Search {
            query,
            game,
            edition,
            document_id,
            limit,
        } => {
            let search = Search {
                query,
                game,
                edition,
                document_id,
                limit,
            };
            search.validate()?;
            request(Method::POST, "/v1/search")
                .json(&search)
                .send()
                .await?
        }
        Command::Import {
            file,
            source_key,
            title,
            game,
            edition,
            wait,
        } => {
            anyhow::ensure!(
                tokio::fs::metadata(&file).await?.len() <= jarvis_knowledge::MAX_UPLOAD as u64,
                "File exceeds 10 MiB"
            );
            let filename = file
                .file_name()
                .context("Expected a filename")?
                .to_string_lossy()
                .into_owned();
            jarvis_knowledge::extract::extension(&filename)?;
            let metadata = ImportMetadata {
                source_key,
                title,
                game,
                edition,
            };
            metadata.validate()?;
            let form = multipart::Form::new()
                .text("metadata", serde_json::to_string(&metadata)?)
                .part(
                    "file",
                    multipart::Part::bytes(tokio::fs::read(file).await?).file_name(filename),
                );
            let response = request(Method::POST, "/v1/documents")
                .multipart(form)
                .send()
                .await?;
            if !wait {
                response
            } else {
                let job: Job = checked(response).await?.json().await?;
                let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
                loop {
                    let response = checked(
                        request(Method::GET, &format!("/v1/jobs/{}", job.id))
                            .send()
                            .await?,
                    )
                    .await?;
                    let value: Value = response.json().await?;
                    match value["state"].as_str() {
                        Some("succeeded") => {
                            println!("{}", serde_json::to_string_pretty(&value)?);
                            return Ok(());
                        }
                        Some("failed") => bail!("Import failed: {}", value["error"]),
                        _ => {}
                    }
                    if tokio::time::Instant::now() >= deadline {
                        bail!("Import still pending; inspect job {}", job.id);
                    }
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
            }
        }
        Command::Original {
            document_id,
            output,
        } => {
            let response = checked(
                request(
                    Method::GET,
                    &format!("/v1/documents/{document_id}/original"),
                )
                .send()
                .await?,
            )
            .await?;
            // Do not silently overwrite a source file chosen by the user.
            use tokio::io::AsyncWriteExt;
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(output)
                .await?;
            file.write_all(&response.bytes().await?).await?;
            return Ok(());
        }
        Command::Delete { document_id } => {
            request(Method::DELETE, &format!("/v1/documents/{document_id}"))
                .send()
                .await?
        }
        Command::Serve { .. } => unreachable!(),
    };
    let response = checked(response).await?;
    if response.status() != reqwest::StatusCode::NO_CONTENT {
        let value: Value = response.json().await?;
        println!("{}", serde_json::to_string_pretty(&value)?);
    }
    Ok(())
}

async fn checked(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if !status.is_success() {
        bail!("Knowledge API {status}: {}", response.text().await?);
    }
    Ok(response)
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Args::command().debug_assert();
    }
}

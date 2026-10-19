use std::{path::PathBuf, time::Duration};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgPoolOptions, FromRow, PgPool};
use uuid::Uuid;

use crate::{extract, MAX_UPLOAD};

#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
    pub root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportMetadata {
    pub source_key: String,
    pub title: String,
    pub game: Option<String>,
    pub edition: Option<String>,
}

impl ImportMetadata {
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("source_key", self.source_key.as_str()),
            ("title", self.title.as_str()),
        ] {
            if value.trim().is_empty() || value.len() > 500 || value.contains('\0') {
                bail!("{name} must contain 1–500 bytes of text");
            }
        }
        for value in [&self.game, &self.edition].into_iter().flatten() {
            if value.len() > 500 || value.contains('\0') {
                bail!("game/edition must contain at most 500 bytes of text");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Job {
    pub id: Uuid,
    pub document_id: Uuid,
    pub state: String,
    pub error: Option<String>,
    pub section_count: i32,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Document {
    pub id: Uuid,
    pub source_key: String,
    pub title: String,
    pub game: Option<String>,
    pub edition: Option<String>,
    pub active_job_id: Option<Uuid>,
    pub latest_job_id: Uuid,
    pub latest_state: String,
    pub latest_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct SourceSection {
    pub id: Uuid,
    pub document_id: Uuid,
    pub job_id: Uuid,
    pub ordinal: i32,
    pub title: String,
    pub source_key: String,
    pub game: Option<String>,
    pub edition: Option<String>,
    pub heading: String,
    pub page: Option<i32>,
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct SearchHit {
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub section: SourceSection,
    pub rank: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Search {
    pub query: String,
    pub game: Option<String>,
    pub edition: Option<String>,
    pub document_id: Option<Uuid>,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    5
}

impl Search {
    pub fn validate(&self) -> Result<()> {
        if self.query.trim().is_empty() || self.query.len() > 1000 || self.query.contains('\0') {
            bail!("query must contain 1–1000 bytes of text");
        }
        if !(1..=20).contains(&self.limit) {
            bail!("limit must be between 1 and 20");
        }
        Ok(())
    }
}

const SECTION_COLUMNS: &str = "c.id,c.document_id,c.job_id,c.ordinal,d.title,d.source_key,d.game,d.edition,c.heading,c.page,c.text";

impl Store {
    pub async fn connect(url: &str, root: PathBuf) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(5))
            .connect(url)
            .await
            .context("Cannot connect to knowledge database")?;
        sqlx::migrate!()
            .run(&pool)
            .await
            .context("Cannot migrate knowledge database")?;
        tokio::fs::create_dir_all(&root).await?;
        Ok(Self {
            pool,
            root: root.canonicalize()?,
        })
    }

    pub fn original_path(&self, document: Uuid, job: Uuid, filename: &str) -> PathBuf {
        // All path segments are generated UUIDs or a validated extension. The
        // uploaded filename and source_key never become filesystem paths.
        self.root.join(document.to_string()).join(format!(
            "{job}.{}",
            extract::extension(filename).expect("validated extension")
        ))
    }

    pub async fn enqueue(
        &self,
        metadata: ImportMetadata,
        filename: &str,
        bytes: &[u8],
    ) -> Result<Job> {
        metadata.validate()?;
        extract::extension(filename)?;
        if bytes.is_empty() || bytes.len() > MAX_UPLOAD {
            bail!("File must contain 1–10 MiB of data");
        }
        if filename.len() > 500 || filename.contains('\0') {
            bail!("Invalid filename");
        }
        let checksum = format!("{:x}", Sha256::digest(bytes));
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(
                &metadata,
                &checksum,
                extract::extension(filename)?,
                "extract-v1",
            ))?)
        );
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO documents(id,source_key,title,game,edition) VALUES($1,$2,$3,$4,$5) ON CONFLICT(source_key) DO NOTHING")
            .bind(Uuid::new_v4()).bind(&metadata.source_key).bind(&metadata.title).bind(&metadata.game).bind(&metadata.edition)
            .execute(&mut *tx).await?;
        // Serialize imports for one logical source, including concurrent duplicate uploads.
        let document: Uuid =
            sqlx::query_scalar("SELECT id FROM documents WHERE source_key=$1 FOR UPDATE")
                .bind(&metadata.source_key)
                .fetch_one(&mut *tx)
                .await?;
        let last: Option<(Uuid, String, String)> = sqlx::query_as("SELECT id,fingerprint,state FROM ingestion_jobs WHERE document_id=$1 ORDER BY sequence DESC LIMIT 1")
            .bind(document).fetch_optional(&mut *tx).await?;
        if let Some((id, old_fingerprint, state)) = last {
            if fingerprint == old_fingerprint && state != "failed" {
                tx.commit().await?;
                return self.job(id).await?.context("Duplicate import disappeared");
            }
        }
        let id = Uuid::new_v4();
        let path = self.original_path(document, id, filename);
        tokio::fs::create_dir_all(path.parent().unwrap()).await?;
        let temporary = path.with_extension("upload");
        tokio::fs::write(&temporary, bytes).await?;
        tokio::fs::rename(&temporary, &path).await?;
        let result = async {
            sqlx::query("INSERT INTO ingestion_jobs(id,document_id,fingerprint,checksum,filename,title,game,edition) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
                .bind(id).bind(document).bind(fingerprint).bind(checksum).bind(filename)
                .bind(metadata.title).bind(metadata.game).bind(metadata.edition).execute(&mut *tx).await?;
            tx.commit().await?;
            anyhow::Ok(())
        }.await;
        if let Err(e) = result {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(e);
        }
        self.job(id).await?.context("New import disappeared")
    }

    pub async fn job(&self, id: Uuid) -> Result<Option<Job>> {
        Ok(sqlx::query_as(
            "SELECT id,document_id,state,error,section_count FROM ingestion_jobs WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn documents(&self) -> Result<Vec<Document>> {
        Ok(sqlx::query_as("SELECT d.*,j.id AS latest_job_id,j.state AS latest_state,j.error AS latest_error FROM documents d JOIN LATERAL (SELECT * FROM ingestion_jobs WHERE document_id=d.id ORDER BY sequence DESC LIMIT 1) j ON true ORDER BY d.created_at,d.id")
            .fetch_all(&self.pool).await?)
    }

    pub async fn sections(&self, document: Uuid) -> Result<Vec<SourceSection>> {
        Ok(sqlx::query_as(&format!("SELECT {SECTION_COLUMNS} FROM chunks c JOIN documents d ON d.id=c.document_id AND d.active_job_id=c.job_id WHERE d.id=$1 ORDER BY c.ordinal"))
            .bind(document).fetch_all(&self.pool).await?)
    }

    pub async fn read(&self, section: Uuid, surrounding: i32) -> Result<Vec<SourceSection>> {
        if !(0..=2).contains(&surrounding) {
            bail!("surrounding_sections must be between 0 and 2");
        }
        Ok(sqlx::query_as(&format!("SELECT {SECTION_COLUMNS} FROM chunks c JOIN documents d ON d.id=c.document_id AND d.active_job_id=c.job_id JOIN chunks target ON target.job_id=c.job_id WHERE target.id=$1 AND c.ordinal BETWEEN target.ordinal-$2 AND target.ordinal+$2 ORDER BY c.ordinal"))
            .bind(section).bind(surrounding).fetch_all(&self.pool).await?)
    }

    pub async fn search(&self, search: &Search) -> Result<Vec<SearchHit>> {
        search.validate()?;
        Ok(sqlx::query_as(&format!("SELECT {SECTION_COLUMNS},ts_rank_cd(c.search_vector,q.query) AS rank FROM chunks c JOIN documents d ON d.id=c.document_id AND d.active_job_id=c.job_id CROSS JOIN websearch_to_tsquery('english',$1) q(query) WHERE c.search_vector @@ q.query AND ($2::text IS NULL OR d.game=$2) AND ($3::text IS NULL OR d.edition=$3) AND ($4::uuid IS NULL OR d.id=$4) ORDER BY rank DESC,c.id LIMIT $5"))
            .bind(&search.query).bind(&search.game).bind(&search.edition).bind(search.document_id).bind(search.limit)
            .fetch_all(&self.pool).await?)
    }

    pub async fn original(&self, document: Uuid) -> Result<Option<(String, Vec<u8>)>> {
        // Keep the document locked until read completes, so delete cannot remove the file halfway through.
        let mut tx = self.pool.begin().await?;
        let row: Option<(Uuid, String)> = sqlx::query_as("SELECT j.id,j.filename FROM documents d JOIN ingestion_jobs j ON j.id=d.active_job_id WHERE d.id=$1 FOR SHARE OF d")
            .bind(document).fetch_optional(&mut *tx).await?;
        let result = match row {
            Some((job, filename)) => {
                let bytes = tokio::fs::read(self.original_path(document, job, &filename)).await?;
                Some((filename, bytes))
            }
            None => None,
        };
        tx.commit().await?;
        Ok(result)
    }

    pub async fn delete(&self, document: Uuid) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        // Lock before clearing the circular active-version FK and deleting cascades.
        let found: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
                .bind(document)
                .fetch_optional(&mut *tx)
                .await?;
        if found.is_none() {
            return Ok(false);
        }
        sqlx::query("UPDATE documents SET active_job_id=NULL WHERE id=$1")
            .bind(document)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM documents WHERE id=$1")
            .bind(document)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO file_cleanup(document_id) VALUES($1) ON CONFLICT DO NOTHING")
            .bind(document)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.cleanup(document).await?;
        Ok(true)
    }

    async fn cleanup(&self, document: Uuid) -> Result<()> {
        let dir = self.root.join(document.to_string());
        match tokio::fs::remove_dir_all(dir).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("Original cleanup pending; worker will retry"),
        }
        sqlx::query("DELETE FROM file_cleanup WHERE document_id=$1")
            .bind(document)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// One session-scoped database lock protects recovery and enforces one worker
    /// even when two service processes accidentally point at the same database.
    pub async fn worker(&self) -> Result<()> {
        // Detach so dropping this future closes the session, releasing its lock;
        // returning a locked session to the pool would leak ownership.
        let mut connection = self.pool.acquire().await?.detach();
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(74219001)")
            .fetch_one(&mut connection)
            .await?;
        if !locked {
            bail!("Another knowledge worker owns this database");
        }
        sqlx::query("UPDATE ingestion_jobs SET state='queued',error=NULL WHERE state='running'")
            .execute(&mut connection)
            .await?;
        loop {
            // Check the lock's connection is still alive. Losing it must stop the worker.
            sqlx::query("SELECT 1").execute(&mut connection).await?;
            let cleanup: Vec<Uuid> =
                sqlx::query_scalar("SELECT document_id FROM file_cleanup LIMIT 10")
                    .fetch_all(&mut connection)
                    .await?;
            for document in cleanup {
                if let Err(error) = self.cleanup(document).await {
                    tracing::warn!(%error, %document, "Original cleanup pending");
                }
            }
            let pending: Option<(Uuid, Uuid, String)> = sqlx::query_as("UPDATE ingestion_jobs SET state='running',error=NULL WHERE id=(SELECT id FROM ingestion_jobs WHERE state='queued' ORDER BY sequence LIMIT 1) RETURNING id,document_id,filename")
                .fetch_optional(&mut connection).await?;
            if let Some((job, document, filename)) = pending {
                if let Err(error) = self.index(job, document, filename).await {
                    tracing::warn!(%job, %error, "Import failed");
                    sqlx::query("UPDATE ingestion_jobs SET state='failed',error=$2,finished_at=now() WHERE id=$1")
                        .bind(job).bind(format!("{error:#}").chars().take(1000).collect::<String>()).execute(&mut connection).await?;
                }
            } else {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }

    async fn index(&self, job: Uuid, document: Uuid, filename: String) -> Result<()> {
        let bytes = tokio::fs::read(self.original_path(document, job, &filename)).await?;
        let sections =
            tokio::task::spawn_blocking(move || extract::extract(&filename, &bytes)).await??;
        let mut tx = self.pool.begin().await?;
        let found: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
                .bind(document)
                .fetch_optional(&mut *tx)
                .await?;
        if found.is_none() {
            return Ok(());
        } // Source was deleted during extraction.
        sqlx::query("DELETE FROM chunks WHERE document_id=$1")
            .bind(document)
            .execute(&mut *tx)
            .await?;
        for (n, section) in sections.iter().enumerate() {
            sqlx::query("INSERT INTO chunks(id,document_id,job_id,ordinal,heading,page,text) VALUES($1,$2,$3,$4,$5,$6,$7)")
                .bind(Uuid::new_v4()).bind(document).bind(job).bind(n as i32).bind(&section.heading).bind(section.page).bind(&section.text)
                .execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE documents d SET active_job_id=j.id,title=j.title,game=j.game,edition=j.edition FROM ingestion_jobs j WHERE d.id=$1 AND j.id=$2")
            .bind(document).bind(job).execute(&mut *tx).await?;
        sqlx::query("UPDATE ingestion_jobs SET state='succeeded',section_count=$2,finished_at=now() WHERE id=$1")
            .bind(job).bind(sections.len() as i32).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}

# Jarvis knowledge: inspect retrieval before adding an agent

This is the first runnable knowledge milestone (M1), with an early read-only MCP integration. It runs independently of Jarvis Desktop, OpenCode, and the future optional Jarvis Server. Desktop still works as before.

It imports UTF-8 text/Markdown and text-based PDFs, keeps originals, indexes sections with PostgreSQL full-text search, and returns the evidence behind a search. The knowledge service does **not** generate answers or call cloud APIs. OpenCode can use its read-only MCP tools to answer with your existing provider. Semantic search, personal facts, Jarvis Server, and the browser pet are later milestones.

## How to run

Requires Rust stable and Docker (Linux containers). Run from `services/knowledge`.

1. Copy `.env.example` to `.env` and replace `JARVIS_KNOWLEDGE_TOKEN` with a random value of at least 24 characters. On Windows: `copy .env.example .env`; on macOS/Linux: `cp .env.example .env`.
2. Start PostgreSQL, then run the Rust service locally:

```sh
docker compose up -d --wait
cargo run --locked -- serve
```

The service listens on `http://127.0.0.1:47831` and applies migrations automatically. Docker Compose runs **only PostgreSQL with pgvector**. Ctrl+C stops the service; `docker compose stop` stops the database and preserves its volume.

Server and CLI commands load `.env` from the current directory. No shell exports are needed. Existing process environment variables take precedence. `.env` and `.data/` are ignored by Git; `.env.example` is committed. Relative data paths resolve from the current directory. Database credentials are for local development; changing them does not change an existing database volume's credentials.

## Try retrieval

In another terminal in this directory:

```sh
cargo run --locked -- capabilities
cargo run --locked -- import tests/fixtures/harbor-rules.md --source-key harbor/base --title "Harbor Traders rules" --game "Harbor Traders" --edition base --wait
cargo run --locked -- search "trade spice" --game "Harbor Traders" --edition base
cargo run --locked -- search "dragon combat"
```

The fictional rulebook returns the Trading section: three copper coins buy one spice crate, once per turn. The second search returns no hits. These results are retrieved evidence; no model generates an answer.

Substitute the UUIDs returned by the commands:

```sh
cargo run --locked -- documents
cargo run --locked -- sections DOCUMENT_UUID
cargo run --locked -- read SECTION_UUID --surrounding 1
cargo run --locked -- original DOCUMENT_UUID --output downloaded-rules.md
cargo run --locked -- delete DOCUMENT_UUID
```

Import changed content with the same `--source-key` to update a source. To try PDF page references:

```sh
cargo run --locked -- import tests/fixtures/harbor-rules.pdf --source-key harbor/pdf --title "Harbor PDF rules" --edition pdf-demo --wait
cargo run --locked -- search lighthouse --edition pdf-demo
```

The hit points to page 2. Original downloads preserve uploaded bytes. Scanned pages require OCR, which is not implemented; PDF table/layout extraction can be imperfect.

## How it works

```text
file + metadata -> immutable original -> persisted import job
  -> one extraction worker -> headings/pages -> sections (~1600 characters)
  -> atomic active-version update -> PostgreSQL English full-text index

question -> English keyword query + exact metadata filters
  -> ranked sections -> inspect text -> read neighbors or download original
```

The `game` and `edition` filters are exact matches. English stemming handles words such as “starts” and “starting”; it does not provide semantic understanding. PostgreSQL's web-search query syntax supports quoted phrases and `OR`. Asking a long natural-language question may return nothing even when a paraphrased answer exists. M2 will compare this baseline with embeddings and hybrid retrieval.

Uploads are limited to 10 MiB, sections to 10,000 per document, searches to 20 hits, and neighboring reads to two sections each side. One worker handles extraction at a time; the database pool has four connections plus a dedicated worker connection. The dev database has a 512 MiB container limit. These are initial bounds, not a measured mini-PC capacity claim. PDF parsing is in-process and trusted-local-input only in this development slice; a small compressed PDF can still expand to substantial memory.

Each logical source has its own history of import jobs. Failed replacements preserve the last successful text and metadata. Original files for older versions remain until source deletion. Jobs recover after process restart. One worker lock prevents two processes from indexing the same database. Keep the database and its original-file directory together when copying or backing them up. Do not point different service instances at one database with different file roots.

## Test with the existing OpenCode/cloud account

After importing `harbor/base` and with the knowledge service running (Python 3.10+):

The test reads the same `.env` configuration.

```sh
python tests/opencode_smoke.py
```

This optional live test uses the **already installed** OpenCode service described in `~/.local/state/opencode/service.json`. It installs no OpenCode binary and copies no provider credentials. It chooses the first available OpenAI model from OpenCode's existing model list, or accepts `--model provider/id`. Only the fictional rulebook question and retrieved passages are sent to that provider.

The test creates a temporary project configuration and session, connects the authenticated `/mcp` endpoint, permits only search/read tools, asks one question, and checks for real tool traces and a source citation. It then removes its session, evicts that temporary location, and deletes the temporary configuration. It never edits the global OpenCode config or the Desktop's session workspace. Python 3.10+ is needed only for this optional test.

For OpenCode **2.0.26**, the test sets `mcp.servers.jarvis_knowledge.codemode` to `false`. This exposes the two tools directly while general `execute` stays denied. With default Code Mode, denying `execute` also hides MCP tools routed through it. This is a v2-specific setting; use the installed API and [v2 MCP documentation](https://dev.opencode.ai/v2/docs/mcp-servers/), not v1 config examples.

This demonstrates agent-selected **keyword retrieval**, without embeddings. The MCP endpoint exposes `search_knowledge` and `read_section`; file imports and deletion stay in the management API. It does not yet implement M4's full personal-memory agent or outage/evaluation matrix. The home-server model/provider is still undecided.

## Verification

```sh
cargo test --locked
docker compose up -d --wait
cargo test --locked --test retrieval -- --ignored --nocapture
cargo clippy --locked --all-targets -- -D warnings
```

The real-database test creates a uniquely named test database, exercises imports/search/replacement/recovery/deletion and HTTP authentication, then drops that test database. It never resets the development corpus. The PostgreSQL account needs CREATE DATABASE permission (the dev account has it). Set `JARVIS_KNOWLEDGE_TEST_DATABASE_URL` in `.env` to choose a different test PostgreSQL instance. A failed test may leave its `jarvis_test_*` database for inspection.

Stop the development database without deleting its data:

```sh
docker compose stop
```

Full API and component ownership: `../../docs/AGENTIC_RAG_CONTRACTS.md`. Roadmap: `../../plan.agentic-rag.md`.

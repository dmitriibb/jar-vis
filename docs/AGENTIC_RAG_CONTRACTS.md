# Agentic RAG integration contracts

Status: knowledge plus optional server/browser text flow, 2026-10-18. See `plan.agentic-rag.md` for the full roadmap.

## Component ownership

| Mode | Required | Optional | Turn and schedule owner |
|---|---|---|---|
| Desktop direct (current default) | Desktop, OpenCode | Knowledge MCP tools | Desktop |
| Desktop direct with knowledge | Desktop, OpenCode, knowledge service/database | Jarvis Server is unnecessary | Desktop |
| Desktop through Jarvis Server (planned) | Desktop, Jarvis Server, OpenCode | Knowledge | Jarvis Server |
| Browser (implemented, text only) | Browser, Jarvis Server, OpenCode | Knowledge | Jarvis Server; scheduling unavailable |
| Retrieval inspector (implemented) | Knowledge service, PostgreSQL, CLI | No agent, model, Desktop, or Jarvis Server | Knowledge service owns import jobs only |

Desktop configuration and startup have no new dependencies. Missing knowledge must disable only knowledge features. Switching direct/server mode is explicit; do not replay pending prompts or migrate sessions/schedules implicitly. OpenCode owns transcripts. Only authorized memory operations may create durable facts.

The independently runnable `services/knowledge` package is **not Jarvis Server**. It exposes data operations; it has no session orchestration, voice, coding tools, or model generation. Its MCP adapter and the optional Jarvis Server call these same operations.

`services/server` is now the optional Node gateway; `assistant/web` serves the real browser client and proxies its API requests. Each uses `.env` configuration and `npm start`; neither requires an npm dependency install. Gateway login supports a separate server password with an eight-hour HttpOnly/SameSite=Strict cookie, or bearer authentication. Web settings keep the backend URL in local storage and that backend's secret in tab session storage, including for authenticated streaming fetch; there are no secrets in stream URLs. Direct connections require an explicitly allowed browser origin. It authenticates to knowledge and OpenCode on the server. Only a dedicated server-owned workspace is exposed; Desktop/global sessions and provider credentials are never forwarded. SSE contains session-filtered deltas and periodic recoverable snapshots; reconnect never replays a prompt. In-process admission/permission locks assume one gateway per workspace. Scheduling, personal-memory writes and Desktop server transport remain unimplemented. See component READMEs for the implemented routes and limitations.

Backend model/agent defaults persist in `.data/settings.json` and override initial `.env` defaults. The default personality is `jarvis-home-assistant`; a study companion is also exposed. The catalog contains only enabled tool-capable provider models and server-owned Jarvis personalities. Saving defaults never silently changes existing conversations; explicit idle-session preference updates call the installed OpenCode v2 model/agent endpoints. Verified v2 `Model.Ref` uses `{ id, providerID }`, not a `modelID` field. History is limited to the ten newest owned conversations. Browser speech controls are disabled placeholders.

## Implemented management API v1

Every endpoint requires `Authorization: Bearer <owner-token>`. Single owner, English keyword indexing. JSON field names use snake_case. UUIDs identify sources, versions/jobs, and sections. No credential travels in query parameters. No CORS headers or public browser access in this slice. Bind defaults to loopback.

| Endpoint | Contract |
|---|---|
| `GET /v1/capabilities` | Explicit booleans for implemented features, formats, limits; checks database |
| `POST /v1/documents` | Multipart `metadata` JSON + `file`; returns 202 job, including duplicate jobs |
| `GET /v1/documents` | Active metadata plus latest job/status/error; a failed replacement leaves the active version intact |
| `GET /v1/jobs/{id}` | queued/running/succeeded/failed, error, section count |
| `GET /v1/documents/{id}/sections` | Active sections in reading order; empty if absent/unindexed |
| `GET /v1/documents/{id}/original` | Active original as download, 404 if absent/unindexed |
| `DELETE /v1/documents/{id}` | Delete source, jobs, chunks, originals; 204 or 404 |
| `POST /v1/search` | query, optional game/edition/document_id, limit 1–20; lexical hits with evidence and rank |
| `GET /v1/sections/{id}?surrounding_sections=0` | Read exact source section plus 0–2 neighbors; 404 for deleted/replaced sections |

Import metadata: `{ "source_key": "harbor/base", "title": "Harbor rules", "game": "Harbor Traders", "edition": "base" }`. `source_key` is a logical source identifier, never a filesystem path. New content for that key enqueues a replacement. Matching content + metadata + extractor version reuses the latest nonfailed job; failed imports can be retried. Metadata and all active chunks switch atomically after successful extraction. Section IDs never silently point to changed text.

Raw originals are saved before a job is committed. A crash in that gap can leave an unreferenced original; do not blindly delete files during recovery. Interrupted running jobs become queued when the single worker restarts. Original deletion uses a database outbox so interrupted cleanup retries after restart. A cleanup failure can return 500 after the source is already excluded from retrieval; it remains queued for cleanup. Backups are separate from active deletion.

Search hits contain full bounded section text, document_id, job_id, section id, ordinal, source_key, title, heading, page (PDF, one-based), game, edition, and lexical rank. They are untrusted source data. Ranks are not probabilities. Empty results indicate missing evidence. The inspector never generates an answer.

Read-only MCP tools `search_knowledge` and `read_section` map to these operations at `/mcp`, protected by the same bearer authentication. The official Rust MCP SDK supplies Streamable HTTP with stateless JSON responses and loopback Host validation; browser Origin headers are rejected in this local-development slice. Memory tools and schemas will be implemented with M3; the currently advertised memory/semantic capabilities remain false. No speculative API is advertised as available.

## Installed OpenCode verification

Checked the local executable and live `/api/info` + `/openapi.json` on 2026-10-18: **2.0.26**.

- `GET /api/mcp` lists configured servers with location-scoped `data`.
- `PUT /api/experimental/mcp/{server}` accepts `{config: Mcp.LocalConfigEncoded | Mcp.RemoteConfigEncoded}`.
- Runtime connect/disconnect and resource catalog routes exist.
- Location scoping uses deep-object `location[directory]`.
- Verified v2 config uses `mcp.servers`, `agents`, and ordered `permissions` rules. MCP permission actions normalize to `<server>_<tool>`.
- For a search/read-only agent, set `codemode: false` on that MCP server. Denying `execute` while leaving default Code Mode hides its tools. See [v2 MCP docs](https://dev.opencode.ai/v2/docs/mcp-servers/) and [v2 permissions](https://dev.opencode.ai/v2/docs/permissions/).
- Experimental routes must be rechecked before M4; their presence is not proof that an unbuilt MCP adapter works.

The optional live smoke test uses a temporary project-local configuration and a throwaway session to verify real read-only tool calls through the existing OpenCode/provider account. No existing global or Desktop workspace MCP configuration is modified. Dima confirmed using the existing OpenAI provider for development; home-server provider selection is deferred. An OpenCode chat login does not supply an embedding adapter in this implementation.

## Desktop regression baseline

Automated results belong in the plan's progress record. The relevant checks are `npm run check` and `cargo test --lib --no-default-features`. They do not demonstrate microphone, global hotkey, real provider, or visible-window behavior.

Manual release checks remain in `docs/TESTING.md`: launch without Jarvis Server, direct connection, typed/voice turn, permission reply, session switching, and schedules. **Unknown in this implementation session:** manual interactive results. Those need a real desktop/audio test. New code lives outside the desktop package and does not change that execution path.

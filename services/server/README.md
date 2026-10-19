# Jarvis Server

Optional headless gateway for Jarvis Web. Uses your **existing OpenCode 2.x installation** and provider account. Desktop continues to connect to OpenCode directly without this server.

## How to run

Requires Node.js 22.16+ (tested on Node 24). No npm dependencies or installation step.

From `services/server`, copy `.env.example` to `.env` (`copy .env.example .env` on Windows, `cp .env.example .env` on macOS/Linux). Replace `JARVIS_SERVER_TOKEN` with a random password of at least 24 characters.

```sh
opencode service start
npm start
```

Listens on `http://127.0.0.1:47832`. Start [Jarvis Web](../../assistant/web/README.md) separately, then enter `JARVIS_SERVER_TOKEN` in its login form. The server password differs from the knowledge-service token. Ctrl+C stops the gateway.

The server discovers OpenCode through its existing `~/.local/state/opencode/service.json` (or `XDG_STATE_HOME`). Alternatively set `OPENCODE_URL`, `OPENCODE_USERNAME`, and `OPENCODE_PASSWORD` in `.env`. It never installs OpenCode, starts a second OpenCode instance, or copies provider credentials.

Initial defaults come from `OPENCODE_MODEL=provider/model` and `JARVIS_DEFAULT_AGENT=jarvis-home-assistant`. If the initial model is blank, it uses the configured OpenCode default from the existing Desktop session directory, then the first enabled tool-capable OpenAI model (or another enabled tool-capable model).

In Web, open **Settings → Backend** to choose the default model and agent from the available catalog. Saved UI choices live in ignored `.data/settings.json`, override these initial `.env` defaults, survive restarts, and are shared by clients of this backend. New sessions use them. The optional “also apply” checkbox changes the current idle conversation through OpenCode's model/agent APIs; busy sessions reject changes. Personalities currently include `jarvis-home-assistant` and `jarvis-study-companion`, both restricted to read-only knowledge tools. Existing `jarvis-web` sessions retain a compatibility alias.

## Optional knowledge

By default the gateway reads `../knowledge/.env` on the server. Start the [knowledge service](../knowledge/README.md) to enable document management and MCP retrieval. You can override its URL/token in the server `.env`; set `JARVIS_KNOWLEDGE_ENV_FILE=` to disable automatic loading. Chat works without knowledge or PostgreSQL.

The generated `.data/workspace/opencode.json` configures the dedicated Jarvis agents with read-only knowledge tools, an eight-step turn limit, and no shell/filesystem/web tools. It includes the local knowledge token; `.data/` is Git-ignored and should be private. This workspace is server-owned; don't edit the generated config. Desktop/global OpenCode configuration is untouched. The gateway reconnects a failed knowledge MCP connection when its health check succeeds.

Ordinary conversations stay in OpenCode. Documents and import jobs stay in knowledge/PostgreSQL. The gateway has no new database. Cookie-only login sessions live in memory and expire on restart. The browser settings flow also supports bearer authentication using a secret kept in that tab's session storage, which reconnects after restart. Chat sessions survive because OpenCode owns them.

## API and behavior

- `POST /api/auth` with `{ "token": "..." }` creates an eight-hour HttpOnly, SameSite=Strict cookie; `GET` checks login; `DELETE` logs out. Trusted CLI clients can instead send the server token as a bearer header.
- `GET /api/status` reports OpenCode/knowledge availability and the model; memory and schedules are explicitly unavailable.
- `GET/PUT /api/settings` reads the model/agent catalog and saves validated shared defaults. It returns no connection secrets.
- `GET/POST /api/sessions` lists the ten latest conversations / creates a conversation in the dedicated workspace only.
- `PUT /api/sessions/:id/preferences` explicitly applies a catalog-validated model/agent pair to an idle session.
- `GET /api/sessions/:id/snapshot` recovers current messages, tool names/states, activity, and pending permissions.
- `GET /api/sessions/:id/events` streams filtered token deltas plus periodic snapshots. Multiple clients share one upstream stream per viewed session. Reconnect fetches state; it never resubmits a prompt.
- `POST /api/sessions/:id/prompt` accepts `{ "text": "..." }`; active sessions reject another prompt with 409. An ambiguous network error is not retried automatically: inspect the conversation before resubmitting.
- `POST /api/sessions/:id/interrupt` stops a turn. `POST /api/sessions/:id/permissions/:requestId` accepts `{ "decision": "once|always|reject" }`; concurrent replies are serialized and a stale reply returns 409.
- `DELETE /api/sessions/:id` interrupts then deletes the conversation.
- `/api/knowledge/...` proxies only the existing document/job/search/section routes with the server-held knowledge token. Uploads are capped; original files download as attachments. There is no general OpenCode/knowledge proxy.

Direct browser connections configured in Web settings use bearer headers and streaming fetch. Exact origins in `JARVIS_WEB_ORIGINS` receive CORS/preflight headers; other origins are rejected. Credentials never travel in an event-stream URL. Use HTTPS for remote connections; an HTTPS page cannot connect to an HTTP backend.

Run one gateway process per workspace; admission/permission serialization is in-process. Browser reads cannot access Desktop's sessions or arbitrary OpenCode locations. The dedicated agent currently permits only knowledge search/read, so it normally won't ask for execution permission. Permission UI/routes support OpenCode pending requests; they do not enable coding tools.

## Laptop / home server

For LAN development set `JARVIS_WEB_HOST=0.0.0.0` in the **web** `.env`, and add the exact browser origin (for example `http://mini-pc:47833`) to this server's `JARVIS_WEB_ORIGINS`. Open that address on your laptop. Gateway and knowledge can remain bound to loopback because the web process proxies requests. PostgreSQL remains loopback-only.

Use a trusted LAN/VPN; HTTP development traffic is unencrypted. For remote deployment put Web behind HTTPS and set `JARVIS_COOKIE_SECURE=true`. Do not expose OpenCode or PostgreSQL directly. Full deployment/backup/resource verification remains M8 work.

## Tests

```sh
npm test
```

For an opt-in real-provider test, start Web, Server, OpenCode and knowledge first:

```sh
npm run test:live
```

This sends only a fictional rulebook question/passages to your existing cloud model. It creates and removes its own temporary document and OpenCode session, checks upload/progress/original/source reads, real MCP tools, correct citation, and token/snapshot events. It does not delete existing documents or sessions.

This is the first working server/web text flow. Browser voice, personal fact editing, server-owned scheduling, Desktop server transport, semantic embeddings, and cross-site pairing remain separate work.

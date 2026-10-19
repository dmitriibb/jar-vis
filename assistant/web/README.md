# Jarvis Web

Lightweight browser companion: floating pet, expandable chat, sessions, live answers, stop, permission replies, document uploads/import status, and clickable source citations. No Tauri or browser framework is required; the Node process serves static assets and proxies `/api` to Jarvis Server. No provider or knowledge credentials are shipped in assets.

## How to run

Requires Node.js 22.16+ and a running [Jarvis Server](../../services/server/README.md). No npm dependencies or installation step.

From `assistant/web`, copy `.env.example` to `.env` (`copy .env.example .env` on Windows, `cp .env.example .env` on macOS/Linux), then:

```sh
npm start
```

Open [http://localhost:47833](http://localhost:47833), click the pet, and enter `JARVIS_SERVER_TOKEN` from the **server** `.env`. The compact header contains four controls: recent sessions (ten latest), new session, settings, and minimize. Shift+Enter adds a new line. The Knowledge tab imports `.txt`, `.md`, and text-based `.pdf` files; reuse a source key to replace a document.

`.env` files are local and Git-ignored; `.env.example` files are committed. Restart after `.env` changes; UI default changes apply without a restart. Transcripts remain in OpenCode. There is no browser voice or personal fact editor yet.

## Settings

- **Connection:** backend URL and API key/secret for this browser. Leave URL blank to use the web process's configured proxy, or enter a Jarvis Server origin such as `http://mini-pc:47832`. The URL is saved in local storage; the secret is saved only in this tab's session storage, bound to that URL. Changing the URL requires entering that backend's secret. Disconnect clears the saved secret. This field accepts a Jarvis Server secret, not a provider API key.
- **Backend:** available models from your existing OpenCode provider and Jarvis agent personalities. Defaults are shared by the connected backend and persist across restarts. The default agent is `jarvis-home-assistant`; `jarvis-study-companion` is another personality. You can apply selections to the current idle conversation as well as future sessions.
- **Speech:** a reserved section for future speech-to-text and text-to-speech settings. Its controls are disabled until those features exist.

For a direct backend URL, allow this Web page's exact origin in the server's `JARVIS_WEB_ORIGINS`. Remote connections should use HTTPS. The app streams with authenticated fetch, so no secret is placed in an SSE URL. No connection secrets are returned by the backend settings API or shipped in static assets.

## Embed on your own site

For same-site pages, add the site's exact origin to `JARVIS_EMBED_ORIGINS` in Web's `.env` and restart Web. Include:

```html
<script src="http://localhost:47833/embed.js" defer></script>
```

Use your actual Jarvis URL when hosting elsewhere. The loader inserts an isolated iframe and resizes it when the pet opens/closes. It validates both the sender origin and iframe window. The loader/host page receives no chat data or passwords. Framing is denied by default except on the same origin and your configured allowlist. Host CSP must allow the Jarvis script and iframe; use matching HTTPS schemes on HTTPS sites.

**Cross-site pairing is not implemented or verified.** Same-site embedding (for example ports on `localhost`, or same-scheme subdomains of one domain) remains the supported loader setup. Manual bearer connections do not establish compatibility with every site's CSP or browser storage restrictions; use the standalone page when embedding is blocked. Pairing/extension support remains M7; this loader is not a claim of working on arbitrary sites.

## Tests and size

```sh
npm test
```

There is no bundle/build step: the runtime serves the HTML, one CSS file, one JS module, and a small SVG pet. No CDN fonts, analytics, external scripts, or browser dependencies. API proxy tests check public-asset isolation, frame policy, and credential forwarding; the server's opt-in live test checks the full cloud/MCP path.

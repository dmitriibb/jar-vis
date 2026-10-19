import http from 'node:http';
import https from 'node:https';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

export function webServer({ serverURL, embedOrigins = [] }) {
  const target = new URL(serverURL);
  if (!['http:', 'https:'].includes(target.protocol)) throw new Error('JARVIS_SERVER_URL must be HTTP(S).');
  for (const origin of embedOrigins) if (new URL(origin).origin !== origin) throw new Error('Use exact origins in JARVIS_EMBED_ORIGINS.');
  const assets = new Map([['/', ['index.html', 'text/html']], ['/app.js', ['app.js', 'text/javascript']], ['/style.css', ['style.css', 'text/css']], ['/pet.svg', ['pet.svg', 'image/svg+xml']], ['/embed.js', ['embed.js', 'text/javascript']]]);
  return http.createServer(async (req, res) => {
    res.setHeader('X-Content-Type-Options', 'nosniff');
    res.setHeader('Referrer-Policy', 'strict-origin');
    res.setHeader('Cache-Control', 'no-store');
    const url = new URL(req.url, 'http://jarvis.invalid');
    if (url.pathname.startsWith('/api/')) {
      const headers = { ...req.headers, host: target.host };
      delete headers.connection;
      const proxy = (target.protocol === 'https:' ? https : http).request(new URL(url.pathname + url.search, target), { method: req.method, headers }, upstream => {
        res.writeHead(upstream.statusCode, upstream.headers);
        upstream.pipe(res);
        res.on('close', () => upstream.destroy());
      });
      proxy.on('error', () => { if (!res.headersSent) { res.writeHead(502, { 'Content-Type': 'application/json' }); res.end('{"error":"Jarvis Server is offline. Start it and reconnect."}'); } else res.end(); });
      proxy.setTimeout(60000, () => proxy.destroy());
      req.on('aborted', () => proxy.destroy());
      res.on('close', () => proxy.destroy());
      req.pipe(proxy); return;
    }
    res.setHeader('Content-Security-Policy', `default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self' http: https:; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'self' ${embedOrigins.join(' ')}`);
    if (url.pathname === '/public-config.json' && req.method === 'GET') {
      res.writeHead(200, { 'Content-Type': 'application/json' }); res.end(JSON.stringify({ embedOrigins })); return;
    }
    const asset = assets.get(url.pathname);
    if (!asset || !['GET', 'HEAD'].includes(req.method)) { res.writeHead(404); res.end('Not found'); return; }
    try {
      const content = await readFile(new URL(`public/${asset[0]}`, import.meta.url));
      res.writeHead(200, { 'Content-Type': `${asset[1]}; charset=utf-8` }); res.end(req.method === 'HEAD' ? undefined : content);
    } catch { res.writeHead(500); res.end('Could not read browser assets.'); }
  });
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const host = process.env.JARVIS_WEB_HOST || '127.0.0.1';
  const port = Number(process.env.JARVIS_WEB_PORT || 47833);
  const server = webServer({ serverURL: process.env.JARVIS_SERVER_URL || 'http://127.0.0.1:47832', embedOrigins: (process.env.JARVIS_EMBED_ORIGINS || '').split(',').map(s => s.trim()).filter(Boolean) });
  server.listen(port, host, () => console.log(`Jarvis Web: http://${host}:${port}`));
  server.on('error', error => { console.error(`Jarvis Web could not listen (${error.code || 'unknown error'}).`); process.exitCode = 1; });
  for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => { server.close(); server.closeAllConnections(); });
}

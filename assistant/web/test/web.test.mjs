import { test } from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { once } from 'node:events';
import { webServer } from '../dev.mjs';

test('serves only public assets, denies framing by default, and proxies cookies and live responses', async t => {
  const upstream = http.createServer((req, res) => { res.writeHead(200, { 'Content-Type': 'application/json', 'Set-Cookie': 'jarvis_session=test; HttpOnly' }); res.end(JSON.stringify({ origin: req.headers.origin, cookie: req.headers.cookie, url: req.url })); });
  upstream.listen(0, '127.0.0.1'); await once(upstream, 'listening');
  const server = webServer({ serverURL: `http://127.0.0.1:${upstream.address().port}` });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  t.after(() => { server.close(); server.closeAllConnections(); upstream.close(); upstream.closeAllConnections(); });
  const base = `http://127.0.0.1:${server.address().port}`;
  const home = await fetch(base);
  assert.equal(home.status, 200); assert.match(home.headers.get('content-security-policy'), /frame-ancestors 'self'/);
  assert.match(await home.text(), /Your knowledge/);
  for (const asset of ['/app.js', '/style.css', '/pet.svg', '/embed.js']) assert.equal((await fetch(base + asset)).status, 200);
  for (const asset of ['/.env', '/package.json', '/dev.mjs', '/%2e%2e/.env']) assert.equal((await fetch(base + asset)).status, 404);
  const response = await fetch(base + '/api/sessions', { headers: { Origin: base, Cookie: 'jarvis_session=test' } });
  assert.ok(response.headers.get('set-cookie').includes('HttpOnly'));
  assert.deepEqual(await response.json(), { origin: base, cookie: 'jarvis_session=test', url: '/api/sessions' });
  // Absolute-form HTTP targets must never override the configured upstream.
  const absolute = await new Promise((resolve, reject) => {
    const request = http.request(base, { path: 'http://127.0.0.1:1/api/sessions' }, response => {
      let value = ''; response.on('data', bytes => { value += bytes; }); response.on('end', () => resolve({ status: response.statusCode, body: value }));
    });
    request.on('error', reject); request.end();
  });
  assert.equal(absolute.status, 200); assert.equal(JSON.parse(absolute.body).url, '/api/sessions');
});

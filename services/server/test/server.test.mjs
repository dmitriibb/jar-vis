import { test } from 'node:test';
import assert from 'node:assert/strict';
import { once } from 'node:events';
import http from 'node:http';
import { createServer } from '../src/server.mjs';
import { normalizeMessages, OpenCode, HttpError } from '../src/opencode.mjs';

const token = 'test-jarvis-owner-password-at-least-24-characters';
const origin = 'http://localhost:47833';
class FakeOpenCode {
  model = 'test/model'; active = false; prompts = 0; replies = 0;
  permissions = [{ id: 'per_1', action: 'search', resources: ['test'] }];
  async ensureReady() {}
  async owned(id) { if (id !== 'ses_ours') throw new HttpError(404, 'Session not found.'); return { id }; }
  async sessions() { return [{ id: 'ses_ours', title: 'Ours' }]; }
  async create() { return { id: 'ses_ours' }; }
  async settings() { return { model: this.model, agent: 'jarvis-home-assistant', models: [], agents: [] }; }
  async saveSettings(value) { this.model = value.model; return value; }
  async applyPreferences(id, value) { return value; }
  async snapshot(id) { await this.owned(id); return { session: { id }, messages: [], active: this.active, permissions: this.permissions }; }
  async fetch() { return new Response('data: {"type":"session.text.delta","data":{"sessionID":"ses_other","delta":"PRIVATE"}}\n\ndata: {"type":"session.text.delta","data":{"sessionID":"ses_ours","delta":"Hello"}}\n\n', { status: 200 }); }
  async api(route, method, body) {
    if (route.endsWith('/prompt')) { this.prompts++; await new Promise(r => setTimeout(r, 25)); this.active = true; return {}; }
    if (route.endsWith('/permission')) return { data: this.permissions };
    if (route.endsWith('/reply')) { this.replies++; this.permissions = []; return {}; }
    if (route.endsWith('/interrupt')) { this.active = false; return {}; }
    return {};
  }
}
async function setup(t, extra = {}) {
  const oc = new FakeOpenCode();
  const server = createServer({ token, origins: [origin], secureCookie: false, ...extra }, oc);
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  t.after(() => { server.close(); server.closeAllConnections(); });
  const base = `http://127.0.0.1:${server.address().port}`;
  const request = (route, options = {}) => fetch(base + '/api' + route, { ...options, headers: { Origin: origin, Authorization: `Bearer ${token}`, ...options.headers } });
  return { oc, server, base, request };
}

test('login, origin checks, cookie expiry/logout, and no unauthenticated access', async t => {
  const { base, request } = await setup(t);
  assert.equal((await fetch(base + '/api/sessions')).status, 401);
  assert.equal((await request('/sessions', { headers: { Origin: 'https://evil.example' } })).status, 403);
  assert.equal((await fetch(base + '/api/auth', { method: 'POST', body: JSON.stringify({ token }) })).status, 403);
  assert.equal((await request('/auth', { method: 'POST', body: '{"token":"wrong"}' })).status, 401);
  const login = await request('/auth', { method: 'POST', body: JSON.stringify({ token }) });
  assert.equal(login.status, 200);
  const cookie = login.headers.get('set-cookie');
  assert.match(cookie, /HttpOnly/); assert.match(cookie, /SameSite=Strict/); assert.ok(!cookie.includes(token));
  const headers = { Cookie: cookie.split(';')[0], Origin: origin };
  assert.equal((await fetch(base + '/api/sessions', { headers })).status, 200);
  assert.equal((await fetch(base + '/api/auth', { method: 'DELETE', headers })).status, 200);
  assert.equal((await fetch(base + '/api/sessions', { headers })).status, 401);
});

test('session scope enforced; concurrent prompts and permission replies resolve once', async t => {
  const { request, oc } = await setup(t);
  assert.equal((await request('/sessions/ses_other/snapshot')).status, 404);
  const prompt = () => request('/sessions/ses_ours/prompt', { method: 'POST', body: '{"text":"hello"}' });
  const values = await Promise.all([prompt(), prompt()]);
  assert.deepEqual(values.map(r => r.status).sort(), [202, 409]); assert.equal(oc.prompts, 1);
  const reply = () => request('/sessions/ses_ours/permissions/per_1', { method: 'POST', body: '{"decision":"once"}' });
  assert.deepEqual((await Promise.all([reply(), reply()])).map(r => r.status).sort(), [200, 409]);
  assert.equal(oc.replies, 1);
  assert.equal((await request('/sessions/ses_ours/interrupt', { method: 'POST' })).status, 200);
});

test('knowledge is optional; management proxy is allowlisted and does not expose secrets', async t => {
  const { request } = await setup(t);
  const status = await (await request('/status')).json(); assert.equal(status.opencode, true); assert.equal(status.knowledge, false);
  assert.equal((await request('/sessions')).status, 200);
  assert.equal((await request('/knowledge/documents')).status, 503);
  assert.equal((await request('/knowledge/config')).status, 404);
  assert.equal((await request('/knowledge/documents/not-a-uuid')).status, 400);
  assert.equal((await request('/sessions/ses_ours/prompt', { method: 'POST', body: '{"text":""}' })).status, 400);
  assert.equal((await request('/sessions/ses_ours/prompt', { method: 'POST', body: JSON.stringify({ text: 'x'.repeat(40000) }) })).status, 413);
});

test('browser settings require authentication, support allowed-origin preflight, and cannot change busy sessions', async t => {
  const { request, base, oc } = await setup(t);
  assert.equal((await fetch(base + '/api/settings')).status, 401);
  const preflight = await request('/settings', { method: 'OPTIONS', headers: { 'Access-Control-Request-Method': 'PUT', 'Access-Control-Request-Headers': 'authorization,content-type' } });
  assert.equal(preflight.status, 204); assert.equal(preflight.headers.get('access-control-allow-origin'), origin);
  assert.equal((await request('/settings', { method: 'OPTIONS', headers: { Origin: 'https://evil.example' } })).status, 403);
  assert.equal((await request('/settings')).status, 200);
  assert.equal((await request('/settings', { method: 'PUT', body: '{"model":"test/luna","agent":"jarvis-home-assistant"}' })).status, 200);
  assert.equal(oc.model, 'test/luna');
  oc.active = true;
  assert.equal((await request('/sessions/ses_ours/preferences', { method: 'PUT', body: '{"model":"test/luna","agent":"jarvis-home-assistant"}' })).status, 409);
  oc.active = false;
  assert.equal((await request('/sessions/ses_ours/preferences', { method: 'PUT', body: '{"model":"test/luna","agent":"jarvis-home-assistant"}' })).status, 200);
});

test('live event stream filters other sessions and reconnect sends snapshot without a prompt', async t => {
  const { request, oc } = await setup(t);
  for (let i = 0; i < 2; i++) {
    const controller = new AbortController();
    const response = await request('/sessions/ses_ours/events', { signal: controller.signal });
    const reader = response.body.getReader(); const decoder = new TextDecoder(); let value = '';
    while (!value.includes('event: snapshot') || !value.includes('Hello')) {
      const part = await reader.read(); value += decoder.decode(part.value);
    }
    controller.abort();
    assert.ok(!value.includes('PRIVATE')); assert.equal(oc.prompts, 0);
    await new Promise(r => setTimeout(r, 30));
  }
});

test('transcript normalization includes tools and errors, without arbitrary tool payloads', () => {
  const messages = normalizeMessages([
    { id: 'a', type: 'assistant', content: [{ type: 'text', text: '<script>untrusted</script>' }, { type: 'tool', name: 'search', state: { status: 'completed', output: 'private raw body' } }] },
    { id: 'u', type: 'user', text: 'Hi' },
  ]);
  assert.equal(messages[0].role, 'user'); assert.equal(messages[1].tools[0].status, 'completed');
  assert.ok(!JSON.stringify(messages).includes('private raw body'));
});

test('knowledge proxy authenticates upstream, streams originals, and hides upstream error bodies', async t => {
  let credentials;
  const upstream = http.createServer((req, res) => {
    credentials = req.headers.authorization;
    if (req.url.endsWith('/original')) { res.writeHead(200); res.end('original bytes'); return; }
    res.writeHead(500, { 'Content-Type': 'application/json' }); res.end('{"error":"secret upstream configuration"}');
  });
  upstream.listen(0, '127.0.0.1'); await once(upstream, 'listening');
  t.after(() => { upstream.close(); upstream.closeAllConnections(); });
  const { request } = await setup(t, { knowledgeURL: `http://127.0.0.1:${upstream.address().port}`, knowledgeToken: 'test-private-knowledge-token' });
  const response = await request('/knowledge/documents/12345678-1234-1234-1234-123456789abc/original');
  assert.equal(response.status, 200); assert.equal(await response.text(), 'original bytes');
  assert.match(response.headers.get('content-disposition'), /^attachment/);
  assert.equal(credentials, 'Bearer test-private-knowledge-token');
  const error = await request('/knowledge/documents');
  assert.equal(error.status, 502); const text = await error.text();
  assert.ok(!text.includes('secret')); assert.ok(!text.includes('test-private-knowledge-token'));
});

test('OpenCode ownership rejects every other workspace, including sibling paths', () => {
  const oc = new OpenCode({ workspace: 'C:\\jarvis\\workspace' });
  assert.equal(oc.owns({ location: { directory: 'C:\\jarvis\\workspace-other' } }), false);
});

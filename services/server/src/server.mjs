import http from 'node:http';
import { randomBytes, timingSafeEqual, createHash } from 'node:crypto';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { OpenCode, HttpError } from './opencode.mjs';

const COOKIE = 'jarvis_session';
const hash = value => createHash('sha256').update(value).digest();
const equal = (a, b) => timingSafeEqual(hash(String(a)), hash(String(b)));
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export async function body(req, limit = 32768) {
  const chunks = []; let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > limit) throw new HttpError(413, 'Request is too large.');
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}
async function jsonBody(req) {
  try { return JSON.parse((await body(req)).toString('utf8')); }
  catch (error) { if (error instanceof HttpError) throw error; throw new HttpError(400, 'Expected a JSON object.'); }
}
function json(res, status, data) { res.writeHead(status, { 'Content-Type': 'application/json' }); res.end(JSON.stringify(data)); }

export function createServer(config, oc = new OpenCode(config)) {
  if (!config.token || config.token.length < 24) throw new Error('Set a JARVIS_SERVER_TOKEN of at least 24 characters in .env.');
  const logins = new Map(), attempts = new Map(), locks = new Map(), admissions = new Map(), hubs = new Map();
  const cookie = value => `${COOKIE}=${value}; HttpOnly; SameSite=Strict; Path=/; Max-Age=${value ? 28800 : 0}${config.secureCookie ? '; Secure' : ''}`;
  const sessionFor = req => {
    const value = (req.headers.cookie || '').split(';').map(s => s.trim()).find(s => s.startsWith(`${COOKIE}=`))?.slice(COOKIE.length + 1);
    const expiry = value && logins.get(value);
    if (expiry && expiry > Date.now()) return value;
    if (value) logins.delete(value);
    return null;
  };
  const authorized = req => sessionFor(req) || (req.headers.authorization?.startsWith('Bearer ') && equal(req.headers.authorization.slice(7), config.token));

  async function exclusive(id, task) {
    const previous = locks.get(id) || Promise.resolve();
    const current = previous.catch(() => {}).then(task);
    locks.set(id, current);
    try { return await current; }
    finally { if (locks.get(id) === current) locks.delete(id); }
  }

  async function knowledge(route, options = {}) {
    if (!config.knowledgeURL || !config.knowledgeToken) throw new HttpError(503, 'Knowledge is not configured. Chat is still available.');
    try {
      return await fetch(`${config.knowledgeURL.replace(/\/$/, '')}${route}`, {
        ...options, headers: { ...options.headers, Authorization: `Bearer ${config.knowledgeToken}` },
        redirect: 'error', signal: AbortSignal.timeout(30000),
      });
    } catch { throw new HttpError(503, 'Knowledge is offline. Chat is still available.'); }
  }

  function subscribe(req, res, id) {
    let hub = hubs.get(id);
    if (!hub) {
      hub = { clients: new Set(), controller: new AbortController(), busy: false, timer: null };
      hubs.set(id, hub);
      hub.publish = (event, data) => {
        const encoded = `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
        for (const client of hub.clients) {
          // Drop slow readers rather than retaining unlimited private transcript buffers.
          if (client.writableLength > 512 * 1024) client.destroy();
          else client.write(encoded);
        }
      };
      hub.refresh = async () => {
        if (hub.busy || !hub.clients.size) return;
        hub.busy = true;
        try { hub.publish('snapshot', await oc.snapshot(id)); }
        catch (error) { hub.publish('offline', { error: error instanceof HttpError ? error.message : 'Session unavailable.' }); }
        finally { hub.busy = false; }
      };
      hub.timer = setInterval(() => {
        for (const [client, request] of [...hub.clients].map(c => [c, c.jarvisRequest])) {
          if (!authorized(request)) client.end();
        }
        hub.refresh();
      }, 1500);
      // The event stream supplies token deltas; snapshots recover durable state on reconnect.
      (async () => {
        while (!hub.controller.signal.aborted) {
          try {
            const upstream = await oc.fetch('/api/event', { headers: { Accept: 'text/event-stream' }, signal: hub.controller.signal });
            if (!upstream.ok || !upstream.body) throw new Error('events unavailable');
            let pending = ''; const decoder = new TextDecoder();
            for await (const bytes of upstream.body) {
              pending += decoder.decode(bytes, { stream: true }).replace(/\r\n/g, '\n');
              if (pending.length > 1024 * 1024) throw new Error('oversized event');
              let boundary;
              while ((boundary = pending.indexOf('\n\n')) !== -1) {
                const frame = pending.slice(0, boundary); pending = pending.slice(boundary + 2);
                const encoded = frame.split('\n').filter(l => l.startsWith('data:')).map(l => l.slice(5).trimStart()).join('\n');
                let event; try { event = JSON.parse(encoded); } catch { continue; }
                if (event.data?.sessionID !== id) continue;
                if (event.type === 'session.text.delta') hub.publish('delta', { text: String(event.data.delta || '').slice(0, 65536) });
                if (event.type === 'session.tool.input.started') hub.publish('activity', { name: event.data.name });
                if (/^session\.execution\./.test(event.type) || event.type.startsWith('permission.')) hub.refresh();
              }
            }
          } catch { /* periodic snapshots continue if OpenCode's event feed is unavailable */ }
          if (!hub.controller.signal.aborted) await new Promise(resolve => {
            const timer = setTimeout(done, 3000);
            function done() { clearTimeout(timer); hub.controller.signal.removeEventListener('abort', done); resolve(); }
            hub.controller.signal.addEventListener('abort', done, { once: true });
          });
        }
      })();
    }
    res.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'keep-alive', 'X-Accel-Buffering': 'no' });
    res.write(': connected\nretry: 2000\n\n');
    res.jarvisRequest = req;
    hub.clients.add(res);
    // A joining client always receives current state even if this hub already exists.
    oc.snapshot(id).then(value => { if (!res.destroyed) res.write(`event: snapshot\ndata: ${JSON.stringify(value)}\n\n`); })
      .catch(() => { if (!res.destroyed) res.write('event: offline\ndata: {"error":"Session unavailable."}\n\n'); });
    res.on('close', () => {
      hub.clients.delete(res);
      if (!hub.clients.size) { clearInterval(hub.timer); hub.controller.abort(); hubs.delete(id); }
    });
  }

  const server = http.createServer(async (req, res) => {
    res.setHeader('Cache-Control', 'no-store');
    res.setHeader('X-Content-Type-Options', 'nosniff');
    res.setHeader('Referrer-Policy', 'no-referrer');
    try {
      const url = new URL(req.url, 'http://jarvis.invalid');
      const route = url.pathname;
      const mutate = !['GET', 'HEAD'].includes(req.method);
      if (req.headers.origin && !config.origins.includes(req.headers.origin)) throw new HttpError(403, 'Browser origin is not allowed.');
      if (req.headers.origin) {
        res.setHeader('Access-Control-Allow-Origin', req.headers.origin);
        res.setHeader('Vary', 'Origin');
        res.setHeader('Access-Control-Allow-Credentials', 'true');
        res.setHeader('Access-Control-Expose-Headers', 'Content-Disposition');
      }
      if (req.method === 'OPTIONS') {
        if (!req.headers.origin || !config.origins.includes(req.headers.origin)) throw new HttpError(403, 'Browser origin is not allowed.');
        res.setHeader('Access-Control-Allow-Methods', 'GET, POST, PUT, DELETE, OPTIONS');
        res.setHeader('Access-Control-Allow-Headers', 'Content-Type, Authorization');
        res.writeHead(204); res.end(); return;
      }
      if (mutate && !req.headers.origin && !req.headers.authorization) throw new HttpError(403, 'Missing browser origin.');

      if (route === '/api/auth' && req.method === 'POST') {
        const ip = req.socket.remoteAddress;
        const attempt = attempts.get(ip) || { count: 0, expires: Date.now() + 300000 };
        if (attempt.expires < Date.now()) { attempt.count = 0; attempt.expires = Date.now() + 300000; }
        attempts.set(ip, attempt);
        if (++attempt.count > 20) throw new HttpError(429, 'Too many login attempts. Try again in five minutes.');
        const value = await jsonBody(req);
        if (typeof value?.token !== 'string' || !equal(value.token, config.token)) throw new HttpError(401, 'Incorrect server password.');
        const key = randomBytes(32).toString('hex');
        logins.set(key, Date.now() + 28800000);
        res.setHeader('Set-Cookie', cookie(key));
        json(res, 200, { authenticated: true }); return;
      }
      if (!authorized(req)) throw new HttpError(401, 'Connect with your Jarvis Server password.');
      if (route === '/api/auth' && req.method === 'GET') { json(res, 200, { authenticated: true }); return; }
      if (route === '/api/auth' && req.method === 'DELETE') {
        logins.delete(sessionFor(req)); res.setHeader('Set-Cookie', cookie('')); json(res, 200, { authenticated: false }); return;
      }
      if (route === '/api/status' && req.method === 'GET') {
        const result = { opencode: false, knowledge: false, model: oc.model, memory: false, schedules: false };
        await Promise.all([
          oc.ensureReady().then(() => { result.opencode = true; result.model = oc.model; }).catch(() => {}),
          knowledge('/v1/capabilities').then(async r => { result.knowledge = r.ok && Boolean((await r.json()).keyword_search); }).catch(() => {}),
        ]);
        if (result.opencode && result.knowledge) await oc.reconnectKnowledge?.().catch(() => {});
        json(res, 200, result); return;
      }
      if (route === '/api/settings' && req.method === 'GET') { json(res, 200, await oc.settings()); return; }
      if (route === '/api/settings' && req.method === 'PUT') {
        const value = await jsonBody(req);
        json(res, 200, await exclusive('settings', () => oc.saveSettings(value))); return;
      }
      if (route === '/api/sessions' && req.method === 'GET') {
        json(res, 200, (await oc.sessions()).map(s => ({ id: s.id, title: s.title || 'New conversation', updated: s.time?.updated }))); return;
      }
      if (route === '/api/sessions' && req.method === 'POST') { json(res, 201, await oc.create()); return; }
      const match = route.match(/^\/api\/sessions\/([\w-]+)(?:\/(snapshot|events|prompt|interrupt|permissions|preferences)(?:\/([\w-]+))?)?$/);
      if (match) {
        const [, id, action, permissionID] = match;
        await oc.owned(id);
        if (action === 'snapshot' && req.method === 'GET') { json(res, 200, await oc.snapshot(id)); return; }
        if (action === 'events' && req.method === 'GET') { subscribe(req, res, id); return; }
        if (action === 'preferences' && req.method === 'PUT') {
          const value = await jsonBody(req);
          const selected = await exclusive(id, async () => {
            if ((await oc.snapshot(id)).active || (admissions.get(id) || 0) > Date.now()) throw new HttpError(409, 'Stop or finish this turn before changing its model or agent.');
            return oc.applyPreferences(id, value);
          });
          json(res, 200, selected); return;
        }
        if (action === 'prompt' && req.method === 'POST') {
          const value = await jsonBody(req);
          if (typeof value?.text !== 'string' || !value.text.trim() || value.text.length > 16000) throw new HttpError(400, 'Enter between 1 and 16000 characters.');
          await exclusive(id, async () => {
            const state = await oc.snapshot(id);
            if (state.active || (admissions.get(id) || 0) > Date.now()) throw new HttpError(409, 'This session is already working. Wait or stop it first.');
            admissions.set(id, Date.now() + 3000);
            // Never retry an ambiguous prompt submission: it could already have run.
            await oc.api(`/api/session/${id}/prompt`, 'POST', { text: value.text.trim() });
          });
          json(res, 202, { accepted: true }); return;
        }
        if (action === 'interrupt' && req.method === 'POST') {
          await exclusive(id, () => oc.api(`/api/session/${id}/interrupt`, 'POST'));
          json(res, 200, { interrupted: true }); return;
        }
        if (action === 'permissions' && permissionID && req.method === 'POST') {
          const value = await jsonBody(req);
          if (!['once', 'always', 'reject'].includes(value?.decision)) throw new HttpError(400, 'Invalid permission decision.');
          await exclusive(id, async () => {
            const pending = (await oc.api(`/api/session/${id}/permission`))?.data || [];
            if (!pending.some(p => p.id === permissionID)) throw new HttpError(409, 'Permission already answered.');
            await oc.api(`/api/session/${id}/permission/${permissionID}/reply`, 'POST', { decision: value.decision });
          });
          json(res, 200, { replied: true }); return;
        }
        if (!action && req.method === 'DELETE') {
          await exclusive(id, async () => {
            await oc.api(`/api/session/${id}/interrupt`, 'POST');
            await oc.api(`/api/session/${id}`, 'DELETE');
          });
          json(res, 200, { deleted: true }); return;
        }
      }
      // Explicit management allowlist. No general proxy to either upstream service.
      const k = route.match(/^\/api\/knowledge\/(documents|jobs|sections|search)(?:\/([\w-]+))?(?:\/(sections|original))?$/);
      if (k) {
        const [, resource, id, suffix] = k;
        if (id && !uuid.test(id)) throw new HttpError(400, 'Expected a source UUID.');
        const valid = (resource === 'documents' && ((!id && ['GET', 'POST'].includes(req.method)) || (id && !suffix && req.method === 'DELETE') || (id && ['sections', 'original'].includes(suffix) && req.method === 'GET')))
          || (['jobs', 'sections'].includes(resource) && id && !suffix && req.method === 'GET')
          || (resource === 'search' && !id && !suffix && req.method === 'POST');
        if (!valid) throw new HttpError(404, 'Route not found.');
        const payload = mutate ? await body(req, resource === 'documents' ? 11 * 1024 * 1024 : 32768) : undefined;
        const response = await knowledge(`/v1/${resource}${id ? '/' + id : ''}${suffix ? '/' + suffix : ''}`, {
          method: req.method, headers: payload ? { 'Content-Type': req.headers['content-type'] || 'application/json' } : {}, body: payload,
        });
        if (!response.ok) {
          // Only validation errors from our known knowledge API are suitable for the UI.
          if (response.status === 400) { const value = await response.json().catch(() => null); throw new HttpError(400, typeof value?.error === 'string' ? value.error : 'Knowledge request was invalid.'); }
          throw new HttpError(response.status === 404 ? 404 : 502, `Knowledge request failed (${response.status}).`);
        }
        res.writeHead(response.status, {
          'Content-Type': suffix === 'original' ? 'application/octet-stream' : 'application/json',
          ...(suffix === 'original' ? { 'Content-Disposition': /^attachment; filename="[0-9a-f-]+\.(txt|md|pdf)"$/i.test(response.headers.get('content-disposition') || '') ? response.headers.get('content-disposition') : `attachment; filename="source-${id}"` } : {}),
        });
        if (response.body) await pipeline(Readable.fromWeb(response.body), res);
        else res.end();
        return;
      }
      throw new HttpError(404, 'Route not found.');
    } catch (error) {
      if (res.headersSent) { res.end(); return; }
      json(res, error instanceof HttpError ? error.status : 500, { error: error instanceof HttpError ? error.message : 'Jarvis Server could not complete this request.' });
    }
  });
  const cleanup = setInterval(() => {
    for (const [key, expiry] of logins) if (expiry < Date.now()) logins.delete(key);
    for (const [key, value] of attempts) if (value.expires < Date.now()) attempts.delete(key);
    for (const [key, expiry] of admissions) if (expiry < Date.now()) admissions.delete(key);
  }, 60000).unref();
  server.on('close', () => { clearInterval(cleanup); for (const hub of hubs.values()) { clearInterval(hub.timer); hub.controller.abort(); for (const client of hub.clients) client.end(); } });
  server.requestTimeout = 45000;
  return server;
}

// Opt-in, synthetic-data-only smoke test through Web -> Jarvis Server -> existing OpenCode.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
const origin = process.env.JARVIS_LIVE_WEB_URL || 'http://127.0.0.1:47833';
const login = await fetch(origin + '/api/auth', { method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ token: process.env.JARVIS_SERVER_TOKEN }) });
assert.equal(login.status, 200, 'Browser gateway login');
const cookie = login.headers.get('set-cookie').split(';')[0];
async function api(route, method = 'GET', value) {
  const form = value instanceof FormData;
  const response = await fetch(origin + '/api' + route, { method, headers: { Origin: origin, Cookie: cookie, ...(!form ? { 'Content-Type': 'application/json' } : {}) }, body: value === undefined ? undefined : form ? value : JSON.stringify(value) });
  assert.ok(response.ok, `${method} ${route}: ${response.status}`);
  return response;
}
let sessionID, documentID;
const events = new AbortController();
try {
  const status = await (await api('/status')).json();
  assert.ok(status.opencode && status.knowledge, 'Start OpenCode and knowledge first');
  console.log('Connected to existing OpenCode and knowledge through the browser proxy.');
  const bytes = await readFile(new URL('../../knowledge/tests/fixtures/harbor-rules.md', import.meta.url));
  const form = new FormData();
  const edition = `web-smoke-${randomUUID()}`;
  form.append('metadata', JSON.stringify({ source_key: edition, title: 'Disposable web smoke fixture', game: 'Harbor Traders', edition }));
  form.append('file', new Blob([bytes]), 'rules.md');
  const job = await (await api('/knowledge/documents', 'POST', form)).json(); documentID = job.document_id;
  const deadline = Date.now() + 180000;
  while (Date.now() < deadline) {
    const result = await (await api(`/knowledge/jobs/${job.id}`)).json();
    assert.notEqual(result.state, 'failed', 'Fixture import');
    if (result.state === 'succeeded') break;
    await new Promise(r => setTimeout(r, 300));
  }
  const sections = await (await api(`/knowledge/documents/${documentID}/sections`)).json();
  assert.ok(sections.length);
  const section = sections.find(s => s.heading.includes('Trading'));
  assert.ok(section);
  assert.equal((await (await api(`/knowledge/sections/${section.id}`)).json())[0].id, section.id);
  assert.equal(Buffer.compare(Buffer.from(await (await api(`/knowledge/documents/${documentID}/original`)).arrayBuffer()), bytes), 0);
  sessionID = (await (await api('/sessions', 'POST', {})).json()).id;
  let sawDelta = false, sawSnapshot = false;
  const response = await fetch(`${origin}/api/sessions/${sessionID}/events`, { headers: { Origin: origin, Cookie: cookie }, signal: events.signal });
  const stream = (async () => {
    try { for await (const chunk of response.body) { const text = new TextDecoder().decode(chunk); sawDelta ||= text.includes('event: delta'); sawSnapshot ||= text.includes('event: snapshot'); } } catch { /* test abort */ }
  })();
  await api(`/sessions/${sessionID}/prompt`, 'POST', { text: `For Harbor Traders edition ${edition}, search knowledge with that exact edition filter, then read the Trading section. How many copper coins buy a spice crate, and how often can I trade? Cite the section UUID.` });
  console.log('Prompt accepted; checking real model/tool results and browser live events…');
  let final;
  while (Date.now() < deadline) {
    const snapshot = await (await api(`/sessions/${sessionID}/snapshot`)).json();
    if (!snapshot.active && snapshot.messages.some(m => m.role === 'assistant' && m.text)) { final = snapshot; break; }
    await new Promise(r => setTimeout(r, 1000));
  }
  assert.ok(final, 'Model finished before timeout');
  const answer = final.messages.filter(m => m.role === 'assistant').map(m => m.text).join('\n');
  assert.match(answer, /three|3/i); assert.match(answer, /once/i); assert.ok(answer.includes(section.id), 'Correct source citation');
  const tools = final.messages.flatMap(m => m.tools).filter(t => t.status === 'completed').map(t => t.name);
  assert.ok(tools.includes('jarvis_knowledge_search_knowledge')); assert.ok(tools.includes('jarvis_knowledge_read_section'));
  assert.ok(sawDelta && sawSnapshot, 'Browser received token deltas and durable snapshots');
  events.abort(); await stream;
  console.log('PASS: upload, import progress, original download, source read, cloud answer, actual MCP search/read, citation, and live browser events.');
} finally {
  events.abort();
  if (sessionID) await api(`/sessions/${sessionID}`, 'DELETE').catch(() => console.error('Disposable session cleanup failed.'));
  if (documentID) await api(`/knowledge/documents/${documentID}`, 'DELETE').catch(() => console.error('Disposable source cleanup failed.'));
  await api('/auth', 'DELETE');
}

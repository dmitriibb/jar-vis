const $ = id => document.getElementById(id);
function readStorage(storage, key) { try { return window[storage].getItem(key); } catch { return null; } }
function writeStorage(storage, key, value) { try { if (value === null) window[storage].removeItem(key); else window[storage].setItem(key, value); } catch { /* Private/embedded contexts may allow only in-memory settings. */ } }
const connection = { url: readStorage('localStorage', 'jarvis.backend.url') || '', secret: '' };
try { const saved = JSON.parse(readStorage('sessionStorage', 'jarvis.backend.auth') || 'null'); if (saved?.url === connection.url) connection.secret = saved.secret || ''; } catch { /* no saved login */ }
const endpoint = route => `${connection.url || location.origin}/api${route}`;
function rememberSecret() { writeStorage('sessionStorage', 'jarvis.backend.auth', connection.secret ? JSON.stringify(connection) : null); }
let backendSettings = null;
let lastErrorKind = '';
const state = { authenticated: false, id: null, events: null, snapshot: null, stream: '', streaming: false, active: false, pending: false, tab: 'chat', generation: 0, documentsGeneration: 0 };
const widgetMode = new URLSearchParams(location.search).get('widget') === '1';
if (widgetMode) { document.body.classList.add('widget-mode'); document.documentElement.classList.add('widget-mode'); }
let parentOrigin = null;
if (widgetMode && document.referrer) {
  const origin = new URL(document.referrer).origin;
  fetch('/public-config.json').then(r => r.json()).then(config => {
    if (origin === location.origin || config.embedOrigins.includes(origin)) { parentOrigin = origin; notifySize(); }
  });
}
function notifySize() { if (parentOrigin) parent.postMessage({ type: 'jarvis:resize', open: !$('panel').hidden }, parentOrigin); }
function openPanel(open = true) {
  $('panel').hidden = !open;
  $('pet').setAttribute('aria-expanded', String(open));
  $('pet').setAttribute('aria-label', open ? 'Minimize Jarvis' : 'Open Jarvis');
  notifySize();
  if (open) (state.authenticated ? $('prompt') : $('password')).focus();
}
$('pet').onclick = () => openPanel($('panel').hidden);
$('close').onclick = () => openPanel(false);
$('open-hero').onclick = () => openPanel();
document.addEventListener('keydown', event => { if (event.key === 'Escape') { if (!$('settings-view').hidden) closeSettings(); else if (!$('session-menu').hidden) $('session-menu').hidden = true; else if (!$('source-view').hidden) $('source-view').hidden = true; else openPanel(false); } });

function error(message, kind = 'action') { lastErrorKind = kind; $('error').textContent = message || ''; $('error').hidden = !message; }
async function api(route, options = {}) {
  const response = await fetch(endpoint(route), { ...options, redirect: 'error', credentials: connection.url ? 'omit' : 'same-origin', headers: {
    ...(options.body instanceof FormData ? {} : { 'Content-Type': 'application/json' }),
    ...(connection.secret ? { Authorization: `Bearer ${connection.secret}` } : {}), ...options.headers,
  } });
  const value = await response.json().catch(() => ({}));
  if (!response.ok) {
    if (response.status === 401) disconnect(true);
    throw new Error(value.error || `Request failed (${response.status}).`);
  }
  return value;
}
async function run(task) { error(''); try { await task(); } catch (e) { error(e.message || 'Could not connect. Try again.'); } }
function node(tag, text, className) { const item = document.createElement(tag); if (text) item.textContent = text; if (className) item.className = className; return item; }
function button(text, action, className = 'text-button') { const item = node('button', text, className); item.type = 'button'; item.onclick = () => run(action); return item; }
function disconnect(clearSecret = false) {
  if (clearSecret) { connection.secret = ''; rememberSecret(); }
  state.authenticated = false; state.generation++; state.documentsGeneration++;
  state.events?.close(); state.events = null; state.id = null; state.snapshot = null; state.stream = ''; state.streaming = false;
  $('messages').replaceChildren(); $('documents').replaceChildren(); $('permissions').replaceChildren();
  $('source-view').hidden = true; $('source-text').textContent = ''; $('prompt').value = '';
  $('connected-view').hidden = true; $('login-view').hidden = false; $('logout').hidden = true;
  $('connection-status').textContent = 'Ready when you are'; $('pet').classList.remove('working');
  $('settings-connection-status').textContent = 'Not connected'; $('backend-fields').disabled = true;
  $('new-session').disabled = true; $('recent-sessions').disabled = true; $('session-menu').hidden = true;
}

async function connect() {
  await api('/auth');
  state.authenticated = true; $('login-view').hidden = true; $('connected-view').hidden = false; $('logout').hidden = false;
  $('settings-connection-status').textContent = 'Connected'; $('new-session').disabled = false; $('recent-sessions').disabled = false;
  if (!state.snapshot) renderMessages();
  await Promise.all([status(), sessions()]);
}
async function status() {
  const result = await api('/status');
  if (!state.authenticated) return;
  $('connection-status').textContent = result.opencode ? 'Connected · your OpenCode' : 'OpenCode is offline';
  $('pet').classList.toggle('offline', !result.opencode);
  $('knowledge-dot').classList.toggle('unavailable', !result.knowledge);
  $('knowledge-status').textContent = result.knowledge ? 'Text, Markdown and text-based PDF · up to 10 MB' : 'Knowledge is offline or not configured. Ordinary chat still works.';
  showModel(state.snapshot?.session.model || result.model);
}
function showModel(model) { $('model-label').textContent = typeof model === 'string' ? model.replace(/^[^/]+\//, '') : model?.id || model?.modelID || 'Your OpenCode model'; }
async function sessions(preferred = state.id) {
  const values = await api('/sessions');
  if (!state.authenticated) return;
  $('sessions').replaceChildren(...values.slice(0, 10).map(s => {
    const item = button(s.title, () => { selectSession(s.id); $('session-menu').hidden = true; $('recent-sessions').setAttribute('aria-expanded', 'false'); }, 'recent-session');
    item.setAttribute('aria-current', String(s.id === preferred)); return item;
  }));
  if (!values.length) { $('sessions').append(node('p', 'No conversations yet.', 'fine')); if (state.id) selectSession(null); return; }
  if (preferred === state.id && preferred && !values.some(s => s.id === preferred)) return;
  const id = values.some(s => s.id === preferred) ? preferred : values[0].id;
  if (id !== state.id) selectSession(id);
}

function eventStream(route) {
  const controller = new AbortController();
  const listeners = new Map();
  const stream = { addEventListener(name, callback) { listeners.set(name, callback); }, close() { controller.abort(); }, onerror: null };
  (async () => {
    while (!controller.signal.aborted && state.authenticated) {
      try {
        const response = await fetch(endpoint(route), { redirect: 'error', headers: { Accept: 'text/event-stream', ...(connection.secret ? { Authorization: `Bearer ${connection.secret}` } : {}) }, credentials: connection.url ? 'omit' : 'same-origin', signal: controller.signal });
        if (response.status === 401) { disconnect(true); return; }
        if (!response.ok || !response.body) throw new Error('Live connection unavailable');
        let pending = ''; const decoder = new TextDecoder();
        for await (const bytes of response.body) {
          pending += decoder.decode(bytes, { stream: true }).replace(/\r\n/g, '\n');
          if (pending.length > 1024 * 1024) throw new Error('Live event too large');
          let boundary;
          while ((boundary = pending.indexOf('\n\n')) !== -1) {
            const frame = pending.slice(0, boundary); pending = pending.slice(boundary + 2);
            const kind = frame.split('\n').find(line => line.startsWith('event:'))?.slice(6).trim();
            const data = frame.split('\n').filter(line => line.startsWith('data:')).map(line => line.slice(5).trimStart()).join('\n');
            if (kind && data) listeners.get(kind)?.({ data });
          }
        }
      } catch { if (!controller.signal.aborted) stream.onerror?.(); }
      if (!controller.signal.aborted) await new Promise(resolve => {
        const timer = setTimeout(done, 2000);
        function done() { clearTimeout(timer); controller.signal.removeEventListener('abort', done); resolve(); }
        controller.signal.addEventListener('abort', done, { once: true });
      });
    }
  })();
  return stream;
}
function selectSession(id) {
  state.generation++; state.id = id; state.events?.close(); state.events = null;
  state.snapshot = null; state.stream = ''; state.streaming = false; state.active = false;
  $('source-view').hidden = true; $('permissions').replaceChildren(); $('live').hidden = true; updateComposer(); renderMessages();
  if (!id) return;
  const generation = state.generation;
  const events = eventStream(`/sessions/${encodeURIComponent(id)}/events`);
  state.events = events;
  events.addEventListener('snapshot', event => {
    if (generation !== state.generation) return;
    if (lastErrorKind === 'connection') error(''); $('connection-status').textContent = 'Connected · your OpenCode';
    const snapshot = JSON.parse(event.data);
    const firstSnapshot = !state.snapshot;
    const messagesChanged = JSON.stringify(snapshot.messages) !== JSON.stringify(state.snapshot?.messages);
    const permissionsChanged = JSON.stringify(snapshot.permissions) !== JSON.stringify(state.snapshot?.permissions);
    const wasStreaming = state.streaming;
    state.snapshot = snapshot; state.active = snapshot.active;
    showModel(snapshot.session.model);
    if (!snapshot.active) { state.stream = ''; state.streaming = false; }
    else if (!state.streaming) {
      const last = snapshot.messages.at(-1);
      state.stream = last?.role === 'assistant' ? last.text : '';
    }
    $('pet').classList.remove('offline'); updateComposer();
    if (firstSnapshot || messagesChanged || wasStreaming !== state.streaming) renderMessages(firstSnapshot);
    if (firstSnapshot || permissionsChanged) renderPermissions(snapshot.permissions);
  });
  events.addEventListener('delta', event => {
    if (generation !== state.generation) return;
    state.stream += JSON.parse(event.data).text; state.streaming = true; state.active = true;
    updateComposer(); renderMessages();
  });
  events.addEventListener('activity', event => { if (generation === state.generation) $('live-text').textContent = `Using ${JSON.parse(event.data).name || 'knowledge'}…`; });
  events.addEventListener('offline', event => { if (generation === state.generation) { $('connection-status').textContent = 'Reconnecting…'; $('pet').classList.add('offline'); error(JSON.parse(event.data).error, 'connection'); } });
  events.onerror = () => { if (generation === state.generation && state.authenticated) { $('connection-status').textContent = 'Reconnecting…'; api('/auth').catch(() => {}); } };
}
function updateComposer() {
  $('send').disabled = state.active || state.pending;
  $('stop').hidden = !state.active;
  $('live').hidden = !state.active;
  if (!state.active) $('live-text').textContent = 'Jarvis is thinking…';
  $('pet').classList.toggle('working', state.active);
}
function citedText(text) {
  const fragment = document.createDocumentFragment();
  // Small DOM-only inline renderer: model/source text never becomes HTML.
  const pattern = /`?\b([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\b`?|\*\*([^*\n]+)\*\*|\*([^*\n]+)\*|`([^`\n]+)`/ig;
  let offset = 0;
  for (const match of text.matchAll(pattern)) {
    fragment.append(document.createTextNode(text.slice(offset, match.index)));
    if (match[1]) {
      const link = button(`Source ${match[1].slice(0, 6)} ↗`, () => showSource(match[1]), 'source-link'); link.title = match[1]; fragment.append(link);
    } else {
      const formatted = node(match[2] ? 'strong' : match[3] ? 'em' : 'code');
      formatted.append(citedText(match[2] || match[3] || match[4])); fragment.append(formatted);
    }
    offset = match.index + match[0].length;
  }
  fragment.append(document.createTextNode(text.slice(offset))); return fragment;
}
function renderMessages(forceBottom = false) {
  const container = $('messages');
  const previousScroll = container.scrollTop;
  const atBottom = container.scrollHeight - container.scrollTop - container.clientHeight < 70;
  let messages = [...(state.snapshot?.messages || [])];
  if (state.streaming && state.stream) {
    const lastUser = messages.findLastIndex(m => m.role === 'user');
    const tail = messages.slice(lastUser + 1);
    if (tail.some(m => m.role === 'assistant')) {
      const last = messages.findLastIndex(m => m.role === 'assistant');
      messages[last] = { ...messages[last], text: state.stream };
    } else messages.push({ role: 'assistant', text: state.stream, tools: [] });
  }
  if (!messages.length) {
    const empty = node('div', '', 'empty-chat'); const image = node('img'); image.src = '/pet.svg'; image.alt = '';
    empty.append(image, node('h2', 'What’s on your mind?'), node('p', 'Ask about your documents, plan something,\nor just start a conversation.'));
    container.replaceChildren(empty); return;
  }
  container.replaceChildren(...messages.map(message => {
    const toolOnly = !message.text && message.tools?.length;
    const item = node('article', '', `message ${message.role}${toolOnly ? ' tool-message' : ''}`);
    if (!toolOnly) item.append(node('div', message.role === 'user' ? 'YOU' : 'JARVIS', 'message-label'));
    for (const tool of message.tools || []) item.append(node('div', `${tool.name.replace('jarvis_knowledge_', '')} · ${tool.status}`, 'tool'));
    const text = node('div', '', 'message-text'); text.append(citedText(message.text || '')); item.append(text);
    if (message.error) item.append(node('p', message.error, 'error'));
    return item;
  }));
  if (atBottom || forceBottom) container.scrollTop = container.scrollHeight;
  else container.scrollTop = previousScroll;
}
function renderPermissions(permissions) {
  $('permissions').replaceChildren(...permissions.map(permission => {
    const item = node('div', '', 'permission'); item.append(node('strong', 'Jarvis needs your permission'), node('p', permission.message || `${permission.action}: ${(permission.resources || []).join(', ')}`));
    for (const [label, decision] of [['Allow once', 'once'], ['Always', 'always'], ['Deny', 'reject']]) item.append(button(label, async () => {
      await api(`/sessions/${state.id}/permissions/${permission.id}`, { method: 'POST', body: JSON.stringify({ decision }) });
      item.remove();
    }));
    return item;
  }));
}
async function createSession() {
  const session = await api('/sessions', { method: 'POST', body: '{}' });
  await sessions(session.id); return session.id;
}
$('login-form').onsubmit = event => { event.preventDefault(); run(async () => {
  const submit = event.submitter; submit.disabled = true;
  try { connection.secret = $('password').value; await api('/auth', { method: 'POST', body: JSON.stringify({ token: connection.secret }) }); rememberSecret(); $('password').value = ''; await connect(); }
  finally { submit.disabled = false; }
}); };
$('logout').onclick = () => run(async () => { try { await api('/auth', { method: 'DELETE' }); } finally { disconnect(true); } });
$('new-session').onclick = () => run(createSession);
$('recent-sessions').onclick = () => run(async () => { const open = $('session-menu').hidden; if (open) await sessions(); $('session-menu').hidden = !open; $('recent-sessions').setAttribute('aria-expanded', String(open)); });
document.addEventListener('click', event => { if (!$('session-menu').contains(event.target) && !$('recent-sessions').contains(event.target)) { $('session-menu').hidden = true; $('recent-sessions').setAttribute('aria-expanded', 'false'); } });
$('chat-form').onsubmit = event => { event.preventDefault(); run(async () => {
  if (state.pending || state.active) return;
  const text = $('prompt').value.trim(); if (!text) return;
  state.pending = true; updateComposer();
  try {
    const id = state.id || await createSession();
    await api(`/sessions/${id}/prompt`, { method: 'POST', body: JSON.stringify({ text }) });
    if (state.id === id) { $('prompt').value = ''; state.active = true; state.stream = ''; state.streaming = false; }
  } finally { state.pending = false; updateComposer(); }
}); };
$('prompt').onkeydown = event => { if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) { event.preventDefault(); $('chat-form').requestSubmit(); } };
$('stop').onclick = () => run(async () => { if (state.id) await api(`/sessions/${state.id}/interrupt`, { method: 'POST', body: '{}' }); });
function tab(name) {
  state.tab = name; $('chat-view').hidden = name !== 'chat'; $('knowledge-view').hidden = name !== 'knowledge';
  for (const key of ['chat', 'knowledge']) { $(`${key}-tab`).classList.toggle('selected', key === name); $(`${key}-tab`).setAttribute('aria-pressed', String(key === name)); }
  if (name === 'knowledge') run(documents);
}
$('chat-tab').onclick = () => tab('chat'); $('knowledge-tab').onclick = () => tab('knowledge');
$('refresh-documents').onclick = () => run(documents);
$('file').onchange = () => { const file = $('file').files[0]; if (file) { $('source-title').value ||= file.name.replace(/\.[^.]+$/, ''); $('source-key').value ||= `documents/${file.name}`; } };
async function documents() {
  const generation = ++state.documentsGeneration;
  const values = await api('/knowledge/documents');
  if (generation !== state.documentsGeneration || !state.authenticated) return;
  $('documents').replaceChildren(...values.map(document => {
    const item = node('div', '', 'document');
    item.append(node('strong', document.title || document.source_key), node('p', `${document.source_key} · ${document.latest_state || 'indexed'}`, 'fine'));
    if (document.latest_error) item.append(node('p', document.latest_error, 'error'));
    const actions = node('div', '', 'document-actions');
    actions.append(button('Read sections', async () => {
      const readGeneration = state.generation;
      const sections = await api(`/knowledge/documents/${document.id}/sections`);
      if (readGeneration !== state.generation || !state.authenticated) return;
      if (!sections.length) throw new Error('This source has no indexed sections yet.');
      $('source-heading').textContent = document.title; $('source-meta').textContent = 'Extracted sections · untrusted source text';
      $('source-text').textContent = sections.map(s => `${s.heading || 'Section'}${s.page ? ` · page ${s.page}` : ''}\n\n${s.text}`).join('\n\n────────\n\n'); $('source-view').hidden = false;
    }));
    actions.append(button('Original ↗', async () => {
      const response = await fetch(endpoint(`/knowledge/documents/${document.id}/original`), { redirect: 'error', headers: connection.secret ? { Authorization: `Bearer ${connection.secret}` } : {}, credentials: connection.url ? 'omit' : 'same-origin' });
      if (!response.ok) throw new Error('Could not download the original.');
      const objectURL = URL.createObjectURL(await response.blob()); const link = node('a'); link.href = objectURL;
      link.download = response.headers.get('content-disposition')?.match(/filename="([^"]+)"/)?.[1] || document.title;
      link.click(); setTimeout(() => URL.revokeObjectURL(objectURL), 1000);
    }));
    actions.append(button('Delete', async () => {
      if (!confirm(`Delete “${document.title}” and its indexed content?`)) return;
      await api(`/knowledge/documents/${document.id}`, { method: 'DELETE' }); await documents();
    })); item.append(actions); return item;
  }));
  if (!values.length) $('documents').append(node('p', 'Your library starts with one document.', 'fine'));
}
$('upload-form').onsubmit = event => { event.preventDefault(); run(async () => {
  const file = $('file').files[0]; if (!file) return;
  if (file.size > 10 * 1024 * 1024) throw new Error('Choose a file smaller than 10 MB.');
  const form = new FormData();
  form.append('metadata', JSON.stringify({ source_key: $('source-key').value, title: $('source-title').value, game: $('game').value || null, edition: $('edition').value || null }));
  form.append('file', file); event.submitter.disabled = true;
  try {
    const job = await api('/knowledge/documents', { method: 'POST', body: form });
    $('import-progress').textContent = `Import queued · ${job.id}`;
    const deadline = Date.now() + 120000;
    while (state.authenticated && Date.now() < deadline) {
      const result = await api(`/knowledge/jobs/${job.id}`);
      $('import-progress').textContent = `Import ${result.state}${result.section_count != null ? ` · ${result.section_count} sections` : ''}`;
      if (result.state === 'failed') throw new Error(result.error || 'Import failed.');
      if (result.state === 'succeeded') { await documents(); return; }
      await new Promise(resolve => setTimeout(resolve, 1000));
    }
    if (state.authenticated) { $('import-progress').textContent = 'Import continues on the server. Refresh your library to check progress.'; await documents(); }
  } finally { event.submitter.disabled = false; }
}); };
async function showSource(id) {
  const generation = state.generation;
  const section = await api(`/knowledge/sections/${id}`);
  if (generation !== state.generation || !state.authenticated) return;
  // Knowledge read endpoint returns the requested section with bounded neighbors.
  const value = Array.isArray(section) ? section[0] : section;
  $('source-heading').textContent = value.heading || value.title || 'Source section';
  $('source-meta').textContent = `${value.title || ''}${value.page ? ` · page ${value.page}` : ''} · ${id}`;
  $('source-text').textContent = value.text || JSON.stringify(section, null, 2); $('source-view').hidden = false;
}
$('source-close').onclick = () => { $('source-view').hidden = true; };
function closeSettings() { $('settings-view').hidden = true; $('settings').setAttribute('aria-expanded', 'false'); $('settings').focus(); }
function settingsTab(name) {
  for (const key of ['connection', 'backend', 'speech']) {
    $(`${key}-settings`).hidden = key !== name;
    $(`${key}-settings-tab`).classList.toggle('selected', key === name);
    $(`${key}-settings-tab`).setAttribute('aria-pressed', String(key === name));
  }
}
async function loadBackendSettings() {
  $('backend-fields').disabled = true;
  if (!state.authenticated) { $('backend-settings-note').textContent = 'Connect to a backend to configure it.'; return; }
  const generation = state.generation;
  const settings = await api('/settings');
  if (!state.authenticated || generation !== state.generation) return;
  backendSettings = settings;
  $('default-model').replaceChildren(...settings.models.map(model => { const option = node('option', `${model.name} · ${model.provider}`); option.value = model.id; return option; }));
  $('default-agent').replaceChildren(...settings.agents.map(agent => { const option = node('option', agent.name); option.value = agent.id; return option; }));
  $('default-model').value = settings.model; $('default-agent').value = settings.agent;
  $('backend-settings-note').textContent = 'These defaults belong to the connected backend.';
  $('backend-fields').disabled = false; $('apply-current').disabled = !state.id || state.active;
  $('apply-current').checked = Boolean(state.id && !state.active);
  $('agent-description').textContent = settings.agents.find(a => a.id === settings.agent)?.description || '';
}
$('settings').onclick = () => run(async () => {
  const open = $('settings-view').hidden;
  $('settings-view').hidden = !open; $('settings').setAttribute('aria-expanded', String(open));
  $('session-menu').hidden = true;
  if (open) {
    $('backend-url').value = connection.url; $('backend-secret').value = '';
    $('backend-secret').placeholder = connection.secret ? 'Secret already set for this tab' : 'Jarvis Server password';
    $('backend-save-status').textContent = '';
    await loadBackendSettings();
  }
});
$('settings-close').onclick = closeSettings;
for (const key of ['connection', 'backend', 'speech']) $(`${key}-settings-tab`).onclick = () => run(async () => { settingsTab(key); if (key === 'backend') await loadBackendSettings(); });
$('default-agent').onchange = () => { $('agent-description').textContent = backendSettings?.agents.find(a => a.id === $('default-agent').value)?.description || ''; };
$('connection-form').onsubmit = event => { event.preventDefault(); run(async () => {
  let url = $('backend-url').value.trim();
  if (url) {
    const parsed = new URL(url);
    if (!['http:', 'https:'].includes(parsed.protocol) || parsed.username || parsed.password || parsed.search || parsed.hash || parsed.pathname !== '/') throw new Error('Use an HTTP(S) server origin without a path, credentials or query.');
    url = parsed.origin;
  }
  const secret = $('backend-secret').value || (url === connection.url ? connection.secret : '');
  if (!secret) throw new Error('Enter the secret for this backend.');
  event.submitter.disabled = true;
  try {
    disconnect(); connection.url = url; connection.secret = secret;
    writeStorage('localStorage', 'jarvis.backend.url', url); writeStorage('sessionStorage', 'jarvis.backend.auth', null);
    await api('/auth', { method: 'POST', body: JSON.stringify({ token: secret }) }); rememberSecret();
    $('backend-secret').value = ''; await connect(); await loadBackendSettings();
    $('settings-connection-status').textContent = 'Connected. Connection saved for this browser.';
  } finally { event.submitter.disabled = false; }
}); };
$('backend-form').onsubmit = event => { event.preventDefault(); run(async () => {
  const selected = { model: $('default-model').value, agent: $('default-agent').value };
  const currentID = state.id; const apply = $('apply-current').checked && !$('apply-current').disabled && currentID;
  event.submitter.disabled = true; $('backend-save-status').textContent = '';
  try {
    await api('/settings', { method: 'PUT', body: JSON.stringify(selected) });
    $('backend-save-status').textContent = 'Defaults saved for new conversations.';
    if (apply) {
      try { await api(`/sessions/${currentID}/preferences`, { method: 'PUT', body: JSON.stringify(selected) }); $('backend-save-status').textContent = 'Defaults saved and applied to this conversation.'; }
      catch (e) { throw new Error(`Defaults saved. Could not fully apply them to this conversation: ${e.message}`); }
    }
    await status();
  } finally { event.submitter.disabled = false; }
}); };

disconnect(); renderMessages();
api('/auth').then(connect).catch(e => { if (!e.message.includes('password')) error(e.message); });
setInterval(() => { if (state.authenticated) run(async () => { await status(); if (state.tab === 'chat') await sessions(); }); }, 30000);

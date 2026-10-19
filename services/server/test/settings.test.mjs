import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { OpenCode, AGENTS } from '../src/opencode.mjs';

class SettingsOpenCode extends OpenCode {
  calls = [];
  async api(route, method = 'GET', body) {
    this.calls.push({ route, method, body });
    if (route.startsWith('/api/model/default')) return { data: null };
    if (route.startsWith('/api/model?')) return { data: ['sol', 'luna'].map(id => ({ id, name: id, providerID: 'openai', enabled: true, capabilities: { tools: true, output: ['text'] } })) };
    if (route === '/api/session' && method === 'POST') return { data: { id: 'ses_new', ...body } };
    if (route.startsWith('/api/session?')) return { data: Array.from({ length: 13 }, (_, i) => ({ id: `ses_${i}`, location: { directory: i === 12 ? path.dirname(this.config.workspace) : this.config.workspace }, time: { updated: i } })) };
    return { data: [] };
  }
}
async function setup(t) {
  const root = await mkdtemp(path.join(tmpdir(), 'jarvis-settings-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  return { workspace: path.join(root, 'workspace'), modelDirectory: root, model: 'openai/sol' };
}

test('defaults persist across restart, use live catalog, and select the home assistant for new sessions', async t => {
  const config = await setup(t); const oc = new SettingsOpenCode(config);
  const initial = await oc.settings();
  assert.equal(initial.agent, 'jarvis-home-assistant'); assert.equal(initial.models.length, 2);
  await oc.saveSettings({ model: 'openai/luna', agent: 'jarvis-home-assistant', secret: 'must-not-save' });
  const saved = JSON.parse(await readFile(oc.settingsFile, 'utf8'));
  assert.deepEqual(saved, { model: 'openai/luna', agent: 'jarvis-home-assistant' });
  const restarted = new SettingsOpenCode(config); const settings = await restarted.settings();
  assert.equal(settings.model, 'openai/luna');
  const session = await restarted.create();
  assert.equal(session.agent, 'jarvis-home-assistant'); assert.deepEqual(session.model, { providerID: 'openai', id: 'luna' });
  const generated = JSON.parse(await readFile(path.join(config.workspace, 'opencode.json'), 'utf8'));
  for (const agent of AGENTS) assert.ok(generated.agents[agent.id]);
  assert.ok(generated.agents['jarvis-web'].hidden, 'Existing conversations keep their old agent alias');
});

test('rejects invented models and unexposed agents; explicit current-session settings use OpenCode endpoints', async t => {
  const oc = new SettingsOpenCode(await setup(t)); await oc.ensureReady();
  await assert.rejects(oc.saveSettings({ model: 'invented/luna', agent: AGENTS[0].id }), /available model/);
  await assert.rejects(oc.saveSettings({ model: 'openai/luna', agent: 'build' }), /available Jarvis agent/);
  await oc.applyPreferences('ses_ours', { model: 'openai/luna', agent: AGENTS[1].id });
  const calls = oc.calls.filter(c => c.route.startsWith('/api/session/ses_ours'));
  assert.deepEqual(calls.map(c => c.route), ['/api/session/ses_ours/model', '/api/session/ses_ours/agent']);
  assert.deepEqual(calls[0].body, { model: { providerID: 'openai', id: 'luna' } });
});

test('recent sessions are the ten newest owned conversations', async t => {
  const oc = new SettingsOpenCode(await setup(t)); const sessions = await oc.sessions();
  assert.equal(sessions.length, 10); assert.equal(sessions[0].id, 'ses_11'); assert.equal(sessions.at(-1).id, 'ses_2');
});

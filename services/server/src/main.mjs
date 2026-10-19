import { readFile } from 'node:fs/promises';
import { parseEnv } from 'node:util';
import path from 'node:path';
import { createServer } from './server.mjs';

let knowledge = {};
const knowledgeFile = process.env.JARVIS_KNOWLEDGE_ENV_FILE ?? '../knowledge/.env';
if (knowledgeFile) {
  try { knowledge = parseEnv(await readFile(knowledgeFile, 'utf8')); }
  catch (error) { if (error.code !== 'ENOENT') throw new Error('Could not load the knowledge .env file.'); }
}
const config = {
  token: process.env.JARVIS_SERVER_TOKEN,
  origins: (process.env.JARVIS_WEB_ORIGINS || 'http://localhost:47833,http://127.0.0.1:47833').split(',').map(s => s.trim()),
  secureCookie: process.env.JARVIS_COOKIE_SECURE === 'true',
  workspace: path.resolve(process.env.JARVIS_WORKSPACE || '.data/workspace'),
  modelDirectory: path.resolve('../../sessions'),
  model: process.env.OPENCODE_MODEL || '',
  agent: process.env.JARVIS_DEFAULT_AGENT || 'jarvis-home-assistant',
  opencodeURL: process.env.OPENCODE_URL || '',
  opencodeUsername: process.env.OPENCODE_USERNAME || 'opencode',
  opencodePassword: process.env.OPENCODE_PASSWORD || '',
  knowledgeURL: process.env.JARVIS_KNOWLEDGE_URL || (knowledgeFile ? knowledge.JARVIS_KNOWLEDGE_URL || 'http://127.0.0.1:47831' : ''),
  knowledgeToken: process.env.JARVIS_KNOWLEDGE_TOKEN || knowledge.JARVIS_KNOWLEDGE_TOKEN || '',
};
const server = createServer(config);
const host = process.env.JARVIS_SERVER_HOST || '127.0.0.1';
const port = Number(process.env.JARVIS_SERVER_PORT || 47832);
server.listen(port, host, () => console.log(`Jarvis Server: http://${host}:${port} (OpenCode credentials stay on the server)`));
server.on('error', error => { console.error(`Jarvis Server could not listen (${error.code || 'unknown error'}).`); process.exitCode = 1; });
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => {
  server.close(); server.closeAllConnections();
  setTimeout(() => process.exit(), 1000).unref();
});

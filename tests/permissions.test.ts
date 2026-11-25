import { test } from 'node:test';
import assert from 'node:assert/strict';
import { permissionDetails } from '../src/permissions.ts';
import type { Agent } from '../src/types.ts';

test('permission disclosures name provider boundaries, remembered access and history deletion', () => {
  const details = (id: Agent['id']) => permissionDetails({ id } as Agent).join(' ');
  for (const id of ['codex', 'claude', 'gemini', 'ollama', 'lmstudio'] as const) {
    assert.match(details(id), /remembered locally across restarts/);
    assert.match(details(id), /Revoke deletes.*saved monitor history and permission/);
    assert.match(details(id), /not the native tool’s credentials or transcripts/);
    assert.match(details(id), /No prompts/);
  }
  assert.match(details('codex'), /refresh provider-managed credentials/);
  assert.match(details('claude'), /subscription tokens are not collected/);
  assert.match(details('gemini'), /Internal Google quota endpoints/);
  assert.match(details('ollama'), /GET \/api\/tags and GET \/api\/ps/);
  assert.match(details('lmstudio'), /GET \/api\/v0\/models/);
});

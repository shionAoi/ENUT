import { tr, type Lang, type TranslationKey } from './i18n.ts';
import type { Agent, ContextInfo } from './types.ts';
import { escapeHtml as e } from './view.ts';
import { accessButton, selectedTool, toolButtons } from './usage-view.ts';

const statusKeys: Record<ContextInfo['mcps'][number]['status'], TranslationKey> = {
  connected: 'statusConnected',
  configured: 'statusConfigured',
  pending: 'statusPending',
  disabled: 'statusDisabled',
  failed: 'statusFailed',
  needs_auth: 'statusNeedsAuth',
  unknown: 'statusUnknown',
};

export function renderContext(agents: Agent[], id: string | null, desktop: boolean, approved: Set<string>, lang: Lang = 'en'): string {
  const agent = selectedTool(agents, id);
  if (!agent) return `<p class="unavailable">${tr(lang, 'noInstalledToolsFull')}</p><p class="meta">${tr(lang, 'contextOpenDesktop')}</p>`;
  const heading = `${toolButtons(agents, agent.id, 'context', lang)}<div class="selected-heading"><h3>${e(agent.name)}</h3>${accessButton(agent, desktop, approved, lang)}</div>`;
  if (!approved.has(agent.id) || !agent.connected) return `${heading}<p class="unavailable">${approved.has(agent.id) ? tr(lang, 'contextUnavailableDisconnected') : tr(lang, 'permissionRequired')}</p><p class="meta">${tr(lang, 'contextConnectHint')}</p>`;
  const context = agent.context;
  if (!context) return `${heading}<p class="unavailable">${tr(lang, 'contextUnavailable')}</p><p class="meta">${tr(lang, 'contextUnsupported')}</p>`;
  return `${heading}<section class="usage-section"><h3>${tr(lang, 'mcpTitle')}</h3><p class="meta">${tr(lang, 'mcpConfiguredNote')}</p>${context.mcps.length ? `<ul class="context-list">${context.mcps.map(m => `<li><span>${e(m.name)}</span><span class="context-status">${tr(lang, statusKeys[m.status] ?? 'statusUnknown')}</span></li>`).join('')}</ul>` : `<p class="unavailable">${tr(lang, 'noMcps')}</p>`}</section><section class="usage-section"><h3>${tr(lang, 'skillsTitle')}</h3>${context.skills.length ? `<ul class="context-list">${context.skills.map(name => `<li>${e(name)}</li>`).join('')}</ul>` : `<p class="unavailable">${tr(lang, 'noSkills')}</p>`}</section>${context.note ? `<p class="connector-note">${e(context.note)}</p>` : ''}<p class="meta">${tr(lang, 'contextNamesOnly')}</p>`;
}

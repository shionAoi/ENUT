import { invoke, isTauri } from '@tauri-apps/api/core';
import { Monitor } from './controller.ts';
import { resolveLang, saveLang, tr, type Lang, type TranslationKey } from './i18n.ts';
import { freshness, pollingDelay, resolveTheme, saveTheme, selectAgents } from './logic.ts';
import { escapeHtml as e, renderTools, renderBilling } from './view.ts';
import { renderUsage, selectedTool } from './usage-view.ts';
import { usagePeriods, type ModelFilter, type UsagePeriod } from './total-usage.ts';
import { renderContext } from './context-view.ts';
import { renderEzControl } from './ez-control-view.ts';
import { renderShell } from './shell.ts';
import type { Agent, KindFilter, ProcessControlSnapshot, Snapshot, Theme } from './types.ts';
import { icon } from './icons.ts';
import { permissionDetails } from './permissions.ts';
import { mountAuth } from './auth-ui.ts';
import { connectIntegration } from './connect.ts';
import { connectionHint } from './connection-actions.ts';
import './style.css';
import './toolbox.css';

let savedLang: string | null = null;
try { savedLang = localStorage.getItem('ai-monitor-lang'); } catch { /* Optional preference storage. */ }
let lang: Lang = resolveLang(savedLang);
document.documentElement.lang = lang;

const root = document.querySelector<HTMLDivElement>('#app')!;
root.innerHTML = renderShell(lang);

const get = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const desktop = isTauri();
document.documentElement.dataset.desktop = String(desktop);
get('hide-window').hidden = !desktop;
root.classList.add('compact');
const expanded = new Set<string>();
let activePage = 'tools';
let filter: KindFilter = 'all';
let usageTool = 'total';
let usageModel: ModelFilter = null;
let contextTool: string | null = null;
let controlSnapshot: ProcessControlSnapshot | null = null;
const collapsedControl = new Set<string>();
let controlBusy = false;
let controlTimer: ReturnType<typeof setTimeout> | undefined;
let period: UsagePeriod = 30;
let compact = true;
let pendingAgent: Agent | null = null;
let pendingLogin = false;
let pendingBilling: 'openai' | 'anthropic' | null = null;
let timer: ReturnType<typeof setTimeout> | undefined;

async function refreshControl() {
  clearTimeout(controlTimer);
  if (!desktop || document.hidden || activePage !== 'control' || controlBusy) return;
  controlBusy = true;
  try {
    controlSnapshot = await invoke<ProcessControlSnapshot>('read_process_control');
    if (activePage === 'control') render();
  } catch { /* Keep previous control reading on transient sampling error. */ }
  finally {
    controlBusy = false;
    scheduleControl();
  }
}

function scheduleControl() {
  clearTimeout(controlTimer);
  if (desktop && !document.hidden && activePage === 'control') {
    controlTimer = setTimeout(() => { void refreshControl(); }, 3000);
  }
}
const dialog = get<HTMLDialogElement>('permission');
const billingDialog = get<HTMLDialogElement>('billing-dialog');
const storageDialog = get<HTMLDialogElement>('usage-storage');

get('usage-storage-button').addEventListener('click', async () => {
  const path = get('usage-storage-path');
  path.textContent = desktop ? tr(lang, 'storageLocating') : tr(lang, 'storageDesktopOnly');
  storageDialog.showModal();
  if (!desktop) return;
  try {
    path.textContent = await invoke<string>('usage_storage_directory');
  } catch {
    path.textContent = tr(lang, 'storageUnavailable');
  }
});

// Tauri 2 arguments are camelCase: https://v2.tauri.app/develop/calling-rust/#passing-arguments
const monitor = new Monitor(desktop, (command, args) => invoke<Snapshot>(command, args), render, () => lang);

function applyLanguage() {
  document.documentElement.lang = lang;
  root.querySelectorAll<HTMLElement>('[data-i18n]').forEach(el => {
    const key = el.dataset.i18n as TranslationKey;
    el.textContent = tr(lang, key);
  });
  root.querySelectorAll<HTMLElement>('[data-i18n-html]').forEach(el => {
    const key = el.dataset.i18nHtml as TranslationKey;
    el.innerHTML = tr(lang, key);
  });
  const hide = get('hide-window');
  hide.title = tr(lang, 'hideWindowTitle');
  hide.setAttribute('aria-label', tr(lang, 'hideWindowTitle'));
  get('compact').textContent = tr(lang, compact ? 'roomyLayout' : 'compactLayout');
  const langBtn = get('language');
  langBtn.dataset.lang = lang;
  langBtn.textContent = tr(lang, 'languageToggle');
  root.querySelector('.toolbox-nav')?.setAttribute('aria-label', tr(lang, 'navLabel'));
  root.querySelector('.filter')?.setAttribute('aria-label', tr(lang, 'filterGroupLabel'));
}

function render() {
  const focused = document.activeElement instanceof HTMLButtonElement ? document.activeElement : null;
  const focusAgent = focused?.dataset.agent;
  const focusAction = focused?.dataset.action;
  const focusTool = focused?.dataset.tool;
  const focusSection = focused?.dataset.section;
  const focusPeriod = focused?.dataset.period;
  const focusModel = focused?.dataset.usageModel;
  const focusModelAgent = focused?.dataset.usageAgent;
  const focusClearModel = focused?.dataset.clearModel;
  const focusControlAgent = focused?.dataset.controlAgent;
  const focusHelp = focused?.matches('.quota-help') ? focused.id : null;
  const focusRegion = focused?.closest('.toolbox-page')?.id;
  const agents = selectAgents(monitor.snapshot.agents, filter);
  const billing = (monitor.snapshot.billing ?? []).filter(r => agents.some(a => (r.provider === 'openai' && a.id === 'codex') || (r.provider === 'anthropic' && a.id === 'claude')));
  get('billing').innerHTML = renderBilling(agents, billing, desktop, lang);
  get('agents').innerHTML = renderTools(agents, 'quotas', desktop, monitor.approved, expanded, Date.now(), lang);
  if (usageTool !== 'total' && !monitor.snapshot.agents.some(agent => agent.installed && agent.id === usageTool)) {
    usageTool = 'total';
    usageModel = null;
  }
  contextTool = selectedTool(monitor.snapshot.agents, contextTool)?.id ?? null;
  get('usage-content').innerHTML = renderUsage(monitor.snapshot.agents, usageTool, period, desktop, monitor.approved, Date.now(), lang, usageModel);
  get('context-content').innerHTML = renderContext(monitor.snapshot.agents, contextTool, desktop, monitor.approved, lang);
  get('control-content').innerHTML = renderEzControl(controlSnapshot, monitor.approved, collapsedControl, desktop, lang);
  get('tool-count').textContent = String(monitor.snapshot.agents.filter(a => a.installed).length);
  get('agents').setAttribute('aria-busy', String(monitor.busy));
  const notices = [monitor.notice].filter(Boolean);
  get('warnings').textContent = monitor.snapshot.warnings.join(' ');
  get('notice').textContent = notices.join(' · ');
  get('notice').hidden = !notices.length;
  const count = agents.length;
  const filterLabel = filter === 'all' ? (lang === 'es' ? 'Todos los tipos' : 'All kinds') : filter === 'cloud' ? tr(lang, 'filterCloud') : tr(lang, 'filterLocal');
  const unitLabel = lang === 'es'
    ? `${count} ${count === 1 ? 'integración instalada' : 'integraciones instaladas'}`
    : `${count} installed ${count === 1 ? 'integration' : 'integrations'}`;
  get('scope').textContent = desktop ? `${unitLabel} · ${filterLabel}` : (lang === 'es' ? 'La detección de escritorio no está disponible en la vista previa del navegador' : 'Desktop discovery is not available in browser preview');
  get('detection').textContent = desktop ? (lang === 'es' ? `${count} detectadas en vista` : `${count} detected in view`) : (lang === 'es' ? 'Detección de escritorio' : 'Desktop discovery');
  get('freshness').textContent = monitor.busy ? (lang === 'es' ? 'Actualizando…' : 'Updating…') : desktop ? freshness(monitor.snapshot.scannedAt, Date.now(), lang) : (lang === 'es' ? 'Vista previa · sin acceso a datos' : 'Preview · no data access');
  const refresh = get<HTMLButtonElement>('refresh');
  refresh.disabled = !desktop || monitor.busy;
  refresh.title = desktop ? tr(lang, 'refreshTitle') : tr(lang, 'refreshRequiredTitle');
  refresh.setAttribute('aria-label', tr(lang, 'refreshLabel'));
  if (focused && focusRegion) {
    const replacement = [...get(focusRegion).querySelectorAll<HTMLButtonElement>('button')]
      .find(b =>
        focusAgent && focusAction ? b.dataset.agent === focusAgent && b.dataset.action === focusAction
          : focusTool ? b.dataset.tool === focusTool && b.dataset.section === focusSection
            : focusPeriod ? b.dataset.period === focusPeriod
              : focusModel && focusModelAgent ? b.dataset.usageModel === focusModel && b.dataset.usageAgent === focusModelAgent
                : focusClearModel ? b.dataset.clearModel === focusClearModel
                  : focusControlAgent ? b.dataset.controlAgent === focusControlAgent
                    : false);
    replacement?.focus({ preventScroll: true });
    if (focusHelp) document.getElementById(focusHelp)?.focus({ preventScroll: true });
  }
}

let explicitTheme = false;
let saved: string | null = null;
try { saved = localStorage.getItem('ai-monitor-theme'); } catch { /* Optional preference storage. */ }
explicitTheme = saved === 'light' || saved === 'dark';
const osTheme = window.matchMedia('(prefers-color-scheme: dark)');
let theme: Theme = resolveTheme(saved, osTheme.matches);
function applyTheme() {
  document.documentElement.dataset.theme = theme;
  const button = get<HTMLButtonElement>('theme');
  button.innerHTML = `${icon(theme === 'light' ? 'moon' : 'sun')}<span class="sr-only">${tr(lang, theme === 'light' ? 'themeDarkMode' : 'themeLightMode')}</span>`;
  button.setAttribute('aria-label', tr(lang, theme === 'light' ? 'switchToDark' : 'switchToLight'));
}
get('theme').addEventListener('click', () => {
  theme = theme === 'light' ? 'dark' : 'light';
  explicitTheme = true;
  try { saveTheme(localStorage, theme); } catch { /* Accessing storage itself may fail. */ }
  applyTheme();
});
osTheme.addEventListener('change', () => { if (!explicitTheme) { theme = resolveTheme(null, osTheme.matches); applyTheme(); } });
get('compact').addEventListener('click', () => {
  compact = !compact;
  root.classList.toggle('compact', compact);
  get('compact').setAttribute('aria-pressed', String(compact));
  get('compact').textContent = tr(lang, compact ? 'roomyLayout' : 'compactLayout');
});
get('language').addEventListener('click', () => {
  lang = lang === 'en' ? 'es' : 'en';
  try { saveLang(localStorage, lang); } catch { /* Accessing storage itself may fail. */ }
  applyLanguage();
  applyTheme();
  auth.setLang();
  render();
});
get('hide-window').addEventListener('click', () => {
  void invoke('hide_to_tray').catch(() => {
    monitor.setNoticeKey('noticeTrayUnavailable');
    render();
  });
});

const toolList = get('agents');
toolList.addEventListener('pointerenter', event => {
  if (event.target instanceof HTMLElement && event.target.matches('.quota-help-wrap')) event.target.removeAttribute('data-dismissed');
}, true);
toolList.addEventListener('focusin', event => {
  if (event.target instanceof HTMLElement) event.target.closest('.quota-help-wrap')?.removeAttribute('data-dismissed');
});
document.addEventListener('keydown', event => {
  if (event.key === 'Escape') {
    toolList.querySelectorAll('.quota-help-wrap:is(:hover, :focus-within)').forEach(wrap => wrap.setAttribute('data-dismissed', ''));
  }
});

root.addEventListener('click', event => {
  const button = (event.target as Element).closest<HTMLButtonElement>('button');
  if (!button) return;
  if (button.dataset.page) {
    activePage = button.dataset.page;
    for (const page of ['tools', 'usage', 'context', 'control', 'settings']) get(`page-${page}`).hidden = page !== activePage;
    root.querySelectorAll<HTMLButtonElement>('[data-page]').forEach(item => {
      if (item === button) item.setAttribute('aria-current', 'page');
      else item.removeAttribute('aria-current');
    });
    if (activePage === 'control') void refreshControl();
    else clearTimeout(controlTimer);
    return;
  }
  if (button.dataset.controlAgent) {
    const id = button.dataset.controlAgent;
    if (collapsedControl.has(id)) collapsedControl.delete(id);
    else collapsedControl.add(id);
    render();
    return;
  }
  if (button.dataset.billing === 'openai' || button.dataset.billing === 'anthropic') {
    const provider = button.dataset.billing;
    if (button.dataset.billingAction === 'disconnect') void monitor.billing(provider).then(schedule);
    else {
      pendingBilling = provider;
      get('billing-dialog-title').textContent = tr(lang, 'connectReportingTitle', { provider: provider === 'openai' ? 'OpenAI' : 'Anthropic' });
      get<HTMLInputElement>('reporting-key').value = '';
      billingDialog.showModal();
    }
    return;
  }
  if (button.dataset.filter) {
    filter = button.dataset.filter as KindFilter;
    root.querySelectorAll<HTMLButtonElement>('[data-filter]').forEach(b => b.setAttribute('aria-pressed', String(b === button)));
    render();
  }
  if (button.dataset.usageModel && button.dataset.usageAgent) {
    const next = { model: button.dataset.usageModel, agentId: button.dataset.usageAgent };
    usageModel = usageModel?.agentId === next.agentId && usageModel?.model === next.model ? null : next;
    render();
    return;
  }
  if (button.dataset.clearModel) {
    usageModel = null;
    render();
    return;
  }
  if (button.dataset.tool) {
    if (button.dataset.section === 'context') contextTool = button.dataset.tool;
    else {
      usageTool = button.dataset.tool;
      usageModel = null;
    }
    render();
    return;
  }
  const selectedPeriod = usagePeriods.find(days => String(days) === button.dataset.period);
  if (selectedPeriod !== undefined) { period = selectedPeriod; render(); return; }
  const agent = monitor.snapshot.agents.find(a => a.id === button.dataset.agent);
  if (!agent) return;
  if (button.dataset.action === 'details') {
    if (expanded.has(agent.id)) expanded.delete(agent.id);
    else expanded.add(agent.id);
    render();
  }
  if (button.dataset.action === 'revoke') {
    void monitor.permission(agent.id, false).then(() => {
      schedule();
      if (activePage === 'control') void refreshControl();
    });
  }
  if (button.dataset.action === 'connect' || button.dataset.action === 'login') {
    pendingAgent = agent;
    pendingLogin = (button.dataset.action === 'login' && agent.authentication === 'required') || (button.dataset.action === 'connect' && agent.id === 'antigravity');
    get('permission-title').textContent = `${pendingLogin ? tr(lang, 'loginTo') : tr(lang, 'connectLabel')} ${agent.name}`;
    const loginBullet = lang === 'es'
      ? ['El inicio de sesión nativo puede abrir un navegador y cambiar la cuenta activa de la herramienta. Las credenciales permanecen en el CLI nativo. El monitoreo se pausa; cancelar nunca otorga acceso.']
      : ['Native sign-in may open a browser and change the tool’s active account. Credentials stay with the native CLI. Monitoring pauses; cancelling never grants access.'];
    get('permission-details').innerHTML = [connectionHint(agent, lang), ...permissionDetails(agent, lang), ...(pendingLogin ? loginBullet : [])].map(detail => `<li>${e(detail)}</li>`).join('');
    get('allow').textContent = tr(lang, pendingLogin ? 'allowNativeLogin' : 'allowRemember');
    dialog.showModal();
  }
});

dialog.addEventListener('close', () => {
  const agent = pendingAgent;
  const login = pendingLogin;
  pendingAgent = null;
  pendingLogin = false;
  if (dialog.returnValue === 'allow' && agent) {
    const grant = async () => {
      await monitor.permission(agent.id, true);
      await monitor.refresh();
      schedule();
      if (activePage === 'control') void refreshControl();
    };
    void (login ? auth.openConnect(agent.id, grant) : connectIntegration(agent.id, auth.openConnect, grant)).catch(() => {
      monitor.setNoticeKey('noticeConnectFailed');
      render();
    });
  }
  dialog.returnValue = '';
});

billingDialog.addEventListener('close', () => {
  const provider = pendingBilling;
  pendingBilling = null;
  const field = get<HTMLInputElement>('reporting-key');
  let credential = field.value;
  field.value = '';
  if (billingDialog.returnValue === 'connect' && provider) void monitor.billing(provider, credential).then(schedule);
  credential = '';
  billingDialog.returnValue = '';
});

async function update() {
  clearTimeout(timer);
  if (!desktop || document.hidden) return;
  // Discovery reads installation state only; refresh remains permission-gated in the backend.
  await monitor.refresh(true);
  if (!monitor.failures) await monitor.refresh();
  schedule();
}
function schedule() {
  clearTimeout(timer);
  if (desktop && !document.hidden) timer = setTimeout(() => { void update(); }, pollingDelay(monitor.failures));
}
get('refresh').addEventListener('click', () => { void update(); if (activePage === 'control') void refreshControl(); });
document.addEventListener('visibilitychange', () => {
  clearTimeout(timer);
  clearTimeout(controlTimer);
  if (!document.hidden) {
    render();
    void update();
    if (activePage === 'control') void refreshControl();
  }
});
// Local countdown/freshness labels do not trigger IPC or network operations.
setInterval(() => { if (!document.hidden && !dialog.open && !billingDialog.open) render(); }, 15000);
applyLanguage();
applyTheme();
const auth = mountAuth(root, desktop, async tool => {
  const agent = monitor.snapshot.agents.find(a => a.id === tool);
  if (agent) await monitor.pause(agent.id);
}, async () => {
  await monitor.refresh(true);
  render();
}, () => lang);
render();
if (desktop) void update();

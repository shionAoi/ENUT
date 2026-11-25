import { tr, type Lang } from './i18n.ts';
import type { Agent } from './types.ts';

export function connectionAction(
  agent: Agent,
  approved: Set<string>,
  lang: Lang = 'en',
): { label: string; action: 'login' | 'connect' | 'revoke' } {
  if (agent.authentication === 'required') return { label: tr(lang, 'loginLabel'), action: 'login' };
  if (agent.connected && approved.has(agent.id)) return { label: tr(lang, 'revokeLabel'), action: 'revoke' };
  return { label: tr(lang, 'connectLabel'), action: 'connect' };
}

export function connectionHint(agent: Agent, lang: Lang = 'en'): string {
  if (lang === 'es') {
    if (agent.authentication === 'required') return 'La herramienta nativa indica que se requiere iniciar sesión. Iniciar sesión abre un flujo explícito de dispositivo o código manual; complétalo antes de conectar el monitoreo.';
    if (agent.id === 'gemini') return 'Conectar permite leer metadatos locales de tokens de Gemini CLI sin iniciar sesión. Revocar elimina la aprobación y los agregados guardados.';
    if (agent.authentication === 'notRequired' || agent.kind === 'local') return 'No se requiere inicio de sesión de proveedor para estos metadatos locales. Conectar recuerda el permiso en este dispositivo; Revocar elimina el historial guardado y la aprobación.';
    if (agent.id === 'antigravity') return 'Conectar inicia el inicio de sesión nativo de Google en el navegador con un modal de código de autorización. Terminar y conectar recuerda el permiso; los inicios posteriores reutilizan la sesión sin abrir el navegador. Revocar elimina el historial guardado y la aprobación, no las credenciales nativas.';
    return 'Conectar otorga permiso para consultar el estado nativo, los registros locales de tokens (cuando corresponda) y reutilizar una sesión guardada. No inicia sesión automáticamente.';
  }
  if (agent.authentication === 'required') return 'The native tool reports login is required. Login starts an explicit native device or manual sign-in flow; finish it before connecting monitoring.';
  if (agent.id === 'gemini') return 'Connect permits reading local Gemini CLI token metadata, without starting sign-in. Revoke removes monitoring approval and saved aggregates.';
  if (agent.authentication === 'notRequired' || agent.kind === 'local') return 'No provider login is required for this local metadata. Connect remembers monitoring permission on this device; Revoke deletes its saved history and approval.';
  if (agent.id === 'antigravity') return 'Connect starts native Google sign-in with a browser and an authorization-code modal. Finish and connect remembers monitoring permission; later app starts reuse the CLI login without opening a browser. Revoke deletes saved monitor history and approval, not native credentials.';
  return 'Connect grants permission to check native status, read local rollout/session token logs where supported, and reuse a saved login. It does not start sign-in. Unavailable status or quotas do not mean you are logged out.';
}

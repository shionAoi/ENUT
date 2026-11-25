import type { Lang } from './i18n.ts';
import type { Agent } from './types.ts';

export function permissionDetails(agent: Agent, lang: Lang = 'en'): string[] {
  if (lang === 'es') {
    const commonEs = [
      'El permiso de monitoreo y el historial de uso normalizado se guardan localmente entre reinicios. Revocar elimina el historial guardado por el monitor y el permiso de esta integración, no las credenciales ni transcripciones de la herramienta nativa.',
      'Mientras la pestaña EZ Control esté visible, lee en vivo el uso de RAM, CPU y nombres saneados de subprocesos de las herramientas aprobadas, sin variables de entorno ni credenciales.',
      'No se envían prompts, chats, completados, embeddings, cambios de presupuesto ni solicitudes de generación de modelos.',
    ];
    if (agent.id === 'ollama') return [
      'Lee nombres de modelos, tamaños y estado cargado usando solo GET /api/tags y GET /api/ps en http://127.0.0.1:11434.',
      'Alcance: entorno local de este equipo. No se leen inicios de sesión en la nube, registros de uso ni credenciales.', ...commonEs];
    if (agent.id === 'lmstudio') return [
      'Lee el inventario de modelos reportado usando solo GET /api/v0/models en http://127.0.0.1:1234.',
      'Alcance: entorno local de este equipo. No se leen inicios de sesión en la nube, registros de uso ni credenciales.', ...commonEs];
    const detailsEs: Record<string, string[]> = {
      codex: [
        'Comprueba metadatos nativos de cuenta y usa operaciones compatibles de cuota de Codex. Un resultado explícito de autenticación requerida habilita Iniciar sesión; cuotas no disponibles o modo API key no significan sesión cerrada. No se publican identificadores ni archivos de credenciales.',
        'Reutilizar el inicio de sesión de Codex puede actualizar credenciales administradas por el proveedor. No se inicia ningún turno de modelo.',
        'Lee contadores de tokens compatibles en registros locales bajo ~/.codex/sessions y ~/.codex/archived_sessions (método ccusage) hasta por 90 días UTC sin seguir enlaces simbólicos.',
        'También detecta nombres acotados de configuración MCP local y habilidades instaladas sin publicar comandos, URLs, rutas privadas ni contenido.',
      ],
      claude: [
        'Lee metadatos JSONL reconocidos bajo los directorios .claude/projects (y ~/.config/claude/projects), incluidos subagentes (método ccusage), hasta 90 días UTC sin seguir enlaces simbólicos.',
        'Los tokens de suscripción de Claude no se recopilan ni reutilizan. Los informes de facturación de API requieren una integración independiente.',
        'También detecta nombres de configuración MCP local y habilidades instaladas sin publicar comandos, rutas ni cuerpos.',
      ],
      antigravity: [
        'Conectar inicia el flujo nativo de Google con código de autorización si se solicita, y lee cuotas mediante agy -p /usage --output-format json junto con conversaciones locales (método ccusage) y metadatos MCP/habilidades.',
        'No se inicia ningún turno de agente y no se publican credenciales, prompts ni contenido privado.',
      ],
      hermes: [
        'Ejecuta hermes usage --json para límites del proveedor, lee el uso histórico local en ~/.hermes/state.db en modo solo lectura (método ccusage) y descubre nombres locales de MCP/habilidades.',
        'No se inicia ninguna sesión de chat ni se publican prompts o contenido.',
      ],
      opencode: [
        'Usa opencode api get para estadísticas, límites, sesiones activas, MCP y habilidades, además de la base de datos local de sesiones en modo solo lectura (método ccusage).',
        'Solo se publican tokens normalizados, cuotas, estado MCP y nombres de habilidades; nunca prompts ni credenciales.',
      ],
      gemini: [
        'Lee metadatos de sesión JSON/JSONL bajo ~/.gemini/tmp hasta por 90 días UTC (método ccusage), sin seguir enlaces simbólicos.',
        'Los tokens registrados reflejan el estado guardado de la transcripción, no un libro contable de facturación.',
      ],
    };
    return [
      ...(detailsEs[agent.id] ?? []),
      'Los registros locales pueden contener texto privado de conversaciones. Este monitor muestra solo metadatos de tokens normalizados y no guarda ni registra prompts.',
      'Las operaciones no disponibles siguen siendo no disponibles; el permiso no garantiza soporte de reporte.',
      ...commonEs,
    ];
  }

  const common = ['Monitoring permission and normalized usage history are remembered locally across restarts. Revoke deletes this integration’s saved monitor history and permission, not the native tool’s credentials or transcripts.',
    'While the EZ Control tab is open, it reads live local RAM, CPU, and sanitized subprocess task names for approved tools without reading environment variables or credentials.',
    'No prompts, chat, completions, embeddings, budget changes or model generation requests are made.'];
  if (agent.id === 'ollama') return [
    'Read model names, sizes and loaded state using GET /api/tags and GET /api/ps on http://127.0.0.1:11434 only.',
    'Scope: this computer’s local runtime. No cloud sign-in, usage logs or credentials are read.', ...common];
  if (agent.id === 'lmstudio') return [
    'Read reported model inventory using GET /api/v0/models on http://127.0.0.1:1234 only.',
    'Scope: this computer’s local runtime. No cloud sign-in, usage logs or credentials are read.', ...common];
  const details: Record<string, string[]> = {
    codex: ['Check native account metadata without refreshing credentials, then use supported Codex quota operations for ChatGPT sign-in. An explicit native authentication-required result enables Login; unavailable subscription quotas or API-key mode do not mean signed out. No account identifiers or credential files are published.',
      'Reusing Codex sign-in can refresh provider-managed credentials. No model turn is started.',
      'Read supported local rollout token counters under ~/.codex/sessions and ~/.codex/archived_sessions (ccusage method) for up to 90 UTC days without following symlinks.',
      'Also discover bounded local MCP configuration names and installed skill names. Configured does not mean connected; no MCP commands, URLs, environment values, private paths or skill bodies are published.'],
    claude: ['Read recognized JSONL session metadata under your home .claude/projects directory, including subagents (ccusage method). Scope: this computer’s last 90 UTC days, not your subscription balance. No directory/file symlinks are followed. Native active-session metadata is used only where its schema is supported; saved transcripts alone never prove a session is currently running.',
      'The monitor retains up to 90 UTC days of observed token aggregates.',
      'Claude subscription tokens are not collected or reused. API billing requires a separate reporting integration, not a subscription login.',
      'Also discover bounded local MCP configuration names and installed skill names. Configuration is not proof of a live connection; commands, URLs, environment values, private paths and skill bodies are not published.'],
    antigravity: ['Connect starts explicit native Google sign-in using the supported CLI 1.5.0 or newer. Return from the browser and paste the authorization code in this app’s modal; it is sent only to the same owned native process and is not saved. Select Finish and connect to enable monitoring. Cancelling never grants access.',
      'Use a recognized stable CLI version 1.1.11 or newer with agy -p /usage --output-format json for provider-reported quotas and resets, and read local Antigravity conversation databases in read-only mode (ccusage method). No agent turn is started.',
      'The CLI uses provider-managed saved sign-in and may refresh it. This monitor does not directly read credentials.',
      'Optionally discover bounded local MCP configuration names and installed skill names. Configured does not prove connected. No commands, URLs, environment values, private paths or skill content are published.'],
    hermes: ['Run hermes usage --json for configured provider account status, and read ~/.hermes/state.db in read-only mode for local token history (ccusage method). No session or inference is started; the CLI may refresh saved sign-in.',
      'Discover bounded local configuration and installed skill names. Configured MCPs are not proven connected. No commands, URLs, environment values, private paths or skill content are published.'],
    opencode: ['Use opencode api get for allowlisted stats, limits, active sessions, mcp and skill status through native CLI authentication discovery, and read OpenCode’s local SQLite/JSON session database in read-only mode (ccusage method).',
      'Publish only normalized tokens, quota, MCP status and skill names. Prompts, skill bodies, raw errors, credentials and private paths are not published.'],
    gemini: ['Read recognized JSON/JSONL session metadata under your home .gemini/tmp directories (ccusage method) and retain up to 90 UTC days of token aggregates on this device. This is not your account quota. No directory/file symlinks are followed.',
      'Recorded tokens reflect saved transcript state, not a lifetime ledger. Deleted, rewound, unsupported, or unreadable records may be missing. Scans are bounded to 4,096 entries and 64 MiB, with 16 MiB per file.',
      'Internal Google quota endpoints and unsupported billing connections are not used.'],
  };
  return [...(details[agent.id] ?? []),
    'Local usage logs may contain private conversation content. Only normalized token metadata is displayed; prompts and conversation text are not stored or logged by this monitor.',
    'Unavailable connector operations remain unavailable; permission does not guarantee reporting support.', ...common];
}

# OpenCode V2

After session permission, use the native CLI's authenticated API discovery with fixed GET status paths: `/api/experimental/session/stats`, `/api/experimental/usage/limits`, `/api/mcp`, `/api/skill`. This may start the local background service. No conversation-content or generation endpoint is requested.

Session statistics provide token/model totals; activity counts steps, not daily tokens. Only supported Go/Console budgets are allowances, not all external-provider subscription quotas. Context allowlists MCP names/status and skill names, dropping paths, bodies and raw errors. Endpoint failures remain independent and explicit.

Sources: https://opencode.ai/docs/api and https://opencode.ai/docs/openapi.json. The adapter targets V2; unsupported versions fail without a model fallback.

Official v2.1.2 `packages/schema/src/token-usage.ts` defines total as input + output + reasoning + cache.read + cache.write. The published limits contract defines meter percent as used allowance and reset times as epoch milliseconds; the first supporting CLI release was not established. Unsupported limits endpoints remain unknown independently of statistics and Context endpoints. Console per-model budget spend is not translated into a provider-wide remaining percentage.

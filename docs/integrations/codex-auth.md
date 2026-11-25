# Codex native device sign-in

This transport implements S3/S12's authentication-only Codex device flow. The
parent manager must gate explicit sign-in separately from monitoring permission,
serialize native operations, enforce the 15-minute attempt lifetime, and call
`cancel` on explicit cancellation or expiry. Successful sign-in does **not** grant
monitoring permission. Native Codex owns credential storage; this app never reads
its credential files, accepts tokens, or receives a pasted authorization code.

Desktop launches also support the Linux fnm default Codex installation at
`~/.local/share/fnm/aliases/default/bin/codex`. Status reads and device login reuse
one command builder: when the resolved executable directory is missing from PATH,
it is added only to that Codex child environment for the Node shebang launcher.
An existing terminal PATH is not reordered when the directory is already present.
No shell profile, global environment, credential or account setting is changed;
this is executable/runtime discovery, not an automatic authentication attempt.

## Backend interface

Before offering Login, monitoring consent allows a status-only `account/read`
with `refreshToken: false`. An explicit `account: null` together with boolean
`requiresOpenaiAuth: true` is authentication-required evidence; it returns no
quotas and never requests subscription limits. A ChatGPT account reports
available authentication and follows the existing quota read. An `apiKey`
account also reports available authentication, but has no subscription quota
request. Missing, malformed or unknown account evidence remains an error, never
an inferred logout. No native credential files are inspected. See the official
[app-server account protocol](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md)
and [account schema](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/src/protocol/v2/account.rs).

Unknown state offers Connect to reuse native login after monitoring permission.
Only authentication-required evidence offers Login and its separate native
device-flow consent. Even a completed device flow still requires explicit
Finish and connect before monitoring permission is granted.

`CodexSession::start(&Path)` accepts a trusted backend-resolved absolute executable
path, not an IPC path. Windows requires a native `.exe`; shell/batch wrappers are
unsupported. It launches only `app-server`, with piped stdin/stdout, discarded
stderr and a fresh external temporary working directory (0700 on Unix). The
transport never launches a browser. The manager's explicit Open browser action
may use the validated progress URL.

`view()` returns only transient `AuthProgress`. `poll()` spends approximately
50 ms advancing the owned child, retaining incomplete frames and writes across
polls. `cancel()` attempts `account/login/cancel` with the native login ID, then
kills and reaps the owned process even on cancellation failure. It is **not**
logout and does not remove existing credentials. Drop requests child termination
without panicking; Tokio performs best-effort background reaping if a synchronous
`try_wait` cannot reap immediately. Explicit asynchronous cancellation is the
deterministic reaping path. Only the owned empty working-directory leaf is removed
on Drop; unexpected files are never recursively deleted.

## Fixed protocol

1. Reuse the status connector's `initialize` and `initialized` messages.
2. Request `account/login/start` with `{ "type": "chatgptDeviceCode" }`.
3. Display the returned device code and instructions to enter it **in the
   browser**, never in this app. Only HTTPS `auth.openai.com/codex/device` is
   accepted, without explicit ports, user information, queries or fragments.
4. Correlate `account/login/completed` by the bounded opaque `loginId`, including
   notifications received before the start response. Ignore unrelated IDs and
   duplicate completion events.
5. After matching success, request only `account/read` with
   `{ "refreshToken": false }`. Only `account.type == "chatgpt"` authenticates.
   Account email and every other response field are discarded, not UI output.

Unsupported RPCs fail closed. There is no inference, token exchange, prose-based
success detection, browser-login fallback, generic command API, or code-submit
API. Terminal progress clears URL, device code and output; failures use generic
messages rather than protocol JSON, native errors or account identifiers.

Startup is limited to 8 seconds; the completion status read to 8 seconds; native
cancellation exchange to 500 ms before mandatory termination/reaping. Frames are
limited to 256 KiB and total stdout to 1 MiB, with a bounded early-completion
queue. Malformed data, EOF and exhausted bounds fail the attempt. There is no
detached protocol-reading worker.

## Sources and compatibility

Verified against the official **rust-v0.165.0** schema; the installed CLI version
is unknown and no live compatibility claim is made:

- [Login request](https://github.com/openai/codex/blob/rust-v0.165.0/codex-rs/app-server-protocol/schema/json/v2/LoginAccountParams.json)
- [Device challenge](https://github.com/openai/codex/blob/rust-v0.165.0/codex-rs/app-server-protocol/schema/json/v2/LoginAccountResponse.json)
- [Completion](https://github.com/openai/codex/blob/rust-v0.165.0/codex-rs/app-server-protocol/schema/json/v2/AccountLoginCompletedNotification.json)
- [Cancel parameters](https://github.com/openai/codex/blob/rust-v0.165.0/codex-rs/app-server-protocol/schema/json/v2/CancelLoginAccountParams.json)
  and [response](https://github.com/openai/codex/blob/rust-v0.165.0/codex-rs/app-server-protocol/schema/json/v2/CancelLoginAccountResponse.json)
- [Account response](https://github.com/openai/codex/blob/rust-v0.165.0/codex-rs/app-server-protocol/schema/json/v2/GetAccountResponse.json)
- [Native device URL and provider-side completion](https://github.com/openai/codex/blob/rust-v0.165.0/codex-rs/login/src/device_code_auth.rs)
- [Native credential ownership](https://developers.openai.com/codex/auth/#credential-storage)
- [Tokio process cancellation/reaping](https://docs.rs/tokio/1.54.1/tokio/process/struct.Child.html)
  and [cancellation-safe buffered reads](https://docs.rs/tokio/1.54.1/tokio/io/trait.AsyncBufReadExt.html#method.fill_buf).

`src-tauri/tests/codex_auth.rs` uses only generated Python fake RPC executables,
fake home directories and external scratch state. No installed CLI, real-account
login, browser, credential lookup or generation endpoint is exercised. Linux
fixtures do not establish Windows runtime compatibility.

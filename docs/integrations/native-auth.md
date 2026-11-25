# Native login instruction bridge

This is an **auth-only terminal relay**, not proof of real-account compatibility.
Development tests execute synthetic programs in scratch homes only. They do not
run installed AI tools, open a browser, or access account state.

## Backend contract

`NativeSession::start(path, args)` is a trusted-backend API, not an IPC endpoint.
The caller must resolve the installed executable, validate its tool identity and
version, and require explicit **Sign in** consent before starting. Monitoring
permission is separate: neither granting monitoring nor scanning starts login.
Native login may open a browser and save credentials in the native tool's store.
The transport preserves the native environment/home; it does not import tokens.

Only these exact argument shapes are accepted:

| Integration | Arguments after the backend-owned executable |
| --- | --- |
| Antigravity | `acp --login` |
| Claude | `auth login` |
| OpenCode | `auth login <integrationID> --method <methodID>` |
| Hermes | `auth add <provider> --type oauth` |

Identifiers are 1–128 ASCII characters, start with an alphanumeric character,
and contain only alphanumerics, `.`, `_`, or `-`. This lexical check does **not**
prove that an integration/method is supported or OAuth: the backend must select
from its validated provider/method catalog and reject API-key methods. There is
no shell, interactive agent/chat fallback, arbitrary option passthrough, API
`--data` code submission, or generic terminal-keystroke endpoint. Relative paths,
non-executable files, and Windows `.cmd`/`.bat` wrappers are rejected; Windows
requires an `.exe`. Windows wrappers are not silently run through a shell.

The PTY is 24×100. Its working directory is a freshly created, private neutral
temporary directory, not the repository; Unix permissions are `0700`. The owned
empty directory is removed on Drop. This avoids repository-local discovery, not
all native global configuration or plugins. No login transcript is written by
the transport.

## Progress and manual entry

- `view()` returns transient `AuthProgress`; `poll()` snapshots collected output
  and calls nonblocking `try_wait()`.
- Running processes remain `waiting`, with `kind: native`. No prose, URL, code,
  or credential is parsed into structured success. `url`/`user_code` stay null.
- Zero exit means `finished`: “Native sign-in process finished. Check status
  separately.” It never means authenticated. Nonzero exit means `failed`.
- The UI renders `output` as text, never HTML, an active terminal, or executable
  links. It offers manual code entry **only when the user explicitly requests
  that step**, not because a phrase was detected in terminal output.
- Device codes must be entered on the provider site, not in this paste-back
  field. Structured Codex device login uses its separate transport.

`submit_code()` accepts **one submission per attempt**, of 1–1024 bytes, using
only `[A-Za-z0-9._~:/?&=%+#@-]`. Spaces, CR/LF, multiline input, Escape, other
controls, braces, brackets, and non-ASCII input are rejected without writing.
The transport sends the code and a newline only through the PTY writer, never
argv, environment variables, temporary files, or logs. Start a new attempt for
a rejected/expired native code; this is not a terminal command channel.

Before writing, old instructions are cleared and a zeroizing copy is registered
for echo redaction. The streaming redactor withholds possible code prefixes
across reads and replaces an exact echoed submission with `[redacted]`. Raw read
buffers and pending prefixes are zeroized; the pending allocation is reserved
before secret input to avoid reallocating secret prefixes. This is **not** a
general detector for unknown bearer tokens, API keys, transformed/wrapped echoes,
or secrets emitted by a misbehaving native binary. Native login output must be
appropriate for human instructions, not credential dumping.

CSI, OSC, DCS and related Escape-delimited terminal strings are stripped with
state retained across chunks; other controls except LF/tab are removed. The
display is capped at 64 KiB (including lossy UTF-8 conversion); total raw output
above 1 MiB fails the attempt and terminates the direct child. Terminal text and
registered input are cleared on completion, failure, cancellation and Drop.
Never copy this progress into usage snapshots, localStorage, telemetry or logs.

## Lifecycle and limits

`cancel()` is idempotent, closes terminal I/O, kills and reaps the direct login
child. Drop invokes the same cleanup. Foreground reaping is bounded to 500 ms;
if termination has not completed, cleanup returns a generic error and transfers
the child to a final background reaper rather than waiting forever in the UI.
Unix reader descriptors are nonblocking and observe cancellation. Reader joins
never block the caller; Windows reader-handle termination remains unverified.
Descendant/browser process-tree termination is not guaranteed.

The parent auth manager owns a maximum 15-minute expiry and cancellation on
permission revocation, attempt replacement, application exit, and user cancel.
This module does not implement those permission/timeout policies. Local runtimes
must show no login required. Provider/method discovery, reliable account-status
verification, and live Windows/native-version compatibility are outside this
relay's verified scope.

## Official sources

- [portable-pty 0.9.0](https://docs.rs/portable-pty/0.9.0/portable_pty/), MIT;
  [Child](https://docs.rs/portable-pty/0.9.0/portable_pty/trait.Child.html),
  [MasterPty](https://docs.rs/portable-pty/0.9.0/portable_pty/trait.MasterPty.html),
  [CommandBuilder](https://docs.rs/portable-pty/0.9.0/portable_pty/cmdbuilder/struct.CommandBuilder.html).
  Version/license and API verified through Context7 and docs.rs; direct dependency
  is pinned, with Cargo.lock recording registry checksums. No Tauri plugin added.
- [Antigravity ACP](https://antigravity.google/docs/cli/acp/): `agy acp --login`;
  [manual SSH code return](https://antigravity.google/docs/install?tab=cli#remote-ssh-oauth-flow).
  Do not substitute an interactive `agy` agent session or guess ACP authentication.
- [Claude CLI reference](https://code.claude.com/docs/en/cli-reference): `claude auth login`.
  That reference establishes the auth-only command, not live paste-back compatibility.
- [OpenCode v2.1.2 provider CLI](https://github.com/anomalyco/opencode/blob/v2.1.2/services/www/src/docs/content/cli/providers.mdx):
  integration/method selection; OAuth paste-code methods require an interactive terminal.
  Context7 also returned legacy `--provider`/method-label documentation; this bridge
  accepts only the parent-selected positional V2 form, not legacy syntax.
- [Hermes auth parser](https://github.com/NousResearch/hermes-agent/blob/main/hermes_cli/subcommands/auth.py):
  `auth add <provider> --type oauth`. The parser also documents `--no-browser`,
  but this relay deliberately does not add that optional variant.

## Focused verification

Run with an absolute external scratch directory substituted for `scratch`:

```text
TMPDIR=scratch CARGO_TARGET_DIR=/tmp/opencode/widget-auth-cargo cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --test native_auth
```

The Unix synthetic suite uses `/usr/bin/python3 -I` fixture executables and fresh
scratch HOME directories. It covers the fixed argv boundary, real PTY input,
split code echo/escape sequences, unchanged state after rejected input, size
limits, nonblocking polling, direct-child cancellation/reaping, Drop, and honest
zero/nonzero exit states. Fixtures are intentionally retained for parent cleanup.
No real-provider authentication or cross-platform runtime proof is implied.

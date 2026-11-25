# Antigravity

The tool is named **Gemini** in the UI; its internal ID, executable, login flow and quota source remain Antigravity. After approval and a successful connection, Tools shows each model group under its native name, including non-Gemini groups. Each row shows a reported window and its remaining percentage. Five-hour and weekly windows appear as “5h” and “Weekly”; other windows keep their reported names. If a group reports the same window twice, the bucket names distinguish the rows, with native quota IDs added when names also match. No missing window or percentage is inferred.

Select **?** beside the tool name for full quota labels, reset times, sources and connection help. These allowances are not token usage or a Gemini Web quota API.

After session permission, run the installed CLI's `--version`. Only recognized stable versions >=1.1.11 may receive `-p /usage --output-format json`. No prompt, browser-cookie or internal HTTP fallback is allowed. The CLI uses and may refresh its own saved sign-in.

Background usage checks receive a closed input pipe, not `/dev/null`, because the native CLI treats character-device input as interactive. On Unix they also run without a controlling terminal. On Linux they first query the desktop Secret Service `Locked` property (without reading any secrets) and defer invoking the CLI while locked so a boot-time keyring delay does not trigger the CLI's 1-hour keyring timeout fallback. Saved credentials and the keyring environment remain unchanged, allowing native token reuse and refresh; explicit native authentication-required errors surface `authentication: "required"` (enabling **Login** without revoking saved history) instead of starting interactive sign-in. Use the explicit Login action when sign-in is required.

The approved one-time development probe on CLI1.5.3 confirmed `command.data.groups[].buckets[]`, with `remaining_fraction` and `reset_time`. Convert fractions to remaining percentage; preserve model-group/window identity. Discard envelope counters, conversation identifiers and response text. Quota output does not establish historical token consumption.

Source: https://github.com/google-antigravity/antigravity-cli/blob/main/CHANGELOG.md (1.1.11 read-only print-mode commands); https://antigravity.google/docs/cli/commands/usage/.

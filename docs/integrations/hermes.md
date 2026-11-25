# Hermes

After session permission, invoke only `hermes usage --json`. Read `windows[].used_percent` and `resets_at`; convert used to remaining percentage. Unknown percentages remain unknown. Ignore raw details, account identity and errors. The CLI may refresh its provider-managed sign-in; the monitor does not collect credentials or start a chat session.

Source: https://github.com/NousResearch/hermes-agent/blob/main/hermes_cli/subcommands/usage.py. Only providers supported by the installed CLI can report. Quota output does not provide token history. Home-default Context metadata is discovered independently; custom profiles and roots are not scanned.

# Context discovery

Context is read only after the corresponding tool's session permission. OpenCode reports MCP runtime status and registered skills through its V2 APIs. Other cloud tools use bounded home-local native configuration and native/shared skill directories; these MCPs are **configured**, not confirmed connected.

Only names and normalized status leave the reader. Commands, environment values, URLs, credential fields, paths and skill content are discarded. Descriptor-relative no-follow access refuses linked entries and special files. Files are bounded to256KiB; skill traversal to1,024 entries and bounded depth. Missing, invalid or skipped sources are disclosed as incomplete, not zero active MCPs. Project, plugin, custom-root and profile-specific discovery is not implemented; discovered installed skills are not proof of activation.

The home `.agents/skills` inventory is discovered by the monitor for all cloud tools. Current official documentation establishes that as a native user-skill directory for Codex, but not as an automatically loaded global directory for every other tool. These shared entries must not be interpreted as effective tool registrations. The legacy `.codex/skills` scan is compatibility discovery whose current native support has not been independently established.

# Agent-assisted local setup

## Status: FUNCTIONAL (repository workflow)

Axiom includes an optional repository skill at [`.agents/skills/axiom-setup/SKILL.md`](../.agents/skills/axiom-setup/SKILL.md). It helps a compatible coding agent inspect this repository, run targeted checks, build the native desktop executable, and explain the local profile location to a person who prefers guided setup.

This is not an AI feature in Axiom itself. The desktop browser contains no AI integration, telemetry, analytics, tracking beacon, account requirement, or automatic upload of browsing information.

## How to use it

Open the repository in an agent that supports repository-local skills and ask it to use `axiom-setup` to prepare a local build. The skill requires approval before it downloads dependencies, changes a profile, installs software, writes outside the repository, or copies itself into an agent-global skill directory.

An agent must not silently “upload itself.” If a user asks to install the skill globally, the agent should show the source and destination, obtain permission, copy it, and verify the result.

## Local profile ownership

The desktop build stores a normal Windows profile in `%LOCALAPPDATA%\Axiom\Profile`, rather than its current working directory. This avoids writing into an installed application directory. Private windows remain memory-only. Test runs should use an explicitly chosen temporary profile rather than a real user profile.

## Limitations

The skill orchestrates local repository setup; it does not grant the browser web compatibility beyond the engine features that are actually implemented. Agents must report test and build failures accurately and must not publish releases without user authorization.

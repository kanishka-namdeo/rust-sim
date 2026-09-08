# DOX Framework

## Purpose

This file is the repository-wide `AGENTS.md` contract. It provides portable guidance for agents and maintainers; it is not a security boundary or deterministic permission system.

## Repository Scope

- Apply this file to the repository root and all descendants.
- Add a nested `AGENTS.md` only for a durable package, service, library, infrastructure domain, or generated-artifact boundary.
- Do not create speculative child files merely to populate an index.

## Hierarchy and Precedence

- Guidance accumulates from the repository root toward the target path.
- Child guidance may clarify or specialize inherited local rules.
- A child may not weaken an explicit repository-wide safety or governance rule.
- Sibling instructions do not apply outside their own subtrees.
- Explicit current user instructions take priority over repository guidance.
- Report unresolved conflicts instead of guessing.
- These are DOX semantics; individual tools may load a different or partial instruction chain.

## Required Child File Shape

Every governed `AGENTS.md` uses this order:

1. Purpose
2. Ownership
3. Local Contracts
4. Work Guidance
5. Verification
6. Child DOX Index

Nested files state their exact governed path, local ownership, local contracts, existing verification, and governed descendants. They reference inherited guidance instead of copying it.

## Read Before Editing

1. Identify every file or folder expected to change.
2. Walk from the repository root to each target path.
3. Read each applicable `AGENTS.md` from root to the nearest governing directory.
4. Use child guidance for local details and parent guidance for repository-wide rules.
5. If behavior is unclear or instructions conflict, stop and report the ambiguity.

## Update After Editing

For every meaningful change:

1. Check whether purpose, ownership, contracts, workflows, constraints, or durable structure changed.
2. Update the nearest owning `AGENTS.md` when those responsibilities changed.
3. Update affected parent Child DOX Index entries.
4. Remove stale or contradictory instructions.
5. Run relevant existing verification.
6. Report any governance file intentionally left unchanged and why.

## Compatibility and Security Limits

- Use ordinary Markdown and headings only; do not require frontmatter, custom parsers, or tool-specific commands.
- Verify instruction discovery and precedence for each supported agent and version.
- Never treat `AGENTS.md` as access control, secret protection, deployment approval, or compliance enforcement.
- Put deterministic restrictions in repository permissions, CI, deployment controls, secret management, or equivalent tooling.

## Child DOX Index

- None. This workspace currently has no governed source subprojects or nested `AGENTS.md` files.

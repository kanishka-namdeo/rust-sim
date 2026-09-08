# DOX Framework

## Purpose

This file is the repository-wide `AGENTS.md` contract. It provides portable guidance for agents and maintainers; it is not a security boundary or deterministic permission system.

Core contract: work products, source materials, instructions, records, assets, and durable docs must stay understandable from the nearest applicable `AGENTS.md` plus every parent `AGENTS.md` above it.

## Repository Scope

- Apply this file to the repository root and all descendants.
- Add a nested `AGENTS.md` only for a durable package, service, library, infrastructure domain, or generated-artifact boundary.
- Do not create speculative child files merely to populate an index.

## Hierarchy and Precedence

- Guidance accumulates from the repository root toward the target path.
- The root is the DOX rail: project-wide instructions, global preferences, durable workflow rules, and the top-level Child DOX Index.
- Child files own domain-specific instructions and their own Child DOX Index.
- Each parent explains what its direct children cover and what stays owned by the parent.
- The closer a doc is to the work, the more specific and practical it must be. Put broad rules in parent docs and concrete details in child docs.
- Conflict order:
  1. Explicit current user instructions take priority over repository guidance.
  2. Applicable child guidance takes priority over broader guidance for local details.
  3. Repository-wide safety and governance rules remain binding; a child may clarify or specialize local rules but may not weaken them.
  4. Sibling instructions do not apply outside their own subtrees.
  5. Report unresolved conflicts instead of guessing; remove recurring ambiguity by updating the owning `AGENTS.md`.
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

- Work Guidance must reflect the current standards of the project or user instructions; if there are no specific standards or instructions yet, leave it empty.
- Verification must reflect an existing check; if no verification framework exists yet, leave it empty and update it when one exists.

## Style

- Keep docs concise, current, and operational.
- Document stable contracts, not diary entries.
- Prefer direct bullets with explicit names.
- Do not duplicate rules across many files unless each scope needs a local version.
- Delete stale notes instead of explaining history.
- Trim obvious statements, repeated rules, misplaced detail, and warnings for risks that no longer exist.

## Read Before Editing

1. Read the root `AGENTS.md`.
2. Identify every file or folder expected to change.
3. Walk from the repository root to each target path.
4. Read every `AGENTS.md` found along each route. If a parent Child DOX Index lists a child whose scope contains the path, read that child and continue from there.
5. Use the nearest `AGENTS.md` as the local contract and parent docs for repository-wide rules.
6. If behavior is unclear or instructions conflict, stop and report the ambiguity.

Do not rely on memory. Re-read the applicable DOX chain in the current session before editing.

## Update After Editing

Every meaningful change requires a DOX pass before the task is done.

1. Check whether purpose, scope, ownership, responsibilities, durable structure, contracts, workflows, operating rules, inputs, outputs, permissions, constraints, side effects, artifacts, or user preferences about behavior, communication, process, organization, or quality changed.
2. Update the nearest owning `AGENTS.md` when those responsibilities changed.
3. Update parent docs when parent-level structure, ownership, workflow, or child index changes. Update child docs when parent changes alter local rules.
4. Refresh every affected Child DOX Index.
5. Remove stale or contradictory instructions.
6. Run relevant existing verification.
7. Report any governance file intentionally left unchanged and why.

Small edits that do not change behavior or contracts may leave docs unchanged, but the DOX pass still must happen.

## User Preferences

When the user requests a durable behavior change, record it here or in the relevant child `AGENTS.md`.

## Compatibility and Security Limits

- Use ordinary Markdown and headings only; do not require frontmatter, custom parsers, or tool-specific commands.
- Verify instruction discovery and precedence for each supported agent and version.
- Never treat `AGENTS.md` as access control, secret protection, deployment approval, or compliance enforcement.
- Put deterministic restrictions in repository permissions, CI, deployment controls, secret management, or equivalent tooling.

## Child DOX Index

- None. Scanned 2026-09-08: only the root `AGENTS.md` exists. `docs/` and `.superpowers/` are working notes, not durable package, service, library, infrastructure-domain, or generated-artifact boundaries, so no nested file is warranted.

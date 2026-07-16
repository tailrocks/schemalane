# Repository agent instruction files

Research date: 2026-07-16

## Decision summary

Use a root `AGENTS.md` as the hand-maintained, tool-neutral source of durable
repository guidance. Make `CLAUDE.md` a relative symlink to `AGENTS.md`, so both
tools read one physical file. Add nested pairs only when a subtree truly has
different commands or constraints. Do not use instruction prose as a security
control. This repository accepts the reduced Windows portability of symlinks.

This shape follows the vendor-neutral AGENTS.md convention and a Claude Code
compatibility mechanism documented by Anthropic. The AGENTS.md project describes
the file as plain Markdown for project context, build and test commands,
conventions, and security considerations; it supports nested files whose closest
guidance wins. The format is now stewarded by the Agentic AI Foundation under
the Linux Foundation, not by Google or another single vendor.
[AGENTS.md format][agents-format]
[OpenAI AAIF announcement][aaif]

## What the official implementations actually load

### Claude Code

- Claude Code loads managed, user, project, and local `CLAUDE.md` instructions.
  Ancestor files from the launch directory upward are loaded in full at startup;
  descendant files are loaded on demand when Claude reads files below them.
- Files are concatenated rather than mechanically merged. More-local content is
  later in context, but Anthropic warns that contradictory rules can be followed
  arbitrarily. Conflict removal is therefore safer than relying on precedence.
- A root project file may be `CLAUDE.md` or `.claude/CLAUDE.md`.
  `CLAUDE.local.md` is for ignored personal project preferences.
- `@path/to/file` imports are expanded at startup. Paths are relative to the
  importing file, recursive imports are limited to five hops, and external
  imports require approval on first encounter.
- `.claude/rules/*.md` supports modular rules and optional `paths` frontmatter.
  Path-scoped rules load only for matching work. Multi-step, task-specific
  procedures belong in skills instead of always-loaded instructions.
- Anthropic targets fewer than 200 lines per `CLAUDE.md`: concise, specific,
  structured instructions receive better adherence. Splitting content into
  imports improves organization but not startup context use because imports are
  still loaded in full.

Anthropic explicitly says Claude Code reads `CLAUDE.md`, not `AGENTS.md`, and
recommends a `CLAUDE.md` containing `@AGENTS.md` plus any Claude-only section.
A symlink also works and guarantees one physical source; imports are more
portable to Windows installations without symlink support. This repository uses
the symlink option. [Claude Code memory and instruction docs][claude-memory]

### OpenAI Codex

- Codex reads one global file, preferring `AGENTS.override.md` over `AGENTS.md`,
  then walks from the project root to the current working directory and reads at
  most one instruction file per directory in the same preference order.
- Root-to-current-directory files are concatenated; closer files appear later
  and override broader guidance. Discovery happens once per run/session and
  stops at the current directory.
- Empty files are skipped. Combined project guidance is capped by
  `project_doc_max_bytes`, 32 KiB by default. Alternate filenames work only when
  configured through `project_doc_fallback_filenames`.
- OpenAI recommends keeping `AGENTS.md` small and using it for build/test
  commands, review expectations, repository conventions, and directory-specific
  instructions. Repeatable procedures with scripts and references belong in
  skills; hard guarantees belong in enforcement infrastructure.

[Codex custom instructions][codex-agents]
[Codex customization guidance][codex-customization]

### Important cross-tool difference

Nested scope is not discovered identically. Codex builds only the chain from
the project root to the launch CWD. Claude loads that ancestor chain and may
later load descendant instructions as files are read. Therefore:

1. Put universal rules at the repository root.
2. Put subtree-only rules in that subtree, but write them so they remain valid
   when loaded later alongside root rules.
3. Launch Codex from the relevant subtree when its nested rules must apply to
   the whole task.
4. Never depend on a nested file to weaken a root security invariant.

## Concrete recommendations for this repository

### Root versus nested scope

- Create root `AGENTS.md` for the specification pointer, crate map, prerequisites,
  universal Rust/SQL/output conventions, exact verification commands, and
  commit/review expectations.
- Replace duplicated root `CLAUDE.md` content with a relative symlink to
  `AGENTS.md`. Put genuine Claude-only behavior in `.claude/rules` or skills.
- Keep the linked root guidance comfortably below both vendor limits. The
  objective is deduplication, not expansion.
- Add nested `AGENTS.md` only if a crate develops materially different commands
  or invariants. Mirror it with a nested `CLAUDE.md` symlink only if Claude must
  discover the same subtree rules; test discovery in both tools.
- Prefer `.claude/rules` with `paths` for Claude-only file-pattern rules. Do not
  copy those rules into every package.

### Content and commands

Keep only facts useful in nearly every relevant session:

- authoritative contract locations and ownership boundaries;
- prerequisites that explain otherwise surprising failures;
- exact, copy-pasteable format, lint, unit, integration, and package commands;
- which checks require Docker/network/credentials and which do not;
- invariants agents cannot reliably infer, such as stdout/stderr discipline,
  SQL identifier quoting, terminal sanitization, and migration compatibility;
- required evidence before declaring work complete.

Do not embed long architecture prose, release runbooks, temporary project plans,
or examples already maintained elsewhere. Link to stable repository documents,
or use a skill for a conditional procedure. The AGENTS.md format itself frames
the file as an agent-focused complement to human README material, not a README
replacement. [AGENTS.md format][agents-format]

Commands should state scope and expected environment. Prefer the same command
CI executes. Separate fast local gates from ignored Docker integration tests so
an agent does not misreport a partial run as full verification. Revalidate
commands whenever CI or workspace membership changes.

### Duplication and maintenance

- One canonical statement per rule. For shared behavior, canonicalize in
  `AGENTS.md` and expose it through the `CLAUDE.md` symlink.
- Hand-maintain committed instructions through normal review. Generator commands
  such as Claude Code `/init` are useful for bootstrapping or suggesting
  improvements, but Anthropic says to refine generated output with facts the tool
  cannot discover. Generated output must not overwrite reviewed policy.
- Update instructions when the same correction or review feedback recurs, not
  for one-off task state. Periodically delete stale rules and resolve conflicts.
- Add a lightweight CI check for broken symlink targets and referenced
  command drift if these files become operationally critical.

[Claude Code memory and instruction docs][claude-memory]
[OpenAI usage guidance][openai-usage]

### Security and irreversible actions

Instruction files are advisory context, not enforcement. Anthropic explicitly
distinguishes `CLAUDE.md` from permissions, sandboxing, and hooks: use settings
or lifecycle hooks when an action must be blocked regardless of model behavior.
The same structural recommendation applies cross-tool: enforce format/lint/test
requirements in CI and enforce destructive-command or secret-access boundaries
in the agent/tool permission layer. [Claude Code configuration debugging][claude-debug]

- Never store secrets, tokens, private URLs, personal data, or machine-specific
  credentials in committed instruction files.
- Keep personal preferences and local endpoints in ignored local/user scope.
- Treat linked or nested instructions as executable influence: review changes
  to them like build scripts, keep links inside the repository when possible,
  and require ownership review for security policy changes.
- Instructions may say to ask before destructive actions, but permissions,
  sandboxing, hooks, branch protection, and CI must provide the guarantee.
- Avoid commands that interpolate untrusted text, download-and-execute remote
  code, print environment variables, or broadly mutate files.

The statement about reviewing instruction changes like build scripts is an
inference from their automatic loading and behavioral influence, not a quoted
vendor requirement.

### Conflict policy

1. Explicit task instructions decide one-off intent, within higher-level safety
   and organizational constraints.
2. Repository root defines universal project truth.
3. Nested instructions may specialize, not silently contradict, root rules.
4. Tool-specific files may add tool behavior, not fork project facts.
5. If two files disagree, fix the files immediately; do not trust load order to
   make ambiguity safe.

The AGENTS.md format says the closest file wins and explicit user prompts
override repository guidance. Claude's documentation is more cautious: all
files are context, and contradictions can be resolved unpredictably. The policy
above adopts the stricter behavior that is reliable across both tools.
[AGENTS.md conflict FAQ][agents-format]
[Claude Code memory and instruction docs][claude-memory]

## Verification checklist for a future migration

- `AGENTS.md` is the only canonical copy of shared repository facts.
- Every `CLAUDE.md` is a relative symlink to its sibling `AGENTS.md`.
- Both files are concise, contain no secrets, and point to existing paths.
- Commands match CI and clearly distinguish Docker-dependent tests.
- No root/nested or AGENTS/CLAUDE contradictions remain.
- Codex launched at root and relevant subtrees reports expected instruction
  sources; Claude `/memory` reports expected files and rules.
- Security-critical requirements are enforced outside instruction prose.

## Primary sources

- [Anthropic: How Claude remembers your project][claude-memory]
- [Anthropic: Debug your configuration][claude-debug]
- [OpenAI Codex: Custom instructions with AGENTS.md][codex-agents]
- [OpenAI Codex: Customization][codex-customization]
- [OpenAI: How OpenAI uses Codex][openai-usage]
- [AGENTS.md open format and FAQ][agents-format]
- [OpenAI: AGENTS.md donation to the Agentic AI Foundation][aaif]

[claude-memory]: https://code.claude.com/docs/en/memory
[claude-debug]: https://code.claude.com/docs/en/debug-your-config
[codex-agents]: https://learn.chatgpt.com/docs/agent-configuration/agents-md.md
[codex-customization]: https://learn.chatgpt.com/docs/customization/overview.md
[openai-usage]: https://openai.com/business/guides-and-resources/how-openai-uses-codex/
[agents-format]: https://agents.md/
[aaif]: https://openai.com/index/agentic-ai-foundation/

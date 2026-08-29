# Prompt Optimizer Skill Design

## Purpose

Create a globally discoverable Codex skill that transforms raw ideas, requirements, and draft prompts into concise, structured, executable, and verifiable prompts.

## Distribution

- Install the skill at `~/.codex/skills/prompt-optimizer`.
- Keep the skill self-contained: `SKILL.md` plus generated `agents/openai.yaml` metadata.
- Do not add scripts, references, assets, or project-specific dependencies.
- Write all skill instructions and interface metadata in English.

## Invocation

- Allow implicit invocation when the user's intent is prompt optimization, rewriting, clarification, organization, or prompt creation, regardless of language.
- Support explicit invocation with `$prompt-optimizer`.
- Do not define or document custom slash-command aliases.
- Do not invoke the skill when the user wants the underlying task performed rather than a prompt created.

## Workflow

1. Extract the goal, necessary context, supplied inputs, requirements, constraints, expected output, and observable completion criteria.
2. Determine whether a material ambiguity prevents reliable optimization.
3. If clarification is required, ask only the single most important question and do not produce a partial prompt.
4. Otherwise, reconstruct the prompt from first principles instead of polishing the original wording.
5. Remove repetition, irrelevant background, empty modifiers, conflicts, unverifiable requirements, and process instructions that do not affect the result.
6. Produce one copy-ready prompt using only the Markdown sections justified by the task's complexity.

## Output Contract

The optimized prompt must preserve the user's language, goal, scope, supplied filenames, paths, commands, proper nouns, and necessary references unless the user requests a change. It must state the actions, constraints, deliverable, and completion conditions clearly without exposing analysis or hidden reasoning.

The skill must not invent missing facts, expand scope, add unnecessary personas, request chain-of-thought, convert optional guidance into mandatory constraints, or execute the task described by the prompt.

## Ambiguity Handling

Clarification is warranted only when multiple reasonable interpretations exist, they would materially change the result, and context or a low-risk assumption cannot resolve them. Conflicting instructions that cannot be reconciled without changing user intent follow the same rule.

## Validation

- Run the bundled `quick_validate.py` validator against the installed skill.
- Confirm the skill contains no scaffold placeholders or unsupported invocation claims.
- Review representative routing cases:
  - A request to optimize or structure a prompt should invoke the skill.
  - `$prompt-optimizer` should explicitly invoke the skill.
  - A request to perform an underlying task should not invoke the skill solely because it contains instructions.
  - A materially ambiguous prompt should yield one targeted question.
  - A clear prompt should yield one concise, copy-ready prompt without an analysis preamble.

## Non-Goals

- Registering custom Codex slash commands
- Executing optimized prompts
- Maintaining domain-specific prompt templates
- Adding deterministic scripts or external integrations

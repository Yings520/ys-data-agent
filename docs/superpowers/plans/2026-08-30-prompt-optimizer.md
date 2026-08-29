# Prompt Optimizer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Install a globally discoverable English Codex skill that turns rough ideas and draft prompts into concise, structured, executable prompts.

**Architecture:** Use one declarative `SKILL.md` for discovery and behavior, plus `agents/openai.yaml` for English UI metadata and explicit implicit-invocation policy. The skill has no scripts, assets, references, external integrations, or project dependencies.

**Tech Stack:** Codex Agent Skills (`SKILL.md`), YAML metadata, bundled skill initializer and validator

---

### Task 1: Initialize the global skill

**Files:**
- Create: `/Users/ysc/.codex/skills/prompt-optimizer/SKILL.md`
- Create: `/Users/ysc/.codex/skills/prompt-optimizer/agents/openai.yaml`

- [ ] **Step 1: Verify that the target does not already exist**

Run:

```bash
rtk ls -la /Users/ysc/.codex/skills/prompt-optimizer
```

Expected: the command reports that the directory does not exist. If it exists, inspect it and stop before overwriting unrelated content.

- [ ] **Step 2: Initialize the skill without optional resource directories**

Run:

```bash
rtk proxy python3 /Users/ysc/.codex/skills/.system/skill-creator/scripts/init_skill.py \
  prompt-optimizer \
  --path /Users/ysc/.codex/skills \
  --interface 'display_name=Prompt Optimizer' \
  --interface 'short_description=Turn rough ideas into executable prompts' \
  --interface 'default_prompt=Use $prompt-optimizer to turn my draft into a concise, structured, executable prompt.'
```

Expected: the initializer creates `prompt-optimizer/SKILL.md` and `prompt-optimizer/agents/openai.yaml` without `scripts`, `references`, or `assets` directories.

- [ ] **Step 3: Inspect the scaffold**

Run:

```bash
rtk find /Users/ysc/.codex/skills/prompt-optimizer -maxdepth 3 -type f
```

Expected files:

```text
/Users/ysc/.codex/skills/prompt-optimizer/SKILL.md
/Users/ysc/.codex/skills/prompt-optimizer/agents/openai.yaml
```

### Task 2: Install the approved instructions and metadata

**Files:**
- Modify: `/Users/ysc/.codex/skills/prompt-optimizer/SKILL.md`
- Modify: `/Users/ysc/.codex/skills/prompt-optimizer/agents/openai.yaml`

- [ ] **Step 1: Replace `SKILL.md` with the approved English content**

Use `apply_patch` to make the file exactly:

```markdown
---
name: prompt-optimizer
description: Transform raw ideas, requirements, or draft prompts into concise, structured, directly executable prompts. Use when the user's core intent is to optimize, rewrite, clarify, organize, or create a better prompt, regardless of language. Do not use when the user wants the underlying task performed instead of a prompt created.
---

# Prompt Optimizer

Transform a user's raw idea, requirement, or draft prompt into a minimal but sufficient prompt that is clear, structured, executable, and verifiable.

## Invocation

Support:

- Automatic invocation when the user intends to optimize, rewrite, clarify, organize, or structure a prompt
- Explicit invocation with `$prompt-optimizer`

## Workflow

### 1. Extract the Task Elements

Identify the following from the user's input:

- **Goal**: The result the prompt should achieve
- **Context**: Background required to perform the task
- **Input**: Existing text, files, code, data, or reference material
- **Requirements**: Work that must be completed
- **Constraints**: Rules, boundaries, prohibitions, and conditions that must be preserved
- **Output**: The expected deliverable and format
- **Completion Criteria**: Observable and verifiable conditions for completion

Keep only information that materially helps execute the task. Do not add content merely to populate every field.

### 2. Decide Whether Clarification Is Necessary

If the available information is sufficient to produce a reliable prompt, optimize it immediately without asking questions.

Ask a clarifying question only when all of the following are true:

1. More than one reasonable interpretation exists.
2. The interpretations would materially change the scope, constraints, output, or completion criteria.
3. The ambiguity cannot be resolved safely from context or through a low-risk assumption.

When clarification is necessary:

- Ask the single most important question first
- Make the question specific and directly relevant to the final prompt
- Do not ask for information already provided
- Do not ask questions merely for completeness
- Do not request background that will not affect execution
- Output only the question; do not generate a partial prompt

### 3. Reconstruct from First Principles

Rebuild the prompt from the intended result instead of merely polishing the original wording.

Determine:

1. What problem actually needs to be solved?
2. What is the minimum sufficient information required to solve it?
3. Which constraints and boundaries must be explicit?
4. Which tasks must be performed?
5. What deliverable and format are expected?
6. How can completion be observed or verified?

Remove or resolve:

- Repetition
- Empty modifiers
- Irrelevant background
- Conflicting instructions
- Vague or unverifiable requirements
- Process instructions that do not affect the result

If conflicting instructions cannot be resolved without changing the user's intent, ask a clarifying question.

Do not:

- Expand the scope without authorization
- Invent facts, files, inputs, or constraints
- Add unnecessary role-playing instructions
- Request hidden reasoning or chain-of-thought
- Turn optional recommendations into mandatory requirements
- Perform the underlying task described by the prompt

### 4. Generate the Final Prompt

Choose the smallest structure appropriate for the task.

For complex tasks, use relevant sections from:

# Task

## Context

## Input

## Requirements

## Constraints

## Execution

## Output

## Completion Criteria

For simple tasks, compress the structure and retain only the sections that improve execution.

Include `Execution` only when sequencing, tool usage, or validation steps materially affect the result. Do not create empty sections to satisfy the template.

## Output Rules

When the request is clear:

- Output one final prompt by default
- Do not include analysis, extraction tables, or a change log
- Use clear Markdown that can be copied directly
- Preserve the user's language unless another language is requested
- Preserve supplied filenames, paths, commands, proper nouns, and necessary references
- Use explicit actions, deliverables, constraints, and completion conditions
- Remain as concise as possible without losing required information

If the user asks for an explanation, add a brief explanation after the final prompt.

## Completion Criteria

The final prompt must:

- Preserve the user's original goal and scope
- Include the input and context required for execution
- State the tasks, constraints, and expected output clearly
- Contain no unnecessary repetition or conflicting instructions
- Use a structure proportional to the task's complexity
- Be directly executable by an AI
- Define observable or verifiable completion conditions
- Be ready to copy and use
```

- [ ] **Step 2: Make `agents/openai.yaml` explicit and English-only**

Use `apply_patch` to make the file exactly:

```yaml
interface:
  display_name: "Prompt Optimizer"
  short_description: "Turn rough ideas into executable prompts"
  default_prompt: "Use $prompt-optimizer to turn my draft into a concise, structured, executable prompt."

policy:
  allow_implicit_invocation: true
```

- [ ] **Step 3: Confirm that no unsupported aliases or scaffold placeholders remain**

Run:

```bash
rtk rg -n 'TBD|TODO|PLACEHOLDER|/opt-prompt|/optimize-prompt|/prompt-opt' /Users/ysc/.codex/skills/prompt-optimizer
```

Expected: no matches.

### Task 3: Validate the installed skill

**Files:**
- Verify: `/Users/ysc/.codex/skills/prompt-optimizer/SKILL.md`
- Verify: `/Users/ysc/.codex/skills/prompt-optimizer/agents/openai.yaml`

- [ ] **Step 1: Run the bundled structural validator**

Run:

```bash
rtk proxy python3 /Users/ysc/.codex/skills/.system/skill-creator/scripts/quick_validate.py /Users/ysc/.codex/skills/prompt-optimizer
```

Expected: `Skill is valid!`

- [ ] **Step 2: Verify the installed file set**

Run:

```bash
rtk find /Users/ysc/.codex/skills/prompt-optimizer -maxdepth 3 -type f
```

Expected: only `SKILL.md` and `agents/openai.yaml`.

- [ ] **Step 3: Review routing invariants**

Confirm against the installed description and instructions:

```text
"Optimize this prompt"                       -> invoke
"帮我把这个需求整理成 Prompt"                 -> invoke
"$prompt-optimizer Rewrite this draft"       -> explicit invocation
"Implement this feature"                     -> do not invoke solely because the request contains instructions
Materially ambiguous prompt                   -> ask one targeted question
Clear draft prompt                            -> return one copy-ready prompt without analysis
```

Expected: every case is directly supported without conflicting instructions.

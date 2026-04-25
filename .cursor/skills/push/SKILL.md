---
name: push
description: Prepare and execute a safe git commit-and-push flow by generating commit title and description from local changes, showing added files, requesting explicit confirmation, then running git commit and git push. Use when the user asks /push, asks to commit and push, or asks to generate commit messages from local diffs before pushing.
---

# Push Command

## Purpose

Run a reliable commit-and-push workflow that:

1. Inspects local changes
2. Generates a commit title and description from the diff
3. Shows newly added files
4. Asks for explicit confirmation
5. Commits and pushes

## Workflow

Follow these steps in order.

### 1) Inspect repository state

Run in parallel:

- `git status --short`
- `git diff`
- `git diff --cached`
- `git log --oneline -10`

If there are no staged or unstaged changes, stop and report that there is nothing to commit.

### 2) Stage intended changes

Stage relevant files:

- Usually `git add -A`
- If the user scope is narrower, stage only requested files

Then re-check:

- `git status --short`

### 3) Generate commit message

Generate:

- **Title**: one concise line (imperative style, <=72 chars)
- **Description**: 1-3 short bullet points focused on why and impact

Base the message on all staged changes and the repository's recent commit style.

### 4) Show added files and ask confirmation

List added files before committing:

- `git diff --cached --name-status`

Highlight files with status `A`.

Request explicit confirmation with a clear summary:

- commit title
- commit description
- list of added files
- push target (current branch and upstream if configured)

Do not commit or push without user confirmation.

### 5) Commit safely

Commit with a heredoc body:

```bash
git commit -m "$(cat <<'EOF'
<title>

<description line 1>
<description line 2>
EOF
)"
```

If commit hooks modify files, stage the changes and create a new commit only when appropriate.

### 6) Push

Push after successful commit:

- If upstream exists: `git push`
- If no upstream: `git push -u origin HEAD`

### 7) Report result

Return:

- commit hash and title
- branch pushed
- confirmation that remote update succeeded

## Output Template

Use this response structure before confirmation:

```markdown
Proposed commit
- Title: <title>
- Description:
  - <bullet 1>
  - <bullet 2>

Added files
- <file1>
- <file2>

Ready to run:
1. git commit
2. git push

Proceed?
```

After execution, report:

```markdown
Push completed
- Commit: <hash> <title>
- Branch: <branch>
- Remote: <remote>
```

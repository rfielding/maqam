# Local Claude Setup (No Tarballs)

This project is configured for a local patch workflow so you can iterate before pushing to GitHub.

## 1) Install Claude CLI

Already installed globally in this environment.

## 2) Save API key locally

Run:

```bash
make claude-setup
```

This stores your key in `~/.config/claude/env` with `600` permissions and auto-loads it in `~/.bashrc`.

## 3) Create a prompt file

Example:

```bash
mkdir -p prompts
cat > prompts/fix.md <<'EOP'
Return ONLY a valid unified git patch for this repository.
Task: <describe change>
Constraints:
- No tarballs
- No markdown fences
- Keep patch minimal
EOP
```

## 4) Run local patch loop

```bash
make claude-patch BRANCH=claude/my-change PROMPT=prompts/fix.md
```

It will:
- switch/create your local branch
- request patch output from Claude
- validate with `git apply --check`
- apply patch to your working tree

## 5) Test and commit locally

```bash
cargo test
git add -A
git commit -m "Apply Claude patch for <task>"
```

Push only when ready.

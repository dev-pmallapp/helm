# Contributing

How to contribute to HELM.

## Workflow

1. **Fork** the repository on GitHub.
2. **Branch** from `main`: `git checkout -b feat/my-feature`.
3. **Implement** following TDD (red → green → refactor).
4. **Run** `make pre-commit` (format + lint + test).
5. **Commit** with a conventional-commit subject.
6. **Push** and open a pull request.

## Commit Messages

Use conventional-commit format:

- `feat:` — new feature
- `fix:` — bug fix
- `test:` — adding or fixing tests
- `refactor:` — code restructuring
- `docs:` — documentation changes
- `chore:` — build, CI, tooling

Keep the subject line under 72 characters. Do not include AI-tool
attribution.

## Code Review

- Every PR needs at least one approval.
- All CI checks must pass (fmt-check, clippy, test).
- All public items must have doc comments.
- New features need corresponding tests.

## What to Avoid

- Do not commit AI-agent state directories (`.codex/`, `.claude/`,
  `.cursor/`, etc.).
- Do not commit `target/`, `__pycache__/`, `*.pyc`, `*.so`.
- Do not commit with broken tests or lint failures.

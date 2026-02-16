<instruction>
Write a git commit subject from the tracked-change context below.

Constraints:
- Output exactly one line.
- Use Conventional Commits format: `<type>(optional-scope): <subject>`.
- Keep it at or under 72 characters.
- Do not add quotes, bullets, Markdown, or explanation.
- If there are no tracked changes, output exactly: `chore: no tracked changes`.
</instruction>

## Tracked Change Context
{{file:.vizier/tmp/commit-context.txt}}

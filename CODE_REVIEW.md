# Devin Review - Feature Spec (from rendered page)

## Goal
Provide an AI-assisted code review experience for GitHub pull requests that:
- Organizes diffs into coherent change sections.
- Detects moved/copied code.
- Flags potential bugs.
- Lets users ask questions about the PR.

## Primary user flows
1. **Submit a PR URL**
   - User enters a GitHub PR URL.
   - The system loads a review page for that PR.
   - Example PRs are offered as quick-start links.

2. **Review organized changes**
   - Changes are grouped into numbered sections (e.g., 1, 2, 3).
   - Each section lists affected files and line changes.
   - Inline diffs show context and code modifications.

3. **See potential issues**
   - A “Potential bug” item is surfaced with file/line location.
   - A “Flags” area summarizes additional concerns.

4. **Interact with the review**
   - Per-file controls: collapse file, copy file path, mark as viewed.
   - Per-comment controls: add a comment, expand comment, close.

5. **Ask Devin**
   - An assistant panel invites questions about the PR.
   - Suggested prompts include:
     - Summarize changes
     - Find potential issues
     - Explain architecture

6. **Detect copy/move operations**
   - A “Copy/move detection” section highlights moved/copied code.
   - The original location of moved/copied code is shown alongside the diff.

## Key features
- **PR URL entry**: Accepts a GitHub PR URL as the entry point.
- **Sample PR links**: Pre-filled example PRs for exploration.
- **Structured diffs**: Change sections with file and line change counts.
- **Inline diff viewer**: Shows added/removed lines with context.
- **Issue/flag detection**: Surfaces at least one potential bug plus flags.
- **File controls**: Collapse, copy path, mark as viewed.
- **Comment controls**: Add/expand/close comments.
- **AI assistant**: Ask questions, with suggested prompts.
- **Copy/move detection**: Identifies moved/copied code.

## UI elements implied by the page
- **Inputs**
  - PR URL text input (placeholder: `github.com/owner/repo/pull/123`).
  - “Ask Devin” textarea (placeholder: `Ask Devin anything about this PR (⌘I)`).
  - “Mark as viewed” checkbox(es).
- **Buttons**
  - Submit PR URL
  - Collapse file
  - Copy file path
  - Add a comment
  - Expand comment
  - Ask Devin
  - Close
- **Links**
  - Home, Docs, Log in
  - Example PRs (three sample links)

## Non-goals / out of scope (not shown)
- Authentication requirements or access control details.
- Pricing or plan selection.
- Repo connection setup beyond PR URL input.
- CI integration or automated status checks.

## Open questions
- Does the review update live as new PR commits arrive?
- What heuristics or models are used for bug detection and flags?
- Are there permission checks for private repos (OAuth, GitHub App, etc.)?
- Can users export or share reviews?
- What is the expected SLA/time-to-review after submission?

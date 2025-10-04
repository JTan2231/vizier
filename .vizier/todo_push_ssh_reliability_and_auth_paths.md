Context
- User reports: “push doesn’t work with SSH right now.”
- Current implementation: vizier-core/src/vcs.rs::push_current_branch uses libgit2 with a credentials callback that (a) consults git-credential helpers, (b) tries ssh-agent (Cred::ssh_key_from_agent), (c) falls back to USERNAME/USER_PASS_PLAINTEXT, then Cred::default(). It does not attempt file-based SSH keys and error messaging is generic.
- UX: vizier-cli surfaces a single-line error and exits; no actionable guidance.

Tension
- Promise: `vizier save --push` should “just work” across common Git remote configurations (HTTPS and SSH) without interactive prompts.
- Reality: SSH remotes often rely on file-based keys (e.g., ~/.ssh/id_ed25519) and/or agents; lack of file-key fallback causes auth failures. Errors aren’t actionable.

Product changes (acceptance criteria)
- SSH auth coverage:
  - Push succeeds when:
    - SSH agent has a valid key loaded for the remote host, OR
    - A default file-based SSH key exists at ~/.ssh/id_ed25519 or ~/.ssh/id_rsa with matching public key, without requiring interactive passphrase input.
  - If only a passphrase-protected key is present and no agent is available, the push fails fast with a clear, human-readable error suggesting remedies (start ssh-agent and add key, or configure HTTPS remote).
- HTTPS auth unchanged; continue to honor `git credential` helpers.
- Non-interactive contract: never prompt for input; exit with descriptive error messages.
- Outcome/CLI messaging:
  - On failure, show: remote name, URL scheme (ssh/https), and the credential strategies attempted (agent, file-id_ed25519, file-id_rsa, helper) with the first failing cause redacted of secrets.
  - On success, continue the existing “Push to origin completed.” path.

Pointer anchors
- vizier-core/src/vcs.rs (push_current_branch): expand credentials callback to attempt, in order: credential helper (for https); ssh-agent; file-based keys (id_ed25519 then id_rsa) if allowed; fallback to USERNAME only where appropriate. Ensure no interactive prompts are triggered.
- vizier-cli/src/actions.rs (push_origin_if_requested): enrich error surface with structured context from core so CLI can print actionable guidance.

Notes
- Keep implementation open: support both agent and default key paths; avoid prescribing specific env var names beyond standard SSH_AUTH_SOCK behavior; do not introduce interactive passphrase prompts.
- Security: do not weaken host key verification policies beyond libgit2 defaults; if host key verification fails, surface that explicitly in the error.
- Tests: add a unit-level smoke test to verify that the credentials callback attempts file-based keys if agent is unavailable (can be a dependency-injected strategy list). End-to-end SSH push will remain out-of-scope for unit tests; document manual verification steps in the PR.

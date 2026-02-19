id = "template.tools.commit"
version = "v1"

policy = {
  dependencies = {
    missing_producer = "wait"
  }
}

artifact_contracts = [
  { id = "prompt_text", version = "v1" },
  { id = "commit_message", version = "v1" }
]

nodes = [
  {
    id = "collect_context"
    kind = "shell"
    uses = "cap.env.shell.command.run"
    args = {
      script = <<-SCRIPT
mkdir -p .vizier/tmp

status="$(git status --short --untracked-files=no)"
if [ -z "$status" ]; then
  status="<none>"
fi

staged_diff="$(git diff --cached --no-ext-diff --unified=3 -- . | head -c 30000)"
if [ -z "$staged_diff" ]; then
  staged_diff="<none>"
fi

unstaged_diff="$(git diff --no-ext-diff --unified=3 -- . | head -c 30000)"
if [ -z "$unstaged_diff" ]; then
  unstaged_diff="<none>"
fi

{
  printf '## Tracked Status\\n%s\\n\\n' "$status"
  printf '## Staged Diff (truncated)\\n%s\\n\\n' "$staged_diff"
  printf '## Unstaged Diff (truncated)\\n%s\\n' "$unstaged_diff"
} > .vizier/tmp/commit-context.txt
SCRIPT
    }
    on = {
      succeeded = ["resolve_prompt"]
    }
  },
  {
    id = "resolve_prompt"
    kind = "builtin"
    uses = "cap.env.builtin.prompt.resolve"
    args = {
      prompt_file = ".vizier/prompts/COMMIT_PROMPTS.md"
    }
    produces = {
      succeeded = [{ custom = { type_id = "prompt_text", key = "commit_prompt" } }]
    }
    on = {
      succeeded = ["invoke_agent"]
    }
    after = [{ node_id = "collect_context" }]
  },
  {
    id = "invoke_agent"
    kind = "agent"
    uses = "cap.agent.invoke"
    needs = [{ custom = { type_id = "prompt_text", key = "commit_prompt" } }]
    produces = {
      succeeded = [{ custom = { type_id = "commit_message", key = "tracked_changes" } }]
    }
    on = {
      succeeded = ["stage_tracked"]
    }
    after = [{ node_id = "resolve_prompt" }]
  },
  {
    id = "stage_tracked"
    kind = "shell"
    uses = "cap.env.shell.command.run"
    args = {
      script = <<-SCRIPT
git reset --quiet
git add -u
SCRIPT
    }
    on = {
      succeeded = ["commit_tracked"]
    }
    after = [{ node_id = "invoke_agent" }]
  },
  {
    id = "commit_tracked"
    kind = "builtin"
    uses = "cap.env.builtin.git.commit"
    needs = [{ custom = { type_id = "commit_message", key = "tracked_changes" } }]
    args = {
      message = "read_payload(commit_message)"
    }
    on = {
      succeeded = ["terminal"]
    }
    after = [{ node_id = "stage_tracked" }]
  },
  {
    id = "terminal"
    kind = "gate"
    uses = "control.terminal"
    after = [{ node_id = "commit_tracked" }]
  }
]

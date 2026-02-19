id = "template.stage.approve"
version = "v2"

cli = {
  positional = ["slug", "branch"]
  named = {
    name = "slug"
  }
}

params = {
  branch = ""
  commit_message = "chore: workflow stage commit"
  slug = ""
  stop_condition_retries = "3"
  stop_condition_script = ""
}

policy = {
  dependencies = {
    missing_producer = "wait"
  }
}

artifact_contracts = [
  { id = "prompt_text", version = "v1" },
  { id = "plan_branch", version = "v1" },
  { id = "plan_doc", version = "v1" },
  { id = "stage_token", version = "v1" }
]

nodes = [
  {
    id = "worktree_prepare"
    kind = "builtin"
    uses = "cap.env.builtin.worktree.prepare"
    args = {
      branch = "$${branch}"
      slug = "$${slug}"
      purpose = "stage-approve"
    }
    needs = [
      { plan_branch = { slug = "$${slug}", branch = "$${branch}" } },
      { plan_doc = { slug = "$${slug}", branch = "$${branch}" } }
    ]
    on = {
      succeeded = ["resolve_prompt"]
    }
  },
  {
    id = "resolve_prompt"
    kind = "builtin"
    uses = "cap.env.builtin.prompt.resolve"
    args = {
      prompt_file = ".vizier/prompts/APPROVE_PROMPTS.md"
    }
    produces = {
      succeeded = [{ custom = { type_id = "prompt_text", key = "approve_main" } }]
    }
    on = {
      succeeded = ["invoke_agent"]
    }
    after = [{ node_id = "worktree_prepare" }]
  },
  {
    id = "invoke_agent"
    kind = "agent"
    uses = "cap.agent.invoke"
    needs = [{ custom = { type_id = "prompt_text", key = "approve_main" } }]
    on = {
      succeeded = ["stage_files"]
    }
    after = [{ node_id = "resolve_prompt" }]
  },
  {
    id = "stage_files"
    kind = "builtin"
    uses = "cap.env.builtin.git.stage"
    args = {
      files_json = "[\".\"]"
    }
    after = [{ node_id = "invoke_agent" }]
    on = {
      succeeded = ["stage_commit"]
    }
  },
  {
    id = "stage_commit"
    kind = "builtin"
    uses = "cap.env.builtin.git.commit"
    args = {
      message = "$${commit_message}"
    }
    after = [{ node_id = "stage_files" }]
    on = {
      succeeded = ["stop_gate"]
    }
  },
  {
    id = "stop_gate"
    kind = "gate"
    uses = "control.gate.stop_condition"
    gates = [
      {
        kind = "script"
        script = "$${stop_condition_script}"
        policy = "retry"
      }
    ]
    retry = {
      mode = "until_gate"
      budget = "$${stop_condition_retries}"
    }
    after = [{ node_id = "stage_commit" }]
    on = {
      succeeded = ["worktree_cleanup"]
      failed = ["stage_commit"]
    }
  },
  {
    id = "worktree_cleanup"
    kind = "builtin"
    uses = "cap.env.builtin.worktree.cleanup"
    on = {
      succeeded = ["terminal"]
    }
    after = [{ node_id = "stop_gate" }]
  },
  {
    id = "terminal"
    kind = "gate"
    uses = "control.terminal"
    produces = {
      succeeded = [{ custom = { type_id = "stage_token", key = "approve:$${slug}" } }]
    }
    after = [{ node_id = "worktree_cleanup" }]
  }
]

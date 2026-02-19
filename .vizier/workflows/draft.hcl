id = "template.stage.draft"
version = "v2"

cli = {
  positional = ["spec_file", "slug", "branch"]
  named = {
    file = "spec_file"
    name = "slug"
  }
}

params = {
  branch = ""
  commit_message = "chore: workflow stage commit"
  slug = ""
  spec_file = ""
  spec_source = "inline"
  spec_text = ""
}

policy = {
  dependencies = {
    missing_producer = "wait"
  }
}

artifact_contracts = [
  { id = "prompt_text", version = "v1" },
  { id = "plan_text", version = "v1" },
  { id = "plan_branch", version = "v1" },
  { id = "plan_doc", version = "v1" }
]

nodes = [
  {
    id = "worktree_prepare"
    kind = "builtin"
    uses = "cap.env.builtin.worktree.prepare"
    args = {
      branch = "$${branch}"
      slug = "$${slug}"
      purpose = "stage-draft"
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
      prompt_file = ".vizier/prompts/DRAFT_PROMPTS.md"
    }
    produces = {
      succeeded = [{ custom = { type_id = "prompt_text", key = "draft_main" } }]
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
    needs = [{ custom = { type_id = "prompt_text", key = "draft_main" } }]
    produces = {
      succeeded = [{ custom = { type_id = "plan_text", key = "draft_plan:$${slug}" } }]
    }
    on = {
      succeeded = ["persist_plan"]
    }
    after = [{ node_id = "resolve_prompt" }]
  },
  {
    id = "persist_plan"
    kind = "builtin"
    uses = "cap.env.builtin.plan.persist"
    args = {
      branch = "$${branch}"
      name_override = "$${slug}"
      spec_file = "$${spec_file}"
      spec_source = "$${spec_source}"
      spec_text = "$${spec_text}"
    }
    needs = [{ custom = { type_id = "plan_text", key = "draft_plan:$${slug}" } }]
    produces = {
      succeeded = [
        { plan_branch = { slug = "$${slug}", branch = "$${branch}" } },
        { plan_doc = { slug = "$${slug}", branch = "$${branch}" } }
      ]
    }
    on = {
      succeeded = ["stage_files"]
    }
    after = [{ node_id = "invoke_agent" }]
  },
  {
    id = "stage_files"
    kind = "builtin"
    uses = "cap.env.builtin.git.stage"
    args = {
      files_json = "[\".\"]"
    }
    produces = {
      succeeded = [
        { plan_branch = { slug = "$${slug}", branch = "$${branch}" } },
        { plan_doc = { slug = "$${slug}", branch = "$${branch}" } }
      ]
    }
    after = [{ node_id = "persist_plan" }]
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
    produces = {
      succeeded = [
        { plan_branch = { slug = "$${slug}", branch = "$${branch}" } },
        { plan_doc = { slug = "$${slug}", branch = "$${branch}" } }
      ]
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
    on = {
      succeeded = ["worktree_cleanup"]
    }
    after = [{ node_id = "stage_commit" }]
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
    after = [{ node_id = "worktree_cleanup" }]
  }
]

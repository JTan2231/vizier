id = "template.develop"
version = "v1"

cli = {
  positional = ["spec_file", "slug", "branch", "target_branch"]
  named = {
    file = "spec_file"
    name = "slug"
    source = "branch"
    target = "target_branch"
  }
}

imports = [
  { name = "develop_draft", path = "workflows/draft.hcl" },
  { name = "develop_approve", path = "workflows/approve.hcl" },
  { name = "develop_merge", path = "workflows/merge.hcl" }
]

links = [
  { from = "develop_draft", to = "develop_approve" },
  { from = "develop_approve", to = "develop_merge" }
]

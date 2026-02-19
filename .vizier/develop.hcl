id = "template.develop"
version = "v1"

imports = [
  { name = "develop_draft", path = "workflows/draft.hcl" },
  { name = "develop_approve", path = "workflows/approve.hcl" },
  { name = "develop_merge", path = "workflows/merge.hcl" }
]

links = [
  { from = "develop_draft", to = "develop_approve" },
  { from = "develop_approve", to = "develop_merge" }
]

Link: This should leverage the new CommitTransaction primitive to make composition predictable. All higher-level workflows (batch file updates, AI-applied refactors) must express intentions as planned IndexEdit/TreeWrite operations and defer execution. Avoid duplicating transaction management here; consume it.

---


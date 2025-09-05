Objective: Provide a cost-free, deterministic testing loop that exercises vizier’s LLM-driven paths without hitting paid providers.

Why this matters in our story: The CLI promises safe, iterative development with “AI help,” but every run currently burns tokens. This breaks the contract for quick feedback and makes regression testing impractical. We need a local model and a record/replay harness so tests are fast, cheap, and stable.

Deliverables:
- Add a `--provider local` path in cli/src/main.rs::provider_arg_to_enum that maps to a new Local provider variant in wire::types::API. If `wire` doesn’t currently support Local, add it as API::Local(LocalModel::MiniCPM | Llama3.1 | Mistral). The goal is compile-time enforced routing, not stringly-typed checks.
- Wire an adapter in prompts crate that, when API::Local is selected, routes requests to: 
  - (A) an on-device server if VIZIER_LOCAL_LLM_ENDPOINT is set (OpenAI-compatible schema), or
  - (B) a pure record-replay stub that loads canned responses from .vizier/fixtures/{hash}.json for deterministic tests.
- Introduce record/replay fixture keying: compute a stable hash from (system_prompt, user_prompt, tool_schema_names, tool_calls_flag, model_name). Use it to store and load responses. Provide a `vizier --save-fixture` mode to regenerate fixtures by calling a real provider once, then store the response.
- Add a `tests/` crate-level integration test that runs `vizier` against a small repo fixture, with provider=local + replay mode; assert:
  - No network calls are performed (gate via env var VIZIER_DISABLE_NETWORK=1; adapter must hard-error if any remote request is attempted in replay).
  - `.vizier/` files updated as expected (TODO created/updated, snapshot touched).
- In Auditor::llm_request and llm_request_with_tools, plumb a `Determinism` enum (Live | Record | Replay). Read from env: VIZIER_LLM_MODE=live|record|replay. Default to live for normal runs, replay inside `cargo test`.
- Provide install-time help: update README with a “Local testing” section showing how to install a local API (Ollama or llama.cpp server) and how to run replay-only tests.

Acceptance criteria:
- `cargo test` executes full-flow tests without contacting external providers and without incurring token costs.
- `vizier --provider local` works with an OpenAI-compatible local endpoint if available; otherwise fails over to replay mode with clear diagnostics.
- Recorded fixtures are deterministic (stable hash) and easy to update via `--save-fixture`.
- Breaks no existing provider paths.

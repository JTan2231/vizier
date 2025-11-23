## Gemini output consumer (vizier-style)

Update (2025-11-26): The Rust Codex/Gemini adapters were removed in favor of script shims (see `examples/agents/gemini.sh`); this document remains as a historical reference for the old JSON-stream adapter shape.

Background from `~/rust/vizier`: `vizier-core/src/codex.rs` adapts Codex agent
events into UI-friendly progress updates. It runs `codex exec --json --output-last-message <tmp> -`
in the repo root, streams newline-delimited JSON events, and uses
`CodexDisplayAdapter` to map each event payload into a `ProgressEvent` (phase,
label, message, detail, path, progress, status, timestamp, raw) before pushing
them to the UI hook.

Gemini’s programmatic outputs live in `packages/core/src/output/*.ts` and the
non-interactive entrypoint at `packages/cli/src/nonInteractiveCli.ts`:

- Output formats: text (default), JSON (`JsonFormatter`), and streaming JSON
  (`StreamJsonFormatter`). Streaming emits JSONL events with types
  `init | message | tool_use | tool_result | error | result`
  (see `packages/core/src/output/types.ts`).
- Headless/streaming is enabled via `--output-format stream-json` (or `json` for
  a single object). There is no separate `exec` subcommand; the CLI accepts the
  prompt via `--prompt/-p` or stdin/positional text and emits events.
- Errors in non-interactive mode are also encoded in JSON/streaming via
  `handleError` in `packages/cli/src/utils/errors.ts`.

Proposed Gemini consumer (mirroring the Codex adapter):

- Launch: spawn `gemini` with `--output-format stream-json` (plus model/profile
  flags) in the target repo root. Feed the prompt on stdin to match Codex’s
  `--output-last-message -` flow. Capture stdout lines; stderr is for logs.
- Parsing: treat stdout as JSONL `JsonStreamEvent`. Stop when you see a
  `result` event (status success/error) or EOF; honor non-zero exit codes as
  failures.
- Adaptation to progress events (Codex-style fields):
  - `init`: phase=`init`, label=model, detail=session_id, status=`running`.
  - `message`: phase=`assistant`/`user`, message=content (append assistant lines
    to build `assistant_text`), status=`stream` when `delta` is true.
  - `tool_use`: phase=`tool`, label=tool_name, detail=tool_id,
    message=stringified parameters, status=`running`.
  - `tool_result`: phase=`tool`, detail=tool_id, message=output or error,
    status=`success`/`error`, progress=1.0.
  - `error`: phase=`error`, message=payload message, status=`error`.
  - `result`: phase=`complete`, status=`success`/`error`, message summarizing
    duration/tokens, stats from `result.stats` → usage.
- Response assembly: return `{assistant_text, usage, events}` mirroring
  `CodexResponse`, where `usage` aggregates `result.stats` tokens (from
  `convertToStreamStats`) and `events` is the raw stream for audit/logging.
- UI hook: forward each adapted progress event to the display system and surface
  the final `assistant_text` (or a structured error) once `result` arrives.

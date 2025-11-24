#!/bin/sh
set -euo pipefail

# codex/filter.sh
# -----------------
# This script sits between the Codex JSONL event stream and Vizier. Codex writes
# newline-delimited JSON events to stdout; Vizier wants progress on stderr and a
# single final agent reply on stdout. The filter:
#   - Parses each event with jq (only dependency) and renders human-friendly
#     progress lines to stderr.
#   - Tracks the final agent message (or a reasonable fallback) and prints that
#     once at the end to stdout.
#
# NOTE: This script _must_ emit actionable output through stdout--the Vizier
#       doesn't act on stderr, and will only use output from stdout.
#
# ANSI color is optional: it enables when stderr is a TTY and both NO_COLOR and
# VIZIER_NO_ANSI are unset; otherwise output stays plain for logs/pipes.
USE_COLOR=0
if [ -t 2 ] && [ -z "${NO_COLOR:-}" ] && [ -z "${VIZIER_NO_ANSI:-}" ]; then
  USE_COLOR=1
fi

if ! command -v jq >/dev/null 2>&1; then
  printf '[codex-filter] jq is required on PATH to render progress\n' >&2
  exit 1
fi

# Collapse whitespace so multi-line JSON/text shows up as single-line progress.
collapse() {
  printf '%s' "$1" | tr '\n' ' ' | sed 's/[[:space:]]\+/ /g; s/^ //; s/ $//'
}

# Wrap text in ANSI color codes when allowed. Pass standard SGR codes (e.g.,
# "32" for green, "33" for yellow). Falls back to raw text when color is off.
paint() {
  code="$1"
  shift
  text="$*"
  if [ "$USE_COLOR" -eq 1 ]; then
    # shellcheck disable=SC2059
    printf '\033[%sm%s\033[0m' "$code" "$text"
  else
    printf '%s' "$text"
  fi
}

# Human-friendly token usage formatter. Accepts the full JSON line so we can
# pull whatever fields the backend emits without coupling to an exact schema.
format_usage() {
  line="$1"
  in=$(printf '%s' "$line" | jq -r 'try (.usage.input_tokens // .usage.prompt_tokens) // ""' 2>/dev/null)
  cached=$(printf '%s' "$line" | jq -r 'try .usage.cached_input_tokens // ""' 2>/dev/null)
  out=$(printf '%s' "$line" | jq -r 'try (.usage.output_tokens // .usage.completion_tokens) // ""' 2>/dev/null)
  reasoning=$(printf '%s' "$line" | jq -r 'try .usage.reasoning_output_tokens // ""' 2>/dev/null)
  total=$(printf '%s' "$line" | jq -r 'try (.usage.total_tokens // .usage.total) // ""' 2>/dev/null)

  parts=""
  if [ -n "$total" ] && [ "$total" != "null" ]; then
    parts="total=$total"
  fi

  if [ -n "$in" ] && [ "$in" != "null" ]; then
    [ -n "$parts" ] && parts="$parts "
    parts="${parts}input=$in"
    if [ -n "$cached" ] && [ "$cached" != "null" ] && [ "$cached" != "0" ]; then
      parts="${parts} (cached=$cached)"
    fi
  fi

  if [ -n "$out" ] && [ "$out" != "null" ]; then
    [ -n "$parts" ] && parts="$parts "
    parts="${parts}output=$out"
  fi

  if [ -n "$reasoning" ] && [ "$reasoning" != "null" ]; then
    [ -n "$parts" ] && parts="$parts "
    parts="${parts}reasoning=$reasoning"
  fi

  printf '%s' "$parts"
}

# Small helper to keep stderr writes consistent.
progress() {
  msg="$1"
  [ -n "$msg" ] && printf '%s\n' "$msg" >&2
}

# We hold on to two strings:
#   final_text    → the agent_message payload (ideal stdout)
#   fallback_text → last useful text seen (covers no agent_message edge cases)
final_text=""
fallback_text=""

# Main loop: consume JSONL, emit progress, remember final output.
while IFS= read -r line; do
  # Ignore accidental blank lines so they don't spam stderr.
  [ -z "$line" ] && continue

  # Most events carry a "type"; when they do not, just surface the raw line.
  type=$(printf '%s' "$line" | jq -r 'try .type // ""' 2>/dev/null)
  if [ -z "$type" ]; then
    progress "$(collapse "$line")"
    continue
  fi

  case "$type" in
    thread.started)
      thread_id=$(printf '%s' "$line" | jq -r 'try .thread_id // ""' 2>/dev/null)
      if [ -n "$thread_id" ] && [ "$thread_id" != "null" ]; then
        progress "$(paint 36 "thread started (id=$thread_id)")"
      else
        progress "$(paint 36 "thread started")"
      fi
      ;;

    turn.started)
      progress "$(paint 35 "turn started")"
      ;;

    turn.completed)
      usage=$(format_usage "$line")
      if [ -n "$usage" ]; then
        progress "$(paint 32 "turn completed — $usage")"
      else
        progress "$(paint 32 "turn completed")"
      fi
      ;;

    item.started|item.completed)
      item_type=$(printf '%s' "$line" | jq -r 'try .item.type // ""' 2>/dev/null)
      item_text=$(printf '%s' "$line" | jq -r 'try .item.text // ""' 2>/dev/null)
      item_status=$(printf '%s' "$line" | jq -r 'try .item.status // ""' 2>/dev/null)
      item_progress=$(printf '%s' "$line" | jq -r 'try (.item.progress // .progress) // ""' 2>/dev/null)

      case "$item_type" in
        # Codex runs short scripts/commands; we narrate start/finish.
        command_execution)
          command=$(printf '%s' "$line" | jq -r 'try .item.command // ""' 2>/dev/null)
          exit_code=$(printf '%s' "$line" | jq -r 'try .item.exit_code // ""' 2>/dev/null)
          if [ "$type" = "item.started" ]; then
            if [ -n "$command" ] && [ "$command" != "null" ]; then
              progress "$(paint 34 "running: $command")"
            else
              progress "$(paint 34 "running command")"
            fi
          else
            message="command finished"
            if [ -n "$exit_code" ] && [ "$exit_code" != "null" ]; then
              message="$message (exit $exit_code)"
            elif [ -n "$item_status" ]; then
              message="$message [$item_status]"
            fi
            progress "$(paint 34 "$message")"
          fi
          ;;

        # Thinking text is both progress and a fallback if no final reply shows.
        reasoning)
          summary=$(collapse "$item_text")
          fallback_text="$summary"
          if [ -n "$item_progress" ] && [ "$item_progress" != "null" ]; then
            progress "$(paint 33 "thinking ($item_progress%): $summary")"
          elif [ -n "$summary" ]; then
            progress "$(paint 33 "thinking: $summary")"
          else
            progress "$(paint 33 "thinking...")"
          fi
          ;;

        # The one we really want: stash for stdout and still surface a brief note.
        agent_message)
          final_text="$item_text"
          fallback_text="$item_text"
          progress "$(paint 32 "agent reply received")"
          ;;

        # Anything else: surface a compact status and remember it as a fallback.
        *)
          summary=$(collapse "$item_text")
          [ -z "$summary" ] && summary=$(collapse "$item_status")
          [ -n "$summary" ] && fallback_text="$summary"
          if [ -n "$summary" ]; then
            progress "$(paint 90 "$item_type: $summary")"
          else
            progress "$(paint 90 "$item_type update")"
          fi
          ;;
      esac
      ;;

    # Generic catch-all: use phase/label/message when present so progress stays meaningful.
    *)
      phase=$(printf '%s' "$line" | jq -r 'try (.phase // .label) // ""' 2>/dev/null)
      message=$(printf '%s' "$line" | jq -r 'try .message // ""' 2>/dev/null)
      detail=$(printf '%s' "$line" | jq -r 'try .detail // .path // ""' 2>/dev/null)
      summary=$(collapse "$message")
      [ -z "$summary" ] && summary=$(collapse "$detail")

      if [ -n "$phase" ] && [ -n "$summary" ]; then
        progress "$(paint 36 "$phase: $summary")"
      elif [ -n "$summary" ]; then
        progress "$(paint 36 "$summary")"
      else
        progress "$(paint 36 "event: $type")"
      fi
      ;;
  esac
done

# Emit the assistant reply once. If Codex never sent an agent_message, use the
# last useful text we saw (reasoning or other item) so callers still get output.
if [ -n "$final_text" ]; then
  printf '%s\n' "$final_text"
elif [ -n "$fallback_text" ]; then
  printf '%s\n' "$fallback_text"
fi

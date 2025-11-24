#!/bin/sh
set -euo pipefail

# Render Codex JSONL progress to readable stderr lines and surface the final
# agent message on stdout. This keeps progress noisy but the end result clean.

if ! command -v jq >/dev/null 2>&1; then
  printf '[codex-filter] jq is required on PATH to render progress\n' >&2
  exit 1
fi

collapse() {
  # Flatten newlines and excess whitespace so progress stays single-line.
  printf '%s' "$1" | tr '\n' ' ' | sed 's/[[:space:]]\+/ /g; s/^ //; s/ $//'
}

format_usage() {
  usage_line="$1"
  fu_input=$(printf '%s' "$usage_line" | jq -r 'try (.usage.input_tokens // .usage.prompt_tokens) // ""' 2>/dev/null)
  fu_cached=$(printf '%s' "$usage_line" | jq -r 'try .usage.cached_input_tokens // ""' 2>/dev/null)
  fu_output=$(printf '%s' "$usage_line" | jq -r 'try (.usage.output_tokens // .usage.completion_tokens) // ""' 2>/dev/null)
  fu_reasoning=$(printf '%s' "$usage_line" | jq -r 'try .usage.reasoning_output_tokens // ""' 2>/dev/null)
  fu_total=$(printf '%s' "$usage_line" | jq -r 'try (.usage.total_tokens // .usage.total) // ""' 2>/dev/null)

  fu_parts=""
  if [ -n "$fu_total" ] && [ "$fu_total" != "null" ]; then
    fu_parts="total=$fu_total"
  fi

  if [ -n "$fu_input" ] && [ "$fu_input" != "null" ]; then
    [ -n "$fu_parts" ] && fu_parts="$fu_parts "
    fu_parts="${fu_parts}input=$fu_input"
    if [ -n "$fu_cached" ] && [ "$fu_cached" != "null" ] && [ "$fu_cached" != "0" ]; then
      fu_parts="${fu_parts} (cached=$fu_cached)"
    fi
  fi

  if [ -n "$fu_output" ] && [ "$fu_output" != "null" ]; then
    [ -n "$fu_parts" ] && fu_parts="$fu_parts "
    fu_parts="${fu_parts}output=$fu_output"
  fi

  if [ -n "$fu_reasoning" ] && [ "$fu_reasoning" != "null" ]; then
    [ -n "$fu_parts" ] && fu_parts="$fu_parts "
    fu_parts="${fu_parts}reasoning=$fu_reasoning"
  fi

  printf '%s' "$fu_parts"
}

print_progress() {
  if [ -n "$1" ]; then
    printf '%s\n' "$1" >&2
  fi
}

final_text=""
fallback_text=""

while IFS= read -r line; do
  [ -z "$line" ] && continue

  type=$(printf '%s' "$line" | jq -r 'try .type // ""' 2>/dev/null)
  if [ -z "$type" ]; then
    print_progress "$(collapse "$line")"
    continue
  fi

  case "$type" in
    thread.started)
      thread_id=$(printf '%s' "$line" | jq -r 'try .thread_id // ""' 2>/dev/null)
      if [ -n "$thread_id" ] && [ "$thread_id" != "null" ]; then
        print_progress "thread started (id=$thread_id)"
      else
        print_progress "thread started"
      fi
      ;;

    turn.started)
      print_progress "turn started"
      ;;

    turn.completed)
      usage=$(format_usage "$line")
      if [ -n "$usage" ]; then
        print_progress "turn completed — $usage"
      else
        print_progress "turn completed"
      fi
      ;;

    item.started|item.completed)
      item_type=$(printf '%s' "$line" | jq -r 'try .item.type // ""' 2>/dev/null)
      item_text=$(printf '%s' "$line" | jq -r 'try .item.text // ""' 2>/dev/null)
      item_status=$(printf '%s' "$line" | jq -r 'try .item.status // ""' 2>/dev/null)
      progress=$(printf '%s' "$line" | jq -r 'try (.item.progress // .progress) // ""' 2>/dev/null)

      case "$item_type" in
        command_execution)
          command=$(printf '%s' "$line" | jq -r 'try .item.command // ""' 2>/dev/null)
          exit_code=$(printf '%s' "$line" | jq -r 'try .item.exit_code // ""' 2>/dev/null)
          if [ "$type" = "item.started" ]; then
            if [ -n "$command" ] && [ "$command" != "null" ]; then
              print_progress "running: $command"
            else
              print_progress "running command"
            fi
          else
            message="command finished"
            if [ -n "$exit_code" ] && [ "$exit_code" != "null" ]; then
              message="$message (exit $exit_code)"
            elif [ -n "$item_status" ]; then
              message="$message [$item_status]"
            fi
            print_progress "$message"
          fi
          ;;

        reasoning)
          summary=$(collapse "$item_text")
          fallback_text="$summary"
          if [ -n "$progress" ] && [ "$progress" != "null" ]; then
            print_progress "thinking ($progress%): $summary"
          elif [ -n "$summary" ]; then
            print_progress "thinking: $summary"
          else
            print_progress "thinking..."
          fi
          ;;

        agent_message)
          final_text="$item_text"
          fallback_text="$item_text"
          print_progress "agent reply received"
          ;;

        *)
          summary=$(collapse "$item_text")
          [ -z "$summary" ] && summary=$(collapse "$item_status")
          [ -n "$summary" ] && fallback_text="$summary"
          if [ -n "$summary" ]; then
            print_progress "$item_type: $summary"
          else
            print_progress "$item_type update"
          fi
          ;;
      esac
      ;;

    *)
      phase=$(printf '%s' "$line" | jq -r 'try (.phase // .label) // ""' 2>/dev/null)
      message=$(printf '%s' "$line" | jq -r 'try .message // ""' 2>/dev/null)
      detail=$(printf '%s' "$line" | jq -r 'try .detail // .path // ""' 2>/dev/null)
      summary=$(collapse "$message")
      [ -z "$summary" ] && summary=$(collapse "$detail")
      if [ -n "$phase" ] && [ -n "$summary" ]; then
        print_progress "$phase: $summary"
      elif [ -n "$summary" ]; then
        print_progress "$summary"
      else
        print_progress "event: $type"
      fi
      ;;
  esac
done

# Surface the final assistant text on stdout; fall back to the last useful text
# so commands still yield something when no agent_message event arrives.
if [ -n "$final_text" ]; then
  printf '%s\n' "$final_text"
elif [ -n "$fallback_text" ]; then
  printf '%s\n' "$fallback_text"
fi

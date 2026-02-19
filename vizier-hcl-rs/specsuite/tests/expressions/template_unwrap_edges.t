result = {
  strip_marker_bool = true
  strip_marker_object = {
    answer = 42
  }

  quoted_prefix_literal = "true"
  quoted_suffix_literal = "true"
  quoted_wrapped_literal = "xtruey"
  directive_lookalike = "true"
  escaped_interpolation_lookalike = "$${true}"
  escaped_directive_lookalike = "%%{ if true }"
  directive_with_escaped_interpolation = "$${true}"
  directive_with_escaped_directive = "%%{ if false }"

  heredoc_single_interp = "true\n"
  heredoc_single_strip_interp = "true"
  heredoc_single_interp_flush = "true\n"
}

result_type = object({
  strip_marker_bool = bool
  strip_marker_object = object({
    answer = number
  })

  quoted_prefix_literal = string
  quoted_suffix_literal = string
  quoted_wrapped_literal = string
  directive_lookalike = string
  escaped_interpolation_lookalike = string
  escaped_directive_lookalike = string
  directive_with_escaped_interpolation = string
  directive_with_escaped_directive = string

  heredoc_single_interp = string
  heredoc_single_strip_interp = string
  heredoc_single_interp_flush = string
})

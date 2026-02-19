strip_marker_bool = "${~true~}"
strip_marker_object = "${~{ answer = 42 }~}"

quoted_prefix_literal = " ${~true~}"
quoted_suffix_literal = "${~true~} "
quoted_wrapped_literal = "x${~true~}y"
directive_lookalike = "%{~ if true }${true}%{ endif ~}"
escaped_interpolation_lookalike = "$${true}"
escaped_directive_lookalike = "%%{ if true }"
directive_with_escaped_interpolation = "%{ if true }$${true}%{ endif }"
directive_with_escaped_directive = "%{ if true }%%{ if false }%{ endif }"

heredoc_single_interp = <<EOT
${true}
EOT

heredoc_single_strip_interp = <<EOT
${~true~}
EOT

heredoc_single_interp_flush = <<-EOT
  ${true}
EOT

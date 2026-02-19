quoted_interp = "A ${~value~} B"
quoted_if = "A %{~ if enabled }X%{ endif ~} B"
quoted_for = "A %{~ for ignored, v in items }${v}%{ endfor ~} B"

heredoc_interp = <<-EOT
  A ${~value~} B
EOT

heredoc_if = <<-EOT
  A %{~ if enabled }X%{ endif ~} B
EOT

heredoc_for = <<-EOT
  A %{~ for ignored, v in items }${v}%{ endfor ~} B
EOT

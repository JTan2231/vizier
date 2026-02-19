result = {
  quoted_interp = "AXB"
  quoted_if = "AXB"
  quoted_for = "AxyB"

  heredoc_interp = "AXB\n"
  heredoc_if = "AXB\n"
  heredoc_for = "AxyB\n"
}

result_type = object({
  quoted_interp = string
  quoted_if = string
  quoted_for = string

  heredoc_interp = string
  heredoc_if = string
  heredoc_for = string
})

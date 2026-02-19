diagnostics {
  error {
    source = "hcl"
    # Message like "duplicate block \"a\" in this body"
    from {
      line   = 2
      column = 1
      byte   = 5
    }
    to {
      line   = 2
      column = 2
      byte   = 6
    }
  }
}

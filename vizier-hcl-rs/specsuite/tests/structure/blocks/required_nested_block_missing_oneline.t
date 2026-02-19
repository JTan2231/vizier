diagnostics {
  error {
    source = "hcl"
    # Message like "missing required block \"d\" in block \"a\""
    from {
      line   = 1
      column = 1
      byte   = 0
    }
    to {
      line   = 1
      column = 2
      byte   = 1
    }
  }
}

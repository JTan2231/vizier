diagnostics {
  error {
    source = "hcl"
    # Message like "a block named \"z\" is not expected here"
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

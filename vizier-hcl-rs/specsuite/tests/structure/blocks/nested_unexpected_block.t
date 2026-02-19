diagnostics {
  error {
    source = "hcl"
    # Message like "a block named \"e\" is not expected here"
    from {
      line   = 2
      column = 3
      byte   = 6
    }
    to {
      line   = 2
      column = 4
      byte   = 7
    }
  }
}

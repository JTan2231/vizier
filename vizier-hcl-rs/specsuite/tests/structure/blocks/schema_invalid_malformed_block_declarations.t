diagnostics {
  error {
    source = "hcldec"
    # Message like "schema \"block\" block must declare a block type"
    from {
      line   = 2
      column = 3
      byte   = 11
    }
    to {
      line   = 4
      column = 4
      byte   = 36
    }
  }
  error {
    source = "hcldec"
    # Message like "schema \"required\" argument must be a boolean literal"
    from {
      line   = 6
      column = 5
      byte   = 56
    }
    to {
      line   = 6
      column = 21
      byte   = 72
    }
  }
  error {
    source = "hcldec"
    # Message like "schema \"block_list\" block must declare a block type"
    from {
      line   = 5
      column = 3
      byte   = 39
    }
    to {
      line   = 8
      column = 4
      byte   = 90
    }
  }
}

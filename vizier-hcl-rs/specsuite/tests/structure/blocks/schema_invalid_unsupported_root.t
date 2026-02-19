diagnostics {
  error {
    source = "hcldec"
    # Message like "unsupported schema root block \"literal\""
    from {
      line   = 1
      column = 1
      byte   = 0
    }
    to {
      line   = 1
      column = 8
      byte   = 7
    }
  }
}

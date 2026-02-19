diagnostics {
  error {
    source = "hcldec"
    # Message like "duplicate schema attribute \"dup\""
    from {
      line   = 5
      column = 3
      byte   = 48
    }
    to {
      line   = 7
      column = 4
      byte   = 82
    }
  }
  error {
    source = "hcldec"
    # Message like "duplicate schema block \"svc\""
    from {
      line   = 11
      column = 3
      byte   = 119
    }
    to {
      line   = 13
      column = 4
      byte   = 150
    }
  }
}

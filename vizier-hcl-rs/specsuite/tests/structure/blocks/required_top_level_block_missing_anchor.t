diagnostics {
  error {
    source = "hcl"
    # Message like "missing required block \"svc\""
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

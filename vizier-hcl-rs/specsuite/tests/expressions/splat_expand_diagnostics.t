diagnostics {
  error {
    # "attribute traversal requires object target"
    from {
      byte = 34
    }
    to {
      byte = 38
    }
  }

  error {
    # "splat traversal" "tuple or object"
    from {
      byte = 68
    }
    to {
      byte = 70
    }
  }

  error {
    # "expansion" "requires tuple"
    from {
      byte = 97
    }
    to {
      byte = 100
    }
  }

  error {
    # "function `length` expects exactly 1 argument"
    from {
      byte = 120
    }
    to {
      byte = 136
    }
  }
}

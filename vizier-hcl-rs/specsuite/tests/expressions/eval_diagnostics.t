diagnostics {
  error {
    # "object does not contain key"
    from {
      byte = 17
    }
    to {
      byte = 25
    }
  }

  error {
    # "tuple index" "out of range"
    from {
      byte = 46
    }
    to {
      byte = 49
    }
  }

  error {
    # "index traversal requires tuple or object target"
    from {
      byte = 68
    }
    to {
      byte = 71
    }
  }

  error {
    # "collection must evaluate to tuple or object"
    from {
      byte = 99
    }
    to {
      byte = 100
    }
  }

  error {
    # "`for` expression `if` filter" "must evaluate to bool"
    from {
      byte = 140
    }
    to {
      byte = 141
    }
  }

  error {
    # "object `for` key expression must evaluate to string-like value"
    from {
      byte = 177
    }
    to {
      byte = 179
    }
  }

  error {
    # "duplicate key" "without grouping"
    from {
      byte = 223
    }
    to {
      byte = 228
    }
  }

  error {
    # "function `length` expects exactly 1 argument"
    from {
      byte = 246
    }
    to {
      byte = 254
    }
  }

  error {
    # "function `keys` expects object argument"
    from {
      byte = 265
    }
    to {
      byte = 276
    }
  }

  error {
    # "unknown function"
    from {
      byte = 290
    }
    to {
      byte = 300
    }
  }

  error {
    # "function `length` expects exactly 1 argument"
    from {
      byte = 313
    }
    to {
      byte = 329
    }
  }

  error {
    # "template `if` condition" "must evaluate to bool"
    from {
      byte = 357
    }
    to {
      byte = 358
    }
  }

  error {
    # "template `for` directive" "collection must evaluate to tuple or object"
    from {
      byte = 413
    }
    to {
      byte = 414
    }
  }
}

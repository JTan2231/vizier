diagnostics {
  error {
    # "tuple index 1" "out of range"
    from {
      byte = 203
    }
    to {
      byte = 206
    }
  }

  error {
    # "tuple index 1" "out of range"
    from {
      byte = 254
    }
    to {
      byte = 256
    }
  }

  error {
    # "index traversal requires tuple or object target"
    from {
      byte = 330
    }
    to {
      byte = 333
    }
  }

  error {
    # "attribute traversal requires object target"
    from {
      byte = 396
    }
    to {
      byte = 400
    }
  }

  error {
    # "splat traversal requires tuple or object"
    from {
      byte = 439
    }
    to {
      byte = 441
    }
  }
}

result = {
  attr_full_index = ["a0", "b0"]
  attr_full_legacy = ["a0", "b0"]
  full_attr_index = ["a0", "b0"]
  full_attr_legacy = ["a0", "b0"]

  attr_full_fail_index_tail = null
  full_attr_fail_legacy_tail = null

  multi_branch_fail_tail_index = null
  multi_branch_fail_tail_attr = null

  full_scalar_chain_fail = null
  full_null_chain_empty = []
}

result_type = object({
  attr_full_index = [string, string]
  attr_full_legacy = [string, string]
  full_attr_index = [string, string]
  full_attr_legacy = [string, string]

  attr_full_fail_index_tail = any
  full_attr_fail_legacy_tail = any

  multi_branch_fail_tail_index = any
  multi_branch_fail_tail_attr = any

  full_scalar_chain_fail = any
  full_null_chain_empty = []
})

diagnostics {
  error {
    # "splat traversal requires tuple or object target"
    from {
      byte = 89
    }
    to {
      byte = 91
    }
  }

  error {
    # "splat traversal requires tuple or object target"
    from {
      byte = 117
    }
    to {
      byte = 119
    }
  }
}

result = {
  full_scalar_singleton = [7]
  full_null_empty = []
  attr_scalar_illegal = null
  attr_null_illegal = null
}

result_type = object({
  full_scalar_singleton = [number]
  full_null_empty = []
  attr_scalar_illegal = any
  attr_null_illegal = any
})

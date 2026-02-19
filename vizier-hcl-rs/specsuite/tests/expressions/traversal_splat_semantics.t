diagnostics {
  error {
    # "attribute traversal requires object target"
    from {
      byte = 118
    }
    to {
      byte = 122
    }
  }
}

result = {
  attr_vs_full_index_attr = [{
    bar = "a"
  }]
  full_index_attr = [
    {
      bar = "a"
    },
    {
      bar = "b"
    },
  ]
  attr_vs_full_trailing_attr = null
  full_trailing_attr = ["a", "b"]
}

result_type = object({
  attr_vs_full_index_attr = [object({
    bar = string
  })]
  full_index_attr = [object({
    bar = string
  }), object({
    bar = string
  })]
  attr_vs_full_trailing_attr = any
  full_trailing_attr = [string, string]
})

result = {
  single_bool = true
  single_object = {
    answer = 42
  }
  nested = true
  multi_interp_counterexample = "true"
  directive_counterexample = "true"
}

result_type = object({
  single_bool = bool
  single_object = object({
    answer = number
  })
  nested = bool
  multi_interp_counterexample = string
  directive_counterexample = string
})

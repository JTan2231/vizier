result = {
  length_string = 4
  length_tuple = 2
  length_object = 2
  keys_object = ["a", "b"]
  values_object = [1, 2]
  nested = 2
  expand_length = 2
  expand_keys = ["a", "b"]
  expand_values = [1, 2]
}
result_type = object({
  length_string = number
  length_tuple = number
  length_object = number
  keys_object = [string, string]
  values_object = [number, number]
  nested = number
  expand_length = number
  expand_keys = [string, string]
  expand_values = [number, number]
})

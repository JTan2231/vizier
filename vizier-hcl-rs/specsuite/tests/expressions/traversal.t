result = {
  attr = "vizier"
  index_tuple = "b"
  legacy_tuple = "c"
  index_object = "vizier"
  legacy_object = "zero"
  nested = 2
  mixed = {
    first = "a"
    second = 2
  }
  attr_splat_names = ["api", "worker"]
  full_splat_names = ["api", "worker"]
  full_splat_nested_values = [10, 20]
  full_splat_index = [2, 4]
  object_attr_splat_names = ["left", "right"]
  object_full_splat_names = ["left", "right"]
}
result_type = object({
  attr = string
  index_tuple = string
  legacy_tuple = string
  index_object = string
  legacy_object = string
  nested = number
  mixed = object({
    first = string
    second = number
  })
  attr_splat_names = [string, string]
  full_splat_names = [string, string]
  full_splat_nested_values = [number, number]
  full_splat_index = [number, number]
  object_attr_splat_names = [string, string]
  object_full_splat_names = [string, string]
})

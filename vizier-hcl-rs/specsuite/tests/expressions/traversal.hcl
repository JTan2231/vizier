attr = obj.name
index_tuple = tuple[1]
legacy_tuple = tuple.2
index_object = obj["name"]
legacy_object = obj.0
nested = obj.nested.value
mixed = {
  first = tuple[0]
  second = obj["nested"].value
}
attr_splat_names = servers.*.name
full_splat_names = servers[*].name
full_splat_nested_values = servers[*].nested.value
full_splat_index = matrix[*][1]
object_attr_splat_names = obj_map.*.name
object_full_splat_names = obj_map[*].name

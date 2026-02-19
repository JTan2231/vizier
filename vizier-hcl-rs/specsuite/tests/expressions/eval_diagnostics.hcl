missing_key = obj.missing
out_of_range = tuple[3]
invalid_target = 1[0]
for_collection = [for v in 1: v]
for_filter = [for v in tuple: v if 1]
for_object_key = {for v in tuple: [] => v}
for_duplicate_key = {for v in tuple: "dup" => v}
fn_arity = length()
fn_type = keys(tuple)
fn_unknown = unknown(1)
fn_expand = length(tuple...)
template_if_type = "x%{ if 1 }y%{ endif }"
template_for_collection = "x%{ for v in 1 }${v}%{ endfor }"

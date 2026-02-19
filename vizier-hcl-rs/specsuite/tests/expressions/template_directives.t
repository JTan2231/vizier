result = {
  if_true = "status:on"
  if_false = "status:off"
  for_inline = "0:a;1:b;2:c;"
  nested = "ac"
}
result_type = object({
  if_true = string
  if_false = string
  for_inline = string
  nested = string
})

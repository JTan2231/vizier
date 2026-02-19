if_true = "status:%{ if enabled }on%{ else }off%{ endif }"
if_false = "status:%{ if !enabled }on%{ else }off%{ endif }"
for_inline = "%{ for i, v in items }${i}:${v};%{ endfor }"
nested = "%{ for ignored, v in items }%{ if v != skip }${v}%{ endif }%{ endfor }"

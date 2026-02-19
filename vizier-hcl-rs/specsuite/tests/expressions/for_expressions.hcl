tuple_basic = [for v in nums: v * 2]
tuple_filtered = [for i, v in nums: "${i}:${v}" if i < 2]
object_basic = {for k, v in records: k => v.value}
object_grouped = {for ignored, v in records: v.group => v.value...}

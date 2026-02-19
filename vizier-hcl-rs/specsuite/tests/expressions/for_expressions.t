result = {
  tuple_basic = [2, 4, 6]
  tuple_filtered = ["0:1", "1:2"]
  object_basic = {
    a = 1
    b = 2
    c = 3
  }
  object_grouped = {
    x = [1, 2]
    y = [3]
  }
}
result_type = object({
  tuple_basic = [number, number, number]
  tuple_filtered = [string, string]
  object_basic = object({
    a = number
    b = number
    c = number
  })
  object_grouped = object({
    x = [number, number]
    y = [number]
  })
})

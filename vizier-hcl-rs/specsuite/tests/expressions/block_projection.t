result = {
  service = {
    api = {
      v1 = {
        port = 8080
      }
      v2 = {
        port = 8081
      }
    }
    worker = {
      v1 = {
        queue = "jobs"
      }
    }
  }
  single = {
    enabled = true
  }
  labeled = {
    alpha = {
      name = "a"
    }
  }
}

result_type = object({
  service = object({
    api = object({
      v1 = object({
        port = number
      })
      v2 = object({
        port = number
      })
    })
    worker = object({
      v1 = object({
        queue = string
      })
    })
  })
  single = object({
    enabled = bool
  })
  labeled = object({
    alpha = object({
      name = string
    })
  })
})

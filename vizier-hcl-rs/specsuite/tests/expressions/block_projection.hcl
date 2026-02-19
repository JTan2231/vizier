service "api" "v1" {
  port = 8080
}

service "api" "v2" {
  port = 8081
}

service "worker" "v1" {
  queue = "jobs"
}

single {
  enabled = true
}

labeled "alpha" {
  name = "a"
}

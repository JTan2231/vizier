service "dup" "v1" {
  value = 1
}

service "dup" "v1" {
  value = 2
}

conflict = "attr"
conflict "x" "y" {
  value = 1
}

path "name" {
  leaf = 1
}

path "name" "leaf" "east" {
  value = 2
}

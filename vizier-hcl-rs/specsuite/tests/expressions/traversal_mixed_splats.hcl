attr_full_index = tuple.*.foo[*][0].bar
attr_full_legacy = tuple.*.foo[*].0.bar
full_attr_index = tuple[*].foo.*[0].bar
full_attr_legacy = tuple[*].foo.*.0.bar

attr_full_fail_index_tail = tuple.*.foo[*][1].bar
full_attr_fail_legacy_tail = tuple[*].foo.*.1.bar

multi_branch_fail_tail_index = branch_first_index_fail[*].foo[*].bar[0]
multi_branch_fail_tail_attr = branch_first_attr_fail[*].foo[*].bar[0]

full_scalar_chain_fail = scalar[*].*[0]
full_null_chain_empty = maybe[*].*[0]

diagnostics {
  error {
    # "unexpected template `else` directive"
    from {
      byte = 19
    }
    to {
      byte = 28
    }
  }

  error {
    # "unexpected template `else` directive"
    from {
      byte = 55
    }
    to {
      byte = 66
    }
  }

  error {
    # "unexpected template `endif` directive"
    from {
      byte = 88
    }
    to {
      byte = 98
    }
  }

  error {
    # "unexpected template `endfor` directive"
    from {
      byte = 127
    }
    to {
      byte = 140
    }
  }

  error {
    # "template `if` directive is missing `%{ endif }`"
    from {
      byte = 159
    }
    to {
      byte = 171
    }
  }

  error {
    # "template `if` directive is missing `%{ endif }`"
    from {
      byte = 201
    }
    to {
      byte = 214
    }
  }

  error {
    # "template `if` directive is missing `%{ endif }`"
    from {
      byte = 250
    }
    to {
      byte = 264
    }
  }

  error {
    # "template `for` directive is missing `%{ endfor }`"
    from {
      byte = 285
    }
    to {
      byte = 302
    }
  }

  error {
    # "template `for` directive is missing `%{ endfor }`"
    from {
      byte = 332
    }
    to {
      byte = 351
    }
  }
}

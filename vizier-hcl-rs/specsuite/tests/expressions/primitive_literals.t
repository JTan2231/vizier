result_type = object({
  whole_number                = number
  fractional_number           = number
  fractional_number_precision = number
  string_ascii                = string
  string_unicode_bmp          = string
  string_unicode_astral       = string
  string_unicode_nonnorm      = string
  string_unicode_nonnorm_escape = string
  string_unicode_nfc_escape     = string
  string_unicode_escape_equal   = bool
  string_unicode_nonnorm_escape_length = number
  string_unicode_nfc_escape_length     = number
  true                        = bool
  false                       = bool
  null                        = any
})
result = {
  # Numbers
  whole_number                = 5
  fractional_number           = 3.2
  fractional_number_precision = 3.14159265358979323846264338327950288419716939937510582097494459

  # Strings
  string_ascii = "hello"
  string_unicode_bmp = "ЖЖ"
  string_unicode_astral = "👩‍👩‍👧‍👦"
  string_unicode_nonnorm = "años" # now a precomposed ñ, because HCL imposes NFC normalization
  string_unicode_nonnorm_escape = "años"
  string_unicode_nfc_escape = "años"
  string_unicode_escape_equal = true
  string_unicode_nonnorm_escape_length = 4
  string_unicode_nfc_escape_length = 4

  # The left hand side of these are quoted to make it clear that we're expecting
  # to get strings here, not really true/false/null.
  "true"  = true
  "false" = false
  "null"  = null
}

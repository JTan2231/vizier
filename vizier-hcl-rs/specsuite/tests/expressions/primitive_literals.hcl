# Numbers
whole_number                = 5
fractional_number           = 3.2
fractional_number_precision = 3.14159265358979323846264338327950288419716939937510582097494459

# Strings
string_ascii           = "hello"
string_unicode_bmp     = "ЖЖ"
string_unicode_astral  = "👩‍👩‍👧‍👦"
string_unicode_nonnorm = "años" # This is intentionally a combining tilde followed by n
string_unicode_nonnorm_escape = "\u0061\u006E\u0303\u006F\u0073"
string_unicode_nfc_escape = "\u0061\u00F1\u006F\u0073"
string_unicode_escape_equal = "\u0061\u006E\u0303\u006F\u0073" == "\u0061\u00F1\u006F\u0073"
string_unicode_nonnorm_escape_length = length("\u0061\u006E\u0303\u006F\u0073")
string_unicode_nfc_escape_length = length("\u0061\u00F1\u006F\u0073")

# Booleans
true  = true
false = false

# Null
null = null

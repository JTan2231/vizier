use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, Expr, Fields, FieldsUnnamed, LitInt, Type};

fn generate_tuple_serialization(fields: &FieldsUnnamed) -> proc_macro2::TokenStream {
    let serializations = fields.unnamed.iter().enumerate().map(|(i, _)| {
        let index = syn::Index::from(i);
        quote! {
            bytes.extend(self.#index.to_bytes());
        }
    });

    quote! {
        #(#serializations)*
    }
}

fn generate_tuple_deserialization(fields: &FieldsUnnamed) -> proc_macro2::TokenStream {
    let deserializations = fields.unnamed.iter().map(|field| {
        let ty = &field.ty;
        quote! {
            let (value, count) = <#ty>::from_bytes(bytes, cursor)?;
            cursor += count;
        info!("cursor: {}", cursor);

            value
        }
    });

    quote! {
        (#(#deserializations),*)
    }
}

fn generate_array_serialization(_: &Type, _: usize) -> proc_macro2::TokenStream {
    quote! {
        for elem in &self.0 {
            bytes.extend(elem.to_bytes());
        }
    }
}

fn generate_array_deserialization(elem_ty: &Type, len: usize) -> proc_macro2::TokenStream {
    let len_lit = LitInt::new(&len.to_string(), Span::call_site());
    quote! {
        {
            let mut array = [<#elem_ty>::default(); #len_lit];
            for elem in &mut array {
                let (e, count) = <#elem_ty>::from_bytes(bytes, cursor)?;
                *elem = e;
                cursor += count;
        info!("cursor: {}", cursor);
            }

            Self(array)
        }
    }
}

// lol
// this is hideous
#[proc_macro_derive(Serialize)]
pub fn byte_serialize_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let (_, ty_generics, where_clause) = input.generics.split_for_impl();

    let (serialization, deserialization) = match &input.data {
        syn::Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => {
                let serialization_fields = fields.named.iter().filter_map(|field| {
                    let ignore = field
                        .attrs
                        .iter()
                        .any(|attr| attr.path().is_ident("ignore"));
                    let field_name = &field.ident;
                    if !ignore {
                        Some(quote! {
                            bytes.extend(self.#field_name.to_bytes());
                        })
                    } else {
                        None
                    }
                });

                let deserialization_fields = fields.named.iter().filter_map(|field| {
                    let ignore = field
                        .attrs
                        .iter()
                        .any(|attr| attr.path().is_ident("ignore"));

                    let field_name = &field.ident;
                    let ty = &field.ty;
                    if !ignore {
                        Some(quote! {
                            #field_name: {
                                let (value, count) = <#ty>::from_bytes(bytes, cursor)?;
                                cursor += count;
                                total_size += count;

                                value
                            },
                        })
                    } else {
                        Some(quote! {
                            #field_name: Default::default(),
                        })
                    }
                });

                (
                    quote! {
                        #(#serialization_fields)*
                    },
                    quote! {
                        #(#deserialization_fields)*
                    },
                )
            }
            Fields::Unnamed(fields) => {
                if fields.unnamed.len() == 1 {
                    if let Type::Array(array) = &fields.unnamed[0].ty {
                        let elem_ty = &array.elem;
                        if let Expr::Lit(expr_lit) = &array.len {
                            if let syn::Lit::Int(lit_int) = &expr_lit.lit {
                                let len = lit_int.base10_parse::<usize>().unwrap();
                                (
                                    generate_array_serialization(elem_ty, len),
                                    generate_array_deserialization(elem_ty, len),
                                )
                            } else {
                                panic!("Array length must be a literal integer")
                            }
                        } else {
                            panic!("Array length must be a literal")
                        }
                    } else {
                        (
                            generate_tuple_serialization(fields),
                            generate_tuple_deserialization(fields),
                        )
                    }
                } else {
                    (
                        generate_tuple_serialization(fields),
                        generate_tuple_deserialization(fields),
                    )
                }
            }
            Fields::Unit => (quote! {}, quote! {}),
        },
        _ => unimplemented!(),
    };

    let expanded = quote! {
        impl Serialize for #name # ty_generics #where_clause {
            fn to_bytes(&self) -> Vec<u8> {
                let mut bytes = Vec::new();
                #serialization
                bytes
            }

            fn from_bytes(mut bytes: &[u8], mut cursor: usize) -> Result<(Self, usize), std::io::Error> {
                let mut total_size = 0;
                Ok((Self {
                    #deserialization
                }, total_size))
            }
        }
    };

    TokenStream::from(expanded)
}

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, Pat, parse_macro_input};

#[proc_macro_attribute]
pub fn tool(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);
    let fn_name = &input.sig.ident;
    let struct_name = fn_name.to_string();
    let struct_name = format!(
        "{}{}Args",
        struct_name[..1].to_uppercase(),
        struct_name[1..].to_string()
    );
    let args_struct_name = format_ident!("{}", struct_name);

    let args: Vec<_> = input
        .sig
        .inputs
        .iter()
        .filter_map(|arg| {
            if let FnArg::Typed(pat_type) = arg {
                if let Pat::Ident(pat_ident) = &*pat_type.pat {
                    Some((pat_ident.ident.clone(), pat_type.ty.clone()))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    let struct_fields = args.iter().map(|(name, ty)| {
        quote! { #name: #ty }
    });

    quote! {
        #[derive(Debug, Default, Serialize, Deserialize)]
        pub struct #args_struct_name {
            #(#struct_fields),*
        }

        impl #args_struct_name {
            pub fn schema() -> serde_json::Value {
                serde_json::json!({
                    "method": stringify!(#fn_name),
                    "args": serde_json::json!(Self::default())
                })
            }
        }

        inventory::submit! {
            ToolInfo {
                name: stringify!(#fn_name),
                schema: #args_struct_name::schema,
            }
        }

        #input
    }
    .into()
}

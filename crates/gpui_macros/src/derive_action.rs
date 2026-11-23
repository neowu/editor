use proc_macro::TokenStream;
use proc_macro2::Ident;
use quote::quote;
use syn::Data;
use syn::DeriveInput;
use syn::LitStr;
use syn::Token;

use crate::register_action::generate_register_action;

pub(crate) fn derive_action(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);

    let struct_name = &input.ident;
    let mut name_argument = None;
    let mut no_build = false;
    let mut no_register = false;
    let mut namespace = None;

    for attr in &input.attrs {
        if attr.path().is_ident("action") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    if name_argument.is_some() {
                        return Err(meta.error("'name' argument specified multiple times"));
                    }
                    meta.input.parse::<Token![=]>()?;
                    let lit: LitStr = meta.input.parse()?;
                    name_argument = Some(lit.value());
                } else if meta.path.is_ident("namespace") {
                    if namespace.is_some() {
                        return Err(meta.error("'namespace' argument specified multiple times"));
                    }
                    meta.input.parse::<Token![=]>()?;
                    let ident: Ident = meta.input.parse()?;
                    namespace = Some(ident.to_string());
                } else if meta.path.is_ident("no_build") {
                    if no_build {
                        return Err(meta.error("'no_build' argument specified multiple times"));
                    }
                    no_build = true;
                } else if meta.path.is_ident("no_register") {
                    if no_register {
                        return Err(meta.error("'no_register' argument specified multiple times"));
                    }
                    no_register = true;
                } else {
                    return Err(meta.error(format!(
                        "'{:?}' argument not recognized, expected \
                        'name', 'namespace', 'no_build', or 'no_register'",
                        meta.path
                    )));
                }
                Ok(())
            })
            .unwrap_or_else(|e| panic!("in #[action] attribute: {e}"));
        }
    }

    let name = name_argument.unwrap_or_else(|| struct_name.to_string());

    assert!(
        !name.contains("::"),
        "in #[action] attribute: `name = \"{name}\"` must not contain `::`, \
        also specify `namespace` instead"
    );

    let full_name = if let Some(namespace) = namespace { format!("{namespace}::{name}") } else { name };

    let is_unit_struct = matches!(&input.data, Data::Struct(data) if data.fields.is_empty());

    let build = if no_build {
        quote! { None }
    } else if is_unit_struct {
        quote! { Some(|| Box::new(#struct_name)) }
    } else {
        quote! { Some(|| Box::new(<#struct_name as ::std::default::Default>::default())) }
    };

    let registration = if no_register {
        quote! {}
    } else {
        generate_register_action(struct_name, &full_name, &build)
    };

    TokenStream::from(quote! {
        #registration

        impl gpui::Action for #struct_name {
            fn name(&self) -> &'static str {
                #full_name
            }

            fn name_for_type() -> &'static str
            where
                Self: Sized
            {
                #full_name
            }

            fn partial_eq(&self, action: &dyn gpui::Action) -> bool {
                action
                    .as_any()
                    .downcast_ref::<Self>()
                    .map_or(false, |a| self == a)
            }

            fn boxed_clone(&self) -> Box<dyn gpui::Action> {
                Box::new(self.clone())
            }
        }
    })
}

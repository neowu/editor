use proc_macro2::Ident;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

pub(crate) fn generate_register_action(type_name: &Ident, name: &str, build: &TokenStream2) -> TokenStream2 {
    quote! {
        gpui::private::inventory::submit! {
            gpui::MacroActionData {
                name: #name,
                type_id: ::std::any::TypeId::of::<#type_name>,
                build: #build,
            }
        }
    }
}

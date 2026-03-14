use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
  Data, Fields, Generics, Ident, ItemStruct, Token,
  parse::{Parse, ParseStream},
  parse_macro_input, parse_quote,
  punctuated::Punctuated,
  token::Token,
};

struct ModelArgs {
  meta_name: Ident,
  meta_fields: Vec<Ident>,
  derive_model: Vec<Ident>,
  derive_metadata: Vec<Ident>,
}

impl Parse for ModelArgs {
  fn parse(input: ParseStream) -> syn::Result<Self> {
    let mut meta_name = None;
    let mut meta_fields = Vec::new();
    let mut derive_model = Vec::new();
    let mut derive_metadata = Vec::new();

    while !input.is_empty() {
      let ident: Ident = input.parse()?;

      if ident == "meta_name" {
        input.parse::<Token![=]>()?;
        meta_name = Some(input.parse()?);
      } else if ident == "meta_fields" {
        let content;
        syn::parenthesized!(content in input);

        let fields: Punctuated<Ident, Token![,]> =
          content.parse_terminated(Ident::parse, Token![,])?;

        meta_fields = fields.into_iter().collect();
      } else if ident == "derive_model" {
        let content;
        syn::parenthesized!(content in input);

        let derives: Punctuated<Ident, Token![,]> =
          content.parse_terminated(Ident::parse, Token![,])?;

        derive_model = derives.into_iter().collect();
      } else if ident == "derive_metadata" {
        let content;
        syn::parenthesized!(content in input);

        let derives: Punctuated<Ident, Token![,]> =
          content.parse_terminated(Ident::parse, Token![,])?;

        derive_metadata = derives.into_iter().collect();
      }

      if input.peek(Token![,]) {
        input.parse::<Token![,]>()?;
      }
    }

    Ok(ModelArgs {
      meta_name: meta_name.expect("meta_name required"),
      meta_fields,
      derive_model,
      derive_metadata,
    })
  }
}

#[proc_macro_attribute]
pub fn model(attr: TokenStream, item: TokenStream) -> TokenStream {
  let args = parse_macro_input!(attr as ModelArgs);
  let input = parse_macro_input!(item as ItemStruct);
  let input2 = input.clone();

  let struct_name = input.ident;
  let metadata_name = format_ident!("{}Metadata", struct_name);

  let mut generics = input.generics.clone();
  if generics.params.is_empty() {
    generics.params.insert(0, parse_quote!('a));
  }
  let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

  let Fields::Named(fields) = input2.clone().fields else {
    panic!("model requires named fields");
  };

  let mut metadata_fields = Vec::new();
  let mut runtime_fields = Vec::new();

  for field in fields.named {
    let ident = field.ident.clone().unwrap();

    if args.meta_fields.iter().any(|f| f == &ident) {
      metadata_fields.push(field);
    } else {
      runtime_fields.push(field);
    }
  }

  let meta_name = args.meta_name;
  let model_derives = args.derive_model;
  let metadata_derives = args.derive_metadata;

  let expanded = quote! {

      #[derive(::serde::Deserialize, #( #metadata_derives ), *)]
      pub struct #metadata_name {
          #( #metadata_fields, )*
      }

      impl NamedItem for #metadata_name {
        fn name(&self) -> &str {
          &self.#meta_name
        }
      }

      #[derive(#( #model_derives ), *)]
      pub struct #struct_name #generics {
          pub metadata: &'a #metadata_name,
          #( #runtime_fields, )*
      }

      impl #impl_generics Model for #struct_name #ty_generics #where_clause {
          type M = #metadata_name;
      }

  };

  expanded.into()
}

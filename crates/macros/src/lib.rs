use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
  Attribute, Fields, FnArg, Ident, ImplItem, ImplItemFn, ItemImpl, ItemStruct, LitStr, Pat,
  PatIdent, Result, Token, Type,
  parse::{Parse, ParseStream},
  parse_macro_input,
  punctuated::Punctuated,
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

  let generics = input.generics.clone();
  // if generics.params.is_empty() {
  //   generics.params.insert(0, parse_quote!('a));
  // }
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
          pub metadata: ::std::sync::Arc<#metadata_name>,
          #( #runtime_fields, )*
      }

      impl #impl_generics Model for #struct_name #ty_generics #where_clause {
          type M = #metadata_name;
      }

  };

  expanded.into()
}

#[proc_macro_attribute]
pub fn runtime(attr: TokenStream, item: TokenStream) -> TokenStream {
  let runtime_id = parse_macro_input!(attr as LitStr);
  let input = parse_macro_input!(item as ItemImpl);

  match expand_runtime(runtime_id, input) {
    Ok(tokens) => tokens.into(),
    Err(err) => err.to_compile_error().into(),
  }
}

fn expand_runtime(runtime_id: LitStr, input: ItemImpl) -> Result<proc_macro2::TokenStream> {
  let self_ty = input.self_ty.clone();
  let self_ident = match self_ty.as_ref() {
    Type::Path(tp) => tp
      .path
      .segments
      .last()
      .map(|s| s.ident.clone())
      .ok_or_else(|| syn::Error::new_spanned(&self_ty, "unsupported self type"))?,
    _ => return Err(syn::Error::new_spanned(&self_ty, "unsupported self type")),
  };
  let typed_mod_ident = format_ident!("{}Actions", self_ident);

  let mut helper_fns = Vec::new();
  let mut match_arms = Vec::new();
  let mut typed_action_mods = Vec::new();
  let mut typed_action_methods = Vec::new();

  for item in &input.items {
    let ImplItem::Fn(func) = item else {
      continue;
    };

    let manual_return = match &func.sig.output {
      syn::ReturnType::Default => false,
      syn::ReturnType::Type(_, _) => true,
    };

    let action_name = func.sig.ident.to_string();
    let helper_name = format_ident!("__runtime_{}", func.sig.ident);

    let mut params = parse_params(func)?;
    let action_cfg = parse_action(&func.attrs)?;
    apply_action_mode_overrides(&mut params, &action_cfg)?;
    let dispatch_action_name = action_cfg
      .rename
      .clone()
      .unwrap_or_else(|| action_name.clone());
    let extraction = generate_param_extraction(&params);

    let body = &func.block;

    let singleton_wrapper = parse_singletons(&func.attrs)?;

    let helper_body = if let Some(singletons) = singleton_wrapper {
      let types = singletons.iter().map(|(ty, _)| quote! { &mut #ty });

      let keys = singletons.iter().map(|(ty, _)| {
        quote! {
          #ty::KEY.into()
        }
      });

      let vars = singletons.iter().map(|(_, ident)| ident);

      quote! {
        ctx.registry.singleton_handle::<
          (#(#types),*),
          ()
        >(
          (#(#keys),*),
          |_registry, (#(#vars),*)| {
            #extraction
            #body
            Ok(())
          }
        )?;
      }
    } else {
      if manual_return {
        let body = body.stmts.clone();
        quote! {
          #extraction
          #(#body)*
          Ok(())
        }
      } else {
        quote! {
          #extraction
          #body
          Ok(None)
        }
      }
    };

    helper_fns.push(quote! {
      fn #helper_name(
        &mut self,
        mut payload: ::rind_core::prelude::RuntimePayload,
        ctx: &mut ::rind_core::prelude::RuntimeContext<'_>,
        dispatch: &::rind_core::prelude::RuntimeDispatcher,
        log: &::rind_core::prelude::LogHandle,
      ) -> ::rind_core::prelude::CoreResult<Option<::rind_core::prelude::RuntimePayload>> {
        #helper_body
      }
    });

    match_arms.push(quote! {
      #dispatch_action_name => {
        self.#helper_name(
          payload,
          ctx,
          dispatch,
          log,
        )?;
      }
    });

    let action_mod_ident = format_ident!("{}", dispatch_action_name);
    let required_params = params
      .iter()
      .filter(|p| matches!(p.mode, ParamMode::Required))
      .collect::<Vec<_>>();
    let optional_params = params
      .iter()
      .filter(|p| !matches!(p.mode, ParamMode::Required))
      .collect::<Vec<_>>();

    let required_decl = required_params.iter().map(|p| {
      let ident = &p.ident;
      let ty = &p.ty;
      quote! { #ident: #ty }
    });
    let optional_decl = optional_params.iter().map(|p| {
      let ident = &p.ident;
      let ty = &p.ty;
      quote! { #ident: ::std::option::Option<#ty> }
    });

    let new_args = required_params.iter().map(|p| {
      let ident = &p.ident;
      let ty = &p.ty;
      quote! { #ident: #ty }
    });
    let required_inits = required_params.iter().map(|p| {
      let ident = &p.ident;
      quote! { #ident }
    });
    let optional_none_inits = optional_params.iter().map(|p| {
      let ident = &p.ident;
      quote! { #ident: ::std::option::Option::None }
    });

    let optional_setters = optional_params.iter().map(|p| {
      let ident = &p.ident;
      let ty = &p.ty;
      quote! {
        pub fn #ident(mut self, value: #ty) -> Self {
          self.#ident = ::std::option::Option::Some(value);
          self
        }
      }
    });

    let insert_required = required_params.iter().map(|p| {
      let ident = &p.ident;
      let key = ident.to_string();
      quote! { p = p.insert(#key, self.#ident); }
    });
    let insert_optional = optional_params.iter().map(|p| {
      let ident = &p.ident;
      let key = ident.to_string();
      quote! {
        if let ::std::option::Option::Some(v) = self.#ident {
          p = p.insert(#key, v);
        }
      }
    });

    let root_ctor_name = format_ident!("{}", dispatch_action_name);
    let root_ctor_args = required_params
      .iter()
      .map(|p| {
        let ident = &p.ident;
        let ty = &p.ty;
        quote! { #ident: #ty }
      })
      .collect::<Vec<_>>();
    let root_ctor_pass = required_params
      .iter()
      .map(|p| {
        let ident = &p.ident;
        quote! { #ident }
      })
      .collect::<Vec<_>>();

    typed_action_methods.push(quote! {
      pub fn #root_ctor_name(&self, #(#root_ctor_args),*) -> #action_mod_ident::Payload {
        #action_mod_ident::Payload::new(#(#root_ctor_pass),*)
      }
    });

    typed_action_mods.push(quote! {
      pub mod #action_mod_ident {
        use super::*;

        pub struct Action;

        pub struct Payload {
          #(#required_decl,)*
          #(#optional_decl,)*
        }

        impl Payload {
          pub fn new(#(#new_args),*) -> Self {
            Self {
              #(#required_inits,)*
              #(#optional_none_inits,)*
            }
          }

          #(#optional_setters)*

          pub fn dispatch(self, dispatch: &::rind_core::prelude::RuntimeDispatcher) -> ::rind_core::prelude::CoreResult<()>  {
            dispatch.dispatch_typed::<#action_mod_ident::Action>(self)
          }

          pub fn orchestrate(self, ctx: &mut ::rind_core::prelude::OrchestratorContext<'_>) -> ::rind_core::prelude::CoreResult<()>  {
            ctx.dispatch_typed::<#action_mod_ident::Action>(self)
          }
        }

        impl ::std::convert::Into<::rind_core::prelude::RuntimePayload> for Payload {
          fn into(self) -> ::rind_core::prelude::RuntimePayload {
            let mut p = ::rind_core::prelude::RuntimePayload::default();
            #(#insert_required)*
            #(#insert_optional)*
            p
          }
        }

        impl ::rind_core::prelude::RuntimeActionSpec for Action {
          const RUNTIME: &'static str = #runtime_id;
          const ACTION: &'static str = #dispatch_action_name;
          type Payload = Payload;
        }

      }
    });
  }

  let expanded = quote! {
      #[allow(non_snake_case)]
      pub mod #typed_mod_ident {
        use super::*;

        pub struct Root;

        impl Root {
          #(#typed_action_methods)*
        }

        #(#typed_action_mods)*
      }

      impl #self_ty {
        #[allow(non_upper_case_globals)]
        pub const actions: #typed_mod_ident::Root = #typed_mod_ident::Root;

        #(#helper_fns)*
      }

      impl ::rind_core::prelude::Runtime for #self_ty {
        fn id(&self) -> &str {
          #runtime_id
        }

        fn handle(
          &mut self,
          action: &str,
          payload: ::rind_core::prelude::RuntimePayload,
          ctx: &mut ::rind_core::prelude::RuntimeContext<'_>,
          dispatch: &::rind_core::prelude::RuntimeDispatcher,
          log: &::rind_core::prelude::LogHandle,
        ) -> ::rind_core::prelude::CoreResult<Option<::rind_core::prelude::RuntimePayload>> {
          match action {
            #(#match_arms,)*
            _ => {}
          }

          Ok(None)
        }
      }
  };

  Ok(expanded)
}

struct ActionArgs {
  required: Vec<Ident>,
  optional: Vec<Ident>,
  rename: Option<String>,
}

impl Default for ActionArgs {
  fn default() -> Self {
    Self {
      required: Vec::new(),
      optional: Vec::new(),
      rename: None,
    }
  }
}

#[derive(Clone)]
enum ParamMode {
  Required,
  Optional,
  Default,
}

#[derive(Clone)]
struct RuntimeParam {
  ident: Ident,
  ty: Type,
  mode: ParamMode,
}

fn parse_params(func: &ImplItemFn) -> Result<Vec<RuntimeParam>> {
  let mut out = Vec::new();

  for arg in &func.sig.inputs {
    let FnArg::Typed(typed) = arg else {
      continue;
    };

    let ident = match &*typed.pat {
      Pat::Ident(PatIdent { ident, .. }) => ident.clone(),
      _ => {
        return Err(syn::Error::new_spanned(
          &typed.pat,
          "unsupported parameter pattern",
        ));
      }
    };

    let ty = (*typed.ty).clone();

    let mut mode = ParamMode::Required;

    for attr in &typed.attrs {
      if attr.path().is_ident("optional") {
        mode = ParamMode::Optional;
      }

      if attr.path().is_ident("default") {
        mode = ParamMode::Default;
      }
    }

    out.push(RuntimeParam { ident, ty, mode });
  }

  Ok(out)
}

fn parse_action(attrs: &[Attribute]) -> Result<ActionArgs> {
  let mut args = ActionArgs::default();
  for attr in attrs {
    if !attr.path().is_ident("action") {
      continue;
    }

    match &attr.meta {
      syn::Meta::Path(_) => {}
      syn::Meta::List(list) => {
        if list.tokens.is_empty() {
          continue;
        }

        if let Ok(rename) = syn::parse2::<Ident>(list.tokens.clone()) {
          args.rename = Some(rename.to_string());
          continue;
        }

        if let Ok(rename) = syn::parse2::<LitStr>(list.tokens.clone()) {
          args.rename = Some(rename.value());
          continue;
        }

        attr.parse_nested_meta(|meta| {
          if meta.path.is_ident("rename") {
            let value = meta.value()?;
            let lit: LitStr = value.parse()?;
            args.rename = Some(lit.value());
            return Ok(());
          }

          if let Some(ident) = meta.path.get_ident() {
            args.rename = Some(ident.to_string());
            return Ok(());
          }

          Err(meta.error("unsupported #[action(...)] entry"))
        })?;
      }
      syn::Meta::NameValue(_) => {
        return Err(syn::Error::new_spanned(
          attr,
          "unsupported #[action = ...] form",
        ));
      }
    }
  }
  Ok(args)
}

fn apply_action_mode_overrides(params: &mut [RuntimeParam], args: &ActionArgs) -> Result<()> {
  for req in &args.required {
    let Some(param) = params.iter_mut().find(|p| p.ident == *req) else {
      return Err(syn::Error::new_spanned(
        req,
        "unknown required parameter in #[action(...)]",
      ));
    };
    param.mode = ParamMode::Required;
  }

  for opt in &args.optional {
    let Some(param) = params.iter_mut().find(|p| p.ident == *opt) else {
      return Err(syn::Error::new_spanned(
        opt,
        "unknown optional parameter in #[action(...)]",
      ));
    };
    param.mode = ParamMode::Optional;
  }

  Ok(())
}

fn generate_param_extraction(params: &[RuntimeParam]) -> proc_macro2::TokenStream {
  let extractions = params.iter().map(|param| {
    let ident = &param.ident;
    let ty = &param.ty;

    let key = ident.to_string();

    match param.mode {
      ParamMode::Required => {
        quote! {
            let mut #ident =
                payload.get::<#ty>(#key)?;
        }
      }

      ParamMode::Optional => {
        quote! {
            let mut #ident =
                payload.get::<#ty>(#key).ok();
        }
      }

      ParamMode::Default => {
        quote! {
            let mut #ident =
                payload
                    .get::<#ty>(#key)
                    .unwrap_or_default();
        }
      }
    }
  });

  quote! {
      #(#extractions)*
  }
}

fn parse_singletons(attrs: &[Attribute]) -> Result<Option<Vec<(Type, Ident)>>> {
  for attr in attrs {
    if !attr.path().is_ident("within_singletons") {
      continue;
    }

    let parsed = attr.parse_args_with(Punctuated::<SingletonEntry, Token![,]>::parse_terminated)?;

    let out = parsed.into_iter().map(|e| (e.ty, e.ident)).collect();

    return Ok(Some(out));
  }

  Ok(None)
}

struct SingletonEntry {
  ty: Type,
  ident: Ident,
}

impl syn::parse::Parse for SingletonEntry {
  fn parse(input: syn::parse::ParseStream) -> Result<Self> {
    let ty = input.parse::<Type>()?;

    input.parse::<Token![=]>()?;

    let ident = input.parse::<Ident>()?;

    Ok(Self { ty, ident })
  }
}

//! Proc macros for `spel04-shim` (spike). Accepts exactly the attribute syntax that
//! SPEL v0.7.0's `generate-idl` reads, and emits a LEZ 0.4 actor dispatcher.
//!
//! `#[lez_program]` on a module:
//!   * strips `#[instruction]`, `#[account(..)]` and `#[effect_handler]` (rustc would
//!     reject unknown attributes on fn parameters, so only the outer macro makes this compile);
//!   * generates `pub enum Instruction` from the `#[instruction]` fns: variants in fn order
//!     (= SPEL IDL instruction order = wire tag), fields = non-account args in fn order
//!     (= SPEL IDL args order = borsh order), so the wire format cannot drift from the IDL;
//!   * generates `__lez_dispatch(&ReceiveInput) -> Response` with the root-only and
//!     from-own-program guards, actor resolution from PDA seeds, and RequireSigner probes;
//!   * generates host-side `actors_to_declare(root, program, &Instruction) -> Vec<Actor>`.
//!
//! Account parameter rules (0.4 actor model):
//!   * the first account parameter is the root actor `(receiver, self)`;
//!   * `pda = ..` accounts are derived by SPEL's compute_pda rule under `self`;
//!   * any other account `foo` needs an arg `foo_id: [u8; 32]` (the guest only sees its receiver).

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TS2;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Attribute, Expr, FnArg, Ident, Item, ItemMod, Pat, Type};

#[proc_macro_attribute]
pub fn account_type(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item = TS2::from(item);
    quote!(
        #[derive(::borsh::BorshSerialize, ::borsh::BorshDeserialize, Clone, Debug, PartialEq, Eq)]
        #item
    )
    .into()
}

enum Seed {
    Const(String),
    Account(String),
    Arg(String),
}

struct Acc {
    name: Ident,
    signer: bool,
    seeds: Vec<Seed>,
}

struct Arg {
    name: Ident,
    ty: Type,
}

struct Ix {
    fn_name: Ident,
    params: Vec<P>,
    accs: Vec<Acc>,
    args: Vec<Arg>,
}

enum P {
    Acc(usize),
    Arg(usize),
}

fn is_ident_attr(a: &Attribute, name: &str) -> bool {
    a.path().is_ident(name)
}

fn is_account_ty(ty: &Type) -> bool {
    matches!(ty, Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "AccountWithMetadata"))
}

fn parse_seed(e: &Expr) -> syn::Result<Seed> {
    let Expr::Call(c) = e else { return Err(syn::Error::new_spanned(e, "seed must be literal(..)/const(..)/account(..)/arg(..)")) };
    let f = match &*c.func {
        Expr::Path(p) => p.path.get_ident().map(|i| i.to_string()).unwrap_or_default(),
        _ => String::new(),
    };
    let s = match c.args.first() {
        Some(Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. })) if c.args.len() == 1 => s.value(),
        _ => return Err(syn::Error::new_spanned(c, "seed takes one string literal")),
    };
    Ok(match f.as_str() {
        "literal" | "const" | "r#const" | "seed_const" => Seed::Const(s),
        "account" => Seed::Account(s),
        "arg" => Seed::Arg(s),
        _ => return Err(syn::Error::new_spanned(c, "unknown seed kind")),
    })
}

fn parse_account_attrs(attrs: &[Attribute], name: Ident) -> syn::Result<Acc> {
    let mut acc = Acc { name, signer: false, seeds: vec![] };
    for a in attrs.iter().filter(|a| is_ident_attr(a, "account")) {
        a.parse_nested_meta(|m| {
            if m.path.is_ident("mut") || m.path.is_ident("init") {
                Ok(())
            } else if m.path.is_ident("signer") {
                acc.signer = true;
                Ok(())
            } else if m.path.is_ident("pda") {
                let e: Expr = m.value()?.parse()?;
                acc.seeds = match &e {
                    Expr::Array(arr) => arr.elems.iter().map(parse_seed).collect::<syn::Result<_>>()?,
                    other => vec![parse_seed(other)?],
                };
                Ok(())
            } else {
                Err(m.error("unknown account constraint"))
            }
        })?;
    }
    Ok(acc)
}

fn camel(id: &Ident) -> Ident {
    let s: String = id
        .to_string()
        .split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect();
    format_ident!("{}", s)
}

#[proc_macro_attribute]
pub fn lez_program(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut module = parse_macro_input!(item as ItemMod);
    let mut enum_name = format_ident!("Instruction");
    let attr_parser = syn::meta::parser(|m| {
        if m.path.is_ident("instruction") {
            let s: syn::LitStr = m.value()?.parse()?;
            enum_name = format_ident!("{}", s.value());
            Ok(())
        } else {
            Err(m.error("unsupported lez_program option"))
        }
    });
    parse_macro_input!(attr with attr_parser);
    match expand(&mut module, &enum_name) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// The shim's crate as the program names it (it is normally aliased, e.g. `spel = { package = "spel04-shim" }`).
fn shim_path() -> TS2 {
    use proc_macro_crate::{crate_name, FoundCrate};
    match crate_name("spel04-shim") {
        Ok(FoundCrate::Name(n)) => {
            let id = format_ident!("{}", n);
            quote!(::#id)
        }
        _ => quote!(::spel04_shim),
    }
}

fn expand(module: &mut ItemMod, enum_name: &Ident) -> syn::Result<TS2> {
    let shim = shim_path();
    let mod_name = module.ident.clone();
    let Some((_, items)) = module.content.as_mut() else {
        return Err(syn::Error::new_spanned(&module.ident, "#[lez_program] needs an inline module"));
    };
    let mut ixs: Vec<Ix> = vec![];
    let mut effect_fn: Option<Ident> = None;
    for item in items.iter_mut() {
        let Item::Fn(f) = item else { continue };
        if f.attrs.iter().any(|a| is_ident_attr(a, "effect_handler")) {
            f.attrs.retain(|a| !is_ident_attr(a, "effect_handler"));
            effect_fn = Some(f.sig.ident.clone());
            continue;
        }
        if !f.attrs.iter().any(|a| is_ident_attr(a, "instruction")) {
            continue;
        }
        f.attrs.retain(|a| !is_ident_attr(a, "instruction"));
        let mut ix = Ix { fn_name: f.sig.ident.clone(), params: vec![], accs: vec![], args: vec![] };
        for input in f.sig.inputs.iter_mut() {
            let FnArg::Typed(pt) = input else { return Err(syn::Error::new_spanned(input, "no self")) };
            let Pat::Ident(pi) = &*pt.pat else { return Err(syn::Error::new_spanned(&pt.pat, "simple names only")) };
            let name = pi.ident.clone();
            if is_account_ty(&pt.ty) {
                let acc = parse_account_attrs(&pt.attrs, name)?;
                pt.attrs.retain(|a| !is_ident_attr(a, "account"));
                ix.params.push(P::Acc(ix.accs.len()));
                ix.accs.push(acc);
            } else {
                ix.params.push(P::Arg(ix.args.len()));
                ix.args.push(Arg { name, ty: (*pt.ty).clone() });
            }
        }
        if ix.accs.is_empty() {
            return Err(syn::Error::new_spanned(&f.sig, "an instruction needs a root account parameter"));
        }
        ixs.push(ix);
    }

    // enum Instruction (variants in fn order; fields = args in order)
    let variants = ixs.iter().map(|ix| {
        let v = camel(&ix.fn_name);
        let fields = ix.args.iter().map(|a| {
            let (n, t) = (&a.name, &a.ty);
            quote!(#n: #t)
        });
        quote!(#v { #(#fields),* })
    });

    let mut arms = vec![];
    let mut declare_arms = vec![];
    for ix in &ixs {
        let v = camel(&ix.fn_name);
        let fname = &ix.fn_name;
        let arg_names: Vec<&Ident> = ix.args.iter().map(|a| &a.name).collect();
        let mut resolve = vec![];
        let mut resolve_host = vec![];
        let mut probes = vec![];
        let root = &ix.accs[0].name;
        for (i, acc) in ix.accs.iter().enumerate() {
            let n = &acc.name;
            if i == 0 {
                resolve.push(quote!(let #n = #shim::AccountWithMetadata::__root(input);));
                resolve_host.push(quote!(let #n = #shim::AccountWithMetadata::__at(__root_id, __program);));
                if acc.signer {
                    let msg = format!("`{n}` (root) must sign");
                    resolve.push(quote!(assert!(#n.is_authorized, #msg);));
                }
                continue;
            }
            if acc.seeds.is_empty() {
                let id_arg = format_ident!("{}_id", n);
                if !ix.args.iter().any(|a| a.name == id_arg) {
                    return Err(syn::Error::new_spanned(
                        n,
                        format!("account `{n}` is neither the root nor a PDA: add an arg `{id_arg}: [u8; 32]`"),
                    ));
                }
                let r = quote!(let #n = #shim::AccountWithMetadata::__at(
                    #shim::lee_core::account::AccountId::new(#id_arg), __program););
                resolve.push(r.clone());
                resolve_host.push(r);
                if acc.signer {
                    probes.push(quote!(
                        if #n.account_id != #root.account_id {
                            __r = __r.send(#shim::__require_signer(&#n));
                        }
                    ));
                }
            } else {
                let parts = acc
                    .seeds
                    .iter()
                    .map(|s| match s {
                        Seed::Const(c) => quote!(#shim::seed_from_str(#c)),
                        Seed::Account(a) => {
                            let a = format_ident!("{}", a);
                            quote!(#shim::ToSeed::to_seed(&#a.account_id))
                        }
                        Seed::Arg(a) => {
                            let a = format_ident!("{}", a);
                            quote!(#shim::ToSeed::to_seed(&#a))
                        }
                    })
                    .collect::<Vec<_>>();
                let r = quote!(let #n = #shim::AccountWithMetadata::__pda(__program, &[#(#parts),*]););
                resolve.push(r.clone());
                resolve_host.push(r);
            }
        }
        let call_args = ix.params.iter().map(|p| match p {
            P::Acc(i) => {
                let n = &ix.accs[*i].name;
                quote!(#n.clone())
            }
            P::Arg(i) => {
                let n = &ix.args[*i].name;
                quote!(#n.clone())
            }
        });
        let acc_names: Vec<&Ident> = ix.accs.iter().map(|a| &a.name).collect();
        arms.push(quote!(
            #enum_name::#v { #(#arg_names),* } => {
                #(#resolve)*
                let mut __r = #mod_name::#fname(#(#call_args),*);
                #(#probes)*
                __r
            }
        ));
        declare_arms.push(quote!(
            #enum_name::#v { #(#arg_names),* } => {
                #(#resolve_host)*
                let mut v: ::std::vec::Vec<#shim::lee_core::account::Actor> = vec![];
                for a in [#(#acc_names.actor()),*] { if !v.contains(&a) { v.push(a); } }
                v
            }
        ));
    }

    let effect_branch = match &effect_fn {
        Some(f) => quote!(#mod_name::#f(input, &#shim::unshift(data))),
        None => quote!(panic!("this program has no effects")),
    };

    Ok(quote!(
        #module

        /// Wire format of a root message: borsh enum (u8 variant + args), generated from the
        /// `#[instruction]` fns in declaration order.
        #[derive(::borsh::BorshSerialize, ::borsh::BorshDeserialize, Clone, Debug)]
        pub enum #enum_name { #(#variants),* }

        /// LEZ 0.4 guest entry: `lee_core::actor_entrypoint!(raw <crate>::__lez_dispatch);`
        #[allow(unused_variables, unused_mut, clippy::redundant_clone)]
        pub fn __lez_dispatch(
            input: &#shim::lee_core::program::ReceiveInput,
        ) -> #shim::lee_core::program::Response {
            let data: &[u8] = &input.message;
            let tag = *data.first().expect("empty message");
            if tag == #shim::REQUIRE_SIGNER_TAG {
                return #shim::__require_signer_handler(input);
            }
            if tag >= #shim::EFFECT_TAG_MIN {
                assert!(input.from_own_program(), "only this program may send an effect");
                return #effect_branch;
            }
            assert!(input.from.is_none(), "instructions must be the transaction's root message");
            let __program = input.receiver.program_account_id;
            let ix: #enum_name = ::borsh::from_slice(data).expect("instruction must decode");
            match ix { #(#arms)* }
        }

        /// Host side: every actor a client must declare for `ix` (root first, deduplicated).
        #[allow(unused_variables)]
        pub fn actors_to_declare(
            __root_id: #shim::lee_core::account::AccountId,
            __program: #shim::lee_core::account::AccountId,
            ix: &#enum_name,
        ) -> ::std::vec::Vec<#shim::lee_core::account::Actor> {
            match ix.clone() { #(#declare_arms)* }
        }
    ))
}

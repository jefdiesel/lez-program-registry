//! Proc macros for `spel04-shim`. They accept exactly the attribute syntax that SPEL v0.7.0's
//! `generate-idl` reads and emit a LEZ 0.4 actor dispatcher. See the runtime crate's docs for
//! the 0.4 conventions.
//!
//! `#[lez_program]` on an inline module:
//! - strips `#[instruction]`, `#[account(..)]` and `#[effect]` (rustc rejects unknown
//!   attributes on fn parameters, so only the outer macro makes this syntax compile);
//! - generates `pub enum Instruction` from the `#[instruction]` fns: variants in fn order
//!   (= SPEL IDL order = wire tag), fields = non-account args in order (= IDL args = borsh order);
//! - generates `pub enum Effect` from the `#[effect]` fns (wire tags `0xC0 + i`) and
//!   `pub enum Message` with a hand-written borsh codec over both;
//! - generates `__lez_dispatch(&ReceiveInput) -> Response` with the root-only / PDA-header
//!   guard, `from_own_program` for effects, root `signer` and `init` checks, PDA actor
//!   resolution, `RequireSigner` probes and the timestamp window;
//! - generates host-side `actors_to_declare(root, program, &Instruction) -> Vec<Actor>`;
//! - checks the instruction and effect order against `wire-tags.lock` (append-only).

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TS2};
use quote::{format_ident, quote};
use syn::{Attribute, Expr, FnArg, Ident, Item, ItemMod, Meta, Pat, Type, parse_macro_input};

/// Borsh and the usual derives for an on-chain record. The released SPEL generator lists every
/// `#[account_type]` struct under the IDL's `accounts[]`.
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
    init: bool,
    seeds: Vec<Seed>,
    /// `owner = loader("x")`: the program loader's actor at account `x`.
    loader_of: Option<String>,
}

impl Acc {
    fn by_id(&self) -> bool {
        self.seeds.is_empty() && self.loader_of.is_none()
    }
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

struct Eff {
    fn_name: Ident,
    init: bool,
    fields: Vec<Arg>,
}

fn err<T>(span: impl quote::ToTokens, msg: impl std::fmt::Display) -> syn::Result<T> {
    Err(syn::Error::new_spanned(span, msg))
}

fn is_ident_attr(a: &Attribute, name: &str) -> bool {
    a.path().is_ident(name)
}

fn last_ident(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

fn is_account_ty(ty: &Type) -> bool {
    last_ident(ty).as_deref() == Some("AccountWithMetadata")
}

fn is_bytes32(ty: &Type) -> bool {
    let Type::Array(a) = ty else { return false };
    let len32 = matches!(&a.len, Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(n), .. }) if n.base10_parse::<usize>().ok() == Some(32));
    len32 && last_ident(&a.elem).as_deref() == Some("u8")
}

fn generic_arg(ty: &Type) -> Option<&Type> {
    let Type::Path(p) = ty else { return None };
    let seg = p.path.segments.last()?;
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else { return None };
    match args.args.first()? {
        syn::GenericArgument::Type(t) => Some(t),
        _ => None,
    }
}

fn is_opt_bytes32(ty: &Type) -> bool {
    last_ident(ty).as_deref() == Some("Option") && generic_arg(ty).is_some_and(is_bytes32)
}

fn lit_str_call(e: &Expr) -> syn::Result<(String, String)> {
    let Expr::Call(c) = e else { return err(e, "expected f(\"..\")") };
    let f = match &*c.func {
        Expr::Path(p) => p.path.get_ident().map(ToString::to_string).unwrap_or_default(),
        _ => String::new(),
    };
    match c.args.first() {
        Some(Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. })) if c.args.len() == 1 => Ok((f, s.value())),
        _ => err(c, "takes exactly one string literal"),
    }
}

fn parse_seed(e: &Expr) -> syn::Result<Seed> {
    let (f, s) = lit_str_call(e)?;
    Ok(match f.as_str() {
        "literal" | "const" | "r#const" | "seed_const" => Seed::Const(s),
        "account" => Seed::Account(s),
        "arg" => Seed::Arg(s),
        _ => return err(e, "seed must be literal(..)/const(..)/account(..)/arg(..)"),
    })
}

fn parse_account_attrs(attrs: &[Attribute], name: Ident) -> syn::Result<Acc> {
    let mut acc = Acc { name, signer: false, init: false, seeds: vec![], loader_of: None };
    for a in attrs.iter().filter(|a| is_ident_attr(a, "account")) {
        a.parse_nested_meta(|m| {
            if m.path.is_ident("mut") {
                Ok(())
            } else if m.path.is_ident("init") {
                acc.init = true;
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
            } else if m.path.is_ident("owner") {
                let e: Expr = m.value()?.parse()?;
                match lit_str_call(&e)? {
                    (f, s) if f == "loader" => acc.loader_of = Some(s),
                    _ => return Err(m.error("spel04-shim supports only owner = loader(\"<account>\")")),
                }
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

/// The shim's crate as the program names it (normally aliased: `spel = { package = "spel04-shim" }`).
fn shim_path() -> TS2 {
    use proc_macro_crate::{FoundCrate, crate_name};
    match crate_name("spel04-shim") {
        Ok(FoundCrate::Name(n)) => {
            let id = format_ident!("{}", n);
            quote!(::#id)
        }
        _ => quote!(::spel04_shim),
    }
}

fn parse_instruction(f: &mut syn::ItemFn) -> syn::Result<Ix> {
    let mut ix = Ix { fn_name: f.sig.ident.clone(), params: vec![], accs: vec![], args: vec![] };
    for input in &mut f.sig.inputs {
        let FnArg::Typed(pt) = input else { return err(input, "instructions take no self") };
        let Pat::Ident(pi) = &*pt.pat else { return err(&pt.pat, "simple parameter names only") };
        let name = pi.ident.clone();
        if name.to_string().starts_with('_') {
            return err(&name, "the IDL strips a leading `_`; don't use one");
        }
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
        return err(&f.sig, "an instruction needs a root account parameter");
    }
    validate(&ix)?;
    Ok(ix)
}

fn validate(ix: &Ix) -> syn::Result<()> {
    let arg = |n: &str| ix.args.iter().find(|a| a.name == n);
    for (i, acc) in ix.accs.iter().enumerate() {
        let n = &acc.name;
        let earlier = |name: &str| ix.accs[..i].iter().any(|a| a.name == name);
        if i == 0 {
            if !acc.by_id() {
                return err(n, "the root account (first account parameter) cannot be a PDA or another program's actor");
            }
            continue;
        }
        if !acc.seeds.is_empty() {
            if acc.signer {
                return err(n, "a PDA cannot sign");
            }
            for s in &acc.seeds {
                match s {
                    Seed::Const(c) if c.len() > 32 => return err(n, format!("const seed '{c}' exceeds 32 bytes")),
                    Seed::Const(_) => {}
                    Seed::Account(a) if !earlier(a) => {
                        return err(n, format!("seed account(\"{a}\") must name an earlier account parameter"));
                    }
                    Seed::Account(_) => {}
                    Seed::Arg(a) => match arg(a) {
                        None => return err(n, format!("seed arg(\"{a}\") names no argument")),
                        Some(x) if last_ident(&x.ty).as_deref() == Some("String") || is_bytes32(&x.ty) => {}
                        Some(x) => {
                            return err(
                                &x.ty,
                                format!("arg seed `{a}` must be String or [u8; 32]: integer seeds derive differently in SPEL's ToSeed and its CLI resolver (F3)"),
                            );
                        }
                    },
                }
            }
        } else if let Some(of) = &acc.loader_of {
            if acc.signer {
                return err(n, "mark the account itself as signer, not its loader actor");
            }
            if !earlier(of) {
                return err(n, format!("loader(\"{of}\") must name an earlier account parameter"));
            }
        } else {
            let id = format!("{n}_id");
            match arg(&id) {
                Some(a) if is_bytes32(&a.ty) => {}
                _ => {
                    return err(
                        n,
                        format!("account `{n}` is neither the root nor a PDA: add an arg `{id}: [u8; 32]` (a 0.4 guest sees only its receiver)"),
                    );
                }
            }
        }
    }
    if let Some(a) = arg("via_pda_seed")
        && !is_opt_bytes32(&a.ty)
    {
        return err(&a.ty, "via_pda_seed must be Option<[u8; 32]>");
    }
    if let Some(a) = arg("now")
        && last_ident(&a.ty).as_deref() != Some("u64")
    {
        return err(&a.ty, "now must be u64 (milliseconds)");
    }
    Ok(())
}

fn parse_effect(f: &mut syn::ItemFn, init: bool) -> syn::Result<Eff> {
    let mut it = f.sig.inputs.iter();
    match it.next() {
        Some(FnArg::Typed(pt)) if matches!(&*pt.ty, Type::Reference(_)) => {}
        _ => return err(&f.sig, "an #[effect] fn takes `input: &ReceiveInput` first"),
    }
    let mut fields = vec![];
    for input in it {
        let FnArg::Typed(pt) = input else { return err(input, "no self") };
        let Pat::Ident(pi) = &*pt.pat else { return err(&pt.pat, "simple parameter names only") };
        fields.push(Arg { name: pi.ident.clone(), ty: (*pt.ty).clone() });
    }
    Ok(Eff { fn_name: f.sig.ident.clone(), init, fields })
}

fn effect_attr(attrs: &[Attribute]) -> syn::Result<Option<bool>> {
    let Some(a) = attrs.iter().find(|a| is_ident_attr(a, "effect")) else { return Ok(None) };
    let mut init = false;
    if let Meta::List(_) = &a.meta {
        a.parse_nested_meta(|m| {
            if m.path.is_ident("init") {
                init = true;
                Ok(())
            } else {
                Err(m.error("only #[effect] or #[effect(init)]"))
            }
        })?;
    }
    Ok(Some(init))
}

/// The committed tag order. Lines: `instruction <tag> <fn>` and `effect <tag> <fn>`.
fn wire_listing(ixs: &[Ix], effs: &[Eff]) -> (Vec<String>, Vec<String>) {
    let i = ixs.iter().enumerate().map(|(n, ix)| format!("instruction {n} {}", ix.fn_name)).collect();
    let e = effs.iter().enumerate().map(|(n, ef)| format!("effect {} {}", 0xC0 + n, ef.fn_name)).collect();
    (i, e)
}

fn check_lock(ixs: &[Ix], effs: &[Eff]) -> syn::Result<Option<String>> {
    let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") else { return Ok(None) };
    let path = std::path::Path::new(&dir).join("wire-tags.lock");
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(None) };
    let (gi, ge) = wire_listing(ixs, effs);
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    for (kind, generated) in [("instruction ", &gi), ("effect ", &ge)] {
        let locked: Vec<&str> = lines.iter().copied().filter(|l| l.starts_with(kind)).collect();
        if locked.len() > generated.len() || locked.iter().zip(generated.iter()).any(|(a, b)| *a != b) {
            return Err(syn::Error::new(
                Span::call_site(),
                format!(
                    "wire tags changed against wire-tags.lock: new {kind}fns may only be appended.\nlocked:\n  {}\ngenerated:\n  {}",
                    locked.join("\n  "),
                    generated.join("\n  ")
                ),
            ));
        }
    }
    Ok(Some(path.display().to_string()))
}

#[allow(clippy::too_many_lines)]
fn expand(module: &mut ItemMod, enum_name: &Ident) -> syn::Result<TS2> {
    let shim = shim_path();
    let mod_name = module.ident.clone();
    let Some((_, items)) = module.content.as_mut() else {
        return err(&module.ident, "#[lez_program] needs an inline module");
    };
    let mut ixs: Vec<Ix> = vec![];
    let mut effs: Vec<Eff> = vec![];
    for item in items.iter_mut() {
        let Item::Fn(f) = item else { continue };
        if let Some(init) = effect_attr(&f.attrs)? {
            f.attrs.retain(|a| !is_ident_attr(a, "effect"));
            effs.push(parse_effect(f, init)?);
            continue;
        }
        if !f.attrs.iter().any(|a| is_ident_attr(a, "instruction")) {
            continue;
        }
        f.attrs.retain(|a| !is_ident_attr(a, "instruction"));
        ixs.push(parse_instruction(f)?);
    }
    if ixs.is_empty() {
        return err(&module.ident, "no #[instruction] fns");
    }
    if ixs.len() >= 0xC0 || effs.len() >= usize::from(0xFF - 0xC0_u8) {
        return err(&module.ident, "too many instructions or effects for the tag space");
    }
    let lock_path = check_lock(&ixs, &effs)?;
    let (gi, ge) = wire_listing(&ixs, &effs);
    let wire_tags = format!("{}\n{}\n", gi.join("\n"), ge.join("\n"));
    let lock_track = lock_path.map(|p| quote!(const _: &str = include_str!(#p);));

    let loader = quote!(#shim::lee_core::program::PROGRAM_LOADER_ACCOUNT_ID);
    let awm = quote!(#shim::AccountWithMetadata);

    // enum Instruction
    let variants = ixs.iter().map(|ix| {
        let v = camel(&ix.fn_name);
        let fields = ix.args.iter().map(|a| {
            let (n, t) = (&a.name, &a.ty);
            quote!(#n: #t)
        });
        quote!(#v { #(#fields),* })
    });
    let ix_names: Vec<String> = ixs.iter().map(|ix| ix.fn_name.to_string()).collect();
    let name_arms = ixs.iter().map(|ix| {
        let v = camel(&ix.fn_name);
        let s = ix.fn_name.to_string();
        quote!(#enum_name::#v { .. } => #s)
    });

    let mut arms = vec![];
    let mut declare_arms = vec![];
    for ix in &ixs {
        let v = camel(&ix.fn_name);
        let fname = &ix.fn_name;
        let arg_names: Vec<&Ident> = ix.args.iter().map(|a| &a.name).collect();
        let has = |n: &str| ix.args.iter().any(|a| a.name == n);
        let root = &ix.accs[0].name;
        let mut resolve = vec![];
        let mut resolve_host = vec![];
        let mut probes_pre = vec![];
        let mut probes_post = vec![];
        for (i, acc) in ix.accs.iter().enumerate() {
            let n = &acc.name;
            if i == 0 {
                resolve.push(quote!(let #n = #awm::__root(input);));
                resolve_host.push(quote!(let #n = #awm::at(__root_id, __program);));
                if acc.signer {
                    let msg = format!("{fname}: `{n}` (the root account) must sign");
                    resolve.push(quote!(assert!(#n.is_authorized, #msg);));
                }
                if acc.init {
                    let msg = format!("{fname}: `{n}` already exists");
                    resolve.push(quote!(assert!(#n.pre_state().is_empty(), #msg);));
                }
                continue;
            }
            let r = if !acc.seeds.is_empty() {
                let parts = acc.seeds.iter().map(|s| match s {
                    Seed::Const(c) => quote!(#shim::seed_from_str(#c)),
                    Seed::Account(a) => {
                        let a = format_ident!("{}", a);
                        quote!(#shim::ToSeed::to_seed(&#a.account_id))
                    }
                    Seed::Arg(a) => {
                        let a = format_ident!("{}", a);
                        quote!(#shim::ToSeed::to_seed(&#a))
                    }
                });
                quote!(let #n = #awm::pda(__program, &[#(#parts),*]);)
            } else if let Some(of) = &acc.loader_of {
                let of = format_ident!("{}", of);
                quote!(let #n = #awm::at(#of.account_id, #loader);)
            } else {
                let id_arg = format_ident!("{}_id", n);
                quote!(let #n = #awm::at(#shim::lee_core::account::AccountId::new(#id_arg), __program);)
            };
            resolve.push(r.clone());
            resolve_host.push(r);
            if acc.signer {
                // Skip the probe when the signer is the root, or another by-id account whose own
                // delivery checks `is_authorized` (never two deliveries to one actor).
                let others = ix.accs[1..].iter().filter(|o| o.by_id() && o.name != *n).map(|o| &o.name);
                let pv = format_ident!("__probe_{}", n);
                if has("via_pda_seed") {
                    // On the PDA-header path `is_authorized` only says "signed this transaction",
                    // which the owner program's caller may have done for any reason: only the
                    // header's own PDA (the root, authorized by its owner program) may sign.
                    let msg = format!("{fname}: on the PDA-header path every signer must be the root (`{n}` is not)");
                    probes_pre.push(quote!(assert!(input.from.is_none() || #n.account_id == #root.account_id, #msg);));
                }
                probes_pre.push(quote!(
                    let #pv = if #n.account_id == #root.account_id #(|| #n.account_id == #others.account_id)* {
                        None
                    } else {
                        Some(#shim::__require_signer(&#n))
                    };
                ));
                probes_post.push(quote!(if let Some(m) = #pv { __r = __r.send(m); }));
            }
        }
        let guard = if has("via_pda_seed") {
            quote!(#shim::__root_guard(input, via_pda_seed);)
        } else {
            let msg = format!("{fname}: instructions must be the transaction's root message");
            quote!(assert!(input.from.is_none(), #msg);)
        };
        let (time_pre, time_post) = if has("now") {
            (quote!(let __now: u64 = now;), quote!(__r = #shim::__bounded(__r, __now);))
        } else {
            (quote!(), quote!())
        };
        let call_args = ix.params.iter().map(|p| match p {
            P::Acc(i) => {
                let n = &ix.accs[*i].name;
                quote!(#n)
            }
            P::Arg(i) => {
                let n = &ix.args[*i].name;
                quote!(#n)
            }
        });
        let acc_names: Vec<&Ident> = ix.accs.iter().map(|a| &a.name).collect();
        arms.push(quote!(
            #enum_name::#v { #(#arg_names),* } => {
                #guard
                let __program = input.receiver.program_account_id;
                #(#resolve)*
                #(#probes_pre)*
                #time_pre
                let mut __r = #mod_name::#fname(#(#call_args),*);
                #(#probes_post)*
                #time_post
                __r
            }
        ));
        declare_arms.push(quote!(
            #enum_name::#v { #(#arg_names),* } => {
                #(#resolve_host)*
                let mut v: ::std::vec::Vec<#shim::lee_core::account::Actor> = ::std::vec::Vec::new();
                for a in [#(#acc_names.actor()),*] { if !v.contains(&a) { v.push(a); } }
                v
            }
        ));
    }

    // enum Effect
    let eff_variants = effs.iter().map(|e| {
        let v = camel(&e.fn_name);
        let fields = e.fields.iter().map(|a| {
            let (n, t) = (&a.name, &a.ty);
            quote!(#n: #t)
        });
        quote!(#v { #(#fields),* })
    });
    let eff_names: Vec<String> = effs.iter().map(|e| e.fn_name.to_string()).collect();
    let init_names: Vec<String> = effs.iter().filter(|e| e.init).map(|e| e.fn_name.to_string()).collect();
    let eff_arms = effs.iter().map(|e| {
        let v = camel(&e.fn_name);
        let f = &e.fn_name;
        let names: Vec<&Ident> = e.fields.iter().map(|a| &a.name).collect();
        let init = if e.init {
            let msg = format!("{f}: the target already exists (write-once)");
            quote!(assert!(input.pre_state.is_empty(), #msg);)
        } else {
            quote!()
        };
        quote!(Effect::#v { #(#names),* } => { #init #mod_name::#f(input, #(#names),*) })
    });
    let n_ix = ixs.len();
    let n_eff = effs.len();

    Ok(quote!(
        #module

        #lock_track

        /// Wire format of a root message: borsh enum (u8 variant + args), generated from the
        /// `#[instruction]` fns in declaration order (= the IDL's instruction order).
        #[derive(::borsh::BorshSerialize, ::borsh::BorshDeserialize, Clone, Debug, PartialEq)]
        pub enum #enum_name { #(#variants),* }

        impl #enum_name {
            /// The instruction's fn name, as in the IDL.
            #[must_use]
            pub fn name(&self) -> &'static str {
                match self { #(#name_arms),* }
            }
        }

        /// Internal messages between this program's own actors, generated from the `#[effect]`
        /// fns in declaration order. On the wire variant `i` is tag `0xC0 + i`.
        #[derive(::borsh::BorshSerialize, ::borsh::BorshDeserialize, Clone, Debug, PartialEq)]
        pub enum Effect { #(#eff_variants),* }

        /// Every message this program accepts.
        #[derive(Clone, Debug, PartialEq)]
        pub enum Message {
            Instruction(#enum_name),
            Effect(Effect),
            RequireSigner,
        }

        impl ::borsh::BorshSerialize for Message {
            fn serialize<W: ::std::io::Write>(&self, w: &mut W) -> ::std::io::Result<()> {
                match self {
                    Self::Instruction(i) => ::borsh::BorshSerialize::serialize(i, w),
                    Self::Effect(e) => ::borsh::BorshSerialize::serialize(&#shim::Shifted(e), w),
                    Self::RequireSigner => w.write_all(&[#shim::REQUIRE_SIGNER_TAG]),
                }
            }
        }

        impl ::borsh::BorshDeserialize for Message {
            fn deserialize_reader<R: ::std::io::Read>(r: &mut R) -> ::std::io::Result<Self> {
                let mut tag = [0u8; 1];
                r.read_exact(&mut tag)?;
                let t = tag[0];
                if usize::from(t) < #n_ix {
                    let head = [t];
                    let mut c = ::std::io::Read::chain(&head[..], &mut *r);
                    Ok(Self::Instruction(::borsh::BorshDeserialize::deserialize_reader(&mut c)?))
                } else if t == #shim::REQUIRE_SIGNER_TAG {
                    Ok(Self::RequireSigner)
                } else if t >= #shim::EFFECT_TAG_MIN && usize::from(t - #shim::EFFECT_TAG_MIN) < #n_eff {
                    let head = [t - #shim::EFFECT_TAG_MIN];
                    let mut c = ::std::io::Read::chain(&head[..], &mut *r);
                    Ok(Self::Effect(::borsh::BorshDeserialize::deserialize_reader(&mut c)?))
                } else {
                    Err(::std::io::Error::new(::std::io::ErrorKind::InvalidData, "unknown message tag"))
                }
            }
        }

        /// Instruction fn names in wire-tag order.
        pub const INSTRUCTION_NAMES: &[&str] = &[#(#ix_names),*];
        /// Effect fn names in wire-tag order (tag = 0xC0 + index).
        pub const EFFECT_NAMES: &[&str] = &[#(#eff_names),*];
        /// The `#[effect(init)]` fns: each refuses a target that already has state.
        pub const INIT_EFFECTS: &[&str] = &[#(#init_names),*];
        /// What `wire-tags.lock` must contain.
        pub const WIRE_TAGS: &str = #wire_tags;

        /// LEZ 0.4 guest entry: `lee_core::actor_entrypoint!(raw <crate>::__lez_dispatch);`
        #[allow(unused_variables, unused_mut, clippy::redundant_clone, clippy::too_many_lines)]
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
                let mut shifted = data.to_vec();
                shifted[0] -= #shim::EFFECT_TAG_MIN;
                let effect: Effect = ::borsh::from_slice(&shifted).expect("effect must decode");
                return match effect { #(#eff_arms)* };
            }
            let ix: #enum_name = ::borsh::from_slice(data).expect("instruction must decode");
            match ix { #(#arms)* }
        }

        /// Host side: every actor a client must declare for `ix`, root first, deduplicated.
        #[allow(unused_variables)]
        #[must_use]
        pub fn actors_to_declare(
            __root_id: #shim::lee_core::account::AccountId,
            __program: #shim::lee_core::account::AccountId,
            ix: &#enum_name,
        ) -> ::std::vec::Vec<#shim::lee_core::account::Actor> {
            match ix.clone() { #(#declare_arms)* }
        }
    ))
}

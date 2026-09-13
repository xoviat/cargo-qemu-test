//! Proc macro behind the `qemu-test` facade.
//!
//! [`macro@tests`] takes a module written in `embedded-test` style and expands it
//! into three cfg-gated items:
//!
//! 1. `#[cfg(target_os = "none")] #[::qemu_test::embedded_tests] mod <name> { .. }`
//!    — the module verbatim (minus `#[host_only]` items), run under QEMU.
//! 2. `#[cfg(not(target_os = "none"))] mod <name> { .. }` — the same functions as
//!    plain `pub(crate)` fns (harness attributes stripped), for the host.
//! 3. `#[cfg(not(target_os = "none"))] fn main()` — a libtest-mimic harness that
//!    registers one trial per test, mirroring embedded-test semantics:
//!    `#[init]` runs before every test, `#[should_panic]` is inverted with
//!    `catch_unwind` (and `expected = "..."` is matched against the panic
//!    payload), `#[ignore]` maps to `with_ignored_flag(true)`, and
//!    `-> Result<(), E>` tests map their `Err` into `Failed`.
//!
//! Because the twins are gated by `cfg(target_os = "none")` rather than chosen at
//! macro-expansion time, the macro never needs to know the compilation target.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input,
    spanned::Spanned,
    Attribute, Ident, Item, ItemFn, ItemMod, LitStr, Path, Result, ReturnType, Token, Type,
    TypeTuple,
};

/// Arguments of `#[test(init = some::path)]` (embedded-test 0.7 per-test init).
#[derive(Default)]
struct TestArgs {
    init: Option<Path>,
}

impl Parse for TestArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut init = None;
        while !input.is_empty() {
            let name: Ident = input.parse()?;
            if name != "init" {
                return Err(syn::Error::new(
                    name.span(),
                    "unsupported #[test] argument; expected `init = path`",
                ));
            }
            input.parse::<Token![=]>()?;
            if init.is_some() {
                return Err(syn::Error::new(name.span(), "duplicate `init` argument"));
            }
            init = Some(input.parse()?);
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(TestArgs { init })
    }
}

/// Arguments of `#[should_panic(expected = "...")]`.
#[derive(Default)]
struct ShouldPanicArgs {
    expected: Option<LitStr>,
}

impl Parse for ShouldPanicArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut expected = None;
        while !input.is_empty() {
            let name: Ident = input.parse()?;
            if name != "expected" {
                return Err(syn::Error::new(
                    name.span(),
                    "unsupported #[should_panic] argument; expected `expected = \"...\"`",
                ));
            }
            input.parse::<Token![=]>()?;
            if expected.is_some() {
                return Err(syn::Error::new(
                    name.span(),
                    "duplicate `expected` argument",
                ));
            }
            expected = Some(input.parse()?);
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(ShouldPanicArgs { expected })
    }
}

/// Classification of one function's attributes.
struct FnAttrs {
    is_test: bool,
    test_init: Option<Path>,
    is_init: bool,
    should_panic: Option<ShouldPanicArgs>,
    ignored: bool,
    host_only: bool,
    target_only: bool,
    /// Attributes that belong to the function on *both* platforms
    /// (`#[cfg(..)]`, `#[allow(..)]`, doc comments, ...).
    passthrough: Vec<Attribute>,
}

fn path_is(attr: &Attribute, name: &str) -> bool {
    attr.path()
        .segments
        .last()
        .map(|s| s.ident == name)
        .unwrap_or(false)
}

fn classify(item: &ItemFn) -> Result<FnAttrs> {
    let mut out = FnAttrs {
        is_test: false,
        test_init: None,
        is_init: false,
        should_panic: None,
        ignored: false,
        host_only: false,
        target_only: false,
        passthrough: Vec::new(),
    };
    for attr in &item.attrs {
        if path_is(attr, "test") {
            if out.is_test {
                return Err(syn::Error::new(attr.span(), "duplicate #[test]"));
            }
            out.is_test = true;
            let args: TestArgs = attr.parse_args().unwrap_or_default();
            out.test_init = args.init;
        } else if path_is(attr, "init") {
            if out.is_init {
                return Err(syn::Error::new(attr.span(), "duplicate #[init]"));
            }
            attr.meta
                .require_path_only()
                .map_err(|_| syn::Error::new(attr.span(), "#[init] takes no arguments"))?;
            out.is_init = true;
        } else if path_is(attr, "should_panic") {
            if out.should_panic.is_some() {
                return Err(syn::Error::new(attr.span(), "duplicate #[should_panic]"));
            }
            out.should_panic = Some(attr.parse_args().unwrap_or_default());
        } else if path_is(attr, "ignore") {
            out.ignored = true;
        } else if path_is(attr, "host_only") {
            out.host_only = true;
        } else if path_is(attr, "target_only") {
            out.target_only = true;
        } else {
            out.passthrough.push(attr.clone());
        }
    }
    if out.is_init && out.is_test {
        return Err(syn::Error::new(
            item.span(),
            "a function cannot be both #[init] and #[test]",
        ));
    }
    if (out.host_only || out.target_only) && !out.is_test {
        return Err(syn::Error::new(
            item.span(),
            "#[host_only]/#[target_only] only make sense on #[test] functions",
        ));
    }
    Ok(out)
}

/// What the test function returns, from the harness's point of view.
enum Outcome {
    Unit,
    Result,
}

fn peel_groups(mut ty: &Type) -> &Type {
    while let Type::Group(g) = ty {
        ty = &g.elem;
    }
    ty
}

fn outcome_of(sig: &syn::Signature) -> Result<Outcome> {
    match &sig.output {
        ReturnType::Default => Ok(Outcome::Unit),
        ReturnType::Type(_, ty) => match peel_groups(ty) {
            Type::Tuple(TypeTuple { elems, .. }) if elems.is_empty() => Ok(Outcome::Unit),
            Type::Path(tp) => match tp.path.segments.last() {
                Some(seg) if seg.ident == "Result" => Ok(Outcome::Result),
                _ => Err(syn::Error::new_spanned(
                    ty,
                    "qemu-test: test functions must return `()` or `Result<(), E>`",
                )),
            },
            _ => Err(syn::Error::new_spanned(
                ty,
                "qemu-test: test functions must return `()` or `Result<(), E>`",
            )),
        },
    }
}

fn check_plain_signature(sig: &syn::Signature, what: &str) -> Result<()> {
    if sig.asyncness.is_some() {
        return Err(syn::Error::new_spanned(
            sig,
            format!("qemu-test: {what} functions cannot be async"),
        ));
    }
    if !sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &sig.generics,
            format!("qemu-test: {what} functions cannot be generic"),
        ));
    }
    if !sig.inputs.is_empty() {
        return Err(syn::Error::new_spanned(
            &sig.inputs,
            format!("qemu-test: {what} functions cannot take arguments"),
        ));
    }
    Ok(())
}

#[proc_macro_attribute]
pub fn tests(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "qemu-test: #[tests] takes no arguments",
        )
        .to_compile_error()
        .into();
    }
    expand(parse_macro_input!(item as ItemMod))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand(input: ItemMod) -> Result<TokenStream2> {
    let ident_span = input.ident.span();
    let vis = input.vis.clone();
    let mod_token = input.mod_token;
    let ident = input.ident.clone();
    let attrs = input.attrs.clone();
    let (brace, items) = match input.content {
        Some(c) => c,
        None => {
            return Err(syn::Error::new(
                ident_span,
                "qemu-test: #[tests] must be applied to an inline module",
            ))
        }
    };
    let mod_attrs = &attrs; // e.g. #[cfg(test)] — copied to both twins
    let mod_ident = &ident;

    // --- inventory ------------------------------------------------------------
    let mut init: Option<Ident> = None;
    struct Test<'a> {
        ident: &'a Ident,
        cfg_attrs: Vec<Attribute>,
        per_test_init: Option<Path>,
        should_panic: Option<ShouldPanicArgs>,
        ignored: bool,
        outcome: Outcome,
    }
    let mut tests: Vec<Test> = Vec::new();

    for item in &items {
        let Item::Fn(f) = item else { continue };
        let cls = classify(f)?;
        if cls.is_init {
            check_plain_signature(&f.sig, "#[init]")?;
            if matches!(outcome_of(&f.sig)?, Outcome::Result) {
                return Err(syn::Error::new_spanned(
                    &f.sig,
                    "qemu-test: #[init] must return ()",
                ));
            }
            if init.is_some() {
                return Err(syn::Error::new_spanned(
                    &f.sig,
                    "qemu-test: at most one #[init] function is allowed",
                ));
            }
            init = Some(f.sig.ident.clone());
        } else if cls.is_test {
            check_plain_signature(&f.sig, "#[test]")?;
            let outcome = outcome_of(&f.sig)?;
            tests.push(Test {
                ident: &f.sig.ident,
                cfg_attrs: cls
                    .passthrough
                    .iter()
                    .filter(|a| path_is(a, "cfg"))
                    .cloned()
                    .collect(),
                per_test_init: cls.test_init,
                should_panic: cls.should_panic,
                ignored: cls.ignored,
                outcome,
            });
        }
    }
    if tests.is_empty() {
        return Err(syn::Error::new(
            ident_span,
            "qemu-test: module contains no #[test] functions",
        ));
    }

    // --- 1. no_std twin: verbatim embedded-test module -------------------------
    let alias: Item = syn::parse2(quote! {
        #[allow(unused_imports)]
        use ::qemu_test as embedded_test;
    })?;
    let mut et_items: Vec<Item> = vec![alias.clone()];
    et_items.extend(items.iter().filter_map(|item| match item {
        Item::Fn(f) => {
            let cls = classify(f).ok()?;
            if cls.host_only {
                return None;
            }
            let mut f = f.clone();
            f.attrs
                .retain(|a| !(path_is(a, "host_only") || path_is(a, "target_only")));
            Some(Item::Fn(f))
        }
        other => Some(other.clone()),
    }));
    let et_mod = ItemMod {
        attrs: Vec::new(),
        vis: syn::Visibility::Inherited,
        unsafety: None,
        mod_token,
        ident: ident.clone(),
        content: Some((brace, et_items)),
        semi: None,
    };

    // --- 2. host twin: plain functions ----------------------------------------
    let host_items: Vec<Item> = items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(f) => {
                let cls = classify(f).ok()?;
                if cls.is_test && cls.target_only {
                    return None;
                }
                let mut f = f.clone();
                f.attrs = cls.passthrough;
                f.vis = syn::parse_quote!(pub(crate));
                Some(Item::Fn(f))
            }
            other => Some(other.clone()),
        })
        .collect();
    let host_mod = ItemMod {
        attrs: Vec::new(),
        vis: syn::Visibility::Inherited,
        unsafety: None,
        mod_token,
        ident: ident.clone(),
        content: Some((brace, host_items)),
        semi: None,
    };

    // --- 3. host harness main ---------------------------------------------------
    let mut trial_stmts: Vec<TokenStream2> = Vec::new();
    for t in &tests {
        let fn_ident = t.ident;
        let name = quote!(::std::concat!(
            ::std::stringify!(#mod_ident),
            "::",
            ::std::stringify!(#fn_ident)
        ));
        let init_call: TokenStream2 = match &t.per_test_init {
            Some(p) => quote!(#p();),
            None => match &init {
                Some(i) => quote!(#mod_ident::#i();),
                None => quote!(),
            },
        };
        let body = match (&t.should_panic, &t.outcome) {
            (Some(sp), _) => {
                let expected_check = match &sp.expected {
                    Some(exp) => quote! {
                        let msg = payload
                            .downcast_ref::<&str>().copied()
                            .or_else(|| payload.downcast_ref::<::std::string::String>().map(|s| s.as_str()));
                        match msg {
                            Some(m) if m.contains(#exp) => {}
                            other => {
                                return ::std::result::Result::Err(
                                    ::qemu_test::libtest_mimic::Failed::from(::std::format!(
                                        "panicked with message {other:?}, expected substring {:?}", #exp
                                    )),
                                );
                            }
                        }
                    },
                    None => quote!(let _ = &payload;),
                };
                // Mirror embedded-test's inversion: any non-success outcome
                // (a panic OR a returned Err) counts as the expected failure.
                let returned_err = match &t.outcome {
                    Outcome::Result => quote! {
                        ::std::result::Result::Ok(::std::result::Result::Err(e)) => {
                            let _ = e; // a failed outcome is exactly what we expected
                            ::std::result::Result::Ok(())
                        }
                    },
                    Outcome::Unit => quote!(),
                };
                quote! {
                    #init_call
                    match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| #mod_ident::#fn_ident())) {
                        ::std::result::Result::Ok(()) => ::std::result::Result::Err(
                            ::qemu_test::libtest_mimic::Failed::from(::std::concat!(
                                ::std::stringify!(#fn_ident),
                                ": test was expected to panic but exited successfully",
                            )),
                        ),
                        #returned_err
                        ::std::result::Result::Err(payload) => {
                            #expected_check
                            ::std::result::Result::Ok(())
                        }
                    }
                }
            }
            (None, Outcome::Unit) => quote! {
                #init_call
                #mod_ident::#fn_ident();
                ::std::result::Result::Ok(())
            },
            (None, Outcome::Result) => quote! {
                #init_call
                #mod_ident::#fn_ident()
                    .map_err(|e| ::qemu_test::libtest_mimic::Failed::from(::std::format!("{e:?}")))?;
                ::std::result::Result::Ok(())
            },
        };
        let cfg_attrs = &t.cfg_attrs;
        let ignored = t.ignored;
        trial_stmts.push(quote! {
            #(#cfg_attrs)*
            {
                let trial = ::qemu_test::libtest_mimic::Trial::test(
                    #name,
                    || -> ::std::result::Result<(), ::qemu_test::libtest_mimic::Failed> { #body },
                );
                let trial = if #ignored { trial.with_ignored_flag(true) } else { trial };
                trials.push(trial);
            }
        });
    }

    Ok(quote! {
        /// Import alias: embedded-test's macro expansion references the
        /// `embedded_test` crate by relative path; this binds that name to the
        /// `qemu-test` facade (which re-exports embedded-test), so test crates
        /// do not need a direct embedded-test dependency.
        #[cfg(target_os = "none")]
        #[allow(unused_imports)]
        use ::qemu_test as embedded_test;

        /// Twin used on `no_std` targets: the original module, run under QEMU.
        #[cfg(target_os = "none")]
        #(#mod_attrs)*
        #[::qemu_test::embedded_tests]
        #et_mod

        /// Twin used on std hosts: the same functions as plain `pub(crate)` items.
        #[cfg(not(target_os = "none"))]
        #(#mod_attrs)*
        #vis
        #host_mod

        /// libtest-mimic harness generated by `qemu-test`.
        ///
        /// Only one `#[qemu_test::tests]` module may exist per test crate, since
        /// each generates this entry point.
        #[cfg(not(target_os = "none"))]
        #[allow(dead_code)] // in a `[lib]`, this `main` is only linked into test binaries
        fn main() {
            let mut trials: ::std::vec::Vec<::qemu_test::libtest_mimic::Trial> = ::std::vec::Vec::new();
            #(#trial_stmts)*
            ::qemu_test::libtest_mimic::run(&::qemu_test::libtest_mimic::Arguments::from_args(), trials).exit();
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn expand_str(item: TokenStream2) -> String {
        expand(syn::parse2(item).unwrap()).unwrap().to_string()
    }

    #[test]
    fn emits_cfg_gated_twins_and_host_main() {
        let out = expand_str(quote! {
            mod tests {
                #[init]
                fn init() {}
                #[test]
                fn it_works() { assert_eq!(2 + 2, 4); }
            }
        });
        assert!(out.contains("cfg (target_os = \"none\")"), "{out}");
        assert!(out.contains("cfg (not (target_os = \"none\"))"), "{out}");
        assert!(out.contains(":: qemu_test :: embedded_tests"), "{out}");
        assert!(out.contains("libtest_mimic :: Trial :: test"), "{out}");
        assert!(out.contains("pub (crate) fn init"), "{out}");
    }

    #[test]
    fn should_panic_uses_catch_unwind_and_expected() {
        let out = expand_str(quote! {
            mod tests {
                #[test]
                #[should_panic(expected = "boom")]
                fn blows_up() { panic!("boom"); }
            }
        });
        assert!(out.contains("catch_unwind"), "{out}");
        assert!(out.contains("boom"), "{out}");
        assert!(out.contains("downcast_ref"), "{out}");
    }

    #[test]
    fn ignore_maps_to_with_ignored_flag() {
        let out = expand_str(quote! {
            mod tests {
                #[test]
                #[ignore]
                fn slow() {}
            }
        });
        assert!(out.contains("with_ignored_flag"), "{out}");
    }

    #[test]
    fn result_tests_map_err_into_failed() {
        let out = expand_str(quote! {
            mod tests {
                #[test]
                fn may_fail() -> Result<(), MyErr> { Ok(()) }
            }
        });
        assert!(out.contains("map_err"), "{out}");
    }

    #[test]
    fn host_only_test_omitted_from_embedded_twin() {
        let out = expand_str(quote! {
            mod tests {
                #[test]
                fn both() {}
                #[test]
                #[host_only]
                fn host_thing() { let _ = std::vec::Vec::<u8>::new(); }
            }
        });
        assert!(out.contains("both"), "{out}");
        let et_pos = out.find("embedded_tests").unwrap();
        let host_pos = out.rfind("host_thing").unwrap();
        assert!(
            et_pos < host_pos,
            "host_only fn must not appear in the no_std twin: {out}"
        );
    }

    #[test]
    fn target_only_test_has_no_host_trial() {
        let item: ItemMod = syn::parse2(quote! {
            mod tests {
                #[test]
                #[target_only]
                fn pokes_registers() {}
            }
        })
        .unwrap();
        let out = expand(item).unwrap().to_string();
        assert!(out.contains("pokes_registers"), "{out}");
        assert!(
            !out.contains(
                "concat ! (stringify ! (tests) , \"::\" , stringify ! (pokes_registers))"
            ),
            "{out}"
        );
    }

    #[test]
    fn cfg_on_test_is_copied_to_host_trial() {
        let out = expand_str(quote! {
            mod tests {
                #[test]
                #[cfg(feature = "spi")]
                fn needs_spi() {}
            }
        });
        let trials_pos = out.find("let mut trials").unwrap();
        assert!(out[trials_pos..].contains("feature = \"spi\""), "{out}");
    }

    #[test]
    fn module_attrs_like_cfg_test_are_preserved() {
        let out = expand_str(quote! {
            #[cfg(test)]
            mod tests {
                #[test]
                fn it_works() {}
            }
        });
        assert!(out.contains("cfg (test)"), "{out}");
    }

    #[test]
    fn rejects_async_tests() {
        let err = expand(
            syn::parse2(quote! {
                mod tests {
                    #[test]
                    async fn networked() {}
                }
            })
            .unwrap(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("async"), "{err}");
    }

    #[test]
    fn rejects_two_inits() {
        let err = expand(
            syn::parse2(quote! {
                mod tests {
                    #[init]
                    fn a() {}
                    #[init]
                    fn b() {}
                    #[test]
                    fn t() {}
                }
            })
            .unwrap(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("at most one"), "{err}");
    }

    #[test]
    fn should_panic_with_result_treats_returned_err_as_expected_failure() {
        let out = expand_str(quote! {
            mod tests {
                #[test]
                #[should_panic]
                fn weird() -> Result<(), E> { Ok(()) }
            }
        });
        assert!(out.contains("catch_unwind"), "{out}");
        // The Ok(Err(e)) arm must short-circuit to Ok(()) before the panic arm.
        let err_arm = out.find("Err (e)").unwrap();
        let payload_arm = out.find("Err (payload)").unwrap();
        assert!(err_arm < payload_arm, "{out}");
    }

    #[test]
    fn per_test_init_is_called_on_host() {
        let out = expand_str(quote! {
            mod tests {
                #[init]
                fn init() {}
                #[test(init = crate::special_setup)]
                fn t() {}
            }
        });
        let trials_pos = out.find("let mut trials").unwrap();
        assert!(out[trials_pos..].contains("special_setup"), "{out}");
    }
}

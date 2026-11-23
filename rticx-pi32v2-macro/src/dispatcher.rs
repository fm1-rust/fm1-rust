//! Harden the receiver and installer emitted by rticx-async-pass 0.2.1.
//! Its dispatcher and idle executor otherwise have safe unit constructors.
use proc_macro2::Ident;
use quote::format_ident;
use rticx_sw_pass::common::codegen::{dispatcher_ident, priority_ty_ident};
use std::collections::HashSet;
use syn::{Expr, Fields, Item, ItemMod, parse_quote, visit_mut::VisitMut};

pub(super) fn protect(module: &mut ItemMod, priorities: &HashSet<u16>) -> syn::Result<()> {
    let Some((_, items)) = &mut module.content else {
        return Err(syn::Error::new(
            module.ident.span(),
            "missing RTICX application body",
        ));
    };
    let mut dispatchers = HashSet::new();
    for &priority in priorities {
        let group = priority_ty_ident(priority, 0);
        let installer = format_ident!("__rticx_internal__{group}__try_install");
        let dispatcher = if priority == 0 {
            format_ident!("__RticxAsyncPrio0ExecutorCore0")
        } else {
            dispatcher_ident(priority, 0)
        };
        let mut saw_installer = false;
        let mut saw_receiver = false;
        for item in &mut *items {
            match item {
                Item::Fn(f) if f.sig.ident == installer => {
                    f.sig.unsafety = Some(parse_quote!(unsafe));
                    f.attrs.push(parse_quote! {
                        #[doc = "# Safety\nThe owning dispatcher must serialize calls after the task \
                                 and its slot are initialized. The selected task must have a queued input."]
                    });
                    saw_installer = true;
                }
                Item::Struct(s) if s.ident == dispatcher && matches!(s.fields, Fields::Unit) => {
                    s.fields =
                        Fields::Named(parse_quote!({ _dispatch: self::__rticx_dispatch::Token }));
                    s.semi_token = None;
                    saw_receiver = true;
                }
                _ => {}
            }
        }
        if !(saw_installer && saw_receiver) {
            return Err(syn::Error::new(
                module.ident.span(),
                format!("unexpected RTICX dispatcher for priority {priority}"),
            ));
        }
        dispatchers.insert(dispatcher);
    }
    if !dispatchers.is_empty() {
        // The application can re-export the runtime under its expected crate name.
        // Define the token here so such a facade cannot substitute a public constructor.
        items.push(parse_quote! {
            mod __rticx_dispatch {
                pub(super) struct Token(());

                /// # Safety
                /// Issue once per dispatcher, exclusively to its IRQ or idle context.
                pub(super) const unsafe fn startup_token() -> Token {
                    Token(())
                }
            }
        });
    }
    for item in items {
        if let Item::Fn(f) = item
            && f.sig.ident == "main"
            && f.attrs.iter().any(|a| {
                a.path()
                    .segments
                    .last()
                    .is_some_and(|s| s.ident == "__entry")
            })
        {
            Startup(&dispatchers).visit_block_mut(&mut f.block);
            return Ok(());
        }
    }
    Err(syn::Error::new(
        module.ident.span(),
        "missing RTICX startup",
    ))
}

struct Startup<'a>(&'a HashSet<Ident>);
impl VisitMut for Startup<'_> {
    // Nested functions (including the user's post_init) are outside startup.
    fn visit_item_mut(&mut self, _: &mut Item) {}
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        if let Expr::MethodCall(call) = expr {
            let mut args = call.args.iter();
            if call.method == "write"
                && let Some(Expr::Path(path)) = args.next()
                && args.next().is_none()
                && let Some(ty) = path.path.get_ident()
                && self.0.contains(ty)
            {
                // The core pass initializes each dispatcher once, with IRQs masked
                // (and initializes its idle executor once on entry to idle).
                call.args[0] = parse_quote!(#ty { _dispatch: unsafe { self::__rticx_dispatch::startup_token() } });
                return;
            }
            if call.method == "exec"
                && call.args.is_empty()
                && let Expr::MethodCall(receiver) = call.receiver.as_ref()
                && receiver.method == "assume_init_mut"
            {
                // Bind the trait's short receiver lifetime even if user code
                // also defines an inherent method that wants a static borrow.
                *expr = parse_quote!(RticIdleTask::exec(#receiver));
                return;
            }
        }
        syn::visit_mut::visit_expr_mut(self, expr);
    }
}

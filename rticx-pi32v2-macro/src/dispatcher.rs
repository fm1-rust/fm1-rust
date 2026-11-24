//! Harden the receiver and installer emitted by rticx-async-pass 0.2.1.
//! Its dispatcher and idle executor otherwise have safe unit constructors.
use proc_macro2::Ident;
use quote::format_ident;
use rticx_sw_pass::common::codegen::{dispatcher_ident, priority_ty_ident};
use std::collections::HashSet;
use syn::{Expr, Fields, Item, ItemMod, Type, parse_quote, visit_mut::VisitMut};

pub(super) fn protect(module: &mut ItemMod, tasks: &[(Ident, u16)]) -> syn::Result<()> {
    let Some((_, items)) = &mut module.content else {
        return Err(syn::Error::new(
            module.ident.span(),
            "missing RTICX application body",
        ));
    };
    let mut dispatchers = HashSet::new();
    let priorities: HashSet<_> = tasks.iter().map(|&(_, p)| p).collect();
    for priority in priorities {
        let group = priority_ty_ident(priority, 0);
        let installer = format_ident!("__rticx_internal__{group}__try_install");
        let dispatcher = if priority == 0 {
            format_ident!("__RticxAsyncPrio0ExecutorCore0")
        } else {
            dispatcher_ident(priority, 0)
        };
        let mut saw_installer = false;
        let mut saw_receiver = false;
        let mut saw_dispatch = false;
        let task_trait = if priority == 0 {
            "RticIdleTask"
        } else {
            "RticTask"
        };
        for item in &mut *items {
            match item {
                Item::Fn(f) if f.sig.ident == installer => {
                    f.sig.unsafety = Some(parse_quote!(unsafe));
                    f.attrs.push(parse_quote! {
                        #[doc = "# Safety\nThe owning dispatcher must serialize calls after the task \
                                 and its slot are initialized. The selected task must have a queued input."]
                    });
                    DispatcherBody.visit_block_mut(&mut f.block);
                    saw_installer = true;
                }
                Item::Struct(s) if s.ident == dispatcher && matches!(s.fields, Fields::Unit) => {
                    s.fields =
                        Fields::Named(parse_quote!({ _dispatch: self::__rticx_dispatch::Token }));
                    s.semi_token = None;
                    saw_receiver = true;
                }
                Item::Impl(i)
                    if matches!(i.self_ty.as_ref(), Type::Path(ty) if ty.path.is_ident(&dispatcher))
                        && i.trait_
                            .as_ref()
                            .is_some_and(|(_, path, _)| path.is_ident(task_trait)) =>
                {
                    let (_, path, _) = i.trait_.as_mut().expect("matched dispatcher trait");
                    *path = parse_quote!(self::rticx_traits::#path);
                    DispatcherBody.visit_item_impl_mut(i);
                    saw_dispatch = true;
                }
                _ => {}
            }
        }
        if !(saw_installer && saw_receiver && saw_dispatch) {
            return Err(syn::Error::new(
                module.ident.span(),
                format!("unexpected RTICX dispatcher for priority {priority}"),
            ));
        }
        dispatchers.insert(dispatcher);
    }
    // The async pass emits one wake function per task, including priority zero.
    // Restrict rewriting to these callbacks and the generated dispatcher bodies.
    let mut wakes: HashSet<_> = tasks
        .iter()
        .map(|(task, _)| format_ident!("__rticx_internal__{task}__wake"))
        .collect();
    for item in &mut *items {
        if let Item::Fn(f) = item
            && wakes.remove(&f.sig.ident)
        {
            DispatcherBody.visit_block_mut(&mut f.block);
        }
    }
    if !wakes.is_empty() {
        return Err(syn::Error::new(
            module.ident.span(),
            "missing RTICX wake callback",
        ));
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
    let entry = items
        .iter_mut()
        .find_map(|item| match item {
            Item::Fn(f)
                if f.sig.ident == "main"
                    && f.attrs.iter().any(|a| {
                        a.path()
                            .segments
                            .last()
                            .is_some_and(|s| s.ident == "__entry")
                    }) =>
            {
                Some(f)
            }
            _ => None,
        })
        .ok_or_else(|| syn::Error::new(module.ident.span(), "missing RTICX startup"))?;
    let mut startup = Startup {
        dispatchers: &dispatchers,
        storage: HashSet::new(),
    };
    startup.visit_block_mut(&mut entry.block);

    for item in items {
        if let Item::Mod(checks) = item
            && checks.ident == "__rticx_trait_checks"
            && let Some((_, body)) = &mut checks.content
        {
            // Explicit imports take precedence over the core pass's `use super::*`.
            body.push(parse_quote! {
                #[allow(unused_imports)]
                use super::rticx_traits::{RticIdleTask, RticTask};
            });
        }
        // Bind only statics initialized by generated startup. Keep application
        // type names in task fields and hand-written code in their original scope.
        if let Item::Static(slot) = item
            && startup.storage.remove(&slot.ident)
        {
            let Type::Path(ty) = slot.ty.as_mut() else {
                return Err(syn::Error::new_spanned(
                    &slot.ty,
                    "unexpected RTICX storage type",
                ));
            };
            let args = &ty.path.segments.last().expect("type path").arguments;
            ty.path = parse_quote!(__rticx_backend::export::TaskStorage #args);
            slot.expr = parse_quote!(__rticx_backend::export::TaskStorage::uninit());
        }
    }
    if !startup.storage.is_empty() {
        return Err(syn::Error::new(
            module.ident.span(),
            "missing RTICX storage",
        ));
    }
    Ok(())
}

struct Startup<'a> {
    dispatchers: &'a HashSet<Ident>,
    storage: HashSet<Ident>,
}

impl VisitMut for Startup<'_> {
    // Nested functions (including the user's post_init) are outside startup.
    fn visit_item_mut(&mut self, _: &mut Item) {}
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        if let Expr::MethodCall(call) = expr {
            if call.method == "write"
                && let Expr::Path(receiver) = call.receiver.as_ref()
                && let Some(slot) = receiver.path.get_ident()
                && call.args.len() == 1
                && let Some(value) = call.args.first_mut()
            {
                self.storage.insert(slot.clone());
                if let Expr::Path(path) = &*value
                    && let Some(ty) = path.path.get_ident()
                    && self.dispatchers.contains(ty)
                {
                    // Startup constructs each dispatcher exactly once.
                    *value = parse_quote!(#ty { _dispatch: unsafe { self::__rticx_dispatch::startup_token() } });
                }
                *expr =
                    parse_quote!(__rticx_backend::export::TaskStorage::write(&mut #slot, #value));
                return;
            }
            if call.method == "exec"
                && call.args.is_empty()
                && let Expr::MethodCall(receiver) = call.receiver.as_ref()
                && receiver.method == "assume_init_mut"
            {
                let slot = &receiver.receiver;
                *expr = parse_quote! {
                    self::rticx_traits::RticIdleTask::exec(
                        __rticx_backend::export::TaskStorage::assume_init_mut(&mut #slot)
                    )
                };
                return;
            }
        }
        syn::visit_mut::visit_expr_mut(self, expr);
    }
}

// The async pass uses method syntax where the app's extension traits are in scope.
// A by-value trait method can take the handle before Rust borrows it for an
// inherent method. In particular, a wake caller must never gain authority to poll.
struct DispatcherBody;
impl VisitMut for DispatcherBody {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        if let Expr::MethodCall(call) = expr
            && let Expr::Path(exec) = call.receiver.as_ref()
            && exec.path.is_ident("exec")
            && matches!(
                call.method.to_string().as_str(),
                "set_pending" | "try_allocate" | "spawn" | "poll"
            )
        {
            let method = &call.method;
            let args = &call.args;
            *expr = parse_quote!(__rticx_backend::executor::Installed::#method(&#exec, #args));
            return;
        }
        if let Expr::MethodCall(call) = expr
            && call.method == "assume_init_mut"
            && call.args.is_empty()
        {
            let slot = &call.receiver;
            *expr = parse_quote!(__rticx_backend::export::TaskStorage::assume_init_mut(&mut #slot));
            return;
        }
        syn::visit_mut::visit_expr_mut(self, expr);
    }
}

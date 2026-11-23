//! Code generation for the single-core AC791N RTICX distribution.
mod dispatcher;
use proc_macro::TokenStream;
use proc_macro2::{Ident, TokenStream as Tokens};
use quote::{format_ident, quote};
use rticx_async_pass::{AsyncPass, AsyncPassBackend};
use rticx_core::{
    Analysis, App, AppArgs, CorePassBackend, RticMacroBuilder, RticTask, SubAnalysis, SubApp,
    parse_utils::RticAttr,
};
use rticx_sw_pass::SwPassBackend;
use std::collections::HashSet;
use syn::{
    Attribute, Expr, ImplItemFn, ItemFn, Path, parse_macro_input, parse_quote, visit_mut::VisitMut,
};

// The runtime can also be re-exported by the board crate.
#[derive(Clone)]
struct Cpu0(Path);

fn is_software(name: &Ident) -> bool {
    matches!(
        name.to_string().as_bytes(),
        [b'S', b'O', b'F', b'T', b'0'..=b'7']
    )
}

// The published Interrupt enum is exhaustively matchable. Only apps that bind
// an additional vector need the newer runtime's index-based helpers.
fn additional_vector(name: &Ident) -> Option<Tokens> {
    (name == "USB0").then(|| quote!(__rticx_backend::export::USB0_IRQ))
}

fn validate_args(args: &Tokens) -> syn::Result<()> {
    let mut args = RticAttr::parse_from_tokens(args.clone(), format_ident!("app"))?;
    if args.take_u32("cores")?.unwrap_or(1) != 1 {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "this distribution runs on CPU0 only",
        ));
    }
    let mut dispatchers = HashSet::new();
    for irq in args.take_ident_array("dispatchers")?.unwrap_or_default() {
        if !dispatchers.insert(irq.clone()) {
            return Err(syn::Error::new(irq.span(), "duplicate software dispatcher"));
        }
        if !is_software(&irq) {
            return Err(syn::Error::new(
                irq.span(),
                "use a software interrupt (SOFT0..SOFT7) as a dispatcher",
            ));
        }
    }
    Ok(())
}

/// Declare a CPU0 application with hardware tasks and async software tasks.
#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    expand(args.into(), input, Cpu0(parse_quote!(::rticx_pi32v2)))
}

/// Declare an FM-1 application, supplying its board and interrupt controller.
#[proc_macro_attribute]
pub fn fm1_app(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = Tokens::from(args);
    expand(
        quote!(device = ::fm1_rticx::bsp, #args),
        input,
        Cpu0(parse_quote!(::fm1_rticx::rticx)),
    )
}

fn validate_cores(module: &syn::ItemMod) -> syn::Result<HashSet<u16>> {
    let mut async_priorities = HashSet::new();
    let Some((_, items)) = &module.content else {
        return Err(syn::Error::new(
            module.ident.span(),
            "expected an inline application module",
        ));
    };
    for item in items {
        let attrs = match item {
            syn::Item::Fn(f) => &f.attrs,
            syn::Item::Struct(s) => &s.attrs,
            _ => continue,
        };
        for attr in attrs {
            let Some(name) = attr.path().get_ident() else {
                continue;
            };
            if !matches!(
                name.to_string().as_str(),
                "task" | "async_task" | "idle" | "shared" | "init" | "post_init"
            ) {
                continue;
            }
            let mut args = RticAttr::parse_from_attr(attr)?;
            if args.take_u32("core")?.unwrap_or(0) != 0 {
                return Err(syn::Error::new(
                    name.span(),
                    "this distribution runs on CPU0 only",
                ));
            }
            if name == "async_task" {
                async_priorities.insert(args.take_u16("priority")?.unwrap_or(0));
            }
        }
    }
    Ok(async_priorities)
}

fn expand(args: Tokens, input: TokenStream, cpu: Cpu0) -> TokenStream {
    let source_file = input
        .clone()
        .into_iter()
        .next()
        .and_then(|t| t.span().local_file());
    let module = parse_macro_input!(input as syn::ItemMod);
    let result = (|| -> syn::Result<Tokens> {
        validate_args(&args)?;
        let priorities = validate_cores(&module)?;
        let mut builder = RticMacroBuilder::new(cpu.clone());
        builder.bind_pre_core_pass(AsyncPass::new(cpu));
        let generated = builder.build_rtic_macro2(args, module, source_file);
        let mut file = syn::parse2::<syn::File>(generated.clone())?;
        if let [syn::Item::Mod(app)] = file.items.as_mut_slice() {
            dispatcher::protect(app, &priorities)?;
            Ok(quote!(#app))
        } else if !file.items.is_empty() && file.items.iter().all(|item| {
            matches!(item, syn::Item::Macro(m) if m.mac.path.segments.last().is_some_and(|s| s.ident == "compile_error"))
        }) {
            Ok(generated)
        } else {
            Err(syn::Error::new(proc_macro2::Span::call_site(), "unexpected RTICX application expansion"))
        }
    })();
    result
        .unwrap_or_else(|error| error.into_compile_error())
        .into()
}

// RTICX emits Rust functions. Assembly calls the entry and handlers through
// the C ABI. Mark them unsafe so application code cannot re-enter a live task.
#[doc(hidden)]
#[proc_macro_attribute]
pub fn __interrupt(_: TokenStream, item: TokenStream) -> TokenStream {
    let mut f = parse_macro_input!(item as ItemFn);
    f.sig.unsafety = Some(parse_quote!(unsafe));
    f.sig.abi = Some(parse_quote!(extern "C"));
    quote!(#f).into()
}

#[doc(hidden)]
#[proc_macro_attribute]
pub fn __entry(_: TokenStream, item: TokenStream) -> TokenStream {
    let mut f = parse_macro_input!(item as ItemFn);
    f.sig.unsafety = Some(parse_quote!(unsafe));
    f.sig.abi = Some(parse_quote!(extern "C"));
    // rticx-async-pass 0.2.1 emits a separate inner block for each slot.
    // Keep its local in the scope of the non-returning entry so IRQs can
    // keep using the address and LLVM cannot reuse it for another slot.
    let mut entry = vec![parse_quote!(unsafe { __rticx_backend::export::reset() };)];
    for stmt in f.block.stmts {
        if let syn::Stmt::Expr(Expr::Block(block), None) = &stmt
            && let [
                syn::Stmt::Local(slot),
                syn::Stmt::Expr(Expr::MethodCall(store), _),
            ] = block.block.stmts.as_slice()
            && matches!(&slot.pat, syn::Pat::Ident(pat) if pat.ident == "__s")
            && store.method == "store"
        {
            let ptr = &store.receiver;
            entry.push(syn::Stmt::Local(slot.clone()));
            // SAFETY: the entry never returns. Both the permanent slot and every
            // recovery use the same task wrapper to infer the future type.
            entry.push(parse_quote!(unsafe { #ptr.install(&__s) };));
        } else {
            entry.push(stmt);
        }
    }
    f.block.stmts = entry;
    quote!(#f).into()
}

impl CorePassBackend for Cpu0 {
    fn entry_name(&self, _: u32) -> Ident {
        format_ident!("main")
    }

    fn entry_attrs(&self) -> Vec<Attribute> {
        vec![parse_quote!(#[__rticx_backend::__entry])]
    }
    fn task_attrs(&self, _: Ident) -> Vec<Attribute> {
        vec![parse_quote!(#[__rticx_backend::__interrupt])]
    }

    fn post_init(&self, _: &AppArgs, _: &SubApp, analysis: &SubAnalysis) -> Option<Tokens> {
        let install = analysis.used_irqs.iter().map(|irq| {
            let name = &irq.name;
            let vector = format_ident!("__rticx_irq_{name}");
            let priority = irq.priority;
            let install = match additional_vector(name) {
                Some(index) => quote!(__rticx_backend::export::install_at(usize::from(#index), #priority, #vector)),
                None => quote!(__rticx_backend::export::install(__rticx_backend::Interrupt::#name, #priority, #vector)),
            };
            quote! {
                unsafe extern "C" { fn #vector(); }
                unsafe { #install; }
            }
        });
        Some(quote!(#(#install)*))
    }

    fn generate_global_definitions(
        &self,
        _: &AppArgs,
        app: &SubApp,
        analysis: &SubAnalysis,
    ) -> Option<Tokens> {
        let mut entries = Vec::new();
        let mut additional = Vec::new();
        for irq in &analysis.used_irqs {
            let name = &irq.name;
            let priority = irq.priority;
            if let Some(index) = additional_vector(name) {
                additional.push(quote!(.with_source(#index, #priority, ceiling)));
            } else {
                entries.push(quote!((__rticx_backend::Interrupt::#name, #priority)));
            }
        }
        let runtime = &self.0;
        let tasks = app.tasks.iter().chain(app.idle.iter()).map(RticTask::name);
        let fields = app.shared.iter().flat_map(|s| &s.resources).map(|f| &f.ty);
        let inputs = app
            .tasks
            .iter()
            .filter(|t| t.args.task_trait == "RticAsyncTask")
            .map(RticTask::name);
        Some(quote! {
            use #runtime as __rticx_backend;
            const _: () = {
                fn require_send<T: ::core::marker::Send>() {}
                fn check_handoffs() {
                    #(require_send::<#tasks>();)*
                    #(require_send::<#fields>();)*
                    #(require_send::<<#inputs as RticAsyncTask>::SpawnInput>();)*
                }
            };
            const fn __rticx_source_mask(ceiling: u16) -> __rticx_backend::export::SourceMask {
                __rticx_backend::export::SourceMask::through(&[#(#entries),*], ceiling) #(#additional)*
            }
        })
    }

    fn generate_resource_proxy_lock_impl(
        &self,
        _: &AppArgs,
        app: &SubApp,
        mut lock: ImplItemFn,
    ) -> ImplItemFn {
        let shared = app
            .shared
            .as_ref()
            .expect("RTICX resource proxy requires shared storage");
        let mut projection = RawProjection {
            storage: shared.name_uppercase(),
            ty: &shared.strct.ident,
            member: None,
        };
        projection.visit_block_mut(&mut lock.block);
        let Some(member) = projection.member else {
            lock.block = parse_quote!({
                compile_error!("unexpected RTICX resource projection");
            });
            return lock;
        };
        let ty = &shared.strct.ident;
        lock.block.stmts.push(parse_quote!({
            const MASK: __rticx_backend::export::SourceMask = __rticx_source_mask(CEILING);
            static BORROW: __rticx_backend::export::BorrowFlag =
                __rticx_backend::export::BorrowFlag::FREE;
            let _ = task_priority;
            let shared_ptr: *mut #ty = resource_ptr;
            unsafe {
                // Project the field directly so rustc checks packed-field alignment.
                __rticx_backend::export::with_ceiling(&MASK, &BORROW, || f(&mut (*shared_ptr).#member))
            }
        }));
        lock
    }

    fn wrap_task_execution(&self, task: &RticTask, call: Tokens) -> Option<Tokens> {
        let name = task.args.binds.as_ref()?;
        is_software(name).then(|| {
            quote! {
                __rticx_backend::export::acknowledge(__rticx_backend::Interrupt::#name);
                #call
            }
        })
    }

    fn populate_idle_loop(&self) -> Option<Tokens> {
        Some(quote!(unsafe { __rticx_backend::export::__rticx_idle() };))
    }
    fn generate_enable_global_interrupts(&self) -> Option<Tokens> {
        Some(quote!(unsafe { __rticx_backend::export::enable() };))
    }
    fn generate_interrupt_free_fn(&self, mut f: ItemFn) -> ItemFn {
        f.block = parse_quote!({ __rticx_backend::export::critical_section::with(|_| f()) });
        f
    }

    fn pre_codegen_validation(&self, app: &App, _: &Analysis) -> syn::Result<()> {
        for task in &app.sub_apps[0].tasks {
            if let Some(irq) = &task.args.binds
                && !(1..=8).contains(&task.args.priority)
            {
                return Err(syn::Error::new(
                    irq.span(),
                    "interrupt priority must be 1..=8",
                ));
            }
        }
        Ok(())
    }
}

// The core pass supplies the field projection in the lock body. Retain a raw
// pointer to Shared until the field is locked. Borrowing the whole struct
// would alias an unrelated field held by a preempted task.
struct RawProjection<'a> {
    storage: Ident,
    ty: &'a Ident,
    member: Option<syn::Member>,
}
impl VisitMut for RawProjection<'_> {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        if let Expr::Reference(r) = expr
            && let Expr::Field(field) = r.expr.as_ref()
            && let Expr::MethodCall(base) = field.base.as_ref()
            && base.method == "assume_init_mut"
            && let Expr::Path(storage) = base.receiver.as_ref()
            && storage.path.is_ident(&self.storage)
        {
            let Self { storage, ty, .. } = self;
            self.member = Some(field.member.clone());
            *expr = parse_quote!((&raw mut #storage).cast::<#ty>());
        } else {
            syn::visit_mut::visit_expr_mut(self, expr);
        }
    }
}

impl SwPassBackend for Cpu0 {
    fn queue_path(&self) -> Path {
        parse_quote!(__rticx_backend::export::Queue)
    }
    fn custom_interrupt_path(&self, _: u32) -> Option<Path> {
        Some(parse_quote!(__rticx_backend::Interrupt))
    }
    fn generate_local_pend_fn(&self, _: u32, mut f: ItemFn) -> ItemFn {
        f.block = parse_quote!({
            __rticx_backend::export::pend(irq_nbr);
        });
        f
    }
    fn generate_cross_pend_fn(&self, _: u32, _: ItemFn) -> Option<ItemFn> {
        None
    }
}
impl AsyncPassBackend for Cpu0 {
    fn async_runtime_path(&self) -> Path {
        parse_quote!(__rticx_backend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsupported_controller_bindings() {
        assert!(validate_args(&quote!(cores = 2)).is_err());
        assert!(validate_args(&quote!(dispatchers = [TIMER5])).is_err());
        assert!(validate_args(&quote!(dispatchers = [SOFT1, SOFT1])).is_err());
        assert!(validate_args(&quote!(dispatchers = [SOFT0, SOFT7])).is_ok());
        let app = parse_quote!(
            mod app {
                #[task(core = 1, binds = ALNK0)]
                struct Audio;
            }
        );
        assert!(validate_cores(&app).is_err());
    }
}

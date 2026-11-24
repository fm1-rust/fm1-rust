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

// An import resolves only real enum variants. An associated-item expression
// could also resolve an application trait's constant for an unsupported name.
fn interrupt_variant(name: &Ident) -> Tokens {
    quote!({
        use __rticx_backend::Interrupt::#name as irq;
        irq
    })
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
    let args = Tokens::from(args);
    let app = Tokens::from(input);
    quote!(::rticx_pi32v2::__expand_app! { [#args] #app }).into()
}

/// Declare an FM-1 application, supplying its board and interrupt controller.
#[proc_macro_attribute]
pub fn fm1_app(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = Tokens::from(args);
    let app = Tokens::from(input);
    quote!(::fm1_rticx::rticx::__expand_app! { [device = ::fm1_rticx::bsp, #args] #app }).into()
}

/// Implementation of the runtime's application macro.
///
/// # Safety
/// The caller must supply the genuine runtime path. Its handoff check must
/// enforce `core::marker::Send` before startup enables interrupts.
#[doc(hidden)]
#[proc_macro]
pub fn __generate_app(input: TokenStream) -> TokenStream {
    use syn::parse::Parser;
    let parse = |input: syn::parse::ParseStream<'_>| {
        let runtime = input.parse::<Path>()?;
        let proof;
        syn::bracketed!(proof in input);
        let proof = proof.parse::<Expr>()?;
        let args;
        syn::bracketed!(args in input);
        let app = expand(args.parse()?, input.parse()?, Cpu0(runtime))?;

        // Direct invocation must establish the same safety contract as the wrapper.
        // A private constructor rules out a struct literal. Const evaluation rules
        // out manufacturing the value with panic!() in an unused function.
        Ok(quote! {
            const _: () = {
                mod __rticx_origin {
                    pub(super) struct Verified(());

                    /// # Safety
                    /// The supplied runtime must uphold the backend's safety contracts.
                    pub(super) const unsafe fn assume_genuine() -> Verified { Verified(()) }
                }
                let _: __rticx_origin::Verified = #proof;
            };
            #app
        })
    };
    parse
        .parse(input)
        .unwrap_or_else(|e| e.into_compile_error())
        .into()
}

fn validate_cores(module: &syn::ItemMod) -> syn::Result<Vec<(Ident, u16)>> {
    let mut async_tasks = Vec::new();
    let Some((_, items)) = &module.content else {
        return Err(syn::Error::new(
            module.ident.span(),
            "expected an inline application module",
        ));
    };
    for item in items {
        let (ident, attrs) = match item {
            syn::Item::Fn(f) => (&f.sig.ident, &f.attrs),
            syn::Item::Struct(s) => (&s.ident, &s.attrs),
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
                async_tasks.push((ident.clone(), args.take_u16("priority")?.unwrap_or(0)));
            }
        }
    }
    Ok(async_tasks)
}

fn expand(args: Tokens, module: syn::ItemMod, cpu: Cpu0) -> syn::Result<Tokens> {
    let source_file = module.ident.span().unwrap().local_file();
    validate_args(&args)?;
    let async_tasks = validate_cores(&module)?;
    let mut builder = RticMacroBuilder::new(cpu.clone());
    builder.bind_pre_core_pass(AsyncPass::new(cpu));
    let generated = builder.build_rtic_macro2(args, module, source_file);
    let mut file = syn::parse2::<syn::File>(generated.clone())?;
    if let [syn::Item::Mod(app)] = file.items.as_mut_slice() {
        dispatcher::protect(app, &async_tasks)?;
        return Ok(quote!(#app));
    }
    if !file.items.is_empty() && file.items.iter().all(|item| {
        matches!(item, syn::Item::Macro(m) if m.mac.path.segments.last().is_some_and(|s| s.ident == "compile_error"))
    }) {
        return Ok(generated);
    }
    Err(syn::Error::new(
        proc_macro2::Span::call_site(),
        "unexpected RTICX application expansion",
    ))
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
            let Some(init) = &slot.init else {
                return syn::Error::new_spanned(slot, "missing RTICX executor slot initializer")
                    .into_compile_error()
                    .into();
            };
            let Expr::Call(call) = init.expr.as_ref() else {
                return syn::Error::new_spanned(
                    &init.expr,
                    "expected RTICX executor slot constructor",
                )
                .into_compile_error()
                .into();
            };
            // Resolve the wrapper in the runtime. A lookalike ManuallyDrop::new
            // could return an ExecSlot for a different future before type erasure.
            let args = &call.args;
            let pat = &slot.pat;
            entry.push(parse_quote!(let #pat = __rticx_backend::export::ManuallyDrop::new(#args);));
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
            let source = interrupt_variant(name);
            quote! {
                unsafe extern "C" { fn #vector(); }
                unsafe { __rticx_backend::export::install(#source, #priority, #vector); }
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
        let entries = analysis.used_irqs.iter().map(|irq| {
            let source = interrupt_variant(&irq.name);
            let priority = irq.priority;
            quote!((#source, #priority))
        });
        let runtime = &self.0;
        let tasks = app.tasks.iter().chain(app.idle.iter()).map(RticTask::name);
        let inputs = app
            .tasks
            .iter()
            .filter(|t| t.args.task_trait == "RticAsyncTask")
            .map(RticTask::name);
        Some(quote! {
            use #runtime as __rticx_backend;
            const _: () = {
                fn require_send<T: #runtime::export::HandoffSend>() {}
                fn check_handoffs() {
                    #(require_send::<#tasks>();)*
                    #(require_send::<<#inputs as RticAsyncTask>::SpawnInput>();)*
                }
            };
            const fn __rticx_source_mask(ceiling: u16) -> __rticx_backend::export::SourceMask {
                __rticx_backend::export::SourceMask::through(&[#(#entries),*], ceiling)
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
        // A struct pattern requires a declared field. Field access alone can invoke
        // DerefMut if cfg removed the field after the app macro saw it.
        lock.block.stmts.push(parse_quote!({
            const _: fn(&#ty) = |#ty { #member: _, .. }| ();
            const MASK: __rticx_backend::export::SourceMask = __rticx_source_mask(CEILING);
            static BORROW: __rticx_backend::export::BorrowFlag =
                __rticx_backend::export::BorrowFlag::FREE;
            let _ = task_priority;
            let shared_ptr: *mut #ty = resource_ptr;
            // The nested mutable reference is invariant in T. An application
            // DerefMut cannot coerce a different physical field to the proxy type.
            fn check_field<T: __rticx_backend::export::HandoffSend>(_: &mut &mut T) {}
            unsafe {
                // Project the field directly so rustc checks packed-field alignment.
                __rticx_backend::export::with_ceiling(&MASK, &BORROW, || {
                    let mut field = &mut (*shared_ptr).#member;
                    check_field::<Self::ResourceType>(&mut field);
                    f(field)
                })
            }
        }));
        lock
    }

    fn wrap_task_execution(&self, task: &RticTask, _: Tokens) -> Option<Tokens> {
        let irq = task.args.binds.as_ref()?;
        let storage = task.name_uppercase();
        let ack = is_software(irq).then(|| {
            quote!(
                __rticx_backend::export::acknowledge(__rticx_backend::Interrupt::#irq);
            )
        });
        // The app can shadow the glob re-export. Only the defining trait limits
        // the task borrow to this invocation.
        Some(quote! {
            #ack
            self::rticx_traits::RticTask::exec(unsafe {
                __rticx_backend::export::TaskStorage::assume_init_mut(&mut #storage)
            });
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

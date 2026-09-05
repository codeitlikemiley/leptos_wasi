//! Minimal SSR response integration kept local so final-WASI builds do not
//! inherit browser-only default features from `leptos_integration_utils`.
//!
//! The behavior mirrors Leptos' MIT-licensed integration utility while keeping
//! the dependency feature graph explicit for WASI Preview 3 components.

use std::{future::Future, pin::Pin, sync::Arc};

use futures::{Stream, StreamExt, stream::once};
use hydration_context::{SharedContext, SsrSharedContext};
use leptos::{
    IntoView, PrefetchLazyFn, WasmSplitManifest,
    context::provide_context,
    nonce::use_nonce,
    reactive::owner::{Owner, Sandboxed},
};
use leptos_meta::{Link, ServerMetaContextOutput};

pub(crate) type PinnedStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;
type PinnedFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
type BoxedFnOnce<T> = Box<dyn FnOnce() -> T + Send>;
type StreamBuilder<IV> = fn(
    IV,
    BoxedFnOnce<PinnedStream<String>>,
    bool,
) -> PinnedFuture<PinnedStream<String>>;

pub(crate) trait ExtendResponse: Sized {
    type ResponseOptions: Send;

    fn from_stream(stream: impl Stream<Item = String> + Send + 'static)
    -> Self;

    fn extend_response(&mut self, options: &Self::ResponseOptions);

    fn set_default_content_type(&mut self, content_type: &str);

    fn from_app<IV>(
        app_fn: impl FnOnce() -> IV + Send + 'static,
        meta_context: ServerMetaContextOutput,
        additional_context: impl FnOnce() + Send + 'static,
        response_options: Self::ResponseOptions,
        stream_builder: StreamBuilder<IV>,
        supports_out_of_order: bool,
    ) -> impl Future<Output = Self> + Send
    where
        IV: IntoView + 'static,
    {
        async move {
            let prefetches = PrefetchLazyFn::default();
            let (owner, stream) = build_response(
                app_fn,
                additional_context,
                stream_builder,
                supports_out_of_order,
            );

            owner.with(|| provide_context(prefetches.clone()));
            let shared_context = owner
                .shared_context()
                .expect("SSR owner must have a shared context");
            let stream =
                stream.await.ready_chunks(32).map(|chunks| chunks.join(""));

            while let Some(pending) = shared_context.await_deferred() {
                pending.await;
            }

            register_split_prefetches(&prefetches);

            let mut stream = Box::pin(
                meta_context.inject_meta_context(stream).await.then({
                    let shared_context = Arc::clone(&shared_context);
                    move |chunk| {
                        let shared_context = Arc::clone(&shared_context);
                        async move {
                            while let Some(pending) =
                                shared_context.await_deferred()
                            {
                                pending.await;
                            }
                            chunk
                        }
                    }
                }),
            );

            let first_chunk = stream.next().await.unwrap_or_default();
            let mut response = Self::from_stream(Sandboxed::new(
                once(async move { first_chunk }).chain(stream).chain(once(
                    async move {
                        owner.unset_with_forced_cleanup();
                        String::new()
                    },
                )),
            ));
            response.extend_response(&response_options);
            response.set_default_content_type("text/html; charset=utf-8");
            response
        }
    }
}

fn register_split_prefetches(prefetches: &PrefetchLazyFn) {
    use leptos::prelude::*;

    if prefetches.0.read_value().is_empty() {
        return;
    }

    let nonce = use_nonce()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let Some(manifest) = use_context::<WasmSplitManifest>() else {
        return;
    };
    let (package_path, modules, split_loader) = &*manifest.0.read_value();
    let requested = prefetches.0.read_value();

    for module in requested
        .iter()
        .flat_map(|key| modules.get(*key).into_iter().flatten())
    {
        // Leptos' `<Link crossorigin=nonce>` serializes as
        // `crossorigin="<nonce>"`. A wasm `fetch` preload needs `nonce="<nonce>"`
        // (and `crossorigin="anonymous"` when `/pkg` is off-origin). Tracked
        // separately upstream: leptos-rs/leptos `crossorigin=nonce` → `nonce=nonce`.
        _ = view! {
            <Link
                rel="preload"
                href=format!("{package_path}/{module}.wasm")
                as_="fetch"
                type_="application/wasm"
                crossorigin=nonce.clone()
            />
        }
        .to_html();
    }
    _ = view! {
        <Link
            rel="modulepreload"
            href=format!("{package_path}/{split_loader}")
            crossorigin=nonce
        />
    }
    .to_html();
}

fn build_response<IV>(
    app_fn: impl FnOnce() -> IV + Send + 'static,
    additional_context: impl FnOnce() + Send + 'static,
    stream_builder: StreamBuilder<IV>,
    supports_out_of_order: bool,
) -> (Owner, PinnedFuture<PinnedStream<String>>)
where
    IV: IntoView + 'static,
{
    let shared_context = Arc::new(SsrSharedContext::new())
        as Arc<dyn SharedContext + Send + Sync>;
    let owner = Owner::new_root(Some(Arc::clone(&shared_context)));
    let stream = Box::pin(Sandboxed::new({
        let owner = owner.clone();
        async move {
            let stream = owner.with(|| {
                additional_context();
                let app = app_fn();
                let nonce = use_nonce()
                    .as_ref()
                    .map(|nonce| format!(" nonce=\"{nonce}\""))
                    .unwrap_or_default();
                let shared_context = Owner::current_shared_context()
                    .expect("SSR owner must have a shared context");
                let chunks =
                    Box::new(move || {
                        Box::pin(
                        shared_context.pending_data().expect(
                            "SSR shared context must expose pending data",
                        )
                        .map(move |chunk| {
                            format!("<script{nonce}>{chunk}</script>")
                        }),
                    ) as PinnedStream<String>
                    });
                stream_builder(app, chunks, supports_out_of_order)
            });
            stream.await
        }
    }));
    (owner, stream)
}

#[cfg(test)]
mod tests {
    use leptos::prelude::{Owner, WriteValue};

    use super::*;

    #[test]
    fn empty_prefetches_are_a_no_op() {
        let owner = Owner::new();
        owner.with(|| {
            register_split_prefetches(&PrefetchLazyFn::default());
        });
    }

    #[test]
    fn missing_wasm_split_manifest_is_a_no_op() {
        let owner = Owner::new();
        owner.with(|| {
            let prefetches = PrefetchLazyFn::default();
            prefetches.0.write_value().insert("missing-chunk");
            register_split_prefetches(&prefetches);
        });
    }
}

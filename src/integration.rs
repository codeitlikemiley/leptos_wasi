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
use leptos_meta::ServerMetaContextOutput;

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
            let (owner, shared_context, stream) = build_response(
                app_fn,
                additional_context,
                stream_builder,
                supports_out_of_order,
            );

            owner.with(|| provide_context(prefetches.clone()));
            let stream =
                stream.await.ready_chunks(32).map(|chunks| chunks.join(""));

            while let Some(pending) = shared_context.await_deferred() {
                pending.await;
            }

            // Prefetch registration has to run on the same owner that rendered
            // the app. `use_nonce` and `WasmSplitManifest` are provided there;
            // calling them after the owner is unset drops the links silently.
            let prefetch_markup =
                owner.with(|| split_prefetch_markup(&prefetches));

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

            let first_chunk = insert_head_markup(
                stream.next().await.unwrap_or_default(),
                &prefetch_markup,
            );
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

/// Markup for lazy split-WASM preloads.
///
/// Must run inside the owner that rendered the app. `leptos_meta::Link` has
/// no `nonce` attribute, and Leptos' own `HydrationScripts` still writes the
/// nonce into `crossorigin` (`leptos` 0.8.22 `hydration/mod.rs`). These
/// preloads are emitted here so `nonce` matches the per-request CSP nonce
/// and `crossorigin` stays a CORS mode.
fn split_prefetch_markup(prefetches: &PrefetchLazyFn) -> String {
    use leptos::prelude::*;

    if prefetches.0.read_value().is_empty() {
        return String::new();
    }

    let nonce = use_nonce().map(|value| value.to_string());
    let Some(manifest) = use_context::<WasmSplitManifest>() else {
        return String::new();
    };
    let (package_path, modules, split_loader) = &*manifest.0.read_value();
    let requested = prefetches.0.read_value();
    let mut markup = String::new();

    for module in requested
        .iter()
        .flat_map(|key| modules.get(*key).into_iter().flatten())
    {
        let href = format!("{package_path}/{module}.wasm");
        push_prefetch_link(
            &mut markup,
            "preload",
            &href,
            Some("fetch"),
            Some("application/wasm"),
            nonce.as_deref(),
        );
    }
    let loader = format!("{package_path}/{split_loader}");
    push_prefetch_link(
        &mut markup,
        "modulepreload",
        &loader,
        None,
        None,
        nonce.as_deref(),
    );
    markup
}

fn push_prefetch_link(
    markup: &mut String,
    rel: &str,
    href: &str,
    as_: Option<&str>,
    type_: Option<&str>,
    nonce: Option<&str>,
) {
    markup.push_str("<link rel=\"");
    markup.push_str(rel);
    markup.push_str("\" href=\"");
    push_attr(markup, href);
    markup.push('"');
    if let Some(as_) = as_ {
        markup.push_str(" as=\"");
        push_attr(markup, as_);
        markup.push('"');
    }
    if let Some(type_) = type_ {
        markup.push_str(" type=\"");
        push_attr(markup, type_);
        markup.push('"');
    }
    markup.push_str(" crossorigin=\"anonymous\"");
    if let Some(nonce) = nonce.filter(|nonce| !nonce.is_empty()) {
        markup.push_str(" nonce=\"");
        push_attr(markup, nonce);
        markup.push('"');
    }
    markup.push('>');
}

fn push_attr(markup: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => markup.push_str("&amp;"),
            '"' => markup.push_str("&quot;"),
            '<' => markup.push_str("&lt;"),
            '>' => markup.push_str("&gt;"),
            _ => markup.push(character),
        }
    }
}

fn insert_head_markup(chunk: String, markup: &str) -> String {
    if markup.is_empty() {
        return chunk;
    }
    if let Some(index) = chunk.find("</head>") {
        let mut updated = String::with_capacity(chunk.len() + markup.len());
        updated.push_str(&chunk[..index]);
        updated.push_str(markup);
        updated.push_str(&chunk[index..]);
        updated
    } else {
        let mut updated = String::with_capacity(chunk.len() + markup.len());
        updated.push_str(markup);
        updated.push_str(&chunk);
        updated
    }
}

fn build_response<IV>(
    app_fn: impl FnOnce() -> IV + Send + 'static,
    additional_context: impl FnOnce() + Send + 'static,
    stream_builder: StreamBuilder<IV>,
    supports_out_of_order: bool,
) -> (
    Owner,
    Arc<SsrSharedContext>,
    PinnedFuture<PinnedStream<String>>,
)
where
    IV: IntoView + 'static,
{
    let shared_context = Arc::new(SsrSharedContext::new());
    let owner = Owner::new_root(Some(
        Arc::clone(&shared_context) as Arc<dyn SharedContext + Send + Sync>
    ));
    let stream = Box::pin(Sandboxed::new({
        let owner = owner.clone();
        let shared_context = Arc::clone(&shared_context);
        async move {
            let stream = owner.with(|| {
                additional_context();
                let app = app_fn();
                let nonce = use_nonce()
                    .as_ref()
                    .map(|nonce| format!(" nonce=\"{nonce}\""))
                    .unwrap_or_default();
                let chunks = Box::new(move || {
                    let pending = shared_context
                        .pending_data()
                        .unwrap_or_else(|| Box::pin(futures::stream::empty()));
                    Box::pin(pending.map(move |chunk| {
                        format!("<script{nonce}>{chunk}</script>")
                    })) as PinnedStream<String>
                });
                stream_builder(app, chunks, supports_out_of_order)
            });
            stream.await
        }
    }));
    (owner, shared_context, stream)
}

#[cfg(test)]
mod tests {
    use leptos::prelude::{Owner, WriteValue};

    use super::*;

    #[test]
    fn empty_prefetches_are_a_no_op() {
        let owner = Owner::new();
        owner.with(|| {
            assert_eq!(split_prefetch_markup(&PrefetchLazyFn::default()), "");
        });
    }

    #[test]
    fn missing_wasm_split_manifest_is_a_no_op() {
        let owner = Owner::new();
        owner.with(|| {
            let prefetches = PrefetchLazyFn::default();
            prefetches.0.write_value().insert("missing-chunk");
            assert_eq!(split_prefetch_markup(&prefetches), "");
        });
    }

    #[test]
    fn split_preloads_carry_the_request_nonce_not_crossorigin() {
        use std::collections::HashMap;

        let owner = Owner::new();
        owner.with(|| {
            leptos::nonce::provide_nonce();
            let nonce = leptos::nonce::use_nonce()
                .expect("nonce context")
                .to_string();
            leptos::context::provide_context(WasmSplitManifest(
                reactive_graph::owner::ArcStoredValue::new((
                    "/pkg".to_owned(),
                    HashMap::from([(
                        "chunk".to_owned(),
                        vec!["chunk&a".to_owned()],
                    )]),
                    "split.js".to_owned(),
                )),
            ));
            let prefetches = PrefetchLazyFn::default();
            prefetches.0.write_value().insert("chunk");
            let markup = split_prefetch_markup(&prefetches);
            assert!(
                markup.contains(&format!("nonce=\"{nonce}\"")),
                "preload links must carry the request nonce, got {markup}"
            );
            assert!(markup.contains("crossorigin=\"anonymous\""));
            assert!(
                !markup.contains(&format!("crossorigin=\"{nonce}\"")),
                "the nonce must not be written into crossorigin, got {markup}"
            );
            assert!(markup.contains("chunk&amp;a.wasm"));
            assert!(markup.contains("rel=\"modulepreload\""));
        });
    }

    #[test]
    fn prefetch_markup_is_inserted_before_the_head_closes() {
        let updated = insert_head_markup(
            "<head><title>t</title></head><body></body>".to_owned(),
            "<link rel=\"modulepreload\" href=\"/pkg/split.js\" nonce=\"abc\">",
        );
        assert!(updated.contains(
            "<link rel=\"modulepreload\" href=\"/pkg/split.js\" nonce=\"abc\"></head>"
        ));
    }
}

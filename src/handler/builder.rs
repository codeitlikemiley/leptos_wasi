//! Builder methods shared by both preview handlers.
//!
//! Both `Handler` types wrap a [`HandlerCore`](super::HandlerCore) and
//! delegate registration to it identically; only their `build` and
//! `handle` signatures differ. The macro writes those delegations, and
//! their documentation, once.

macro_rules! common_handler_methods {
    () => {
        /// Registers a typed Leptos server function.
        #[must_use]
        pub fn with_server_fn<T>(mut self) -> Self
        where
            T: ServerFn + 'static,
            T::Server: ServerWithBody<
                    T::Error,
                    T::InputStreamError,
                    T::OutputStreamError,
                >,
            ReqBody<T>: From<Bytes> + Send + 'static,
            ResBody<T>: Into<Body> + Send + 'static,
        {
            self.core = self.core.with_server_fn::<T>();
            self
        }

        /// Registers a static-file callback for one URI prefix.
        pub fn static_files_handler<T>(
            mut self,
            prefix: T,
            handler: impl Fn(String) -> Option<Body> + 'static + Send + Clone,
        ) -> Result<Self, RegistrationError>
        where
            T: TryInto<Uri>,
            <T as TryInto<Uri>>::Error: std::error::Error,
        {
            self.core = self.core.static_files_handler(prefix, handler)?;
            Ok(self)
        }

        /// Generates Leptos routes for the application.
        pub fn generate_routes<IV>(
            self,
            app: impl Fn() -> IV + 'static + Send + Clone,
        ) -> Result<Self, RegistrationError>
        where
            IV: IntoView + 'static,
        {
            self.generate_routes_with_exclusions_and_discovery_context(
                app,
                None,
                || {},
            )
        }

        /// Generates routes with deterministic route-discovery context.
        ///
        /// The context closure runs only while discovering the application's
        /// route list. It receives synthetic standard contexts and may be
        /// skipped when an identical application/context closure type is
        /// already cached. It must not inspect authentication, headers, or any
        /// other request-dependent state.
        ///
        /// Use [`Self::handle_with_context`] for per-request context.
        pub fn generate_routes_with_discovery_context<IV>(
            self,
            app: impl Fn() -> IV + 'static + Send + Clone,
            context: impl Fn() + 'static + Send + Clone,
        ) -> Result<Self, RegistrationError>
        where
            IV: IntoView + 'static,
        {
            self.generate_routes_with_exclusions_and_discovery_context(
                app, None, context,
            )
        }

        /// Compatibility alias for route-discovery context.
        ///
        /// This method has always applied `context` while discovering routes;
        /// request-dependent context belongs in [`Self::handle_with_context`].
        pub fn generate_routes_with_context<IV>(
            self,
            app: impl Fn() -> IV + 'static + Send + Clone,
            context: impl Fn() + 'static + Send + Clone,
        ) -> Result<Self, RegistrationError>
        where
            IV: IntoView + 'static,
        {
            self.generate_routes_with_discovery_context(app, context)
        }

        /// Generates routes with exclusions and deterministic discovery context.
        ///
        /// Route discovery is cached per concrete application/context closure
        /// type, and only when both are zero-sized (function items or
        /// non-capturing closures) so that one type cannot describe two
        /// different applications. Function pointers and capturing closures
        /// re-run discovery on every request.
        /// The context closure runs against synthetic standard contexts
        /// only when route discovery executes. Route structure, exclusions,
        /// and discovery context must be deterministic deployment
        /// configuration rather than request-dependent state.
        ///
        /// Use [`Self::handle_with_context`] for per-request context.
        pub fn generate_routes_with_exclusions_and_discovery_context<IV>(
            mut self,
            app: impl Fn() -> IV + 'static + Send + Clone,
            excluded: Option<Vec<String>>,
            context: impl Fn() + 'static + Send + Clone,
        ) -> Result<Self, RegistrationError>
        where
            IV: IntoView + 'static,
        {
            self.core = self
                .core
                .generate_routes_with_exclusions_and_discovery_context(
                    app, excluded, context,
                )?;
            Ok(self)
        }

        /// Compatibility alias for exclusions plus route-discovery context.
        ///
        /// This method has always applied `context` while discovering routes;
        /// request-dependent context belongs in [`Self::handle_with_context`].
        pub fn generate_routes_with_exclusions_and_context<IV>(
            self,
            app: impl Fn() -> IV + 'static + Send + Clone,
            excluded: Option<Vec<String>>,
            context: impl Fn() + 'static + Send + Clone,
        ) -> Result<Self, RegistrationError>
        where
            IV: IntoView + 'static,
        {
            self.generate_routes_with_exclusions_and_discovery_context(
                app, excluded, context,
            )
        }
    };
}

pub(crate) use common_handler_methods;

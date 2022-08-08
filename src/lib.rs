//! This is a library designed to ease the creation of a collection of singleton services namespaced to a single
//! base directory path with an arbitrary network of dependencies, starting services as needed,
//! using unix sockets as the communication mechanism.
//!
//! In the case that a service already exists, then this will communicate with the appropriate
//! socket, rather than start a new one.
//!
//! To get started, take a look at the [`declare_service`] macro.

/// Re-export of the chaining-transformation convenience functions crate.
pub use chain_trans;

mod cleanable_path;
pub mod mapfut;
pub mod socket_shims;
pub mod timefut;

pub mod liveness {
    //! Module containing utilities for managing the liveness socket.

    use std::{
        path::{Path, PathBuf},
        process::Command,
    };

    /// Environment variable used by [`super::declare_service`] as a means of communicating the liveness
    /// socket path.
    pub const LIVENESS_ENV_VAR: &str = "SUSS_LIVENESS_SOCKET_PATH";

    /// Ensure that, for the command given, the environment variable [`LIVENESS_ENV_VAR`] exists
    /// with the correct liveness socket path as passed to this function, or if the liveness path
    /// is None, ensures that the environment variable doesn't exist. This function is
    /// automatically used with [`super::declare_service`]
    ///
    /// On a service server, see [`retrieve_liveness_path`] for obtaining the liveness path from
    /// the environment and clearing the environment to avoid polluting child processes.
    pub fn set_liveness_environment<'c>(
        command: &'c mut Command,
        child_liveness_path_state: Option<&Path>,
    ) -> &'c mut Command {
        match child_liveness_path_state {
            Some(liveness_path) => command.env(LIVENESS_ENV_VAR, liveness_path.as_os_str()),
            None => command.env_remove(LIVENESS_ENV_VAR),
        }
    }

    /// Retrieve the liveness path from the environment in a server, and clear the environment of
    /// the current process to avoid accidentally leaking the liveness environment into any child
    /// processes started by the server.
    ///
    /// In your service declarations, use [`set_liveness_environment`] on your commands to
    /// configure this to work.
    pub fn retrieve_liveness_path() -> Option<PathBuf> {
        let path = std::env::var_os(LIVENESS_ENV_VAR).map(PathBuf::from);
        std::env::remove_var(LIVENESS_ENV_VAR);
        path
    }
}

/// Provide async_trait for convenience.
pub use async_trait::async_trait;
use chain_trans::Trans;
use cleanable_path::CleanablePathBuf;
pub use futures_lite::future;

use socket_shims::{DefaultUnixSocks, UnixSocketImplementation};
use std::{
    ffi::{OsStr, OsString},
    fmt::Debug,
    os::unix::net::UnixListener,
    path::Path,
};
use std::{io::Result as IoResult, os::unix::net::UnixStream, process::Child, time::Duration};
use timefut::with_timeout;
use tracing::{debug, error, info, instrument, trace, warn};

/// Trait used to define a single, startable service, with a relative socket path. For a more
/// concise way of implementing services, take a look at the [`declare_service`] and
/// [`declare_service_bundle`] macros that let you implement this trait far more concisely. For the
/// items emitted by service bundles, have a look at [`ReifiedService`], which embeds an executor
/// prefix and base context directory along with an abstract service implementation encoded by this
/// trait.
///
/// A service - in `suss` terminology - is a process that can be communicated with
/// through a [`std::os::unix::net::UnixStream`].
///
/// Services are run with a *base context path*, which acts as a runtime namespace and
/// allows multiple instances of collections of services without accidental interaction
/// between the groups - for instance, if you wanted a service to run once per user, you
/// could set the context directory as somewhere within $HOME or a per-user directory.
///
/// Services can also be provided an optional *executor prefix* - this is something that - in the
/// case of command execution to start a service, should be added to the start of all service
/// commands as the actual executable to run. This is useful for some cases:
/// * It may be useful to run anything with a prefix of /usr/bin/env in certain operating
///   environments like nixos
/// * It can be used to override implementations with a custom version of certain services
/// * It can be used to instrument services with some kind of notification or liveness system
///   outside of the one internally managed by this process
/// * Anything else you can think of, as long as it ends up delegating to something that actually
///   runs a valid service, or maybe fails if you want to conditionally prevent services from
///   functioning.
///
/// Each service has an associated *socket name*, which is a place in the *context base path*
/// that services put their receiver unix sockets. Services are checked for running-status by
/// if their associated socket files exist (and can be connected to) - i.e. they try to create a
/// `UnixStream` and if it fails, try to start the service.
///
/// Socket files, actually running the service, etc. are not handled by this trait. Instead, they
/// are handled by a [`ServerExt`], which takes care of things like cleaning up socket files
/// afterward automatically in [`Drop`]
#[async_trait(?Send)]
pub trait Service: Debug {
    /// A connection to the service server - must be generatable from a bare
    /// [`std::os::unix::net::UnixStream`]
    type ServiceClientConnection;

    /// Obtain the name of the socket file in the base context path. In your collection of
    /// services, the result should be unique, or you might end up with service collisions when
    /// trying to grab sockets.
    fn socket_name(&self) -> &std::ffi::OsStr;

    /// Convert a bare unix stream into a [`Self::ServiceClientConnection`]
    fn wrap_connection(&self, bare_stream: UnixStream) -> IoResult<Self::ServiceClientConnection>;

    /// This should *synchronously* attempt to start the service, with the given ephemeral liveness
    /// socket path passed through if present to that service - in [`declare_service!`], this is
    /// done with an environment variable.
    ///
    /// The provided executor argument list should be prefixed to any commandline executions if possible -
    /// it provides a convenient means of allowing replaceable and instrumentable services.
    ///
    /// The ephemeral socket path should be connected to and then immediately shut down
    /// by the running service process (this is handled automatically by [`ServiceExt`] if you
    /// use that to run your service).
    ///
    /// Ephemeral liveness check timeouts are applied by the library later on.
    fn run_service_command_raw(
        &self,
        executor_commandline_prefix: Option<&[impl AsRef<OsStr> + Sized + Debug]>,
        liveness_path: Option<&Path>,
    ) -> IoResult<Child>;

    /// This function is applied to the child process after it has passed the liveness check but
    /// before it has been connected to. In here you can add it to a threadpool or something if you want to
    /// .wait on it. Bear in mind it is an async function so don't block.
    ///
    /// The default version of this function will simply drop the child and leave it a zombie -
    /// this is desired if you want the services to be more persistent, but if you want to tie the
    /// lifetime of the service to the lifetime of the parent process, spawning a task that just
    /// .wait()s on the child or does some async equivalent may be sufficient. Well, it might also
    /// block your own process until the child dies but hey ho!, sort that out yourself :) - you
    /// probably want to use your runtime's equivalent of `spawn` for this.
    ///
    /// Of course this function, like the [`Self::run_service_command_raw`] function, are not used at all
    /// if the service already exists in base context directory.
    async fn after_post_liveness_subprocess(&self, _: Child) -> IoResult<()> {
        Ok(())
    }
}

/// Utility function to obtain a random path in [`std::env::tempdir`], of the form
/// `$tempdir/temp-XXXXXXXXXXXXXXXX.sock` (16 xs), where the x's are replaced by numbers
/// from 0-9a-f (hex)
fn get_random_sockpath() -> std::path::PathBuf {
    use nanorand::rand::{chacha::ChaCha20, Rng};
    let mut path = std::env::temp_dir();
    let mut gen = ChaCha20::new();
    // 1 byte => 2 chars
    // 16 chars => 8 bytes => 64 bits => u64
    path.push(format!("temp-{:016x}.sock", gen.generate::<u64>()));
    path
}

#[async_trait(?Send)]
pub trait ServiceExt: Service {
    /// Reify this [`Service`] into a [`ReifiedService`] that carries around necessary context for
    /// connecting to it.
    fn reify(self, base_context_directory: &Path) -> ReifiedService<'_, Self>
    where
        Self: Sized,
    {
        ReifiedService::reify_service(self, base_context_directory)
    }

    /// Reify this [`Service`] into a [`ReifiedService`] that carries around necessary context for
    /// connecting to it, including an executor prefix command.
    fn reify_with_executor<'i, EPC: AsRef<OsStr> + Sized + Debug>(
        self,
        base_context_directory: &'i Path,
        executor_prefix: &'i [EPC],
    ) -> ReifiedService<'i, Self, EPC>
    where
        Self: Sized,
    {
        ReifiedService::reify_service_with_executor(self, base_context_directory, executor_prefix)
    }

    /// Attempt to connect to an already running service. This will not try to start the service on
    /// failure - for that, see [`Self::connect_to_service`]
    ///
    /// See [`Service`] for information on base context directories.
    #[instrument]
    async fn connect_to_running_service(
        &self,
        base_context_directory: &Path,
    ) -> IoResult<<Self as Service>::ServiceClientConnection> {
        use crate::socket_shims::UnixSocketImplementation;
        let server_socket_path = base_context_directory.join(<Self as Service>::socket_name(self));
        info!(
            "Attempting connection to service @ {}",
            server_socket_path.display()
        );
        match DefaultUnixSocks::us_connect(&server_socket_path).await {
            Ok(non_std_unix_stream) => {
                info!("Successfully obtained async unix socket");
                trace!("Attempting conversion to std::os::unix::net::UnixStream");
                let std_unix_stream = DefaultUnixSocks::us_to_std(non_std_unix_stream)?;
                trace!("Wrapping into the final client connection...");
                self.wrap_connection(std_unix_stream)
            }
            Err(e) => {
                error!(
                    "Failed to connect to service @ {}",
                    server_socket_path.display()
                );
                Err(e)
            }
        }
    }

    /// Attempt to connect to the given service in the given runtime context directory.
    ///
    /// See [`Service`] for information on executor commandline prefixes and the base context
    /// directory.
    ///
    /// If the service is not already running, then `liveness_timeout` is the maximum time before a
    /// non-response to the liveness check will result in an error.
    #[instrument]
    async fn connect_to_service(
        &self,
        executor_commandline_prefix: Option<&[impl AsRef<OsStr> + Sized + Debug]>,
        base_context_directory: &Path,
        liveness_timeout: Duration,
    ) -> IoResult<<Self as Service>::ServiceClientConnection> {
        use socket_shims::UnixSocketImplementation;
        match self
            .connect_to_running_service(base_context_directory)
            .await
        {
            Ok(s) => Ok(s),
            Err(e) => {
                warn!("Error connecting to existing service - {} - attempting on-demand service start", e);
                let ephemeral_socket_path = CleanablePathBuf::new(get_random_sockpath());
                info!(
                    "Creating ephemeral liveness socket @ {}",
                    ephemeral_socket_path.as_ref().display()
                );
                let ephem = DefaultUnixSocks::ul_bind(ephemeral_socket_path.as_ref())
                    .await
                    .map_err(|e| {
                        error!(
                            "Couldn't create ephemeral liveness socket @ {} - {}",
                            ephemeral_socket_path.as_ref().display(),
                            e
                        );
                        e
                    })?;

                // We have an ephemeral socket, so begin running the child process, using `unblock`
                let child_proc = self
                    .run_service_command_raw(
                        executor_commandline_prefix,
                        Some(ephemeral_socket_path.as_ref()),
                    )
                    .map_err(|e| {
                        error!("Could not start child service process - {}", e);
                        e
                    })?;

                // Now wait for a liveness ping
                let mut temp_unix_stream = with_timeout(
                    DefaultUnixSocks::ul_try_accept_connection(&ephem),
                    liveness_timeout,
                )
                .await
                .unwrap_or_else(|| {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!(
                            "Timed out waiting for service to become live after {}",
                            humantime::format_duration(liveness_timeout)
                        ),
                    ))
                })
                .map_err(|e| {
                    error!(
                        "Failed to receive liveness ping for service on ephemeral socket {} - {}",
                        ephemeral_socket_path.as_ref().display(),
                        e
                    );
                    e
                })?;

                DefaultUnixSocks::us_shutdown(&mut temp_unix_stream).await?;
                drop(temp_unix_stream);
                drop(ephem);
                drop(ephemeral_socket_path);

                self.after_post_liveness_subprocess(child_proc).await?;
                info!("Successfully received ephemeral liveness ping - trying to connect to service again.");
                self.connect_to_running_service(base_context_directory)
                    .await
            }
        }
    }
}

impl<S: Service> ServiceExt for S {}

/// Server implementation for a [`Service`]
#[async_trait]
pub trait Server<S: Service>: Debug {
    /// Type that wraps a unix socket listener.
    type ListenerWrapper;
    type FinalOutput;

    /// Wrap a listening socket into a more structured form - for instance an API wrapper or
    /// something similar.
    ///
    /// This takes a raw [`UnixListener`]. Async frameworks should let you convert to and from
    /// standard library unix sockets.
    async fn wrap_listener_socket(
        &self,
        service: &S,
        socket: UnixListener,
    ) -> IoResult<Self::ListenerWrapper>;

    /// Run the server. Note that you don't need to worry about cleaning up the socket path - that's
    /// handled by the library.
    async fn run_server(
        &self,
        service: &S,
        wrapper: Self::ListenerWrapper,
    ) -> IoResult<Self::FinalOutput>;
}

/// Internal function to notify a liveness socket by connecting and then immediately disconnecting
/// :)
///
/// This will report any io errors but you probably don't care about those.
#[instrument]
async fn notify_liveness_socket(liveness_socket_path: &Path) -> IoResult<()> {
    let mut sock = DefaultUnixSocks::us_connect(liveness_socket_path).await
        .map_err(|e| {
            warn!("Couldn't connect to parent process's ephemeral liveness socket @ {} - error was: {}", liveness_socket_path.display(), e);
            e
        })?.trans_inspect(|_sock| {
            info!(
                "Ping'ed liveness socket @ {} with connection, shutting ephemeral connection.",
                liveness_socket_path.display()
            );
        });
    DefaultUnixSocks::us_shutdown(&mut sock).await
}

/// Extension trait that lets you run servers well
#[async_trait(?Send)]
pub trait ServerExt<S: Service>: Server<S> {
    /// Create the listener socket, notify the liveness socket, and when finally returning, clean the listener socket up after ourselves.
    ///
    /// ## Methods used to create sockets
    /// This function currently uses [`socket_shims::DefaultUnixSocks`] for managing asynchronous
    /// connections, and passes around standard library sockets which can be freely converted back
    /// and forth to async frameworks.
    ///
    /// ## Liveness Socket
    /// This library uses an ephemeral unix socket started by a parent process as a means to
    /// indicate that a service is live. This can be extracted from the environment created by the
    /// conventional means of service definition via [`liveness::retrieve_liveness_path`].
    ///
    /// The only thing necessary to indicate liveness is simply connecting to the socket (and then
    /// you can shut down the socket connection).
    ///
    /// In this implementation, the liveness socket is ping'd after the creation of a receiving
    /// socket at the standard path for the service. This is a protocol requirement - if you ping
    /// the liveness socket with a connection, it means that a socket exists to connect to.
    #[instrument]
    async fn start_and_run_server(
        &self,
        service: &S,
        context_base_path: &Path,
        liveness_socket_path: Option<&Path>,
    ) -> IoResult<Self::FinalOutput> {
        let socket_path: CleanablePathBuf = context_base_path.join(service.socket_name()).into();
        info!(
            "Obtaining socket on path {}",
            socket_path.as_ref().display()
        );
        let raw_listener_socket = DefaultUnixSocks::ul_bind(socket_path.as_ref()).await?;
        let _ = match liveness_socket_path {
            Some(p) => notify_liveness_socket(p).await,
            None => {
                info!("No liveness socket path provided, assuming autonomous.");
                Ok(())
            }
        };

        debug!("Wrapping raw socket in API");
        let api = self
            .wrap_listener_socket(service, DefaultUnixSocks::ul_to_std(raw_listener_socket)?)
            .await?;
        info!("Starting service @ {}", socket_path.as_ref().display());
        let res = self.run_server(service, api).await?;
        info!("Cleaning up socket @ {}", socket_path.as_ref().display());
        drop(socket_path);
        Ok(res)
    }
}

impl<S: Service, T: Server<S>> ServerExt<S> for T {}

#[derive(Debug)]
/// Holds a particular instance of a [`Service`], along with a base context directory and optional
/// executor prefix.
///
/// This lets you interact with services in a manner not requiring you to carry around
/// base_context_directories and executor_prefixes, and they are what [`ServiceBundle`]s produce
/// for you under the hood.
pub struct ReifiedService<
    'info,
    S: Service,
    ExecutorPrefixComponent: AsRef<OsStr> + Sized + Debug = OsString,
> {
    executor_prefix: Option<&'info [ExecutorPrefixComponent]>,
    base_context_directory: &'info Path,
    bare_service: S,
}

impl<'info, S: Service, ExecutorPrefixComponent: AsRef<OsStr> + Sized + Debug>
    ReifiedService<'info, S, ExecutorPrefixComponent>
{
    /// Reify a service into a specific base context directory
    pub fn reify_service(service: S, base_context_directory: &'info Path) -> Self {
        Self {
            executor_prefix: None,
            base_context_directory,
            bare_service: service,
        }
    }

    /// Reify a service along with an executor prefix.
    pub fn reify_service_with_executor(
        service: S,
        base_context_directory: &'info Path,
        executor_prefix: &'info [ExecutorPrefixComponent],
    ) -> Self {
        Self {
            executor_prefix: Some(executor_prefix),
            base_context_directory,
            bare_service: service,
        }
    }

    /// Connect to this [`Service`], trying to start it if not possible.
    ///
    /// The timeout is for how long to wait until concluding that - in the case we attempted to
    /// start a service because it wasn't running - the service failed to begin.
    ///
    /// If you don't care about starting the service on-demand, take a look at
    /// [`Self::connect_to_running`]
    #[instrument]
    pub async fn connect(
        &self,
        liveness_timeout: Duration,
    ) -> IoResult<S::ServiceClientConnection> {
        self.bare_service
            .connect_to_service(
                self.executor_prefix,
                self.base_context_directory,
                liveness_timeout,
            )
            .await
    }

    /// Connect to this [`Service`], without attempts to start it upon failure.
    ///
    /// If you want to try and start the service on-demand, take a look at [`Self::connect`]
    #[instrument]
    pub async fn connect_to_running(&self) -> IoResult<S::ServiceClientConnection> {
        self.bare_service
            .connect_to_running_service(self.base_context_directory)
            .await
    }

    #[instrument]
    /// Run an actual server for this service, with a provided implementation and optional [`liveness`]
    /// socket path.
    pub async fn serve_service_implementation<ServiceServer: ServerExt<S>>(
        &self,
        server: &ServiceServer,
        liveness_socket_path: Option<&Path>,
    ) -> IoResult<ServiceServer::FinalOutput> {
        server
            .start_and_run_server(
                &self.bare_service,
                self.base_context_directory,
                liveness_socket_path,
            )
            .await
    }
}

/// Trait implemented by "bundles" of services that all work together and call each other.
///
/// Provides a unified interface for applying *base context directories* and *executor commands* to
/// all of a collection of services, to then instantiate a defined service (these are inherent impl
/// methods on generated types).
pub trait ServiceBundle<ExecutorPrefixComponent: AsRef<OsStr> + Sized = OsString> {
    /// Create the service bundle with the given base context directory,
    fn new(base_context_directory: &Path) -> Self;

    /// Create the service bundle with the base context directory, along with an executor prefix
    fn with_executor_prefix(
        base_context_directory: &Path,
        executor_prefix: &[ExecutorPrefixComponent],
    ) -> Self;
}

#[macro_export]
/// A macro that aids in generating the common case of a service that has a command name and calls
/// out to a command.
///
/// This creates unit-type items that implement the [`Service`] trait, where the services are
/// started by standard [`std::process::Command`] execution - including correct executor prefix
/// implementation.
///
/// Wrapping [`std::os::unix::net::UnixStream`]s in higher-level abstractions can be specified in a
/// number of ways (in future, currently we only implement bare functions). These methods are
/// called *USP*s (**U**nix **S**tream **P**reprocessors), of which there is currently one (though
/// it should be able to implement any other with sufficient effort).
///
/// Using this macro goes something like the following:
///
/// ```rust,compile_fail
/// use suss::declare_service;
///
/// declare_service! {
///     /// My wonderful service
///     pub WonderfulService = {
///         "some-wonderful-command" "--and" "--commandline" "args" @ "unix-socket-filename.sock"
///         as some_usp_method some_usp_method_specifications
///     }
/// }
/// ```
///
/// Services are just unit types in this case, and can have any visibility you like and
/// documentation or other things like `#[derive]` on them as desired.
///
/// The first part of the definition controls what command to run to execute the service, and the
/// socket it will serve on. The ephemeral liveness socket, as described in
/// [`ServerExt::start_and_run_server`], is passed through via an environment variable.
///
/// The literal after the @ is the name of the socket within the *base context directory* that
/// this service hosts itself upon. For example, if your base context directory is `/var/run`, and
/// the socket name for a service is `hello-service.sock`, then the service should receive
/// connections on `/var/run/hello-service.sock`.
///
/// Note that there is *no easy way* to pass in the base context directory to the command. This is
/// a concious decision - this library is designed for *services*, not just *subprocesses*, and
/// hence other programs should be able to find a service via some method derived from the
/// environment.
///
/// If nothing else, storing a context directory in an environment variable will do
/// the trick, but the point is that generally the base context directory should be defined by
/// environment, whether that be `XDG`, or a global fixed directory, or an environment variable, or
/// any combination of the above or some other environmental context.
///
/// This defines how a service is started and how to locate it. The stuff after the *as* provides
/// information on what to do once you've got a connection.
///
/// ### Methods
///
/// #### Raw
///
/// The `raw` method is essentially an arbitrary function that takes a
/// [`std::os::unix::net::UnixStream`] and produces (wrapped in a [`std::io::Result`]), a
/// higher-level abstraction over the stream that the rest of the world will have access to.
///
/// ```rust,compile_fail
///  ...rest-of-arg... as raw |name_of_raw_std_unix_socket_variable| -> Io<abstracted_and_wrapped_connection_type> {
///     Ok(some_wrapped_type)
///  }
/// ```
macro_rules! declare_service {
    {
        $(#[$service_meta:meta])*
        $vis:vis $service_name:ident = {
            $command:literal $($args:literal)* @ $socket_name:literal
                as $unix_stream_preprocess_method:ident $($unix_stream_preprocess_spec:tt)*
        }
    } => {
        $(#[$service_meta])*
        #[derive(Debug)]
        $vis struct $service_name;

        impl $crate::Service for $service_name {
            type ServiceClientConnection = $crate::declare_service!(@socket_connection_type $unix_stream_preprocess_method $($unix_stream_preprocess_spec)*);

            #[inline]
            fn socket_name(&self) -> &::std::ffi::OsStr {
                ::std::ffi::OsStr::new($socket_name)
            }

            #[inline]
            fn wrap_connection(&self, bare_stream: ::std::os::unix::net::UnixStream) -> IoResult<Self::ServiceClientConnection> {
                $crate::declare_service!(@wrap_implementation bare_stream $unix_stream_preprocess_method $($unix_stream_preprocess_spec)*)
            }

            fn run_service_command_raw(
                &self,
                executor_commandline_prefix: ::core::option::Option<&[impl ::core::convert::AsRef<::std::ffi::OsStr> + ::std::fmt::Debug]>,
                liveness_path: ::core::option::Option<&::std::path::Path>,
            ) -> ::std::io::Result<::std::process::Child> {
                use ::std::{process::Command, iter::{Iterator, IntoIterator, once}, ffi::OsStr};
                use $crate::chain_trans::prelude::*;
                // Build an iterator out of all the CLI components and unconditionally take the
                // first. This ends up being generally simpler in the long run than trying to wrangle
                // matches and conditional inclusion of items.
                let mut all_components_iterator = executor_commandline_prefix
                    .map(|l| l.iter()).into_iter()
                    .flatten()
                    .map(::core::convert::AsRef::as_ref)
                    // This is the part that ensures that at least the first element always exists.
                    .chain(once(OsStr::new($command)))
                    // CLI args
                    .chain([$(OsStr::new($args)),*].into_iter());

                let program = all_components_iterator.next().expect("There must be at least one thing in the iterator - the program to run, itself.");
                Command::new(program)
                    .trans_mut(|cmd| { $crate::liveness::set_liveness_environment(cmd, liveness_path); })
                    .args(all_components_iterator)
                    .spawn()
            }
        }
    };
    // macro "method" for extracting the result type from the preprocess method and specification
    {@socket_connection_type raw |$unix_socket:ident| -> Io<$result:ty> $body:block } => { $result };
    // macro "method" for implementing the connection wrapper stuff
    {@wrap_implementation $stream_ident:ident raw |$unix_socket:ident| -> Io<$result:ty> $body:block} => {{
        let inner_closure = |$unix_socket| -> ::std::io::Result<$result> { $body };
        inner_closure($stream_ident)
    }};
}

#[macro_export]
/// This macro lets you create a service bundle, for unified initialisation of a collection of
/// services.
///
/// The generated structure implements [`ServiceBundle`] as well as providing methods for reifying
/// the services associated with it. Documentation for the services is attached to the service
/// type.
///
/// For documentation on how service definitions work, see [`declare_service`]. The only difference
/// is that instead of `pub ServiceTypeName = ` we have `pub service_bundle_function_name() -> ServiceTypeName = `
///
/// This is used like the following:
/// ```rust,compile_fail
/// use suss::{declare_service_bundle, ServiceBundle};
///
/// declare_service_bundle!{
///     /// Some wonderful docs
///     pub WonderfulServices {
///         /// This is a wonderful service that prints hello
///         pub fn wonderful_hello_service() -> WonderfulHelloService = { ... service_definition ... };
///         /// This is a wonderful crate-private service that echos back written data
///         pub(crate) fn wonderful_echo_service() -> WonderfulEchoService = { ... definition ... };
///     }
/// }
///
/// // If a service needs to be started, this is the time to wait before assuming there was an
/// // error if the liveness socket doesn't get pinged
/// let liveness_timeout = std::time::Duration::from_milli(500);
///
/// // Configure the shared runtime directory for all your services.
/// let wonderful_bundle = WonderfulServices::new("/fancy/rumtime/base/directory");
/// // Inside crate only
/// let echo_api = wonderful_bundle.wonderful_echo_service().connect(liveness_timeout).await?;
/// // public interface
/// let hello_api = wonderful_bundle.wonderful_hello_service().connect(liveness_timeout).await?;
/// // Try to connect to an already running service.
/// let hello_api_two = wonderful_bundle.wonderful_hello_service().connect_to_running().await?;
/// ```
macro_rules! declare_service_bundle {
    {
        $(#[$bundle_meta:meta])*
        $bundle_vis:vis $bundle_name:ident {$(
            $(#[$service_meta:meta])*
            $service_vis:vis fn $service_fn_name:ident () -> $service_type_name:ident = { $($service_definition:tt)*}
        );*}
    } => {

        $(#[$bundle_meta])*
        $bundle_vis struct $bundle_name {
            executor_prefix: ::core::option::Option::<::std::vec::Vec::<::std::ffi::OsString>>,
            base_context_path: ::std::path::PathBuf
        }

        impl $crate::ServiceBundle for $bundle_name {
            fn new(base_context_path: &::std::path::Path) -> Self {
                use ::std::borrow::ToOwned;
                Self {
                    base_context_path: base_context_path.to_owned(),
                    executor_prefix: ::core::option::Option::None
                }
            }

            fn with_executor_prefix(base_context_path: &::std::path::Path, executor_prefix: &[::std::ffi::OsString]) -> Self {
                use ::std::borrow::ToOwned;
                Self {
                    base_context_path: base_context_path.to_owned(),
                    executor_prefix: ::core::option::Option::Some(executor_prefix.to_owned())
                }
            }
        }


        // Implement service types by calling the declare_service! macro :)
        $(
            $crate::declare_service!{
                $(#[$service_meta])*
                $service_vis $service_type_name = { $($service_definition)* }
            }
        )*

        // Now create the reification functions on our service bundle :)
        impl $bundle_name {$(
            $service_vis fn $service_fn_name(&self) -> $crate::ReifiedService<'_, $service_type_name> {
                match &self.executor_prefix {
                    Some(ep) => $crate::ReifiedService::reify_service_with_executor($service_type_name, &self.base_context_path, ep.as_slice()),
                    None => $crate::ReifiedService::reify_service($service_type_name, &self.base_context_path)
                }
            }
        )*}
    }
}

/// Module for usually-necessary imports.
pub mod prelude {
    pub use super::{declare_service, declare_service_bundle, ServiceBundle};
}

#[cfg(test)]
mod tests {
    use std::env::temp_dir;

    use futures_lite::future::block_on;

    use super::*;

    #[test]
    pub fn service_declaration_and_start_fail_test() {
        use std::os::unix::net::UnixStream;
        let tmpdir = temp_dir();
        declare_service! {
            /// Basic test service
            pub TestService = {
                "sfdjfkosdgjsadgjlas" @ "test-service.sock" as raw |unix_socket| -> Io<UnixStream>  {
                    Ok(unix_socket)
                }
            }
        }

        assert!(block_on(
            TestService
                .reify(&tmpdir)
                .connect(Duration::from_millis(50))
        )
        .is_err());
    }

    #[test]
    pub fn service_bundle_macro_test() {
        declare_service_bundle! {
            pub TestBundle {
                /// some service
                pub fn echo_service() -> EchoService = {
                    "echo-executable-wekdasjkfgnjsd" "--and" "--some" "--args" @ "echo-service.sock" as raw |unix_socket| -> Io<UnixStream> { Ok(unix_socket) }
                };
                pub fn hello_service() -> HelloService = {
                    "hello-executable-fjskldgkjsagd" "a" @ "hello-service.sock" as raw |unix_socket| -> Io<UnixStream> { Ok(unix_socket) }
                }
            }
        }

        let tmpdir = temp_dir();
        let wonderful_bundle = TestBundle::new(&tmpdir);
        assert!(block_on(
            wonderful_bundle
                .echo_service()
                .connect(Duration::from_millis(50))
        )
        .is_err());
        assert!(block_on(
            wonderful_bundle
                .hello_service()
                .connect(Duration::from_millis(50))
        )
        .is_err())
    }
}

// suss - library for creating single, directory namespaced unix socket servers in a network
// Copyright (C) 2022  Matti Bryce <mattibryce@protonmail.com>

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published
// by the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.

// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

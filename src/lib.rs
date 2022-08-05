//! This is a library designed to ease the creation of a collection of singleton services namespaced to a single
//! base directory path with an arbitrary network of dependencies, starting services as needed,
//! using unix sockets as the communication mechanism.
//!
//! In the case that a service already exists, then this will communicate with the appropriate
//! socket, rather than start a new one.

mod cleanable_path;
pub mod mapfut;
pub mod socket_shims;
pub mod timefut;

/// Provide async_trait for convenience.
pub use async_trait::async_trait;
use cleanable_path::CleanablePathBuf;
pub use futures_lite::future;

use socket_shims::{DefaultUnixSocks, UnixSocketImplementation};
use std::{fmt::Debug, future::Future, os::unix::net::UnixListener, path::Path};
use std::{io::Result as IoResult, os::unix::net::UnixStream, process::Child, time::Duration};
use timefut::with_timeout;
use tracing::{error, info, instrument, trace, warn};

/// Trait used to define a single, startable service, with a relative socket path.
///
/// A service - in `suss` terminology - is a process that can be communicated with
/// through a [`std::os::unix::net::UnixStream`].
///
/// Services are run with a *base context path*, which acts as a runtime namespace and
/// allows multiple instances of collections of services without accidental interaction
/// between the groups - for instance, if you wanted a service to run once per user, you
/// could set the context directory as somewhere within $HOME or a per-user directory.
///
/// Each service has an associated *socket name*, which is a place in the *context base path*
/// that all services put their receiver unix sockets. Services are checked for running-status by
/// if their associated socket files exist (and can be connected to) - i.e. they try to create a
/// `UnixStream` and if it fails, try to start the service.
///
/// Socket files, actually running the service, etc. are not handled by this trait. Instead, they
/// are handled by a [ServerService], which takes care of things like cleaning up socket files
/// afterward automatically in [`Drop`]
#[async_trait]
pub trait Service: Debug {
    /// A connection to the service server - must be generatable from a bare
    /// [`std::os::unix::net::UnixStream`]
    type ServiceClientConnection: Send;

    /// Obtain the name of the socket file in the base context path. In your collection of
    /// services, the result should be unique, or you might end up with service collisions when
    /// trying to grab sockets.
    fn socket_name(&self) -> &std::ffi::OsStr;

    /// Convert a bare unix stream into a [`Self::ServiceClientConnection`]
    fn wrap_connection(&self, bare_stream: UnixStream) -> IoResult<Self::ServiceClientConnection>;

    /// This should *synchronously* attempt to start the service, with the given ephemeral liveness
    /// socket path passed through if present to that service (probably by way of a command line
    /// argument).
    ///
    /// Ephemeral liveness check timeouts are applied by the library later on.
    fn run_service_command_raw(&self, liveness_path: Option<&Path>) -> IoResult<Child>;

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

#[async_trait]
pub trait ServiceExt: Service {
    /// Attempt to connect to an already running service. This will not try to start the service on
    /// failure - for that, see [`Self::connect_to_service`]
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
    /// If the service is not already running, then `liveness_timeout` is the maximum time before a
    /// non-response to the liveness check will result in an error.
    #[instrument]
    async fn connect_to_service(
        &self,
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
                    .run_service_command_raw(Some(ephemeral_socket_path.as_ref()))
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
                }).map_err(|e| {
                    error!("Failed to receive liveness ping for service on ephemeral socket {} - {}", ephemeral_socket_path.as_ref().display(),  e);
                    e
                })?;

                DefaultUnixSocks::us_shutdown(&mut temp_unix_stream).await?;
                drop(temp_unix_stream);
                drop(ephem);
                drop(ephemeral_socket_path);

                self.after_post_liveness_subprocess(child_proc).await?;
                info!("Successfully received ephemeral liveness ping - trying to connect to service again.");
                self.connect_to_running_service(base_context_directory).await
            }
        }
    }
}

impl<S: Service> ServiceExt for S {}

/// Represents a running service on a [`UnixListener`]. The unix socket can be preprocessed and
/// wrapped in some other type that may encapsulate listening behaviour beyond bare socket
/// communication.
///
/// When this object is consumed, it will destroy the unix listener and delete the socket file
/// automatically.
pub struct ServerService<ServiceSpec: Service, SocketWrapper = UnixListener> {
    service: ServiceSpec,
    unix_listener_socket: SocketWrapper,
    socket_path: CleanablePathBuf,
}

impl <ServiceSpec: Service, SocketWrapper> Debug for ServerService<ServiceSpec, SocketWrapper> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerService")
            .field("service", &self.service)
            .field("socket_path", &self.socket_path)
            .finish_non_exhaustive()
    }
}

impl<ServiceSpec: Service, SocketWrapper> ServerService<ServiceSpec, SocketWrapper> {
    /// This attempts to initially open the unix socket with the name appropriate to the service
    /// (see [`Service`], in the current service namespace directory (the context base path). You
    /// must provide a means of converting a raw unix listener socket into a `SocketWrapper` that
    /// you can then use - this method can be fallible.
    #[instrument(skip(unix_listener_wrapping))]
    pub fn try_and_open_raw_socket(
        service: ServiceSpec,
        context_base_path: &Path,
        unix_listener_wrapping: impl FnOnce(UnixListener) -> IoResult<SocketWrapper>,
    ) -> IoResult<Self> {
        let socket_path: CleanablePathBuf = context_base_path.join(service.socket_name()).into();
        let raw_listener = UnixListener::bind(&socket_path)?;
        Ok(Self {
            service,
            unix_listener_socket: unix_listener_wrapping(raw_listener)?,
            socket_path,
        })
    }

    /// Runs an arbitrary async service server, consuming the service.
    ///
    /// Arguments:
    ///  * the service function runs on the initially provided socket wrapper, and returns some
    ///  arbitrary result.
    ///
    ///  * The liveness path is a component of the mechanism by which this library allows one
    ///  service to start another. When provided, it is a path to an ephemeral unix socket that the
    ///  parent service listens on for a single connection. The act of connecting to it indicates
    ///  that the main service socket is open - which is done during the creation of this structure.
    ///  This provides several uses:
    ///     * It means that the moment a connection is received by the parent service or process, it
    ///     can connect to this service, after starting the current service. This avoids things
    ///     like polling.
    ///     * It avoids PID data races in the case that a PID file is being used to indicate
    ///     liveness - a unique socket address prevents a new process starting after premature
    ///     termination, with the same PID, creating a PIDfile.
    ///  Being unable to connect to the liveness path is not an error - the parent process probably
    ///  unexpectedly died, but the processes involved are generally speaking "persistent on-demand".
    ///  
    ///  * `die_with_parent_prefailure` tells the server function to error out if a liveness path is
    ///  provided and yet the unix socket can't be connected to - probably something to do with the
    ///  parent process being dead. This ties the lifetime of this service to the lifetime of the  
    ///
    /// Uses [`socket_shims::DefaultUnixSocks`] for creating and managing any temporary sockets
    /// asynchronously.
    #[instrument(skip(the_service_function))]
    pub async fn run_server<T, F: Future<Output = IoResult<T>>>(
        self,
        the_service_function: impl FnOnce(SocketWrapper, ServiceSpec) -> F,
        liveness_path: Option<&Path>,
        die_with_parent_prefailure: bool,
    ) -> IoResult<T> {
        match liveness_path {
            Some(parent_socket_path) => {
                match DefaultUnixSocks::us_connect(parent_socket_path).await {
                    Ok(mut raw_socket) => {
                        info!("Ping'ed liveness socket @ {} with connection, shutting ephemeral connection.", parent_socket_path.display());
                        let _ = DefaultUnixSocks::us_shutdown(&mut raw_socket).await;
                    }
                    Err(ephemeral_error) => {
                        if die_with_parent_prefailure {
                            error!("Could not connect to parent process's ephemeral liveness socket @ {}", parent_socket_path.display());
                            return Err(ephemeral_error);
                        } else {
                            warn!("Couldn't connect to parent process's ephemeral liveness socket @ {} - continuing service anyway, error was: {}", parent_socket_path.display(), ephemeral_error);
                            drop(ephemeral_error);
                        }
                    }
                }
            }
            None => info!("No liveness path, assuming autonomous"),
        };
        let Self {
            service,
            unix_listener_socket,
            socket_path,
        } = self;

        let res = the_service_function(unix_listener_socket, service).await;
        drop(socket_path);
        res
    }
}

/// Module for usually-necessary imports.
pub mod prelude {
    pub use super::{Service, ServiceExt, ServerService};
}

#[cfg(test)]
mod tests {
    use super::*;
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

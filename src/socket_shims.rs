//! Provide semi-unified interface to unix sockets api for arbitrary different async runtimes or
//! perhaps actually-sync-under-the-hood interfaces. 
use std::{net::Shutdown, path::Path};

use super::IoResult;
use async_trait::async_trait;
use blocking::unblock;



#[async_trait(?Send)]
/// Provide a unified interface to unix sockets in various points of existence.
///
/// Note that this is very unideal... In future, I am likely to do a couple things:
/// * Move the common interface out into a crate
/// * Make the trait delegate to `poll` functions and add a crapton of Future structs for the
///   various functions, to avoid the allocations involved in [`async_trait`]. Maybe even see if I
///   can't make a macro for that.
pub trait UnixSocketInterface {
    /// Unix stream type - equivalent to [`std::os::unix::net::UnixStream`]
    type UnixStream;
    /// Unix listener type - equivalent to [`std::os::unix::net::UnixListener`]
    type UnixListener;
    /// Unix socket address type - equivalent to [`std::os::unix::net::SocketAddr`]
    type SocketAddr;

    /// Attempt to connect to a stream. Analogous to [`std::os::unix::net::UnixStream::connect`]
    async fn unix_stream_connect(socket_path: impl AsRef<Path>) -> IoResult<Self::UnixStream>;

    /// Attempt to shutdown a stream. Because shutting down read vs write directions is not
    /// available in all runtimes - *looks @ tokio* - it is necessary for this to be kind of
    /// barebones.
    ///
    /// This will do as much shutdown as it is possible to do with the API. 
    async fn unix_stream_shutdown(s: &mut Self::UnixStream) -> IoResult<()>;  

    /// Write a [`u8`] slice to the unix stream, returning the # of actual bytes written.
    ///
    /// Ok(0) means it prob cant take anymore.
    async fn unix_stream_write(s: &mut Self::UnixStream, buf: &[u8]) -> IoResult<usize>;
    
    /// Write a [`u8`] slice to the unix stream, repeating until it actually works all the way.
    async fn unix_stream_write_all(s: &mut Self::UnixStream, buf: &[u8]) -> IoResult<()>;
    
    /// Read a [`u8`] slice into the buffer, returning the # of actual bytes read
    ///
    /// Ok(0) means it prob cant take anymore out of the stream for now
    async fn unix_stream_read(s: &mut Self::UnixStream, buf: &mut [u8]) -> IoResult<usize>;
    
    /// Read a [`u8`] slice from the unix stream, repeating until it reads everything.
    async fn unix_stream_read_all(s: &mut Self::UnixStream, buf: &mut [u8]) -> IoResult<()>;

    /// Bind as a unix listening socket at the path
    async fn unix_listener_bind(path: impl AsRef<Path>) -> IoResult<Self::UnixListener>;

    /// Accept one new connection
    async fn unix_listener_accept(s: &Self::UnixListener) -> IoResult<(Self::UnixListener, Self::SocketAddr)>;
}





// The part where we select the "default" unix socks barebones common interface.
// Priority for now is:
// * async-std
// * tokio
// * std fallback
// #[cfg(feature = "async-std")]
// pub type DefaultUnixSocks = AsyncStdUnixSocks;
// #[cfg(all(feature = "tokio", not(feature = "async-std")))]
// pub type DefaultUnixSocks = TokioUnixSocks;
// #[cfg(not(any(feature = "tokio", feature = "async-std")))]
// pub type DefaultUnixSocks = FallbackSyncUnixSocks;

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

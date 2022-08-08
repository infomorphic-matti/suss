//! Provide semi-unified interface to unix sockets api for arbitrary different async runtimes or
//! perhaps actually-sync-under-the-hood interfaces.
use std::{net::Shutdown, path::Path};

use super::IoResult;
use async_trait::async_trait;
use blocking::{unblock, Unblock};

#[async_trait(?Send)]
/// Provide a unified interface to unix sockets in various points of existence. You can provide
/// your own version of this in future if you have a runtime that is not supported.
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
    async fn unix_listener_accept(
        s: &mut Self::UnixListener,
    ) -> IoResult<(Self::UnixStream, Self::SocketAddr)>;
}

#[cfg(feature = "async-std")]
#[derive(Debug, Clone, Copy)]
/// Implementation of unix sockets using the [`async_std`] primitives.
pub struct AsyncStdUSocks;

#[cfg(feature = "async-std")]
use async_std::os::unix::net as async_std_us;

#[cfg(feature = "async-std")]
#[async_trait(?Send)]
impl UnixSocketInterface for AsyncStdUSocks {
    type UnixStream = async_std_us::UnixStream;
    type UnixListener = async_std_us::UnixListener;
    type SocketAddr = async_std_us::SocketAddr;

    async fn unix_stream_connect(socket_path: impl AsRef<Path>) -> IoResult<Self::UnixStream> {
        Self::UnixStream::connect(socket_path.as_ref()).await
    }

    async fn unix_stream_shutdown(s: &mut Self::UnixStream) -> IoResult<()> {
        s.shutdown(Shutdown::Both)
    }

    async fn unix_stream_write(s: &mut Self::UnixStream, buf: &[u8]) -> IoResult<usize> {
        use async_std::io::WriteExt;
        s.write(buf).await
    }

    async fn unix_stream_write_all(s: &mut Self::UnixStream, buf: &[u8]) -> IoResult<()> {
        use async_std::io::WriteExt;
        s.write_all(buf).await
    }

    async fn unix_stream_read(s: &mut Self::UnixStream, buf: &mut [u8]) -> IoResult<usize> {
        use async_std::io::ReadExt;
        s.read(buf).await
    }

    async fn unix_stream_read_all(s: &mut Self::UnixStream, buf: &mut [u8]) -> IoResult<()> {
        use async_std::io::ReadExt;
        s.read_exact(buf).await
    }

    async fn unix_listener_bind(path: impl AsRef<Path>) -> IoResult<Self::UnixListener> {
        Self::UnixListener::bind(path.as_ref()).await
    }

    async fn unix_listener_accept(
        s: &mut Self::UnixListener,
    ) -> IoResult<(Self::UnixStream, Self::SocketAddr)> {
        s.accept().await
    }
}

#[cfg(feature = "tokio")]
pub struct TokioUSocks;

#[cfg(feature = "tokio")]
use tokio::net as tokio_us;

#[cfg(feature = "tokio")]
#[async_trait(?Send)]
impl UnixSocketInterface for TokioUSocks {
    type UnixStream = tokio_us::UnixStream;
    type UnixListener = tokio_us::UnixListener;
    type SocketAddr = tokio_us::unix::SocketAddr;

    async fn unix_stream_connect(socket_path: impl AsRef<Path>) -> IoResult<Self::UnixStream> {
        Self::UnixStream::connect(socket_path.as_ref()).await
    }

    async fn unix_stream_shutdown(s: &mut Self::UnixStream) -> IoResult<()> {
        use tokio::io::AsyncWriteExt;
        s.shutdown().await
    }

    async fn unix_stream_write(s: &mut Self::UnixStream, buf: &[u8]) -> IoResult<usize> {
        use tokio::io::AsyncWriteExt;
        s.write(buf).await
    }

    async fn unix_stream_write_all(s: &mut Self::UnixStream, buf: &[u8]) -> IoResult<()> {
        use tokio::io::AsyncWriteExt;
        s.write_all(buf).await
    }

    async fn unix_stream_read(s: &mut Self::UnixStream, buf: &mut [u8]) -> IoResult<usize> {
        use tokio::io::AsyncReadExt;
        s.read(buf).await
    }

    async fn unix_stream_read_all(s: &mut Self::UnixStream, buf: &mut [u8]) -> IoResult<()> {
        use tokio::io::AsyncReadExt;
        // For some reason tokio read_exact returns a usize even though it just fills the buffer.
        // We discard it.
        s.read_exact(buf).await.map(|_| ())
    }

    async fn unix_listener_bind(path: impl AsRef<Path>) -> IoResult<Self::UnixListener> {
        Self::UnixListener::bind(path.as_ref())
    }

    async fn unix_listener_accept(
        s: &mut Self::UnixListener,
    ) -> IoResult<(Self::UnixStream, Self::SocketAddr)> {
        s.accept().await
    }
}

/// Uses [`blocking::unblock`] and `blocking::Unblock` to avoid blocking async threads. This is
/// simple and it will work with a crate like [`pollster`] if you don't care about async stuff -
/// that should avoid pulling in lots of heavier dependencies. You could also use something like
/// [`smol`] for a lightweight async runtime if you want a little async, but not the heavyweights
/// of [`async_std`] or [`tokio`]
pub struct StdThreadpoolUSocks;

use std::os::unix::net as std_us;

#[async_trait(?Send)]
impl UnixSocketInterface for StdThreadpoolUSocks {
    type UnixStream = Unblock<std_us::UnixStream>;
    type UnixListener = Unblock<std_us::UnixListener>;
    type SocketAddr = std_us::SocketAddr;

    async fn unix_stream_connect(socket_path: impl AsRef<Path>) -> IoResult<Self::UnixStream> {
        let pathref_for_thread_sharing = socket_path.as_ref();
        unblock(|| std_us::UnixStream::connect(pathref_for_thread_sharing))
            .await
            .map(|stream| Unblock::new(stream))
    }

    async fn unix_stream_shutdown(s: &mut Self::UnixStream) -> IoResult<()> {
        s.with_mut(|inner_sock| inner_sock.shutdown(Shutdown::Both))
            .await
    }

    async fn unix_stream_write(s: &mut Self::UnixStream, buf: &[u8]) -> IoResult<usize> {
        use futures_lite::AsyncWriteExt;
        s.write(buf).await
    }

    async fn unix_stream_write_all(s: &mut Self::UnixStream, buf: &[u8]) -> IoResult<()> {
        use futures_lite::AsyncWriteExt;
        s.write_all(buf).await
    }

    async fn unix_stream_read(s: &mut Self::UnixStream, buf: &mut [u8]) -> IoResult<usize> {
        use futures_lite::AsyncReadExt;
        s.read(buf).await
    }

    async fn unix_stream_read_all(s: &mut Self::UnixStream, buf: &mut [u8]) -> IoResult<()> {
        use futures_lite::AsyncReadExt;
        s.read_exact(buf).await
    }

    async fn unix_listener_bind(path: impl AsRef<Path>) -> IoResult<Self::UnixListener> {
        let pathref_for_thread_sharing = path.as_ref();
        unblock(|| std_us::UnixListener::bind(pathref_for_thread_sharing))
            .await
            .map(|listener| Unblock::new(listener))
    }

    async fn unix_listener_accept(
        s: &mut Self::UnixListener,
    ) -> IoResult<(Self::UnixStream, Self::SocketAddr)> {
        // Have to wait for an accepted connection, then wrap in Unblock
        s.with_mut(|listener| listener.accept())
            .await
            .map(|(connection, addr)| (Unblock::new(connection), addr))
    }
}

// The part where we select the "default" unix socks barebones common interface.
// Priority for now is:
// * async-std
// * tokio
// * std fallback
#[cfg(feature = "async-std")]
pub type DefaultUnixSocks = AsyncStdUSocks;
#[cfg(all(feature = "tokio", not(feature = "async-std")))]
pub type DefaultUnixSocks = TokioUSocks;
#[cfg(not(any(feature = "tokio", feature = "async-std")))]
pub type DefaultUnixSocks = StdThreadpoolUSocks;

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

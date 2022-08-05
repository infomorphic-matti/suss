//! Module that contains implementations of a unified asynchronous unix socket API, falling back on
//! synchronous [`std::os::unix::net`] functions and types when using an unfamiliar async runtime
//! (or no async runtime at all!).

use std::{net::Shutdown, path::Path};

use super::IoResult;
use async_trait::async_trait;
use blocking::unblock;

/// Types implementing this provide a runtime-specific unix socket implementation.
///
/// This library does not need any complexity around actually sending anything, and the
/// messiness involved in that is not very fun, so we only care about connection and shutdown!
#[async_trait]
pub trait UnixSocketImplementation {
    type UnixStream;
    type UnixListener;

    /// Get a unix stream by connecting to a path.
    async fn us_connect(path: &Path) -> IoResult<Self::UnixStream>;
    /// Shutdown a stream connection. Not all APIs allow for direction control,
    /// so there is none here.
    async fn us_shutdown(v: &mut Self::UnixStream) -> IoResult<()>;

    /// Convert a [`std::os::unix::net::UnixStream`] into the native unix stream type
    fn us_from_std(stdunix: std::os::unix::net::UnixStream) -> IoResult<Self::UnixStream>;
    /// Inverse of [`Self::us_from_std`]
    fn us_to_std(nativeunix: Self::UnixStream) -> IoResult<std::os::unix::net::UnixStream>;

    /// Bind a unix listener socket to an address (path)
    async fn ul_bind(path: &Path) -> IoResult<Self::UnixListener>;

    /// Listen for a single new connection. Socket address is not used, because different APIs
    /// have different types and its annoying (well, tokio has mio and the rest use std's)
    async fn ul_try_accept_connection(v: &Self::UnixListener) -> IoResult<Self::UnixStream>;
    
    /// Convert a [`std::os::unix::net::UnixListener`] into the native unix stream type
    fn ul_from_std(stdunix: std::os::unix::net::UnixListener) -> IoResult<Self::UnixListener>;
    /// Inverse of [`Self::ul_from_std`]
    fn ul_to_std(nativeunix: Self::UnixListener) -> IoResult<std::os::unix::net::UnixListener>;
}

#[cfg(feature = "tokio")]
/// Implementations of ultrabarebones unix sockets using tokio-native APIs
pub struct TokioUnixSocks;

#[cfg(feature = "tokio")]
#[async_trait]
impl UnixSocketImplementation for TokioUnixSocks {
    type UnixStream = tokio::net::UnixStream;
    type UnixListener = tokio::net::UnixListener;

    async fn us_connect(path: &Path) -> IoResult<Self::UnixStream> {
        Self::UnixStream::connect(path).await
    }

    async fn us_shutdown(v: &mut Self::UnixStream) -> IoResult<()> {
        <Self::UnixStream as tokio::io::AsyncWriteExt>::shutdown(v).await
    }
    
    fn us_from_std(stdunix: std::os::unix::net::UnixStream) -> IoResult<Self::UnixStream> { Self::UnixStream::from_std(stdunix) }
    fn us_to_std(nativeunix: Self::UnixStream) -> IoResult<std::os::unix::net::UnixStream> { nativeunix.into_std() }

    async fn ul_bind(path: &Path) -> IoResult<Self::UnixListener> {
        Self::UnixListener::bind(path)
    }

    async fn ul_try_accept_connection(v: &Self::UnixListener) -> IoResult<Self::UnixStream> {
        v.accept().await.map(|(stream, _)| stream)
    }
    
    fn ul_from_std(stdunix: std::os::unix::net::UnixListener) -> IoResult<Self::UnixListener> { Self::UnixListener::from_std(stdunix) }
    fn ul_to_std(nativeunix: Self::UnixListener) -> IoResult<std::os::unix::net::UnixListener> { nativeunix.into_std() }
}

#[cfg(feature = "async-std")]
/// Implementation of ultrabarebones unix sockets using async-std native APIs
pub struct AsyncStdUnixSocks;

#[cfg(feature = "async-std")]
#[async_trait]
impl UnixSocketImplementation for AsyncStdUnixSocks {
    type UnixStream = async_std::os::unix::net::UnixStream;
    type UnixListener = async_std::os::unix::net::UnixListener;

    async fn us_connect(path: &Path) -> IoResult<Self::UnixStream> {
        Self::UnixStream::connect(path).await
    }

    async fn us_shutdown(v: &mut Self::UnixStream) -> IoResult<()> {
        v.shutdown(Shutdown::Both)
    }
    
    fn us_from_std(stdunix: std::os::unix::net::UnixStream) -> IoResult<Self::UnixStream> { Ok(Self::UnixStream::from(stdunix)) }
    fn us_to_std(nativeunix: Self::UnixStream) -> IoResult<std::os::unix::net::UnixStream> { nativeunix.try_into() }

    async fn ul_bind(path: &Path) -> IoResult<Self::UnixListener> {
        Self::UnixListener::bind(path).await
    }

    async fn ul_try_accept_connection(v: &Self::UnixListener) -> IoResult<Self::UnixStream> {
        v.accept().await.map(|(stream, _)| stream)
    }
    
    fn ul_from_std(stdunix: std::os::unix::net::UnixListener) -> IoResult<Self::UnixListener> { Ok(Self::UnixListener::from(stdunix)) }
    fn ul_to_std(nativeunix: Self::UnixListener) -> IoResult<std::os::unix::net::UnixListener> { nativeunix.try_into() }
}

/// Falls back to using the synchronous std unix sockets on a threadpool (provided by [`unblock`])
pub struct FallbackSyncUnixSocks;

#[async_trait]
impl UnixSocketImplementation for FallbackSyncUnixSocks {
    type UnixStream = std::os::unix::net::UnixStream;
    type UnixListener = std::os::unix::net::UnixListener;

    async fn us_connect(path: &Path) -> IoResult<Self::UnixStream> {
        // [unblock] requires a 'static function to be passed to it, and unfortunately at the
        // minute there is not a runtime-agnostic scoped async threadpool so we can't fix the
        // 'static requirement. Therefore we simply clone().
        let op = path.to_owned();
        unblock(move || Self::UnixStream::connect(op)).await
    }

    async fn us_shutdown(v: &mut Self::UnixStream) -> IoResult<()> {
        // Note that we do not need to "unblock" here - async_std i dont think does either, so
        // we won't
        v.shutdown(Shutdown::Both)
    }

    fn us_from_std(stdunix: std::os::unix::net::UnixStream) -> IoResult<Self::UnixStream> { Ok(stdunix) }
    fn us_to_std(nativeunix: Self::UnixStream) -> IoResult<std::os::unix::net::UnixStream> { Ok(nativeunix) }

    async fn ul_bind(path: &Path) -> IoResult<Self::UnixListener> {
        // [unblock] requires a 'static function to be passed to it, and unfortunately at the
        // minute there is not a runtime-agnostic scoped async threadpool so we can't fix the
        // 'static requirement. Therefore we simply clone().
        let op = path.to_owned();
        unblock(move || Self::UnixListener::bind(op)).await
    }

    async fn ul_try_accept_connection(v: &Self::UnixListener) -> IoResult<Self::UnixStream> { 
        // [unblock] requires a 'static function to be passed to it, and unfortunately at the
        // minute there is not a runtime-agnostic scoped async threadpool so we can't fix the
        // 'static requirement. Therefore we simply try and duplicate the unix listener handle.
        let duplicated_listener_handle = v.try_clone()?;
        unblock(move || duplicated_listener_handle.accept()).await.map(|(stream, _)| stream)
    }
    
    fn ul_from_std(stdunix: std::os::unix::net::UnixListener) -> IoResult<Self::UnixListener> { Ok(stdunix) }
    fn ul_to_std(nativeunix: Self::UnixListener) -> IoResult<std::os::unix::net::UnixListener> { Ok(nativeunix) }
}

// The part where we select the "default" unix socks barebones common interface.
// Priority for now is:
// * async-std
// * tokio
// * std fallback
#[cfg(feature = "async-std")]
pub type DefaultUnixSocks = AsyncStdUnixSocks;
#[cfg(all(feature = "tokio", not(feature = "async-std")))]
pub type DefaultUnixSocks = TokioUnixSocks;
#[cfg(not(any(feature = "tokio", feature = "async-std")))]
pub type DefaultUnixSocks = FallbackSyncUnixSocks;



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

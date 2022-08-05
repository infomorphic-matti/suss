//! Like [`crate::socket_shims`], but allows the creation of a timeout instead, falling back to an `unblock`
//! in the worst case.

use std::{future::Future, time::Duration};

#[cfg(feature = "async-std")]
pub async fn async_std_sleep(time: Duration) {
    async_std::task::sleep(time).await
}
#[cfg(feature = "tokio")]
pub async fn tokio_sleep(time: Duration) {
    tokio::time::sleep(time).await
}

pub async fn std_sleep(time: Duration) {
    blocking::unblock(move || std::thread::sleep(time)).await
}

#[cfg(feature = "async-std")]
/// Sleep for the alloted period of time. Uses async runtime environments where possible.
pub async fn sleep(time: Duration) {
    async_std_sleep(time).await
}

#[cfg(all(feature = "tokio", not(feature = "async-std")))]
/// Sleep for the alloted period of time. Uses async runtime environments where possible.
pub async fn sleep(time: Duration) {
    tokio_sleep(time).await
}

#[cfg(not(any(feature = "tokio", feature = "async-std")))]
/// Sleep for the alloted period of time. Uses async runtime environments where possible.
pub async fn sleep(time: Duration) {
    std_sleep(time).await
}

/// Run the result of the future until either it completes, or a certain period of time has
/// passed. In the second case, return [None], while in the first case return [Some] with the
/// output of the future in place.
///
/// If the future is ready at the same time the timeout expires, [Some] is returned.
pub async fn with_timeout<T>(
    inner_future: impl Future<Output = T>,
    timeout: Duration,
) -> Option<T> {
    use crate::future::FutureExt;
    use crate::mapfut::map_fut;
    map_fut(inner_future, Some)
        .or(map_fut(sleep(timeout), |_| None))
        .await
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

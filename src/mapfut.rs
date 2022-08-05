//! Simple module for mapping the result of a future in-place.

use std::future::Future;

/// Takes a future and a map, and maps the result of that future with a function. 
pub async fn map_fut<T, Q>(f: impl Future<Output = T>, t: impl FnOnce(T) -> Q) -> Q {
    t(f.await)
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

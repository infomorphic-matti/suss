//! Path that gets removed from the file system when dropped.

use std::{
    borrow::Borrow,
    path::{Path, PathBuf},
};

/// Path that has [`std::fs::remove_file`] called on drop - used to clean up sockets.
///
/// Cannot be mutated once made, due to risk of not cleaning up the original path pre-mutation
#[derive(Debug)]
pub(crate) struct CleanablePathBuf(PathBuf);

impl CleanablePathBuf {
    /// Make sure the path at the provided location - if it has a file - is deleted.
    #[inline]
    pub fn new(p: PathBuf) -> Self {
        Self(p)
    }
}

impl From<PathBuf> for CleanablePathBuf {
    #[inline]
    fn from(p: PathBuf) -> Self {
        Self::new(p)
    }
}

impl AsRef<Path> for CleanablePathBuf {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Borrow<Path> for CleanablePathBuf {
    fn borrow(&self) -> &Path {
        &self.0
    }
}

impl Drop for CleanablePathBuf {
    #[allow(unused_must_use)]
    fn drop(&mut self) {
        std::fs::remove_file(&self.0);
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

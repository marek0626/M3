#![no_std]

use m3core::errors::Error;

mod genericfile;
mod m3fs;

pub use genericfile::*;

pub mod client {
    pub use crate::m3fs::*;
}

pub fn vfs_init() -> Result<(), Error> {
    m3core::vfs::register_file_type(
        crate::genericfile::GENERIC_FILE_MAGIC,
        crate::genericfile::GenericFile::unserialize,
    )
    .expect("Couldn't install GenericFile handler.");

    m3core::vfs::register_fs_type(
        crate::m3fs::M3FS_MAGIC,
        crate::m3fs::M3FS::new,
        crate::m3fs::M3FS::unserialize,
    )
    .expect("Couldn't install M3FS context.");
    Ok(())
}

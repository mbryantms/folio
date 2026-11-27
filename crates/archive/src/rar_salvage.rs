//! Reading a RAR entry whose data fails its checksum.
//!
//! UnRAR decodes an entry and checks its CRC only at the end. The `unrar`
//! crate collects the decoded bytes through UnRAR's data callback and drops
//! them when the check fails, so a page with one flipped bit is lost
//! whole. This module makes the same calls into the same linked UnRAR
//! library (`unrar_sys`, the crate's own FFI layer) and keeps those bytes,
//! saying they failed the check.
//!
//! **Why keeping them is safe.** A CRC catches accidental damage; it is
//! not an integrity guarantee against a hostile file, whose author can
//! simply give bad bytes a good checksum. So bytes that fail it are no less
//! trustworthy than any other page bytes: they reach the same image
//! decoders under the same limits. What is *not* relaxed: the entry is
//! found by its header (which UnRAR did read and check), its declared
//! unpacked size is held to [`crate::ArchiveLimits::max_entry_bytes`]
//! before anything is decoded, the callback refuses to collect a byte past
//! that declared size, encrypted entries and other volumes are refused,
//! and any error other than UnRAR's "bad data" is still an error.
//!
//! The unsafe surface is the four calls (open, read header, process, close)
//! plus the callback. The callback only appends to a `Vec` it is handed
//! through `user_data` and never panics; the handle is closed by a guard on
//! every path.
//!
//! Only Linux (and NetBSD) pass the archive name as bytes, the way the
//! crate does there; elsewhere [`salvage_entry`] answers `Ok(None)` and the
//! caller keeps the original read error.
#![allow(unsafe_code)]

use crate::{ArchiveError, entry_name::validate as sanitize_entry_name};
use std::path::Path;

/// The bytes UnRAR decoded for one entry.
#[derive(Debug)]
pub(crate) struct Salvaged {
    pub(crate) bytes: Vec<u8>,
    /// The entry's declared unpacked size.
    pub(crate) declared_size: u64,
    /// Whether UnRAR reported the entry's data as damaged (checksum or
    /// decoder failure). `false` means it read cleanly on this pass.
    pub(crate) damaged: bool,
}

/// Decode the entry whose canonical name (see [`crate::entry_name`]) is
/// `want`, keeping its bytes when only its data is damaged. `Ok(None)`
/// when no such entry exists (or on a platform this does not support).
/// `max_entry_bytes` bounds the declared size before decoding starts.
#[cfg(any(target_os = "linux", target_os = "netbsd"))]
pub(crate) fn salvage_entry(
    rar_name: &Path,
    want: &str,
    max_entry_bytes: u64,
) -> Result<Option<Salvaged>, ArchiveError> {
    use unrar_sys as native;

    let c_name = std::ffi::CString::new(rar_name.as_os_str().as_encoded_bytes())
        .map_err(|_| ArchiveError::Malformed("cbr open: nul in archive name".to_owned()))?;
    let mut open = native::OpenArchiveDataEx::new(c_name.as_ptr(), native::RAR_OM_EXTRACT);
    // SAFETY: `open` and the name it points at outlive the call; UnRAR
    // only reads them (and writes `open_result`).
    let raw = unsafe { native::RAROpenArchiveEx(&mut open as *mut _) };
    if raw.is_null() || open.open_result != native::ERAR_SUCCESS as u32 {
        if !raw.is_null() {
            // SAFETY: a non-null handle UnRAR returned, closed once.
            unsafe { native::RARCloseArchive(raw) };
        }
        return Err(ArchiveError::Malformed(format!(
            "cbr open: UnRAR error {}",
            open.open_result
        )));
    }
    let handle = Handle(raw);
    if open.flags & native::ROADF_VOLUME != 0 {
        return Err(ArchiveError::Malformed(
            crate::cbr::MULTI_VOLUME_MESSAGE.to_owned(),
        ));
    }

    let mut headers_read = 0usize;
    loop {
        let mut header = Box::<native::HeaderDataEx>::default();
        // SAFETY: the handle is open; `header` is a full `HeaderDataEx`.
        let rc = unsafe { native::RARReadHeaderEx(handle.0, &mut *header as *mut _) };
        match rc {
            native::ERAR_SUCCESS => {}
            native::ERAR_END_ARCHIVE => return Ok(None),
            // The damaged trailing header `cbr` already reads past.
            native::ERAR_BAD_DATA if headers_read > 0 => return Ok(None),
            other => {
                return Err(ArchiveError::Malformed(format!(
                    "cbr header: UnRAR error {other}"
                )));
            }
        }
        headers_read += 1;
        let raw_name = wide_to_string(&header.filename_w);
        let canonical = sanitize_entry_name(&raw_name)
            .map(|s| s.canonical)
            .unwrap_or_else(|_| raw_name.to_ascii_lowercase());
        let is_dir = header.flags & native::RHDF_DIRECTORY != 0;
        if is_dir || canonical != want {
            // SAFETY: the handle is positioned on this header.
            let rc = unsafe {
                native::RARProcessFile(
                    handle.0,
                    native::RAR_SKIP,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            };
            if rc != native::ERAR_SUCCESS {
                return Err(ArchiveError::Malformed(format!(
                    "cbr skip: UnRAR error {rc}"
                )));
            }
            continue;
        }
        if header.flags & native::RHDF_ENCRYPTED != 0 {
            return Err(ArchiveError::Encrypted);
        }
        if header.flags & (native::RHDF_SPLITBEFORE | native::RHDF_SPLITAFTER) != 0 {
            return Err(ArchiveError::Malformed(
                crate::cbr::MULTI_VOLUME_MESSAGE.to_owned(),
            ));
        }
        let declared_size = (u64::from(header.unp_size_high) << 32) | u64::from(header.unp_size);
        if declared_size > max_entry_bytes {
            return Err(ArchiveError::CapExceeded(crate::Cap::new(
                crate::SafetyCheck::EntrySize,
                "entry size",
            )));
        }
        let mut sink = Sink {
            bytes: Vec::with_capacity(usize::try_from(declared_size).unwrap_or(0)),
            limit: declared_size,
            overflow: false,
        };
        // SAFETY: `sink` lives on this frame until after the callback is
        // cleared below; UnRAR calls `collect` only inside RARProcessFile.
        let rc = unsafe {
            native::RARSetCallback(
                handle.0,
                Some(collect),
                &mut sink as *mut Sink as native::LPARAM,
            );
            let rc = native::RARProcessFile(
                handle.0,
                native::RAR_TEST,
                std::ptr::null(),
                std::ptr::null(),
            );
            native::RARSetCallback(handle.0, None, 0);
            rc
        };
        if sink.overflow {
            return Err(ArchiveError::CapExceeded(crate::Cap::new(
                crate::SafetyCheck::EntrySize,
                "entry size",
            )));
        }
        return match rc {
            native::ERAR_SUCCESS => Ok(Some(Salvaged {
                bytes: sink.bytes,
                declared_size,
                damaged: false,
            })),
            native::ERAR_BAD_DATA => Ok(Some(Salvaged {
                bytes: sink.bytes,
                declared_size,
                damaged: true,
            })),
            native::ERAR_MISSING_PASSWORD | native::ERAR_BAD_PASSWORD => {
                Err(ArchiveError::Encrypted)
            }
            other => Err(ArchiveError::Malformed(format!(
                "cbr read: UnRAR error {other}"
            ))),
        };
    }
}

#[cfg(not(any(target_os = "linux", target_os = "netbsd")))]
pub(crate) fn salvage_entry(
    _rar_name: &Path,
    _want: &str,
    _max_entry_bytes: u64,
) -> Result<Option<Salvaged>, ArchiveError> {
    Ok(None)
}

/// Where the callback puts decoded bytes, bounded by the entry's declared
/// size.
#[cfg(any(target_os = "linux", target_os = "netbsd"))]
struct Sink {
    bytes: Vec<u8>,
    limit: u64,
    overflow: bool,
}

/// UnRAR's callback: append decoded data to the [`Sink`] behind
/// `user_data`; stop (`-1`) on anything past the declared size, a password
/// request or a volume change.
#[cfg(any(target_os = "linux", target_os = "netbsd"))]
extern "C" fn collect(
    msg: unrar_sys::UINT,
    user_data: unrar_sys::LPARAM,
    p1: unrar_sys::LPARAM,
    p2: unrar_sys::LPARAM,
) -> std::os::raw::c_int {
    if user_data == 0 {
        return -1;
    }
    // SAFETY: `user_data` is the `&mut Sink` `salvage_entry` registered,
    // alive for the whole RARProcessFile call that invokes this.
    let sink = unsafe { &mut *(user_data as *mut Sink) };
    match msg {
        unrar_sys::UCM_PROCESSDATA => {
            let Ok(len) = usize::try_from(p2) else {
                sink.overflow = true;
                return -1;
            };
            if sink.bytes.len() as u64 + len as u64 > sink.limit {
                sink.overflow = true;
                return -1;
            }
            if len > 0 && p1 != 0 {
                // SAFETY: UnRAR hands `len` readable bytes at `p1` for the
                // duration of this call.
                let chunk = unsafe { std::slice::from_raw_parts(p1 as *const u8, len) };
                sink.bytes.extend_from_slice(chunk);
            }
            0
        }
        unrar_sys::UCM_NEEDPASSWORD
        | unrar_sys::UCM_NEEDPASSWORDW
        | unrar_sys::UCM_CHANGEVOLUME
        | unrar_sys::UCM_CHANGEVOLUMEW => -1,
        _ => 0,
    }
}

/// Closes the UnRAR handle on every path out of [`salvage_entry`].
#[cfg(any(target_os = "linux", target_os = "netbsd"))]
struct Handle(*const unrar_sys::Handle);

#[cfg(any(target_os = "linux", target_os = "netbsd"))]
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: an open handle from RAROpenArchiveEx, closed exactly once.
        unsafe { unrar_sys::RARCloseArchive(self.0) };
    }
}

/// UnRAR's wide file name (UTF-32 `wchar_t` on Linux), up to its NUL, as a
/// string; invalid code points become U+FFFD, as the crate's lossy name
/// does.
#[cfg(any(target_os = "linux", target_os = "netbsd"))]
fn wide_to_string(w: &[unrar_sys::WCHAR]) -> String {
    w.iter()
        .take_while(|&&c| c != 0)
        .map(|&c| char::from_u32(c as u32).unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

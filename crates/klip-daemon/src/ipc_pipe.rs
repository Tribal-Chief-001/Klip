//! The Windows control channel: a named pipe that only the user running the daemon can open.
//!
//! The Unix socket is `0600`. A default named pipe is readable by other local accounts, and this one
//! carries the clipboard history and can stop the daemon, so every instance gets a DACL with one entry:
//! full access for the current user's SID. Remote clients are rejected as well.
use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::ptr::null_mut;
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// The SID of the user running this process, in the text form SDDL uses (`S-1-5-21-...`).
fn current_user_sid() -> io::Result<String> {
    // SAFETY: plain Win32 calls with buffers sized by the API itself; every handle and every
    // LocalAlloc'd string is released on every path.
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let sid = (|| {
            let mut needed = 0u32;
            // The first call only reports the size it needs (it "fails" with a short buffer).
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed);
            if needed == 0 {
                return Err(io::Error::last_os_error());
            }
            // u64 words: TOKEN_USER needs pointer alignment, which a byte buffer does not promise.
            let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
            if GetTokenInformation(
                token,
                TokenUser,
                buf.as_mut_ptr() as *mut c_void,
                needed,
                &mut needed,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut text: *mut u16 = null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut len = 0;
            while *text.add(len) != 0 {
                len += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text as *mut c_void);
            Ok(sid)
        })();
        CloseHandle(token);
        sid
    }
}

/// The security attributes every pipe instance is created with.
pub struct PipeSecurity {
    attributes: SECURITY_ATTRIBUTES,
    descriptor: *mut c_void,
}

// SAFETY: the descriptor is process-local memory that is only read when an instance is created, and
// instances are created from one task at a time.
unsafe impl Send for PipeSecurity {}

impl PipeSecurity {
    /// `D:P(A;;GA;;;<current user's SID>)`: protected, one entry, full access for this user only.
    pub fn owner_only() -> io::Result<Self> {
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{})", current_user_sid()?)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut descriptor: *mut c_void = null_mut();
        // SAFETY: `sddl` is NUL-terminated and outlives the call; the result is freed in Drop.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            attributes: SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            },
            descriptor,
        })
    }

    /// One more instance of the pipe. The first one must be `first` (it fails if the name is taken,
    /// so a second daemon, or a squatter, cannot sit on our pipe).
    pub fn create(&mut self, name: &str, first: bool) -> io::Result<NamedPipeServer> {
        let mut options = ServerOptions::new();
        options
            .first_pipe_instance(first)
            .reject_remote_clients(true);
        // SAFETY: `attributes` is a valid SECURITY_ATTRIBUTES whose descriptor lives as long as `self`.
        unsafe {
            options.create_with_security_attributes_raw(
                name,
                &mut self.attributes as *mut SECURITY_ATTRIBUTES as *mut c_void,
            )
        }
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe {
            LocalFree(self.descriptor);
        }
    }
}

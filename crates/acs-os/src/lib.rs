//! ACS OS: the operating-system calls the standard library does not offer.
//!
//! Every other crate in the workspace has `unsafe_code = forbid`. Whatever cannot be done without
//! `unsafe` lives here, behind a safe function, and there is very little of it (decision 0020).

/// Keeps this process's stdin, stdout and stderr out of the child processes it starts from now on,
/// unless a child is explicitly given one of them.
///
/// Windows hands every inheritable handle to every child, whatever the child's own stdio is set
/// to, and the handles a process was started with are inheritable. A detached child would so keep
/// the hook's stdout pipe open, and the harness reading that pipe to its end would wait for the
/// child. Unix closes such descriptors on `exec`, so there this does nothing.
///
/// Best effort: a handle that cannot be changed (there is none, say) is left as it is.
pub fn keep_stdio_from_children() {
    #[cfg(windows)]
    {
        use std::io::{stderr, stdin, stdout};
        clear_inherit(stdin());
        clear_inherit(stdout());
        clear_inherit(stderr());
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn clear_inherit(stream: impl std::os::windows::io::AsRawHandle) {
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    // SAFETY: the handle comes from a live standard stream and is only read by the call, which
    // changes a flag on the kernel object and touches no memory. If the handle is null or invalid
    // the call fails and returns 0, which is what "best effort" above allows.
    let _ = unsafe { SetHandleInformation(stream.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) };
}

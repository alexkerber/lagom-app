//! User-owned handles for the Windows agent transport and its credentials.
use super::{Code, Peer, Stream};
use sha2::{Digest, Sha256};
use std::{
    ffi::{c_void, OsString},
    fs::{File, OpenOptions},
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr::null_mut,
    time::Duration,
};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient, ServerOptions};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::{
        Console::*,
        Pipes::PeekNamedPipe,
        SystemServices::{
            ACCESS_ALLOWED_ACE_TYPE, SECURITY_MANDATORY_MEDIUM_RID, SYSTEM_MANDATORY_LABEL_ACE_TYPE,
        },
        Threading::*,
        IO::CancelIoEx,
    },
};

struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

// TOKEN_USER contains a pointer into the same buffer. Keep it borrowed for the callback.
fn with_user<T>(f: impl FnOnce(PSID) -> io::Result<T>) -> io::Result<T> {
    unsafe {
        let mut token = null_mut();
        check(OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY,
            &mut token,
        ))?;
        let token = OwnedHandle::from_raw_handle(token);
        let mut buf = [0usize;
            (size_of::<TOKEN_USER>() + SECURITY_MAX_SID_SIZE as usize).div_ceil(size_of::<usize>())];
        let mut len = 0;
        check(GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buf.as_mut_ptr().cast(),
            size_of_val(&buf) as u32,
            &mut len,
        ))?;
        f((*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid)
    }
}

fn user_sid() -> io::Result<String> {
    with_user(|sid| unsafe {
        let mut wide = null_mut();
        check(ConvertSidToStringSidW(sid, &mut wide))?;
        let _allocation = Local(wide.cast());
        let mut len = 0;
        while *wide.add(len) != 0 {
            len += 1;
        }
        OsString::from_wide(std::slice::from_raw_parts(wide, len))
            .into_string()
            .map_err(|_| io::ErrorKind::InvalidData.into())
    })
}

fn private_descriptor() -> io::Result<Local> {
    let sid = user_sid()?;
    let sddl: Vec<_> = format!("O:{sid}D:P(A;;FA;;;{sid})S:(ML;;NWNR;;;ME)\0")
        .encode_utf16()
        .collect();
    let mut descriptor = null_mut();
    unsafe {
        check(ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        ))?;
    }
    Ok(Local(descriptor))
}

impl Local {
    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

// Inspect the open handle so replacing a path cannot change what we authenticate.
fn verify_private(handle: HANDLE, object: SE_OBJECT_TYPE) -> io::Result<()> {
    with_user(|sid| unsafe {
        let (mut owner, mut acl, mut labels, mut descriptor) =
            (null_mut(), null_mut(), null_mut(), null_mut());
        let status = GetSecurityInfo(
            handle,
            object,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut acl,
            &mut labels,
            &mut descriptor,
        );
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let _allocation = Local(descriptor);
        let mut ace = null_mut();
        let (mut control, mut revision) = (0, 0);
        check(GetSecurityDescriptorControl(
            descriptor,
            &mut control,
            &mut revision,
        ))?;
        if control & SE_DACL_PROTECTED == 0 {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        // Windows treats an unlabeled object as medium integrity.
        if !labels.is_null() {
            for i in 0..(*labels).AceCount {
                check(GetAce(labels, i.into(), &mut ace))?;
                let label = ace.cast::<SYSTEM_MANDATORY_LABEL_ACE>();
                if (*label).Header.AceType == SYSTEM_MANDATORY_LABEL_ACE_TYPE as u8 {
                    let level = (&raw mut (*label).SidStart).cast();
                    if IsValidSid(level) == 0
                        || *GetSidSubAuthorityCount(level) != 1
                        || (*GetSidIdentifierAuthority(level)).Value
                            != SECURITY_MANDATORY_LABEL_AUTHORITY.Value
                        || *GetSidSubAuthority(level, 0) < SECURITY_MANDATORY_MEDIUM_RID as u32
                    {
                        return Err(io::ErrorKind::PermissionDenied.into());
                    }
                }
            }
        }
        if !owner.is_null() && EqualSid(sid, owner) != 0 && !acl.is_null() && (*acl).AceCount == 1 {
            check(GetAce(acl, 0, &mut ace))?;
            let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
            if (*ace).Header.AceType == ACCESS_ALLOWED_ACE_TYPE as u8
                && (*ace).Header.AceFlags & INHERITED_ACE as u8 == 0
                && EqualSid(sid, (&raw mut (*ace).SidStart).cast()) != 0
            {
                return Ok(());
            }
        }
        Err(io::ErrorKind::PermissionDenied.into())
    })
}

pub(super) fn private_file(path: &Path) -> io::Result<File> {
    let name: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
    let descriptor = private_descriptor()?;
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_DELETE | FILE_SHARE_READ | FILE_SHARE_WRITE,
            &descriptor.attributes(),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(handle) };
    verify_private(handle, SE_FILE_OBJECT)?;
    Ok(file)
}

pub(crate) fn open_token(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .share_mode(FILE_SHARE_READ)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || meta.len() > 256
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    verify_private(file.as_raw_handle(), SE_FILE_OBJECT)?;
    Ok(file)
}

pub(crate) fn socket_path(dir: &Path) -> Result<PathBuf, Code> {
    let sid = user_sid().map_err(|_| Code::Unavailable)?;
    let dir = dir.canonicalize().map_err(|_| Code::AppUnreachable)?;
    let profile = Sha256::digest(dir.as_os_str().as_encoded_bytes());
    Ok(format!(r"\\.\pipe\lagom-{sid}-{profile:x}").into())
}

pub(super) struct Socket {
    listener: Stream,
    path: PathBuf,
    pub generation: i64,
}

impl Socket {
    fn instance(path: &Path, first: bool) -> io::Result<Stream> {
        let descriptor = private_descriptor()?;
        let mut attributes = descriptor.attributes();
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(path, (&raw mut attributes).cast())
        }
    }

    pub fn bind(path: &Path, generation: i64) -> io::Result<Self> {
        Ok(Self {
            listener: Self::instance(path, true)?,
            path: path.into(),
            generation,
        })
    }

    pub async fn accept(&mut self) -> io::Result<(Stream, bool)> {
        self.listener.connect().await?;
        // Keep the name owned between clients.
        let next = match Self::instance(&self.path, false) {
            Ok(next) => next,
            Err(e) => {
                self.listener.disconnect()?;
                return Err(e);
            }
        };
        Ok((std::mem::replace(&mut self.listener, next), true))
    }
}

pub(crate) async fn connect(dir: &Path) -> Result<NamedPipeClient, Code> {
    let path = socket_path(dir)?;
    loop {
        match ClientOptions::new()
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(&path)
        {
            Ok(pipe) => {
                verify_private(pipe.as_raw_handle(), SE_KERNEL_OBJECT)
                    .map_err(|_| Code::Unauthorized)?;
                return Ok(pipe);
            }
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(_) => return Err(Code::AppUnreachable),
        }
    }
}

// Mio can report a queued pipe write as complete. Hold the permit and deadline until the
// payload clears, then send the frame terminator. Cancel pending I/O if the caller times out.
pub(crate) async fn write_frame<T>(stream: &mut T, bytes: &[u8]) -> io::Result<()>
where
    T: tokio::io::AsyncWrite + Unpin + AsRawHandle,
{
    use tokio::io::AsyncWriteExt;

    struct Pending<'a, T: AsRawHandle>(&'a mut T);
    impl<T: AsRawHandle> Drop for Pending<'_, T> {
        fn drop(&mut self) {
            unsafe {
                CancelIoEx(self.0.as_raw_handle(), std::ptr::null());
            }
        }
    }
    let pending = Pending(stream);
    let (end, payload) = bytes.split_last().expect("newline-terminated frame");
    pending.0.write_all(payload).await?;
    pending.0.write_all(std::slice::from_ref(end)).await?;
    std::mem::forget(pending);
    Ok(())
}

pub(super) fn clone_peer(stream: &Stream) -> io::Result<Peer> {
    stream.as_handle().try_clone_to_owned()
}

pub(super) fn peer_connected(peer: &Peer) -> bool {
    let mut available = 0;
    unsafe {
        PeekNamedPipe(
            peer.as_raw_handle(),
            null_mut(),
            0,
            null_mut(),
            &mut available,
            null_mut(),
        ) != 0
            && available == 0
    }
}

pub(crate) fn installed_binary() -> io::Result<PathBuf> {
    use ::windows::{core::w, Win32::System::Registry::*};
    let mut path = vec![0u16; 32768];
    let mut bytes = (path.len() * 2) as u32;
    // Inno records the actual directory, including installs with a custom /DIR.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\app.lagommail.desktop_is1"),
            w!("InstallLocation"),
            RRF_RT_REG_SZ,
            None,
            Some(path.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status.is_err() {
        return Err(io::Error::from_raw_os_error(status.0 as i32));
    }
    path.truncate(bytes as usize / 2);
    let dir = OsString::from_wide(path.strip_suffix(&[0]).unwrap_or(&path));
    Ok(PathBuf::from(dir).join("lagom.exe"))
}

pub(crate) fn attach_console() {
    unsafe {
        // Preserve MCP pipes and shell redirection when AttachConsole installs console handles.
        let inherited = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .map(|id| (id, GetStdHandle(id)));
        if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            for (id, handle) in inherited {
                if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                    SetStdHandle(id, handle);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pipe_name_stays_owned_across_connections() {
        let profile = tempfile::tempdir().unwrap();
        let path = socket_path(profile.path()).unwrap();
        let mut socket = Socket::bind(&path, 1).unwrap();
        for _ in 0..2 {
            let _client = tokio::time::timeout(Duration::from_secs(5), connect(profile.path()))
                .await
                .unwrap()
                .unwrap();
            let (_connected, _) = socket.accept().await.unwrap();
            assert!(Socket::bind(&path, 2).is_err());
        }
    }
}

//! Windows file identity and security-descriptor checks.
//!
//! These are the Windows counterparts of the Unix owner/mode checks: a file's
//! owner SID must belong to the trusted principals (the operating system for
//! system helpers, the current user for controlled artifacts), and its DACL
//! may grant write rights to nobody else. Every check reads the
//! descriptor through an already-open handle, so the object checked is the
//! object used.

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_SUCCESS, GENERIC_ALL, GENERIC_WRITE, HANDLE, HLOCAL, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, CreateWellKnownSid, DACL_SECURITY_INFORMATION, EqualSid,
    GetAce, GetLengthSid, GetTokenInformation, INHERIT_ONLY_ACE, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_MAX_SID_SIZE, TOKEN_OWNER, TOKEN_QUERY, TOKEN_USER,
    TokenOwner, TokenUser, WELL_KNOWN_SID_TYPE, WinBuiltinAdministratorsSid, WinLocalSystemSid,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, DELETE, FILE_APPEND_DATA, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA,
    FILE_WRITE_EA, GetFileInformationByHandle, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Rights that let a principal change, replace or re-permission an object.
pub(crate) const WRITE_RIGHTS: u32 = FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_WRITE_EA
    | FILE_WRITE_ATTRIBUTES
    | FILE_DELETE_CHILD
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER
    | GENERIC_WRITE
    | GENERIC_ALL;

/// Rights that let a principal replace an existing child of a directory or
/// the directory itself. Creating a new sibling is not among them: a shared
/// ancestor (such as a volume root) may let others add entries, but not swap
/// the ones on our path.
pub(crate) const REPLACE_RIGHTS: u32 =
    FILE_DELETE_CHILD | DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_ALL;

/// An owned copy of a security identifier.
#[derive(Clone)]
pub(crate) struct Sid(Vec<u8>);

impl Sid {
    fn as_psid(&self) -> PSID {
        self.0.as_ptr().cast_mut().cast::<c_void>()
    }

    /// Copy a SID out of memory owned by a descriptor or token buffer.
    ///
    /// # Safety
    /// `psid` must point at a valid SID that stays alive for this call.
    unsafe fn copy_from(psid: PSID) -> Option<Self> {
        if psid.is_null() {
            return None;
        }
        // SAFETY: the caller guarantees `psid` is a valid SID; GetLengthSid
        // only reads its header.
        let length = usize::try_from(unsafe { GetLengthSid(psid) }).ok()?;
        if length == 0 {
            return None;
        }
        let mut bytes = vec![0_u8; length];
        // SAFETY: `psid` is valid for `length` bytes (its own reported
        // length) and `bytes` is a distinct allocation of that length.
        unsafe { ptr::copy_nonoverlapping(psid.cast::<u8>(), bytes.as_mut_ptr(), length) };
        Some(Self(bytes))
    }

    fn well_known(kind: WELL_KNOWN_SID_TYPE) -> io::Result<Self> {
        let mut bytes = vec![0_u8; SECURITY_MAX_SID_SIZE as usize];
        let mut size = SECURITY_MAX_SID_SIZE;
        // SAFETY: `bytes` is writable for `size` bytes, the documented
        // maximum SID size, and no domain SID is needed for these kinds.
        let ok = unsafe {
            CreateWellKnownSid(kind, ptr::null_mut(), bytes.as_mut_ptr().cast(), &mut size)
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        bytes.truncate(size as usize);
        Ok(Self(bytes))
    }

    /// `NT SERVICE\\TrustedInstaller`, S-1-5-80-956008885-3418522649-
    /// 1831038044-1853292631-2271478464, which owns the files Windows
    /// installs. It has no WELL_KNOWN_SID_TYPE, so its binary form is built
    /// here: revision 1, six sub-authorities, NT authority 5.
    fn trusted_installer() -> Self {
        let mut bytes = vec![1_u8, 6, 0, 0, 0, 0, 0, 5];
        for sub_authority in [
            80_u32,
            956_008_885,
            3_418_522_649,
            1_831_038_044,
            1_853_292_631,
            2_271_478_464,
        ] {
            bytes.extend_from_slice(&sub_authority.to_le_bytes());
        }
        Self(bytes)
    }

    fn equals(&self, other: PSID) -> bool {
        // SAFETY: both pointers name valid SIDs: ours is owned, `other`
        // comes from a live descriptor. EqualSid only reads them.
        unsafe { EqualSid(self.as_psid(), other) != 0 }
    }
}

/// The principals a check trusts: who may own the object, and who else may
/// hold the rights being checked.
pub(crate) struct Principals {
    owners: Vec<Sid>,
    trusted: Vec<Sid>,
}

impl Principals {
    /// The current user, for caller-selected (controlled) artifacts: owned by
    /// the user or the token's default owner, with SYSTEM and Administrators
    /// (already able to bypass any DACL) as the only other writers.
    pub(crate) fn current_user() -> io::Result<Self> {
        let token = Token::open_current()?;
        let owners = vec![token.user()?, token.default_owner()?];
        let mut trusted = owners.clone();
        trusted.push(Sid::well_known(WinLocalSystemSid)?);
        trusted.push(Sid::well_known(WinBuiltinAdministratorsSid)?);
        Ok(Self { owners, trusted })
    }

    /// The operating system, for system helpers: owned by and writable only
    /// by TrustedInstaller, SYSTEM or Administrators (the analogue of
    /// root-owned and not group/other-writable).
    pub(crate) fn operating_system() -> io::Result<Self> {
        let trusted = vec![
            Sid::trusted_installer(),
            Sid::well_known(WinLocalSystemSid)?,
            Sid::well_known(WinBuiltinAdministratorsSid)?,
        ];
        Ok(Self {
            owners: trusted.clone(),
            trusted,
        })
    }

    fn owns(&self, owner: PSID) -> bool {
        self.owners.iter().any(|sid| sid.equals(owner))
    }

    fn is_trusted(&self, sid: PSID) -> bool {
        self.trusted.iter().any(|candidate| candidate.equals(sid))
    }
}

struct Token(HANDLE);

impl Token {
    fn open_current() -> io::Result<Self> {
        let mut handle: HANDLE = ptr::null_mut();
        // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no
        // closing; `handle` is a valid out-pointer.
        let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(handle))
    }

    /// Read one variable-size token information class into an aligned buffer.
    fn information(&self, class: i32) -> io::Result<Vec<u64>> {
        let mut needed = 0_u32;
        // SAFETY: a null buffer of length zero asks only for the size.
        unsafe { GetTokenInformation(self.0, class, ptr::null_mut(), 0, &mut needed) };
        if needed == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0_u64; (needed as usize).div_ceil(8)];
        // SAFETY: `buffer` is 8-byte aligned and at least `needed` bytes.
        let ok = unsafe {
            GetTokenInformation(
                self.0,
                class,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(buffer)
    }

    fn user(&self) -> io::Result<Sid> {
        let buffer = self.information(TokenUser)?;
        // SAFETY: GetTokenInformation(TokenUser) filled `buffer` with a
        // TOKEN_USER whose SID points inside the same live buffer.
        unsafe {
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            Sid::copy_from(user.User.Sid)
        }
        .ok_or_else(|| io::Error::other("token user SID is missing"))
    }

    fn default_owner(&self) -> io::Result<Sid> {
        let buffer = self.information(TokenOwner)?;
        // SAFETY: GetTokenInformation(TokenOwner) filled `buffer` with a
        // TOKEN_OWNER whose SID points inside the same live buffer.
        unsafe {
            let owner = &*buffer.as_ptr().cast::<TOKEN_OWNER>();
            Sid::copy_from(owner.Owner)
        }
        .ok_or_else(|| io::Error::other("token default owner SID is missing"))
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenProcessToken and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// An object's owner and DACL, read from an open handle.
struct Descriptor {
    descriptor: PSECURITY_DESCRIPTOR,
    owner: PSID,
    dacl: *mut ACL,
}

impl Descriptor {
    fn of(file: &File) -> io::Result<Self> {
        let mut owner: PSID = ptr::null_mut();
        let mut dacl: *mut ACL = ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: the handle is open with READ_CONTROL; every out-pointer is
        // valid. On success the system allocates `descriptor`, which owns the
        // memory `owner` and `dacl` point into; Drop frees it.
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        Ok(Self {
            descriptor,
            owner,
            dacl,
        })
    }

    /// True when no principal outside `principals` is granted any of `rights`.
    /// A missing DACL grants everyone everything and therefore fails. Allow
    /// ACE kinds this check does not parse (callback, object) fail closed.
    fn only_trusted_hold(&self, principals: &Principals, rights: u32) -> bool {
        if self.dacl.is_null() {
            return false;
        }
        // SAFETY: a non-null DACL from GetSecurityInfo is valid while the
        // descriptor lives; the header is read only.
        let count = unsafe { (*self.dacl).AceCount };
        for index in 0..u32::from(count) {
            let mut ace: *mut c_void = ptr::null_mut();
            // SAFETY: `index` is below the DACL's own ACE count.
            if unsafe { GetAce(self.dacl, index, &mut ace) } == 0 || ace.is_null() {
                return false;
            }
            // SAFETY: GetAce returned a pointer to a live ACE, which always
            // starts with an ACE_HEADER.
            let header = unsafe { &*ace.cast::<ACE_HEADER>() };
            if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
                continue;
            }
            match u32::from(header.AceType) {
                ACCESS_DENIED_ACE_TYPE => continue,
                ACCESS_ALLOWED_ACE_TYPE => {
                    // SAFETY: the header says this is an ACCESS_ALLOWED_ACE,
                    // whose SID starts at `SidStart` inside the same ACE.
                    let (mask, sid) = unsafe {
                        let allowed = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                        (
                            allowed.Mask,
                            ptr::addr_of!(allowed.SidStart).cast_mut().cast(),
                        )
                    };
                    if mask & rights != 0 && !principals.is_trusted(sid) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        true
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: the descriptor was allocated by GetSecurityInfo and is
        // freed exactly once.
        unsafe { LocalFree(self.descriptor as HLOCAL) };
    }
}

/// Facts about one opened file system object, without following a reparse
/// point at the last component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ObjectFacts {
    pub(crate) volume: u32,
    pub(crate) index: u64,
    pub(crate) modified: u64,
    pub(crate) attributes: u32,
    pub(crate) is_directory: bool,
    pub(crate) is_reparse_point: bool,
    pub(crate) links: u32,
    pub(crate) size: u64,
}

/// Open `path` for metadata and security reads only. A reparse point at the
/// last component (symlink, junction) is opened as itself, never followed.
pub(crate) fn open_object(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

pub(crate) fn facts(file: &File) -> io::Result<ObjectFacts> {
    let mut info = BY_HANDLE_FILE_INFORMATION {
        dwFileAttributes: 0,
        ftCreationTime: zero_filetime(),
        ftLastAccessTime: zero_filetime(),
        ftLastWriteTime: zero_filetime(),
        dwVolumeSerialNumber: 0,
        nFileSizeHigh: 0,
        nFileSizeLow: 0,
        nNumberOfLinks: 0,
        nFileIndexHigh: 0,
        nFileIndexLow: 0,
    };
    // SAFETY: the handle is open and `info` is a valid out-pointer.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(ObjectFacts {
        volume: info.dwVolumeSerialNumber,
        index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        modified: (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32)
            | u64::from(info.ftLastWriteTime.dwLowDateTime),
        attributes: info.dwFileAttributes,
        is_directory: info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        is_reparse_point: info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
        links: info.nNumberOfLinks,
        size: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
    })
}

fn zero_filetime() -> windows_sys::Win32::Foundation::FILETIME {
    windows_sys::Win32::Foundation::FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    }
}

/// The object's owner is one of the principals' owners.
pub(crate) fn owned_by(file: &File, principals: &Principals) -> io::Result<bool> {
    let descriptor = Descriptor::of(file)?;
    Ok(!descriptor.owner.is_null() && principals.owns(descriptor.owner))
}

/// No principal outside the trusted set holds any of `rights` on the object.
pub(crate) fn only_trusted_hold(
    file: &File,
    principals: &Principals,
    rights: u32,
) -> io::Result<bool> {
    Ok(Descriptor::of(file)?.only_trusted_hold(principals, rights))
}

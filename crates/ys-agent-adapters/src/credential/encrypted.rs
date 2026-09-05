//! Domain-independent v1 encrypted envelope and immutable file primitives.
//! Provider's on-disk version, AES-GCM nonce layout and caller-supplied AAD remain unchanged.

use ring::{
    aead::{self, Aad, LessSafeKey, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
};
use zeroize::{Zeroize, Zeroizing};

pub(super) const KEY_LENGTH: usize = 32;
const NONCE_LENGTH: usize = 12;
const ENVELOPE_VERSION: u8 = 1;

#[derive(Debug)]
pub(super) enum EnvelopeError {
    Invalid,
    Internal,
}

pub(super) fn encrypt(
    key: &[u8; KEY_LENGTH],
    aad: &[u8],
    secret: &str,
) -> Result<Vec<u8>, EnvelopeError> {
    if secret.is_empty() {
        return Err(EnvelopeError::Invalid);
    }
    let cipher = cipher(key)?;
    let mut nonce = [0; NONCE_LENGTH];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| EnvelopeError::Internal)?;
    let mut ciphertext = Zeroizing::new(secret.as_bytes().to_vec());
    cipher
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            Aad::from(aad),
            &mut *ciphertext,
        )
        .map_err(|_| EnvelopeError::Internal)?;
    let mut encoded = Vec::with_capacity(1 + NONCE_LENGTH + ciphertext.len());
    encoded.push(ENVELOPE_VERSION);
    encoded.extend_from_slice(&nonce);
    encoded.extend_from_slice(&ciphertext);
    Ok(encoded)
}

pub(super) fn decrypt(
    key: &[u8; KEY_LENGTH],
    aad: &[u8],
    encrypted: &[u8],
) -> Result<String, EnvelopeError> {
    if encrypted.len() <= 1 + NONCE_LENGTH || encrypted[0] != ENVELOPE_VERSION {
        return Err(EnvelopeError::Invalid);
    }
    let mut nonce = [0; NONCE_LENGTH];
    nonce.copy_from_slice(&encrypted[1..=NONCE_LENGTH]);
    let mut ciphertext = Zeroizing::new(encrypted[(1 + NONCE_LENGTH)..].to_vec());
    let plaintext = cipher(key)?
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            Aad::from(aad),
            &mut ciphertext,
        )
        .map_err(|_| EnvelopeError::Invalid)?;
    String::from_utf8(plaintext.to_vec()).map_err(|e| {
        e.into_bytes().zeroize();
        EnvelopeError::Invalid
    })
}

fn cipher(key: &[u8; KEY_LENGTH]) -> Result<LessSafeKey, EnvelopeError> {
    UnboundKey::new(&aead::AES_256_GCM, key)
        .map(LessSafeKey::new)
        .map_err(|_| EnvelopeError::Internal)
}

pub(super) fn write_new_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let result = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(path);
        return Err(error);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Pinned directory descriptors prevent a path component from being replaced by a symlink
/// between validation and an operation. Existing protection is checked, never silently repaired.
#[cfg(unix)]
pub(super) mod private_files {
    use rustix::{
        fd::OwnedFd,
        fs::{self, AtFlags, FileType, Mode, OFlags},
        io::Errno,
    };
    use std::{
        ffi::OsStr,
        fs::File,
        io::{self, Read, Write},
        path::{Component, Path},
    };
    use zeroize::Zeroizing;

    pub struct PrivateDirectory(OwnedFd);
    fn denied() -> io::Error {
        io::Error::from(io::ErrorKind::PermissionDenied)
    }
    fn directory_flags() -> OFlags {
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
    }
    fn check(fd: &OwnedFd, directory: bool) -> io::Result<()> {
        let stat = fs::fstat(fd)?;
        let kind = FileType::from_raw_mode(stat.st_mode);
        if stat.st_uid != rustix::process::geteuid().as_raw()
            || (stat.st_mode & 0o7777) != if directory { 0o700 } else { 0o600 }
            || kind
                != if directory {
                    FileType::Directory
                } else {
                    FileType::RegularFile
                }
            || (!directory && stat.st_nlink != 1)
        {
            return Err(denied());
        }
        Ok(())
    }
    impl PrivateDirectory {
        pub fn open(path: &Path) -> io::Result<Self> {
            if !path.is_absolute() {
                return Err(denied());
            }
            let mut fd = fs::open("/", directory_flags(), Mode::empty())?;
            let mut normal = false;
            for component in path.components() {
                match component {
                    Component::RootDir => {}
                    Component::Normal(name) => {
                        normal = true;
                        fd = match fs::openat(&fd, name, directory_flags(), Mode::empty()) {
                            Ok(next) => next,
                            Err(Errno::NOENT) => {
                                match fs::mkdirat(&fd, name, Mode::RWXU) {
                                    Ok(()) | Err(Errno::EXIST) => {}
                                    Err(e) => return Err(e.into()),
                                }
                                fs::fsync(&fd)?;
                                fs::openat(&fd, name, directory_flags(), Mode::empty())?
                            }
                            Err(e) => return Err(e.into()),
                        };
                    }
                    _ => return Err(denied()),
                }
            }
            if !normal {
                return Err(denied());
            }
            check(&fd, true)?;
            Ok(Self(fd))
        }
        pub fn child(&self, name: &str) -> io::Result<Self> {
            Self::name(name)?;
            match fs::mkdirat(&self.0, name, Mode::RWXU) {
                Ok(()) | Err(Errno::EXIST) => {}
                Err(e) => return Err(e.into()),
            }
            let fd = fs::openat(&self.0, name, directory_flags(), Mode::empty())?;
            check(&fd, true)?;
            fs::fsync(&self.0)?;
            Ok(Self(fd))
        }
        fn name(name: &str) -> io::Result<()> {
            if Path::new(name).components().count() != 1
                || Path::new(name).file_name() != Some(OsStr::new(name))
            {
                return Err(denied());
            }
            Ok(())
        }
        fn open_file(&self, name: &str) -> io::Result<OwnedFd> {
            Self::name(name)?;
            let fd = fs::openat(
                &self.0,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )?;
            check(&fd, false)?;
            Ok(fd)
        }
        pub fn read(&self, name: &str, limit: u64) -> io::Result<Zeroizing<Vec<u8>>> {
            let file = File::from(self.open_file(name)?);
            let mut bytes = Zeroizing::new(Vec::new());
            file.take(limit + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > limit {
                return Err(io::Error::from(io::ErrorKind::InvalidData));
            }
            Ok(bytes)
        }
        pub fn write_new(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
            Self::name(name)?;
            let fd = fs::openat(
                &self.0,
                name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )?;
            check(&fd, false)?;
            let mut file = File::from(fd);
            // A failed write is deliberately left for journal recovery, never overwritten.
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::fsync(&self.0)?;
            Ok(())
        }
        pub fn remove(&self, name: &str) -> io::Result<()> {
            match self.open_file(name) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e),
            }
            match fs::unlinkat(&self.0, name, AtFlags::empty()) {
                Ok(()) | Err(Errno::NOENT) => {}
                Err(e) => return Err(e.into()),
            }
            fs::fsync(&self.0)?;
            Ok(())
        }
    }
}

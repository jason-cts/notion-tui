use super::Credentials;
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    time::Duration,
};

#[derive(Clone)]
pub struct TokenStore {
    directory: PathBuf,
}

impl TokenStore {
    pub fn default_location() -> Result<Self> {
        let base = directories::BaseDirs::new()
            .context("Could not locate your configuration directory")?;
        Ok(Self {
            directory: base.config_dir().join("notion-tui"),
        })
    }

    fn ensure_directory(&self) -> Result<()> {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&self.directory)
            .context("Could not create the OAuth configuration directory")?;
        let metadata = fs::symlink_metadata(&self.directory)?;
        if !metadata.is_dir() {
            bail!("OAuth configuration directory must be a real directory, not a symlink");
        }
        check_private(&metadata)?;
        Ok(())
    }

    fn open(&self, name: &str, create: bool) -> Result<Option<File>> {
        let path = self.directory.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.is_file() {
                    bail!("OAuth credential and lock files must be regular files, not symlinks");
                }
                check_private(&metadata)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Could not inspect OAuth credential file"),
        }
        let mut options = OpenOptions::new();
        options.read(true).write(create).create(create);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if !create && error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => {
                return Err(error).context("Could not open private OAuth credential file");
            }
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            bail!("OAuth credential file is not a regular file");
        }
        check_private(&metadata)?;
        Ok(Some(file))
    }

    pub(super) async fn lock(&self) -> Result<File> {
        self.ensure_directory()?;
        let file = self
            .open("auth.lock", true)?
            .context("Could not open OAuth session lock")?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(35);
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(file),
                Err(error)
                    if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
                {
                    if tokio::time::Instant::now() >= deadline {
                        bail!("Another process is updating OAuth login; try again shortly");
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => return Err(error).context("Could not lock OAuth credentials"),
            }
        }
    }

    pub(super) fn load(&self) -> Result<Option<Credentials>> {
        let Some(file) = self.open("auth.json", false)? else {
            return Ok(None);
        };
        let mut contents = Vec::new();
        file.take(128 * 1024 + 1).read_to_end(&mut contents)?;
        if contents.len() > 128 * 1024 {
            bail!("OAuth credential file is too large; use --login to replace it");
        }
        let credentials: Credentials = serde_json::from_slice(&contents).map_err(|_| {
            anyhow::anyhow!(
                "Saved OAuth credentials are invalid; use notion-tui --login to replace them"
            )
        })?;
        if credentials.version != 1
            || credentials.access_token.is_empty()
            || credentials.refresh_token.is_empty()
        {
            bail!("Saved OAuth credentials are incomplete or unsupported; use notion-tui --login");
        }
        Ok(Some(credentials))
    }

    pub(super) fn save(&self, credentials: &Credentials) -> Result<()> {
        // Also rejects an existing symlink before an atomic replacement.
        self.open("auth.json", false)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        serde_json::to_writer(&mut file, credentials)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(self.directory.join("auth.json"))
            .map_err(|error| error.error)?;
        #[cfg(unix)]
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }

    pub async fn logout(&self) -> Result<()> {
        let _lock = self.lock().await?;
        if self.open("auth.json", false)?.is_some() {
            fs::remove_file(self.directory.join("auth.json"))?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn at(directory: PathBuf) -> Self {
        Self { directory }
    }
}

fn check_private(metadata: &fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: geteuid has no preconditions and does not access memory.
        let uid = unsafe { libc::geteuid() };
        if metadata.uid() != uid || metadata.permissions().mode() & 0o077 != 0 {
            bail!(
                "OAuth files must be owned by you and private: use directory permissions 0700 and file permissions 0600"
            );
        }
    }
    Ok(())
}

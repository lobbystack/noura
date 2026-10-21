//! The plugin host and its library: the plugins installed on this device,
//! each with a copy of its manifest and component and the code this
//! device compiled from them.
//!
//! The library is a folder in this device's app data:
//!
//! ```text
//! <library>/<id>/plugin.toml        the manifest, as installed
//! <library>/<id>/component.wasm     the component, as installed
//! <library>/<id>/<key>.cwasm        the code compiled from it
//! ```
//!
//! The key names the component's bytes and the engine's configuration, so
//! a new build or a Wasmtime update compiles again. The host runs compiled
//! code only from its library and never accepts code compiled elsewhere.

use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use wasmtime::Engine;
use wasmtime::component::Component;

use crate::{
    Capability, InstallError, InstallErrorCode, PluginError, PluginInstance, PluginManifest,
    WorkspaceReads,
    manifest::read_manifest_bytes,
    runtime::{self, Backend, EpochTicker, Limits},
};

/// The installed copy of a plugin's manifest and component.
const INSTALLED_MANIFEST: &str = "plugin.toml";
const INSTALLED_COMPONENT: &str = "component.wasm";
const COMPILED_EXTENSION: &str = "cwasm";

/// The largest component the host installs.
const MAX_COMPONENT_BYTES: u64 = 64 << 20;

/// Folders being written or replaced start with this, so a crash leaves
/// nothing that looks installed.
const WORK_PREFIX: &str = ".work-";

/// Names this process's work folders apart.
static WORK_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Compiles, caches and runs plugins. Each running plugin gets its own
/// store.
pub struct PluginHost {
    engine: Engine,
    limits: Limits,
    epochs: Arc<EpochTicker>,
    library: PathBuf,
}

/// A plugin folder the host read and compiled in memory, waiting for you
/// to approve its capabilities. Dropping it leaves nothing behind.
#[derive(Clone)]
pub struct Candidate {
    manifest: PluginManifest,
    folder: PathBuf,
    manifest_bytes: Vec<u8>,
    component_bytes: Vec<u8>,
    component: Component,
}

impl Candidate {
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// The folder it came from.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Names the manifest's and the component's bytes, so a reload can
    /// tell a new build from the one installed.
    pub fn digest(&self) -> String {
        digest(&self.manifest_bytes, &self.component_bytes)
    }
}

/// Where the code of a loaded plugin came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeSource {
    /// The library's compiled code.
    Cache,
    /// The host compiled the installed component, and cached the result.
    Compiled,
}

/// An installed plugin, compiled and ready to start.
pub struct LoadedPlugin {
    manifest: PluginManifest,
    component: Component,
    source: CodeSource,
    digest: String,
}

impl LoadedPlugin {
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    pub fn source(&self) -> CodeSource {
        self.source
    }

    /// See [`Candidate::digest`].
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

impl PluginHost {
    /// A host on Cranelift whose library is the folder `library`.
    pub fn new(library: impl Into<PathBuf>) -> Result<Self, InstallError> {
        Self::with_backend(library, Backend::Cranelift, Limits::default())
    }

    pub fn with_backend(
        library: impl Into<PathBuf>,
        backend: Backend,
        limits: Limits,
    ) -> Result<Self, InstallError> {
        let engine = runtime::engine(backend)
            .map_err(|error| InstallError::storage("start the plugin host", &error))?;
        let epochs = EpochTicker::start(engine.clone())
            .map_err(|error| InstallError::storage("start the plugin host", &error))?;
        let host = Self {
            engine,
            limits,
            epochs: Arc::new(epochs),
            library: library.into(),
        };
        host.remove_leftover_work();
        Ok(host)
    }

    /// Read the plugin folder `folder` and compile its component, without
    /// writing anything or running any of its code. Refuses a plugin whose
    /// ID is installed already.
    pub fn inspect(&self, folder: &Path) -> Result<Candidate, InstallError> {
        let (manifest, manifest_bytes) = read_manifest_bytes(folder)?;
        if self.is_installed(&manifest.id) {
            return Err(InstallError::new(
                InstallErrorCode::AlreadyInstalled,
                format!(
                    "A plugin with the ID {} is installed already. Uninstall it first.",
                    manifest.id
                ),
            ));
        }
        self.candidate(folder, manifest, manifest_bytes)
    }

    /// Read a new build of the installed plugin `id` from `folder`, as
    /// [`Self::inspect`] does for a new plugin.
    pub fn inspect_update(&self, id: &str, folder: &Path) -> Result<Candidate, InstallError> {
        let (manifest, manifest_bytes) = read_manifest_bytes(folder)?;
        if manifest.id != id {
            return Err(InstallError::new(
                InstallErrorCode::InvalidManifest,
                format!(
                    "The folder now holds the plugin {new} in place of {id}. Uninstall {id} \
                     to install {new}.",
                    new = manifest.id
                ),
            ));
        }
        self.candidate(folder, manifest, manifest_bytes)
    }

    /// Names the build in `folder` as [`Candidate::digest`] does, without
    /// compiling it, so a reload can skip a build that is installed
    /// already.
    pub fn folder_digest(&self, folder: &Path) -> Result<String, InstallError> {
        let (manifest, manifest_bytes) = read_manifest_bytes(folder)?;
        let entry = resolve_entry(folder, &manifest.entry)?;
        let component_bytes = read_component(&entry, &manifest.entry)?;
        Ok(digest(&manifest_bytes, &component_bytes))
    }

    /// The installed plugin's manifest and the digest of its installed
    /// build, read without compiling it.
    pub fn installed(&self, id: &str) -> Result<(PluginManifest, String), InstallError> {
        let (manifest, manifest_bytes, component_bytes) = self.read_installed(id)?;
        Ok((manifest, digest(&manifest_bytes, &component_bytes)))
    }

    fn read_installed(&self, id: &str) -> Result<(PluginManifest, Vec<u8>, Vec<u8>), InstallError> {
        if !self.is_installed(id) {
            return Err(not_installed(id));
        }
        let folder = self.library.join(id);
        let manifest_bytes = std::fs::read(folder.join(INSTALLED_MANIFEST))
            .map_err(|error| InstallError::storage("read the installed plugin", &error))?;
        let manifest = std::str::from_utf8(&manifest_bytes)
            .map_err(|_| {
                InstallError::new(
                    InstallErrorCode::InvalidManifest,
                    "The installed plugin's manifest isn't UTF-8 text.",
                )
            })
            .and_then(PluginManifest::parse)?;
        let component_bytes = std::fs::read(folder.join(INSTALLED_COMPONENT))
            .map_err(|error| InstallError::storage("read the installed plugin", &error))?;
        Ok((manifest, manifest_bytes, component_bytes))
    }

    fn candidate(
        &self,
        folder: &Path,
        manifest: PluginManifest,
        manifest_bytes: Vec<u8>,
    ) -> Result<Candidate, InstallError> {
        let entry = resolve_entry(folder, &manifest.entry)?;
        let component_bytes = read_component(&entry, &manifest.entry)?;
        if Engine::detect_precompiled(&component_bytes).is_some() {
            return Err(InstallError::new(
                InstallErrorCode::PrecompiledCode,
                format!(
                    "{} holds code compiled elsewhere. Noura installs only WebAssembly \
                     components and compiles them itself.",
                    manifest.entry
                ),
            ));
        }
        let component = self.compile(&component_bytes, &manifest.entry)?;
        Ok(Candidate {
            manifest,
            folder: folder.to_owned(),
            manifest_bytes,
            component_bytes,
            component,
        })
    }

    fn compile(&self, bytes: &[u8], entry: &str) -> Result<Component, InstallError> {
        let component = Component::new(&self.engine, bytes).map_err(|error| {
            tracing::warn!(error = %format!("{error:#}"), "a plugin's component didn't compile");
            InstallError::new(
                InstallErrorCode::NotAComponent,
                format!("{entry} isn't a WebAssembly component that Noura can run."),
            )
        })?;
        if let Some(import) = runtime::forbidden_import(&component) {
            return Err(InstallError::new(
                InstallErrorCode::ForbiddenImport,
                format!(
                    "The plugin imports {import}, which Noura doesn't provide. Plugins reach \
                     files and the network only through capabilities."
                ),
            ));
        }
        Ok(component)
    }

    /// Install `candidate` in the library with its compiled code. Refuses
    /// a plugin whose ID is installed already.
    pub fn install(&self, candidate: &Candidate) -> Result<(), InstallError> {
        let id = &candidate.manifest.id;
        if self.is_installed(id) {
            return Err(InstallError::new(
                InstallErrorCode::AlreadyInstalled,
                format!("A plugin with the ID {id} is installed already. Uninstall it first."),
            ));
        }
        let work = self.write_work_folder(candidate)?;
        std::fs::rename(&work, self.library.join(id)).map_err(|error| {
            let _ = std::fs::remove_dir_all(&work);
            InstallError::storage("install the plugin", &error)
        })?;
        sync_folder(&self.library);
        Ok(())
    }

    /// Replace the installed plugin with the new build `candidate`, as a
    /// reload does.
    pub fn update(&self, candidate: &Candidate) -> Result<(), InstallError> {
        let id = &candidate.manifest.id;
        let installed = self.library.join(id);
        if !self.is_installed(id) {
            return Err(not_installed(id));
        }
        let work = self.write_work_folder(candidate)?;
        let old = self.work_path();
        std::fs::rename(&installed, &old)
            .and_then(|()| std::fs::rename(&work, &installed))
            .map_err(|error| {
                let _ = std::fs::remove_dir_all(&work);
                if !installed.exists() {
                    let _ = std::fs::rename(&old, &installed);
                }
                InstallError::storage("install the new build", &error)
            })?;
        sync_folder(&self.library);
        if let Err(error) = std::fs::remove_dir_all(&old) {
            tracing::warn!(%error, "an old plugin build wasn't removed");
        }
        Ok(())
    }

    /// Load the installed plugin `id`: its compiled code from the library,
    /// or the installed component compiled again when that code is missing
    /// or was compiled for another engine.
    pub fn load(&self, id: &str) -> Result<LoadedPlugin, InstallError> {
        let (manifest, manifest_bytes, component_bytes) = self.read_installed(id)?;
        let folder = self.library.join(id);
        let compiled = folder.join(self.compiled_name(&component_bytes));
        let (component, source) = match self.load_compiled(&compiled) {
            Some(component) => (component, CodeSource::Cache),
            None => {
                let component = self.compile(&component_bytes, &manifest.entry)?;
                self.cache(&folder, &component, &component_bytes)?;
                (component, CodeSource::Compiled)
            }
        };
        if let Some(import) = runtime::forbidden_import(&component) {
            return Err(InstallError::new(
                InstallErrorCode::ForbiddenImport,
                format!("The plugin imports {import}, which Noura doesn't provide."),
            ));
        }
        Ok(LoadedPlugin {
            manifest,
            component,
            source,
            digest: digest(&manifest_bytes, &component_bytes),
        })
    }

    /// Remove the installed plugin `id`: its manifest, its component and
    /// its compiled code. The folder it came from stays as it is.
    pub fn uninstall(&self, id: &str) -> Result<(), InstallError> {
        if !self.is_installed(id) {
            return Err(not_installed(id));
        }
        let old = self.work_path();
        std::fs::rename(self.library.join(id), &old)
            .map_err(|error| InstallError::storage("uninstall the plugin", &error))?;
        sync_folder(&self.library);
        std::fs::remove_dir_all(&old)
            .map_err(|error| InstallError::storage("remove the plugin's files", &error))
    }

    /// The IDs of the installed plugins, sorted.
    pub fn installed_ids(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.library) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|id| crate::valid_plugin_id(id) && self.is_installed(id))
            .collect();
        ids.sort();
        ids
    }

    /// Start a loaded plugin in a new store. It may use the capabilities
    /// its manifest declares that `approved` holds; a call that needs any
    /// other capability fails without reaching `workspace`.
    pub fn start(
        &self,
        loaded: &LoadedPlugin,
        approved: &[Capability],
        workspace: Option<Arc<dyn WorkspaceReads>>,
    ) -> Result<PluginInstance, PluginError> {
        let granted = loaded
            .manifest
            .capabilities
            .iter()
            .filter(|capability| approved.contains(capability))
            .cloned()
            .collect();
        PluginInstance::start(
            &self.engine,
            &self.epochs,
            self.limits,
            &loaded.component,
            granted,
            workspace,
        )
    }

    fn is_installed(&self, id: &str) -> bool {
        crate::valid_plugin_id(id) && self.library.join(id).join(INSTALLED_MANIFEST).is_file()
    }

    /// Write `candidate`'s files and compiled code to a new work folder.
    fn write_work_folder(&self, candidate: &Candidate) -> Result<PathBuf, InstallError> {
        std::fs::create_dir_all(&self.library)
            .map_err(|error| InstallError::storage("create the plugin library", &error))?;
        let work = self.work_path();
        let written = (|| {
            std::fs::create_dir(&work)?;
            write_durably(&work.join(INSTALLED_MANIFEST), &candidate.manifest_bytes)?;
            write_durably(&work.join(INSTALLED_COMPONENT), &candidate.component_bytes)
        })();
        if let Err(error) = written {
            let _ = std::fs::remove_dir_all(&work);
            return Err(InstallError::storage("copy the plugin", &error));
        }
        if let Err(error) = self.cache(&work, &candidate.component, &candidate.component_bytes) {
            let _ = std::fs::remove_dir_all(&work);
            return Err(error);
        }
        sync_folder(&work);
        Ok(work)
    }

    /// Write `component`'s compiled code into the plugin folder `folder`,
    /// and remove code compiled from other builds or for other engines.
    fn cache(
        &self,
        folder: &Path,
        component: &Component,
        component_bytes: &[u8],
    ) -> Result<(), InstallError> {
        let name = self.compiled_name(component_bytes);
        let code = component
            .serialize()
            .map_err(|error| InstallError::storage("cache the compiled plugin", &error))?;
        write_durably(&folder.join(&name), &code)
            .map_err(|error| InstallError::storage("cache the compiled plugin", &error))?;
        if let Ok(entries) = std::fs::read_dir(folder) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                let stale = path
                    .extension()
                    .is_some_and(|extension| extension == COMPILED_EXTENSION)
                    && entry.file_name() != name.as_str();
                if stale && let Err(error) = std::fs::remove_file(&path) {
                    tracing::warn!(%error, "stale compiled plugin code wasn't removed");
                }
            }
        }
        Ok(())
    }

    /// The compiled code at `path`, when it is there and this engine can
    /// run it.
    fn load_compiled(&self, path: &Path) -> Option<Component> {
        if !path.is_file() {
            return None;
        }
        // SAFETY: Wasmtime runs deserialized code without verifying it. The
        // path is in this host's library, in this device's app data, where
        // only Noura writes, and `cache` wrote it from a component this host
        // compiled. Code from a plugin's folder never gets here: `inspect`
        // refuses precompiled code, and a file name holds the hash of the
        // component it was compiled from.
        match unsafe { Component::deserialize_file(&self.engine, path) } {
            Ok(component) => Some(component),
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "cached plugin code didn't load");
                None
            }
        }
    }

    /// The compiled code's file name for these component bytes on this
    /// engine.
    fn compiled_name(&self, component_bytes: &[u8]) -> String {
        let mut engine = std::hash::DefaultHasher::new();
        self.engine
            .precompile_compatibility_hash()
            .hash(&mut engine);
        let component = blake3::hash(component_bytes).to_hex();
        format!(
            "{}-{:016x}.{COMPILED_EXTENSION}",
            &component[..32],
            engine.finish()
        )
    }

    fn work_path(&self) -> PathBuf {
        let n = WORK_COUNTER.fetch_add(1, Ordering::Relaxed);
        self.library
            .join(format!("{WORK_PREFIX}{}-{n}", std::process::id()))
    }

    /// Remove work folders an interrupted install or update left.
    fn remove_leftover_work(&self) {
        let Ok(entries) = std::fs::read_dir(&self.library) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(WORK_PREFIX))
                && let Err(error) = std::fs::remove_dir_all(entry.path())
            {
                tracing::warn!(%error, "an interrupted plugin install wasn't cleaned up");
            }
        }
    }
}

fn not_installed(id: &str) -> InstallError {
    InstallError::new(
        InstallErrorCode::NotInstalled,
        format!("The plugin {id} isn't installed on this device."),
    )
}

/// The entry component's path, once it is sure to name a file inside
/// `folder`, through links too.
fn resolve_entry(folder: &Path, entry: &str) -> Result<PathBuf, InstallError> {
    let outside = || {
        InstallError::new(
            InstallErrorCode::EntryOutsideFolder,
            format!("The entry component {entry} isn't inside the plugin's folder."),
        )
    };
    let missing = || {
        InstallError::new(
            InstallErrorCode::EntryMissing,
            format!("The plugin's folder has no {entry}. Build the plugin, then try again."),
        )
    };
    let folder = folder
        .canonicalize()
        .map_err(|error| InstallError::storage("read the plugin's folder", &error))?;
    let path = match folder.join(entry).canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(missing()),
        Err(error) => return Err(InstallError::storage("read the entry component", &error)),
    };
    if !path.starts_with(&folder) {
        return Err(outside());
    }
    if !path.is_file() {
        return Err(missing());
    }
    Ok(path)
}

fn read_component(path: &Path, entry: &str) -> Result<Vec<u8>, InstallError> {
    let size = std::fs::metadata(path)
        .map_err(|error| InstallError::storage("read the entry component", &error))?
        .len();
    if size > MAX_COMPONENT_BYTES {
        return Err(InstallError::new(
            InstallErrorCode::NotAComponent,
            format!("{entry} is larger than 64 MB."),
        ));
    }
    std::fs::read(path).map_err(|error| InstallError::storage("read the entry component", &error))
}

fn digest(manifest: &[u8], component: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&(manifest.len() as u64).to_le_bytes());
    hasher.update(manifest);
    hasher.update(component);
    hasher.finalize().to_hex().to_string()
}

fn write_durably(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Sync a folder so the entries just written or renamed in it are on disk.
/// The library is a cache of what you installed, so a folder that can't be
/// synced, as on Windows, is noted and passed over.
fn sync_folder(folder: &Path) {
    #[cfg(unix)]
    if let Err(error) = std::fs::File::open(folder).and_then(|folder| folder.sync_all()) {
        tracing::warn!(%error, "a plugin library folder wasn't synced");
    }
    #[cfg(not(unix))]
    let _ = folder;
}

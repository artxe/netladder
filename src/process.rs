use std::{ffi::OsStr, path::Path};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Debug, Clone, Copy)]
pub struct ProcessIdentity<'a> {
    pub name: &'a OsStr,
    pub executable_path: Option<&'a Path>,
}

/// Keeps one `System` alive between refreshes so executable paths are only
/// resolved once per process instead of for every process on every refresh.
pub struct ProcessDirectory {
    system: System,
}

impl ProcessDirectory {
    pub fn new() -> Self {
        Self {
            system: System::new(),
        }
    }

    pub fn refresh(&mut self) {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
        );
    }

    pub fn lookup(&self, pid: u32) -> Option<ProcessIdentity<'_>> {
        let process = self.system.process(Pid::from_u32(pid))?;
        Some(ProcessIdentity {
            name: process.name(),
            executable_path: process.exe(),
        })
    }
}

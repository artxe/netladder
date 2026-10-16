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

    /// PID 0 stands for "no owner" and resolves to no process, although the
    /// process list holds a system entry under it.
    pub fn lookup(&self, pid: u32) -> Option<ProcessIdentity<'_>> {
        if pid == 0 {
            return None;
        }
        let process = self.system.process(Pid::from_u32(pid))?;
        Some(ProcessIdentity {
            name: process.name(),
            executable_path: process.exe(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ProcessDirectory;

    #[test]
    fn pid_zero_resolves_to_no_process() {
        // A packet whose socket has no owner carries PID 0, which must not be
        // credited to the system entry that holds PID 0.
        let mut directory = ProcessDirectory::new();
        directory.refresh();
        assert!(directory.lookup(std::process::id()).is_some());
        assert!(directory.lookup(0).is_none());
    }
}

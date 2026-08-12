use std::collections::HashMap;
use std::path::PathBuf;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

#[derive(Debug, Clone)]
pub struct ProcessIdentity {
    pub name: String,
    pub executable_path: Option<PathBuf>,
}

fn snapshot() -> System {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::OnlyIfNotSet),
    );
    system
}

pub fn process_identities() -> HashMap<u32, ProcessIdentity> {
    snapshot()
        .processes()
        .iter()
        .map(|(pid, process)| {
            (
                pid.as_u32(),
                ProcessIdentity {
                    name: process.name().to_string_lossy().into_owned(),
                    executable_path: process.exe().map(PathBuf::from),
                },
            )
        })
        .collect()
}

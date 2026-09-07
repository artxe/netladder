//! Relaunches NetLadder with administrator rights when it was started without
//! them. Release builds already request elevation through their manifest, but
//! debug builds (`cargo run`) do not, and WinDivert cannot open without it.

use std::{env, ffi::OsStr, iter, mem::size_of, os::windows::ffi::OsStrExt, ptr};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
    System::Threading::{GetCurrentProcess, OpenProcessToken},
    UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
};

/// Marks a process that was already relaunched, so a declined or failed
/// elevation cannot loop.
const RELAUNCH_FLAG: &str = "--elevated";

pub fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned = 0u32;
        let queried = GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        CloseHandle(token);
        queried != 0 && elevation.TokenIsElevated != 0
    }
}

/// Whether to hand off to an elevated copy. Preview mode exists to look at
/// the UI without administrator rights, so it never relaunches; an already
/// elevated process has nothing to gain.
pub fn should_relaunch(preview_requested: bool, elevated: bool) -> bool {
    !preview_requested && !elevated
}

/// Starts an elevated copy of this executable with the same arguments.
/// Returns `true` when the copy was launched and this process should exit;
/// `false` when the user declined the prompt or the launch failed.
pub fn relaunch_elevated() -> bool {
    if env::args().any(|argument| argument == RELAUNCH_FLAG) {
        return false;
    }
    let Ok(executable) = env::current_exe() else {
        return false;
    };
    let parameters = env::args()
        .skip(1)
        .map(quote_argument)
        .chain(iter::once(RELAUNCH_FLAG.to_owned()))
        .collect::<Vec<_>>()
        .join(" ");
    let directory = executable.parent().map(wide);

    let operation = wide("runas");
    let file = wide(executable.as_os_str());
    let parameters = wide(parameters);
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            directory
                .as_ref()
                .map_or(ptr::null(), |directory| directory.as_ptr()),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute reports success with a value above 32.
    result as usize > 32
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(iter::once(0)).collect()
}

fn quote_argument(argument: String) -> String {
    if argument.contains(' ') || argument.contains('"') {
        format!("\"{}\"", argument.replace('"', "\\\""))
    } else {
        argument
    }
}

#[cfg(test)]
mod tests {
    use super::{quote_argument, should_relaunch};

    #[test]
    fn relaunches_only_when_unelevated_and_not_previewing() {
        assert!(should_relaunch(false, false));
        assert!(!should_relaunch(false, true));
        assert!(!should_relaunch(true, false));
        assert!(!should_relaunch(true, true));
    }

    #[test]
    fn quotes_arguments_with_spaces_or_quotes() {
        assert_eq!(quote_argument("--elevated".into()), "--elevated");
        assert_eq!(quote_argument("a b".into()), "\"a b\"");
        assert_eq!(quote_argument("say \"hi\"".into()), "\"say \\\"hi\\\"\"");
    }
}

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

/// Quotes one argument by the Windows command-line rules: backslashes are
/// literal unless they precede a quote, so those (and the ones before the
/// closing quote) are doubled.
fn quote_argument(argument: String) -> String {
    if !argument.is_empty() && !argument.contains([' ', '\t', '"']) {
        return argument;
    }
    let mut quoted = String::with_capacity(argument.len() + 2);
    quoted.push('"');
    let mut backslashes = 0;
    for character in argument.chars() {
        if character == '\\' {
            backslashes += 1;
        } else {
            if character == '"' {
                quoted.extend(iter::repeat_n('\\', backslashes + 1));
            }
            backslashes = 0;
        }
        quoted.push(character);
    }
    quoted.extend(iter::repeat_n('\\', backslashes));
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use std::{iter, slice};

    use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};

    use super::{quote_argument, should_relaunch};

    /// Splits a command line the way the relaunched process reads it.
    fn parse_command_line(command_line: &str) -> Vec<String> {
        let wide: Vec<u16> = command_line.encode_utf16().chain(iter::once(0)).collect();
        let mut count = 0;
        unsafe {
            let arguments = CommandLineToArgvW(wide.as_ptr(), &mut count);
            assert!(!arguments.is_null());
            let parsed = (0..count as usize)
                .map(|index| {
                    let argument = *arguments.add(index);
                    let length = (0..).take_while(|&at| *argument.add(at) != 0).count();
                    String::from_utf16_lossy(slice::from_raw_parts(argument, length))
                })
                .collect();
            LocalFree(arguments.cast());
            parsed
        }
    }

    #[test]
    fn relaunch_passes_every_argument_through_unchanged() {
        let arguments = [
            "plain",
            "with space",
            "",
            "tab\tinside",
            r"C:\folder with space\",
            r#"say "hi""#,
            r#"slash\"quote"#,
            r"C:\no\spaces\",
        ];
        let command_line = iter::once("netladder.exe".to_owned())
            .chain(arguments.map(|argument| quote_argument(argument.to_owned())))
            .collect::<Vec<_>>()
            .join(" ");

        assert_eq!(parse_command_line(&command_line)[1..], arguments);
    }

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

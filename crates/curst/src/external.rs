//! editing a body in the user's own editor
//!
//! the body is written as typed, `{{placeholders}}` and all, never with
//! values resolved, to an owner-only file in a private temporary folder
//! that is removed however the editor ends

use std::fs::OpenOptions;
use std::io::Write;
use std::process::Command;

/// the editor to run: `$VISUAL`, then `$EDITOR`, then `vi`, split into
/// words without a shell
pub(crate) fn editor_command(env: impl Fn(&str) -> Option<String>) -> Vec<String> {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|key| env(key))
        .map(|value| {
            value
                .split_whitespace()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .find(|words| !words.is_empty())
        .unwrap_or_else(|| vec!["vi".to_owned()])
}

/// `text` after the user edited it with `command`, which gets the file as
/// its last argument
pub(crate) fn edit(text: &str, extension: &str, command: &[String]) -> Result<String, String> {
    let (program, args) = command
        .split_first()
        .ok_or_else(|| "no editor is set".to_owned())?;

    // the folder, and the file in it, go when `folder` is dropped
    let folder = tempfile::Builder::new()
        .prefix("curst-")
        .tempdir()
        .map_err(|e| format!("could not make a temporary folder: {e}"))?;
    let path = folder.path().join(format!("body.{extension}"));
    let mut options = OpenOptions::new();

    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }

    options
        .open(&path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|e| format!("could not write the body for the editor: {e}"))?;

    let status = Command::new(program)
        .args(args)
        .arg(&path)
        .status()
        .map_err(|e| format!("could not start {program}: {e}"))?;

    if !status.success() {
        return Err(format!(
            "{program} did not finish cleanly, so the body was kept"
        ));
    }

    std::fs::read_to_string(&path).map_err(|e| format!("could not read the edited body: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_editor_comes_from_the_environment() {
        let env = |pairs: &'static [(&str, &str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| (*v).to_owned())
            }
        };

        assert_eq!(editor_command(env(&[])), ["vi"]);
        assert_eq!(editor_command(env(&[("EDITOR", "nano")])), ["nano"]);
        assert_eq!(
            editor_command(env(&[("EDITOR", "nano"), ("VISUAL", "code --wait")])),
            ["code", "--wait"]
        );
        assert_eq!(
            editor_command(env(&[("VISUAL", "  "), ("EDITOR", "vim")])),
            ["vim"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_body_goes_through_a_private_file_that_is_then_gone() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("editor.sh");
        let seen = dir.path().join("seen");

        // records where the file was, its mode and content, then appends
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$1\" > '{seen}'\nls -l \"$1\" | cut -c1-10 >> '{seen}'\ncat \"$1\" >> '{seen}'\necho '  \"b\": 2' >> \"$1\"\n",
                seen = seen.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();

        let command = vec![script.display().to_string()];
        let edited = edit("{\n  \"a\": \"{{token}}\",\n", "json", &command).unwrap();
        let seen = std::fs::read_to_string(&seen).unwrap();
        let mut lines = seen.lines();
        let path = lines.next().unwrap();

        assert_eq!(edited, "{\n  \"a\": \"{{token}}\",\n  \"b\": 2\n");
        assert!(path.ends_with("body.json"));
        // owner read and write, nothing for anyone else, as `ls -l` shows it
        // on macOS and Linux alike
        assert_eq!(lines.next(), Some("-rw-------"));
        assert!(seen.contains("{{token}}"));
        assert!(!std::path::Path::new(path).exists());

        let failing = vec!["false".to_owned()];

        assert!(
            edit("x", "txt", &failing)
                .unwrap_err()
                .contains("did not finish")
        );
        assert!(edit("x", "txt", &[]).is_err());
        assert!(edit("x", "txt", &["/no/such/editor".to_owned()]).is_err());
    }
}

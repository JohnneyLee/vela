//! Writeback: translate-and-replace-into-the-source-window. Reuses the capture
//! chain for reading and the clipboard/input helpers for writing.

pub mod paste;
pub mod policy;
pub mod protect;
pub mod target;
pub mod typeout;
pub mod undo;

/// Write a ready text back into the remembered source window.
/// Used by the action bar's "替换回去" button.
pub fn write_ready(text: &str, exe: &str) -> Result<(), String> {
    let pol = policy::for_exe(exe);
    // Bring the source window back and verify it is still the same process
    // before pasting. Do not require the source to already be foreground,
    // because clicking the Vela button has just made Vela the foreground.
    target::switch_back_and_verify()?;
    if let Err(err) = paste::paste_text(text, &pol) {
        // Fall back to typing character by character.
        typeout::type_text(text, &pol).map_err(|_| err)?;
    }
    Ok(())
}

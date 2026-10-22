//! The production clipboard-paste rung on Wayland: the clipboard through the
//! data-control protocol ([`WaylandSelection`]), and the paste chord through
//! the trusted, system-owned `wtype` ([`SystemWtypeChord`]).
//!
//! Neither part runs a clipboard helper found on `PATH`: the clipboard is
//! spoken to directly, and `wtype` is the same revalidated helper the typing
//! rung uses, given only key names.

use std::sync::Arc;
use std::time::Duration;

use narrivy_types::SessionCancellation;

use crate::recovery::{PasteKeys, SystemWtypeChord, WtypeSubmitterStatus, wtype_submitter_status};
use crate::wayland_selection::{SelectionSnapshot, WaylandSelection};

use super::routed::{ClipboardHandoff, ClipboardOwnership, PasteChord, RungQualification};

/// The clipboard side of the rung.
pub struct WaylandClipboardHandoff {
    selection: Arc<WaylandSelection>,
    /// The write a restore undoes.
    last_write: Option<u64>,
    /// Reads of that write completed before the paste chord.
    reads_before_chord: u32,
}

impl WaylandClipboardHandoff {
    #[must_use]
    pub fn new(selection: Arc<WaylandSelection>) -> Self {
        Self {
            selection,
            last_write: None,
            reads_before_chord: 0,
        }
    }
}

impl ClipboardHandoff for WaylandClipboardHandoff {
    type Snapshot = SelectionSnapshot;

    fn probe(&mut self) -> Result<(), &'static str> {
        // Connected at construction; a lost compositor shows up as a failed
        // snapshot or write, which the ladder treats as this rung failing.
        Ok(())
    }

    fn snapshot(&mut self) -> Result<Option<SelectionSnapshot>, &'static str> {
        self.selection.snapshot().map(Some)
    }

    fn write(&mut self, text: &str) -> Result<ClipboardOwnership, &'static str> {
        let serial = self.selection.write_text(text)?;
        self.last_write = Some(serial);
        Ok(ClipboardOwnership {
            write_serial: serial,
        })
    }

    fn before_chord(&mut self, ownership: &ClipboardOwnership) {
        self.reads_before_chord = self.selection.reads(ownership.write_serial);
    }

    fn wait_until_consumed(
        &mut self,
        ownership: &ClipboardOwnership,
        bound: Duration,
    ) -> Option<bool> {
        Some(self.selection.wait_until_read_after(
            ownership.write_serial,
            self.reads_before_chord,
            bound,
        ))
    }

    fn still_owned(&mut self, ownership: &ClipboardOwnership, _text: &str) -> bool {
        self.selection.ownership(ownership.write_serial).clipboard
    }

    fn restore(&mut self, prior: &SelectionSnapshot) -> Result<(), &'static str> {
        // The selection worker puts back only what this write still owns.
        let serial = self.last_write.take().ok_or("clipboard_write_unknown")?;
        self.selection.restore(serial, prior)
    }
}

/// The paste chord, sent through the trusted `wtype`.
pub struct WtypePasteChord {
    chord: Option<SystemWtypeChord>,
    unavailable: &'static str,
    keys: PasteKeys,
}

impl WtypePasteChord {
    /// Shift+Insert: pastes in almost every Linux application, terminals
    /// included, so no target detection is needed for it.
    #[must_use]
    pub fn shift_insert() -> Self {
        Self::with_keys(PasteKeys::ShiftInsert)
    }

    #[must_use]
    pub fn with_keys(keys: PasteKeys) -> Self {
        let unavailable = match wtype_submitter_status() {
            WtypeSubmitterStatus::Available(_) => "",
            WtypeSubmitterStatus::Missing => "wtype_missing",
            WtypeSubmitterStatus::NonExecutable => "wtype_nonexecutable",
            WtypeSubmitterStatus::Untrusted => "wtype_untrusted",
            WtypeSubmitterStatus::IdentityChanged => "wtype_identity_changed",
            WtypeSubmitterStatus::UnsupportedPlatform => "wtype_isolation_unavailable",
        };
        let chord = if unavailable.is_empty() {
            SystemWtypeChord::new().ok()
        } else {
            None
        };
        Self {
            unavailable: if chord.is_none() && unavailable.is_empty() {
                "wtype_unavailable"
            } else {
                unavailable
            },
            chord,
            keys,
        }
    }
}

impl PasteChord for WtypePasteChord {
    fn qualification(&self) -> RungQualification {
        if self.chord.is_some() {
            RungQualification::Qualified
        } else {
            RungQualification::Unqualified(self.unavailable)
        }
    }

    fn send_paste_chord(&mut self, cancellation: &SessionCancellation) -> Result<(), &'static str> {
        match self.chord.as_mut() {
            Some(chord) => chord.send(self.keys, cancellation),
            None => Err(self.unavailable),
        }
    }
}

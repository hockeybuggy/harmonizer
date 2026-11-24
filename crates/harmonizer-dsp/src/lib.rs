//! Realtime polyphonic harmonizer.
//!
//! Every held MIDI note gets a voice that re-pitches the incoming audio to that note. Two
//! engines implement the re-pitching:
//!
//! - [`Mode::Absolute`] uses [`psola::PsolaEngine`]: it tracks the pitch of a monophonic input and
//!   resynthesises it at exactly the held note's frequency.
//! - [`Mode::Relative`] uses [`vocoder::VocoderEngine`]: a phase vocoder that transposes any input
//!   by the interval between the held note and a reference note.
//!
//! [`Harmonizer`] owns both engines, the voice allocator, envelopes, and the dry/wet mix. Nothing
//! in its processing path allocates once it has been constructed.

pub mod engine;
pub mod harmonizer;
pub mod params;
pub mod pitch;
pub mod psola;
pub mod util;
pub mod vocoder;
pub mod voices;

pub use engine::{Engine, MAX_VOICES, VoiceBuffers, VoiceControl};
pub use harmonizer::Harmonizer;
pub use params::{Mode, Params, Quality};

use crate::engine::MAX_VOICES;

#[derive(Clone, Copy, Debug, Default)]
pub struct Voice {
    pub active: bool,
    pub held: bool,
    pub sustained: bool,
    pub note: u8,
    pub velocity: f32,
    pub envelope: f32,
    age: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Allocation {
    pub slot: usize,
    /// The slot was free or stolen, so its engine state must be reset before it renders.
    pub needs_reset: bool,
}

/// Maps held notes to engine voice slots and runs their attack/release envelopes.
#[derive(Default)]
pub struct VoiceAllocator {
    voices: [Voice; MAX_VOICES],
    sustain_pedal: bool,
    next_age: u64,
}

impl VoiceAllocator {
    pub fn voices(&self) -> &[Voice; MAX_VOICES] {
        &self.voices
    }

    pub fn active(&self) -> impl Iterator<Item = (usize, &Voice)> {
        self.voices.iter().enumerate().filter(|(_, v)| v.active)
    }

    pub fn note_on(&mut self, note: u8, velocity: f32) -> Allocation {
        self.next_age += 1;
        let age = self.next_age;

        // Retriggering a sounding note keeps its engine state so the voice continues smoothly.
        if let Some(slot) = self.voices.iter().position(|v| v.active && v.note == note) {
            let voice = &mut self.voices[slot];
            voice.held = true;
            voice.sustained = false;
            voice.velocity = velocity;
            voice.age = age;
            return Allocation {
                slot,
                needs_reset: false,
            };
        }

        let slot = self
            .voices
            .iter()
            .position(|v| !v.active)
            .unwrap_or_else(|| self.steal());
        self.voices[slot] = Voice {
            active: true,
            held: true,
            sustained: false,
            note,
            velocity,
            envelope: 0.0,
            age,
        };
        Allocation {
            slot,
            needs_reset: true,
        }
    }

    /// Prefers the quietest releasing voice, then the oldest sustained one, then the oldest held.
    fn steal(&self) -> usize {
        let releasing = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, v)| !v.held && !v.sustained)
            .min_by(|(_, a), (_, b)| a.envelope.total_cmp(&b.envelope));
        if let Some((slot, _)) = releasing {
            return slot;
        }
        let sustained = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, v)| !v.held && v.sustained)
            .min_by_key(|(_, v)| v.age);
        if let Some((slot, _)) = sustained {
            return slot;
        }
        self.voices
            .iter()
            .enumerate()
            .min_by_key(|(_, v)| v.age)
            .expect("MAX_VOICES > 0")
            .0
    }

    pub fn note_off(&mut self, note: u8) {
        for voice in self
            .voices
            .iter_mut()
            .filter(|v| v.active && v.note == note)
        {
            if voice.held {
                voice.held = false;
                if self.sustain_pedal {
                    voice.sustained = true;
                }
            }
        }
    }

    pub fn set_sustain(&mut self, down: bool) {
        self.sustain_pedal = down;
        if !down {
            for voice in &mut self.voices {
                voice.sustained = false;
            }
        }
    }

    pub fn sustain(&self) -> bool {
        self.sustain_pedal
    }

    pub fn release_all(&mut self) {
        self.sustain_pedal = false;
        for voice in &mut self.voices {
            voice.held = false;
            voice.sustained = false;
        }
    }

    pub fn kill_all(&mut self) {
        self.voices = Default::default();
        self.sustain_pedal = false;
    }

    /// Advances one voice's envelope by one sample and returns its new level.
    pub fn step_envelope(&mut self, slot: usize, attack_step: f32, release_step: f32) -> f32 {
        let voice = &mut self.voices[slot];
        if voice.held || voice.sustained {
            voice.envelope = (voice.envelope + attack_step).min(1.0);
        } else {
            voice.envelope = (voice.envelope - release_step).max(0.0);
        }
        voice.envelope
    }

    /// Frees voices whose release has finished. Call once per block, after rendering.
    pub fn retire_finished(&mut self) {
        for voice in &mut self.voices {
            if voice.active && !voice.held && !voice.sustained && voice.envelope <= 0.0 {
                voice.active = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_notes_get_distinct_slots() {
        let mut alloc = VoiceAllocator::default();
        let a = alloc.note_on(60, 1.0);
        let b = alloc.note_on(64, 1.0);
        assert_ne!(a.slot, b.slot);
        assert!(a.needs_reset && b.needs_reset);
    }

    #[test]
    fn retrigger_reuses_slot_without_reset() {
        let mut alloc = VoiceAllocator::default();
        let a = alloc.note_on(60, 1.0);
        alloc.note_off(60);
        let b = alloc.note_on(60, 0.5);
        assert_eq!(
            b,
            Allocation {
                slot: a.slot,
                needs_reset: false
            }
        );
    }

    #[test]
    fn released_voice_retires_after_envelope_reaches_zero() {
        let mut alloc = VoiceAllocator::default();
        let slot = alloc.note_on(60, 1.0).slot;
        alloc.step_envelope(slot, 1.0, 0.5);
        alloc.note_off(60);
        alloc.step_envelope(slot, 1.0, 0.5);
        alloc.retire_finished();
        assert!(alloc.voices()[slot].active);
        alloc.step_envelope(slot, 1.0, 0.5);
        alloc.retire_finished();
        assert!(!alloc.voices()[slot].active);
    }

    #[test]
    fn steals_releasing_voice_before_held_ones() {
        let mut alloc = VoiceAllocator::default();
        for note in 0..MAX_VOICES as u8 {
            let slot = alloc.note_on(note, 1.0).slot;
            alloc.step_envelope(slot, 1.0, 0.0);
        }
        alloc.note_off(5);
        let stolen = alloc.note_on(100, 1.0);
        assert_eq!(alloc.voices()[stolen.slot].note, 100);
        assert!(alloc.voices().iter().all(|v| v.note != 5));
    }

    #[test]
    fn steals_oldest_when_all_held() {
        let mut alloc = VoiceAllocator::default();
        for note in 0..MAX_VOICES as u8 {
            alloc.note_on(note, 1.0);
        }
        alloc.note_on(100, 1.0);
        assert!(alloc.voices().iter().all(|v| v.note != 0));
    }

    #[test]
    fn sustain_holds_voice_through_note_off_until_pedal_up() {
        let mut alloc = VoiceAllocator::default();
        let slot = alloc.note_on(60, 1.0).slot;
        alloc.step_envelope(slot, 1.0, 0.5);

        alloc.set_sustain(true);
        alloc.note_off(60);
        assert!(!alloc.voices()[slot].held);
        assert!(alloc.voices()[slot].sustained);
        assert_eq!(alloc.step_envelope(slot, 1.0, 0.5), 1.0);
        alloc.retire_finished();
        assert!(alloc.voices()[slot].active);

        alloc.set_sustain(false);
        assert!(!alloc.voices()[slot].sustained);
        assert_eq!(alloc.step_envelope(slot, 1.0, 0.5), 0.5);
        assert_eq!(alloc.step_envelope(slot, 1.0, 0.5), 0.0);
        alloc.retire_finished();
        assert!(!alloc.voices()[slot].active);
    }

    #[test]
    fn pedal_up_keeps_keys_still_held() {
        let mut alloc = VoiceAllocator::default();
        let sustained_slot = alloc.note_on(60, 1.0).slot;
        let held_slot = alloc.note_on(64, 1.0).slot;
        alloc.step_envelope(sustained_slot, 1.0, 0.5);
        alloc.step_envelope(held_slot, 1.0, 0.5);

        alloc.set_sustain(true);
        alloc.note_off(60);
        alloc.set_sustain(false);

        assert!(!alloc.voices()[sustained_slot].held);
        assert!(!alloc.voices()[sustained_slot].sustained);
        assert!(alloc.voices()[held_slot].held);
        assert_eq!(alloc.step_envelope(held_slot, 1.0, 0.5), 1.0);
    }

    #[test]
    fn retrigger_under_sustain_keeps_slot_and_envelope() {
        let mut alloc = VoiceAllocator::default();
        let first = alloc.note_on(60, 0.8);
        alloc.step_envelope(first.slot, 1.0, 0.5);

        alloc.set_sustain(true);
        alloc.note_off(60);
        let second = alloc.note_on(60, 1.0);
        assert_eq!(
            second,
            Allocation {
                slot: first.slot,
                needs_reset: false
            }
        );
        let voice = alloc.voices()[first.slot];
        assert!(voice.held);
        assert!(!voice.sustained);
        assert_eq!(voice.envelope, 1.0);

        alloc.set_sustain(false);
        assert!(alloc.voices()[first.slot].held);
    }

    #[test]
    fn release_all_and_kill_all_clear_sustain_state() {
        let mut alloc = VoiceAllocator::default();
        let slot = alloc.note_on(60, 1.0).slot;
        alloc.set_sustain(true);
        alloc.note_off(60);
        alloc.release_all();
        assert!(!alloc.sustain());
        assert!(!alloc.voices()[slot].sustained);

        let slot2 = alloc.note_on(64, 1.0).slot;
        alloc.note_off(64);
        assert!(!alloc.voices()[slot2].sustained);

        alloc.set_sustain(true);
        alloc.kill_all();
        assert!(!alloc.sustain());
    }

    #[test]
    fn steals_sustained_before_held_and_releasing_before_sustained() {
        let mut alloc = VoiceAllocator::default();
        for note in 0..MAX_VOICES as u8 {
            let slot = alloc.note_on(note, 1.0).slot;
            alloc.step_envelope(slot, 1.0, 0.0);
        }
        alloc.note_off(6);
        alloc.set_sustain(true);
        alloc.note_off(3);
        alloc.note_off(5);

        let first_stolen = alloc.note_on(100, 1.0);
        assert_eq!(alloc.voices()[first_stolen.slot].note, 100);
        assert!(alloc.voices().iter().all(|v| v.note != 6));

        let second_stolen = alloc.note_on(101, 1.0);
        assert_eq!(alloc.voices()[second_stolen.slot].note, 101);
        assert!(alloc.voices().iter().all(|v| v.note != 3));
    }

    #[test]
    fn duplicate_note_off_does_not_latch_already_releasing_voice() {
        let mut alloc = VoiceAllocator::default();
        let slot = alloc.note_on(60, 1.0).slot;
        alloc.step_envelope(slot, 1.0, 0.25);
        alloc.note_off(60);
        alloc.set_sustain(true);
        alloc.note_off(60);
        assert!(!alloc.voices()[slot].sustained);
    }
}

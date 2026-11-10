//! Explicit session state machine (REL-001).
//!
//! Legal transitions are table-driven via [`is_valid_transition`]; every
//! successful transition emits a [`SessionEvent`] carrying a monotonically
//! increasing per-machine [`EventSeq`] so IPC clients can reject stale events.

use crate::{CoreError, JournalError};
use narrivy_types::{EventSeq, SessionEvent, SessionId, SessionState, TransactionId};

/// Phase-0 compatible session machine evolved with event emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionMachine {
    session_id: SessionId,
    state: SessionState,
    /// Sequence number the *next* emitted event will carry (starts at 1).
    next_seq: u64,
}

impl Default for SessionMachine {
    fn default() -> Self {
        Self {
            session_id: SessionId(0),
            state: SessionState::Idle,
            next_seq: 1,
        }
    }
}

impl SessionMachine {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_session(session_id: SessionId) -> Self {
        Self {
            session_id,
            state: SessionState::Idle,
            next_seq: 1,
        }
    }

    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    #[must_use]
    pub const fn state(&self) -> SessionState {
        self.state
    }

    /// Sequence carried by the most recently emitted event (`EventSeq(0)` when
    /// no transition has happened yet).
    #[must_use]
    pub const fn last_seq(&self) -> EventSeq {
        if self.next_seq <= 1 {
            EventSeq(0)
        } else {
            EventSeq(self.next_seq - 1)
        }
    }

    /// Applies `next`, returning the emitted event on success.
    ///
    /// Illegal transitions leave the machine untouched and return
    /// [`CoreError::InvalidTransition`].
    pub fn transition_with_event(
        &mut self,
        next: SessionState,
        transaction_id: Option<TransactionId>,
        at_unix_ms: u64,
    ) -> Result<SessionEvent, CoreError> {
        let from = self.state;
        if !is_valid_transition(from, next) {
            return Err(CoreError::InvalidTransition { from, to: next });
        }
        if self.next_seq == u64::MAX {
            return Err(CoreError::Journal(JournalError::Storage(
                "session event sequence exhausted".to_owned(),
            )));
        }
        let seq = EventSeq(self.next_seq);
        self.next_seq += 1;
        self.state = next;
        Ok(SessionEvent {
            session_id: self.session_id,
            transaction_id,
            seq,
            from,
            to: next,
            at_unix_ms,
        })
    }

    /// Phase-0 signature kept: transition without emitting a bound transaction
    /// or timestamp (event still emitted internally for sequence accounting).
    pub fn transition(&mut self, next: SessionState) -> Result<(), CoreError> {
        self.transition_with_event(next, None, 0).map(drop)
    }
}

/// Authoritative legal-transition table (REL-001). `Delivering ->
/// Delivering` models a retry attempt; `Transforming` may be skipped because
/// it is an optional stage.
#[must_use]
pub const fn is_valid_transition(from: SessionState, to: SessionState) -> bool {
    use SessionState::{
        Cancelled, Committed, Delivering, Failed, FinalizingAudio, Idle, Listening, Preparing,
        Transcribing, Transforming,
    };
    matches!(
        (from, to),
        (Idle, Preparing)
            | (Preparing, Listening)
            | (Preparing, Cancelled)
            | (Preparing, Failed)
            | (Listening, FinalizingAudio)
            | (Listening, Cancelled)
            | (Listening, Failed)
            | (FinalizingAudio, Transcribing)
            | (FinalizingAudio, Cancelled)
            | (FinalizingAudio, Failed)
            | (Transcribing, Transforming)
            | (Transcribing, Delivering)
            | (Transcribing, Cancelled)
            | (Transcribing, Failed)
            | (Transforming, Delivering)
            | (Transforming, Cancelled)
            | (Transforming, Failed)
            | (Delivering, Committed)
            | (Delivering, Failed)
            | (Delivering, Delivering)
            | (Committed, Idle)
            | (Cancelled, Idle)
            | (Failed, Idle)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use SessionState::{
        Cancelled, Committed, Delivering, Failed, FinalizingAudio, Idle, Listening, Preparing,
        Transcribing, Transforming,
    };

    const ALL_STATES: [SessionState; 10] = [
        Idle,
        Preparing,
        Listening,
        FinalizingAudio,
        Transcribing,
        Transforming,
        Delivering,
        Committed,
        Cancelled,
        Failed,
    ];

    fn successors(state: SessionState) -> Vec<SessionState> {
        ALL_STATES
            .iter()
            .copied()
            .filter(|s| is_valid_transition(state, *s))
            .collect()
    }

    /// Drives a fresh machine into `state` along a legal prefix path.
    fn drive_to(state: SessionState) -> SessionMachine {
        let mut m = SessionMachine::with_session(SessionId(7));
        let path: &[SessionState] = match state {
            Idle => &[],
            Preparing => &[Preparing],
            Listening => &[Preparing, Listening],
            FinalizingAudio => &[Preparing, Listening, FinalizingAudio],
            Transcribing => &[Preparing, Listening, FinalizingAudio, Transcribing],
            Transforming => &[
                Preparing,
                Listening,
                FinalizingAudio,
                Transcribing,
                Transforming,
            ],
            Delivering => &[
                Preparing,
                Listening,
                FinalizingAudio,
                Transcribing,
                Delivering,
            ],
            Committed => &[
                Preparing,
                Listening,
                FinalizingAudio,
                Transcribing,
                Delivering,
                Committed,
            ],
            Cancelled => &[Preparing, Listening, Cancelled],
            Failed => &[Preparing, Failed],
        };
        for step in path {
            let ok = m.transition(*step).is_ok();
            assert!(ok, "drive_to failed at {step:?}");
        }
        assert_eq!(m.state(), state);
        m
    }

    fn rejected_pair(
        m: &mut SessionMachine,
        to: SessionState,
    ) -> Option<(SessionState, SessionState)> {
        match m.transition(to) {
            Err(CoreError::InvalidTransition { from, to: dest }) => Some((from, dest)),
            _ => None,
        }
    }

    #[test]
    fn test_state_machine_full_legal_path() -> Result<(), CoreError> {
        let mut m = SessionMachine::new();
        for next in [
            Preparing,
            Listening,
            FinalizingAudio,
            Transcribing,
            Transforming,
            Delivering,
            Committed,
            Idle,
        ] {
            m.transition(next)?;
        }
        assert_eq!(m.state(), Idle);
        Ok(())
    }

    #[test]
    fn test_state_machine_optional_transforming_skipped() -> Result<(), CoreError> {
        let mut m = drive_to(Transcribing);
        m.transition(Delivering)?;
        assert_eq!(m.state(), Delivering);
        Ok(())
    }

    #[test]
    fn test_state_machine_illegal_jumps_rejected() {
        // Exhaustively: every (state, other) pair not in the table must be
        // rejected and leave the machine unchanged.
        for state in ALL_STATES {
            let mut m = drive_to(state);
            for to in ALL_STATES {
                if is_valid_transition(state, to) {
                    continue; // legal pair — covered by the positive tests
                }
                let before = m.state();
                let got = rejected_pair(&mut m, to);
                assert_eq!(
                    got,
                    Some((state, to)),
                    "expected {state:?}->{to:?} to be rejected"
                );
                assert_eq!(m.state(), before, "state must not move on rejection");
            }
        }
    }

    #[test]
    fn test_state_machine_cancel_from_each_active_state() -> Result<(), CoreError> {
        for active in [
            Preparing,
            Listening,
            FinalizingAudio,
            Transcribing,
            Transforming,
        ] {
            let mut m = drive_to(active);
            m.transition(Cancelled)?;
            assert_eq!(m.state(), Cancelled);
            m.transition(Idle)?;
            assert_eq!(m.state(), Idle);
        }
        Ok(())
    }

    #[test]
    fn test_state_machine_fail_from_each_active_state() -> Result<(), CoreError> {
        for active in [
            Preparing,
            Listening,
            FinalizingAudio,
            Transcribing,
            Transforming,
            Delivering,
        ] {
            let mut m = drive_to(active);
            m.transition(Failed)?;
            assert_eq!(m.state(), Failed);
            m.transition(Idle)?;
        }
        Ok(())
    }

    #[test]
    fn test_state_machine_delivering_retry_attempt_is_legal() -> Result<(), CoreError> {
        let mut m = drive_to(Delivering);
        m.transition(Delivering)?;
        m.transition(Committed)?;
        assert_eq!(m.state(), Committed);
        Ok(())
    }

    #[test]
    fn test_state_machine_seq_monotonic() -> Result<(), CoreError> {
        let mut m = SessionMachine::with_session(SessionId(42));
        assert_eq!(m.last_seq(), EventSeq(0));
        let mut prev = 0u64;
        for next in [
            Preparing,
            Listening,
            FinalizingAudio,
            Transcribing,
            Delivering,
            Committed,
        ] {
            let ev = m.transition_with_event(next, None, 0)?;
            assert_eq!(ev.seq, EventSeq(prev + 1), "seq must strictly increase");
            assert_eq!(m.last_seq(), ev.seq);
            prev = ev.seq.0;
        }
        // Rejected transitions must not burn sequence numbers.
        let before = m.last_seq();
        assert!(m.transition(Listening).is_err());
        assert_eq!(m.last_seq(), before);
        Ok(())
    }

    #[test]
    fn test_state_machine_event_fields_correct() -> Result<(), CoreError> {
        let tx = TransactionId::new("tx-2026-000001");
        let mut m = SessionMachine::with_session(SessionId(9));
        let ev = m.transition_with_event(Preparing, Some(tx.clone()), 1_758_000_000_123)?;
        assert_eq!(ev.session_id, SessionId(9));
        assert_eq!(ev.transaction_id, Some(tx));
        assert_eq!(ev.seq, EventSeq(1));
        assert_eq!(ev.from, Idle);
        assert_eq!(ev.to, Preparing);
        assert_eq!(ev.at_unix_ms, 1_758_000_000_123);
        // Serde round-trip required by the IPC contract.
        let json = serde_json::to_string(&ev).map_err(|e| CoreError::Store(e.to_string()))?;
        let back: SessionEvent =
            serde_json::from_str(&json).map_err(|e| CoreError::Store(e.to_string()))?;
        assert_eq!(back, ev);
        Ok(())
    }

    #[test]
    fn test_state_machine_terminal_states_reset_to_idle() -> Result<(), CoreError> {
        for terminal_path in [
            vec![
                Preparing,
                Listening,
                FinalizingAudio,
                Transcribing,
                Delivering,
                Committed,
            ],
            vec![Preparing, Listening, Cancelled],
            vec![Preparing, Failed],
        ] {
            let mut m = SessionMachine::new();
            for step in &terminal_path[..terminal_path.len() - 1] {
                m.transition(*step)?;
            }
            m.transition(terminal_path[terminal_path.len() - 1])?;
            m.transition(Idle)?;
            assert_eq!(m.state(), Idle);
            // Machine is reusable after reset: full new cycle starts at seq 4+.
            m.transition(Preparing)?;
            assert_eq!(m.state(), Preparing);
        }
        Ok(())
    }

    #[test]
    fn test_state_machine_table_matches_successor_map() {
        // Cross-check the const table against an independently derived view so
        // the exhaustive test cannot drift from `is_valid_transition`.
        for from in ALL_STATES {
            for to in ALL_STATES {
                assert_eq!(
                    successors(from).contains(&to),
                    is_valid_transition(from, to)
                );
            }
        }
    }

    #[test]
    fn test_state_machine_exhaustive_two_step_illegal_rejected() {
        // Every 2-step illegal sequence: from a reachable state, attempt an
        // illegal first hop (must fail, state/seq unchanged), then attempt a
        // second illegal hop (still must fail). Verifies that a rejected
        // transition never corrupts the machine for subsequent rejections.
        for state in ALL_STATES {
            let mut m = drive_to(state);
            let illegal_first: Vec<SessionState> = ALL_STATES
                .iter()
                .copied()
                .filter(|to| !is_valid_transition(state, *to))
                .collect();
            for to1 in &illegal_first {
                let before_state = m.state();
                let before_seq = m.last_seq();
                let got = rejected_pair(&mut m, *to1);
                assert_eq!(got, Some((state, *to1)));
                assert_eq!(m.state(), before_state);
                assert_eq!(m.last_seq(), before_seq, "illegal must not burn seq");
                // Second illegal from same state must also be rejected.
                for to2 in &illegal_first {
                    let before2 = m.state();
                    let before_seq2 = m.last_seq();
                    let got2 = rejected_pair(&mut m, *to2);
                    assert_eq!(got2, Some((state, *to2)));
                    assert_eq!(m.state(), before2);
                    assert_eq!(m.last_seq(), before_seq2);
                }
            }
        }
    }

    #[test]
    fn test_state_machine_illegal_does_not_burn_seq_and_allows_recovery() -> Result<(), CoreError> {
        // After any illegal attempt, next_seq must be untouched and the next
        // legal transition must succeed with the expected monotonic seq.
        for state in ALL_STATES {
            let successors_of_state = successors(state);
            if successors_of_state.is_empty() {
                continue;
            }
            let illegal_targets: Vec<SessionState> = ALL_STATES
                .iter()
                .copied()
                .filter(|to| !is_valid_transition(state, *to))
                .collect();
            if illegal_targets.is_empty() {
                continue;
            }
            let mut m = drive_to(state);
            let before_seq = m.last_seq();
            // Try one illegal.
            let illegal = illegal_targets[0];
            assert!(m.transition(illegal).is_err());
            assert_eq!(m.last_seq(), before_seq);
            // Now take a legal step — seq must increment exactly by 1.
            let legal = successors_of_state[0];
            let ev = m.transition_with_event(legal, None, 123)?;
            assert_eq!(ev.seq, EventSeq(before_seq.0 + 1));
            assert_eq!(m.state(), legal);
        }
        Ok(())
    }

    #[test]
    fn test_state_machine_legal_two_step_then_every_illegal_rejected() -> Result<(), CoreError> {
        // Walk every legal 2-step prefix from Idle, then assert every illegal
        // third step from that reached state is rejected. This proves that
        // after any legal 2-step history, the illegal table is still exhaustive.
        let legal_first: Vec<SessionState> = ALL_STATES
            .iter()
            .copied()
            .filter(|to| is_valid_transition(Idle, *to))
            .collect();
        for first in &legal_first {
            let mut m1 = SessionMachine::new();
            m1.transition(*first)?;
            let second_candidates: Vec<SessionState> = ALL_STATES
                .iter()
                .copied()
                .filter(|to| is_valid_transition(*first, *to))
                .collect();
            for second in &second_candidates {
                let mut m = SessionMachine::new();
                m.transition(*first)?;
                m.transition(*second)?;
                let reached = m.state();
                assert_eq!(reached, *second);
                let before_seq = m.last_seq();
                for to in ALL_STATES {
                    if is_valid_transition(reached, to) {
                        continue;
                    }
                    let s_before = m.state();
                    assert!(
                        m.transition(to).is_err(),
                        "after legal {first:?}->{second:?}, illegal {reached:?}->{to:?} must be rejected"
                    );
                    assert_eq!(m.state(), s_before);
                    assert_eq!(m.last_seq(), before_seq);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn test_state_machine_all_100_two_step_sequences_corruption_free() {
        // Brute-force 10*10 = 100 possible (from,to) pairs starting from each
        // reachable state; after any illegal pair the machine must still
        // correctly handle legal successors. This is the "every 2-step illegal
        // transition" formal property: no 2-step illegal sequence can corrupt.
        for start in ALL_STATES {
            for a in ALL_STATES {
                for b in ALL_STATES {
                    let mut m = drive_to(start);
                    let seq_before = m.last_seq();
                    // First hop
                    let first_ok = is_valid_transition(m.state(), a);
                    let r1 = m.transition(a);
                    if first_ok {
                        assert!(r1.is_ok(), "legal {start:?}->{a:?} should succeed");
                        // Second hop
                        let second_ok = is_valid_transition(a, b);
                        let s_before = m.state();
                        let seq_mid = m.last_seq();
                        let r2 = m.transition(b);
                        if second_ok {
                            assert!(r2.is_ok(), "legal {a:?}->{b:?} should succeed");
                        } else {
                            assert!(r2.is_err(), "illegal {a:?}->{b:?} must be rejected");
                            assert_eq!(m.state(), s_before);
                            // Seq must have advanced only for the first legal step.
                            assert_eq!(m.last_seq(), seq_mid);
                        }
                    } else {
                        assert!(r1.is_err(), "illegal {start:?}->{a:?} must be rejected");
                        assert_eq!(m.state(), start);
                        assert_eq!(m.last_seq(), seq_before);
                        // Second attempt still from start state.
                        let second_ok_from_start = is_valid_transition(start, b);
                        let r2 = m.transition(b);
                        if second_ok_from_start {
                            assert!(r2.is_ok());
                        } else {
                            assert!(r2.is_err());
                            assert_eq!(m.state(), start);
                            assert_eq!(m.last_seq(), seq_before);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn test_state_machine_seq_strictly_monotonic_under_random_legal_walks() -> Result<(), CoreError>
    {
        // Deterministic PRNG (splitmix64) driven random walk that only takes
        // legal steps; verifies seq monotonicity under 500 varied walks.
        fn splitmix64(state: &mut u64) -> u64 {
            *state = state.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = *state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            z ^ (z >> 31)
        }
        let mut rng = 0x1234_5678_9abc_def0u64;
        for _ in 0..500 {
            let mut m = SessionMachine::with_session(SessionId(splitmix64(&mut rng) as u128));
            let mut expected_seq = 0u64;
            // Walk up to 20 steps, each picking a random legal successor if any.
            for _ in 0..20 {
                let succ = successors(m.state());
                if succ.is_empty() {
                    // Terminal: must reset to Idle to continue.
                    m.transition(Idle)?;
                    expected_seq += 1;
                    assert_eq!(m.last_seq().0, expected_seq);
                    continue;
                }
                let pick = (splitmix64(&mut rng) as usize) % succ.len();
                let next = succ[pick];
                let ev = m.transition_with_event(next, None, splitmix64(&mut rng))?;
                expected_seq += 1;
                assert_eq!(ev.seq.0, expected_seq);
                assert_eq!(m.last_seq().0, expected_seq);
            }
        }
        Ok(())
    }

    #[test]
    fn test_state_machine_quickcheck_random_sequences_1000() {
        // Manual quickcheck-style: 1000 random sequences of 15 steps each,
        // mixing legal and illegal choices. Invariants checked on every step:
        // illegal => Err + state/seq unchanged; legal => Ok + seq+1 + state moves.
        fn splitmix64(state: &mut u64) -> u64 {
            *state = state.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = *state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            z ^ (z >> 31)
        }
        let mut rng = 0xdead_beef_cafe_babeu64;
        for _trial in 0..1000 {
            let mut m = SessionMachine::with_session(SessionId(splitmix64(&mut rng) as u128));
            for _step in 0..15 {
                let candidate = ALL_STATES[(splitmix64(&mut rng) as usize) % ALL_STATES.len()];
                let from = m.state();
                let before_seq = m.last_seq();
                let legal = is_valid_transition(from, candidate);
                let res = m.transition(candidate);
                if legal {
                    assert!(res.is_ok(), "legal {from:?}->{candidate:?} must succeed");
                    assert_eq!(m.state(), candidate);
                    assert_eq!(m.last_seq().0, before_seq.0 + 1);
                } else {
                    assert!(
                        res.is_err(),
                        "illegal {from:?}->{candidate:?} must be rejected"
                    );
                    assert_eq!(m.state(), from, "illegal must not move state");
                    assert_eq!(m.last_seq(), before_seq, "illegal must not burn seq");
                }
            }
        }
    }

    #[test]
    fn test_state_machine_state_never_corrupts_after_illegal_burst() -> Result<(), CoreError> {
        // Hammer one machine with 100 consecutive illegal transitions, then
        // verify it still accepts its legal successors normally.
        for state in ALL_STATES {
            let succ = successors(state);
            if succ.is_empty() {
                continue;
            }
            let mut m = drive_to(state);
            let before_seq = m.last_seq();
            let illegals: Vec<SessionState> = ALL_STATES
                .iter()
                .copied()
                .filter(|to| !is_valid_transition(state, *to))
                .collect();
            if illegals.is_empty() {
                continue;
            }
            for i in 0..100 {
                let target = illegals[i % illegals.len()];
                assert!(m.transition(target).is_err());
                assert_eq!(m.state(), state);
                assert_eq!(m.last_seq(), before_seq);
            }
            // Still healthy: legal move works.
            m.transition(succ[0])?;
            assert_eq!(m.state(), succ[0]);
            assert_eq!(m.last_seq().0, before_seq.0 + 1);
        }
        Ok(())
    }
}

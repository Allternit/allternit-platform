//! Meeting session state machine and the per-meeting `gizzi` status
//! (docs/meetings/CONTRACT.md section 3). Pure functions, no I/O.
//!
//! `scheduled -> launching -> joining -> waiting_admit -> live <-> paused -> ended`,
//! plus `joining -> live` (open call, no lobby) and any non-ended state
//! `-> ended`. `end_reason` is set exactly when the state is `ended`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Scheduled,
    Launching,
    Joining,
    WaitingAdmit,
    Live,
    Paused,
    Ended,
}

impl State {
    pub const ALL: [State; 7] = [
        State::Scheduled,
        State::Launching,
        State::Joining,
        State::WaitingAdmit,
        State::Live,
        State::Paused,
        State::Ended,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            State::Scheduled => "scheduled",
            State::Launching => "launching",
            State::Joining => "joining",
            State::WaitingAdmit => "waiting_admit",
            State::Live => "live",
            State::Paused => "paused",
            State::Ended => "ended",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        State::ALL.into_iter().find(|state| state.as_str() == s)
    }

    /// A pod is (or is about to be) in the call: holds capacity, blocks
    /// deleting the meeting. `scheduled` has no pod yet.
    pub fn holds_pod(self) -> bool {
        !matches!(self, State::Scheduled | State::Ended)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    LeftByUser,
    Removed,
    NotAdmitted,
    EmptyRoom,
    MaxDuration,
    OtherNotetaker,
    JoinFailed,
    GuestBlocked,
    NoCapacity,
    Error,
}

impl EndReason {
    pub const ALL: [EndReason; 10] = [
        EndReason::LeftByUser,
        EndReason::Removed,
        EndReason::NotAdmitted,
        EndReason::EmptyRoom,
        EndReason::MaxDuration,
        EndReason::OtherNotetaker,
        EndReason::JoinFailed,
        EndReason::GuestBlocked,
        EndReason::NoCapacity,
        EndReason::Error,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EndReason::LeftByUser => "left_by_user",
            EndReason::Removed => "removed",
            EndReason::NotAdmitted => "not_admitted",
            EndReason::EmptyRoom => "empty_room",
            EndReason::MaxDuration => "max_duration",
            EndReason::OtherNotetaker => "other_notetaker",
            EndReason::JoinFailed => "join_failed",
            EndReason::GuestBlocked => "guest_blocked",
            EndReason::NoCapacity => "no_capacity",
            EndReason::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        EndReason::ALL.into_iter().find(|reason| reason.as_str() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// Already in the requested state (a repeated pod report): nothing to do.
    Unchanged,
    To(State),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionError {
    /// The machine has no edge `from -> to`.
    Illegal { from: State, to: State },
    /// `-> ended` without an `end_reason`.
    MissingEndReason,
    /// An `end_reason` on a transition that does not end the session.
    UnexpectedEndReason,
}

impl TransitionError {
    pub fn message(&self) -> String {
        match self {
            TransitionError::Illegal { from, to } => {
                format!("A session cannot go from {} to {}", from.as_str(), to.as_str())
            }
            TransitionError::MissingEndReason => "Ending a session needs an end_reason".to_string(),
            TransitionError::UnexpectedEndReason => {
                "end_reason is only set when the session ends".to_string()
            }
        }
    }
}

fn edge(from: State, to: State) -> bool {
    use State::*;
    match (from, to) {
        (Ended, _) => false,
        (_, Ended) => true,
        (Scheduled, Launching)
        | (Launching, Joining)
        | (Joining, WaitingAdmit)
        | (Joining, Live)
        | (WaitingAdmit, Live)
        | (Live, Paused)
        | (Paused, Live) => true,
        _ => false,
    }
}

/// Validate `from -> to`. Same-state reports are `Unchanged` (pods resend
/// their state in every batch), except that an ended session stays ended.
pub fn transition(
    from: State,
    to: State,
    end_reason: Option<EndReason>,
) -> Result<Transition, TransitionError> {
    if to != State::Ended && end_reason.is_some() {
        return Err(TransitionError::UnexpectedEndReason);
    }
    if from == to {
        return Ok(Transition::Unchanged);
    }
    if !edge(from, to) {
        return Err(TransitionError::Illegal { from, to });
    }
    if to == State::Ended && end_reason.is_none() {
        return Err(TransitionError::MissingEndReason);
    }
    Ok(Transition::To(to))
}

/// `gizzi` for one session (the live page's status): `not_admitted` for a
/// session that ended because the bot was not let in, else the state.
pub fn session_gizzi(state: State, end_reason: Option<EndReason>) -> &'static str {
    match (state, end_reason) {
        (State::Ended, Some(EndReason::NotAdmitted | EndReason::GuestBlocked)) => "not_admitted",
        (state, _) => state.as_str(),
    }
}

/// `gizzi` for a meeting row, first match wins (contract section 3):
/// the latest session's outcome or state; else `scheduled` when Gizzi will
/// auto-join a future meeting; else `off`.
pub fn meeting_gizzi(
    latest: Option<(State, Option<EndReason>)>,
    will_auto_join: bool,
    starts_in_future: bool,
) -> &'static str {
    if let Some((state, reason)) = latest {
        return session_gizzi(state, reason);
    }
    if will_auto_join && starts_in_future {
        "scheduled"
    } else {
        "off"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use State::*;

    #[test]
    fn the_happy_path_and_the_lobby_skip_are_legal() {
        let path = [Scheduled, Launching, Joining, WaitingAdmit, Live, Paused, Live];
        for pair in path.windows(2) {
            assert_eq!(transition(pair[0], pair[1], None), Ok(Transition::To(pair[1])), "{pair:?}");
        }
        assert_eq!(transition(Joining, Live, None), Ok(Transition::To(Live)));
    }

    #[test]
    fn every_open_state_can_end_with_a_reason() {
        for from in State::ALL.into_iter().filter(|s| *s != Ended) {
            assert_eq!(
                transition(from, Ended, Some(EndReason::JoinFailed)),
                Ok(Transition::To(Ended)),
                "{from:?}"
            );
            assert_eq!(transition(from, Ended, None), Err(TransitionError::MissingEndReason));
        }
    }

    #[test]
    fn illegal_transitions_are_rejected() {
        for (from, to) in [
            (Scheduled, Live),
            (Scheduled, Joining),
            (Launching, Live),
            (Launching, WaitingAdmit),
            (WaitingAdmit, Joining),
            (Live, Joining),
            (Live, WaitingAdmit),
            (Paused, WaitingAdmit),
            (Joining, Paused),
            (Ended, Live),
            (Ended, Scheduled),
        ] {
            assert_eq!(
                transition(from, to, None),
                Err(TransitionError::Illegal { from, to }),
                "{from:?} -> {to:?}"
            );
        }
        // Count: exactly 7 forward edges + 6 "-> ended" edges exist.
        let legal = State::ALL
            .iter()
            .flat_map(|f| State::ALL.iter().map(move |t| (*f, *t)))
            .filter(|(f, t)| f != t && edge(*f, *t))
            .count();
        assert_eq!(legal, 13);
    }

    #[test]
    fn repeats_are_unchanged_and_ended_is_terminal() {
        assert_eq!(transition(Live, Live, None), Ok(Transition::Unchanged));
        assert_eq!(transition(Ended, Ended, Some(EndReason::Removed)), Ok(Transition::Unchanged));
        assert_eq!(
            transition(Ended, Paused, None),
            Err(TransitionError::Illegal { from: Ended, to: Paused })
        );
    }

    #[test]
    fn end_reason_only_with_ended() {
        assert_eq!(
            transition(Joining, Live, Some(EndReason::Error)),
            Err(TransitionError::UnexpectedEndReason)
        );
    }

    #[test]
    fn names_round_trip() {
        for s in State::ALL {
            assert_eq!(State::parse(s.as_str()), Some(s));
        }
        for r in EndReason::ALL {
            assert_eq!(EndReason::parse(r.as_str()), Some(r));
        }
        assert_eq!(State::parse("running"), None);
        assert_eq!(EndReason::parse("kicked"), None);
    }

    #[test]
    fn gizzi_derivation_first_match_wins() {
        assert_eq!(meeting_gizzi(Some((Ended, Some(EndReason::NotAdmitted))), true, true), "not_admitted");
        assert_eq!(meeting_gizzi(Some((Ended, Some(EndReason::GuestBlocked))), false, false), "not_admitted");
        assert_eq!(meeting_gizzi(Some((Ended, Some(EndReason::JoinFailed))), true, true), "ended");
        assert_eq!(meeting_gizzi(Some((WaitingAdmit, None)), false, false), "waiting_admit");
        assert_eq!(meeting_gizzi(None, true, true), "scheduled");
        assert_eq!(meeting_gizzi(None, true, false), "off", "past meeting never shows scheduled");
        assert_eq!(meeting_gizzi(None, false, true), "off");
    }
}

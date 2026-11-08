//! One current-intent startup barrier; optional-track failures after admission leave siblings alone.
use crate::{owned_child::{ChildOps, ExitObservation, OwnedTrack, SharedRegistry, TrackOutcome},
    protocol::{self, State, TrackKind, TrackRequest}, Result};
use std::{path::Path,time::{Duration,Instant}};
#[derive(Debug,Clone)]
pub struct SessionPoll {
    pub ready:bool,
    pub failed:bool,
    pub phases:Vec<(TrackKind,State)>,
    pub notices:Vec<(TrackKind,String)>,
}
pub struct AudioSession {
    tracks:Vec<OwnedTrack>,
    outcomes:Vec<TrackOutcome>,
    admitted:bool,
    failed:bool,
    admission_deadline:Instant,
}
impl AudioSession {
    pub fn begin(exe:&Path,requests:Vec<TrackRequest>,now:Instant)->Result<Self> {
        Self::begin_tracks(requests,now,|request,deadline|OwnedTrack::begin(exe,request,deadline))
    }
    pub fn begin_with(requests:Vec<TrackRequest>,now:Instant,registry:SharedRegistry,
        mut nonce:impl FnMut()->Result<String>,
        mut spawn:impl FnMut(&TrackRequest)->Result<Box<dyn ChildOps+Send>>)->Result<Self> {
        Self::begin_tracks(requests,now,|request,deadline| {
            let nonce=nonce()?;
            let child_request=request.clone();
            OwnedTrack::begin_with(request,deadline,nonce,registry.clone(),||spawn(&child_request))
        })
    }
    fn begin_tracks(requests:Vec<TrackRequest>,now:Instant,
        mut begin:impl FnMut(TrackRequest,Instant)->Result<OwnedTrack>)->Result<Self> {
        protocol::validate_requests(&requests).map_err(|e|e.to_string())?;
        let deadline=now.checked_add(Duration::from_millis(protocol::STARTUP_MS)).ok_or("Audio admission clock overflow")?;
        let mut session=Self{tracks:Vec::with_capacity(requests.len()),outcomes:Vec::with_capacity(requests.len()),admitted:false,failed:false,admission_deadline:deadline};
        for request in requests {
            let kind=request.kind;
            let result=if session.failed {Err("Audio admission cancelled because a requested track failed".into())}
                else {begin(request,deadline)};
            match result {
                Ok(track)=>{if track.failure().is_some(){session.failed=true;}session.tracks.push(track);},
                Err(error)=>{
                    session.failed=true;
                    session.outcomes.push(TrackOutcome{kind,terminal:None,exit:ExitObservation::NotStarted,
                        exit_code:None,usable:None,failure:Some(error)});
                }
            }
        }
        if session.failed {session.signal_all_stops(now);}
        Ok(session)
    }
    /// Every direct event is attempted before the caller can proceed to video wait or cleanup.
    pub fn signal_all_stops(&mut self,now:Instant)->Vec<(TrackKind,String)> {
        let mut errors=Vec::new();
        for track in &mut self.tracks {
            if let Err(error)=track.request_stop(now) {errors.push((track.kind(),error));}
        }
        errors
    }
    /// Caller cancellation (including metadata failure) rolls back admission without losing owners.
    pub fn cancel(&mut self,now:Instant)->Vec<(TrackKind,String)> {
        if !self.admitted {self.failed=true;}
        let mut errors=self.signal_all_stops(now);
        for track in &mut self.tracks {
            if let Err(error)=track.cancel_storage(now) {errors.push((track.kind(),error));}
        }
        errors
    }
    pub fn poll(&mut self,now:Instant)->SessionPoll {
        let mut notices=Vec::new();let mut rollback=!self.admitted && !self.tracks.is_empty() && now>=self.admission_deadline;
        for track in &mut self.tracks {
            let poll=track.poll(now);
            if let Some(notice)=poll.notice {notices.push((track.kind(),notice));}
            if !self.admitted && (track.failure().is_some() || !matches!(poll.phase,State::Starting|State::Running|State::Recovering)) {
                rollback=true;
            }
            if let Some(outcome)=poll.outcome {
                if let Some(old)=self.outcomes.iter_mut().find(|old|old.kind==outcome.kind) {*old=outcome;}
                else {self.outcomes.push(outcome);}
            }
        }
        if rollback && !self.failed {self.failed=true;notices.extend(self.signal_all_stops(now));}
        SessionPoll{ready:!self.failed&&!self.admitted&&self.tracks.iter().all(|track|track.phase()==State::Running),failed:self.failed,phases:self.tracks.iter().map(|track|(track.kind(),track.phase())).collect(),notices}
    }
    /// Commit only after a final real nonblocking live poll of every requested track.
    pub fn video_admitted(&mut self)->Result<()> {
        if self.admitted {return Ok(());}
        let poll=self.poll(Instant::now());
        if !poll.ready {return Err("Requested audio tracks are not all ready; video admission blocked".into());}
        self.admitted=true;Ok(())
    }
    pub fn outcomes(&self)->&[TrackOutcome] {&self.outcomes}
}
impl Drop for AudioSession {
    fn drop(&mut self) {
        self.signal_all_stops(Instant::now());
        // Fields drop only after every sibling has received its initial direct Stop attempt.
    }
}

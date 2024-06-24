use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use web_time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{
    db::{BucketInfo, Data, Metadata},
    poll::Signal,
    room::Room,
};

const GRACE_PERIOD: u64 = 20;
pub const MAX_CONNECTION: u64 = 3600;
const FIRST_POLL: u64 = 1;
const POLL: u64 = 10;
const CONNECT: u64 = 5;
const FAST_POLL: u64 = 1;

pub type Auth = Data<AuthData, AuthMetadata, AuthInfo>;

pub struct AuthInfo {}
impl BucketInfo for AuthInfo {
    const PREFIX: &'static str = "auth";
    const KEY_LENGTH: u8 = 32;
}

#[derive(Serialize, Deserialize, Default)]
pub struct AuthData {
    sent_sdp: bool,
    ice_done: bool,
    queue: Vec<Signal>,
    read: usize,
    connect_at: Option<SystemTime>,
    sent_join: bool,
    read_connect: bool,
}

pub struct AuthMetadata {
    kill_at: SystemTime,
    next_poll: SystemTime,
    room: Option<String>,
    peer: Option<String>,
}
impl Default for AuthMetadata {
    fn default() -> Self {
        AuthMetadata {
            kill_at: SystemTime::now() + Duration::from_secs(MAX_CONNECTION),
            next_poll: SystemTime::now() + Duration::from_secs(FIRST_POLL),
            room: None,
            peer: None,
        }
    }
}
impl Metadata for AuthMetadata {}
impl From<HashMap<String, String>> for AuthMetadata {
    fn from(value: HashMap<String, String>) -> Self {
        let kill_at = value
            .get("kill_at")
            .filter(|v| !v.is_empty())
            .map(|v| v.parse().unwrap())
            .map(|v| UNIX_EPOCH + Duration::from_secs(v))
            .expect("missing kill_at");
        let next_poll = value
            .get("next_poll")
            .filter(|v| !v.is_empty())
            .map(|v| v.parse().unwrap())
            .map(|v| UNIX_EPOCH + Duration::from_secs(v))
            .expect("missing next_poll");
        let room = value.get("room").filter(|v| !v.is_empty()).cloned();
        let peer = value.get("peer").filter(|v| !v.is_empty()).cloned();

        AuthMetadata {
            kill_at,
            next_poll,
            room,
            peer,
        }
    }
}
impl From<AuthMetadata> for HashMap<String, String> {
    fn from(value: AuthMetadata) -> Self {
        let mut map = HashMap::new();
        let kill_at = value
            .kill_at
            .duration_since(UNIX_EPOCH)
            .expect("time travel on kill_at?")
            .as_secs()
            .to_string();
        let next_poll = value
            .next_poll
            .duration_since(UNIX_EPOCH)
            .expect("time travel on next_poll?")
            .as_secs()
            .to_string();
        let room = value.room.unwrap_or_default();
        let peer = value.peer.unwrap_or_default();

        map.insert("kill_at".to_owned(), kill_at);
        map.insert("next_poll".to_owned(), next_poll);
        map.insert("room".to_owned(), room);
        map.insert("peer".to_owned(), peer);
        map
    }
}

impl Auth {
    pub fn set_room(&mut self, room: &Room) {
        self.meta.room = Some(room.key.clone());
        self.modified = true;
    }

    pub fn set_peer(&mut self, peer: Option<String>) {
        self.meta.peer = peer;
        self.modified = true;
    }

    pub fn get_room(&self) -> Option<&String> {
        self.meta.room.as_ref()
    }

    pub fn get_peer(&self) -> Option<&String> {
        self.meta.peer.as_ref()
    }

    pub fn poll(&mut self) {
        let secs = if self.meta.peer.is_some() {
            // Fast polling after both parties are connected
            FAST_POLL
        } else {
            POLL
        };
        self.meta.next_poll = SystemTime::now() + Duration::from_secs(secs);
        self.modified = true;
    }

    pub fn send_signal<S>(&mut self, signals: S)
    where
        S: IntoIterator<Item = Signal>,
    {
        let data = self.data.as_mut().expect("invalid state");

        for signal in signals.into_iter() {
            if !signal.can_send() {
                continue;
            }

            match signal {
                Signal::SetSDP(_) => {
                    if data.sent_sdp {
                        // Can't set SDP twice
                        continue;
                    }

                    data.sent_sdp = true;
                    self.modified = true;
                }
                Signal::AddCandidate(ref ice) => {
                    if data.ice_done {
                        // Already done with ICE candidates
                        continue;
                    }

                    if ice.0.is_empty() {
                        data.ice_done = true;
                        self.modified = true;
                    }
                }
                _ => {}
            };

            self.modified = true;
            data.queue.push(signal);
        }
    }

    fn read_signals(&mut self, peer: &Auth) -> Vec<Signal> {
        self.try_connect(peer);

        let data = self.data.as_mut().expect("invalid state");
        let queue = &peer.data.as_ref().expect("invalid state").queue;

        let signals = queue.get(data.read..).unwrap_or_default();
        data.read = queue.len();

        signals.to_vec()
    }

    pub fn pull_signals(&mut self, peer: Option<&Auth>) -> Vec<Signal> {
        let mut signals = match peer {
            Some(peer) => self.read_signals(peer),
            None => vec![],
        };

        let data = self.data.as_mut().expect("invalid state");
        if let Some(ref room) = self.meta.room {
            if !data.sent_join {
                data.sent_join = true;
                signals.push(Signal::JoinRoom(room.clone()));
            }
        };
        if let Some(at) = data.connect_at {
            if !data.read_connect {
                data.read_connect = true;
                signals.push(Signal::ConnectAt(at));
            }
        };
        signals.push(Signal::NextPoll(self.meta.next_poll));
        signals
    }

    pub fn try_connect(&mut self, peer: &Auth) {
        let s_data = self.data.as_mut().expect("invalid state");
        let p_data = peer.data.as_ref().expect("invalid state");

        if s_data.connect_at.is_some() {
            return;
        }
        if p_data.connect_at.is_some() {
            s_data.connect_at = p_data.connect_at;
            s_data.read_connect = false;
            self.modified = true;
            return;
        }

        // Need both SDPs
        if !s_data.sent_sdp || !p_data.sent_sdp {
            return;
        }
        // Need at least one ICE list to be done
        if !s_data.ice_done && !p_data.ice_done {
            return;
        }

        let at = peer.meta.next_poll + Duration::from_secs(CONNECT);
        s_data.connect_at = Some(at);
        s_data.read_connect = false;
        self.modified = true;
    }

    pub fn is_done(&self, peer: &Auth) -> bool {
        let s_data = self.data.as_ref().expect("invalid state");
        let p_data = peer.data.as_ref().expect("invalid state");

        // didn't establish a p2p connection
        if s_data.connect_at.is_none() {
            return false;
        }
        // not all ice candidates were sent
        if !s_data.ice_done || !p_data.ice_done {
            return false;
        }
        // not all messages have been read
        if p_data.queue.len() - s_data.read > 0 {
            return false;
        }

        true
    }

    pub fn is_alive(&self) -> bool {
        let limit = self
            .meta
            .kill_at
            .min(self.meta.next_poll + Duration::from_secs(GRACE_PERIOD));
        SystemTime::now() < limit
    }

    pub fn get_keys_to_kill(&self) -> Vec<String> {
        if self.is_alive() {
            return vec![];
        }

        let mut keys = vec![Self::get_bucket_key(&self.key)];
        match &self.meta.peer {
            Some(k) => keys.push(Self::get_bucket_key(k)),
            None => {}
        };
        match &self.meta.room {
            Some(k) => keys.push(Room::get_bucket_key(k)),
            None => {}
        };
        keys
    }
}

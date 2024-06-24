use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{
    auth::Auth,
    db::{BucketInfo, Data, Metadata},
};

pub type Room = Data<RoomData, RoomMetadata, RoomInfo>;

pub struct RoomInfo {}
impl BucketInfo for RoomInfo {
    const PREFIX: &'static str = "room";
    const KEY_LENGTH: u8 = 6;
}

#[derive(Serialize, Deserialize, Default)]
pub struct RoomData {
    offer: String,
    answer: Option<String>,
}

#[derive(Default)]
pub struct RoomMetadata {}

impl Metadata for RoomMetadata {}
impl From<HashMap<String, String>> for RoomMetadata {
    fn from(_: HashMap<String, String>) -> Self {
        RoomMetadata {}
    }
}
impl From<RoomMetadata> for HashMap<String, String> {
    fn from(_: RoomMetadata) -> Self {
        HashMap::new()
    }
}

impl Room {
    pub fn get_peer(&self, peer: &Auth) -> Option<String> {
        let data = self.data.as_ref().expect("invalid state");

        if peer.key == data.offer {
            return data.answer.clone();
        }
        Some(data.offer.clone())
    }

    pub fn join_room(&mut self, peer: &mut Auth) -> bool {
        let data = self.data.as_mut().expect("invalid state");

        let is_offer = data.offer.is_empty();
        let is_answer = data.answer.is_none();
        if !is_offer && !is_answer {
            return false;
        }

        if is_offer {
            data.offer = peer.key.clone();
        } else {
            data.answer = Some(peer.key.clone());
        }

        peer.set_room(self);
        self.modified = true;

        true
    }
}

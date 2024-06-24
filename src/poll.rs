use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use web_time::SystemTime;
use worker::{console_log, Bucket, Env, Include, Request, Response, Result};

use crate::{
    auth::{Auth, AuthInfo},
    db::BucketInfo,
    room::Room,
};

pub type IceCandidate = (String, Option<String>, Option<u16>);

#[derive(Serialize, Deserialize, Clone)]
pub enum Signal {
    SetSDP(String),
    AddCandidate(IceCandidate),
    JoinRoom(String),
    ConnectAt(SystemTime),
    NextPoll(SystemTime),
}

impl Signal {
    pub fn can_send(&self) -> bool {
        match self {
            Self::SetSDP(_) => true,
            Self::AddCandidate(_) => true,
            Self::JoinRoom(_) => false,
            Self::ConnectAt(_) => false,
            Self::NextPoll(_) => false,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct IdentResponse {
    token: String,
}

pub async fn ident(env: Env) -> Result<Response> {
    let bucket = env.bucket("rtc")?;
    let auth = Auth::create(&bucket).await?;
    let token = auth.key.clone();
    auth.write(&bucket).await?;
    Response::from_json(&IdentResponse { token })
}

pub async fn poll(mut req: Request, env: Env) -> Result<Response> {
    let token = match req.headers().get("Authorization")? {
        Some(token) => token,
        None => return Response::error("Missing token.", 403),
    };
    let signals = req.json::<Vec<Signal>>().await?;
    if signals
        .iter()
        .filter(|s| !matches!(s, Signal::JoinRoom(_)))
        .any(|s| !s.can_send())
    {
        return Response::error("Invalid signals: can't send.", 400);
    }

    let bucket = env.bucket("rtc")?;
    let mut user = match Auth::load(&bucket, &token).await? {
        Some(user) => user,
        None => return Response::error("Invalid token.", 403),
    };

    let peer = match user.get_peer() {
        Some(peer) => Some(peer.clone()),
        None => {
            let room = match user.get_room() {
                Some(code) => {
                    // User is in room
                    match Room::load(&bucket, code).await? {
                        Some(room) => room,
                        None => return Response::error("Room expired.", 400),
                    }
                }
                None => {
                    // Joining or creating
                    let room = match signals.iter().find(|s| matches!(s, Signal::JoinRoom(_))) {
                        Some(Signal::JoinRoom(code)) => Room::load(&bucket, code).await?,
                        None => Some(Room::create(&bucket).await?),
                        Some(_) => return Response::error("server logic error.", 500),
                    };
                    let mut room = match room {
                        Some(room) => room,
                        None => return Response::error("Room not found.", 404),
                    };
                    if !room.join_room(&mut user) {
                        return Response::error("Room is full.", 400);
                    };
                    room
                }
            };

            let peer = room.get_peer(&user).clone();
            room.write(&bucket).await?;
            user.set_peer(peer.clone());
            peer
        }
    };
    let peer = match peer {
        Some(peer) => Auth::load(&bucket, &peer).await?,
        None => None,
    };

    if let Some(ref peer) = peer {
        if user.is_done(peer) {
            return Response::error("Connection done.", 400);
        }
    }

    user.poll();
    user.send_signal(signals);
    let signals = user.pull_signals(peer.as_ref());
    user.write(&bucket).await?;

    Response::from_json(&signals)
}

pub async fn cleanup(bucket: Bucket) {
    let objects = bucket
        .list()
        .prefix(AuthInfo::PREFIX)
        .include(vec![Include::CustomMetadata])
        .execute()
        .await
        .expect("couldn't list objects");

    let mut to_delete = HashSet::new();
    for obj in objects.objects().iter() {
        to_delete.extend(
            Auth::read(obj)
                .await
                .unwrap_or_else(|_| panic!("couldn't read object {}", obj.key()))
                .get_keys_to_kill(),
        );
    }

    console_log!("deleting {:?}", to_delete);
    for key in to_delete.iter() {
        bucket.delete(key).await.unwrap();
    }
}

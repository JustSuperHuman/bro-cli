//! Frames on the socket: `u32` little-endian length of the rest, a kind byte, then the body. Control messages
//! carry JSON; session bytes (input, output, snapshots) carry `[u8 id length][id][bytes]` so output is never
//! re-encoded.

use crate::answer::Profile;
use crate::{Peers, Role, SessionInfo};
use std::io::{self, Read, Write};

/// Client → server.
pub(crate) const HELLO: u8 = 1;
pub(crate) const SPAWN: u8 = 2;
pub(crate) const INPUT: u8 = 3;
pub(crate) const RESIZE: u8 = 4;
pub(crate) const KILL: u8 = 5;
pub(crate) const ATTACH: u8 = 6;
pub(crate) const BLOB: u8 = 7;
pub(crate) const CLOSE: u8 = 8;
pub(crate) const SHUTDOWN: u8 = 9;
pub(crate) const LIST: u8 = 10;
pub(crate) const SET: u8 = 11;
/// What this client's terminal looks like (the answers to the program's queries follow it).
pub(crate) const PROFILE: u8 = 12;
/// A message for the primary (a viewer asking it to launch something).
pub(crate) const RELAY: u8 = 13;
/// Lost sessions a window brought back or let go.
pub(crate) const FORGET_LOST: u8 = 14;
/// Stop watching a session (it keeps running).
pub(crate) const DETACH: u8 = 15;

/// Server → client.
pub(crate) const WELCOME: u8 = 101;
pub(crate) const REFUSED: u8 = 102;
pub(crate) const SPAWNED: u8 = 103;
pub(crate) const SNAPSHOT: u8 = 104;
pub(crate) const OUTPUT: u8 = 105;
pub(crate) const EXIT: u8 = 106;
pub(crate) const TAKEN: u8 = 107;
pub(crate) const LISTED: u8 = 108;
/// Another client started a session.
pub(crate) const ADDED: u8 = 109;
/// Another client changed a session's metadata (a rename).
pub(crate) const BLOBBED: u8 = 110;
/// A session's size changed: in line with its output, so a screen resizes exactly between the bytes before and
/// after.
pub(crate) const RESIZED: u8 = 111;
/// Whether this client controls a session now (its size is this client's).
pub(crate) const CONTROL: u8 = 112;
/// A viewer's message, for the primary.
pub(crate) const RELAYED: u8 = 113;
/// A relayed message had no primary to go to.
pub(crate) const UNRELAYED: u8 = 114;
/// Nothing else to say: the connection is alive (a client that hears nothing for a while takes it as lost).
pub(crate) const PING: u8 = 115;
/// Who's connected changed.
pub(crate) const PEERS: u8 = 116;
/// The server was asked to stop (`bro host stop`, a restart): its sessions end, and it isn't coming back.
pub(crate) const STOPPING: u8 = 117;

/// A frame larger than this is a broken peer, not a message.
const MAX_FRAME: usize = 256 << 20;

/// One encoded frame.
pub(crate) fn frame(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + body.len());
    out.extend_from_slice(&((body.len() + 1) as u32).to_le_bytes());
    out.push(kind);
    out.extend_from_slice(body);
    out
}

/// A frame with a JSON body.
pub(crate) fn json(kind: u8, value: &impl serde::Serialize) -> Vec<u8> {
    frame(kind, &serde_json::to_vec(value).unwrap_or_default())
}

/// A frame carrying one session's bytes.
pub(crate) fn data(kind: u8, id: &str, bytes: &[u8]) -> Vec<u8> {
    let id = &id.as_bytes()[..id.len().min(255)];
    let mut out = Vec::with_capacity(6 + id.len() + bytes.len());
    out.extend_from_slice(&((bytes.len() + id.len() + 2) as u32).to_le_bytes());
    out.push(kind);
    out.push(id.len() as u8);
    out.extend_from_slice(id);
    out.extend_from_slice(bytes);
    out
}

/// The session id and bytes of a [`data`] body.
pub(crate) fn split_data(body: &[u8]) -> Option<(&str, &[u8])> {
    let (&n, rest) = body.split_first()?;
    let n = n as usize;
    if rest.len() < n {
        return None;
    }
    Some((std::str::from_utf8(&rest[..n]).ok()?, &rest[n..]))
}

pub(crate) fn read_frame(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("bad frame length {len}")));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    let kind = buf.remove(0);
    Ok((kind, buf))
}

pub(crate) fn write_all(w: &mut impl Write, frame: &[u8]) -> io::Result<()> {
    w.write_all(frame)?;
    w.flush()
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub(crate) struct Hello {
    pub protocol: u32,
    pub token: String,
    pub role: Role,
    /// The client's process (shown in status).
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub profile: Option<Profile>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub(crate) struct Refused {
    pub reason: String,
    /// The server's protocol, so the window can say why.
    pub protocol: u32,
    /// A window drives the server (refused to the background bro, which doesn't take over).
    #[serde(default)]
    pub busy: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub(crate) struct Spawned {
    pub id: String,
    pub pid: Option<u32>,
    pub error: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub(crate) struct Id {
    pub id: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub(crate) struct Resize {
    pub id: String,
    pub rows: u16,
    pub cols: u16,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub(crate) struct Blob {
    pub id: String,
    pub blob: serde_json::Value,
    /// What's written to disk to bring it back (see [`crate::SpawnRequest::keep`]).
    #[serde(default)]
    pub keep: serde_json::Value,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub(crate) struct Exit {
    pub id: String,
    pub code: Option<i32>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub(crate) struct Set {
    pub key: String,
    pub value: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub(crate) struct Control {
    pub id: String,
    pub you: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub(crate) struct Relay {
    pub body: serde_json::Value,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub(crate) struct Ids {
    pub ids: Vec<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub(crate) struct Added {
    pub session: SessionInfo,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub(crate) struct PeersMsg {
    pub peers: Peers,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buf = vec![];
        buf.extend(frame(LIST, b""));
        buf.extend(data(OUTPUT, "abc", b"\x1b[31mhi"));
        buf.extend(json(KILL, &Id { id: "x".into() }));
        let mut r = buf.as_slice();
        assert_eq!(read_frame(&mut r).unwrap(), (LIST, vec![]));
        let (kind, body) = read_frame(&mut r).unwrap();
        assert_eq!(kind, OUTPUT);
        assert_eq!(split_data(&body), Some(("abc", &b"\x1b[31mhi"[..])));
        let (kind, body) = read_frame(&mut r).unwrap();
        assert_eq!(kind, KILL);
        assert_eq!(serde_json::from_slice::<Id>(&body).unwrap().id, "x");
        assert!(read_frame(&mut r).is_err(), "nothing left");
    }

    #[test]
    fn a_zero_or_huge_length_is_refused() {
        assert!(read_frame(&mut &[0u8, 0, 0, 0][..]).is_err());
        assert!(read_frame(&mut &[0xff, 0xff, 0xff, 0xff, 1][..]).is_err());
    }
}

//! OSC introspection helpers (optional)
//!
//! These helpers implement *read-only* OSC endpoints that let external controllers discover:
//! - available parameter names
//! - parameter metadata (cur/target/min/max/smooth)
//! - MIDI mapping patterns
//!
//! The intent is to make ShadeCore play nicely with TouchOSC / Max / custom controllers where you
//! want to build UI dynamically rather than hard-coding parameter lists.
//!
//! ## Expected OSC namespace
//! The code assumes a prefix like `/shadecore` (configurable in your OSC runtime).
//!
//! Queries:
//! - `/shadecore/list/params`
//! - `/shadecore/get/<param>`
//! - `/shadecore/list/mappings`
//!
//! Replies:
//! - `/shadecore/reply/list/params`   (string args: param names)
//! - `/shadecore/reply/get/<param>`   (float args: cur, tgt, min, max, smooth) OR ("unknown_param")
//! - `/shadecore/reply/list/mappings` (string args: patterns)
//!
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::net::UdpSocket;

use rosc::{OscMessage, OscPacket, OscType};

use crate::ParamStore;
use crate::logi;

fn osc_send_reply(sock: &UdpSocket, to: SocketAddr, addr: String, args: Vec<OscType>) {
    let msg = OscMessage { addr, args };
    let pkt = OscPacket::Message(msg);
    match rosc::encoder::encode(&pkt) {
        Ok(buf) => { let _ = sock.send_to(&buf, to); }
        Err(e) => { logi!("OSC", "encode error: {e}");}
    }
}

/// Returns true if the message was handled as introspection (and therefore should not be treated as a param update).
pub fn osc_try_introspect(
    prefix: &str,
    addr: &str,
    store: &Arc<Mutex<ParamStore>>,
    sock: &UdpSocket,
    to: SocketAddr,
) -> bool {
    // /prefix/list/params  (or /prefix/list)
    if addr == format!("{}/list/params", prefix) || addr == format!("{}/list", prefix) {
        if let Ok(s) = store.lock() {
            let mut names: Vec<String> = s.values.keys().cloned().collect();
            names.sort();
            let args = names.into_iter().map(OscType::String).collect::<Vec<_>>();
            osc_send_reply(sock, to, format!("{}/reply/list/params", prefix), args);
            logi!("OSC", "introspect list/params -> {} items", s.values.len());}
        return true;
    }

    // /prefix/get/<param>
    if let Some(name) = addr.strip_prefix(&format!("{}/get/", prefix)) {
        if let Ok(s) = store.lock() {
            let cur = s.values.get(name).copied();
            let tgt = s.targets.get(name).copied();
            let rng = s.ranges.get(name).copied();
            let sm  = s.smooth.get(name).copied();
            if let (Some(cur), Some(tgt), Some((mn, mx)), Some(sm)) = (cur, tgt, rng, sm) {
                osc_send_reply(
                    sock,
                    to,
                    format!("{}/reply/get/{}", prefix, name),
                    vec![
                        OscType::Float(cur),
                        OscType::Float(tgt),
                        OscType::Float(mn),
                        OscType::Float(mx),
                        OscType::Float(sm),
                    ],
                );
                logi!("OSC", "introspect get/{name} cur={cur} tgt={tgt} range=({mn},{mx}) smooth={sm}");} else {
                osc_send_reply(
                    sock,
                    to,
                    format!("{}/reply/get/{}", prefix, name),
                    vec![OscType::String("unknown_param".into())],
                );
                logi!("OSC", "introspect get/{name} -> unknown_param");}
        }
        return true;
    }

    // /prefix/list/mappings  (or /prefix/mappings)
    if addr == format!("{}/list/mappings", prefix) || addr == format!("{}/mappings", prefix) {
        let args = vec![
            OscType::String(format!("prefix={}", prefix)),
            OscType::String(format!("{}/param/<name> (normalized 0..1)", prefix)),
            OscType::String(format!("{}/raw/<name> (raw value)", prefix)),
            OscType::String(format!("{}/list/params", prefix)),
            OscType::String(format!("{}/get/<name>", prefix)),
        ];
        osc_send_reply(sock, to, format!("{}/reply/list/mappings", prefix), args);
        logi!("OSC", "introspect list/mappings");return true;
    }

    false
}
// ShadeCore OSC introspection helpers (drop-in)
//
// Add these helpers near your OSC handler, and update your OSC receive loop to
// keep the sender address (from) and pass `&sock` + `from` into handle_packet.
//
// Supported query messages (assumes prefix "/shadecore"):
//   /shadecore/list/params
//   /shadecore/get/<param>
//   /shadecore/list/mappings
//
// Replies are sent back to the sender as OSC messages:
//   /shadecore/reply/list/params   (string args: param names)
//   /shadecore/reply/get/<param>   (float args: cur, tgt, min, max, smooth) OR ("unknown_param")
//   /shadecore/reply/list/mappings (string args: patterns)
//
// NOTE: This is intentionally "modular" and does not assume anything about your
// OSC mappings schema. It always exposes the direct routes:
//   /prefix/param/<name> (normalized 0..1)
//   /prefix/raw/<name>   (raw)
// and your introspection endpoints.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::net::UdpSocket;

use rosc::{OscMessage, OscPacket, OscType};

use crate::ParamStore;

fn osc_send_reply(sock: &UdpSocket, to: SocketAddr, addr: String, args: Vec<OscType>) {
    let msg = OscMessage { addr, args };
    let pkt = OscPacket::Message(msg);
    match rosc::encoder::encode(&pkt) {
        Ok(buf) => { let _ = sock.send_to(&buf, to); }
        Err(e) => { println!("[osc] encode error: {e}"); }
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
            println!("[osc] introspect list/params -> {} items", s.values.len());
        }
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
                println!("[osc] introspect get/{name} cur={cur} tgt={tgt} range=({mn},{mx}) smooth={sm}");
            } else {
                osc_send_reply(
                    sock,
                    to,
                    format!("{}/reply/get/{}", prefix, name),
                    vec![OscType::String("unknown_param".into())],
                );
                println!("[osc] introspect get/{name} -> unknown_param");
            }
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
        println!("[osc] introspect list/mappings");
        return true;
    }

    false
}

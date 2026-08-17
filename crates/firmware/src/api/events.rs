//! `GET /api/v1/events` — the WebSocket the UI's live positions arrive on.
//!
//! # The failure this is designed against
//!
//! A device of this kind held one WebSocket per open browser tab against a cap
//! of five, and rejected the sixth by accepting the TCP connection and dropping
//! it with no status and no log line. A polling integration with a stale
//! address exhausted the cap on its own, and the operator could not reach the
//! settings that would have fixed it. `docs/provenance.md` records where that
//! was read.
//!
//! Two things went wrong there and only one of them was the cap. Ours differs
//! on both:
//!
//! 1. **A slot is refused out loud.** [`WS_MAX`] sockets may be upgraded at
//!    once and the next gets `503` with a `Retry-After`, which is a thing a
//!    client can act on rather than a socket that shuts without speaking.
//! 2. **A refused WebSocket cannot cost the operator the REST API.** The task
//!    pool is [`crate::api::HTTP_TASKS`] and `WS_MAX` is smaller than it, so
//!    tasks are always left over to answer requests. That is a property of the
//!    two numbers rather than of anyone's care, and
//!    [`crate::api::REST_TASKS_RESERVED`] is where it is checked.
//!
//! There is a third difference that costs nothing: our UI shows the connection
//! state as a status pill and never blocks on it, so a device that refuses
//! every WebSocket is still fully operable. The reference's UI covered its own
//! settings screen with a full-page overlay whenever the socket was down, which
//! is what turned a connection limit into a lockout.
//!
//! # Why the subscription *is* the permit
//!
//! The delta channel is a `PubSubChannel` with a fixed number of subscriber
//! slots, and `subscriber()` refuses once they are gone. So the admission check
//! and the resource are the same object: there is no counter to keep in step
//! with reality, and no path that takes one without the other. It is released
//! by `Drop`, which runs whether this returns normally, returns early through
//! `?`, or has its future dropped when the socket dies — the three ways a
//! hand-written release is missed.
//!
//! # A dead tab does not hold a slot forever
//!
//! `picoserve` sets TCP keep-alive at 30 s and a socket timeout at 45 s on
//! every connection it accepts, so a browser that vanished without closing — a
//! laptop lid, a phone asleep, a Wi-Fi drop — has its socket torn down and its
//! subscription dropped inside a minute. Nothing here had to be written for
//! that, and it is worth naming because the reference needed its own 20 s ping
//! and 10 s pong timer to get the same effect.
//!
//! # A slow client cannot make the state task wait
//!
//! Deltas arrive through a subscription that the publisher never blocks on:
//! the state task calls `publish_immediate`, which drops for a subscriber that
//! has fallen behind rather than parking. So a browser that stops reading
//! costs itself a gap in its own position updates — which the next delta
//! corrects — and costs the radio nothing. That is the same trade the broker
//! session already makes, and it is why this reads deltas rather than being
//! handed them.

use picoserve::io::{Read, Write};
use picoserve::response::ws::{Message, SocketRx, SocketTx, WebSocketCallback};
use somfy_api::{ShadeStateEvent, WsEvent};

use crate::rpc::{Reply, Request, RPC};
use crate::tasks::DeltaSubscriber;

/// WebSockets this device will hold at once.
///
/// Two, which is the number of screens a household actually has open — a phone
/// and a laptop — and is deliberately **smaller than the task pool**, because
/// the whole point is that WebSockets cannot consume the capacity REST needs.
/// A third tab is refused with a status rather than by being made to feel like
/// a broken network; it still lists shades, still commands them, and still
/// shows positions on every page load, because everything except live updates
/// arrives over REST.
///
/// A policy figure. What bounds it from above is the task pool; what bounds it
/// from below is that one tab must work.
pub const WS_MAX: usize = 2;

/// Inbound WebSocket frame buffer.
///
/// The UI never sends a message — it is a one-way stream of state — so the only
/// inbound frames are the control frames the protocol requires: `Close`, whose
/// payload is a 2-byte code plus a reason, and `Ping`, whose payload RFC 6455
/// caps at 125 bytes. 192 covers the largest of those with room for the frame
/// header, and nothing larger can be delivered by a conforming client.
const FRAME_BYTES: usize = 192;

/// One connected client.
///
/// Holds the subscription for the life of the socket, which is what makes the
/// permit and the resource the same thing — see this module's docs.
pub struct Events {
    deltas: DeltaSubscriber,
}

impl Events {
    /// Take a slot, or report that they are all taken.
    ///
    /// The caller turns `None` into `503`. It is fallible here rather than
    /// deeper because this is the last point at which there is still a response
    /// to send: after the upgrade there is no status code left to use.
    pub fn admit(deltas: DeltaSubscriber) -> Events {
        Events { deltas }
    }
}

impl WebSocketCallback for Events {
    async fn run<R: Read, W: Write<Error = R::Error>>(
        mut self,
        mut rx: SocketRx<R>,
        mut tx: SocketTx<W>,
    ) -> Result<(), W::Error> {
        // **The whole table first, then deltas.** A client that has just
        // connected knows nothing, and after a *reconnect* it knows something
        // worse than nothing: whatever the positions were when the socket
        // dropped. The UI does not re-fetch on reconnect, so without this a
        // shade that moved during the outage would show its old position until
        // it moved again.
        // A snapshot cut short by the state task not answering is reported and
        // not retried: the client has the same information from
        // `GET /api/v1/shades`, and a socket that spins on a fault is worse
        // than one that starts a little behind.
        if send_snapshot(&mut tx).await? {
            esp_println::println!(
                "api: a websocket opened without its opening snapshot — the client's positions \
                 will catch up on the next movement"
            );
        }

        let mut frame = [0u8; FRAME_BYTES];
        loop {
            // One `select` over both directions. The delta subscription is the
            // signal, which is what lets an inbound `Close` be answered
            // promptly instead of after the next state change.
            match rx
                .next_message(&mut frame, self.deltas.next_message())
                .await
            {
                // A client that speaks. Ours does not, so this handles the
                // protocol's own traffic and nothing else.
                Ok(picoserve::futures::Either::First(Ok(message))) => match message {
                    Message::Ping(payload) => tx.send_pong(payload).await?,
                    Message::Close(_) => break,
                    // Text, binary and unsolicited pongs are ignored rather
                    // than refused: this endpoint has no inbound vocabulary, so
                    // there is nothing a message could mean. Commands travel
                    // over REST, where a rejection has somewhere to go.
                    Message::Text(_) | Message::Binary(_) | Message::Pong(_) => {}
                },
                // A frame this device could not read. Closed with the
                // protocol's own code rather than dropped silently, because a
                // client that is speaking a language we do not understand
                // should be told which of us stopped.
                Ok(picoserve::futures::Either::First(Err(_))) => {
                    return tx.close(Some((1002, "unreadable frame"))).await;
                }
                Ok(picoserve::futures::Either::Second(delta)) => {
                    match delta {
                        embassy_sync::pubsub::WaitResult::Message(delta) => {
                            tx.send_json(&WsEvent::ShadeState(ShadeStateEvent::from(&delta)))
                                .await?;
                        }
                        // This client fell behind and the channel dropped
                        // messages for it. Not an error and not worth closing
                        // over: a delta is a report about a position that the
                        // next delta reports again, so the client is at most
                        // one movement stale and self-corrects.
                        embassy_sync::pubsub::WaitResult::Lagged(missed) => {
                            esp_println::println!(
                                "api: a websocket client missed {} state update(s)",
                                missed,
                            );
                        }
                    }
                }
                // The socket itself failed. There is nothing to close with.
                Err(_) => return Ok(()),
            }
        }

        tx.close(None).await
    }
}

/// Send the current state of every shade, one message each.
///
/// Walked over the RPC seam a shade at a time for the reason
/// [`crate::rpc`] gives: a buffer holding all thirty-two would be paid for
/// in Wi-Fi driver headroom on every boot, including the ones where nobody
/// opens a browser.
///
/// Returns whether the walk was cut short by the state task not answering,
/// which is reported rather than retried — the client has the same information
/// from `GET /api/v1/shades`, and a WebSocket that spins on a fault is worse
/// than one that starts a little behind.
async fn send_snapshot<W: Write>(tx: &mut SocketTx<W>) -> Result<bool, W::Error> {
    let mut slot = 0u8;
    loop {
        let Some(Reply::Shade(found)) = RPC.call(Request::ShadeFrom(slot)).await else {
            return Ok(true);
        };
        let Some(shade) = found else { return Ok(false) };
        tx.send_json(&WsEvent::ShadeState(ShadeStateEvent {
            id: shade.id,
            position: shade.position,
            tilt_position: shade.tilt_position,
            direction: shade.direction,
        }))
        .await?;
        // Past this one, so a full registry terminates rather than repeating
        // its last slot forever.
        let Some(next) = shade.id.checked_add(1) else {
            return Ok(false);
        };
        slot = next;
    }
}
